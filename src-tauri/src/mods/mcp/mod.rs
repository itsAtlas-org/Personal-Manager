// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The read-only MCP server (Mods spec 1.16, PR C3): everything that decides what a connected app may
//! ask and be told, and the local socket that carries it. A connection ([`serve`]) feeds
//! [`jsonrpc::handle`] one line at a time, bound to the app that authenticated and the store it
//! authenticated against ([`Bound`]).
//!
//! - [`transport`]: the Unix socket, in a verified 0700 folder.
//! - [`serve`]: one connection's rules (first frame, caps, idle clock).
//! - [`supervisor`]: when the socket exists, and the hooks that close it.
//! - [`device`]: this computer's own record of whether the server is on for a vault and which apps it
//!   paired (a file, never the store: a restore or a shared vault can never switch it on).
//! - [`clients`]: minting, listing, revoking and verifying an app.
//! - [`tools`]: the five read tools and the scope each needs.
//! - [`limits`]: the per-app limiter and its words.
//! - [`jsonrpc`]: the protocol's messages.
//!
//! **A request's checks** ([`dispatch`], [`list_tools`]), in order, each failing with a fixed
//! sentence and a stable `MOD-####` code, none of them trusting what an earlier request found.
//! Before them all, the app's general allowance admits the request ([`limits::McpLimits::admit_call`]):
//! checks 1 to 3 read the device's file and take the DB guard, so a request that is going to be
//! refused anyway (an unknown tool, a listing) must still cost the app a token, or an app could loop
//! on one and starve PM's own screens of the guard.
//! 1. PM is not locked and its store is open (`Host::app_locked`);
//! 2. the store is the one the connection was made against (`store_epoch`, read under the DB guard:
//!    a vault switch, a lock cycle or a restore moves it), and the open vault is this device's
//!    ([`Here::is_open_in`], asked after the guard is taken);
//! 3. the app still verifies: its row exists and this device still holds its pairing, with the
//!    scopes the pairing was made for, and the switch is on ([`clients::recheck`]);
//! 4. the tool exists and the app holds its scope (an unknown tool and a tool the app may not use are
//!    told the same way);
//! 5. the app's limiter admits the tool's own family ([`limits::McpLimits::in_family`]: one search at
//!    a time, a few a minute);
//! 6. the arguments are within their cap;
//! 7. the tool's core runs; for a read under the very guard that did checks 2 and 3, so what it reads
//!    is what those checks saw;
//! 8. the checks 1 to 3 once more, so an answer is never delivered for an app that was revoked, a
//!    switch turned off or a store closed while it was being made.
//!
//! These checks are the guarantee. The hooks (store close, lock, vault switch, revoke, quit) that
//! close live connections early are tidying: a test proves every refusal with the hooks switched off.
//!
//! **A second front for the same cores.** A mod's frame reaches the shared question cores through the
//! dispatcher's mod-shaped checks (a session ticket, the mod's row, its grants); an app reaches them
//! through these, over one [`Projection`](crate::mods::api::projection::Projection) built from its
//! scopes. The grant check therefore lives in two places (frame grants and client scopes), over one
//! projection and one core per call, and `Authorised`, `calls::read` and `ModFacts` are never reused
//! for an app.

// Built before its callers: the commands behind Settings (slice M1c) are what call the clients' and
// the device file's functions, and until they land only the restore reset (`device::all_off`) and the
// socket's connections (`serve`) do. Tests cover every function.
#![cfg_attr(not(test), allow(dead_code))]

use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rusqlite::Connection;
use serde_json::Value;

use crate::mods::api::limits::CALL_TIMEOUT;
use crate::mods::api::{blocking, search_calls, Caller, Code, Host, ModError, Reply};
use crate::mods::devmode::Here;

pub mod clients;
pub mod device;
pub mod jsonrpc;
pub mod limits;
pub mod serve;
pub mod supervisor;
pub mod tools;
pub mod transport;

#[cfg(test)]
mod clients_tests;
#[cfg(test)]
mod device_tests;
#[cfg(test)]
mod dispatch_tests;
#[cfg(test)]
mod jsonrpc_tests;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod serve_tests;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod transport_tests;

use clients::{LastUsed, VerifiedClient};

/// The scopes an app may be given: permission ids, frozen by API 1.0 like the tool names.
/// `calendar.details` is never offered.
pub const SCOPES: [&str; 4] = [
    "documents.text",
    "tasks.read",
    "calendar.read",
    "projects.read",
];

/// The most one request's arguments may weigh, in bytes of JSON (the frame's own cap).
pub const MAX_ARGS_BYTES: usize = 1 << 20;

/// Why minting, revoking or switching did not happen. What it SAYS (`Display`) is PM's own fixed
/// sentence, safe to show the person: a database or disk error's text (which can carry SQL or a path)
/// stays in `Debug` and in the log line written where the error is first caught.
// The two errors are read through `Debug` only, which is the point.
#[allow(dead_code)]
#[derive(Debug)]
pub enum Fault {
    /// A request PM refuses, with the sentence to say.
    Refused(&'static str),
    Db(rusqlite::Error),
    Disk(io::Error),
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Refused(why) => why,
            Self::Db(_) => "PM couldn't read or save its connected apps. Try again.",
            Self::Disk(_) => "PM couldn't save that on this computer. Try again.",
        })
    }
}

impl std::error::Error for Fault {}

impl From<rusqlite::Error> for Fault {
    fn from(e: rusqlite::Error) -> Self {
        eprintln!("mcp: a database step failed: {e}");
        Self::Db(e)
    }
}

impl From<io::Error> for Fault {
    fn from(e: io::Error) -> Self {
        eprintln!("mcp: a disk step failed: {e}");
        Self::Disk(e)
    }
}

/// What the server keeps for the life of the process: where this computer's files are, the per-app
/// limiter, the clock of `last_used_at`, and the listener with its connections ([`supervisor`]).
#[derive(Debug)]
pub struct Supervisor {
    data_dir: PathBuf,
    limits: limits::McpLimits,
    last_used: LastUsed,
    life: supervisor::Lifecycle,
}

impl Supervisor {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            limits: limits::McpLimits::default(),
            last_used: LastUsed::default(),
            life: supervisor::Lifecycle::default(),
        }
    }

    /// The listener and its connections.
    pub fn life(&self) -> &supervisor::Lifecycle {
        &self.life
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn limits(&self) -> &limits::McpLimits {
        &self.limits
    }

    pub fn last_used(&self) -> &LastUsed {
        &self.last_used
    }
}

/// What a connection is bound to once it has authenticated: the app, and the store it was made
/// against. Every request checks both again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bound {
    pub client: i64,
    pub store_epoch: u64,
}

impl Bound {
    pub fn caller(self) -> Caller {
        Caller::Mcp {
            client: self.client,
            store_epoch: self.store_epoch,
        }
    }
}

// The sentences an app is told, each one fixed.

fn ended() -> ModError {
    ModError::new(
        Code::SessionEnded,
        "This connection has ended. Connect the app again.",
    )
}

fn not_connected() -> ModError {
    ModError::new(
        Code::NotGranted,
        "This app isn't connected to PM on this computer.",
    )
}

fn no_such_tool() -> ModError {
    ModError::new(Code::UnknownMethod, "PM has no tool by that name.")
}

fn too_large_args() -> ModError {
    ModError::new(Code::TooLarge, "The request is too large.")
}

fn too_large_answer() -> ModError {
    ModError::new(Code::TooLarge, "The answer is too large to send.")
}

/// This device in the vault open now, or the refusal for there being none.
fn here_of<H: Host>(host: &H) -> Result<Here, ModError> {
    let (vault, root) = host
        .store()
        .active_vault_place()
        .ok_or_else(ModError::locked)?;
    Here::open(host.mcp().data_dir(), &vault, &root).map_err(ModError::internal)
}

/// PM is open and not locked. Takes the DB guard itself (inside `Host::app_locked`), so it is asked
/// with none held.
fn open_and_unlocked<H: Host>(host: &H) -> Result<(), ModError> {
    if matches!(host.app_locked(), Ok(false)) {
        Ok(())
    } else {
        Err(ModError::locked())
    }
}

/// Checks 2 and 3, under the guard `conn` is: the store is the connection's, the open vault is
/// `here`'s, and the app still verifies. The app's scopes as they are now.
fn verify_here<H: Host>(
    host: &H,
    here: &Here,
    bound: Bound,
    conn: &Connection,
) -> Result<VerifiedClient, ModError> {
    // Read under the guard, so it is the epoch of the store `conn` reaches.
    if host.store().store_epoch() != bound.store_epoch {
        return Err(ended());
    }
    // Asked after the guard was taken (the open vault can't change while it is held).
    if !here.is_open_in(host.store()) {
        return Err(ended());
    }
    clients::recheck(conn, here, bound.client).ok_or_else(not_connected)
}

/// Checks 1 to 3 around `work`, which runs under the same DB guard as check 3: blocking.
fn guarded<H: Host, T>(
    host: &H,
    bound: Bound,
    work: impl FnOnce(&Connection, &VerifiedClient) -> Result<T, ModError>,
) -> Result<T, ModError> {
    open_and_unlocked(host)?;
    let here = here_of(host)?;
    let conn = host.store().conn().map_err(|_| ModError::locked())?;
    let client = verify_here(host, &here, bound, &conn)?;
    work(&conn, &client)
}

/// The tools `bound`'s app may use now (`tools/list`), each as the protocol describes it, its
/// argument schema made for the scopes the app holds now.
pub async fn list_tools<H: Host>(host: &H, bound: Bound) -> Result<Vec<Value>, ModError> {
    // Admitted like any request, before the checks that read the store and the device's file.
    let _held = host.mcp().limits().admit_call(bound.client)?;
    let host = host.clone();
    blocking(move || guarded(&host, bound, |_conn, client| Ok(tools::descriptors(client)))).await
}

/// Answer one tool call (`tools/call`): the ordered checks of the module doc, then the tool. The
/// answer is the tool's result as a value, held to [`tools::RESULT_BUDGET`].
pub(crate) async fn dispatch<H: Host>(
    host: &H,
    client: i64,
    store_epoch: u64,
    tool: &str,
    args: &str,
) -> Result<Reply, ModError> {
    let bound = Bound {
        client,
        store_epoch,
    };
    // Before the checks: the general allowance, held until the answer is made.
    let held = host.mcp().limits().admit_call(client)?;
    // 1-3. The store is open and unlocked, the connection's, and the app still verifies.
    let verified = {
        let host = host.clone();
        blocking(move || {
            guarded(&host, bound, |conn, client| {
                // Stamped on a request that got this far, once a minute at most; failure is ignored.
                host.mcp()
                    .last_used()
                    .touch(conn, client.id, Instant::now());
                Ok(client.clone())
            })
        })
        .await?
    };
    // 4. The tool exists and the app holds its scope: both read the same way to the app.
    let tool = tools::find(tool)
        .filter(|t| verified.holds(t.scope))
        .ok_or_else(no_such_tool)?;
    // 5. The tool's own family, held with the rest until the answer is made.
    let _held = host.mcp().limits().in_family(held, client, tool.family)?;
    // 6. The arguments.
    if args.len() > MAX_ARGS_BYTES {
        return Err(too_large_args());
    }
    let args = tools::args_object(args)?;
    // 7. The tool.
    let value = if tool.name == "search" {
        search(host, bound, &args).await?
    } else {
        let host = host.clone();
        blocking(move || {
            guarded(&host, bound, |conn, client| {
                tools::run_read(tool, conn, &tools::projection_of(client), &args)
            })
        })
        .await?
    };
    // 8. Still allowed, now that the answer exists.
    {
        let host = host.clone();
        blocking(move || guarded(&host, bound, |_, _| Ok(()))).await?;
    }
    if !tools::fits(&value) {
        return Err(too_large_answer());
    }
    Ok(Reply::value(value))
}

/// The `search` tool: the same core as `documents.search`, under the call timeout, with the app's
/// checks as its `check` step (each of its three DB steps asks them again under its own guard).
async fn search<H: Host>(host: &H, bound: Bound, args: &Value) -> Result<Value, ModError> {
    let query = search_calls::Query::parse(args)?;
    let host = host.clone();
    let work = blocking(move || {
        // The app lock is asked with no guard held (it takes its own); the checks inside the
        // core's guards cover everything else, and step 8 asks the lock once more.
        open_and_unlocked(&host)?;
        let here = here_of(&host)?;
        search_calls::answer(&host, &query, |conn| {
            let client = verify_here(&host, &here, bound, conn)?;
            if !client.holds("documents.text") {
                return Err(no_such_tool());
            }
            Ok(tools::projection_of(&client))
        })
    });
    tokio::time::timeout(CALL_TIMEOUT, work)
        .await
        .unwrap_or_else(|_elapsed| Err(ModError::call_timed_out()))
}
