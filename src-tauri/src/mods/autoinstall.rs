// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The pilots' auto-install (Mods spec §5, D11, D17; PR B plan B8): a mod that PM itself moved out
//! of core (the Map first) is installed once for a user who had the feature, without a click, the
//! first time PM can fetch it.
//!
//! **A marker starts it.** The pass acts only on a marker row in `settings` (`mods_autoinstall_map`
//! and its kind), which a migration writes for a store that used the feature, and which a restore
//! writes for a store whose backup holds the pilot's catalogue row ([`mark_restored`]: the mod's
//! files are per computer, so a restore onto another one owes them). Nothing else in PM writes a
//! marker, so a store without one is never touched: no download, no row, no listing entry.
//!
//! **What a pass does**, per marker present (one transaction each, under the ledger lock):
//! - the mod already has a catalogue row (the user installed it, or an earlier pass did): settle it,
//!   which is the handover and the marker's removal, with no download;
//! - no row, and the cached index is *current* and lists the id as a first-party entry this PM can
//!   serve and the blacklist doesn't name: download that file ([`updater`]'s pinned, capped,
//!   sha256-checked fetch) and install it unconsented ([`super::ledger::Ledger::land_pilot`]); the
//!   marker's removal and the handover run in the same transaction as the row, so they land with it
//!   or not at all;
//! - the mod has a catalogue row whose files are missing on this computer (a restore brought the
//!   row, not the files; whatever state the row is in, paused included, because the files decide):
//!   with a current index that doesn't blacklist the row's version or file, put back exactly the
//!   pinned version and file, the way the Mods tab's repair does
//!   ([`super::ledger::Ledger::repair_target`]), then settle. It is the same file the person's own
//!   click on Repair would fetch, signature and sha256 checked, never a newer or unsigned one;
//! - otherwise leave the marker, and the Mods tab shows the mod as "Installing" (a synthesized
//!   entry, never a row: a fake row in a truth-class table would ride in backups and shared vaults
//!   with an invented hash and version).
//!
//! **Only the writer installs** ([`crate::lock_session::this_instance_writes`]): a read-only guest of
//! a shared vault has its store closed, and a mod installed there would be files the writer's ledger
//! never heard of. **No download without a usable, current index** ([`decide`]): an expired or missing
//! index may restrict (the blacklist applies elsewhere), never install. Putting back the files of a
//! row the person already has is no exception: it fetches the row's own pinned, hash-checked file,
//! but the row came from a restore (a backup is data a person may have been handed), so it too waits
//! for a current index and is left when that index's blacklist names the row's version or file.
//!
//! **The handover** moves what the core feature kept in the store into the mod's own storage, then
//! deletes the old rows, verify-then-delete: the mod's `settings` key is read back before any old
//! row goes, and an error anywhere rolls the whole install back, leaving the old rows and the marker
//! for the next pass. What only the page can read (the per-device `pm.map.*` keys) is handed over by
//! [`adopt_map_prefs`], which the page calls while the flag is set; the page keeps its keys, because
//! the mod's storage is per vault and every other vault on the device is owed them too.

use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Manager};

use super::fetch::{fetch_package, Endpoints, ReqwestTransport, Transport};
use super::ids::ModId;
use super::index::Index;
use super::index_state;
use super::keys::TrustedKeys;
use super::ledger::{self, LedgerError, Mint, PilotLanded};
use super::pinboard_move;
use super::policy::{self, ApiFit, InstallKind, MOD_API_OLDEST_MAJOR, MOD_API_VERSION};
use super::refresh;
use super::safety;
use super::store;
use super::updater::Download;
use crate::db;
use crate::doc_vectors::{NODE_CAP_MAX, NODE_CAP_MIN};
use crate::AppState;

/// A mod that auto-installs for the users who had its feature in core.
pub struct Pilot {
    /// The mod's id.
    pub id: &'static str,
    /// The `settings` key whose presence says "install this one for me".
    pub marker: &'static str,
    /// What the mod is called while it has no row.
    pub name: &'static str,
    /// The sidebar tab it takes over.
    pub tab_label: &'static str,
    /// The `settings` key that records, once the install landed, that this vault's device-local
    /// leftovers (the sidebar's placement) may now be carried over. Kept: it is one short row.
    pub landed: &'static str,
    /// A second `settings` key that says the feature's data is still owed to the mod, besides the
    /// marker: the Pinboard's `pinboard_move_pending` (a board exists that has not been copied). A
    /// manual install from the Mods tab settles a pilot when either is there ([`finish_manual`]).
    /// A pilot with one runs its handover on every settle, whether or not `landed` is set: the key
    /// is its own once-only guard ([`settle`]). The Map has none, and keeps the landed note's gate.
    pub pending: Option<&'static str>,
    /// Runs in the install's transaction, after the row: carries the core feature's rows over.
    handover: fn(&Connection, &str) -> Result<(), LedgerError>,
}

/// The compiled table. A marker for an id that isn't here is ignored, and nothing outside this
/// table can name a pilot.
pub const PILOTS: [Pilot; 2] = [
    Pilot {
        id: "atlas.map",
        marker: "mods_autoinstall_map",
        name: "Document Map",
        tab_label: "Document Map",
        landed: "mods_pilot_landed_map",
        pending: None,
        handover: hand_over_map,
    },
    // The Pinboard (PR C). Awake since v62, which writes `mods_autoinstall_pinboard` for a store whose
    // board has a widget or that holds a `note:` document, and `pinboard_move_pending` for a store
    // whose board is not blank. `mark_restored` can also write the marker, for a restored store with
    // a signed catalogue `atlas.pinboard` row, and that store has no pending move. The move itself
    // (`pinboard_move`) runs only when the pending key is there.
    Pilot {
        id: "atlas.pinboard",
        marker: "mods_autoinstall_pinboard",
        name: "Pinboard",
        tab_label: "Pinboard",
        landed: "mods_pilot_landed_pinboard",
        pending: Some(pinboard_move::PENDING),
        handover: pinboard_move::hand_over_pinboard,
    },
];

/// The pilots whose marker is in the store, in table order.
pub fn pending(conn: &Connection) -> rusqlite::Result<Vec<&'static Pilot>> {
    let mut out = Vec::new();
    for p in &PILOTS {
        if has(conn, p.marker)? {
            out.push(p);
        }
    }
    Ok(out)
}

/// A restore's step ([`crate::backup::adopt_mods::reset_for_adopt`]): a store restored with a
/// pilot's signed catalogue row is owed that mod's files on this computer, which the backup can't
/// carry, so the marker is written and the next pass settles the row or puts its files back.
/// Nothing is written for a store without the row: a mod the person removed stays removed.
pub fn mark_restored(conn: &Connection) -> rusqlite::Result<()> {
    for p in &PILOTS {
        conn.execute(
            "INSERT OR IGNORE INTO settings(key, value) \
             SELECT ?1, '1' WHERE EXISTS ( \
                 SELECT 1 FROM mods WHERE id = ?2 AND installed_from = ?3 AND signed = 1)",
            params![p.marker, p.id, store::FROM_CATALOGUE],
        )?;
    }
    Ok(())
}

/// The text of a `settings` row, `None` for a missing row and for one that holds another type (a
/// BLOB, an integer, bytes that are not UTF-8): a shared vault's member can plant those, and a typed
/// read of one is an error that would stop the Map's handover and so its install. A value that can't
/// be read carries nothing; only a database that failed is an `Err`.
fn get(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    match conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
            r.get(0)
        })
        .optional()
    {
        Err(e) if super::refs::is_wrong_type(&e) => Ok(None),
        other => other,
    }
}

fn to_ledger(e: crate::error::Error) -> LedgerError {
    LedgerError::Store(e.to_string())
}

// ---- The decision (pure) ----------------------------------------------------------------------

/// What the pass reads of a pilot's existing row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Held<'a> {
    pub installed_from: &'a str,
    pub signed: bool,
    /// The row's files aren't on this computer (a restore brought the row), whatever its state.
    pub files_missing: bool,
    /// The row's pinned version and file, which a repair would fetch: the blacklist is read against
    /// them before it does.
    pub version: &'a str,
    pub sha256: Option<&'a str>,
}

/// Why a marker is left for a later pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leave {
    /// No cached index, or it is past its life: it may restrict, it may not install.
    NoCurrentIndex,
    NotListed,
    Blacklisted,
    NeedsNewerPm,
    ApiRetired,
    /// A row with this id exists but isn't a catalogue install (a developer's test folder, a file):
    /// nothing installs onto it and its storage isn't ours to move things into.
    OtherRow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// It is installed: do the handover and clear the marker, download nothing.
    Settle,
    /// It is installed but its files aren't here: put back the pinned version and file, then settle.
    Repair,
    Download(Download),
    Leave(Leave),
}

/// What to do for `pilot` (pure). `held` is its row, if any; `index` the cached one.
pub fn decide(
    pilot: &Pilot,
    held: Option<Held<'_>>,
    index: Option<&Index>,
    now: DateTime<Utc>,
) -> Step {
    if let Some(h) = held {
        if h.installed_from != store::FROM_CATALOGUE || !h.signed {
            return Step::Leave(Leave::OtherRow);
        }
        if !h.files_missing {
            return Step::Settle;
        }
        // The version and file come from the row, which a restore may have brought from anywhere:
        // like a download, a repair waits for a current index and for the blacklist to clear them.
        let Some(index) = index.filter(|i| i.current_at(now)) else {
            return Step::Leave(Leave::NoCurrentIndex);
        };
        return if index.blacklisted(pilot.id, h.version, h.sha256).is_some() {
            Step::Leave(Leave::Blacklisted)
        } else {
            Step::Repair
        };
    }
    let Some(index) = index.filter(|i| i.current_at(now)) else {
        return Step::Leave(Leave::NoCurrentIndex);
    };
    let Some(entry) = index.entry(pilot.id) else {
        return Step::Leave(Leave::NotListed);
    };
    // No tier check: the index format refuses an `atlas.` id that isn't first party (`index.rs`),
    // so an entry for a pilot is first party by the time it is parsed.
    if index
        .blacklisted(pilot.id, entry.version.as_str(), Some(&entry.sha256))
        .is_some()
    {
        return Step::Leave(Leave::Blacklisted);
    }
    match policy::api_fit(entry.api, MOD_API_VERSION, MOD_API_OLDEST_MAJOR) {
        ApiFit::NeedsNewerPm => return Step::Leave(Leave::NeedsNewerPm),
        ApiFit::Retired => return Step::Leave(Leave::ApiRetired),
        ApiFit::Fits | ApiFit::OlderMajor => {}
    }
    Step::Download(Download {
        id: pilot.id.to_owned(),
        version: entry.version.as_str().to_owned(),
        url: entry.url.clone(),
        sha256: entry.sha256.clone(),
    })
}

// ---- What runs in the install's transaction ------------------------------------------------------

/// Whether `settings` has a row for `key`, whatever its value holds (a shared vault's member can
/// plant a BLOB, which a typed read would turn into an error that blocks every install).
pub(super) fn has(conn: &Connection, key: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM settings WHERE key = ?1)",
        [key],
        |r| r.get(0),
    )
}

/// The pilot's marker, its handover and its landed note, in the caller's transaction. Fails (so the
/// transaction rolls back and the marker stays) when the mod's row isn't there, and when the marker
/// isn't there either: a pass plans against one store, then waits on a download, and "the active
/// store" at the end may be another vault (a switch or a lock/unlock meanwhile). The marker, read
/// in this very transaction, is what says *this* store asked for the install; without it nothing
/// is installed here and no settings are touched.
pub(crate) fn finish(conn: &Connection, pilot: &Pilot) -> Result<(), LedgerError> {
    if !has(conn, pilot.marker)? {
        return Err(policy::Refusal::Changed.into());
    }
    settle(conn, pilot)
}

/// A manual install from the Mods tab (the person's own click on a catalogue mod), in the install's
/// own transaction: for a pilot's id, when its marker or its pending key is in the store, the same
/// handover runs as for an auto-install, before the row commits. Without it the Pinboard's frame
/// could start (the moment the row commits and is turned on) and write its own empty `board` before
/// any pass settled the move, and the move would then find a board in the mod and have to keep both
/// (`pinboard_move`, step 4). Nothing happens for any other mod, for a store owed nothing, for an
/// update or a repair, or for a file the person picked (it is no catalogue install, and no pass would
/// settle it either: [`decide`] leaves such a row alone).
pub(crate) fn finish_manual(
    conn: &Connection,
    mod_id: &str,
    kind: InstallKind,
    from_catalogue: bool,
) -> Result<(), LedgerError> {
    if kind != InstallKind::Fresh || !from_catalogue {
        return Ok(());
    }
    let Some(pilot) = PILOTS.iter().find(|p| p.id == mod_id) else {
        return Ok(());
    };
    let owed = has(conn, pilot.marker)?
        || match pilot.pending {
            Some(key) => has(conn, key)?,
            None => false,
        };
    if !owed {
        return Ok(());
    }
    settle(conn, pilot)
}

/// [`finish`] without the marker's say-so: the mod's row must be here; the handover runs once per
/// vault; the marker (if there is one) goes after it and the landed note is written.
fn settle(conn: &Connection, pilot: &Pilot) -> Result<(), LedgerError> {
    let row: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM mods WHERE id = ?1)",
        [pilot.id],
        |r| r.get(0),
    )?;
    if !row {
        return Err(LedgerError::NotInstalled);
    }
    // The handover is once per vault for a pilot with no pending key of its own: a restore writes
    // the marker again for a vault that landed before (the files are owed, not the rows), and its
    // core rows and device prefs were dealt with. A pilot WITH a pending key (the Pinboard) is not
    // gated on the landed note at all: its handover does nothing without that key, and the key is
    // the only thing that says a copy is owed. The note survives a backup (`settings` is never
    // stripped), so a store restored after the move, or settled once with nothing owed, still holds
    // it when a later re-arm writes the key again (the restore of a mod-data-off backup brings the
    // old board back this way), and a gate on the note would delete the marker and skip the move.
    if pilot.pending.is_some() || !has(conn, pilot.landed)? {
        (pilot.handover)(conn, pilot.id)?;
    }
    // The marker goes after the handover, in the same transaction: a failure above keeps both.
    db::delete_setting(conn, pilot.marker).map_err(to_ledger)?;
    db::set_setting(conn, pilot.landed, "1").map_err(to_ledger)?;
    Ok(())
}

/// The core Map's rows in `settings`, dead once the Map is a mod. The first is the webview-written
/// pref blob (`{nodeCap?, tsneEnabled?}`, both carried: see [`hand_over_map`]); the others are the layout caches.
const MAP_PREF: &str = "map";
const MAP_DEAD_KEYS: [&str; 3] = [MAP_PREF, "doc_layout_meta", "project_layout_cache"];
/// Set by the Map's handover: this vault's device-local view prefs are still on the device that did
/// the install (the page hands them over, [`adopt_map_prefs`]).
pub const ADOPT_FLAG: &str = "mods_adopt_map_prefs";
/// The Map mod's one content key (`mods/atlas.map/src/settings.ts`).
const MAP_KV_KEY: &str = "settings";

/// The Map's handover: what a mod can keep of `settings.map` becomes the mod's `settings` key,
/// content class, revision 1, unless the mod already has one (the user's data in the mod wins); the
/// mod's key is read back; then the dead rows go. Two fields carry:
/// - `nodeCap`, clamped as the core clamped it;
/// - `tsneEnabled: false`, as `enhanced: false` (the user turned the core's t-SNE off, so the mod's
///   Enhanced layout starts off for them too). A `true` or an absent `tsneEnabled` carries nothing:
///   the core read absent as on, and its Settings wrote both fields on every save, so a stored
///   `true` says nothing about who had openTSNE. Anything that is not a JSON boolean carries nothing.
///
/// It does not read the venv or the `.pm-tsne` marker: the venv is per machine and this runs per
/// vault. A pref blob that doesn't parse, has nothing valid in it, or holds another SQLite type than
/// text (planted by a shared vault's member, see [`get`]) carries nothing over and is still deleted:
/// the core reader treated it as the default too. It never makes the handover fail.
fn hand_over_map(conn: &Connection, mod_id: &str) -> Result<(), LedgerError> {
    let old = get(conn, MAP_PREF)?;
    let old = old
        .as_deref()
        .and_then(|text| serde_json::from_str::<Value>(text).ok());
    let mut carried = Map::new();
    if let Some(cap) = old
        .as_ref()
        .and_then(|v| v.get("nodeCap").and_then(Value::as_u64))
        .map(|n| n.clamp(NODE_CAP_MIN as u64, NODE_CAP_MAX as u64))
    {
        carried.insert("nodeCap".to_owned(), json!(cap));
    }
    if old
        .as_ref()
        .is_some_and(|v| v.get("tsneEnabled") == Some(&Value::Bool(false)))
    {
        carried.insert("enhanced".to_owned(), json!(false));
    }
    if !carried.is_empty() {
        let inserted = conn.execute(
            "INSERT OR IGNORE INTO mod_kv(mod_id, key, class, value, revision) \
             VALUES (?1, ?2, 'content', ?3, 1)",
            params![mod_id, MAP_KV_KEY, Value::Object(carried).to_string()],
        )?;
        // Verify before anything is deleted, but only our own row: when the insert was ignored the
        // mod already has a `settings` row, the user's data wins whatever it holds (a BLOB or
        // non-UTF-8 value planted by a shared vault's member included, which a typed read would
        // fail on and so block the install), and there is nothing of ours to check.
        if inserted > 0 {
            let stored = conn
                .query_row(
                    "SELECT value FROM mod_kv WHERE mod_id = ?1 AND key = ?2",
                    params![mod_id, MAP_KV_KEY],
                    |r| r.get::<_, String>(0),
                )
                .optional()?;
            let ok = stored
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .is_some_and(|v| v.is_object());
            if !ok {
                return Err(LedgerError::Store(
                    "the Map's settings weren't stored, so the old ones were kept".to_owned(),
                ));
            }
        }
    }
    for key in MAP_DEAD_KEYS {
        db::delete_setting(conn, key).map_err(to_ledger)?;
    }
    db::set_setting(conn, ADOPT_FLAG, "1").map_err(to_ledger)?;
    Ok(())
}

/// Delete the core Map's dead rows ([`MAP_DEAD_KEYS`], and nothing else in `settings`) from a store
/// that is not owed the Map: the launch pass's sweep. Only the Map's handover deleted them, and
/// migration v61 writes the Map's marker only for a store that holds a document, so a store with none
/// kept the three rows for ever. With the marker pending the rows stay until the handover, which
/// reads `map` for the two prefs it carries. Idempotent. Returns how many rows went.
pub fn sweep_map_leftovers(conn: &Connection) -> rusqlite::Result<usize> {
    let map = PILOTS
        .iter()
        .find(|p| p.id == "atlas.map")
        .map_or("", |p| p.marker);
    if map.is_empty() || has(conn, map)? {
        return Ok(0);
    }
    let mut gone = 0;
    for key in MAP_DEAD_KEYS {
        gone += conn.execute("DELETE FROM settings WHERE key = ?1", [key])?;
    }
    Ok(gone)
}

// ---- The page's half: the device-local prefs and the sidebar -----------------------------------

/// What the page asks once it has the rows: which pilots landed in this vault, and whether the
/// Map's device-local prefs are still waiting to be handed over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoverState {
    pub landed: Vec<String>,
    pub adopt_map_prefs: bool,
    /// The Pinboard's three device prefs wait to be handed over ([`pinboard_move::adopt_pinboard_prefs`]).
    pub adopt_pinboard_prefs: bool,
}

pub fn handover_state(conn: &Connection) -> rusqlite::Result<HandoverState> {
    let mut landed = Vec::new();
    for p in &PILOTS {
        if has(conn, p.landed)? {
            landed.push(p.id.to_owned());
        }
    }
    Ok(HandoverState {
        landed,
        adopt_map_prefs: has(conn, ADOPT_FLAG)?,
        adopt_pinboard_prefs: has(conn, pinboard_move::ADOPT_FLAG)?,
    })
}

/// Why the device-local prefs weren't handed over.
#[derive(Debug)]
pub enum AdoptRefusal {
    /// The arguments aren't what the Map's settings allow (the page is untrusted input).
    BadValue,
    /// The Map isn't installed from the catalogue in this store.
    NotInstalled,
    Db(rusqlite::Error),
}

impl From<rusqlite::Error> for AdoptRefusal {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e)
    }
}

/// Merge the page's device-local Map prefs into the mod's `settings` key, only into fields it doesn't
/// have yet (once the user has changed anything in the mod it holds all four, and nothing here
/// overwrites that), then clear the flag. `Ok(false)` when there was nothing to hand over (another
/// device did it first): the page keeps its keys either way. Nothing is written without the flag
/// the handover sets, so this is dormant until a pilot has landed.
pub fn adopt_map_prefs(
    conn: &mut Connection,
    layout_mode: &str,
    cohesion: f64,
    labels: bool,
) -> Result<bool, AdoptRefusal> {
    if !matches!(layout_mode, "project" | "semantic") || !cohesion.is_finite() {
        return Err(AdoptRefusal::BadValue);
    }
    let cohesion = cohesion.clamp(0.0, 0.5);
    let tx = conn.transaction()?;
    if !has(&tx, ADOPT_FLAG)? {
        return Ok(false);
    }
    // A test folder's rows are dropped at the next start, and its storage with them.
    let from_catalogue: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM mods WHERE id = 'atlas.map' AND installed_from = ?1 AND signed = 1)",
        [store::FROM_CATALOGUE],
        |r| r.get(0),
    )?;
    if !from_catalogue {
        return Err(AdoptRefusal::NotInstalled);
    }
    // The value is read as a SQLite value: a BLOB or non-UTF-8 text planted by a shared vault's
    // member reads as "not an object" (like any other non-object), not as an error that would
    // keep the flag set for ever.
    let existing: Option<(Option<String>, i64)> = tx
        .query_row(
            "SELECT value, revision FROM mod_kv WHERE mod_id = 'atlas.map' AND key = ?1",
            [MAP_KV_KEY],
            |r| {
                let text = match r.get::<_, String>(0) {
                    Ok(t) => Some(t),
                    Err(e) if super::refs::is_wrong_type(&e) => None,
                    Err(e) => return Err(e),
                };
                Ok((text, r.get(1)?))
            },
        )
        .optional()?;
    let (mut object, revision) = match existing {
        Some((text, revision)) => (
            match text.and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
                Some(Value::Object(o)) => o,
                _ => Map::new(),
            },
            revision,
        ),
        None => (Map::new(), 0),
    };
    let mut changed = false;
    for (key, value) in [
        ("layoutMode", json!(layout_mode)),
        ("cohesion", json!(cohesion)),
        ("labels", json!(labels)),
    ] {
        if !object.contains_key(key) {
            object.insert(key.to_owned(), value);
            changed = true;
        }
    }
    if changed {
        tx.execute(
            "INSERT INTO mod_kv(mod_id, key, class, value, revision) \
             VALUES ('atlas.map', ?1, 'content', ?2, ?3) \
             ON CONFLICT(mod_id, key) DO UPDATE SET value = excluded.value, \
               revision = excluded.revision, \
               updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')",
            params![MAP_KV_KEY, Value::Object(object).to_string(), revision + 1],
        )?;
    }
    tx.execute("DELETE FROM settings WHERE key = ?1", [ADOPT_FLAG])?;
    tx.commit()?;
    Ok(true)
}

// ---- The pass -------------------------------------------------------------------------------------

/// One pass at a time: the background refresher, the post-unlock pass and the "Try now" button can
/// all start one, and two installing the same pilot would only fight over its files.
static RUNNING: AtomicBool = AtomicBool::new(false);

struct Running;

impl Running {
    fn claim() -> Option<Self> {
        RUNNING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::Release);
    }
}

/// Run `f` off the async runtime (the store and the files). `None` when the thread panicked.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| eprintln!("mods: an auto-install task failed ({e})"))
        .ok()
}

/// Whether this instance writes the open store right now ([`crate::lock_session`]): a guest of a
/// shared vault has no open store, but this says it in so many words, at every step.
fn writer_now(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    crate::lock_session::this_instance_writes(state.inner())
}

/// Whether the open store holds any pilot's marker. The guard is dropped before any ledger is taken.
fn any_marker(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    let Ok(conn) = state.conn() else {
        return false;
    };
    pending(&conn).is_ok_and(|p| !p.is_empty())
}

fn plan_now(
    app: &AppHandle,
    index: Option<&Index>,
    now: DateTime<Utc>,
) -> Vec<(&'static Pilot, Step)> {
    if !writer_now(app) || !any_marker(app) {
        return Vec::new();
    }
    // A pilot needs its store's mods folder, so this is the one place that mints it for one.
    ledger::with_active_ledger(app, Mint::Always, |l| l.pilot_plan(index, now))
        .map(|plan| {
            plan.unwrap_or_else(|e| {
                eprintln!("mods: pilots not planned ({e})");
                Vec::new()
            })
        })
        .unwrap_or_default()
}

/// One pass over the markers present, with the index in force (`None`: none was ever accepted, so
/// nothing is downloaded). Quiet when there is no marker, when this instance isn't the writer, or
/// when another pass is running. Tells the page once, if anything landed.
pub async fn run<T: Transport>(
    app: &AppHandle,
    transport: &T,
    endpoints: &Endpoints,
    index: Option<&Index>,
    keys: &TrustedKeys,
) {
    let Some(_running) = Running::claim() else {
        return;
    };
    let now = Utc::now();
    let plan = {
        let (app, index) = (app.clone(), index.cloned());
        blocking(move || plan_now(&app, index.as_ref(), now))
            .await
            .unwrap_or_default()
    };
    let mut changed = false;
    for (pilot, step) in plan {
        match step {
            Step::Leave(why) => eprintln!("mods: {} stays installing ({why:?})", pilot.id),
            Step::Settle => {
                let app2 = app.clone();
                let done = blocking(move || {
                    writer_now(&app2)
                        .then(|| {
                            ledger::with_active_ledger(&app2, Mint::Always, |l| {
                                l.settle_pilot(pilot)
                            })
                        })
                        .flatten()
                })
                .await
                .flatten();
                match done {
                    Some(Ok(())) => changed = true,
                    Some(Err(e)) => eprintln!("mods: {} not settled ({e})", pilot.id),
                    None => {}
                }
            }
            Step::Repair => {
                let app2 = app.clone();
                let target = blocking(move || {
                    writer_now(&app2)
                        .then(|| {
                            ledger::with_active_ledger(&app2, Mint::Always, |l| {
                                ModId::parse(pilot.id)
                                    .map_err(|e| LedgerError::Store(e.to_string()))
                                    .and_then(|id| l.repair_target(&id))
                            })
                        })
                        .flatten()
                })
                .await
                .flatten();
                let target = match target {
                    Some(Ok(t)) => t,
                    Some(Err(e)) => {
                        eprintln!("mods: {} can't be put back ({e})", pilot.id);
                        continue;
                    }
                    None => continue,
                };
                // The row's own file: its sha256 must match before anything reads it, and the
                // ledger's repair accepts nothing else.
                let bytes =
                    match fetch_package(transport, endpoints, &target.url, &target.sha256).await {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            if !e.is_quiet() {
                                eprintln!("mods: {} not downloaded: {e}", pilot.id);
                            }
                            continue;
                        }
                    };
                let (app2, index2, keys2) = (app.clone(), index.cloned(), keys.clone());
                let done = blocking(move || {
                    writer_now(&app2)
                        .then(|| {
                            ledger::with_active_ledger(&app2, Mint::Always, |l| {
                                l.repair_pilot(
                                    pilot,
                                    &target,
                                    &bytes,
                                    &keys2,
                                    index2.as_ref(),
                                    Utc::now(),
                                )
                            })
                        })
                        .flatten()
                })
                .await
                .flatten();
                match done {
                    Some(Ok(())) => {
                        eprintln!("mods: {} put back for you", pilot.id);
                        changed = true;
                    }
                    Some(Err(e)) => eprintln!("mods: {} not put back ({e})", pilot.id),
                    None => {}
                }
            }
            Step::Download(download) => {
                // `decide` only says Download for a current index, so `index` is there.
                let Some(index) = index else { continue };
                let bytes = match fetch_package(
                    transport,
                    endpoints,
                    &download.url,
                    &download.sha256,
                )
                .await
                {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        if !e.is_quiet() {
                            eprintln!("mods: {} not downloaded: {e}", pilot.id);
                        }
                        continue;
                    }
                };
                let (app2, index2, keys2) = (app.clone(), index.clone(), keys.clone());
                let landed = blocking(move || {
                    writer_now(&app2)
                        .then(|| {
                            ledger::with_active_ledger(&app2, Mint::Always, |l| {
                                l.land_pilot(pilot, &download, &bytes, &keys2, &index2, Utc::now())
                            })
                        })
                        .flatten()
                })
                .await
                .flatten();
                match landed {
                    Some(PilotLanded::Installed(i)) => {
                        eprintln!("mods: {} installed {} for you", pilot.id, i.version);
                        changed = true;
                    }
                    Some(PilotLanded::Failed(why)) => {
                        eprintln!("mods: {} not installed: {why}", pilot.id);
                    }
                    None => {}
                }
            }
        }
    }
    if changed {
        refresh::emit_changed(app, refresh::CHANGED_AUTOINSTALLED);
    }
}

/// A pass with the index this machine already holds, for after the store opens (and when a fetch
/// couldn't be made): offline, a current cached index still installs.
pub async fn run_cached(app: &AppHandle) {
    // Everything that reads a file or the store is off the async runtime, and nothing is built (no
    // HTTP client, no index read) for a store with no marker, which is every store but a pilot's.
    let app2 = app.clone();
    let held = blocking(move || {
        if !writer_now(&app2) || !any_marker(&app2) {
            return None;
        }
        let data_dir = crate::paths::data_dir(&app2).ok()?;
        let loaded = index_state::load(&data_dir, safety::ring());
        let keys = safety::trusted_keys(&loaded.state);
        Some((keys, loaded.index.map(|v| v.index)))
    })
    .await
    .flatten();
    let Some((keys, index)) = held else {
        return;
    };
    let endpoints = Endpoints::machine();
    let transport = ReqwestTransport::new(&endpoints);
    run(app, &transport, &endpoints, index.as_ref(), &keys).await;
}

/// After a writer's store opens: clear what the core Map left behind, then start a pass in the
/// background. Both are cheap when no marker is there.
pub fn spawn_after_open(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let app2 = app.clone();
        blocking(move || {
            if !writer_now(&app2) {
                return;
            }
            let state = app2.state::<AppState>();
            let Ok(conn) = state.conn() else { return };
            if let Err(e) = sweep_map_leftovers(&conn) {
                eprintln!("mods: the Map's old rows were not cleared ({e})");
            }
        })
        .await;
        run_cached(&app).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::index::tests::good_index;
    use crate::mods::safety::tests::{index_with, now};

    const KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn store() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), KEY).unwrap();
        (dir, conn)
    }

    fn mod_row(conn: &Connection, from: &str, signed: bool) {
        conn.execute(
            "INSERT INTO mods(id, name, version, tier, author, source_repo, source_commit, signed, \
                              install_id, installed_from, package_sha256, manifest, files) \
             VALUES ('atlas.map', 'Map', '1.0.0', 'first', 'Atlas', 'r/r', ?1, ?2, \
                     '0123456789abcdef', ?3, ?4, '{}', '[]')",
            params!["0".repeat(40), signed, from, "a".repeat(64)],
        )
        .unwrap();
    }

    fn put(conn: &Connection, key: &str, value: &str) {
        db::set_setting(conn, key, value).unwrap();
    }

    #[test]
    fn decide_is_a_pure_walk_of_the_rows_and_the_index() {
        let p = &PILOTS[0];
        let index = index_with(|_| {});
        let held = |from, signed| {
            Some(Held {
                installed_from: from,
                signed,
                files_missing: false,
                version: "1.0.0",
                sha256: Some("aa"),
            })
        };
        // A catalogue row settles, whatever the index says (even none).
        assert_eq!(
            decide(p, held("catalogue", true), None, now()),
            Step::Settle
        );
        assert_eq!(
            decide(p, held("catalogue", true), Some(&index), now()),
            Step::Settle
        );
        // A catalogue row whose files are missing is put back (a repair, from the pinned file) once
        // a current index doesn't blacklist it; any other row with missing files is no business of
        // the pass's.
        let missing = |from, signed| {
            Some(Held {
                installed_from: from,
                signed,
                files_missing: true,
                version: "1.0.0",
                sha256: Some("aa"),
            })
        };
        assert_eq!(
            decide(p, missing("catalogue", true), None, now()),
            Step::Leave(Leave::NoCurrentIndex)
        );
        assert_eq!(
            decide(
                p,
                missing("catalogue", true),
                Some(&index),
                now() + chrono::Duration::days(90)
            ),
            Step::Leave(Leave::NoCurrentIndex)
        );
        assert_eq!(
            decide(p, missing("catalogue", true), Some(&index), now()),
            Step::Repair
        );
        for (from, signed) in [("folder", true), ("file", true), ("catalogue", false)] {
            assert_eq!(
                decide(p, missing(from, signed), Some(&index), now()),
                Step::Leave(Leave::OtherRow),
                "{from} {signed}"
            );
        }
        // Any other row is left: a test folder's rows are dropped at the next start, a file's are
        // unsigned or third party.
        for (from, signed) in [("folder", true), ("file", true), ("catalogue", false)] {
            assert_eq!(
                decide(p, held(from, signed), Some(&index), now()),
                Step::Leave(Leave::OtherRow),
                "{from} {signed}"
            );
        }
        // No row: only a current index lists a download.
        assert_eq!(
            decide(p, None, None, now()),
            Step::Leave(Leave::NoCurrentIndex)
        );
        let after = now() + chrono::Duration::days(90);
        assert_eq!(
            decide(p, None, Some(&index), after),
            Step::Leave(Leave::NoCurrentIndex)
        );
        match decide(p, None, Some(&index), now()) {
            Step::Download(d) => {
                assert_eq!((d.id.as_str(), d.version.as_str()), ("atlas.map", "1.2.0"));
                assert!(d.url.ends_with("atlas.map-1.2.0.pmmod"));
            }
            other => panic!("{other:?}"),
        }
        let empty = index_with(|v| v["mods"] = json!([]));
        assert_eq!(
            decide(p, None, Some(&empty), now()),
            Step::Leave(Leave::NotListed)
        );
        // The fixture really is a first-party entry; the index format keeps it so for an `atlas.` id.
        assert_eq!(good_index(1)["mods"][0]["tier"], "first");
    }

    #[test]
    fn only_a_marker_in_the_store_makes_a_pilot_pending() {
        let (_dir, conn) = store();
        assert!(pending(&conn).unwrap().is_empty());
        // A marker for an id that is no pilot is ignored; the Pinboard's is one now (PR C, K8).
        put(&conn, "mods_autoinstall_nothing", "1");
        assert!(
            pending(&conn).unwrap().is_empty(),
            "a marker for no pilot here"
        );
        put(&conn, "mods_autoinstall_map", "1");
        let ids: Vec<&str> = pending(&conn).unwrap().iter().map(|p| p.id).collect();
        assert_eq!(ids, ["atlas.map"]);
        put(&conn, PILOTS[1].marker, "1");
        let ids: Vec<&str> = pending(&conn).unwrap().iter().map(|p| p.id).collect();
        assert_eq!(ids, ["atlas.map", "atlas.pinboard"], "in table order");
        conn.execute("DELETE FROM settings WHERE key = ?1", [PILOTS[1].marker])
            .unwrap();
        let state = handover_state(&conn).unwrap();
        assert_eq!(
            state,
            HandoverState {
                landed: vec![],
                adopt_map_prefs: false,
                adopt_pinboard_prefs: false
            }
        );
        put(&conn, "mods_pilot_landed_map", "1");
        put(&conn, ADOPT_FLAG, "1");
        put(&conn, pinboard_move::ADOPT_FLAG, "1");
        assert_eq!(
            serde_json::to_value(handover_state(&conn).unwrap()).unwrap(),
            json!({"landed": ["atlas.map"], "adoptMapPrefs": true, "adoptPinboardPrefs": true})
        );
        put(&conn, PILOTS[1].landed, "1");
        assert_eq!(
            handover_state(&conn).unwrap().landed,
            ["atlas.map", "atlas.pinboard"]
        );
    }

    #[test]
    fn a_planted_type_at_a_marker_or_a_note_never_fails_the_pass_or_the_page_state() {
        // A shared vault's member can write any type into `settings`; a typed read of one is an
        // error that would stop every pilot's pass and the page's handover query.
        let (_dir, conn) = store();
        for key in [
            PILOTS[0].marker,
            PILOTS[1].marker,
            PILOTS[0].landed,
            PILOTS[1].landed,
            ADOPT_FLAG,
            pinboard_move::ADOPT_FLAG,
        ] {
            conn.execute(
                "INSERT INTO settings(key, value) VALUES (?1, x'00ff00')",
                [key],
            )
            .unwrap();
        }
        let ids: Vec<&str> = pending(&conn).unwrap().iter().map(|p| p.id).collect();
        assert_eq!(ids, ["atlas.map", "atlas.pinboard"]);
        assert_eq!(
            handover_state(&conn).unwrap(),
            HandoverState {
                landed: vec!["atlas.map".to_owned(), "atlas.pinboard".to_owned()],
                adopt_map_prefs: true,
                adopt_pinboard_prefs: true
            }
        );
    }

    #[test]
    fn adopting_the_device_prefs_fills_only_what_the_mod_lacks_then_clears_the_flag() {
        let (_dir, mut conn) = store();
        mod_row(&conn, "catalogue", true);
        // Without the handover's flag nothing is written and nothing is refused.
        assert!(!adopt_map_prefs(&mut conn, "semantic", 0.25, true).unwrap());
        assert_eq!(
            conn.query_row("SELECT count(*) FROM mod_kv", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        // With it, and the handover's nodeCap in place: the other three are added, revision bumped.
        put(&conn, ADOPT_FLAG, "1");
        conn.execute(
            "INSERT INTO mod_kv(mod_id, key, class, value, revision) \
             VALUES ('atlas.map', 'settings', 'content', '{\"nodeCap\":2500}', 1)",
            [],
        )
        .unwrap();
        assert!(adopt_map_prefs(&mut conn, "semantic", 0.25, true).unwrap());
        let (value, revision): (String, i64) = conn
            .query_row(
                "SELECT value, revision FROM mod_kv WHERE key = 'settings'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let v: Value = serde_json::from_str(&value).unwrap();
        assert_eq!(
            v,
            json!({"nodeCap": 2500, "layoutMode": "semantic", "cohesion": 0.25, "labels": true})
        );
        assert_eq!(revision, 2);
        assert!(get(&conn, ADOPT_FLAG).unwrap().is_none());
        // A second device finds the flag gone: nothing to hand over, the page may drop its keys.
        assert!(!adopt_map_prefs(&mut conn, "project", 0.0, false).unwrap());
        assert_eq!(
            conn.query_row("SELECT revision FROM mod_kv", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
    }

    #[test]
    fn adopting_after_a_handover_that_wrote_enhanced_false_leaves_it_untouched() {
        let (_dir, mut conn) = store();
        mod_row(&conn, "catalogue", true);
        put(&conn, MAP_PREF, r#"{"nodeCap":900,"tsneEnabled":false}"#);
        hand_over_map(&conn, "atlas.map").unwrap();
        assert!(get(&conn, MAP_PREF).unwrap().is_none());
        let stored = |conn: &Connection| -> Value {
            let text: String = conn
                .query_row("SELECT value FROM mod_kv WHERE key = 'settings'", [], |r| {
                    r.get(0)
                })
                .unwrap();
            serde_json::from_str(&text).unwrap()
        };
        assert_eq!(stored(&conn), json!({"nodeCap": 900, "enhanced": false}));
        // The page's device prefs add only what is missing; `enhanced` and `nodeCap` stay.
        assert!(adopt_map_prefs(&mut conn, "semantic", 0.25, true).unwrap());
        assert_eq!(
            stored(&conn),
            json!({
                "nodeCap": 900, "enhanced": false,
                "layoutMode": "semantic", "cohesion": 0.25, "labels": true
            })
        );
    }

    #[test]
    fn a_settings_key_the_mod_already_has_wins_whatever_it_holds_and_the_old_rows_go() {
        // INSERT OR IGNORE leaves the mod's own row; there is nothing of ours to verify, and a
        // planted row must not stop the install.
        let (_dir, conn) = store();
        mod_row(&conn, "catalogue", true);
        conn.execute(
            "INSERT INTO mod_kv(mod_id, key, class, value, revision) \
             VALUES ('atlas.map', 'settings', 'content', '[1]', 2)",
            [],
        )
        .unwrap();
        for pref in [r#"{"nodeCap":900}"#, r#"{"tsneEnabled":false}"#] {
            put(&conn, MAP_PREF, pref);
            put(&conn, "doc_layout_meta", "{}");
            hand_over_map(&conn, "atlas.map").unwrap_or_else(|e| panic!("{pref}: {e:?}"));
            assert!(get(&conn, MAP_PREF).unwrap().is_none(), "{pref}");
            assert!(get(&conn, "doc_layout_meta").unwrap().is_none(), "{pref}");
            assert!(get(&conn, ADOPT_FLAG).unwrap().is_some(), "{pref}");
            let (value, revision): (String, i64) = conn
                .query_row(
                    "SELECT value, revision FROM mod_kv WHERE key = 'settings'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!((value.as_str(), revision), ("[1]", 2), "{pref}");
            db::delete_setting(&conn, ADOPT_FLAG).unwrap();
        }
    }

    #[test]
    fn what_the_user_already_changed_in_the_mod_is_never_overwritten_by_the_device() {
        let (_dir, mut conn) = store();
        mod_row(&conn, "catalogue", true);
        put(&conn, ADOPT_FLAG, "1");
        let mine =
            json!({"nodeCap": 700, "layoutMode": "project", "cohesion": 0.1, "labels": false});
        conn.execute(
            "INSERT INTO mod_kv(mod_id, key, class, value, revision) \
             VALUES ('atlas.map', 'settings', 'content', ?1, 6)",
            [mine.to_string()],
        )
        .unwrap();
        assert!(adopt_map_prefs(&mut conn, "semantic", 0.5, true).unwrap());
        let (value, revision): (String, i64) = conn
            .query_row("SELECT value, revision FROM mod_kv", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&value).unwrap(), mine);
        assert_eq!(revision, 6, "nothing changed, so nothing was written");
        assert!(get(&conn, ADOPT_FLAG).unwrap().is_none());
    }

    #[test]
    fn with_no_settings_key_yet_the_device_prefs_become_the_first_revision() {
        let (_dir, mut conn) = store();
        mod_row(&conn, "catalogue", true);
        put(&conn, ADOPT_FLAG, "1");
        assert!(adopt_map_prefs(&mut conn, "project", 0.0, false).unwrap());
        let (value, revision): (String, i64) = conn
            .query_row(
                "SELECT value, revision FROM mod_kv WHERE class = 'content'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&value).unwrap(),
            json!({"layoutMode": "project", "cohesion": 0.0, "labels": false})
        );
        assert_eq!(revision, 1);
    }

    #[test]
    fn untrusted_values_are_refused_or_clamped_and_a_test_folder_gets_nothing() {
        let (_dir, mut conn) = store();
        mod_row(&conn, "catalogue", true);
        put(&conn, ADOPT_FLAG, "1");
        for bad in ["", "Semantic", "grid", "semantic\0"] {
            assert!(matches!(
                adopt_map_prefs(&mut conn, bad, 0.1, false),
                Err(AdoptRefusal::BadValue)
            ));
        }
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(matches!(
                adopt_map_prefs(&mut conn, "project", bad, false),
                Err(AdoptRefusal::BadValue)
            ));
        }
        assert!(
            get(&conn, ADOPT_FLAG).unwrap().is_some(),
            "a refusal keeps the flag"
        );
        assert!(adopt_map_prefs(&mut conn, "project", 9.0, false).unwrap());
        let value: String = conn
            .query_row("SELECT value FROM mod_kv", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&value).unwrap()["cohesion"],
            0.5
        );

        // A folder row's storage dies with it at the next start: not a place to hand things to.
        let (_dir, mut conn) = store();
        mod_row(&conn, "folder", true);
        put(&conn, ADOPT_FLAG, "1");
        assert!(matches!(
            adopt_map_prefs(&mut conn, "project", 0.1, false),
            Err(AdoptRefusal::NotInstalled)
        ));
        assert!(get(&conn, ADOPT_FLAG).unwrap().is_some());
        // And no row at all.
        let (_dir, mut conn) = store();
        put(&conn, ADOPT_FLAG, "1");
        assert!(matches!(
            adopt_map_prefs(&mut conn, "project", 0.1, false),
            Err(AdoptRefusal::NotInstalled)
        ));
    }

    #[test]
    fn only_one_pass_runs_at_a_time_and_the_claim_is_given_back() {
        let first = Running::claim().expect("free");
        assert!(Running::claim().is_none(), "a second pass stands down");
        drop(first);
        assert!(Running::claim().is_some(), "and the claim is released");
    }

    #[test]
    fn every_step_of_a_pass_asks_whether_this_instance_writes_first() {
        // The pass needs an app handle, so its order is pinned in the source: the plan asks before it
        // reads the store, and each install or settle asks again inside its own blocking step (the
        // baton can move while a file downloads).
        let src = include_str!("autoinstall.rs");
        let body = src.split("// ---- The pass ----").nth(1).unwrap();
        let body = body.split("#[cfg(test)]\nmod tests").next().unwrap();
        let plan = body
            .split("fn plan_now(")
            .nth(1)
            .unwrap()
            .split("\n}\n")
            .next()
            .unwrap();
        assert!(plan.find("writer_now(app)").unwrap() < plan.find("with_active_ledger").unwrap());
        assert!(plan.find("writer_now(app)").unwrap() < plan.find("any_marker(app)").unwrap());
        let run = body
            .split("pub async fn run<")
            .nth(1)
            .unwrap()
            .split("\n}\n")
            .next()
            .unwrap();
        assert_eq!(
            run.matches("writer_now(&app2)").count(),
            4,
            "settle, install, and a repair's two steps each ask"
        );
        assert_eq!(run.matches("with_active_ledger").count(), 4);
        // A download is only started from a plan step, never from here.
        assert_eq!(run.matches("fetch_package(").count(), 2);
        assert!(run.contains("Step::Download(download) =>"));
        assert!(run.contains("Step::Repair =>"));
        // The marker guard is dropped before a ledger is taken (the DB lock isn't re-entrant).
        let any = body
            .split("fn any_marker(")
            .nth(1)
            .unwrap()
            .split("\n}\n")
            .next()
            .unwrap();
        assert!(!any.contains("with_active_ledger"));
    }

    #[test]
    fn the_hooks_are_where_the_plan_says_and_nowhere_else() {
        // After an accepted index (refresh's work_on) and after a writer's store opens
        // (ledger::after_settle, not after_writer_open, which returns early for a store with no mods).
        let refresh = include_str!("refresh.rs");
        let work = refresh.split("async fn work_on<").nth(1).unwrap();
        assert!(
            work.find("apply_in_background(app, &index, now)").unwrap()
                < work.find("autoinstall::run(").unwrap(),
            "restrictions first"
        );
        assert!(work.find("autoinstall::run(").unwrap() < work.find("plan_pass").unwrap());
        let ledger = include_str!("ledger.rs");
        let settle = ledger.split("pub fn after_settle(").nth(1).unwrap();
        let settle = settle.split("\n}\n").next().unwrap();
        assert!(
            settle.find("after_writer_open(app)").unwrap()
                < settle.find("autoinstall::spawn_after_open(app)").unwrap()
        );
    }

    /// The pass is awake since v61 and v62, which write the Map's marker for a store that has
    /// documents and the Pinboard's for a store that used it (`db/migrations.rs`, the one writer). No
    /// other code in this crate names a marker key outside this file, those migrations and the tests:
    /// a second writer would start downloads nobody asked for, so it can't happen by accident.
    #[test]
    fn only_this_file_and_the_migration_name_a_marker_key() {
        fn walk(dir: &std::path::Path, out: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    // Test code may seed a marker; production code may not (this file defines them).
                    let production = text.split("#[cfg(test)]\nmod tests").next().unwrap();
                    let own = path.ends_with("mods/autoinstall.rs")
                        || path.ends_with("db/migrations.rs")
                        || path.components().any(|c| c.as_os_str() == "tests");
                    // Built so this file doesn't match its own search.
                    if !own && production.contains(concat!("mods_auto", "install_")) {
                        out.push(path.display().to_string());
                    }
                }
            }
        }
        let mut found = Vec::new();
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut found,
        );
        assert!(found.is_empty(), "{found:?}");
        // And the migrations really write the marker of each pilot (the pass is not dormant).
        let migrations = include_str!("../db/migrations.rs");
        let production = migrations.split("#[cfg(test)]\nmod tests").next().unwrap();
        for (pilot, version) in [(&PILOTS[0], "v61"), (&PILOTS[1], "v62")] {
            assert!(
                production.contains(&format!("SELECT '{}', '1'", pilot.marker)),
                "{version} must write {}",
                pilot.marker
            );
        }
    }

    #[test]
    fn the_pilots_table_names_each_pilot_once_and_the_pinboard_is_awake() {
        let ids: Vec<&str> = PILOTS.iter().map(|p| p.id).collect();
        assert_eq!(ids, ["atlas.map", "atlas.pinboard"]);
        let p = &PILOTS[1];
        assert_eq!(
            (p.marker, p.name, p.tab_label, p.landed, p.pending),
            (
                "mods_autoinstall_pinboard",
                "Pinboard",
                "Pinboard",
                "mods_pilot_landed_pinboard",
                Some("pinboard_move_pending")
            )
        );
        // No two pilots share an id, a marker or a landed key.
        for (i, a) in PILOTS.iter().enumerate() {
            for b in &PILOTS[i + 1..] {
                assert!(a.id != b.id && a.marker != b.marker && a.landed != b.landed);
            }
        }
        // The Map has no second key; and a store with nothing in it is owed nothing.
        assert_eq!(PILOTS[0].pending, None);
        let (_dir, conn) = store();
        assert!(pending(&conn).unwrap().is_empty());
    }

    /// Put the three dead rows and one live neighbour of each kind in a store.
    fn plant_dead_map_rows(conn: &Connection) {
        put(conn, "map", r#"{"nodeCap":900}"#);
        put(conn, "doc_layout_meta", r#"{"x":1}"#);
        put(conn, "project_layout_cache", "[1,2]");
        for neighbour in [
            "map_other",
            "doc_layout",
            "pinboard",
            "mods_pilot_landed_pinboard",
        ] {
            put(conn, neighbour, "keep");
        }
    }

    fn settings_keys(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT key FROM settings ORDER BY key")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn the_sweep_clears_the_core_maps_dead_rows_when_no_marker_is_pending_and_nothing_else() {
        let (_dir, conn) = store();
        plant_dead_map_rows(&conn);
        let before = settings_keys(&conn);
        assert_eq!(sweep_map_leftovers(&conn).unwrap(), 3);
        let after = settings_keys(&conn);
        let gone: Vec<&String> = before.iter().filter(|k| !after.contains(k)).collect();
        assert_eq!(gone, ["doc_layout_meta", "map", "project_layout_cache"]);
        // Every other row is as it was, value included.
        for key in [
            "map_other",
            "doc_layout",
            "pinboard",
            "mods_pilot_landed_pinboard",
        ] {
            assert_eq!(get(&conn, key).unwrap().as_deref(), Some("keep"), "{key}");
        }
        // Idempotent: a second sweep finds nothing and changes nothing.
        assert_eq!(sweep_map_leftovers(&conn).unwrap(), 0);
        assert_eq!(settings_keys(&conn), after);
    }

    #[test]
    fn the_sweep_waits_for_the_handover_while_the_maps_marker_is_pending() {
        let (_dir, conn) = store();
        plant_dead_map_rows(&conn);
        put(&conn, PILOTS[0].marker, "1");
        let before = settings_keys(&conn);
        assert_eq!(sweep_map_leftovers(&conn).unwrap(), 0);
        assert_eq!(settings_keys(&conn), before, "nothing was deleted");
        assert_eq!(
            get(&conn, "map").unwrap().as_deref(),
            Some(r#"{"nodeCap":900}"#),
            "the handover still has the pref to carry"
        );
        // The handover then moves the prefs and deletes them itself, as it always did; after it the
        // marker is gone and the sweep has nothing left to do.
        mod_row(&conn, "catalogue", true);
        finish(&conn, &PILOTS[0]).unwrap();
        assert_eq!(get(&conn, "map").unwrap(), None);
        assert_eq!(sweep_map_leftovers(&conn).unwrap(), 0);
        // Only the Map's own marker holds it back: the Pinboard's does not.
        let (_dir, conn) = store();
        plant_dead_map_rows(&conn);
        put(&conn, PILOTS[1].marker, "1");
        assert_eq!(sweep_map_leftovers(&conn).unwrap(), 3);
    }

    /// The three wrong SQLite types a shared vault's member can plant in a text column.
    const PLANTED: [(&str, &str); 3] = [
        ("a BLOB", "x'00ff00'"),
        ("an integer", "42"),
        ("non-UTF-8 text", "CAST(x'c328ff' AS TEXT)"),
    ];

    fn plant_wrong_type(conn: &Connection, key: &str, literal: &str) {
        conn.execute("DELETE FROM settings WHERE key = ?1", [key])
            .unwrap();
        conn.execute(
            &format!("INSERT INTO settings(key, value) VALUES (?1, {literal})"),
            [key],
        )
        .unwrap();
    }

    fn mod_kv_rows(conn: &Connection) -> i64 {
        conn.query_row("SELECT count(*) FROM mod_kv", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn a_map_pref_of_another_type_carries_nothing_and_never_blocks_the_auto_pass_or_the_manual_install(
    ) {
        for (what, literal) in PLANTED {
            for manual in [false, true] {
                let label = format!("{what}, manual: {manual}");
                let (_dir, conn) = store();
                plant_dead_map_rows(&conn);
                plant_wrong_type(&conn, MAP_PREF, literal);
                put(&conn, PILOTS[0].marker, "1");
                mod_row(&conn, "catalogue", true);
                if manual {
                    finish_manual(&conn, "atlas.map", InstallKind::Fresh, true)
                        .unwrap_or_else(|e| panic!("{label}: {e:?}"));
                } else {
                    finish(&conn, &PILOTS[0]).unwrap_or_else(|e| panic!("{label}: {e:?}"));
                }
                // Nothing was adopted from it.
                assert_eq!(mod_kv_rows(&conn), 0, "{label}");
                // The dead rows went, the pref among them, and only those.
                for key in MAP_DEAD_KEYS {
                    assert!(!has(&conn, key).unwrap(), "{label}: {key}");
                }
                for key in ["map_other", "doc_layout", "pinboard"] {
                    assert!(has(&conn, key).unwrap(), "{label}: {key}");
                }
                // The install settled: marker gone, note and the page's flag written.
                assert!(!has(&conn, PILOTS[0].marker).unwrap(), "{label}");
                assert!(has(&conn, PILOTS[0].landed).unwrap(), "{label}");
                assert!(has(&conn, ADOPT_FLAG).unwrap(), "{label}");
            }
        }
    }

    #[test]
    fn a_map_pref_of_another_type_reads_as_missing_and_a_real_one_still_carries() {
        let (_dir, conn) = store();
        for (what, literal) in PLANTED {
            plant_wrong_type(&conn, MAP_PREF, literal);
            // `settings.value` is TEXT, so SQLite's affinity stores an integer as its text: that one
            // reads as "42" (not a pref object, so it carries nothing); the other two stay unreadable.
            let stored: String = conn
                .query_row(
                    "SELECT typeof(value) FROM settings WHERE key = 'map'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let expected = if stored == "text" && what == "an integer" {
                Some("42".to_owned())
            } else {
                None
            };
            assert_eq!(get(&conn, MAP_PREF).unwrap(), expected, "{what} ({stored})");
        }
        // The tolerance is for the wrong type only: a text pref is still carried.
        put(&conn, MAP_PREF, r#"{"nodeCap":900}"#);
        mod_row(&conn, "catalogue", true);
        hand_over_map(&conn, "atlas.map").unwrap();
        let carried: String = conn
            .query_row("SELECT value FROM mod_kv WHERE key = 'settings'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&carried).unwrap(),
            json!({"nodeCap": 900})
        );
    }

    fn plant_wrong_type_kv(conn: &Connection, literal: &str) {
        conn.execute(
            &format!(
                "INSERT INTO mod_kv(mod_id, key, class, value, revision) \
                 VALUES ('atlas.map', 'settings', 'content', {literal}, 3)"
            ),
            [],
        )
        .unwrap();
    }

    #[test]
    fn a_mod_settings_row_of_another_type_never_blocks_the_auto_pass_or_the_manual_install() {
        for (what, literal) in PLANTED {
            for manual in [false, true] {
                let label = format!("{what}, manual: {manual}");
                let (_dir, conn) = store();
                plant_dead_map_rows(&conn);
                put(&conn, MAP_PREF, r#"{"nodeCap":900,"tsneEnabled":false}"#);
                put(&conn, PILOTS[0].marker, "1");
                mod_row(&conn, "catalogue", true);
                plant_wrong_type_kv(&conn, literal);
                if manual {
                    finish_manual(&conn, "atlas.map", InstallKind::Fresh, true)
                        .unwrap_or_else(|e| panic!("{label}: {e:?}"));
                } else {
                    finish(&conn, &PILOTS[0]).unwrap_or_else(|e| panic!("{label}: {e:?}"));
                }
                // The mod's own row is untouched (the user's data wins), the dead rows went.
                let revision: i64 = conn
                    .query_row("SELECT revision FROM mod_kv", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(revision, 3, "{label}");
                assert_eq!(mod_kv_rows(&conn), 1, "{label}");
                for key in MAP_DEAD_KEYS {
                    assert!(!has(&conn, key).unwrap(), "{label}: {key}");
                }
                assert!(!has(&conn, PILOTS[0].marker).unwrap(), "{label}");
                assert!(has(&conn, PILOTS[0].landed).unwrap(), "{label}");
                assert!(has(&conn, ADOPT_FLAG).unwrap(), "{label}");
            }
        }
    }

    #[test]
    fn adopting_the_device_prefs_over_a_mod_settings_row_of_another_type_still_settles() {
        for (what, literal) in PLANTED {
            let (_dir, mut conn) = store();
            mod_row(&conn, "catalogue", true);
            put(&conn, ADOPT_FLAG, "1");
            plant_wrong_type_kv(&conn, literal);
            assert!(
                adopt_map_prefs(&mut conn, "semantic", 0.25, true)
                    .unwrap_or_else(|e| panic!("{what}: {e:?}")),
                "{what}"
            );
            assert!(get(&conn, ADOPT_FLAG).unwrap().is_none(), "{what}");
            let (value, revision): (String, i64) = conn
                .query_row("SELECT value, revision FROM mod_kv", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .unwrap();
            assert_eq!(revision, 4, "{what}");
            assert_eq!(
                serde_json::from_str::<Value>(&value).unwrap(),
                json!({"layoutMode": "semantic", "cohesion": 0.25, "labels": true}),
                "{what}"
            );
        }
    }

    #[test]
    fn a_dead_row_of_another_type_is_still_deleted_by_the_handover_and_the_sweep() {
        for (what, literal) in PLANTED {
            let (_dir, conn) = store();
            plant_dead_map_rows(&conn);
            mod_row(&conn, "catalogue", true);
            for key in ["doc_layout_meta", "project_layout_cache"] {
                plant_wrong_type(&conn, key, literal);
            }
            hand_over_map(&conn, "atlas.map").unwrap();
            for key in MAP_DEAD_KEYS {
                assert!(!has(&conn, key).unwrap(), "{what}: {key}");
            }
            let (_dir, conn) = store();
            plant_dead_map_rows(&conn);
            for key in MAP_DEAD_KEYS {
                plant_wrong_type(&conn, key, literal);
            }
            assert_eq!(sweep_map_leftovers(&conn).unwrap(), 3, "{what}");
            assert!(has(&conn, "pinboard").unwrap(), "{what}");
        }
    }

    #[test]
    fn a_planted_marker_of_any_type_still_holds_the_sweep_back() {
        let (_dir, conn) = store();
        plant_dead_map_rows(&conn);
        conn.execute(
            "INSERT INTO settings(key, value) VALUES (?1, x'ff')",
            [PILOTS[0].marker],
        )
        .unwrap();
        assert_eq!(sweep_map_leftovers(&conn).unwrap(), 0);
        assert!(get(&conn, "doc_layout_meta").unwrap().is_some());
    }

    #[test]
    fn the_sweep_runs_after_a_writers_store_opens_and_nowhere_else() {
        let src = include_str!("autoinstall.rs");
        let production = src.split("#[cfg(test)]\nmod tests").next().unwrap();
        let spawn = production
            .split("pub fn spawn_after_open(")
            .nth(1)
            .unwrap()
            .split("\n}\n")
            .next()
            .unwrap();
        assert!(
            spawn.contains("writer_now(&app2)"),
            "only the writer deletes"
        );
        assert!(
            spawn.find("sweep_map_leftovers").unwrap() < spawn.find("run_cached").unwrap(),
            "before the pass"
        );
        assert_eq!(production.matches("sweep_map_leftovers(").count(), 2);
    }
}
