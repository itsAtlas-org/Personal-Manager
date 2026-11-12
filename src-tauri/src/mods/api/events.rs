// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Host API's events (Mods spec §1.5 "Events"): what PM tells a frame, and who may hear it.
//!
//! | Event | Permission | Payload | Source |
//! |---|---|---|---|
//! | `documents.added` | `documents.meta` | `{documents: [...]}`, `documents.list`'s DTO, every field | a document landing (the two `DOCUMENT_LANDED` points, notes, chats: [`note_landed`]) and an un-archive, coalesced |
//! | `documents.deleted` | `documents.meta` | `{ids: [...]}` | every document delete, and an archive, after its commit |
//! | `theme.changed` | `storage` | the snapshot `theme.get` answers with | PM's page, per frame |
//! | `grants.changed` | none | `{}` | the writers of a grant, after their commit |
//! | `mod.paused`, `session.closed` | none | `{}`, `{reason}` | the session's end (`runtime`, `outbox`) |
//!
//! **Rust filters before anything is sent.** [`emit_with`] is the one way an event reaches a frame. For
//! each live session it asks, in one DB guard, the questions a call would be asked: PM isn't locked, the
//! session is still at its store, the mod runs, and its grant for the event's permission stands (D3's
//! exclusions and the sealed-engine rule included, [`grants::check`]). Only then is the payload built, once,
//! and each permitted session's outbox handed it. A mod that isn't granted gets nothing: not the
//! event, not a notice that there was one. The payload is never PM's own event: `documents.added`
//! is built from the row's columns by `documents::dto` (never the `Document`, which carries paths and
//! account ids), `documents.deleted` carries ids, `theme.changed` is rebuilt from validated fields.
//!
//! **Just before it is delivered, again.** The session's generation and its outbox are asked after the
//! DB guard is gone ([`ModRuntime::still_current`], [`Outbox::deliver`]), so an uninstall, pause, lock
//! or store swap in between delivers nothing, and an event can never reach the next session of the
//! same mod: a session is a new random id with its own outbox, chosen from the live sessions at
//! emission time and closed with its session.
//!
//! **After the commit.** A landing is announced after its row committed (the emit points are
//! post-commit). A deletion is only *noted* where it happens, inside the deleter's transaction
//! (`ingest::delete_document`, the one function every document row goes through, and the rebuild's
//! wipe); [`settle_deleted`] runs a moment later, takes the DB guard, which waits out any writer still
//! holding it, and announces only the ids that are gone: a transaction that rolled back left its
//! documents, so says nothing. Ids are never reused (`AUTOINCREMENT`), so "gone" can't be mistaken
//! for "gone and back". An archived document wasn't visible to a mod (`documents::LISTED`), so its
//! deletion isn't announced to one either: an id a mod couldn't read must not be learnable from an
//! event, as it isn't from a call.
//!
//! **A mod's readable set is the documents that aren't archived**, so shelving a document is, to a
//! mod, a deletion and taking it off the shelf is a landing (no event of their own): the writers of
//! `documents.importance` call [`note_listing_change`] around their UPDATE. What is announced is
//! decided at the flush from the committed rows ([`settle_deleted`] announces the ids that are no
//! longer readable, [`flush_landed`] the ones that are), so a rolled-back archive, or one undone
//! within the interval, says nothing, and an id a mod never could read is never announced. A Rebuild
//! re-creates documents (new ids after a wipe, in-place rewrites otherwise) and says so with one
//! resync notice when it settles ([`documents_resync`]).
//!
//! **Coalesced.** A landing or a deletion waits [`limits::EVENT_COALESCE_MS`] for the next, so an
//! import of a thousand documents is a few events, not a thousand. Each [`Hub`] set is bounded
//! ([`limits::MAX_PENDING_IDS`]) and keyed to the store it was noted in: ids noted over one store are
//! never announced over another, and each emission re-checks that under its own DB guard (the store
//! can be swapped between a flush's checks and its emissions). Ids past the cap are lost to every
//! frame; each session that may hear the event is told it `missed` that many
//! ([`crate::mods::outbox`]'s resync notice), as it is for an event its window or the size cap dropped.
//!
//! **Not built here:** the Pinboard's events (`tasks.changed`, `projects.*`, `calendar.synced`,
//! `lifecycle.suspend`, PR C). Adding one is a row in the table, a name in [`EVENTS`] and a call to
//! [`emit_with`].

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::Connection;
use serde_json::{json, Value};
use tauri::AppHandle;

use super::table::{self, Row};
use super::{documents, grants, limits, Host, ModError};
use crate::mods::ids::ModId;
use crate::mods::outbox::{self, Delivery};
use crate::mods::policy::MOD_API_VERSION;
use crate::mods::runtime::Audience;

pub const DOCUMENTS_ADDED: &str = "documents.added";
pub const DOCUMENTS_DELETED: &str = "documents.deleted";
pub const THEME_CHANGED: &str = "theme.changed";
pub const GRANTS_CHANGED: &str = "grants.changed";

/// Every event this PM sends: a table row each (a test holds the two together), and the names the
/// page lets through to a frame (`modBridge.ts`'s `MOD_EVENTS`, pinned by a test).
#[cfg_attr(not(test), allow(dead_code))] // the coverage tests, and the Developer tab (slice 6)
pub const EVENTS: &[&str] = &[
    DOCUMENTS_ADDED,
    DOCUMENTS_DELETED,
    THEME_CHANGED,
    GRANTS_CHANGED,
    outbox::MOD_PAUSED,
    outbox::SESSION_CLOSED,
];

/// How many documents one query reads for `documents.added`: a flood of landings is announced in
/// slices, so no emission holds the DB guard over the whole of it.
const LANDED_SLICE: usize = 500;

/// What one emission did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Emitted {
    /// Events handed to a frame's channel.
    pub sent: usize,
    /// Events dropped for a full window or an over-size payload (counted in the outbox too).
    pub dropped: usize,
    /// Sessions that were not told: not granted, not running, locked, stale, or closed meanwhile.
    pub refused: usize,
}

impl Emitted {
    fn add(&mut self, other: Emitted) {
        self.sent += other.sent;
        self.dropped += other.dropped;
        self.refused += other.refused;
    }
}

/// Tell every session `audience` names that may hear `name`, the payloads `payloads` builds (once,
/// from the DB under the guard, and only if some session may hear it; each payload is one event).
///
/// Synchronous, and it takes the DB guard itself: call it from a blocking task, never while holding
/// the guard.
pub(super) fn emit_with<H: Host>(
    host: &H,
    name: &'static str,
    audience: &Audience,
    payloads: impl FnOnce(&Connection) -> Result<Vec<Value>, ModError>,
) -> Emitted {
    emit_counting(host, name, audience, 0, payloads)
}

/// [`emit_with`], where `lost` events of this name were never built (the hub's cap): each session
/// that may hear `name` is told it missed that many, with the events (if any) or on their own
/// (`Outbox::note_missed`), so a flood the hub couldn't hold is never silent.
fn emit_counting<H: Host>(
    host: &H,
    name: &'static str,
    audience: &Audience,
    lost: u64,
    payloads: impl FnOnce(&Connection) -> Result<Vec<Value>, ModError>,
) -> Emitted {
    #[cfg(test)]
    host.at(super::Point::BeforeEmit);
    let Some(row) = table::event_row(table::ROWS, name, MOD_API_VERSION) else {
        // A name with no row is never sent: the table test forbids a registered one.
        return Emitted::default();
    };
    let recipients = host.runtime().live_recipients(audience);
    let mut out = Emitted::default();
    if recipients.is_empty() {
        return out;
    }
    // PM locked: no frame is told anything (its page is gone with the app tree, and a lock must
    // never be a way to learn what happened behind it). Read before the DB guard: it takes its own.
    if !matches!(host.app_locked(), Ok(false)) {
        out.refused = recipients.len();
        return out;
    }
    let (permitted, events) = {
        let Ok(conn) = host.store().conn() else {
            out.refused = recipients.len();
            return out;
        };
        // Read under the guard, so it is the epoch of the store the checks below reach.
        let epoch = host.store().store_epoch();
        let mut heard: HashMap<ModId, bool> = HashMap::new();
        let mut permitted = Vec::new();
        for recipient in recipients {
            let ok = recipient.ticket.store_epoch == epoch
                && *heard
                    .entry(recipient.ticket.mod_id.clone())
                    .or_insert_with(|| may_hear(host, &conn, row, &recipient.ticket.mod_id));
            if ok {
                permitted.push(recipient);
            } else {
                out.refused += 1;
            }
        }
        if permitted.is_empty() {
            return out;
        }
        match payloads(&conn) {
            Ok(events) => (permitted, events),
            Err(e) => {
                // A store that moved on is expected, not a failure to report.
                if e.code != super::Code::SessionEnded {
                    eprintln!("mods: couldn't build the {name} event ({})", e.message);
                }
                out.refused += permitted.len();
                return out;
            }
        }
    };
    #[cfg(test)]
    host.at(super::Point::BeforeDelivery);
    for recipient in permitted {
        if host.runtime().still_current(&recipient.ticket).is_err() {
            out.refused += 1;
            continue;
        }
        recipient.outbox.note_missed(lost);
        for payload in &events {
            match recipient.outbox.deliver(name, payload.clone()) {
                Delivery::Sent => out.sent += 1,
                Delivery::Dropped => out.dropped += 1,
                Delivery::Closed => {
                    out.refused += 1;
                    break;
                }
            }
        }
    }
    out
}

/// Whether the mod `id` may be told `row`'s event now: it runs, and holds the permission the row
/// names (an event without one goes to any running mod's session).
fn may_hear<H: Host>(host: &H, conn: &Connection, row: &Row, id: &ModId) -> bool {
    let Ok(Some(facts)) = grants::read_mod(conn, id) else {
        return false;
    };
    if facts.running().is_err() {
        return false;
    }
    match row.permission {
        None => true,
        Some(permission) => grants::check(conn, &facts, permission, host.engine_sealed()).is_ok(),
    }
}

// ---- Coalescing ---------------------------------------------------------------------------------

#[derive(Default)]
struct Pending {
    ids: BTreeSet<i64>,
    /// The store these ids were noted over.
    epoch: u64,
    /// A flush is waiting on its timer.
    scheduled: bool,
    /// Ids dropped past [`limits::MAX_PENDING_IDS`] since the last take: frames are told they missed
    /// that many ([`Hub::take`]).
    dropped: u64,
}

/// What is waiting for its interval to end: the documents that landed and the ids that may have
/// gone. Managed state (`lib.rs`); a test makes its own, with a short interval.
#[derive(Default)]
pub struct Hub {
    landed: Mutex<Pending>,
    deleted: Mutex<Pending>,
    /// A test's interval, in place of [`limits::EVENT_COALESCE_MS`].
    delay: Option<Duration>,
}

fn lock(pending: &Mutex<Pending>) -> MutexGuard<'_, Pending> {
    pending.lock().unwrap_or_else(|e| e.into_inner())
}

impl Hub {
    #[cfg(test)]
    pub fn with_delay(delay: Duration) -> Self {
        Self {
            delay: Some(delay),
            ..Self::default()
        }
    }

    fn delay(&self) -> Duration {
        self.delay
            .unwrap_or(Duration::from_millis(limits::EVENT_COALESCE_MS))
    }

    /// Note `ids` over `epoch`; true when the caller must start the timer. Ids noted over another
    /// store are dropped first: they are never announced over this one.
    fn note(pending: &Mutex<Pending>, ids: &[i64], epoch: u64) -> bool {
        let mut p = lock(pending);
        if p.epoch != epoch {
            p.ids.clear();
            p.dropped = 0;
            p.epoch = epoch;
        }
        for id in ids {
            if p.ids.len() >= limits::MAX_PENDING_IDS && !p.ids.contains(id) {
                p.dropped += 1;
            } else {
                p.ids.insert(*id);
            }
        }
        let start = !p.ids.is_empty() && !p.scheduled;
        p.scheduled |= start;
        start
    }

    /// Take what is pending: the store it was noted over, the ids, and how many more were dropped
    /// for the cap (which the frames are told they missed).
    fn take(pending: &Mutex<Pending>) -> (u64, Vec<i64>, u64) {
        let mut p = lock(pending);
        p.scheduled = false;
        (
            p.epoch,
            std::mem::take(&mut p.ids).into_iter().collect(),
            std::mem::take(&mut p.dropped),
        )
    }

    /// Ids waiting, and ids dropped for the cap since the last take: (landed, deleted).
    #[cfg(test)]
    fn counts(&self) -> ((usize, u64), (usize, u64)) {
        let (l, d) = (lock(&self.landed), lock(&self.deleted));
        ((l.ids.len(), l.dropped), (d.ids.len(), d.dropped))
    }
}

#[derive(Clone, Copy)]
enum Which {
    Landed,
    Deleted,
}

/// Start the timer for one set: when it ends the set is flushed on the blocking pool. The host is
/// moved in, so the flush has what it needs and nothing is held while it waits.
fn schedule<H: Host>(host: &H, which: Which) {
    let host = host.clone();
    let delay = host.hub().delay();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(delay).await;
        let _ = tauri::async_runtime::spawn_blocking(move || match which {
            Which::Landed => drop(flush_landed(&host)),
            Which::Deleted => drop(settle_deleted(&host)),
        })
        .await;
    });
}

// ---- documents.added ----------------------------------------------------------------------------

/// A document joined what a mod may read (a connector brought it, an import or a note or a chat
/// indexed it, or it was taken off the shelf) and its row has committed: tell the frames that may
/// know, after the interval. Cheap when no frame is live. Called from the `DOCUMENT_LANDED` emit
/// points (`ingest.rs`, `connector_sync.rs`) and, through [`note_landed`], from every other writer of
/// a new document and from the un-archive, whether or not the document is in the review queue: a
/// mod mirrors the list, not the queue. A document announced twice (a re-sync) is announced twice;
/// the payload is the document as it is *at the flush* (an archived one is left out,
/// [`documents::added_payloads`]), so a mod keys by id.
pub fn document_landed<H: Host>(host: &H, id: i64) {
    if !host.runtime().any_live() {
        return;
    }
    let epoch = host.store().store_epoch();
    if Hub::note(&host.hub().landed, &[id], epoch) {
        schedule(host, Which::Landed);
    }
}

/// The end of a landing interval: the documents that landed, in slices, as the list's DTO.
pub(super) fn flush_landed<H: Host>(host: &H) -> Emitted {
    let (epoch, ids, lost) = Hub::take(&host.hub().landed);
    let mut total = Emitted::default();
    for slice in ids.chunks(LANDED_SLICE) {
        if host.store().store_epoch() != epoch {
            break;
        }
        // The check above is made with no guard held: the store can be swapped before the emission
        // takes it, so the payload closure (under the guard) asks again, as `settle_deleted` does.
        let said = emit_with(host, DOCUMENTS_ADDED, &Audience::Everyone, |conn| {
            stays_over(host, epoch)?;
            documents::added_payloads(conn, slice)
        });
        total.add(said);
    }
    total.add(say_lost(host, DOCUMENTS_ADDED, epoch, lost));
    total
}

/// Tell the sessions that may hear `name` that they missed `lost` events the hub couldn't hold
/// (nothing, when none, or when the store is no longer the one they were noted over).
fn say_lost<H: Host>(host: &H, name: &'static str, epoch: u64, lost: u64) -> Emitted {
    if lost == 0 {
        return Emitted::default();
    }
    emit_counting(host, name, &Audience::Everyone, lost, |_| {
        stays_over(host, epoch)?;
        Ok(Vec::new())
    })
}

/// Fails unless the store is still the one `epoch` names. Asked inside an emission's payload
/// closure, which runs under the DB guard, so what was noted over one store is never announced
/// over the next.
fn stays_over<H: Host>(host: &H, epoch: u64) -> Result<(), ModError> {
    if host.store().store_epoch() == epoch {
        Ok(())
    } else {
        Err(ModError::session_ended())
    }
}

// ---- documents.deleted --------------------------------------------------------------------------

/// Ids that may have just left what a mod may read (deleted, or archived: a mod's readable set is
/// the documents that aren't archived, [`documents::LISTED`]), noted inside the writer's transaction
/// (so before its commit); announced after the interval if they are out of that set then
/// ([`settle_deleted`]). The writer reports only documents a mod could see ([`is_listed`]).
pub fn documents_deleted<H: Host>(host: &H, ids: &[i64]) {
    if ids.is_empty() || !host.runtime().any_live() {
        return;
    }
    let epoch = host.store().store_epoch();
    if Hub::note(&host.hub().deleted, ids, epoch) {
        schedule(host, Which::Deleted);
    }
}

/// The end of a deletion interval: of the ids noted, the ones a mod can no longer read (no row, or
/// archived), as ids in slices of [`limits::MAX_IDS_PER_EVENT`]. Taking the DB guard waits for any
/// writer still holding it (the one that noted them, mid-transaction), so what it finds is committed
/// or rolled back: a rolled-back delete or archive leaves the document listed and says nothing, and
/// so does an archive that was undone within the interval.
pub(super) fn settle_deleted<H: Host>(host: &H) -> Emitted {
    let (epoch, ids, lost) = Hub::take(&host.hub().deleted);
    let mut total = Emitted::default();
    if ids.is_empty() && lost == 0 {
        return total;
    }
    let gone = {
        let Ok(conn) = host.store().conn() else {
            return total;
        };
        if host.store().store_epoch() != epoch {
            return total;
        }
        match unlisted(&conn, &ids) {
            Ok(gone) => gone,
            Err(e) => {
                eprintln!("mods: couldn't settle deleted documents ({})", e.message);
                return total;
            }
        }
    };
    // The guard is gone: the store can still be swapped before an emission takes it again, which
    // would announce this store's ids over the next. Each emission checks the epoch under its own
    // guard, and sends nothing if it moved.
    for slice in gone.chunks(limits::MAX_IDS_PER_EVENT) {
        let said = emit_with(host, DOCUMENTS_DELETED, &Audience::Everyone, |_| {
            stays_over(host, epoch)?;
            Ok(vec![json!({ "ids": slice })])
        });
        total.add(said);
    }
    total.add(say_lost(host, DOCUMENTS_DELETED, epoch, lost));
    total
}

/// Of `ids`, the ones a mod can't read now (no row in `documents`, or an archived one), in id order.
fn unlisted(conn: &Connection, ids: &[i64]) -> Result<Vec<i64>, ModError> {
    let wanted = serde_json::to_string(ids).map_err(ModError::internal)?;
    let mut stmt = conn
        .prepare(&format!(
            "SELECT value FROM json_each(?1) \
             WHERE value NOT IN (SELECT d.id FROM documents d WHERE {}) ORDER BY value",
            documents::LISTED
        ))
        .map_err(ModError::internal)?;
    let rows = stmt
        .query_map([wanted], |r| r.get::<_, i64>(0))
        .map_err(ModError::internal)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(ModError::internal)
}

/// Whether a mod could see document `id` (it exists and isn't archived). Asked by the deleter just
/// before the row goes, inside its transaction.
pub fn is_listed(conn: &Connection, id: i64) -> bool {
    conn.query_row(
        &format!(
            "SELECT {} FROM documents d WHERE d.id = ?1",
            documents::LISTED
        ),
        [id],
        |r| r.get::<_, bool>(0),
    )
    .unwrap_or(false)
}

/// The ids of every document a mod can see: the rebuild's wipe reads them just before it clears the
/// table.
pub fn listed_ids(conn: &Connection) -> Vec<i64> {
    let sql = format!(
        "SELECT d.id FROM documents d WHERE {} ORDER BY d.id",
        documents::LISTED
    );
    conn.prepare(&sql)
        .and_then(|mut stmt| {
            stmt.query_map([], |r| r.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default()
}

/// What a writer of documents reports, in terms of the set a mod may read (the documents that aren't
/// archived): ids that may have left it, or may have joined it. Both are only *noted* where they
/// happen, possibly inside a transaction that hasn't committed; what is announced is decided at the
/// end of the interval from the committed rows ([`settle_deleted`], [`flush_landed`]), so a
/// rollback announces nothing.
#[derive(Clone, Copy)]
pub enum Note<'a> {
    /// Deleted, or put on the shelf.
    Left(&'a [i64]),
    /// Created, or taken off the shelf.
    Joined(&'a [i64]),
}

type Notify = Arc<dyn for<'a> Fn(Note<'a>) + Send + Sync>;

/// Where [`note_deleted`] and [`note_landed`] report. Set once by `setup()` with the app; empty in a
/// unit test of the code that writes.
static NOTES: Mutex<Option<Notify>> = Mutex::new(None);

/// Report document writes to the app's mod events (`lib.rs` setup).
pub fn install_notes(app: AppHandle) {
    set_notes(Some(Arc::new(move |note| report(&app, note))));
}

/// Hand a note to the hub of `host`.
fn report<H: Host>(host: &H, note: Note<'_>) {
    match note {
        Note::Left(ids) => documents_deleted(host, ids),
        Note::Joined(ids) => {
            for id in ids {
                document_landed(host, *id);
            }
        }
    }
}

#[cfg(test)]
thread_local! {
    static TEST_NOTES: std::cell::RefCell<Option<Notify>> = const { std::cell::RefCell::new(None) };
}

fn set_notes(notify: Option<Notify>) {
    *NOTES.lock().unwrap_or_else(|e| e.into_inner()) = notify;
}

fn send_note(note: Note<'_>) {
    // A test of the code that writes reports to its own host, on its own thread, so one test's
    // writes can never reach another's.
    #[cfg(test)]
    if let Some(notify) = TEST_NOTES.with(|n| n.borrow().clone()) {
        notify(note);
        return;
    }
    let notify = NOTES.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(notify) = notify {
        notify(note);
    }
}

/// What the code under test reported on this thread, for the tests of the writers (which have a
/// store of their own, not a host): the notes they made, in order, whatever they would announce.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Noted {
        Left(Vec<i64>),
        Joined(Vec<i64>),
    }

    /// Records this thread's notes until dropped.
    pub struct Recording(Arc<Mutex<Vec<Noted>>>);

    pub fn record() -> Recording {
        let seen: Arc<Mutex<Vec<Noted>>> = Arc::default();
        let into = Arc::clone(&seen);
        TEST_NOTES.with(|n| {
            *n.borrow_mut() = Some(Arc::new(move |note| {
                into.lock().unwrap().push(match note {
                    Note::Left(ids) => Noted::Left(ids.to_vec()),
                    Note::Joined(ids) => Noted::Joined(ids.to_vec()),
                });
            }));
        });
        Recording(seen)
    }

    impl Recording {
        /// What was noted since the last call.
        pub fn take(&self) -> Vec<Noted> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    impl Drop for Recording {
        fn drop(&mut self) {
            TEST_NOTES.with(|n| *n.borrow_mut() = None);
        }
    }
}

/// The document rows `ids` were deleted (or archived) in the caller's transaction, not yet
/// committed. Never blocks, never touches the DB: it only notes, and the announcement waits for the
/// commit ([`settle_deleted`]). Callers: `ingest::delete_document` and the rebuild's wipe beside it
/// (deletes), and [`note_listing_change`] (an archive).
pub fn note_deleted(ids: &[i64]) {
    send_note(Note::Left(ids));
}

/// The document rows `ids` were created (or taken off the shelf), committed or about to be. Never
/// blocks, never touches the DB: the announcement is made at the end of the interval from the rows
/// as they are then ([`flush_landed`]), so an id whose write rolled back, or that is archived by
/// then, says nothing. Callers: every writer of a new document that isn't one of the two
/// `DOCUMENT_LANDED` emit points (notes, chat births), and [`note_listing_change`] (an un-archive).
pub fn note_landed(ids: &[i64]) {
    send_note(Note::Joined(ids));
}

/// Whether a document with this importance is in a mod's readable set ([`documents::LISTED`] in
/// Rust, for a writer that already holds the old value).
pub fn importance_is_listed(importance: Option<&str>) -> bool {
    importance != Some(documents::ARCHIVE)
}

/// A writer is about to change `id`'s importance (the one column that archives a document): it
/// reads [`is_listed`] first, and calls this after its UPDATE with that answer. An archive
/// (listed, now not) is noted as a deletion and an un-archive (not listed, now is) as a landing, so
/// a mod's readable set stays what its events say. Nothing else (a move between two levels that are
/// both readable, a row that was never readable and still isn't) is news, so an id a mod couldn't
/// read is never announced. Inside the writer's transaction, like every note.
pub fn note_listing_change(conn: &Connection, id: i64, was_listed: bool) {
    match (was_listed, is_listed(conn, id)) {
        (true, false) => note_deleted(&[id]),
        (false, true) => note_landed(&[id]),
        _ => {}
    }
}

// ---- theme.changed ------------------------------------------------------------------------------

/// One string of the theme snapshot, as short as any axis name is.
fn short_text<'a>(theme: &'a Value, key: &str) -> Option<&'a str> {
    theme[key]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 64)
}

/// The theme snapshot PM's page posted, rebuilt from exactly the fields `theme.get` answers with
/// (`modTheme.ts`): custom properties (names that start `--`, text values) and five axes. Anything
/// else in what the page sent is left out, and a snapshot that doesn't have the shape is refused.
fn theme_payload(theme: &Value) -> Option<Value> {
    let vars = theme["vars"].as_object()?;
    let mut clean = serde_json::Map::new();
    for (name, value) in vars {
        if !name.starts_with("--") {
            return None;
        }
        clean.insert(name.clone(), json!(value.as_str()?));
    }
    Some(json!({
        "vars": clean,
        "mode": short_text(theme, "mode")?,
        "system": short_text(theme, "system")?,
        "accent": short_text(theme, "accent")?,
        "colorblind": theme["colorblind"].as_bool()?,
        "depth": short_text(theme, "depth")?,
    }))
}

/// PM's theme changed, as the page of `session`'s frame saw it: tell that frame, if it holds
/// `storage`. The page posts one per frame, so a frame is told its own and never another's (and a
/// frame mounted after a change reads `theme.get`).
pub fn theme_changed<H: Host>(host: &H, session: &str, theme: &Value) -> Emitted {
    let Some(payload) = theme_payload(theme) else {
        return Emitted::default();
    };
    emit_with(
        host,
        THEME_CHANGED,
        &Audience::Session(session.to_owned()),
        |_| Ok(vec![payload]),
    )
}

// ---- a Rebuild ----------------------------------------------------------------------------------

/// A Rebuild over store `epoch` has settled (finished, or failed part way): every live session is
/// sent the resync notice, once, because it re-listed nothing itself and its list may now be
/// anything.
///
/// **Why a notice and not `documents.added` for each document.** On the arm that wipes (a changed
/// embedder width) every document comes back under a new id (`AUTOINCREMENT`), over minutes; on the
/// others the rows are updated in place, with titles, chunk counts and filings that no event
/// describes. Announcing each document would send a mod a flood it must reconcile with a wipe it
/// was already told of (`documents.deleted`, noted by the wipe), and the in-place arm would
/// announce every document of the vault. One notice at the end says what is true: "you missed
/// events, list again", and the list is then right. The notice is the outbox's own (an empty
/// `grants.changed` carrying `missed`), which reveals nothing, so it goes to every running mod,
/// granted `documents.meta` or not. A frame that is not at that store (a swap during the run), a
/// locked PM and a mod that isn't running hear nothing, as for any event.
pub fn documents_resync<H: Host>(host: &H, epoch: u64) -> Emitted {
    emit_counting(host, GRANTS_CHANGED, &Audience::Everyone, 1, |_| {
        stays_over(host, epoch)?;
        Ok(Vec::new())
    })
}

// ---- grants.changed -----------------------------------------------------------------------------

/// The grants of `mods` changed (a decision recorded, an install's consent written), and the change
/// has committed: each is told to re-ask. The payload is empty on purpose: it says only that the
/// answers to "may I?" may differ, never what another mod holds (D3 makes one unsigned mod's grant
/// another's refusal, so naming permissions could leak it). The writers of a grant
/// (`grants::record_install_consent`, `grants::set_decision`) are called by the Mods tab's commands,
/// which call this after their commit, for the mod and, when the change touched a read or an egress
/// permission of an unsigned mod, for every other unsigned mod (`tab::consent::announce`).
pub fn grants_changed<H: Host>(host: &H, mods: &[ModId]) -> Emitted {
    let mut total = Emitted::default();
    let distinct: BTreeSet<&ModId> = mods.iter().collect();
    for id in distinct {
        let said = emit_with(host, GRANTS_CHANGED, &Audience::Mod(id.clone()), |_| {
            Ok(vec![json!({})])
        });
        total.add(said);
    }
    total
}

#[cfg(test)]
mod tests;
