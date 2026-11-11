// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Host API's events (Mods spec §1.5 "Events"): what PM tells a frame, and who may hear it.
//!
//! | Event | Permission | Payload | Source |
//! |---|---|---|---|
//! | `documents.added` | `documents.meta` | `{documents: [...]}`, `documents.list`'s DTO, every field | a document landing (`ingest.rs`, `connector_sync.rs`), coalesced |
//! | `documents.deleted` | `documents.meta` | `{ids: [...]}` | every document delete, after its commit |
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
//! **Coalesced.** A landing or a deletion waits [`limits::EVENT_COALESCE_MS`] for the next, so an
//! import of a thousand documents is a few events, not a thousand. Each [`Hub`] set is bounded
//! ([`limits::MAX_PENDING_IDS`]) and keyed to the store it was noted in: ids noted over one store are
//! never announced over another.
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
                eprintln!("mods: couldn't build the {name} event ({})", e.message);
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
    /// Ids dropped past [`limits::MAX_PENDING_IDS`], in all.
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

    /// Take what is pending, with the store it was noted over.
    fn take(pending: &Mutex<Pending>) -> (u64, Vec<i64>) {
        let mut p = lock(pending);
        p.scheduled = false;
        (p.epoch, std::mem::take(&mut p.ids).into_iter().collect())
    }

    /// Ids waiting, and ids dropped for the cap, in all: (landed, deleted).
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

/// A document landed (a connector brought it, or an import indexed it) and its row has committed:
/// tell the frames that may know, after the interval. Cheap when no frame is live. Called from the
/// existing `DOCUMENT_LANDED` emit points (`ingest.rs`, `connector_sync.rs`), whether or not the
/// document is in the review queue: a mod mirrors the list, not the queue. A document announced
/// twice (a re-sync) is announced twice; the payload is the document as it is, so a mod keys by id.
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
    let (epoch, ids) = Hub::take(&host.hub().landed);
    let mut total = Emitted::default();
    for slice in ids.chunks(LANDED_SLICE) {
        if host.store().store_epoch() != epoch {
            break;
        }
        let said = emit_with(host, DOCUMENTS_ADDED, &Audience::Everyone, |conn| {
            documents::added_payloads(conn, slice)
        });
        total.sent += said.sent;
        total.dropped += said.dropped;
        total.refused += said.refused;
    }
    total
}

// ---- documents.deleted --------------------------------------------------------------------------

/// Ids that may have just been deleted, noted inside the deleter's transaction (so before its
/// commit); announced after the interval if they are gone then ([`settle_deleted`]). The deleter
/// reports only documents a mod could see ([`is_listed`]).
pub fn documents_deleted<H: Host>(host: &H, ids: &[i64]) {
    if ids.is_empty() || !host.runtime().any_live() {
        return;
    }
    let epoch = host.store().store_epoch();
    if Hub::note(&host.hub().deleted, ids, epoch) {
        schedule(host, Which::Deleted);
    }
}

/// The end of a deletion interval: of the ids noted, the ones no longer in `documents`, as ids in
/// slices of [`limits::MAX_IDS_PER_EVENT`]. Taking the DB guard waits for any writer still holding it
/// (the one that noted them, mid-transaction), so what it finds is committed or rolled back.
pub(super) fn settle_deleted<H: Host>(host: &H) -> Emitted {
    let (epoch, ids) = Hub::take(&host.hub().deleted);
    let mut total = Emitted::default();
    if ids.is_empty() {
        return total;
    }
    let gone = {
        let Ok(conn) = host.store().conn() else {
            return total;
        };
        if host.store().store_epoch() != epoch {
            return total;
        }
        match absent(&conn, &ids) {
            Ok(gone) => gone,
            Err(e) => {
                eprintln!("mods: couldn't settle deleted documents ({})", e.message);
                return total;
            }
        }
    };
    for slice in gone.chunks(limits::MAX_IDS_PER_EVENT) {
        let said = emit_with(host, DOCUMENTS_DELETED, &Audience::Everyone, |_| {
            Ok(vec![json!({ "ids": slice })])
        });
        total.sent += said.sent;
        total.dropped += said.dropped;
        total.refused += said.refused;
    }
    total
}

/// Of `ids`, the ones with no row in `documents`, in id order.
fn absent(conn: &Connection, ids: &[i64]) -> Result<Vec<i64>, ModError> {
    let wanted = serde_json::to_string(ids).map_err(ModError::internal)?;
    let mut stmt = conn
        .prepare(
            "SELECT value FROM json_each(?1) \
             WHERE value NOT IN (SELECT id FROM documents) ORDER BY value",
        )
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

type Notify = Arc<dyn Fn(&[i64]) + Send + Sync>;

/// Where [`note_deleted`] reports. Set once by `setup()` with the app; empty in a unit test of the
/// code that deletes.
static DELETE_NOTES: Mutex<Option<Notify>> = Mutex::new(None);

/// Report document deletions to the app's mod events (`lib.rs` setup).
pub fn install_delete_notes(app: AppHandle) {
    set_delete_notes(Some(Arc::new(move |ids| documents_deleted(&app, ids))));
}

#[cfg(test)]
thread_local! {
    static TEST_NOTES: std::cell::RefCell<Option<Notify>> = const { std::cell::RefCell::new(None) };
}

fn set_delete_notes(notify: Option<Notify>) {
    *DELETE_NOTES.lock().unwrap_or_else(|e| e.into_inner()) = notify;
}

/// The document rows `ids` were deleted in the caller's transaction, not yet committed. Never
/// blocks, never touches the DB: it only notes, and the announcement waits for the commit
/// ([`settle_deleted`]). The one caller is `ingest::delete_document` and the rebuild's wipe beside it.
pub fn note_deleted(ids: &[i64]) {
    // A test of the code that deletes reports to its own host, on its own thread, so one test's
    // deletions can never reach another's.
    #[cfg(test)]
    if let Some(notify) = TEST_NOTES.with(|n| n.borrow().clone()) {
        notify(ids);
        return;
    }
    let notify = DELETE_NOTES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if let Some(notify) = notify {
        notify(ids);
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

// ---- grants.changed -----------------------------------------------------------------------------

/// The grants of `mods` changed (a decision recorded, an install's consent written), and the change
/// has committed: each is told to re-ask. The payload is empty on purpose: it says only that the
/// answers to "may I?" may differ, never what another mod holds (D3 makes one unsigned mod's grant
/// another's refusal, so naming permissions could leak it). The writers of a grant
/// (`grants::record_install_consent`, `grants::set_decision`) have no caller yet; slice 6's commands
/// call this after their commit, for the mod and, when the grant was a read or an egress one of an
/// unsigned mod, for every other unsigned mod.
#[cfg_attr(not(test), allow(dead_code))] // the Mods tab's prompts (slice 6)
pub fn grants_changed<H: Host>(host: &H, mods: &[ModId]) -> Emitted {
    let mut total = Emitted::default();
    let distinct: BTreeSet<&ModId> = mods.iter().collect();
    for id in distinct {
        let said = emit_with(host, GRANTS_CHANGED, &Audience::Mod(id.clone()), |_| {
            Ok(vec![json!({})])
        });
        total.sent += said.sent;
        total.dropped += said.dropped;
        total.refused += said.refused;
    }
    total
}

#[cfg(test)]
mod tests;
