// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Host API's dispatcher (Mods spec §1.5, §1.8, §1.9): the one core every mod call goes
//! through, whoever makes it. Today the caller is a mod's sealed frame, relayed by PM's page through
//! `mod_call`; the read-only MCP server (PR C) becomes a second [`Caller`] with no window.
//!
//! **The check order** ([`dispatch`]), each step refusing with a stable `MOD-####` code:
//! 1. the main window only (a frame's call came from PM's page in `main`; an MCP call has no
//!    window and skips this);
//! 2. the store is open and PM isn't locked (`settings::app_locked_for_mods`), read AFTER step 3's
//!    ticket is taken: enabling the lock writes its flag and then moves every session to a new
//!    generation, so a lock enabled after the ticket is caught by the generation, and one enabled
//!    before it by this read; read the other way round, one enabled between the two slips past
//!    both. (A stale session is therefore told "session ended" before "locked": only the lock's own
//!    sessions learn it is the lock.)
//! 3. the session is live, at the generation the call began under, over the store it was opened in
//!    (`runtime::CallTicket`);
//! 4. the mod runs: its row is active and turned on (paused, parked, installing, blacklisted and an
//!    unaccepted auto-install all refuse), and the method exists in this API (the table);
//! 5. the grant ([`grants`], D3's exclusions and the sealed-engine rule included);
//! 6. the rate bucket and the concurrency cap ([`limits`]), then the call's family's own (the
//!    experimental AI and network calls: tighter, because they hold their admission while they wait
//!    on something outside PM);
//! 7. the handler, with the arguments' size cap first.
//!
//! **Never past an await on stale facts.** Steps 2-5 run in one blocking task, each DB read a short
//! `conn()` taken and dropped in turn (the DB mutex isn't re-entrant, I-14). After every `.await`
//! the call is asked again whether it is current ([`runtime::ModRuntime::still_current`]); and a
//! handler that touches data does so only after [`recheck`] under the very DB guard it uses, which
//! re-runs steps 3-5 there, and asks once more immediately before it commits. So a call in flight
//! across an uninstall, a pause, a lock, a curtain or a vault switch commits nothing.
//!
//! **What a handler answers** is a [`Reply`]: a value; a page action, when PM's page holds what
//! the answer needs (the live theme, the document reader), which the dispatcher has already
//! authorised; bytes, for the vectors; or an error. The page never performs anything Rust didn't authorise, and never one
//! a frame names: the action comes from here.
//!
//! **Events** go the other way ([`events`]): Rust asks the same grant questions per session, builds
//! the payload from the store (never PM's own event), and sends only into the outbox of a session
//! that may hear it (`mods::outbox`), bounded by the frame's acknowledgements.

mod doc_storage;
mod documents;
mod embeddings;
pub mod error;
pub mod events;
pub mod grants;
mod handlers;
pub mod limits;
mod net;
pub mod permissions;
mod storage;
pub mod table;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;

use std::io;
use std::time::Instant;

use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;
use tauri::ipc::InvokeResponseBody;
use tauri::{AppHandle, Manager};

pub use error::{Code, ModError};
pub use limits::Calls;

use self::grants::ModFacts;
use self::table::{Lookup, Row};
use super::netfetch::{HopFetcher, SystemHops};
use super::policy::MOD_API_VERSION;
use super::runtime::{CallTicket, ModRuntime};
use super::{EngineState, MAIN_LABEL};
use crate::AppState;

/// Who is calling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// A mod's sealed frame: the window PM's page relayed the call from, and the session PM opened
    /// for that frame. The mod is the session's, never anything the frame said.
    Frame { window: String, session: String },
}

/// The events PM posts to frames, each with its row in the table (`events.rs`).
#[cfg_attr(not(test), allow(unused_imports))]
pub use events::EVENTS;

/// What the dispatcher reaches: the store, the frame sessions, the call limits, the events waiting
/// for their interval, the engine's seal and the app lock. The app is the host; a test builds its own.
pub trait Host: Clone + Send + Sync + 'static {
    fn store(&self) -> &AppState;
    fn runtime(&self) -> &ModRuntime;
    fn calls(&self) -> &Calls;
    fn hub(&self) -> &events::Hub;
    fn engine_sealed(&self) -> bool;
    /// Whether PM is locked now. An error means the store is closed, which refuses the same way.
    fn app_locked(&self) -> crate::error::Result<bool>;

    /// The network edge `net.fetch` goes through: the real one resolves, screens and pins; a test
    /// scripts it (`netfetch::fake`), so the fetch's rules run with no network.
    type Hops: HopFetcher + 'static;
    // Nothing in a build calls it until the `net.fetch` handler lands (X6).
    #[allow(dead_code)]
    fn hops(&self) -> &Self::Hops;

    /// A test's hook into a call in flight, so it can change the world at an exact point (an
    /// uninstall between the checks and the commit). Nothing in a build.
    #[cfg(test)]
    fn at(&self, _point: Point) {}
}

/// Where a call in flight stops for a test's hook.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Point {
    /// Steps 1-5 passed; the blocking task has returned.
    Authorised,
    /// In a handler's blocking task, before it takes the DB guard.
    BeforeData,
    /// In a handler's transaction, its write made, just before the commit. The DB guard is held:
    /// a hook here must not take it.
    BeforeCommit,
    /// An event's recipients are chosen and its payload built; nothing is delivered yet. The DB guard
    /// is released.
    BeforeDelivery,
    /// The app lock has just been read (its answer is on its way back to the caller).
    LockRead,
    /// An event is about to choose its recipients: the moment between a flush's own checks and the
    /// emission.
    BeforeEmit,
}

impl Host for AppHandle {
    fn store(&self) -> &AppState {
        self.state::<AppState>().inner()
    }

    fn runtime(&self) -> &ModRuntime {
        self.state::<ModRuntime>().inner()
    }

    fn calls(&self) -> &Calls {
        self.state::<Calls>().inner()
    }

    fn hub(&self) -> &events::Hub {
        self.state::<events::Hub>().inner()
    }

    /// Unsealed until the engine has said otherwise (`EngineState`), including before it is managed.
    fn engine_sealed(&self) -> bool {
        self.try_state::<EngineState>()
            .is_some_and(|state| state.engine_sealed())
    }

    fn app_locked(&self) -> crate::error::Result<bool> {
        crate::settings::app_locked_for_mods(self.store())
    }

    type Hops = SystemHops;

    fn hops(&self) -> &SystemHops {
        static HOPS: SystemHops = SystemHops::new();
        &HOPS
    }
}

/// A page action: an authorised call PM's page answers, from what only it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PageAction {
    /// PM's live theme: its CSS custom properties and its axes (`modTheme.ts`).
    #[serde(rename = "theme.get")]
    ThemeGet,
    /// PM's own document reader, opened on a document Rust found readable by the caller
    /// (`ModFrame.tsx`, through the app's `ReaderProvider`).
    #[serde(rename = "ui.openDocument")]
    UiOpenDocument,
}

impl PageAction {
    #[cfg_attr(not(test), allow(dead_code))]
    pub const ALL: [PageAction; 2] = [Self::ThemeGet, Self::UiOpenDocument];

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn name(self) -> &'static str {
        match self {
            Self::ThemeGet => "theme.get",
            Self::UiOpenDocument => "ui.openDocument",
        }
    }
}

/// The answer to one call, as `mod_call` returns it to PM's page.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Reply {
    /// The call's result, for the frame.
    Value { value: Value },
    /// Authorised: PM's page performs `action` with `args` (which Rust checked) and answers.
    Page { action: PageAction, args: Value },
    /// Refused, or failed: `{code, message}` for the frame.
    Error { error: ModError },
    /// A binary answer (`embeddings.documentVectors`, layout in `embeddings.rs`). It never goes
    /// through JSON: [`Reply::into_body`] hands it to the webview as a raw IPC body, which the page
    /// receives as an `ArrayBuffer` and transfers to the frame.
    #[serde(skip)]
    Binary { bytes: Vec<u8> },
}

impl Reply {
    pub fn value(value: Value) -> Self {
        Self::Value { value }
    }

    /// The reply as the IPC body `mod_call` returns: JSON for everything but [`Reply::Binary`], whose
    /// bytes go out raw.
    pub fn into_body(self) -> InvokeResponseBody {
        match self {
            Self::Binary { bytes } => InvokeResponseBody::Raw(bytes),
            other => InvokeResponseBody::Json(serde_json::to_string(&other).unwrap_or_else(|e| {
                // Every other variant is plain data, so this can't fail; if it ever does the page
                // is told PM failed, never handed nothing.
                let error = ModError::internal(e);
                serde_json::to_string(&Self::Error { error }).unwrap_or_default()
            })),
        }
    }

    /// The reply held to its cap: [`limits::MAX_RESULT_BYTES`] of JSON, or
    /// [`limits::MAX_BINARY_RESULT_BYTES`] of bytes.
    fn capped(self) -> Self {
        let too_large = match &self {
            Self::Value { value } | Self::Page { args: value, .. } => {
                !fits_json(value, limits::MAX_RESULT_BYTES)
            }
            Self::Binary { bytes } => bytes.len() > limits::MAX_BINARY_RESULT_BYTES,
            Self::Error { .. } => false,
        };
        if too_large {
            Self::Error {
                error: ModError::new(Code::TooLarge, "The answer is too large to send."),
            }
        } else {
            self
        }
    }
}

/// Whether `value` serialises to at most `cap` bytes, found by counting without building the text
/// (and stopping at the cap).
fn fits_json(value: &Value, cap: usize) -> bool {
    struct Count {
        n: usize,
        cap: usize,
    }
    impl io::Write for Count {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.n += buf.len();
            if self.n > self.cap {
                return Err(io::Error::other("past the cap"));
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(&mut Count { n: 0, cap }, value).is_ok()
}

/// A call that passed steps 1-5: its ticket and its table row. What the mod is (its grants, its
/// manifest) a handler reads again from [`recheck`], under the guard it touches data with, never
/// from what step 4 saw.
#[derive(Debug, Clone)]
pub struct Authorised {
    pub ticket: CallTicket,
    pub row: &'static Row,
}

/// Answer one call. Never fails: every refusal is a [`Reply::Error`] with its code.
pub async fn dispatch<H: Host>(host: &H, caller: Caller, method: &str, args: &str) -> Reply {
    dispatch_in(host, table::ROWS, caller, method, args).await
}

/// [`dispatch`] against `rows`: this PM's table, or a test's that gives a real handler another
/// permission.
async fn dispatch_in<H: Host>(
    host: &H,
    rows: &'static [Row],
    caller: Caller,
    method: &str,
    args: &str,
) -> Reply {
    match call(host, rows, caller, method, args).await {
        Ok(reply) => reply.capped(),
        Err(error) => Reply::Error { error },
    }
}

async fn call<H: Host>(
    host: &H,
    rows: &'static [Row],
    caller: Caller,
    method: &str,
    args: &str,
) -> Result<Reply, ModError> {
    let Caller::Frame { window, session } = caller;
    // 1. The main window only.
    if window != MAIN_LABEL {
        return Err(ModError::new(
            Code::NotGranted,
            "Mods run only in PM's main window.",
        ));
    }
    // 2-5, off the async runtime.
    let auth = {
        let host = host.clone();
        let method = method.to_owned();
        blocking(move || authorise(&host, rows, &session, &method)).await?
    };
    #[cfg(test)]
    host.at(Point::Authorised);
    still_current(host, &auth)?;
    // 6. The rate bucket and the concurrency cap. The admission counts the call as in flight until
    // it returns.
    let _admission = host.calls().admit(&auth.ticket.mod_id, Instant::now())?;
    // ...and, for a call that waits on a model or a website, its family's own (the row says which),
    // held for as long as the call runs. A general call gets an admission that counts nothing.
    let _family =
        host.calls()
            .admit_family(&auth.ticket.mod_id, auth.row.family, Instant::now())?;
    // 7. The handler.
    if args.len() > limits::MAX_ARGS_BYTES {
        return Err(ModError::new(
            Code::TooLarge,
            "A call's arguments are too large.",
        ));
    }
    let args: Value = serde_json::from_str(args)
        .map_err(|_| ModError::bad_args("A call's arguments must be JSON."))?;
    handlers::run(host, &auth, &args).await
}

/// Steps 2-5, in one blocking task.
fn authorise<H: Host>(
    host: &H,
    rows: &'static [Row],
    session: &str,
    method: &str,
) -> Result<Authorised, ModError> {
    // 3. The session, at its generation: first, so the lock check below can't be overtaken by a lock
    // enabled between the two (the generation moves after the lock's flag is written).
    let ticket = host
        .runtime()
        .begin_call(session)
        .map_err(|_| ModError::session_ended())?;
    // 2. The store is open and PM isn't locked (this takes and drops its own `conn()`).
    if !matches!(host.app_locked(), Ok(false)) {
        return Err(ModError::locked());
    }
    let conn = host.store().conn().map_err(|_| ModError::locked())?;
    if host.store().store_epoch() != ticket.store_epoch {
        return Err(ModError::session_ended());
    }
    // 4. The mod runs, and the method exists.
    let facts = running_mod(&conn, &ticket)?;
    let row = method_row(table::lookup_call(rows, method, MOD_API_VERSION))?;
    // 5. The grant.
    if let Some(permission) = row.permission {
        grants::check(&conn, &facts, permission, host.engine_sealed())?;
    }
    Ok(Authorised { ticket, row })
}

/// The row a looked-up name gives, or its refusal.
fn method_row(lookup: Lookup) -> Result<&'static Row, ModError> {
    match lookup {
        Lookup::Found(row) => Ok(row),
        Lookup::Removed(_) => Err(ModError::new(
            Code::RemovedMethod,
            "This PM no longer has that call.",
        )),
        Lookup::Unknown => Err(ModError::new(
            Code::UnknownMethod,
            "PM has no call by that name.",
        )),
    }
}

/// The ticket's mod, read from its row, if it still runs. No row means it was uninstalled.
fn running_mod(conn: &Connection, ticket: &CallTicket) -> Result<ModFacts, ModError> {
    let facts = grants::read_mod(conn, &ticket.mod_id)
        .map_err(ModError::internal)?
        .ok_or_else(ModError::session_ended)?;
    facts.running()?;
    Ok(facts)
}

/// Whether the call may go on: its session is still at its generation (the runtime's answer). Asked
/// after every `.await`.
pub(crate) fn still_current<H: Host>(host: &H, auth: &Authorised) -> Result<(), ModError> {
    host.runtime()
        .still_current(&auth.ticket)
        .map_err(|_| ModError::session_ended())
}

/// Before a handler reads or writes a mod's data, under the DB guard (or the transaction) it is
/// about to use: steps 3-5 again, so nothing that changed since [`authorise`] is missed. The store
/// epoch is read under that guard, so it is the epoch of the store the handler will touch. Returns
/// the mod as it is now. A handler that commits calls [`still_current`] once more right before its
/// commit.
pub(crate) fn recheck<H: Host>(
    host: &H,
    conn: &Connection,
    auth: &Authorised,
) -> Result<ModFacts, ModError> {
    still_current(host, auth)?;
    if host.store().store_epoch() != auth.ticket.store_epoch {
        return Err(ModError::session_ended());
    }
    let facts = running_mod(conn, &auth.ticket)?;
    if let Some(permission) = auth.row.permission {
        grants::check(conn, &facts, permission, host.engine_sealed())?;
    }
    Ok(facts)
}

/// Run blocking work (DB, files) on the blocking pool. A panic there is PM's failure.
pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, ModError> + Send + 'static,
) -> Result<T, ModError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(ModError::internal)?
}
