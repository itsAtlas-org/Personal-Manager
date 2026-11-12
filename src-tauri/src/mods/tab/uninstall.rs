// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Uninstalling from the Mods tab (Mods spec §1.3 "Uninstall dialog", §6, INVARIANTS.md I-16): first
//! [`preview`], which says what leaves and what stays, then [`run`], which uninstalls through the
//! ledger and, only when asked, deletes what the mod made.
//!
//! **What leaves** with the row (v60's cascade): the mod's private storage (the content the user
//! may value included, which is why the dialog shows its size), its grants, secrets, calendar
//! overlay entries and AI reservations, its files, and its tab. **What stays:** the tasks and
//! milestones it made (`mod_ledger`, outside the cascade, so "made by X" survives and a reinstall
//! can edit its own again) and the documents whose `source_id` begins `mod:<id>:`. Those are the
//! user's own, and I-16 says nothing the user made is deleted without their say-so: the dialog's
//! "also delete the N things it made?" is that say-so, one flag, answered per uninstall.
//!
//! **Deleting what it made goes through PM's own delete paths**, never raw SQL on user tables: a
//! task through `tasks::remove` (and the project touch the delete command does), a milestone through
//! `milestones::remove` (its flags and links go with it), a document through
//! `ingest::delete_document` (chunks, tags, mod events), each in its own step. The uninstall
//! itself comes first and is the one that matters; a gift that couldn't be deleted is reported, not
//! retried silently, and the rest still go.
//!
//! The answer also names what the page prunes from `localStorage` ([`Prune`]), and the caller
//! announces `prefs://stored` for the calendar's blob, which the ledger already pruned.

use std::path::Path;

use rusqlite::{params, Connection};
use serde::Serialize;

use super::error::Result;
use super::listing;
use crate::error::Error as AppError;
use crate::mods::ids::ModId;
use crate::mods::ledger::{Ledger, LedgerError, LedgerStore};
use crate::mods::store;

/// Entries a folder walk looks at before it stops counting: a size for a dialog, never a hang.
const WALK_LIMIT: usize = 200_000;

/// What the dialog shows before anything happens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallPreview {
    pub id: String,
    pub name: String,
    pub leaves: Leaves,
    pub stays: Stays,
    /// The safety list removed it: the dialog says why it is being uninstalled.
    pub removed_by_safety_list: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Leaves {
    /// Its private storage in the database: content the user may value, and cache.
    pub content_bytes: u64,
    pub cache_bytes: u64,
    /// Per-document data (always rebuildable cache).
    pub document_data_bytes: u64,
    /// Its files on disk.
    pub files_bytes: u64,
    /// Its tab, by label.
    pub nav_tab: Option<String>,
    pub calendar_overlay: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stays {
    pub tasks: u64,
    pub milestones: u64,
    /// Documents whose `source_id` begins `mod:<id>:`.
    pub documents: u64,
}

/// The folder's size in bytes: regular files only, links not followed, at most [`WALK_LIMIT`]
/// entries looked at. A folder that isn't there is 0. Blocking.
pub fn folder_bytes(root: &Path) -> u64 {
    let mut total = 0u64;
    let mut seen = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            seen += 1;
            if seen > WALK_LIMIT {
                return total;
            }
            let Ok(meta) = entry.file_type().and_then(|t| {
                if t.is_dir() {
                    Ok(None)
                } else if t.is_file() {
                    entry.metadata().map(|m| Some(m.len()))
                } else {
                    Ok(Some(0)) // a link or special file: not followed, not counted
                }
            }) else {
                continue;
            };
            match meta {
                None => stack.push(entry.path()),
                Some(len) => total += len,
            }
        }
    }
    total
}

/// The `source_id` prefix of the documents a mod made.
pub fn document_prefix(id: &ModId) -> String {
    format!("mod:{id}:")
}

fn count(conn: &Connection, sql: &str, id: &str) -> rusqlite::Result<u64> {
    conn.query_row(sql, [id], |r| r.get::<_, i64>(0))
        .map(|n| n.max(0) as u64)
}

fn bytes(conn: &Connection, sql: &str, args: impl rusqlite::Params) -> rusqlite::Result<u64> {
    conn.query_row(sql, args, |r| r.get::<_, i64>(0))
        .map(|n| n.max(0) as u64)
}

/// The database half of the preview, for an installed `id`.
struct Stored {
    name: String,
    reason: Option<String>,
    /// Without the files' size, which is read off the disk.
    leaves: Leaves,
    stays: Stays,
}

fn read(conn: &Connection, id: &ModId) -> rusqlite::Result<Option<Stored>> {
    let row: Option<(String, String, Option<String>)> = {
        let mut stmt =
            conn.prepare("SELECT name, manifest, state_reason FROM mods WHERE id = ?1")?;
        let mut rows = stmt.query_map([id.as_str()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.next().transpose()?
    };
    let Some((name, manifest, reason)) = row else {
        return Ok(None);
    };
    let kv = |class: &str| {
        bytes(
            conn,
            "SELECT COALESCE(SUM(length(CAST(value AS BLOB))), 0) FROM mod_kv \
             WHERE mod_id = ?1 AND class = ?2",
            params![id.as_str(), class],
        )
    };
    let view = listing::read_manifest(&manifest);
    let leaves = Leaves {
        content_bytes: kv(store::KV_CONTENT)?,
        cache_bytes: kv(store::KV_CACHE)?,
        document_data_bytes: bytes(
            conn,
            "SELECT COALESCE(SUM(length(CAST(value AS BLOB))), 0) FROM mod_doc_data WHERE mod_id = ?1",
            [id.as_str()],
        )?,
        files_bytes: 0,
        nav_tab: view.nav_tab,
        calendar_overlay: view.calendar_overlay,
    };
    let prefix = document_prefix(id);
    let stays = Stays {
        tasks: count(
            conn,
            "SELECT count(*) FROM mod_ledger WHERE mod_id = ?1 AND task_id IS NOT NULL",
            id.as_str(),
        )?,
        milestones: count(
            conn,
            "SELECT count(*) FROM mod_ledger WHERE mod_id = ?1 AND milestone_id IS NOT NULL",
            id.as_str(),
        )?,
        documents: bytes(
            conn,
            "SELECT count(*) FROM documents WHERE substr(source_id, 1, length(?1)) = ?1",
            [prefix],
        )?,
    };
    Ok(Some(Stored {
        name,
        reason,
        leaves,
        stays,
    }))
}

/// What uninstalling `id` would take and leave. Blocking: a folder walk, then one store read (the
/// walk first, so the DB lock isn't held across file I/O).
pub fn preview<S: LedgerStore>(ledger: &Ledger<'_, S>, id: &ModId) -> Result<UninstallPreview> {
    let files_bytes = folder_bytes(&ledger.layout().mod_dir(id));
    let mut stored = ledger
        .with_conn(|c| Ok(read(c, id)?))?
        .ok_or_else(super::error::ModsError::not_installed)?;
    stored.leaves.files_bytes = files_bytes;
    Ok(UninstallPreview {
        id: id.as_str().to_owned(),
        name: stored.name,
        leaves: stored.leaves,
        stays: stored.stays,
        removed_by_safety_list: stored.reason.as_deref() == Some(store::REASON_BLACKLIST_REMOVED),
    })
}

/// What the page removes from this device's `localStorage` after an uninstall (Mods spec §1.3: the
/// mod's id in sidebar placement and fold prefs). The calendar's blob is the store's, pruned by the
/// ledger, and announced by the caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prune {
    /// The tab id to drop from `pm.nav.placement`'s `hidden` and `pinned` lists.
    pub tab: String,
    /// Every `pm.fold.` key (from `foldPrefs`) that, after that prefix, is exactly this or begins
    /// with this and a `:` came from the mod's rows in the Mods tab and should go. The Mods tab
    /// keys a mod's folds `mods.mod.<id>` and `mods.mod.<id>:<section>`: ids never hold a `:`, so
    /// `acme.notes` can't be mistaken for `acme.notes.extra`.
    pub fold_prefix: String,
}

pub fn prune_of(id: &ModId) -> Prune {
    Prune {
        tab: format!("mod:{id}"),
        fold_prefix: format!("mods.mod.{id}"),
    }
}

/// What the mod made, deleted at the user's word.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GiftsDeleted {
    pub tasks: u64,
    pub milestones: u64,
    pub documents: u64,
    /// The first thing that couldn't be deleted, in words; the rest went regardless.
    pub problem: Option<String>,
}

/// What an uninstall did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Uninstalled {
    pub id: String,
    /// False when the folder couldn't be removed; the next launch's sweep quarantines it.
    pub files_removed: bool,
    /// `None` unless the user asked for the gifts to go.
    pub gifts_deleted: Option<GiftsDeleted>,
    pub prune: Prune,
}

/// The ids of what `id` made, from the ledger that outlives its row.
struct Gifts {
    tasks: Vec<i64>,
    milestones: Vec<i64>,
    documents: Vec<i64>,
}

fn read_gifts(conn: &Connection, id: &ModId) -> rusqlite::Result<Gifts> {
    let ids = |sql: &str, arg: String| -> rusqlite::Result<Vec<i64>> {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([arg], |r| r.get::<_, i64>(0))?;
        rows.collect()
    };
    Ok(Gifts {
        tasks: ids(
            "SELECT task_id FROM mod_ledger WHERE mod_id = ?1 AND task_id IS NOT NULL",
            id.as_str().to_owned(),
        )?,
        milestones: ids(
            "SELECT milestone_id FROM mod_ledger WHERE mod_id = ?1 AND milestone_id IS NOT NULL",
            id.as_str().to_owned(),
        )?,
        documents: ids(
            "SELECT id FROM documents WHERE substr(source_id, 1, length(?1)) = ?1",
            document_prefix(id),
        )?,
    })
}

/// Delete one task the way the delete command does: the row, then a touch of its project.
fn delete_task(conn: &Connection, task: i64) -> crate::error::Result<()> {
    let project = crate::tasks::project_of(conn, task)?;
    crate::tasks::remove(conn, task)?;
    if let Some(project) = project {
        crate::projects::touch(conn, &project)?;
    }
    Ok(())
}

/// Delete one milestone the way the delete command does (its flags go inside `remove`), then touch
/// its project and record the observation.
fn delete_milestone(conn: &Connection, milestone: i64) -> crate::error::Result<()> {
    let project = crate::milestones::project_of(conn, milestone)?;
    crate::milestones::remove(conn, milestone)?;
    if let Some(project) = project {
        crate::projects::touch(conn, &project)?;
        crate::project_activity::record(
            conn,
            &project,
            crate::project_activity::Kind::Milestone,
            Some(milestone),
        );
    }
    Ok(())
}

/// Delete the mod's gifts through PM's delete paths, one at a time, collecting how many went and
/// the first failure. Blocking, on the store through the ledger's binding.
fn delete_gifts<S: LedgerStore>(ledger: &Ledger<'_, S>, id: &ModId) -> GiftsDeleted {
    let mut out = GiftsDeleted::default();
    let gifts = match ledger.with_conn(|c| Ok(read_gifts(c, id)?)) {
        Ok(g) => g,
        Err(e) => {
            out.problem = Some(e.to_string());
            return out;
        }
    };
    let fail = |out: &mut GiftsDeleted, e: String| {
        out.problem.get_or_insert(e);
    };
    for task in gifts.tasks {
        match ledger.with_conn(|c| delete_task(c, task).map_err(into_ledger)) {
            Ok(()) => out.tasks += 1,
            Err(e) => fail(&mut out, e.to_string()),
        }
    }
    for milestone in gifts.milestones {
        match ledger.with_conn(|c| delete_milestone(c, milestone).map_err(into_ledger)) {
            Ok(()) => out.milestones += 1,
            Err(e) => fail(&mut out, e.to_string()),
        }
    }
    for doc in gifts.documents {
        let deleted = ledger.with_conn(|c| {
            let tx = c.transaction()?;
            crate::ingest::delete_document(&tx, doc).map_err(into_ledger)?;
            tx.commit()?;
            Ok(())
        });
        match deleted {
            Ok(()) => out.documents += 1,
            Err(e) => fail(&mut out, e.to_string()),
        }
    }
    out
}

fn into_ledger(e: AppError) -> LedgerError {
    LedgerError::Store(e.to_string())
}

/// Uninstall `id`, and delete what it made when `also_delete_gifts`. The uninstall goes first: if
/// it fails nothing was deleted, and if a gift then can't be deleted the mod is still gone.
/// Blocking.
pub fn run<S: LedgerStore>(
    ledger: &Ledger<'_, S>,
    id: &ModId,
    also_delete_gifts: bool,
) -> Result<Uninstalled> {
    let done = ledger.uninstall(id)?;
    let gifts_deleted = also_delete_gifts.then(|| delete_gifts(ledger, id));
    Ok(Uninstalled {
        id: id.as_str().to_owned(),
        files_removed: done.files_removed,
        gifts_deleted,
        prune: prune_of(id),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn a_folders_size_counts_files_only_and_a_missing_folder_is_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(folder_bytes(&dir.path().join("gone")), 0);
        fs::create_dir_all(dir.path().join("a/b")).unwrap();
        fs::write(dir.path().join("a/x"), [0u8; 10]).unwrap();
        fs::write(dir.path().join("a/b/y"), [0u8; 25]).unwrap();
        fs::write(dir.path().join("z"), [0u8; 5]).unwrap();
        assert_eq!(folder_bytes(dir.path()), 40);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_neither_followed_nor_counted() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("big"), vec![0u8; 4096]).unwrap();
        fs::create_dir(dir.path().join("mod")).unwrap();
        fs::write(dir.path().join("mod/own"), [0u8; 7]).unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("mod/dir-link")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("big"), dir.path().join("mod/file-link"))
            .unwrap();
        assert_eq!(folder_bytes(&dir.path().join("mod")), 7);
    }

    #[test]
    fn what_the_page_prunes_names_the_tab_and_the_fold_prefix_of_exactly_this_mod() {
        let p = prune_of(&ModId::parse("acme.notes").unwrap());
        assert_eq!(p.tab, "mod:acme.notes");
        assert_eq!(p.fold_prefix, "mods.mod.acme.notes");
        // The page matches the prefix exactly or followed by `:`, and an id never holds a `:`.
        assert_eq!(
            document_prefix(&ModId::parse("acme.notes").unwrap()),
            "mod:acme.notes:"
        );
    }
}
