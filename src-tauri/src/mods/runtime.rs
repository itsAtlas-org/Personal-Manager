// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The `pmmod` scheme's state and its serve decision (Mods spec §1.4): which mods are installed and
//! exactly which of their files may be served, the one-time entry URLs ModFrame loads, and the frame
//! sessions the bridge calls under.
//!
//! **Entry tickets.** `mod_frame_open` mints a random single-use token bound to one mod's entry page
//! and a new session. The frame loads `<mod origin>/<id>/<entry page>?entry=<token>`. On WebKit the
//! navigation guard asks [`ModRuntime::is_pending_entry`] (which never consumes) before the frame
//! may navigate there; the handler then consumes the token on the first request that carries it,
//! and only that response is served as a document. Every other HTML, XHTML, XML or SVG response is
//! an attachment (headers.rs), so a mod's frame can't load a second page, another mod's page, or
//! its own page again: on WebView2, where the guard never sees frames, the token is the whole wall.
//! On macOS the attachment is no wall at all: wry's WKWebView response delegate shows any type
//! WebKit can display and never reads `Content-Disposition` (read from wry 0.57.1, not measured), so
//! there the guard alone keeps a non-entry page out. It can: a `pmmod` URL always parses (its host is
//! opaque), so the guard never fails open for one.
//!
//! **Races, and how each ends:**
//! - A token used twice: the first request takes it; the second gets an attachment.
//! - A token for mod A presented under mod B's path (or for another file): the token is burnt, A's
//!   session is closed, and B's file goes out as it would without a token.
//! - Two frames of one mod: each has its own token and session; whichever frame loads a URL takes
//!   that URL's session, and both are the same mod with the same grants.
//! - A session used after close, or a token whose session closed: closing drops the session and
//!   its unused token together, under one lock.
//! - A token or pending session older than [`ENTRY_TTL`]: expired, and swept on every open.
//! - An uninstall or reinstall: every ticket and session of that mod goes with it.
//! - Main's page loading again (a reload): every ticket and session goes ([`ModRuntime::close_all`]),
//!   since the page that held them is gone and its ModFrames never ran their cleanup.
//!
//! **A call is current** (the dispatcher's question, `mods::api`) while its session is still open,
//! still live, and still at the generation the call began under ([`CallTicket`]). Every event that
//! must stop a call already in flight ends or bumps it: the app lock bumps every session
//! ([`ModRuntime::bump_all`]); a pause ([`ModRuntime::pause`]), an uninstall or a reinstall ends that
//! mod's; a closed or curtained store ends them all ([`ModRuntime::clear`]); a reload ends
//! them all. A session is also bound to the store that was open when it was minted (`AppState`'s
//! store epoch), so a frame opened over one store never reads or writes another.
//!
//! **Events** (`outbox`, `api::events`). A session owns the outbox that carries its frame's events,
//! the Tauri channel `mod_frame_open` was given. Whatever ends a session, its outbox is closed
//! with it, after a goodbye when the frame is live and the session didn't end by the frame's own
//! doing ([`Ending`]); the goodbyes are sent only once the registry's lock is released, so a send
//! never runs under it. [`ModRuntime::live_recipients`] hands the emitter the live sessions to tell.
//! What each writer says: a row that stays but stops serving (the blacklist's or a user's pause, a
//! park, a safety-list hold, an install over a paused row) is `paused`
//! ([`ModRuntime::pause`], or [`ModRuntime::replace_all`] for a mod whose row is still there); a row
//! that is gone or removed is `uninstalled`; an install over an installed mod is `replaced`; a
//! closed or curtained store says nothing ([`ModRuntime::clear`]), since no mod was changed.
//!
//! **Serving is bounded.** Each request reads and hashes its whole file, so at most
//! [`MAX_SERVING`] are read at once and [`MAX_WAITING`] more wait their turn ([`ServeGate`]); past
//! that a request is refused with a 503.
//!
//! The registry is in memory. The ledger (`ledger.rs`) persists installs in the `mods` table and
//! fills the registry from it whenever this instance opens a store as its writer.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::http::{Method, Request, Response, StatusCode, Uri};
use tauri::Url;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::engine::Platform;
use super::folder::FolderSessions;
use super::headers;
use super::ids::ModId;
use super::outbox::{Ending, Outbox};
use super::package::{self, VerifiedPackage};
use super::MAIN_LABEL;
use crate::pathguard;

/// How long a minted entry URL (and its session, until the frame loads) stays usable. ModFrame
/// loads it at once; a minute covers a slow first paint.
pub const ENTRY_TTL: Duration = Duration::from_secs(60);
/// Entry URLs waiting to load at once. More means something is minting without loading.
const MAX_PENDING: usize = 32;
const MAX_SESSIONS: usize = 64;
const TOKEN_BYTES: usize = 32;
const SESSION_BYTES: usize = 16;
/// Files read (and hashed) at once. Each can be up to a package entry's 16 MiB, so this bounds the
/// handler's memory and its share of tokio's blocking threads, which PM's own file and DB work use.
pub const MAX_SERVING: usize = 6;
/// Requests waiting for a read. A page loads a few dozen files; hundreds more is a mod flooding.
pub const MAX_WAITING: usize = 512;

/// The scheme's turnstile: [`MAX_SERVING`] reads at once, [`MAX_WAITING`] in line behind them.
pub struct ServeGate {
    reads: Arc<Semaphore>,
    waiting: AtomicUsize,
    max_waiting: usize,
}

/// One request's place in line, or its turn: it leaves the line when dropped, the turn included.
struct InLine<'a>(&'a AtomicUsize);

impl Drop for InLine<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl ServeGate {
    pub fn new(max_serving: usize, max_waiting: usize) -> Self {
        Self {
            reads: Arc::new(Semaphore::new(max_serving)),
            waiting: AtomicUsize::new(0),
            max_waiting,
        }
    }

    /// Wait for a turn to read. `None` when the line is full (the request is refused) or the gate
    /// is closed (never, today). Holding the permit is holding the turn.
    pub async fn enter(&self) -> Option<OwnedSemaphorePermit> {
        if self.waiting.fetch_add(1, Ordering::SeqCst) >= self.max_waiting {
            self.waiting.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        // Leaves the line once the turn comes, or if this future is dropped while waiting.
        let _in_line = InLine(&self.waiting);
        Arc::clone(&self.reads).acquire_owned().await.ok()
    }
}

/// Why a frame call was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    NotInstalled,
    TooManyFrames,
    /// The session was closed, never existed, expired before its frame loaded, or (for a call in
    /// flight) moved to a later generation.
    ClosedOrUnknown,
    /// The session's frame hasn't loaded its entry page yet.
    NotReady,
    Random,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotInstalled => "That mod isn't installed.",
            Self::TooManyFrames => "Too many mod frames are opening at once.",
            Self::ClosedOrUnknown => "That mod session has ended.",
            Self::NotReady => "That mod hasn't finished loading.",
            Self::Random => "PM couldn't make a random token.",
        })
    }
}

/// What a call holds from the moment its session is resolved: which mod it is (PM's own record,
/// never anything the frame said), the session's generation then, and the store the session was
/// opened over. [`ModRuntime::still_current`] answers whether it may go on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallTicket {
    pub session: String,
    pub mod_id: ModId,
    generation: u64,
    /// `AppState::store_epoch` when the session was minted.
    pub store_epoch: u64,
}

/// What ModFrame gets: the URL its iframe loads once, and the session its bridge calls under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameOpened {
    pub entry_url: String,
    pub session: String,
}

/// One installed mod: its folder and exactly the files the verified package listed under `app/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledMod {
    id: ModId,
    /// Canonical, so containment compares like with like (pathguard's contract).
    root: PathBuf,
    entry: String,
    files: BTreeMap<String, ServedFile>,
}

/// A file as verified: served only while the bytes on disk still match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServedFile {
    pub sha256: [u8; 32],
    pub size: u64,
}

impl InstalledMod {
    /// A verified package whose files have been written under `root`.
    pub fn from_package(pkg: &VerifiedPackage, root: &Path) -> std::io::Result<Self> {
        let files = pkg.files.iter().map(|(path, f)| {
            let served = ServedFile {
                sha256: f.sha256,
                size: f.bytes.len() as u64,
            };
            (path.clone(), served)
        });
        Self::from_ledger(
            pkg.manifest.id.clone(),
            root,
            &pkg.manifest.entry.page,
            files,
        )
    }

    /// A mod rebuilt from its `mods` row at startup (`ledger.rs`), from the row's own file list:
    /// the disk is never asked what to serve, and every serve re-checks the bytes against these
    /// hashes. Only `app/` files are served.
    pub fn from_ledger(
        id: ModId,
        root: &Path,
        entry: &str,
        files: impl IntoIterator<Item = (String, ServedFile)>,
    ) -> std::io::Result<Self> {
        let files = files
            .into_iter()
            .filter(|(path, _)| path.starts_with(headers::APP_DIR))
            .collect();
        Ok(Self {
            id,
            root: std::fs::canonicalize(root)?,
            entry: entry.to_owned(),
            files,
        })
    }
}

struct Ticket {
    session: String,
    mod_id: ModId,
    path: String,
    expires: Instant,
}

enum SessionState {
    /// Minted, its frame not loaded yet; its token is in `tickets`.
    Pending { token: String, expires: Instant },
    /// Its entry page was served.
    Live,
}

struct Session {
    mod_id: ModId,
    state: SessionState,
    /// Changed by every bump ([`ModRuntime::bump_all`]); a call that began under another value is
    /// no longer current. Drawn from [`Inner::last_generation`], so no two sessions, and no session
    /// before and after a bump, ever share one.
    generation: u64,
    /// The store this session may reach (`AppState::store_epoch` when it was minted).
    store_epoch: u64,
    /// Where this session's events go: the channel its frame's page opened it with.
    outbox: Arc<Outbox>,
    /// When the frame was asked for, and when it last did anything PM can see (a bridge call began):
    /// what the frozen-page watchdog reads to name the frame that was last active (`watchdog`).
    opened: Instant,
    touched: Instant,
    /// How many times this session's frame has asked PM to show a key field (`secrets.set`), counted
    /// here so the limit is Rust's: PM's page can't be talked round it and a new frame starts at 0.
    secret_asks: u32,
}

impl Session {
    /// Whether `ticket` is this session's own call: live, the same mod, generation and store.
    fn is_current(&self, ticket: &CallTicket) -> bool {
        matches!(self.state, SessionState::Live)
            && self.mod_id == ticket.mod_id
            && self.generation == ticket.generation
            && self.store_epoch == ticket.store_epoch
    }
}

#[derive(Default)]
struct Inner {
    installed: HashMap<ModId, Arc<InstalledMod>>,
    tickets: HashMap<String, Ticket>,
    sessions: HashMap<String, Session>,
    /// The last generation handed out.
    last_generation: u64,
    /// Outboxes of sessions this change ended, waiting to be told and closed once the lock is
    /// released ([`Locked`]'s drop). Every way a session leaves `sessions` goes through
    /// [`Inner::retire`], so none can leave its outbox open.
    retired: Vec<(Arc<Outbox>, Ending)>,
}

impl Inner {
    fn next_generation(&mut self) -> u64 {
        self.last_generation += 1;
        self.last_generation
    }

    /// A session has left `sessions`: its outbox is closed, after a goodbye if its frame ever
    /// loaded. A frame that never loaded has nobody to tell.
    fn retire(&mut self, session: Session, ending: Ending) {
        let live = matches!(session.state, SessionState::Live);
        let ending = if live { ending } else { Ending::Silent };
        self.retired.push((session.outbox, ending));
    }

    /// Remove every session `doomed` names, retiring each with `ending`.
    fn remove_sessions_where(&mut self, ending: Ending, doomed: impl Fn(&Session) -> bool) {
        let ids: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, s)| doomed(s))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            if let Some(session) = self.sessions.remove(&id) {
                self.retire(session, ending);
            }
        }
    }

    fn sweep(&mut self, now: Instant) {
        self.tickets.retain(|_, t| t.expires > now);
        self.remove_sessions_where(Ending::Silent, |s| match &s.state {
            SessionState::Pending { expires, .. } => *expires <= now,
            SessionState::Live => false,
        });
    }

    fn drop_mod(&mut self, id: &ModId, ending: Ending) {
        self.tickets.retain(|_, t| &t.mod_id != id);
        self.remove_sessions_where(ending, |s| &s.mod_id == id);
    }

    /// Whether `token` is a live ticket for exactly this mod and file. Never consumes.
    fn ticket_matches(&self, target: &Target, token: &str, now: Instant) -> bool {
        self.tickets
            .get(token)
            .is_some_and(|t| t.expires > now && t.mod_id == target.id && t.path == target.path)
    }

    /// Consume `token`. True only when it was this mod's ticket for this file, unexpired, with its
    /// session still waiting; the session is then live. Any other use burns the token and closes
    /// its session.
    fn take_ticket(&mut self, target: &Target, token: &str, now: Instant) -> bool {
        let valid = self.ticket_matches(target, token, now);
        let Some(ticket) = self.tickets.remove(token) else {
            return false;
        };
        let waiting = matches!(
            self.sessions.get(&ticket.session),
            Some(Session { state: SessionState::Pending { token: t, .. }, .. }) if t == token
        );
        if valid && waiting {
            if let Some(session) = self.sessions.get_mut(&ticket.session) {
                session.state = SessionState::Live;
            }
            true
        } else {
            if let Some(session) = self.sessions.remove(&ticket.session) {
                self.retire(session, Ending::Silent);
            }
            false
        }
    }
}

/// The registry's lock. Dropping it releases the lock first, then tells and closes the outboxes of
/// the sessions the change ended ([`Inner::retire`]): a goodbye is a send into a webview channel, and
/// that never runs while the registry is held.
struct Locked<'a> {
    guard: Option<MutexGuard<'a, Inner>>,
}

impl Deref for Locked<'_> {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        self.guard.as_ref().expect("held until dropped")
    }
}

impl DerefMut for Locked<'_> {
    fn deref_mut(&mut self) -> &mut Inner {
        self.guard.as_mut().expect("held until dropped")
    }
}

impl Drop for Locked<'_> {
    fn drop(&mut self) {
        let Some(mut guard) = self.guard.take() else {
            return;
        };
        let retired = std::mem::take(&mut guard.retired);
        drop(guard);
        for (outbox, ending) in retired {
            outbox.finish(ending.notices());
        }
    }
}

/// Who an event is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Audience {
    /// Every live session of every mod.
    Everyone,
    /// Every live session of one mod (`grants.changed`, which the Mods tab's commands send).
    Mod(ModId),
    /// One session.
    Session(String),
}

/// A live session an event may go to: its ticket (for the generation and store it is current in)
/// and its outbox.
#[derive(Clone)]
pub struct Recipient {
    pub ticket: CallTicket,
    pub outbox: Arc<Outbox>,
}

/// A frame that is mounted, as the watchdog reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountedFrame {
    pub mod_id: ModId,
    /// When the page asked for it.
    pub opened: Instant,
    /// When it last made a bridge call (or `opened`, if it never has).
    pub touched: Instant,
}

/// The scheme's state, managed by `setup()` for the life of the process.
pub struct ModRuntime {
    platform: Platform,
    /// PM's own page origins, which alone may frame a mod.
    frame_ancestors: Vec<String>,
    inner: Mutex<Inner>,
    gate: ServeGate,
    /// Held by every change to the mod folders and the `mods` rows (`ledger.rs`: install, update,
    /// uninstall, the startup load and sweep, the wipe), so the sweep can never see an install's
    /// folder between its rename and its row's commit.
    ledger: Mutex<()>,
    /// Whether unsigned mods may be served: developer mode is on for this device in the open vault
    /// (`devmode`). Set by the writer's startup before the load and by the dev-mode switch, from the
    /// store's record, and read by every ledger load; false (nothing unsigned served) until then and
    /// whenever the store closes, so no path can serve an unsigned mod without developer mode having
    /// been looked up. The record in the store is the truth, this is its in-memory shadow.
    unsigned_allowed: AtomicBool,
    /// The test folders loaded this session (`folder`): memory only, forgotten with the store, the
    /// mod or developer mode. A `folder` row with no session here is never served.
    folders: FolderSessions,
}

impl ModRuntime {
    pub fn new(platform: Platform, frame_ancestors: Vec<String>) -> Self {
        Self {
            platform,
            frame_ancestors,
            inner: Mutex::new(Inner::default()),
            gate: ServeGate::new(MAX_SERVING, MAX_WAITING),
            ledger: Mutex::new(()),
            unsigned_allowed: AtomicBool::new(false),
            folders: FolderSessions::default(),
        }
    }

    /// The loaded test folders (`folder`).
    pub fn folders(&self) -> &FolderSessions {
        &self.folders
    }

    /// Whether unsigned mods may be served now (developer mode is on for this device in this vault).
    pub fn unsigned_allowed(&self) -> bool {
        self.unsigned_allowed.load(Ordering::SeqCst)
    }

    /// Record what the store says about developer mode for this device. The caller reads it from
    /// the open store ([`super::devmode::is_on`]) and then loads, so the registry follows.
    pub fn set_unsigned_allowed(&self, allowed: bool) {
        self.unsigned_allowed.store(allowed, Ordering::SeqCst);
    }

    /// The ledger's lock. Synchronous work only: never held across an `.await`. A panic part way
    /// through a ledger change leaves the disk in a state the next sweep handles, so a poisoned
    /// lock is taken all the same.
    pub fn ledger_lock(&self) -> MutexGuard<'_, ()> {
        self.ledger.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Make the registry exactly `mods` (a ledger load). A mod that stays exactly as it was keeps its
    /// frames; one that changed loses them (told `replaced`); one that left loses them too, told
    /// `paused` when `parked` names it (its row is still there but isn't served: parked for its
    /// files, held by the safety list, paused, waiting for consent) and `uninstalled` when it
    /// doesn't (no row). Without the distinction a mod parked at a reload would hear "uninstalled",
    /// and a Mods tab built on the goodbye would show it gone.
    pub fn replace_all(&self, mods: Vec<InstalledMod>, parked: &HashSet<ModId>) {
        let mut inner = self.lock();
        let incoming: HashMap<ModId, InstalledMod> =
            mods.into_iter().map(|m| (m.id.clone(), m)).collect();
        let stale: Vec<ModId> = inner
            .installed
            .iter()
            .filter(|(id, m)| incoming.get(*id) != Some(m.as_ref()))
            .map(|(id, _)| id.clone())
            .collect();
        for id in &stale {
            inner.installed.remove(id);
            // A mod that is still there, changed, was replaced; one whose row no longer serves was
            // paused; one that is gone was uninstalled.
            let ending = if incoming.contains_key(id) {
                Ending::Replaced
            } else if parked.contains(id) {
                Ending::Paused
            } else {
                Ending::Uninstalled
            };
            inner.drop_mod(id, ending);
        }
        for (id, m) in incoming {
            inner.installed.entry(id).or_insert_with(|| Arc::new(m));
        }
    }

    /// The store the registry was loaded from is closed or curtained (another profile took the
    /// writer role, a lock hand-off, "Remove PM data"): serve nothing and end every session. No mod
    /// was uninstalled or paused, so no frame is told it was: the sessions' store is gone and the
    /// page that held them goes with it; their next call finds the session ended.
    pub fn clear(&self) {
        // Nothing unsigned is served by a store that hasn't looked developer mode up (the writer's
        // next open sets it again).
        self.set_unsigned_allowed(false);
        // A test folder is a session of the store that closed: its row goes at the next writer open
        // (or sooner), and nothing serves it without the session.
        self.folders.forget_all();
        let mut inner = self.lock();
        inner.installed.clear();
        inner.tickets.clear();
        inner.remove_sessions_where(Ending::Silent, |_| true);
    }

    /// Where an installed mod is served from, for the ledger's tests.
    #[cfg(test)]
    pub fn installed_root(&self, id: &ModId) -> Option<PathBuf> {
        self.lock().installed.get(id).map(|m| m.root.clone())
    }

    /// Whether the registry holds `id`, so its frame is served when it opens: the truth a restore's
    /// "is back" reads (`backup::adopt_mods::settled`), since an `active` row can still be held
    /// back (an unaccepted pilot, the safety list, files that didn't check out).
    pub fn is_installed(&self, id: &ModId) -> bool {
        self.lock().installed.contains_key(id)
    }

    /// The scheme handler's turnstile.
    pub fn gate(&self) -> &ServeGate {
        &self.gate
    }

    /// Every change under this lock is a handful of map inserts and removals that can't panic part
    /// way, so a poisoned lock still holds consistent state and is read through.
    fn lock(&self) -> Locked<'_> {
        Locked {
            guard: Some(self.inner.lock().unwrap_or_else(|e| e.into_inner())),
        }
    }

    /// Install (or replace) a mod in the registry. A replacement ends every frame of the old one.
    pub fn install(&self, installed: InstalledMod) {
        let mut inner = self.lock();
        inner.drop_mod(&installed.id, Ending::Replaced);
        inner
            .installed
            .insert(installed.id.clone(), Arc::new(installed));
    }

    /// Remove a mod, its unused entry URLs and its sessions. Each live frame is told its session
    /// closed (`session.closed`, "uninstalled") on the way out.
    pub fn uninstall(&self, id: &ModId) {
        // An uninstalled test folder is no longer loaded (its files are the developer's: only the
        // session is dropped).
        self.folders.forget(id);
        self.remove(id, Ending::Uninstalled);
    }

    /// A mod was paused or parked (its row is no longer active): stop serving it and end every
    /// session it has, so a call in flight commits nothing (spec §1.8). The writer of the pause
    /// calls this after its row commits; until it does, the dispatcher's own read of the row
    /// already refuses new calls and every commit. Each live frame is told `mod.paused`, then
    /// `session.closed`, as its last two events.
    pub fn pause(&self, id: &ModId) {
        self.remove(id, Ending::Paused);
    }

    fn remove(&self, id: &ModId, ending: Ending) {
        let mut inner = self.lock();
        inner.installed.remove(id);
        inner.drop_mod(id, ending);
    }

    /// The live sessions `audience` names, each with the ticket it is current under and its outbox:
    /// what an event is delivered to. Only a session whose frame has loaded (a pending one has
    /// nobody to hear it). The emitter asks again whether each is [`Self::still_current`] just
    /// before it delivers.
    pub fn live_recipients(&self, audience: &Audience) -> Vec<Recipient> {
        let inner = self.lock();
        inner
            .sessions
            .iter()
            .filter(|(session, s)| {
                matches!(s.state, SessionState::Live)
                    && match audience {
                        Audience::Everyone => true,
                        Audience::Mod(id) => &s.mod_id == id,
                        Audience::Session(wanted) => *session == wanted,
                    }
            })
            .map(|(session, s)| Recipient {
                ticket: CallTicket {
                    session: session.clone(),
                    mod_id: s.mod_id.clone(),
                    generation: s.generation,
                    store_epoch: s.store_epoch,
                },
                outbox: Arc::clone(&s.outbox),
            })
            .collect()
    }

    /// The frames PM has handed out and not closed, live or still loading: what "a mod frame is
    /// mounted" means to the frozen-page watchdog (`watchdog`). A session is the page asking for a
    /// frame, so this is PM's own record of mounts, not anything the page reports.
    pub fn mounted_frames(&self) -> Vec<MountedFrame> {
        self.lock()
            .sessions
            .values()
            .map(|s| MountedFrame {
                mod_id: s.mod_id.clone(),
                opened: s.opened,
                touched: s.touched,
            })
            .collect()
    }

    /// Each live or loading session's event counters, by mod and then by what was sent (the
    /// Developer tab's readout). The registry's lock is released before any outbox is asked, so the
    /// two locks are never held together.
    pub fn outbox_stats(&self) -> Vec<(ModId, super::outbox::Stats)> {
        let outboxes: Vec<(ModId, Arc<Outbox>)> = self
            .lock()
            .sessions
            .values()
            .map(|s| (s.mod_id.clone(), Arc::clone(&s.outbox)))
            .collect();
        let mut stats: Vec<_> = outboxes
            .into_iter()
            .map(|(id, outbox)| (id, outbox.stats()))
            .collect();
        stats.sort_by(|a, b| (a.0.as_str(), a.1.sent).cmp(&(b.0.as_str(), b.1.sent)));
        stats
    }

    /// Whether any frame is live: the cheap question an emitter asks before it holds anything for one.
    pub fn any_live(&self) -> bool {
        self.lock()
            .sessions
            .values()
            .any(|s| matches!(s.state, SessionState::Live))
    }

    /// The frame has taken events up to `seq` (`mod_events_ack`). An unknown session is ignored: the
    /// page may ack once more as its frame is torn down.
    pub fn ack(&self, session: &str, seq: u64) {
        let outbox = self
            .lock()
            .sessions
            .get(session)
            .map(|s| Arc::clone(&s.outbox));
        if let Some(outbox) = outbox {
            outbox.ack(seq);
        }
    }

    /// Move every session to a new generation, so no call already in flight may go on: the app was
    /// locked (`settings::set_app_lock`). The sessions themselves stay; their next calls meet the
    /// lock check first.
    pub fn bump_all(&self) {
        let mut inner = self.lock();
        let ids: Vec<String> = inner.sessions.keys().cloned().collect();
        for id in ids {
            let next = inner.next_generation();
            if let Some(session) = inner.sessions.get_mut(&id) {
                session.generation = next;
            }
        }
    }

    /// [`ModRuntime::open_frame_in`] over store epoch 0, for tests that never swap a store.
    #[cfg(test)]
    pub fn open_frame(&self, id: &ModId) -> Result<FrameOpened, FrameError> {
        self.open_frame_at(id, Instant::now(), 0)
    }

    /// [`Self::open_frame_with`] at a time of the test's choosing, its events going nowhere.
    #[cfg(test)]
    fn open_frame_at(
        &self,
        id: &ModId,
        now: Instant,
        store_epoch: u64,
    ) -> Result<FrameOpened, FrameError> {
        self.open_frame_with(id, now, store_epoch, super::outbox::testing::discarding())
    }

    /// Mint an entry URL and a session for `id`, bound to the store open now (`store_epoch`, read
    /// from `AppState::store_epoch` by `mod_frame_open`): its calls reach that store or none. Its
    /// events go to `outbox`, which the session owns from here: it is closed whenever the session ends.
    pub fn open_frame_in(
        &self,
        id: &ModId,
        store_epoch: u64,
        outbox: Arc<Outbox>,
    ) -> Result<FrameOpened, FrameError> {
        self.open_frame_with(id, Instant::now(), store_epoch, outbox)
    }

    fn open_frame_with(
        &self,
        id: &ModId,
        now: Instant,
        store_epoch: u64,
        outbox: Arc<Outbox>,
    ) -> Result<FrameOpened, FrameError> {
        let token = random_hex(TOKEN_BYTES)?;
        let session = random_hex(SESSION_BYTES)?;
        let mut inner = self.lock();
        inner.sweep(now);
        let entry = inner
            .installed
            .get(id)
            .ok_or(FrameError::NotInstalled)?
            .entry
            .clone();
        if inner.tickets.len() >= MAX_PENDING || inner.sessions.len() >= MAX_SESSIONS {
            return Err(FrameError::TooManyFrames);
        }
        let expires = now + ENTRY_TTL;
        inner.tickets.insert(
            token.clone(),
            Ticket {
                session: session.clone(),
                mod_id: id.clone(),
                path: entry.clone(),
                expires,
            },
        );
        let generation = inner.next_generation();
        inner.sessions.insert(
            session.clone(),
            Session {
                mod_id: id.clone(),
                state: SessionState::Pending {
                    token: token.clone(),
                    expires,
                },
                generation,
                store_epoch,
                outbox,
                opened: now,
                touched: now,
                secret_asks: 0,
            },
        );
        Ok(FrameOpened {
            entry_url: format!(
                "{}/{id}/{entry}?{ENTRY_PARAM}={token}",
                self.platform.mod_origin()
            ),
            session,
        })
    }

    /// The navigation guard's question: is `url` an entry URL a frame is about to load? Read-only,
    /// so asking can't burn a token before the handler sees the request.
    pub fn is_pending_entry(&self, url: &Url) -> bool {
        self.is_pending_entry_at(url, Instant::now())
    }

    fn is_pending_entry_at(&self, url: &Url, now: Instant) -> bool {
        let Some(target) = target_of_url(url, self.platform.mod_origin()) else {
            return false;
        };
        let Some(token) = &target.token else {
            return false;
        };
        self.lock().ticket_matches(&target, token, now)
    }

    /// End every session and every unused entry URL: main's page is loading again, so every frame
    /// that held one is gone, and none of them ran its cleanup. Without this each would stay callable
    /// for the rest of the run and count against [`MAX_SESSIONS`] until PM restarted.
    pub fn close_all(&self) {
        let mut inner = self.lock();
        inner.tickets.clear();
        // Nobody to tell: the page that held the channels is gone.
        inner.remove_sessions_where(Ending::Silent, |_| true);
    }

    /// End a session. Its unused entry URL dies with it, and its outbox closes with nothing more
    /// said (the frame asked, or is gone). Ending one that has already ended is fine: a frame's
    /// unmount and its navigation-kill can both close it.
    pub fn close(&self, session: &str) {
        let mut inner = self.lock();
        if let Some(ended) = inner.sessions.remove(session) {
            if let SessionState::Pending { token, .. } = &ended.state {
                inner.tickets.remove(token);
            }
            inner.retire(ended, Ending::Silent);
        }
    }

    /// The mod a live session belongs to: PM's own record, never anything the frame said. (The
    /// dispatcher asks [`Self::begin_call`], which says the same and more.)
    #[cfg(test)]
    pub fn session_mod(&self, session: &str) -> Result<ModId, FrameError> {
        self.session_mod_at(session, Instant::now())
    }

    #[cfg(test)]
    fn session_mod_at(&self, session: &str, now: Instant) -> Result<ModId, FrameError> {
        self.begin_call_at(session, now).map(|ticket| ticket.mod_id)
    }

    /// Begin a bridge call under `session`: only a live session, resolved to its mod by PM's own
    /// record. The ticket carries the session's generation and store, for [`Self::still_current`].
    pub fn begin_call(&self, session: &str) -> Result<CallTicket, FrameError> {
        self.begin_call_at(session, Instant::now())
    }

    fn begin_call_at(&self, session: &str, now: Instant) -> Result<CallTicket, FrameError> {
        let mut inner = self.lock();
        inner.sweep(now);
        // A call is the frame being active (the watchdog's "last active").
        if let Some(s) = inner.sessions.get_mut(session) {
            s.touched = now;
        }
        match inner.sessions.get(session) {
            Some(Session {
                mod_id,
                state: SessionState::Live,
                generation,
                store_epoch,
                ..
            }) => Ok(CallTicket {
                session: session.to_owned(),
                mod_id: mod_id.clone(),
                generation: *generation,
                store_epoch: *store_epoch,
            }),
            Some(_) => Err(FrameError::NotReady),
            None => Err(FrameError::ClosedOrUnknown),
        }
    }

    /// Whether a call begun with `ticket` may go on: its session is still open and live, for the
    /// same mod, at the same generation. The dispatcher asks after every `.await` and immediately
    /// before any commit.
    pub fn still_current(&self, ticket: &CallTicket) -> Result<(), FrameError> {
        let inner = self.lock();
        match inner.sessions.get(&ticket.session) {
            Some(session) if session.is_current(ticket) => Ok(()),
            _ => Err(FrameError::ClosedOrUnknown),
        }
    }

    /// Count one more key-field ask (`secrets.set`) against the call's session, if it is still
    /// current: `Ok(true)` when it may go ahead, `Ok(false)` when the session has already asked
    /// `max` times (nothing is counted then). The count lives and dies with the session, and the
    /// check and the count are one step under the registry's lock, so parallel asks cannot both
    /// take the last one.
    pub fn note_secret_ask(&self, ticket: &CallTicket, max: u32) -> Result<bool, FrameError> {
        let mut inner = self.lock();
        match inner.sessions.get_mut(&ticket.session) {
            Some(session) if session.is_current(ticket) => {
                if session.secret_asks >= max {
                    return Ok(false);
                }
                session.secret_asks += 1;
                Ok(true)
            }
            _ => Err(FrameError::ClosedOrUnknown),
        }
    }

    /// Answer one `pmmod` request from the webview labelled `label`.
    pub fn serve(&self, label: &str, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
        self.serve_at(label, req, Instant::now())
    }

    fn serve_at(&self, label: &str, req: &Request<Vec<u8>>, now: Instant) -> Response<Vec<u8>> {
        // v1 is main-window-only: no other window can load a mod file or spend an entry token.
        if label != MAIN_LABEL {
            return refusal(StatusCode::NOT_FOUND);
        }
        if req.method() != Method::GET {
            return refusal(StatusCode::METHOD_NOT_ALLOWED);
        }
        let Some(target) = target_of_request(req.uri()) else {
            return refusal(StatusCode::NOT_FOUND);
        };
        // Decide under the lock; read the file after it is released.
        let (installed, is_entry) = {
            let mut inner = self.lock();
            let is_entry = target
                .token
                .as_deref()
                .is_some_and(|t| inner.take_ticket(&target, t, now));
            (inner.installed.get(&target.id).cloned(), is_entry)
        };
        let Some(installed) = installed else {
            return refusal(StatusCode::NOT_FOUND);
        };
        let Some(want) = installed.files.get(&target.path) else {
            return refusal(StatusCode::NOT_FOUND);
        };
        let bytes = match read_verified(&installed.root, &target.path, want) {
            Ok(bytes) => bytes,
            Err(why) => {
                eprintln!(
                    "mods: refused to serve {}/{}: {why}",
                    installed.id, target.path
                );
                return refusal(StatusCode::NOT_FOUND);
            }
        };
        let ancestors: Vec<&str> = self.frame_ancestors.iter().map(String::as_str).collect();
        let Some(headers) = headers::file_headers(
            self.platform.mod_origin(),
            installed.id.as_str(),
            &target.path,
            &ancestors,
            is_entry,
        ) else {
            return refusal(StatusCode::INTERNAL_SERVER_ERROR);
        };
        let mut response = Response::builder().status(StatusCode::OK);
        for (name, value) in headers {
            response = response.header(name, value);
        }
        response
            .body(bytes)
            .unwrap_or_else(|_| refusal(StatusCode::INTERNAL_SERVER_ERROR))
    }
}

/// An empty response with the refusal headers.
pub fn refusal(status: StatusCode) -> Response<Vec<u8>> {
    let mut response = Response::builder().status(status);
    for (name, value) in headers::refusal_headers() {
        response = response.header(name, value);
    }
    response.body(Vec::new()).unwrap_or_else(|_| {
        let mut bare = Response::new(Vec::new());
        *bare.status_mut() = status;
        bare
    })
}

/// The query parameter that carries an entry token.
const ENTRY_PARAM: &str = "entry";

/// A request for one mod file, maybe carrying an entry token.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    id: ModId,
    /// A package path under `app/`.
    path: String,
    token: Option<String>,
}

/// `/<id>/<package path>` and an optional `entry=<token>` query. The path is held to the package
/// path rules, so nothing percent-encoded, no dot segments, nothing outside `app/`; anything else
/// in the query is ignored (a bundler's cache-busting `?v=1`), and so is a malformed token.
fn parse_target(path: &str, query: Option<&str>) -> Option<Target> {
    let (id, file) = path.strip_prefix('/')?.split_once('/')?;
    let id = ModId::parse(id).ok()?;
    package::check_path(file).ok()?;
    if !file.starts_with(headers::APP_DIR) {
        return None;
    }
    let token = query
        .and_then(|q| q.strip_prefix(ENTRY_PARAM)?.strip_prefix('='))
        .filter(|t| is_token(t))
        .map(str::to_owned);
    Some(Target {
        id,
        path: file.to_owned(),
        token,
    })
}

/// The handler's URI. wry hands every engine's request over as `pmmod://localhost/…` (on WebView2 it
/// turns `http://pmmod.localhost/…` back into that before the handler sees it), so only that
/// authority is accepted: no other host, no port, no user info.
fn target_of_request(uri: &Uri) -> Option<Target> {
    if uri.scheme_str() != Some(super::SCHEME) || uri.authority()?.as_str() != "localhost" {
        return None;
    }
    parse_target(uri.path(), uri.query())
}

/// The guard's URL, as the engine sees it: under this platform's mod origin exactly.
fn target_of_url(url: &Url, mod_origin: &str) -> Option<Target> {
    let host = url.host_str()?;
    let plain = url.port().is_none() && url.username().is_empty() && url.password().is_none();
    if !plain || format!("{}://{host}", url.scheme()) != mod_origin {
        return None;
    }
    parse_target(url.path(), url.query())
}

fn is_token(s: &str) -> bool {
    s.len() == TOKEN_BYTES * 2 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn random_hex(bytes: usize) -> Result<String, FrameError> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|_| FrameError::Random)?;
    Ok(hex::encode(buf))
}

/// The file under `root`, only while it is still inside `root` and still the verified bytes. A
/// file changed or swapped for a link after install is refused, not served.
fn read_verified(root: &Path, path: &str, want: &ServedFile) -> Result<Vec<u8>, &'static str> {
    let mut full = root.to_path_buf();
    full.extend(path.split('/'));
    let canonical = std::fs::canonicalize(&full).map_err(|_| "it is missing")?;
    if !pathguard::within_root(&canonical, root) {
        return Err("it resolves outside the mod's folder");
    }
    let file = File::open(&canonical).map_err(|_| "it can't be opened")?;
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return Err("it isn't a file");
    }
    let mut bytes = Vec::new();
    file.take(want.size + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "it can't be read")?;
    let hash: [u8; 32] = Sha256::digest(&bytes).into();
    if bytes.len() as u64 != want.size || hash != want.sha256 {
        return Err("it changed after it was verified");
    }
    Ok(bytes)
}

/// The origins that may frame a mod: PM's own page, and in a dev build the dev server that serves
/// it (`build.devUrl`).
pub fn frame_ancestors(platform: Platform, dev_url: Option<&Url>) -> Vec<String> {
    let mut out = vec![platform.app_origin().to_owned()];
    if let Some(url) = dev_url {
        if let Some(host) = url.host_str() {
            let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
            out.push(format!("{}://{host}{port}", url.scheme()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: &str = "tauri://localhost";

    struct Setup {
        runtime: ModRuntime,
        _dir: tempfile::TempDir,
    }

    /// A runtime with `atlas.map` (and `acme.tool`) installed from real files.
    fn setup(platform: Platform) -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let runtime = ModRuntime::new(platform, vec![ORIGIN.to_owned()]);
        for (id, files) in [
            (
                "atlas.map",
                vec![
                    ("app/index.html", b"<!doctype html><p>map".as_slice()),
                    ("app/main.js", b"console.log(1)".as_slice()),
                    ("app/icon.svg", b"<svg/>".as_slice()),
                    ("app/other.html", b"<p>other".as_slice()),
                    ("app/data.bin", b"\x00\x01".as_slice()),
                ],
            ),
            ("acme.tool", vec![("app/index.html", b"<p>acme".as_slice())]),
        ] {
            let root = dir.path().join(id);
            let mut served = BTreeMap::new();
            for (path, bytes) in files {
                let full = root.join(path);
                std::fs::create_dir_all(full.parent().unwrap()).unwrap();
                std::fs::write(&full, bytes).unwrap();
                served.insert(
                    path.to_owned(),
                    ServedFile {
                        sha256: Sha256::digest(bytes).into(),
                        size: bytes.len() as u64,
                    },
                );
            }
            runtime.install(InstalledMod {
                id: ModId::parse(id).unwrap(),
                root: std::fs::canonicalize(&root).unwrap(),
                entry: "app/index.html".into(),
                files: served,
            });
        }
        Setup { runtime, _dir: dir }
    }

    fn id(s: &str) -> ModId {
        ModId::parse(s).unwrap()
    }

    /// The URI the handler sees for an entry URL (wry's form on every engine).
    fn handler_uri(entry_url: &str) -> String {
        entry_url.replacen("http://pmmod.localhost", "pmmod://localhost", 1)
    }

    fn get(rt: &ModRuntime, label: &str, uri: &str) -> Response<Vec<u8>> {
        let req = Request::builder().uri(uri).body(Vec::new()).unwrap();
        rt.serve(label, &req)
    }

    fn header<'a>(r: &'a Response<Vec<u8>>, name: &str) -> Option<&'a str> {
        r.headers().get(name).and_then(|v| v.to_str().ok())
    }

    fn is_document(r: &Response<Vec<u8>>) -> bool {
        r.status() == StatusCode::OK
            && header(r, "Content-Type") == Some("text/html; charset=utf-8")
            && header(r, "Content-Disposition").is_none()
    }

    fn is_attachment(r: &Response<Vec<u8>>) -> bool {
        r.status() == StatusCode::OK && header(r, "Content-Disposition") == Some("attachment")
    }

    #[test]
    fn an_entry_url_renders_once_then_its_session_is_live() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let opened = rt.open_frame(&id("atlas.map")).unwrap();
        assert!(opened
            .entry_url
            .starts_with("pmmod://localhost/atlas.map/app/index.html?entry="));
        assert_eq!(rt.session_mod(&opened.session), Err(FrameError::NotReady));
        let url = Url::parse(&opened.entry_url).unwrap();
        // The guard may ask as often as it likes: asking consumes nothing.
        assert!(rt.is_pending_entry(&url));
        assert!(rt.is_pending_entry(&url));

        let first = get(rt, "main", &opened.entry_url);
        assert!(is_document(&first));
        assert_eq!(first.body(), b"<!doctype html><p>map");
        assert_eq!(rt.session_mod(&opened.session), Ok(id("atlas.map")));
        // Used: the guard says no, and the same URL again is an attachment.
        assert!(!rt.is_pending_entry(&url));
        let second = get(rt, "main", &opened.entry_url);
        assert!(is_attachment(&second));
        // The session survives that: it was the frame's own load that went live.
        assert_eq!(rt.session_mod(&opened.session), Ok(id("atlas.map")));
    }

    #[test]
    fn subresources_are_served_with_the_mods_headers_and_documents_never_without_a_token() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let js = get(rt, "main", "pmmod://localhost/atlas.map/app/main.js");
        assert_eq!(js.status(), StatusCode::OK);
        assert_eq!(js.body(), b"console.log(1)");
        assert_eq!(
            header(&js, "Content-Type"),
            Some("text/javascript; charset=utf-8")
        );
        let csp = header(&js, "Content-Security-Policy").unwrap();
        assert!(csp.starts_with(headers::FIXED_DIRECTIVES));
        assert!(csp.contains("script-src pmmod://localhost/atlas.map/app/;"));
        assert!(csp.ends_with("frame-ancestors tauri://localhost"));
        for (name, value) in [
            ("X-Content-Type-Options", "nosniff"),
            ("X-DNS-Prefetch-Control", "off"),
            ("Cache-Control", "no-store"),
            ("Permissions-Policy", headers::PERMISSIONS_POLICY),
        ] {
            assert_eq!(header(&js, name), Some(value), "{name}");
        }
        assert_eq!(header(&js, "Content-Disposition"), None);
        // A query (a bundler's cache-buster) changes nothing.
        assert_eq!(
            get(rt, "main", "pmmod://localhost/atlas.map/app/main.js?v=3").body(),
            b"console.log(1)"
        );
        for uri in [
            "pmmod://localhost/atlas.map/app/index.html",
            "pmmod://localhost/atlas.map/app/other.html",
            "pmmod://localhost/atlas.map/app/icon.svg",
            "pmmod://localhost/atlas.map/app/data.bin",
            &format!(
                "pmmod://localhost/atlas.map/app/index.html?entry={}",
                "0".repeat(64)
            ),
        ] {
            assert!(is_attachment(&get(rt, "main", uri)), "{uri}");
        }
    }

    #[test]
    fn only_the_main_window_and_only_get() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let opened = rt.open_frame(&id("atlas.map")).unwrap();
        for label in ["calendar", "briefing", "", "Main"] {
            let r = get(rt, label, &opened.entry_url);
            assert_eq!(r.status(), StatusCode::NOT_FOUND, "{label}");
            assert!(r.body().is_empty());
        }
        // Another window's request didn't spend the token.
        let post = Request::builder()
            .method(Method::POST)
            .uri(opened.entry_url.as_str())
            .body(Vec::new())
            .unwrap();
        assert_eq!(
            rt.serve("main", &post).status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert!(is_document(&get(rt, "main", &opened.entry_url)));
    }

    #[test]
    fn only_listed_app_files_under_the_mods_own_path_are_served() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        for uri in [
            // Not listed, or not a mod.
            "pmmod://localhost/atlas.map/app/missing.js",
            "pmmod://localhost/atlas.nope/app/index.html",
            "pmmod://localhost/atlas.map/manifest.json",
            "pmmod://localhost/atlas.map/LICENCE.txt",
            "pmmod://localhost/atlas.map/",
            "pmmod://localhost/atlas.map",
            "pmmod://localhost/",
            // Malformed or escaping.
            "pmmod://localhost/atlas.map/app/../../acme.tool/app/index.html",
            "pmmod://localhost/atlas.map/app/%2e%2e/x",
            "pmmod://localhost/atlas.map/app//main.js",
            "pmmod://localhost/Atlas.Map/app/main.js",
            "pmmod://localhost/atlas.map/APP/main.js",
            "pmmod://localhost/atlas.map/app/main.js%00",
            // Another authority or scheme.
            "pmmod://evil/atlas.map/app/main.js",
            "pmmod://localhost:8080/atlas.map/app/main.js",
            "pmmod://user@localhost/atlas.map/app/main.js",
            "http://pmmod.localhost/atlas.map/app/main.js",
            "tauri://localhost/atlas.map/app/main.js",
        ] {
            let r = get(rt, "main", uri);
            assert_eq!(r.status(), StatusCode::NOT_FOUND, "{uri}");
            assert!(r.body().is_empty(), "{uri}");
            assert!(header(&r, "Content-Security-Policy")
                .unwrap()
                .starts_with("sandbox;"));
        }
    }

    #[test]
    fn a_token_for_one_mod_under_another_mods_path_is_burnt() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let a = rt.open_frame(&id("atlas.map")).unwrap();
        let token = a.entry_url.rsplit_once('=').unwrap().1;
        let under_b = format!("pmmod://localhost/acme.tool/app/index.html?entry={token}");
        assert!(!rt.is_pending_entry(&Url::parse(&under_b).unwrap()));
        assert!(is_attachment(&get(rt, "main", &under_b)));
        // A's own URL no longer works, and its session is gone.
        assert!(!rt.is_pending_entry(&Url::parse(&a.entry_url).unwrap()));
        assert!(is_attachment(&get(rt, "main", &a.entry_url)));
        assert_eq!(rt.session_mod(&a.session), Err(FrameError::ClosedOrUnknown));
    }

    #[test]
    fn a_token_for_the_entry_on_another_file_is_burnt() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let a = rt.open_frame(&id("atlas.map")).unwrap();
        let token = a.entry_url.rsplit_once('=').unwrap().1;
        let other = format!("pmmod://localhost/atlas.map/app/other.html?entry={token}");
        assert!(!rt.is_pending_entry(&Url::parse(&other).unwrap()));
        assert!(is_attachment(&get(rt, "main", &other)));
        assert!(is_attachment(&get(rt, "main", &a.entry_url)));
        assert_eq!(rt.session_mod(&a.session), Err(FrameError::ClosedOrUnknown));
    }

    #[test]
    fn two_frames_have_their_own_tokens_and_sessions() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let one = rt.open_frame(&id("atlas.map")).unwrap();
        let two = rt.open_frame(&id("atlas.map")).unwrap();
        assert_ne!(one.entry_url, two.entry_url);
        assert_ne!(one.session, two.session);
        assert!(is_document(&get(rt, "main", &two.entry_url)));
        assert_eq!(rt.session_mod(&one.session), Err(FrameError::NotReady));
        assert_eq!(rt.session_mod(&two.session), Ok(id("atlas.map")));
        assert!(is_document(&get(rt, "main", &one.entry_url)));
        assert_eq!(rt.session_mod(&one.session), Ok(id("atlas.map")));
        // Closing one leaves the other.
        rt.close(&one.session);
        assert_eq!(
            rt.session_mod(&one.session),
            Err(FrameError::ClosedOrUnknown)
        );
        assert_eq!(rt.session_mod(&two.session), Ok(id("atlas.map")));
    }

    #[test]
    fn the_frames_handed_out_are_listed_with_when_each_last_did_anything_for_the_watchdog() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        assert!(rt.mounted_frames().is_empty());
        let one = rt.open_frame(&id("atlas.map")).unwrap();
        let two = rt.open_frame(&id("atlas.map")).unwrap();
        // Handed out, not yet loaded: still mounted as far as the page is concerned.
        let frames = rt.mounted_frames();
        assert_eq!(frames.len(), 2);
        assert!(frames.iter().all(|f| f.opened == f.touched));
        // A bridge call is the frame being active: only that session's `touched` moves.
        let later = Instant::now() + Duration::from_secs(30);
        let _ = rt.session_mod_at(&one.session, later);
        let frames = rt.mounted_frames();
        assert_eq!(frames.iter().filter(|f| f.touched >= later).count(), 1);
        assert_eq!(frames.iter().filter(|f| f.touched < later).count(), 1);
        // A closed session is no longer mounted.
        rt.close(&one.session);
        rt.close(&two.session);
        assert!(rt.mounted_frames().is_empty());
    }

    #[test]
    fn a_closed_session_is_dead_and_takes_its_unused_url_with_it() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let opened = rt.open_frame(&id("atlas.map")).unwrap();
        rt.close(&opened.session);
        rt.close(&opened.session); // twice is fine
        assert!(!rt.is_pending_entry(&Url::parse(&opened.entry_url).unwrap()));
        assert!(is_attachment(&get(rt, "main", &opened.entry_url)));
        assert_eq!(
            rt.session_mod(&opened.session),
            Err(FrameError::ClosedOrUnknown)
        );
        assert_eq!(
            rt.begin_call(&opened.session),
            Err(FrameError::ClosedOrUnknown)
        );
        // Closed after loading, too.
        let live = rt.open_frame(&id("atlas.map")).unwrap();
        assert!(is_document(&get(rt, "main", &live.entry_url)));
        rt.close(&live.session);
        assert_eq!(
            rt.session_mod(&live.session),
            Err(FrameError::ClosedOrUnknown)
        );
        assert_eq!(
            rt.session_mod("not-a-session"),
            Err(FrameError::ClosedOrUnknown)
        );
    }

    #[test]
    fn a_call_begins_only_under_a_live_session_and_carries_its_mod_and_store() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let opened = rt
            .open_frame_in(
                &id("atlas.map"),
                7,
                super::super::outbox::testing::discarding(),
            )
            .unwrap();
        assert_eq!(rt.begin_call(&opened.session), Err(FrameError::NotReady));
        assert!(is_document(&get(rt, "main", &opened.entry_url)));
        let ticket = rt.begin_call(&opened.session).unwrap();
        assert_eq!(ticket.mod_id, id("atlas.map"));
        assert_eq!(ticket.session, opened.session);
        assert_eq!(ticket.store_epoch, 7);
        assert_eq!(rt.still_current(&ticket), Ok(()));
        assert_eq!(
            rt.begin_call("not-a-session"),
            Err(FrameError::ClosedOrUnknown)
        );
    }

    /// Every event that must stop a call in flight does: a bump (the app lock), a pause, an
    /// uninstall, a reinstall, a closed store (the registry emptied), a reload and a close. A
    /// ticket from one session never passes for another.
    #[test]
    fn a_call_in_flight_stops_being_current_on_every_bump_and_end() {
        fn live(rt: &ModRuntime, m: &str) -> CallTicket {
            let opened = rt.open_frame(&id(m)).unwrap();
            assert!(is_document(&get(rt, "main", &opened.entry_url)));
            rt.begin_call(&opened.session).unwrap()
        }
        fn reinstall(rt: &ModRuntime) {
            let again = rt.lock().installed[&id("atlas.map")].as_ref().clone();
            rt.install(again);
        }
        // (event, does it end only atlas.map's sessions)
        type Event = (&'static str, fn(&ModRuntime), bool);
        let events: [Event; 7] = [
            ("bump", ModRuntime::bump_all, false),
            ("pause", |rt| rt.pause(&id("atlas.map")), true),
            ("uninstall", |rt| rt.uninstall(&id("atlas.map")), true),
            ("reinstall", reinstall, true),
            ("store closed", ModRuntime::clear, false),
            (
                "registry reloaded without it",
                |rt| rt.replace_all(Vec::new(), &HashSet::new()),
                false,
            ),
            ("reload", ModRuntime::close_all, false),
        ];
        for (what, event, only_that_mod) in events {
            // Each event against a fresh runtime, so one's effect can't stand in for another's.
            let s = setup(Platform::Linux);
            let rt = &s.runtime;
            let ticket = live(rt, "atlas.map");
            let other = live(rt, "acme.tool");
            event(rt);
            assert_eq!(
                rt.still_current(&ticket),
                Err(FrameError::ClosedOrUnknown),
                "{what}"
            );
            let other_after = rt.still_current(&other);
            assert_eq!(other_after.is_ok(), only_that_mod, "{what}: the other mod");
        }
        // A bump keeps the session: its next call begins under the new generation.
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let before = live(rt, "atlas.map");
        rt.bump_all();
        let after = rt.begin_call(&before.session).unwrap();
        assert_eq!(rt.still_current(&after), Ok(()));
        assert_eq!(rt.still_current(&before), Err(FrameError::ClosedOrUnknown));
        // A closed session, and a ticket presented under another session's name or store.
        let a = live(rt, "atlas.map");
        let b = live(rt, "acme.tool");
        let forged = CallTicket {
            session: b.session.clone(),
            ..a.clone()
        };
        assert_eq!(rt.still_current(&forged), Err(FrameError::ClosedOrUnknown));
        let other_store = CallTicket {
            store_epoch: a.store_epoch + 1,
            ..a.clone()
        };
        assert_eq!(
            rt.still_current(&other_store),
            Err(FrameError::ClosedOrUnknown)
        );
        rt.close(&a.session);
        assert_eq!(rt.still_current(&a), Err(FrameError::ClosedOrUnknown));
        assert_eq!(rt.still_current(&b), Ok(()));
    }

    #[test]
    fn an_unused_entry_url_and_its_session_expire() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let t0 = Instant::now();
        let opened = rt.open_frame_at(&id("atlas.map"), t0, 0).unwrap();
        let url = Url::parse(&opened.entry_url).unwrap();
        let late = t0 + ENTRY_TTL;
        assert!(rt.is_pending_entry_at(&url, t0 + ENTRY_TTL - Duration::from_secs(1)));
        assert!(!rt.is_pending_entry_at(&url, late));
        let req = Request::builder()
            .uri(opened.entry_url.as_str())
            .body(Vec::new())
            .unwrap();
        assert!(is_attachment(&rt.serve_at("main", &req, late)));
        assert_eq!(
            rt.session_mod_at(&opened.session, late),
            Err(FrameError::ClosedOrUnknown)
        );
        // A live session doesn't expire.
        let live = rt.open_frame_at(&id("atlas.map"), t0, 0).unwrap();
        let req = Request::builder()
            .uri(live.entry_url.as_str())
            .body(Vec::new())
            .unwrap();
        assert!(is_document(&rt.serve_at("main", &req, t0)));
        assert_eq!(
            rt.session_mod_at(&live.session, t0 + ENTRY_TTL * 10),
            Ok(id("atlas.map"))
        );
    }

    #[test]
    fn opening_is_bounded_and_needs_an_installed_mod() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        assert_eq!(
            rt.open_frame(&id("atlas.nope")),
            Err(FrameError::NotInstalled)
        );
        let t0 = Instant::now();
        for _ in 0..MAX_PENDING {
            rt.open_frame_at(&id("atlas.map"), t0, 0).unwrap();
        }
        assert_eq!(
            rt.open_frame_at(&id("atlas.map"), t0, 0),
            Err(FrameError::TooManyFrames)
        );
        // Expired ones are swept, which makes room.
        assert!(rt
            .open_frame_at(&id("atlas.map"), t0 + ENTRY_TTL, 0)
            .is_ok());
    }

    #[test]
    fn uninstalling_or_reinstalling_ends_every_frame_of_that_mod() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let live = rt.open_frame(&id("atlas.map")).unwrap();
        assert!(is_document(&get(rt, "main", &live.entry_url)));
        let pending = rt.open_frame(&id("atlas.map")).unwrap();
        let other = rt.open_frame(&id("acme.tool")).unwrap();
        let again = rt.lock().installed[&id("atlas.map")].as_ref().clone();
        rt.install(again);
        assert_eq!(
            rt.session_mod(&live.session),
            Err(FrameError::ClosedOrUnknown)
        );
        assert!(!rt.is_pending_entry(&Url::parse(&pending.entry_url).unwrap()));
        assert!(rt.is_pending_entry(&Url::parse(&other.entry_url).unwrap()));
        rt.uninstall(&id("acme.tool"));
        assert_eq!(
            rt.session_mod(&other.session),
            Err(FrameError::ClosedOrUnknown)
        );
        assert_eq!(
            get(rt, "main", "pmmod://localhost/acme.tool/app/index.html").status(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn a_file_changed_or_swapped_after_install_is_not_served() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let root = rt.lock().installed[&id("atlas.map")].root.clone();
        std::fs::write(root.join("app/main.js"), b"console.log(2)").unwrap();
        assert_eq!(
            get(rt, "main", "pmmod://localhost/atlas.map/app/main.js").status(),
            StatusCode::NOT_FOUND
        );
        std::fs::write(root.join("app/main.js"), b"console.log(1)!").unwrap();
        assert_eq!(
            get(rt, "main", "pmmod://localhost/atlas.map/app/main.js").status(),
            StatusCode::NOT_FOUND
        );
        std::fs::remove_file(root.join("app/main.js")).unwrap();
        assert_eq!(
            get(rt, "main", "pmmod://localhost/atlas.map/app/main.js").status(),
            StatusCode::NOT_FOUND
        );
        // A link out of the folder, even to identical bytes, is refused by containment.
        #[cfg(unix)]
        {
            let outside = root.parent().unwrap().join("outside.js");
            std::fs::write(&outside, b"console.log(1)").unwrap();
            std::os::unix::fs::symlink(&outside, root.join("app/main.js")).unwrap();
            assert_eq!(
                get(rt, "main", "pmmod://localhost/atlas.map/app/main.js").status(),
                StatusCode::NOT_FOUND
            );
        }
    }

    #[test]
    fn on_webview2_the_entry_url_is_http_pmmod_localhost_and_the_handler_sees_wrys_form() {
        let s = setup(Platform::Windows);
        let rt = &s.runtime;
        let opened = rt.open_frame(&id("atlas.map")).unwrap();
        assert!(opened
            .entry_url
            .starts_with("http://pmmod.localhost/atlas.map/app/index.html?entry="));
        // The guard is never asked on WebView2, but if it were, only this origin would match.
        assert!(rt.is_pending_entry(&Url::parse(&opened.entry_url).unwrap()));
        assert!(!rt.is_pending_entry(&Url::parse(&handler_uri(&opened.entry_url)).unwrap()));
        let r = get(rt, "main", &handler_uri(&opened.entry_url));
        assert!(is_document(&r));
        assert!(header(&r, "Content-Security-Policy")
            .unwrap()
            .contains("script-src http://pmmod.localhost/atlas.map/app/;"));
    }

    #[test]
    fn the_guard_matches_only_this_platforms_origin_and_a_well_formed_token() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let opened = rt.open_frame(&id("atlas.map")).unwrap();
        let token = opened.entry_url.rsplit_once('=').unwrap().1.to_owned();
        for url in [
            format!("http://pmmod.localhost/atlas.map/app/index.html?entry={token}"),
            format!("pmmod://localhost:1/atlas.map/app/index.html?entry={token}"),
            format!("pmmod://x@localhost/atlas.map/app/index.html?entry={token}"),
            format!("pmmod://localhost/atlas.map/app/index.html?entry={token}&x=1"),
            format!("pmmod://localhost/atlas.map/app/index.html?x=1&entry={token}"),
            format!(
                "pmmod://localhost/atlas.map/app/index.html?entry={}",
                token.to_uppercase()
            ),
            format!("pmmod://localhost/atlas.map/app/index.html#entry={token}"),
            "pmmod://localhost/atlas.map/app/index.html".to_owned(),
        ] {
            assert!(!rt.is_pending_entry(&Url::parse(&url).unwrap()), "{url}");
        }
        assert!(rt.is_pending_entry(&Url::parse(&opened.entry_url).unwrap()));
    }

    #[test]
    fn targets_parse_strictly() {
        let t = parse_target("/atlas.map/app/x/main.js", Some("entry=ab")).unwrap();
        assert_eq!(t.id, id("atlas.map"));
        assert_eq!(t.path, "app/x/main.js");
        assert_eq!(t.token, None, "a short token is no token");
        for (path, query) in [
            ("atlas.map/app/x.js", None),
            ("/atlas.map", None),
            ("/atlas.map/", None),
            ("/atlas.map/app/", None),
            ("/atlas.map/app/./x.js", None),
            ("/atlas..map/app/x.js", None),
            ("/con.map/app/x.js", None),
            ("/atlas.map/app/con.js", None),
            ("/atlas.map/app/x.js/", None),
        ] {
            assert_eq!(parse_target(path, query), None, "{path}");
        }
    }

    #[test]
    fn frame_ancestors_are_pms_page_plus_the_dev_server_in_dev() {
        assert_eq!(
            frame_ancestors(Platform::Linux, None),
            ["tauri://localhost"]
        );
        let dev = Url::parse("http://localhost:1420/").unwrap();
        assert_eq!(
            frame_ancestors(Platform::Windows, Some(&dev)),
            ["http://tauri.localhost", "http://localhost:1420"]
        );
        // Every ancestor is one the header builder accepts.
        let all = frame_ancestors(Platform::MacOs, Some(&dev));
        let refs: Vec<&str> = all.iter().map(String::as_str).collect();
        assert!(headers::csp("pmmod://localhost", "a.b", &refs).is_some());
    }

    #[test]
    fn a_reload_of_mains_page_ends_every_session_and_frees_the_cap() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let mut live = Vec::new();
        for _ in 0..MAX_SESSIONS {
            let opened = rt.open_frame(&id("atlas.map")).unwrap();
            assert!(is_document(&get(rt, "main", &opened.entry_url)));
            live.push(opened);
        }
        // Every frame leaked by a reload would sit here for the rest of the run.
        assert_eq!(
            rt.open_frame(&id("atlas.map")),
            Err(FrameError::TooManyFrames)
        );
        let pending = rt.open_frame(&id("acme.tool"));
        assert_eq!(pending, Err(FrameError::TooManyFrames));
        rt.close_all();
        for opened in &live {
            assert_eq!(
                rt.begin_call(&opened.session),
                Err(FrameError::ClosedOrUnknown)
            );
        }
        // An entry URL minted before the reload is dead too.
        let before = rt.open_frame(&id("atlas.map")).unwrap();
        rt.close_all();
        assert!(!rt.is_pending_entry(&Url::parse(&before.entry_url).unwrap()));
        assert!(is_attachment(&get(rt, "main", &before.entry_url)));
        // And the cap is free again.
        let after = rt.open_frame(&id("atlas.map")).unwrap();
        assert!(is_document(&get(rt, "main", &after.entry_url)));
        assert_eq!(rt.session_mod(&after.session), Ok(id("atlas.map")));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_burst_of_requests_is_read_a_few_at_a_time_and_a_flood_is_refused() {
        let gate = Arc::new(ServeGate::new(3, 40));
        let reading = Arc::new(AtomicUsize::new(0));
        let most = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..40 {
            let (gate, reading, most) = (gate.clone(), reading.clone(), most.clone());
            tasks.push(tokio::spawn(async move {
                let Some(_turn) = gate.enter().await else {
                    return false;
                };
                let now = reading.fetch_add(1, Ordering::SeqCst) + 1;
                most.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(5)).await;
                reading.fetch_sub(1, Ordering::SeqCst);
                true
            }));
        }
        for task in tasks {
            assert!(task.await.unwrap(), "a request within the line was refused");
        }
        let most = most.load(Ordering::SeqCst);
        assert!((1..=3).contains(&most), "{most} read at once");
        assert_eq!(gate.waiting.load(Ordering::SeqCst), 0);

        // A full line refuses the next request at once, and a place frees when one leaves.
        let gate = ServeGate::new(1, 2);
        let turn = gate.enter().await.unwrap();
        // Poll each once so both are in line behind the held turn.
        let mut first = Box::pin(gate.enter());
        let mut second = Box::pin(gate.enter());
        assert!(futures_poll_once(&mut first).await.is_none());
        assert!(futures_poll_once(&mut second).await.is_none());
        assert_eq!(gate.waiting.load(Ordering::SeqCst), 2);
        assert!(gate.enter().await.is_none(), "a third in line");
        // One gives up its place (its future dropped): the line has room again.
        drop(second);
        assert_eq!(gate.waiting.load(Ordering::SeqCst), 1);
        drop(turn);
        assert!(first.await.is_some());
        assert_eq!(gate.waiting.load(Ordering::SeqCst), 0);
    }

    /// Poll a future once: `Some` with its output if it was ready, `None` if it is still waiting.
    async fn futures_poll_once<F: std::future::Future + Unpin>(f: &mut F) -> Option<F::Output> {
        std::future::poll_fn(|cx| {
            std::task::Poll::Ready(match std::pin::Pin::new(&mut *f).poll(cx) {
                std::task::Poll::Ready(out) => Some(out),
                std::task::Poll::Pending => None,
            })
        })
        .await
    }

    #[test]
    fn tokens_and_sessions_are_random_and_hex() {
        let s = setup(Platform::Linux);
        let a = s.runtime.open_frame(&id("atlas.map")).unwrap();
        let b = s.runtime.open_frame(&id("atlas.map")).unwrap();
        let token = |o: &FrameOpened| o.entry_url.rsplit_once('=').unwrap().1.to_owned();
        assert!(is_token(&token(&a)) && is_token(&token(&b)));
        assert_ne!(token(&a), token(&b));
        assert_eq!(a.session.len(), SESSION_BYTES * 2);
        assert_ne!(a.session, b.session);
    }
}
