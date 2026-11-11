// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Applying the signed index's blacklist to the installed rows (Mods spec §1.10), and the two small
//! records slice 6's strips read: the mods the list resumed, and the background updates that wait
//! for the user. The decisions are `safety::plan_row`'s (pure); this is the edge that carries them
//! out, under the ledger lock, one row at a time, each in its own transaction.
//!
//! **Order, and what a failure leaves.** The registry is cut *first* (`runtime.pause` for a pause,
//! `runtime.uninstall` for a removal; both end the mod's frames and its calls in flight), then the
//! row is written, then the files go. So a failure never
//! leaves a restricted mod running: if the write fails, the mod is off for this run and the next
//! apply (every fetch, every launch) finds the row unchanged and writes it again; if the files
//! can't be removed, the row already says removed and the next apply removes them
//! (`Action::FinishRemoval`). Every row write compares against the state it was planned from, so
//! another owner's change in between (the user's pause, an update) wins and is looked at again next
//! time.
//!
//! **A removal deletes what is rebuildable and nothing the user made** (I-16, I-31): the grants,
//! the cache-class `mod_kv` rows, the whole of `mod_doc_data` and the calendar overlay (all derived,
//! SYNC-SET.md) and the mod's folder. Its content-class `mod_kv` rows, its gifts (`mod_ledger`, which
//! is outside the cascade anyway), its secrets, its AI spend and the row itself stay. The row stays
//! `paused` with `state_reason = blacklist-removed`: the typed "pending removal" state slice 6's
//! uninstall dialog reads, and the reason nothing installs onto it, loads its files or copies them
//! back (`Refusal::RemovedPending`, `Row::removed`). The dialog's answer is an ordinary
//! [`Ledger::uninstall`].
//!
//! The secrets stay because they are the user's own typed credentials, not something the mod made,
//! and with the grants gone and the mod off nothing can send them; the uninstall dialog's cascade
//! removes them.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::{time_text, Ledger, LedgerStore};
use crate::mods::ids::ModId;
use crate::mods::index::Index;
use crate::mods::safety::{self, Action, ModNotice, RowView, SafetyCheck};
use crate::mods::{disk, store};

/// What one pass of the blacklist did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ApplyReport {
    /// For the page's strips, in the order they happened. The strips must also be derivable from
    /// the rows and the cached index (the page may not have been listening yet).
    pub notices: Vec<ModNotice>,
    /// Rows that couldn't be written this time (`id`, why); the next pass tries them again.
    pub failed: Vec<(String, String)>,
    /// Files a removed mod still had that couldn't be deleted (`id`, why); the next pass retries.
    pub files_left: Vec<(String, String)>,
    /// Why nothing was looked at, when the rows couldn't be read.
    pub skipped: Option<String>,
}

impl ApplyReport {
    pub fn is_empty(&self) -> bool {
        self.notices.is_empty()
            && self.failed.is_empty()
            && self.files_left.is_empty()
            && self.skipped.is_none()
    }
}

/// An installed row as the plan reads it, with the name the strips show.
struct Looked {
    view: RowView,
    id: String,
}

fn read_views(conn: &Connection) -> rusqlite::Result<Vec<Looked>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, version, package_sha256, state, state_reason FROM mods ORDER BY id",
    )?;
    let rows = stmt.query_map([], |r| {
        let id: String = r.get(0)?;
        Ok(Looked {
            view: RowView {
                id: id.clone(),
                name: r.get(1)?,
                version: r.get(2)?,
                package_sha256: r.get(3)?,
                state: r.get(4)?,
                reason: r.get(5)?,
            },
            id,
        })
    })?;
    rows.collect()
}

impl<S: LedgerStore> Ledger<'_, S> {
    /// Apply `index`'s blacklist to every installed row (module doc), at `now`. Run by the writer:
    /// at startup before the load, and after every accepted fetch and every restore. Synchronous
    /// file and DB work, so from a blocking thread. It never installs anything; a row that resumes
    /// is loaded back into the registry here.
    pub fn apply_blacklist(&self, index: &Index, now: DateTime<Utc>) -> ApplyReport {
        let _ledger = self.runtime.ledger_lock();
        let mut report = ApplyReport::default();
        let rows = match self.with_conn(|c| Ok(read_views(c)?)) {
            Ok(rows) => rows,
            Err(e) => {
                report.skipped = Some(e.to_string());
                return report;
            }
        };
        let at = time_text(now);
        let mut resumed = false;
        for Looked { view, id } in &rows {
            let action = safety::plan_row(view, index, now);
            let mod_id = ModId::parse(id).ok();
            match action {
                Action::Keep => {}
                Action::Pause {
                    reason: _,
                    fixed_in,
                } => {
                    // Off first: a write that fails must not leave a restricted mod running.
                    self.cut(mod_id.as_ref(), Cut::Pause);
                    match self.with_conn(|c| Ok(pause_row(c, view, &at)?)) {
                        Ok(true) => report.notices.push(ModNotice::Paused {
                            id: id.clone(),
                            name: view.name.clone(),
                            version: view.version.clone(),
                            reason: reason_of(index, view),
                            fixed_in,
                        }),
                        Ok(false) => {}
                        Err(e) => report.failed.push((id.clone(), e.to_string())),
                    }
                }
                Action::Remove { reason } => {
                    self.cut(mod_id.as_ref(), Cut::Remove);
                    match self.with_conn(|c| Ok(remove_row(c, view, Some(&at))?)) {
                        Ok(true) => {
                            report.notices.push(ModNotice::Removed {
                                id: id.clone(),
                                name: view.name.clone(),
                                version: view.version.clone(),
                                reason,
                            });
                            self.delete_files(id, mod_id.as_ref(), &mut report);
                        }
                        Ok(false) => {}
                        Err(e) => report.failed.push((id.clone(), e.to_string())),
                    }
                }
                Action::FinishRemoval => {
                    self.cut(mod_id.as_ref(), Cut::Remove);
                    match self.with_conn(|c| Ok(remove_row(c, view, None)?)) {
                        Ok(_) => self.delete_files(id, mod_id.as_ref(), &mut report),
                        Err(e) => report.failed.push((id.clone(), e.to_string())),
                    }
                }
                Action::Resume => match self.with_conn(|c| Ok(resume_row(c, view, &at)?)) {
                    Ok(true) => {
                        resumed = true;
                        report.notices.push(ModNotice::Resumed {
                            id: id.clone(),
                            name: view.name.clone(),
                            version: view.version.clone(),
                        });
                    }
                    Ok(false) => {}
                    Err(e) => report.failed.push((id.clone(), e.to_string())),
                },
            }
        }
        if resumed {
            // A resumed mod is loaded the way a startup loads it (its files checked first), and
            // the registry made exactly the rows that serve.
            let check = SafetyCheck::serve(Some(index));
            match self.with_conn(|c| Ok(super::read_rows(c)?)) {
                Ok(rows) => {
                    self.load(&rows, now, &check);
                }
                Err(e) => report.failed.push(("(registry)".into(), e.to_string())),
            }
        }
        report
    }

    /// Switch the mod off in the registry and end its frames and API access. Both runtime calls
    /// drop every session of the mod, so a `mod_call` already in flight holds a ticket that is no
    /// longer current and commits nothing (the dispatcher re-checks it before any write). A pause
    /// tells each live frame `mod.paused` then `session.closed`; a removal tells it
    /// `session.closed` ("uninstalled").
    fn cut(&self, id: Option<&ModId>, how: Cut) {
        if let Some(id) = id {
            match how {
                Cut::Pause => self.runtime.pause(id),
                Cut::Remove => self.runtime.uninstall(id),
            }
        }
    }

    /// Remove a removed mod's folder. A failure is reported and retried by the next pass.
    fn delete_files(&self, id: &str, mod_id: Option<&ModId>, report: &mut ApplyReport) {
        let Some(mod_id) = mod_id else {
            // An id that can't name a folder has none of ours to remove.
            return;
        };
        if let Err(e) = disk::remove_mod(self.layout, mod_id) {
            report.files_left.push((id.to_owned(), e.to_string()));
        }
    }
}

/// How [`Ledger::cut`] switches a mod off, which is what its live frames are told.
#[derive(Clone, Copy)]
enum Cut {
    Pause,
    Remove,
}

/// The index's words for the entry that pauses `view`, for the strip (the plan already chose to
/// pause, so an entry matches; an empty string is only for a list that changed in between).
fn reason_of(index: &Index, view: &RowView) -> String {
    index
        .blacklisted(&view.id, &view.version, view.package_sha256.as_deref())
        .map(|e| e.reason.clone())
        .unwrap_or_default()
}

/// Pause an active row (or take over a ledger park; [`super::safety::plan_row`] leaves another
/// owner's pause or park alone) for the blacklist. Compare-and-set on the state and reason the
/// plan read; `false` when the row changed since.
fn pause_row(conn: &mut Connection, view: &RowView, at: &str) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE mods SET state = ?2, state_reason = ?3, updated_at = ?4 \
         WHERE id = ?1 AND state = ?5 AND state_reason IS ?6",
        params![
            view.id,
            store::STATE_PAUSED,
            store::REASON_BLACKLIST,
            at,
            view.state,
            view.reason
        ],
    )?;
    Ok(n == 1)
}

/// Remove a row's grants and its rebuildable data, and (when `to_removed` is `Some`) mark it
/// removed. `to_removed = None` is the repeat that finishes an interrupted removal: no state
/// change, the deletions again. `false` when the marking found the row changed since the plan.
fn remove_row(
    conn: &mut Connection,
    view: &RowView,
    to_removed: Option<&str>,
) -> rusqlite::Result<bool> {
    let tx = conn.transaction()?;
    if let Some(at) = to_removed {
        let n = tx.execute(
            "UPDATE mods SET state = ?2, state_reason = ?3, updated_at = ?4 \
             WHERE id = ?1 AND state = ?5 AND state_reason IS ?6",
            params![
                view.id,
                store::STATE_PAUSED,
                store::REASON_BLACKLIST_REMOVED,
                at,
                view.state,
                view.reason
            ],
        )?;
        if n != 1 {
            return Ok(false);
        }
    }
    tx.execute("DELETE FROM mod_grants WHERE mod_id = ?1", [&view.id])?;
    tx.execute(
        "DELETE FROM mod_kv WHERE mod_id = ?1 AND class = ?2",
        params![view.id, store::KV_CACHE],
    )?;
    // Cache-class by rule in API v0 (SYNC-SET.md): a Rebuild re-creates every document id.
    tx.execute("DELETE FROM mod_doc_data WHERE mod_id = ?1", [&view.id])?;
    tx.execute("DELETE FROM mod_overlay WHERE mod_id = ?1", [&view.id])?;
    clear_waiting(&tx, &view.id, None)?;
    tx.commit()?;
    Ok(true)
}

/// Turn a blacklist pause back on, and record it for the "X is back on" strip. Compare-and-set on
/// the blacklist's own pause; `false` when the row changed since.
fn resume_row(conn: &mut Connection, view: &RowView, at: &str) -> rusqlite::Result<bool> {
    let tx = conn.transaction()?;
    let n = tx.execute(
        "UPDATE mods SET state = ?2, state_reason = NULL, updated_at = ?3 \
         WHERE id = ?1 AND state = ?4 AND state_reason = ?5",
        params![
            view.id,
            store::STATE_ACTIVE,
            at,
            store::STATE_PAUSED,
            store::REASON_BLACKLIST
        ],
    )?;
    if n != 1 {
        return Ok(false);
    }
    note_resumed(&tx, &view.id, &view.version, at)?;
    tx.commit()?;
    Ok(true)
}

// ---- The records slice 6's strips read ----------------------------------------------------------

/// A mod the blacklist turned back on, for "X is back on, fixed in version N".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resumed {
    pub id: String,
    pub version: String,
    pub at: String,
}

/// Why a background update is waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WaitingKind {
    /// It asks for more than was granted (`Refusal::NeedsApproval`): the user approves it.
    Approval,
    /// It needs a newer PM ("update PM to get version N").
    NewerPm,
}

/// A background update that didn't land, keyed by mod id in `settings[mods_updates_waiting]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Waiting {
    pub version: String,
    pub sha256: String,
    pub kind: WaitingKind,
    /// What it asks for beyond what was granted, as the policy names them (`permission:ai`,
    /// `host:example.com`, ...), for the approval screen's first line.
    #[serde(default)]
    pub widenings: Vec<String>,
    /// The API range's `min`, for a [`WaitingKind::NewerPm`] entry.
    #[serde(default)]
    pub needs_api: Option<String>,
}

fn read_setting(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
        r.get(0)
    })
    .optional()
}

fn write_setting(conn: &Connection, key: &str, value: Option<&str>) -> rusqlite::Result<()> {
    match value {
        Some(v) => conn.execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, v],
        ),
        None => conn.execute("DELETE FROM settings WHERE key = ?1", [key]),
    }
    .map(|_| ())
}

/// The waiting updates. A record that can't be read is read as none: it is only ever a note.
pub fn read_waiting(conn: &Connection) -> rusqlite::Result<BTreeMap<String, Waiting>> {
    Ok(read_setting(conn, store::SETTING_UPDATES_WAITING)?
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default())
}

fn write_waiting(conn: &Connection, map: &BTreeMap<String, Waiting>) -> rusqlite::Result<()> {
    if map.is_empty() {
        return write_setting(conn, store::SETTING_UPDATES_WAITING, None);
    }
    let text = serde_json::to_string(map).unwrap_or_else(|_| "{}".into());
    write_setting(conn, store::SETTING_UPDATES_WAITING, Some(&text))
}

/// Record (or replace) the update waiting for `id`.
pub fn set_waiting(conn: &Connection, id: &str, waiting: Waiting) -> rusqlite::Result<()> {
    let mut map = read_waiting(conn)?;
    map.insert(id.to_owned(), waiting);
    write_waiting(conn, &map)
}

/// Forget `id`'s waiting update: always when `installed` is `None` (the mod went), otherwise when
/// the installed version has caught up with it (an approved install, or a newer update). A version
/// that doesn't parse can't be compared, and counts as caught up.
pub fn clear_waiting(conn: &Connection, id: &str, installed: Option<&str>) -> rusqlite::Result<()> {
    let mut map = read_waiting(conn)?;
    let Some(waiting) = map.get(id) else {
        return Ok(());
    };
    let caught_up = match installed {
        None => true,
        Some(v) => match (
            semver::Version::parse(v),
            semver::Version::parse(&waiting.version),
        ) {
            (Ok(have), Ok(want)) => have >= want,
            _ => true,
        },
    };
    if caught_up {
        map.remove(id);
        write_waiting(conn, &map)?;
    }
    Ok(())
}

/// The mods the blacklist resumed, newest last.
pub fn read_resumed(conn: &Connection) -> rusqlite::Result<Vec<Resumed>> {
    Ok(read_setting(conn, store::SETTING_RESUMED)?
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default())
}

fn write_resumed(conn: &Connection, list: &[Resumed]) -> rusqlite::Result<()> {
    if list.is_empty() {
        return write_setting(conn, store::SETTING_RESUMED, None);
    }
    let text = serde_json::to_string(list).unwrap_or_else(|_| "[]".into());
    write_setting(conn, store::SETTING_RESUMED, Some(&text))
}

/// Record that `id` was resumed at `version`: one entry per mod, the latest replacing an older one.
fn note_resumed(conn: &Connection, id: &str, version: &str, at: &str) -> rusqlite::Result<()> {
    let mut list = read_resumed(conn)?;
    list.retain(|r| r.id != id);
    list.push(Resumed {
        id: id.to_owned(),
        version: version.to_owned(),
        at: at.to_owned(),
    });
    write_resumed(conn, &list)
}

/// The strip's dismissal: forget that `id` was resumed.
pub fn dismiss_resumed(conn: &Connection, id: &str) -> rusqlite::Result<()> {
    let mut list = read_resumed(conn)?;
    let before = list.len();
    list.retain(|r| r.id != id);
    if list.len() != before {
        write_resumed(conn, &list)?;
    }
    Ok(())
}
