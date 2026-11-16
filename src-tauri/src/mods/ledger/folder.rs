// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The folder test-mod loader (Mods spec §1.15, plan decision 8, INVARIANTS.md I-35; slice 6,
//! part 4): a developer's unpacked folder, loaded for the session and served from where it is.
//! `mods::folder` reads the folder and keeps the session record; this is the ledger half.
//!
//! **Session only, and what that means for the row.** The dispatcher reads the `mods` row on every
//! call and every grant references it, so a loaded folder has a row (`installed_from = 'folder'`,
//! `package_sha256` NULL, `signed = 0`, tier `third` whatever its manifest says, consented: the user
//! typed its id) for as long as the session lasts, and the row is deleted, never kept:
//! - **unloaded** (the Developer tab's Unload, or the Mods tab's uninstall): the row, and with it
//!   everything that cascades from it (grants, storage, document data, secrets, overlay entries),
//!   goes in one transaction, then the registry forgets the session. Gifts and spend history stay
//!   (I-29), as for any uninstall;
//! - **developer mode goes off:** every folder is unloaded the same way ([`Ledger::unload_folders_held`],
//!   called by the switch before its reload), so nothing of a test mod outlives the switch;
//! - **a restart, or a store opened again** (the lock session's re-open, a vault switch back): the
//!   writer's startup deletes every folder row before it reads any ([`purge_rows`]), and its load
//!   serves a folder row only through a session ([`Ledger::folder_served`]), so a row a crash left
//!   behind is never served and is gone at the next open. The store's close itself can't delete
//!   (the connection is already shut), so the guarantee is "gone at the next open, never served
//!   before"; the folders' sessions are forgotten with the store ([`ModRuntime::clear`]);
//! - **a backup** strips folder rows from its copy (`backup/strip.rs`, track B) and nothing else
//!   carries one out: no file of a test mod is ever inside PM's mods folder (they are the
//!   developer's own, never copied), so the vault's folders, the sweep, the other stores' recovery
//!   and a restore's copying all have nothing of it; the row's `files` and `manifest` columns are in
//!   the database only.
//!
//! A test mod's own storage therefore lasts the session. It isn't the user's content in the I-16
//! sense (the user never chose to keep it, and the screen says test mods and what they store last
//! until PM restarts); what the mod *made* in PM (tasks, notes) is PM's and stays.
//!
//! **What loading checks** is what an unsigned file install checks, in the same order and with the
//! same typed refusals: developer mode on for this device (read from the store here, again in the
//! commit's transaction), the manifest (`package::verify_folder`: every manifest rule, the licence,
//! no `atlas.` id outside a development build), the safety list by id and version, the API range,
//! and the id not held by anything but an earlier loaded folder. D3 across the unsigned set and the
//! typed id are the tab layer's (`tab::folder`), as for the unsigned file.
//!
//! **A reload** (a watcher's change, or the Reload command) rescans the folder and replaces what is
//! served, keeping the grants, the install id and the storage, but only for a manifest that is the
//! same mod from the same maker and asks for nothing more than the user saw
//! ([`policy::decide_reload`]); anything else leaves the old one running and says why on the
//! session, until the developer loads the folder again and the user sees the new screen. A scan that
//! fails (a half-written manifest) does the same, and the next change tries again.

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::Serialize;

use super::unsigned::{held, read_origin, reinstall};
use super::{
    file_records_json, project_refs_json, prune_calendar_hidden, read_current, read_granted,
    time_text, write_row, Committed, Current, Installed, Ledger, LedgerError, LedgerStore, Mint,
    Row, RowWrite, ServeCheck, UnsignedOptions,
};
use crate::mods::devmode;
use crate::mods::disk::{self, Step};
use crate::mods::folder::{self, Scan};
use crate::mods::ids::ModId;
use crate::mods::package::{self, PackageError};
use crate::mods::policy::{
    self, ApiFit, InstallKind, Pinned, Refusal, MOD_API_OLDEST_MAJOR, MOD_API_VERSION,
};
use crate::mods::runtime::InstalledMod;
use crate::mods::safety::{self, SafetyCheck};
use crate::mods::store;
use tauri::{AppHandle, Manager};

/// Whether this build may load an `atlas.` folder: a development build only (spec §1.1, last
/// bullet; plan decision 9). A release build never does, whatever a manifest says.
pub const ALLOW_ATLAS_FOLDERS: bool = cfg!(debug_assertions);

/// What loading a folder did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderLoad {
    pub installed: Installed,
    /// It replaced an earlier loaded folder of the same id (a fresh install id, grants and
    /// secrets reset).
    pub reload: bool,
}

/// What a folder load would be, for the consent screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderPreview {
    pub reload: bool,
    pub installed_version: Option<String>,
}

/// What a reload did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reloaded {
    /// What is served changed (a new scan that matches the old one is a no-op: no frame is
    /// restarted).
    pub changed: bool,
    pub version: String,
}

/// A loaded folder, for the Developer tab (the Mods tab's own row says the rest: name, state).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestModStatus {
    pub id: String,
    /// The folder, as a path to show.
    pub folder: String,
    /// Why the last reload didn't take, in words, until one does.
    pub problem: Option<String>,
    /// PM is watching the folder, so a save reloads it (otherwise, the Reload button).
    pub watching: bool,
}

/// What the checks decided, for the write that follows them.
struct FChecked {
    manifest_text: String,
    current: Option<Current>,
}

impl<S: LedgerStore> Ledger<'_, S> {
    /// Every check a folder load makes, before anything is written, as the consent screen needs
    /// them: or the refusal the load itself would give. Writes nothing. Not a promise: the load
    /// runs every check again, and its commit reads developer mode and the row again.
    pub fn preview_folder(
        &self,
        scan: &Scan,
        opts: &UnsignedOptions<'_>,
    ) -> Result<FolderPreview, LedgerError> {
        let _ledger = self.runtime.ledger_lock();
        let checked = self.folder_checks(scan, opts)?;
        Ok(FolderPreview {
            reload: checked.current.is_some(),
            installed_version: checked.current.map(|c| c.pinned.version),
        })
    }

    fn folder_checks(
        &self,
        scan: &Scan,
        opts: &UnsignedOptions<'_>,
    ) -> Result<FChecked, LedgerError> {
        let m = &scan.pkg.manifest;
        // Developer mode, read here by Rust for this device, in the open store.
        if !self.with_conn(|c| Ok(devmode::is_on(c, opts.device)?))? {
            return Err(Refusal::DevModeOff.into());
        }
        match policy::api_fit(m.api, MOD_API_VERSION, MOD_API_OLDEST_MAJOR) {
            ApiFit::NeedsNewerPm => return Err(Refusal::NeedsNewerPm { min: m.api.min }.into()),
            ApiFit::Retired => return Err(Refusal::ApiRetired.into()),
            ApiFit::Fits | ApiFit::OlderMajor => {}
        }
        // `verify_folder` let an `atlas.` id through only where the build allows it, so a reserved
        // id here is the catalogue mod's own source under test.
        safety::folder_check(
            opts.index,
            m.id.as_str(),
            &m.version.to_string(),
            m.id.is_reserved(),
        )
        .map_err(Refusal::Catalogue)?;
        let manifest_text = manifest_text(scan)?;
        let (current, origin) =
            self.with_conn(|c| Ok((read_current(c, &m.id)?, read_origin(c, &m.id)?)))?;
        let maker = (
            m.publisher.author.as_str(),
            m.publisher.source_repo.as_str(),
        );
        match (&current, &origin) {
            (Some(c), Some(o)) => policy::decide_folder(Some(held(o, c)), maker)?,
            _ => policy::decide_folder(None, maker)?,
        };
        Ok(FChecked {
            manifest_text,
            current,
        })
    }

    /// Load a scanned folder (module doc): the row, the session, and the registry. `now` is the
    /// time of the user's consent (they are looking at the stark screen that asked for this).
    /// Blocking: row writes under the ledger lock (the folder was read by the caller, outside it).
    pub fn load_folder(
        &self,
        scan: &Scan,
        opts: &UnsignedOptions<'_>,
        now: DateTime<Utc>,
    ) -> Result<FolderLoad, LedgerError> {
        let m = &scan.pkg.manifest;
        let _ledger = self.runtime.ledger_lock();
        let FChecked {
            manifest_text,
            current,
        } = self.folder_checks(scan, opts)?;
        let version = m.version.to_string();
        let write = RowWrite {
            id: m.id.as_str(),
            name: &m.name,
            version: &version,
            // Third party and unsigned, whatever the manifest claims: a test folder is never
            // first party, so no band, badge or policy can read it as one.
            tier: crate::mods::ids::Tier::Third.as_str(),
            author: &m.publisher.author,
            source_repo: &m.publisher.source_repo,
            source_commit: &m.publisher.commit,
            signed: false,
            installed_from: store::FROM_FOLDER,
            package_sha256: None,
            manifest: &manifest_text,
            files: file_records_json(&scan.pkg)?,
            project_refs: project_refs_json(m),
            consented_at: Some(time_text(now)),
            install_id: disk::new_install_id().map_err(LedgerError::Disk)?,
            now: time_text(now),
        };
        let served = folder::served(scan).map_err(LedgerError::Disk)?;
        let expected = current.as_ref().map(|c| &c.pinned);
        let Committed { state, consented } =
            self.with_conn(|c| commit_folder(c, &write, opts.device, expected))?;
        // The session is what lets the load serve the row, so it is recorded before the registry.
        self.runtime
            .folders()
            .set(m.id.clone(), scan.root.clone(), served.clone());
        if policy::serves(&state, consented) && self.runtime.unsigned_allowed() {
            self.runtime.install(served);
        } else {
            self.runtime.pause(&m.id);
        }
        Ok(FolderLoad {
            installed: Installed {
                kind: InstallKind::Fresh,
                version,
                state,
                pruned: Vec::new(),
            },
            reload: current.is_some(),
        })
    }

    /// Replace what a loaded folder serves with `scanned`, the result of reading it again, if the
    /// change is one a reload may make (module doc). The caller read the folder before taking the
    /// ledger lock. On any refusal the old one keeps running and the reason is kept on the session.
    pub fn apply_reload(
        &self,
        id: &ModId,
        scanned: Result<Scan, PackageError>,
        check: &dyn ServeCheck,
    ) -> Result<Reloaded, LedgerError> {
        let _ledger = self.runtime.ledger_lock();
        let folders = self.runtime.folders();
        let Some(source) = folders.get(id) else {
            return Err(LedgerError::NotInstalled);
        };
        let outcome = self.reload_inner(id, &source.root, &source.served, scanned, check);
        match &outcome {
            Ok(_) => folders.set_problem(id, None),
            Err(e) => folders.set_problem(id, Some(e.to_string())),
        }
        outcome
    }

    fn reload_inner(
        &self,
        id: &ModId,
        root: &std::path::Path,
        serving: &InstalledMod,
        scanned: Result<Scan, PackageError>,
        check: &dyn ServeCheck,
    ) -> Result<Reloaded, LedgerError> {
        let scan = scanned.map_err(LedgerError::Package)?;
        // Loaded again from another folder while this one was being read: that load's own.
        if scan.root != root {
            return Err(LedgerError::NotInstalled);
        }
        let m = &scan.pkg.manifest;
        let (current, granted, files_now) = self.with_conn(|c| {
            let current = read_current(c, id)?.ok_or(LedgerError::NotInstalled)?;
            let files: String =
                c.query_row("SELECT files FROM mods WHERE id = ?1", [id.as_str()], |r| {
                    r.get(0)
                })?;
            Ok((current, read_granted(c, id)?, files))
        })?;
        policy::decide_reload(
            id.as_str(),
            &current.pinned.author,
            &current.pinned.source_repo,
            &current.manifest,
            m,
            &granted,
        )?;
        let version = m.version.to_string();
        if let Some(why) = check.hold(id.as_str(), &version, None) {
            return Err(Refusal::Catalogue(why).into());
        }
        let manifest_text = manifest_text(&scan)?;
        let served = folder::served(&scan).map_err(LedgerError::Disk)?;
        let files = file_records_json(&scan.pkg)?;
        // Nothing the row records has changed: nothing is written and no frame is restarted.
        if served == *serving && manifest_text == current.manifest && files == files_now {
            return Ok(Reloaded {
                changed: false,
                version,
            });
        }
        let refs = project_refs_json(m);
        let at = time_text(Utc::now());
        self.with_conn(|conn| {
            // Compare-and-set on the row as read: another owner's change since wins (a pause is
            // not touched here: the state is not in this statement).
            let n = conn.execute(
                "UPDATE mods SET name = ?2, version = ?3, source_commit = ?4, manifest = ?5, \
                                 files = ?6, project_refs = ?7, updated_at = ?8 \
                 WHERE id = ?1 AND installed_from = ?9 AND manifest = ?10 AND version = ?11",
                params![
                    id.as_str(),
                    m.name,
                    version,
                    m.publisher.commit,
                    manifest_text,
                    files,
                    refs,
                    at,
                    store::FROM_FOLDER,
                    current.manifest,
                    current.pinned.version,
                ],
            )?;
            if n == 1 {
                Ok(())
            } else {
                Err(Refusal::Changed.into())
            }
        })?;
        let replaced = served != *serving;
        self.runtime.folders().update(id, served.clone());
        // The registry changes only if what is served did (a changed notice restarts no frame), and
        // only if the row serves: a paused folder is read and recorded, and comes back on the
        // user's resume (which loads the registry from the session).
        if replaced
            && policy::serves(&current.state, current.consented)
            && self.runtime.unsigned_allowed()
        {
            self.runtime.install(served);
        }
        Ok(Reloaded {
            changed: true,
            version,
        })
    }

    /// Unload one folder: its row and what cascades from it, then the session and the registry
    /// entry. Never touches a file: the folder is the developer's. Refused for any row that isn't a
    /// loaded folder, so this can never delete an installed mod.
    pub fn unload_folder(&self, id: &ModId) -> Result<(), LedgerError> {
        let _ledger = self.runtime.ledger_lock();
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            let n = tx.execute(
                "DELETE FROM mods WHERE id = ?1 AND installed_from = ?2",
                params![id.as_str(), store::FROM_FOLDER],
            )?;
            if n == 0 {
                return Err(LedgerError::NotInstalled);
            }
            prune_calendar_hidden(&tx, id)?;
            disk::failpoint(Step::Commit).map_err(LedgerError::Disk)?;
            tx.commit()?;
            Ok(())
        })?;
        self.runtime.uninstall(id);
        Ok(())
    }

    /// Unload every folder (developer mode went off). The caller holds the ledger lock. The rows go
    /// first; if that fails nothing is served from them anyway (the load serves a folder only
    /// through a session, and every session is forgotten here regardless), and the next writer open
    /// deletes them.
    pub(super) fn unload_folders_held(&self) -> Result<Vec<String>, LedgerError> {
        let gone = self.with_conn(purge_rows);
        let sessions: Vec<ModId> = self
            .runtime
            .folders()
            .roots()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        for id in &sessions {
            self.runtime.uninstall(id);
        }
        self.runtime.folders().forget_all();
        let gone = gone?;
        for id in gone.iter().filter_map(|i| ModId::parse(i).ok()) {
            // A row with no session (a crash's) was never in the registry; this is a no-op for it.
            self.runtime.uninstall(&id);
        }
        Ok(gone)
    }

    /// What the load serves for a folder row: its session's registry entry, only while the row serves
    /// (active and consented), developer mode is on, and the safety list doesn't hold it. A row with
    /// no session serves nothing.
    pub(super) fn folder_served(&self, row: &Row, check: &dyn ServeCheck) -> Option<InstalledMod> {
        if !self.runtime.unsigned_allowed() || !policy::serves(&row.state, row.consented) {
            return None;
        }
        let id = ModId::parse(&row.id).ok()?;
        let served = self.runtime.folders().served(&id)?;
        // A folder has no package hash, so the list is read by id and version only.
        check
            .hold(&row.id, &row.version, None)
            .is_none()
            .then_some(served)
    }
}

/// `manifest.json` as read, as text.
fn manifest_text(scan: &Scan) -> Result<String, LedgerError> {
    scan.pkg
        .files
        .get(package::MANIFEST)
        .and_then(|f| String::from_utf8(f.bytes.clone()).ok())
        .ok_or_else(|| {
            LedgerError::Package(PackageError {
                refusal: package::Refusal::Manifest,
                detail: "its manifest isn't text".into(),
            })
        })
}

/// The row, in one transaction that reads developer mode and the row again: a switch-off since the
/// checks, or a change to the row, refuses the load.
fn commit_folder(
    conn: &mut Connection,
    w: &RowWrite<'_>,
    device: &str,
    expected: Option<&Pinned>,
) -> Result<Committed, LedgerError> {
    let tx = conn.transaction()?;
    if !devmode::is_on(&tx, device)? {
        return Err(Refusal::DevModeOff.into());
    }
    let id = ModId::parse(w.id).map_err(|_| Refusal::UnreadableRecord)?;
    let current = read_current(&tx, &id)?;
    if current.as_ref().map(|c| &c.pinned) != expected {
        return Err(Refusal::Changed.into());
    }
    let committed = match &current {
        None => write_row(&tx, w, InstallKind::Fresh, None)?,
        Some(cur) => {
            let origin = read_origin(&tx, &id)?.ok_or(Refusal::Changed)?;
            policy::decide_folder(Some(held(&origin, cur)), (w.author, w.source_repo))?;
            // A fresh install id, grants and secrets reset, storage kept: the same as an unsigned
            // file over an earlier unsigned file, whose writer this is.
            reinstall(&tx, w, cur)?
        }
    };
    disk::failpoint(Step::Commit).map_err(LedgerError::Disk)?;
    tx.commit()?;
    Ok(committed)
}

/// Delete every folder row and what cascades from it (module doc), and each one's overlay entry in
/// the calendar's hidden set, in one transaction. Returns the ids. Used by the writer's startup and
/// by developer mode going off.
pub(super) fn purge_rows(conn: &mut Connection) -> Result<Vec<String>, LedgerError> {
    let tx = conn.transaction()?;
    let ids: Vec<String> = {
        let mut stmt = tx.prepare("SELECT id FROM mods WHERE installed_from = ?1 ORDER BY id")?;
        let rows = stmt.query_map([store::FROM_FOLDER], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    if ids.is_empty() {
        return Ok(ids);
    }
    tx.execute(
        "DELETE FROM mods WHERE installed_from = ?1",
        [store::FROM_FOLDER],
    )?;
    for id in ids.iter().filter_map(|i| ModId::parse(i).ok()) {
        prune_calendar_hidden(&tx, &id)?;
    }
    disk::failpoint(Step::Commit).map_err(LedgerError::Disk)?;
    tx.commit()?;
    Ok(ids)
}

// ---- Wiring into the app ----------------------------------------------------------------------

/// Read the loaded folder `id` again and replace what it serves (module doc, "A reload"): the
/// watcher's and the Reload command's one entry. The folder is read before the ledger lock is taken.
/// Blocking.
pub fn reload_test_mod(app: &AppHandle, id: &ModId) -> Result<Reloaded, LedgerError> {
    let Some(runtime) = app.try_state::<crate::mods::runtime::ModRuntime>() else {
        return Err(LedgerError::Store("mods aren't running".into()));
    };
    let Some(source) = runtime.folders().get(id) else {
        return Err(LedgerError::NotInstalled);
    };
    let scanned = folder::scan(&source.root, ALLOW_ATLAS_FOLDERS);
    let cached = crate::mods::refresh::cached_index(app);
    let check = SafetyCheck::serve(cached.as_ref());
    super::with_active_ledger(app, Mint::IfRows, |ledger| {
        ledger.apply_reload(id, scanned, &check)
    })
    .unwrap_or_else(|| Err(LedgerError::Store("PM's data isn't open".into())))
}

/// Unload one loaded folder.
pub fn unload_test_mod(app: &AppHandle, id: &ModId) -> Result<(), LedgerError> {
    super::with_active_ledger(app, Mint::IfRows, |ledger| ledger.unload_folder(id))
        .unwrap_or_else(|| Err(LedgerError::Store("PM's data isn't open".into())))
}

/// The loaded folders, for the Developer tab.
pub fn list_test_mods(app: &AppHandle) -> Vec<TestModStatus> {
    let Some(runtime) = app.try_state::<crate::mods::runtime::ModRuntime>() else {
        return Vec::new();
    };
    let watch = app.try_state::<crate::mods::folder_watch::FolderWatch>();
    runtime
        .folders()
        .roots()
        .into_iter()
        .filter_map(|(id, root)| {
            let source = runtime.folders().get(&id)?;
            Some(TestModStatus {
                id: id.as_str().to_owned(),
                folder: root.to_string_lossy().into_owned(),
                problem: source.problem,
                watching: watch.as_ref().is_some_and(|w| w.watching(&root)),
            })
        })
        .collect()
}
