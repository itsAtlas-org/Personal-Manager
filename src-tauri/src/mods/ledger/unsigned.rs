// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The unsigned file install (Mods spec §1.1, §1.15, D3; slice 6, part 3): a third-party `.pmmod`
//! with no signature, installed from the Mods tab while developer mode is on for this device. It is
//! the signed install's sibling, not a flag on it: the checks differ, the row says `signed = 0`,
//! and it is always a *fresh* install (a developer reinstalls the version they are working on, so
//! there is no version rule and no pin to match).
//!
//! **What it refuses, before anything is written** (`unsigned_checks`, run by the preview and again
//! by the install):
//! - a package that carries a signature (it goes through the signed install, never this one);
//! - developer mode off for this device in this vault, read from the store's own record here
//!   ([`devmode::is_on`]), never trusted from a caller; and the install's commit reads it again
//!   in its own transaction, so a switch-off while the files were staging wins;
//! - a manifest that doesn't say it is third party, and every `atlas.` id (`ids::check_tier`);
//! - an id the signed catalogue lists, or a file the blacklist names by id and version or by its
//!   sha256 (`safety::unsigned_check`);
//! - the API range this PM doesn't serve, and (Windows) paths that don't fit;
//! - an id held by an installed mod that didn't come from an unsigned file: a signed mod's id can't
//!   be taken over by a file anyone can write, and a removed mod's row waits for its uninstall
//!   ([`policy::decide_unsigned`]).
//!
//! **Over an earlier unsigned install of the same id** the commit mints a *new install id* (v60's
//! `mod_ledger` names an unsigned maker by it, so the new file can't edit what the old one made,
//! `policy::made_it`), resets the grants and the secrets (the user re-approves on the stark screen
//! again), and keeps the mod's own storage, its content included (I-16: never delete what the user
//! kept without their say-so). The old version's folder is pruned after the row commits; the same
//! version's folder is quarantined and put back if the install then fails. Only from the SAME maker
//! (the author and repository the stored row shows): the kept storage is the first maker's, and a
//! file naming another author or repository would be served it, so that is refused
//! ([`Refusal::MakerChanged`]) and the developer uninstalls first, choosing what happens to the data.
//!
//! D3 across the unsigned set is judged BEFORE the consent screen is offered and again before the
//! install (`tab::unsigned`, `tab::consent::plan_unsigned`); this file is the ledger half.

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use super::{
    file_records_json, project_refs_json, read_current, time_text, write_row, Committed, Current,
    Installed, Ledger, LedgerError, LedgerStore, RowWrite,
};
use crate::mods::devmode;
use crate::mods::disk::{self, Step, VersionName};
use crate::mods::ids::{ModId, Tier};
use crate::mods::index::Index;
use crate::mods::package::{self, PackageError, VerifiedPackage};
use crate::mods::policy::{self, HeldBy, InstallKind, Pinned, Refusal};
use crate::mods::runtime::InstalledMod;
use crate::mods::{safety, store};

/// What an unsigned install is checked against.
pub struct UnsignedOptions<'a> {
    /// This device in the open vault (`devmode::Here`): developer mode is per device and per vault.
    pub device: &'a devmode::Here,
    /// The cached safety list, for the catalogue's ids and the blacklist.
    pub index: Option<&'a Index>,
}

/// What an unsigned install did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsignedInstall {
    pub installed: Installed,
    /// It replaced an earlier unsigned install of the same id (a fresh install id, grants and
    /// secrets reset).
    pub reinstall: bool,
}

/// What an unsigned install would be, for the consent screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsignedPreview {
    pub reinstall: bool,
    /// The version of the install it would replace.
    pub installed_version: Option<String>,
}

/// What the checks decided, for the write that follows them.
struct UChecked {
    version: VersionName,
    manifest_text: String,
    current: Option<Current>,
}

/// The facts about the row that holds an id that an unsigned file reads.
pub(super) struct Origin {
    signed: bool,
    installed_from: String,
    author: String,
    source_repo: String,
}

pub(super) fn read_origin(conn: &Connection, id: &ModId) -> rusqlite::Result<Option<Origin>> {
    conn.query_row(
        "SELECT signed, installed_from, author, source_repo FROM mods WHERE id = ?1",
        [id.as_str()],
        |r| {
            Ok(Origin {
                signed: r.get(0)?,
                installed_from: r.get(1)?,
                author: r.get(2)?,
                source_repo: r.get(3)?,
            })
        },
    )
    .optional()
}

pub(super) fn held<'a>(origin: &'a Origin, current: &'a Current) -> HeldBy<'a> {
    HeldBy {
        signed: origin.signed,
        installed_from: &origin.installed_from,
        reason: current.reason.as_deref(),
        author: &origin.author,
        source_repo: &origin.source_repo,
    }
}

impl<S: LedgerStore> Ledger<'_, S> {
    /// Every check an unsigned install makes, before anything is written, as the consent screen needs
    /// them: or the refusal the install itself would give. Writes nothing. Not a promise: the install
    /// runs every check again, and its commit reads developer mode and the row again.
    pub fn preview_unsigned(
        &self,
        pkg: &VerifiedPackage,
        package_sha256: &str,
        opts: &UnsignedOptions<'_>,
    ) -> Result<UnsignedPreview, LedgerError> {
        let _ledger = self.runtime.ledger_lock();
        let checked = self.unsigned_checks(pkg, package_sha256, opts)?;
        Ok(UnsignedPreview {
            reinstall: checked.current.is_some(),
            installed_version: checked.current.map(|c| c.pinned.version),
        })
    }

    fn unsigned_checks(
        &self,
        pkg: &VerifiedPackage,
        package_sha256: &str,
        opts: &UnsignedOptions<'_>,
    ) -> Result<UChecked, LedgerError> {
        let m = &pkg.manifest;
        // A signed package is the signed install's, with the Atlas keys; never this path, where
        // nothing would be checked against a signature.
        if pkg.is_signed() {
            return Err(LedgerError::Package(PackageError {
                refusal: package::Refusal::Signed,
                detail: "it is signed, so it installs as a signed file".into(),
            }));
        }
        // Developer mode, read here by Rust for this device, in the open store.
        self.bound_to(opts.device)?;
        if !self.with_conn(|c| Ok(devmode::is_on(c, opts.device)?))? {
            return Err(Refusal::DevModeOff.into());
        }
        if m.publisher.tier != Tier::Third {
            return Err(Refusal::Tier("an unsigned file must say it is third party".into()).into());
        }
        let version = self.static_checks(pkg)?;
        safety::unsigned_check(
            opts.index,
            m.id.as_str(),
            &m.version.to_string(),
            package_sha256,
        )
        .map_err(Refusal::Catalogue)?;
        let manifest_text = pkg
            .files
            .get(package::MANIFEST)
            .and_then(|f| String::from_utf8(f.bytes.clone()).ok())
            .ok_or_else(|| {
                LedgerError::Package(PackageError {
                    refusal: package::Refusal::Manifest,
                    detail: "its manifest isn't text".into(),
                })
            })?;
        let (current, origin) =
            self.with_conn(|c| Ok((read_current(c, &m.id)?, read_origin(c, &m.id)?)))?;
        let maker = (
            m.publisher.author.as_str(),
            m.publisher.source_repo.as_str(),
        );
        match (&current, &origin) {
            (Some(c), Some(o)) => policy::decide_unsigned(Some(held(o, c)), maker)?,
            _ => policy::decide_unsigned(None, maker)?,
        };
        Ok(UChecked {
            version,
            manifest_text,
            current,
        })
    }

    /// Install an unsigned package (module doc). `package_sha256` is the `.pmmod`'s own, as the
    /// consent screen showed it. Blocking: files and rows, under the ledger lock.
    pub fn install_unsigned(
        &self,
        pkg: &VerifiedPackage,
        package_sha256: &str,
        opts: &UnsignedOptions<'_>,
        now: DateTime<Utc>,
    ) -> Result<UnsignedInstall, LedgerError> {
        let m = &pkg.manifest;
        let _ledger = self.runtime.ledger_lock();
        let UChecked {
            version,
            manifest_text,
            current,
        } = self.unsigned_checks(pkg, package_sha256, opts)?;
        let write = RowWrite {
            id: m.id.as_str(),
            name: &m.name,
            version: version.as_str(),
            tier: m.publisher.tier.as_str(),
            author: &m.publisher.author,
            source_repo: &m.publisher.source_repo,
            source_commit: &m.publisher.commit,
            signed: false,
            installed_from: store::FROM_FILE,
            package_sha256: Some(package_sha256),
            manifest: &manifest_text,
            files: file_records_json(pkg)?,
            project_refs: project_refs_json(m),
            // The user is looking at the stark screen that asked for this.
            consented_at: Some(time_text(now)),
            // A fresh install id, always: a reinstall is a fresh install too.
            install_id: disk::new_install_id().map_err(LedgerError::Disk)?,
            now: time_text(now),
        };

        let staged = disk::stage(self.layout, &pkg.files).map_err(LedgerError::Disk)?;
        // The same version again replaces its own folder (put back if this fails).
        let same_version = current
            .as_ref()
            .is_some_and(|c| c.pinned.version == version.as_str());
        let placed = disk::place(self.layout, staged, &m.id, &version, same_version, now)
            .map_err(LedgerError::Disk)?;
        let installed = InstalledMod::from_package(pkg, placed.dir()).map_err(LedgerError::Disk)?;
        let expected = current.as_ref().map(|c| &c.pinned);
        self.bound_to(opts.device)?;
        let Committed { state, consented } =
            self.with_conn(|c| commit_unsigned(c, &write, opts.device, expected))?;
        placed.keep();
        // Served only while the registry's own copy of developer mode agrees with the store's.
        if policy::serves(&state, consented) && self.runtime.unsigned_allowed() {
            self.runtime.install(installed);
        } else {
            self.runtime.pause(&m.id);
        }
        let reinstall = current.is_some();
        let pruned = if reinstall {
            disk::prune_versions(self.layout, &m.id, &version)
        } else {
            Vec::new()
        };
        Ok(UnsignedInstall {
            installed: Installed {
                kind: InstallKind::Fresh,
                version: version.as_str().to_owned(),
                state,
                pruned,
            },
            reinstall,
        })
    }
}

/// The row, in one transaction that reads developer mode and the row again: a switch-off since the
/// checks, or a change to the row, refuses the install (its files are rolled back by the caller).
pub(super) fn commit_unsigned(
    conn: &mut Connection,
    w: &RowWrite<'_>,
    device: &devmode::Here,
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
            policy::decide_unsigned(Some(held(&origin, cur)), (w.author, w.source_repo))?;
            reinstall(&tx, w, cur)?
        }
    };
    disk::failpoint(Step::Commit).map_err(LedgerError::Disk)?;
    tx.commit()?;
    Ok(committed)
}

/// An unsigned file over an earlier unsigned install of the same id: a fresh install id, no grants,
/// no secrets, and everything else the row holds (storage, document data, spend) kept.
pub(super) fn reinstall(
    tx: &Connection,
    w: &RowWrite<'_>,
    cur: &Current,
) -> Result<Committed, LedgerError> {
    // Developer mode is on (the caller just read it), so a pause that developer mode being off
    // made is over; any other owner's pause stays.
    let (state, reason) = if cur.reason.as_deref() == Some(store::REASON_DEV_MODE) {
        (store::STATE_ACTIVE.to_owned(), None)
    } else {
        policy::state_after_install(Some((cur.state.as_str(), cur.reason.as_deref())))
    };
    tx.execute("DELETE FROM mod_grants WHERE mod_id = ?1", [w.id])?;
    tx.execute("DELETE FROM mod_secrets WHERE mod_id = ?1", [w.id])?;
    let changed = tx.execute(
        "UPDATE mods SET name = ?2, version = ?3, tier = ?4, author = ?5, source_repo = ?6, \
                         source_commit = ?7, signed = 0, install_id = ?8, installed_from = ?9, \
                         package_sha256 = ?10, manifest = ?11, files = ?12, project_refs = ?13, \
                         state = ?14, state_reason = ?15, consented_at = ?16, updated_at = ?17 \
         WHERE id = ?1",
        params![
            w.id,
            w.name,
            w.version,
            w.tier,
            w.author,
            w.source_repo,
            w.source_commit,
            w.install_id,
            w.installed_from,
            w.package_sha256,
            w.manifest,
            w.files,
            w.project_refs,
            state,
            reason,
            w.consented_at,
            w.now,
        ],
    )?;
    if changed != 1 {
        return Err(Refusal::Changed.into());
    }
    Ok(Committed {
        state,
        consented: true,
    })
}
