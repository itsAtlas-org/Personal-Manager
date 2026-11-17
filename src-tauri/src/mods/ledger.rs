// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The install ledger (Mods spec §1.3, §6): install, update, repair and uninstall, each keeping the
//! `mods` row and the files it names in step, and the startup load and sweep that rebuild the
//! registry from the rows without believing the disk.
//!
//! **Order, and what each failure leaves:**
//! - **Install, update, repair.** Every policy check runs before anything is written ([`policy`]).
//!   Then the verified bytes go into a fresh staging folder, every file and folder fsynced; a rename
//!   puts it at `<id>/<version>/`; one transaction writes the row after re-reading what the checks
//!   read; then the registry. A failure before the commit moves the folder back into staging and
//!   removes it (a repair's old folder goes back), so no row ever names missing files and no folder
//!   is left outside staging and quarantine. An update's old version serves until its row is
//!   replaced, and its folder is pruned only after that.
//! - **Uninstall.** One transaction deletes the row (v60's cascade takes grants, storage, document
//!   data, secrets, overlay entries and reservations; `mod_ledger` and `usage_log.mod_id` stay,
//!   INVARIANTS.md I-29) and the mod's entry in the calendar's hidden set; then the registry forgets
//!   it; then its folder goes. A folder that won't go has no row, so the next sweep quarantines it.
//! - **Startup, as the writer only** ([`after_writer_open`]). Every row is checked against the disk
//!   and loaded, or parked with its reason, never deleted (a version folder a sweep quarantined is
//!   put back first when the row names it again); then the sweep's moves run from that same
//!   positive listing, and its deletions (staging, expired quarantine) on a blocking thread after.
//!   A listing that can't be read loads nothing and sweeps nothing (I-30).
//!
//! **Each store has its own folder** (`disk::Layout`): the vault id names the vault's, and a random
//! store id kept in the store's own `settings` ([`STORE_ID_KEY`]) names the store's inside it. A
//! restore keeps the vault id, so without the store id a restored copy and the vault it came from
//! would share one folder and each would sweep, prune and uninstall the other's files; a restore
//! clears the store id ([`forget_store_id`]), and the copy mints its own when it first needs one.
//!
//! Every change holds the runtime's ledger lock. Every DB step goes through [`LedgerStore`], which
//! takes the DB lock for that step only (never across file I/O) and refuses it when the open store
//! is no longer the store the change began in (another vault, or another store of this one). All of
//! this is synchronous: the Mods tab (slice 6) calls it from `spawn_blocking`.

use std::collections::BTreeSet;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::MutexGuard;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};

use super::api::grants::GrantError;
use super::disk::{self, Layout, PathRefusal, Step, SweepReport, VersionName};
use super::headers::APP_DIR;
use super::ids::{self, ModId};
use super::keys::TrustedKeys;
use super::package::{self, ApiRange, PackageError, VerifiedPackage};
use super::policy::{
    self, ApiFit, InstallKind, Pinned, Refusal, MOD_API_OLDEST_MAJOR, MOD_API_VERSION,
};
use super::runtime::{InstalledMod, ModRuntime, ServedFile};
use super::safety::SafetyCheck;
use super::secrets;
use super::store;
use crate::AppState;

// Applying the signed list's blacklist to the installed rows, and the background updater's half
// (slice 5). What slice 6's strips read is re-exported for it.
mod blacklist;
mod updates;
// The user's own pause, resume and first-open consent (slice 6).
mod user;
// The unsigned file install, and developer mode's switch for this device (slice 6, part 3).
mod dev_switch;
mod unsigned;
// A developer's unpacked test folder: loaded for the session, never persisted (slice 6, part 4).
mod folder;
// The pilots' auto-install: a fresh unconsented install whose marker and handover share its
// transaction (PR B, B8).
mod pilots;
#[allow(unused_imports)]
pub use blacklist::{
    clear_waiting, dismiss_resumed, read_resumed, read_waiting, set_waiting, ApplyReport, Resumed,
    Waiting, WaitingKind,
};
pub use dev_switch::{set_dev_mode_for_this_device, DevSwitch};
pub use folder::{
    list_test_mods, reload_test_mod, unload_test_mod, FolderLoad, Reloaded, TestModStatus,
    ALLOW_ATLAS_FOLDERS,
};
pub use pilots::PilotLanded;
pub use unsigned::{UnsignedInstall, UnsignedOptions};
#[allow(unused_imports)]
pub use updates::{Landed, Pass, RepairTarget};
pub use user::{ChangeRefusal, Standing};

/// The `settings` key of the calendar's view prefs, whose `hidden` set names a mod's overlay by
/// [`overlay_source`] (SYNC-SET.md). An uninstall takes the mod out of it, and so does a backup's
/// strip for a folder mod (`backup::strip`).
pub(crate) const CALENDAR_UI_KEY: &str = "calendar_ui";
/// How a mod's calendar overlay is named among calendar sources: `mod:<id>`, the sidebar
/// placement's form, except where [`LEGACY_OVERLAY_SOURCES`] keeps an older name.
pub const OVERLAY_SOURCE_PREFIX: &str = "mod:";
/// Overlays that keep the source id they had before they were mods, so the hidden-calendar sets
/// users already saved still apply (spec §4: the Pinboard's timeline stays `pm:pinboard`,
/// `src/lib/calendar-layout.ts`'s `PINBOARD_CALENDAR_ID`).
const LEGACY_OVERLAY_SOURCES: [(&str, &str); 1] = [("atlas.pinboard", "pm:pinboard")];

/// The calendar source id of a mod's overlay. PR-2's overlay must draw under exactly this name.
pub fn overlay_source(id: &ModId) -> String {
    LEGACY_OVERLAY_SOURCES
        .iter()
        .find(|(m, _)| *m == id.as_str())
        .map_or_else(
            || format!("{OVERLAY_SOURCE_PREFIX}{id}"),
            |(_, s)| (*s).to_owned(),
        )
}

/// The `settings` key holding this store's id, the name of its own folder under the vault's
/// (`data_dir/mods/<vault_id>/<store_id>`). Written once, by the first install or by a startup that
/// finds rows with none; deleted only by a restore, whose copy is a store of its own.
pub const STORE_ID_KEY: &str = "mods_store_id";

/// Why a ledger change didn't happen.
#[derive(Debug)]
pub enum LedgerError {
    /// The verifier refused the file.
    Package(PackageError),
    Refused(Refusal),
    Disk(io::Error),
    Db(rusqlite::Error),
    /// No store is open (locked, or another profile is the writer), or its id can't be read.
    Store(String),
    /// The open store is another one now: another vault's, or another store of this vault.
    VaultChanged,
    NotInstalled,
    /// The user's own change (a pause, a resume, a first-open "Turn on") isn't allowed now.
    Change(ChangeRefusal),
    /// The grants a consent wrote were refused (`mods::api::grants`); nothing was written.
    Grants(GrantError),
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Package(e) => write!(f, "{e}"),
            Self::Refused(r) => write!(f, "This mod wasn't installed: {r}."),
            Self::Disk(e) => write!(
                f,
                "PM couldn't write the mod's files ({e}). Nothing changed."
            ),
            Self::Db(e) => write!(f, "PM couldn't record the mod ({e}). Nothing changed."),
            Self::Store(why) => f.write_str(why),
            Self::VaultChanged => {
                f.write_str("The vault changed while the mod was installing. Nothing changed.")
            }
            Self::NotInstalled => f.write_str("That mod isn't installed."),
            Self::Change(why) => write!(f, "{why}"),
            Self::Grants(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for LedgerError {}

impl From<rusqlite::Error> for LedgerError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e)
    }
}

impl From<crate::mods::devmode::Error> for LedgerError {
    fn from(e: crate::mods::devmode::Error) -> Self {
        match e {
            crate::mods::devmode::Error::Db(e) => Self::Db(e),
            crate::mods::devmode::Error::Disk(e) => Self::Disk(e),
        }
    }
}

impl From<Refusal> for LedgerError {
    fn from(r: Refusal) -> Self {
        Self::Refused(r)
    }
}

impl From<PathRefusal> for LedgerError {
    fn from(r: PathRefusal) -> Self {
        Self::Refused(Refusal::Path(r))
    }
}

impl From<LedgerError> for crate::error::Error {
    fn from(e: LedgerError) -> Self {
        Self::Other(e.to_string())
    }
}

/// The store the ledger writes to, one step at a time.
pub trait LedgerStore {
    /// Run `f` on the open store, provided it is still the one `layout` names: its vault id and its
    /// store id ([`same_store`]). The DB lock is held for `f` only, so `f` must do no file I/O and
    /// must never reach for the store again.
    fn with_conn<T>(
        &self,
        layout: &Layout,
        f: impl FnOnce(&mut Connection) -> Result<T, LedgerError>,
    ) -> Result<T, LedgerError>;
}

impl LedgerStore for AppState {
    fn with_conn<T>(
        &self,
        layout: &Layout,
        f: impl FnOnce(&mut Connection) -> Result<T, LedgerError>,
    ) -> Result<T, LedgerError> {
        let mut conn = self.conn().map_err(|e| LedgerError::Store(e.to_string()))?;
        if self.active_vault_id().as_deref() != Some(layout.vault_id()) {
            return Err(LedgerError::VaultChanged);
        }
        same_store(&conn, layout)?;
        f(&mut conn)
    }
}

/// This store's id, if it has one. One that isn't the shape PM mints (a hand edit) is an error,
/// never a reason to mint over it.
pub fn read_store_id(conn: &Connection) -> Result<Option<String>, LedgerError> {
    let id: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [STORE_ID_KEY],
            |r| r.get(0),
        )
        .optional()?;
    match id {
        Some(id) => {
            disk::check_store_id(&id)?;
            Ok(Some(id))
        }
        None => Ok(None),
    }
}

/// Whether the open store is the one `layout` was made for: the same store id. Two stores of one
/// vault (a restored copy and its original) share the vault id, so this is what tells them apart.
fn same_store(conn: &Connection, layout: &Layout) -> Result<(), LedgerError> {
    match read_store_id(conn)? {
        Some(id) if id == layout.store_id() => Ok(()),
        _ => Err(LedgerError::VaultChanged),
    }
}

/// When a store may be given its id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mint {
    /// An install: the store needs its folder now.
    Always,
    /// A startup: only a store with rows needs one (a restored copy's rows park against it and are
    /// repaired into it); a store that never installed a mod has no folder to load or sweep.
    IfRows,
}

/// This store's id, minting one when `mint` allows and it has none. `None`: no id, and none needed.
pub fn store_id_for(conn: &Connection, mint: Mint) -> Result<Option<String>, LedgerError> {
    if let Some(id) = read_store_id(conn)? {
        return Ok(Some(id));
    }
    let needed = match mint {
        Mint::Always => true,
        Mint::IfRows => conn.query_row("SELECT EXISTS(SELECT 1 FROM mods)", [], |r| r.get(0))?,
    };
    if !needed {
        return Ok(None);
    }
    let fresh = disk::new_store_id().map_err(LedgerError::Disk)?;
    // `OR IGNORE`, then read back: whichever id landed first is the store's.
    conn.execute(
        "INSERT OR IGNORE INTO settings(key, value) VALUES (?1, ?2)",
        params![STORE_ID_KEY, fresh],
    )?;
    read_store_id(conn)
}

/// Make a restored copy a store of its own: it keeps the vault's id, so without this it would share
/// the folder of the store it was made from. Its id is minted again when it first needs one. Its
/// mods come back as exact copies from the store it was made from when that store is on this
/// machine ([`Ledger::recover_missing`]), and otherwise park until repaired there (spec §1.11:
/// missing files after a restore park).
pub fn forget_store_id(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM settings WHERE key = ?1", [STORE_ID_KEY])
        .map(|_| ())
}

/// Where a signed package came from: the Atlas release the index lists, or a `.pmmod` file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Catalogue,
    File,
}

impl Source {
    fn stored(self) -> &'static str {
        match self {
            Self::Catalogue => store::FROM_CATALOGUE,
            Self::File => store::FROM_FILE,
        }
    }
}

/// Whether the user is there to see what an update asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    /// They clicked Install or Update on a screen that showed this version's permissions.
    Approved,
    /// A background update: one that asks for more ([`policy::widening`]) waits for them.
    Unattended,
}

/// Slice 5's checks: the signed index entry agrees with the package and its sha256, and nothing on
/// the blacklist matches it. The install path asks it before writing anything.
pub trait CatalogueCheck {
    fn check(
        &self,
        pkg: &VerifiedPackage,
        package_sha256: &str,
        source: Source,
    ) -> Result<(), String>;
}

impl<F: Fn(&VerifiedPackage, &str, Source) -> Result<(), String>> CatalogueCheck for F {
    fn check(
        &self,
        pkg: &VerifiedPackage,
        package_sha256: &str,
        source: Source,
    ) -> Result<(), String> {
        self(pkg, package_sha256, source)
    }
}

/// Slice 5's check before a row is served at startup (spec §1.10: "the cached blacklist is
/// re-applied to restored `mods` rows before any frame mounts"): `Some(reason)` keeps the row out
/// of the registry this time. Writing the pause into the row is the blacklist's own (its owner,
/// with its words); the ledger only declines to serve it.
///
/// **Slice 6 seam (a user or dev-mode pause under a standing blacklist entry).** The blacklist
/// leaves such a row alone ([`super::safety::plan_row`]), and this check runs only at startup. So
/// the resume path (the user lifting their own pause, mid-session) must itself ask a
/// `SafetyCheck` built from `refresh::cached_index`, and refuse, or re-pause with
/// `safety::blacklist_words`, when it holds the row. Otherwise the mod serves until the next pass
/// (up to six hours) re-pauses it. No resume exists on this branch to carry that call yet.
pub trait ServeCheck {
    fn hold(&self, id: &str, version: &str, package_sha256: Option<&str>) -> Option<String>;
}

impl<F: Fn(&str, &str, Option<&str>) -> Option<String>> ServeCheck for F {
    fn hold(&self, id: &str, version: &str, package_sha256: Option<&str>) -> Option<String> {
        self(id, version, package_sha256)
    }
}

pub struct InstallOptions<'a> {
    pub source: Source,
    /// False only for a pilot auto-installed for an existing user (D17): it is recorded, but its
    /// frame waits for the first-open screen.
    pub consented: bool,
    pub approval: Approval,
    pub catalogue: &'a dyn CatalogueCheck,
}

/// What an install did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub kind: InstallKind,
    pub version: String,
    pub state: String,
    /// Old version folders an update removed.
    pub pruned: Vec<String>,
}

/// What an install would be, for the consent screen ([`Ledger::preview`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub kind: InstallKind,
    /// The version installed now, for an update or a repair.
    pub installed_version: Option<String>,
    /// What an update asks for that the installed version didn't have.
    pub widening: Vec<policy::Widening>,
}

/// What the checks decided, for the write that follows them.
struct Checked {
    kind: InstallKind,
    current: Option<Current>,
    /// The permissions the installed row has granted.
    granted: BTreeSet<String>,
    version: VersionName,
    manifest_text: String,
}

/// What an uninstall did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Uninstalled {
    /// False when the folder couldn't be removed; the next boot sweep quarantines it.
    pub files_removed: bool,
}

/// What a startup load did, for the log.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StartupReport {
    pub loaded: Vec<String>,
    pub parked: Vec<(String, &'static str)>,
    pub unparked: Vec<String>,
    /// Version folders put back from quarantine or copied from another store of the vault
    /// (`<id>/<version>` from where [`disk::recover`] found them).
    pub recovered: Vec<String>,
    /// Rows whose missing files another store may hold, but couldn't be looked for or copied
    /// (`<id>/<version>: <error>`), so that store's folder is no proof they're gone.
    pub unrecovered: Vec<String>,
    /// Rows the [`ServeCheck`] kept from being served, with its reason.
    pub held: Vec<(String, String)>,
    /// Unsigned rows this load paused because developer mode is off for this device, and rows it
    /// resumed because it is on ([`policy::dev_mode_state`]).
    pub dev_paused: Vec<String>,
    pub dev_resumed: Vec<String>,
    /// Why nothing was loaded or swept, when the listing couldn't be read.
    pub skipped: Option<String>,
    pub sweep: SweepReport,
}

/// A file as the row records it: `{path, sha256, size}`, the hash lowercase hex.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileRecord {
    path: String,
    sha256: String,
    size: u64,
}

/// What the row pins and holds, read before an install decides.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Current {
    pinned: Pinned,
    state: String,
    reason: Option<String>,
    consented: bool,
    manifest: String,
}

/// One row, as the startup load reads it.
#[derive(Debug, Clone)]
struct Row {
    id: String,
    version: String,
    package_sha256: Option<String>,
    /// Whether an Atlas key signed it; an unsigned row is served only while developer mode is on.
    signed: bool,
    /// `mods.installed_from`: a `folder` row is a test folder, served only through its session.
    installed_from: String,
    state: String,
    reason: Option<String>,
    consented: bool,
    manifest: String,
    files: String,
}

impl Row {
    /// Whether this is a developer's loaded test folder (`ledger/folder.rs`): its files are not in
    /// this store's mods folder, so nothing here may look for them, recover them or sweep for them.
    fn folder(&self) -> bool {
        self.installed_from == store::FROM_FOLDER
    }

    /// Whether the signed list removed this mod (`store::REASON_BLACKLIST_REMOVED`).
    fn removed(&self) -> bool {
        self.reason.as_deref() == Some(store::REASON_BLACKLIST_REMOVED)
    }
}

/// The ledger for one vault's folders, its store and the registry.
pub struct Ledger<'a, S: LedgerStore> {
    layout: &'a Layout,
    store: &'a S,
    runtime: &'a ModRuntime,
}

fn time_text(now: DateTime<Utc>) -> String {
    now.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

impl<'a, S: LedgerStore> Ledger<'a, S> {
    pub fn new(layout: &'a Layout, store: &'a S, runtime: &'a ModRuntime) -> Self {
        Self {
            layout,
            store,
            runtime,
        }
    }

    /// One step on the open store, bound to this ledger's store ([`LedgerStore::with_conn`]). The
    /// Mods tab's commands read through it too: `f` holds the DB lock, so it does no file I/O and
    /// never reaches for the store again.
    pub(crate) fn with_conn<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, LedgerError>,
    ) -> Result<T, LedgerError> {
        self.store.with_conn(self.layout, f)
    }

    /// The device was named in the vault this ledger is for: its developer-mode answer is that
    /// vault's, so a device named before a vault switch never answers for the store after it.
    pub(crate) fn bound_to(&self, device: &super::devmode::Here) -> Result<(), LedgerError> {
        if device.vault() == self.layout.vault_id() {
            Ok(())
        } else {
            Err(LedgerError::VaultChanged)
        }
    }

    /// The folders this ledger writes to.
    pub(crate) fn layout(&self) -> &Layout {
        self.layout
    }

    /// Install, update or repair from a signed `.pmmod`'s bytes, verified against `keys`: the
    /// embedded Atlas keys less those this machine has seen revoked (`keys::KeyRing::trusted`, with
    /// the revocations `index_state` records), so a revoked key is refused here as it is for the
    /// index.
    pub fn install_package(
        &self,
        bytes: &[u8],
        keys: &TrustedKeys,
        opts: &InstallOptions<'_>,
        now: DateTime<Utc>,
    ) -> Result<Installed, LedgerError> {
        self.install_package_with(bytes, keys, opts, now, &|_, _| Ok(()))
    }

    /// [`Self::install_package`] with `in_tx` run inside the transaction that writes the row, after
    /// the row and before the commit (the pilots' auto-install: its marker and handover land with
    /// the row or not at all). An error from it rolls the row back, and the files go the way a
    /// failed commit's do.
    pub(crate) fn install_package_with(
        &self,
        bytes: &[u8],
        keys: &TrustedKeys,
        opts: &InstallOptions<'_>,
        now: DateTime<Utc>,
        in_tx: InTx<'_>,
    ) -> Result<Installed, LedgerError> {
        let pkg = package::verify(bytes, keys.public_keys()).map_err(LedgerError::Package)?;
        let sha = hex::encode(Sha256::digest(bytes));
        self.install_verified_with(&pkg, &sha, opts, now, in_tx)
    }

    /// Every check an install makes, before anything is written, as the Mods tab's consent screen
    /// needs them: what this package would be (fresh, an update, a repair) and what an update would
    /// widen, or the refusal the install itself would give. Writes nothing. Not a promise: the
    /// install runs every check again, and its commit re-reads the row.
    pub fn preview(
        &self,
        pkg: &VerifiedPackage,
        package_sha256: &str,
        opts: &InstallOptions<'_>,
    ) -> Result<Preview, LedgerError> {
        let _ledger = self.runtime.ledger_lock();
        let checked = self.checks(pkg, package_sha256, opts)?;
        let widening = match &checked.current {
            Some(c) if checked.kind == InstallKind::Update => {
                policy::widening(&c.manifest, &pkg.manifest, &checked.granted)
            }
            _ => Vec::new(),
        };
        Ok(Preview {
            kind: checked.kind,
            installed_version: checked.current.map(|c| c.pinned.version),
            widening,
        })
    }

    /// The rules every install shares, signed or not: the id belongs to the tier it claims, the
    /// version names a folder, this PM serves the API range, and (on Windows) the paths fit.
    /// Returns the version's folder name.
    fn static_checks(&self, pkg: &VerifiedPackage) -> Result<VersionName, LedgerError> {
        let m = &pkg.manifest;
        ids::check_tier(&m.id, m.publisher.tier).map_err(|e| Refusal::Tier(e.to_string()))?;
        let version = VersionName::new(&m.version)?;
        match policy::api_fit(m.api, MOD_API_VERSION, MOD_API_OLDEST_MAJOR) {
            ApiFit::NeedsNewerPm => return Err(Refusal::NeedsNewerPm { min: m.api.min }.into()),
            ApiFit::Retired => return Err(Refusal::ApiRetired.into()),
            ApiFit::Fits | ApiFit::OlderMajor => {}
        }
        if cfg!(windows) {
            disk::check_windows_paths(
                disk::path_units(self.layout.store_dir()),
                &m.id,
                &version,
                pkg.files.keys().map(String::as_str),
            )?;
        }
        Ok(version)
    }

    /// The checks, under the ledger lock the caller holds.
    fn checks(
        &self,
        pkg: &VerifiedPackage,
        package_sha256: &str,
        opts: &InstallOptions<'_>,
    ) -> Result<Checked, LedgerError> {
        let m = &pkg.manifest;
        // This is the signed install. An unsigned package (`package::verify_unsigned`) has its own
        // path (`ledger::unsigned`) with its own rules, and must never get in through this one.
        if !pkg.is_signed() {
            return Err(LedgerError::Package(PackageError {
                refusal: package::Refusal::Unsigned,
                detail: "it isn't signed".into(),
            }));
        }
        let version = self.static_checks(pkg)?;
        opts.catalogue
            .check(pkg, package_sha256, opts.source)
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
        let (current, granted) =
            self.with_conn(|c| Ok((read_current(c, &m.id)?, read_granted(c, &m.id)?)))?;
        // The signed list removed the installed mod: its row waits for the user's answer about
        // what it made and stored (I-16), and nothing installs onto it until that uninstall.
        if current
            .as_ref()
            .is_some_and(|c| c.reason.as_deref() == Some(store::REASON_BLACKLIST_REMOVED))
        {
            return Err(Refusal::RemovedPending.into());
        }
        let kind = policy::decide(
            current.as_ref().map(|c| &c.pinned),
            &m.version,
            package_sha256,
            &m.publisher,
        )?;
        // Unattended work only updates or repairs what is installed: a mod the user uninstalled
        // while its update downloaded must not come back.
        if kind == InstallKind::Fresh && opts.approval == Approval::Unattended {
            return Err(Refusal::NothingToUpdate.into());
        }
        if let (InstallKind::Update, Approval::Unattended, Some(c)) =
            (kind, opts.approval, &current)
        {
            let wider = policy::widening(&c.manifest, m, &granted);
            if !wider.is_empty() {
                return Err(Refusal::NeedsApproval(wider).into());
            }
        }
        Ok(Checked {
            kind,
            current,
            granted,
            version,
            manifest_text,
        })
    }

    /// The install path from a package the verifier passed, with the `.pmmod`'s own sha256. The
    /// Mods tab's consent flow verifies once (it needs the manifest to read the user's answers
    /// against) and installs what it verified.
    pub(crate) fn install_verified(
        &self,
        pkg: &VerifiedPackage,
        package_sha256: &str,
        opts: &InstallOptions<'_>,
        now: DateTime<Utc>,
    ) -> Result<Installed, LedgerError> {
        self.install_verified_with(pkg, package_sha256, opts, now, &|_, _| Ok(()))
    }

    fn install_verified_with(
        &self,
        pkg: &VerifiedPackage,
        package_sha256: &str,
        opts: &InstallOptions<'_>,
        now: DateTime<Utc>,
        in_tx: InTx<'_>,
    ) -> Result<Installed, LedgerError> {
        let m = &pkg.manifest;
        let _ledger = self.runtime.ledger_lock();

        // ---- Every check, before anything is written ----
        let Checked {
            kind,
            current,
            version,
            manifest_text,
            ..
        } = self.checks(pkg, package_sha256, opts)?;
        let write = RowWrite {
            id: m.id.as_str(),
            name: &m.name,
            version: version.as_str(),
            tier: m.publisher.tier.as_str(),
            author: &m.publisher.author,
            source_repo: &m.publisher.source_repo,
            source_commit: &m.publisher.commit,
            signed: true,
            installed_from: opts.source.stored(),
            package_sha256: Some(package_sha256),
            manifest: &manifest_text,
            files: file_records_json(pkg)?,
            project_refs: project_refs_json(m),
            consented_at: opts.consented.then(|| time_text(now)),
            install_id: disk::new_install_id().map_err(LedgerError::Disk)?,
            now: time_text(now),
        };

        // ---- Files, then the row, then the registry ----
        let staged = disk::stage(self.layout, &pkg.files).map_err(LedgerError::Disk)?;
        let repair = kind == InstallKind::Repair;
        let placed = disk::place(self.layout, staged, &m.id, &version, repair, now)
            .map_err(LedgerError::Disk)?;
        let installed = InstalledMod::from_package(pkg, placed.dir()).map_err(LedgerError::Disk)?;
        let expected = current.as_ref().map(|c| &c.pinned);
        // The state and consent come from the row as the commit reads it, not as the checks did:
        // a pause written while the files were staging survives the install.
        let Committed { state, consented } =
            self.with_conn(|c| commit_with(c, &write, kind, expected, in_tx))?;
        placed.keep();
        if policy::serves(&state, consented) {
            self.runtime.install(installed);
        } else {
            // The row stays, but doesn't serve (another owner's pause, a mod waiting for consent):
            // a frame still open is told it was paused, not that the mod was uninstalled.
            self.runtime.pause(&m.id);
        }
        let pruned = match kind {
            InstallKind::Update => disk::prune_versions(self.layout, &m.id, &version),
            InstallKind::Fresh | InstallKind::Repair => Vec::new(),
        };
        Ok(Installed {
            kind,
            version: version.as_str().to_owned(),
            state,
            pruned,
        })
    }

    /// Uninstall: the row (and everything that cascades from it) and the calendar's hidden entry in
    /// one transaction, then the registry, then the folder. Gifts and spend history stay (I-29).
    /// The sidebar's placement and fold prefs are the page's `localStorage`; slice 6's Mods tab
    /// prunes `mod:<id>` from them when this returns.
    pub fn uninstall(&self, id: &ModId) -> Result<Uninstalled, LedgerError> {
        let _ledger = self.runtime.ledger_lock();
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            if tx.execute("DELETE FROM mods WHERE id = ?1", [id.as_str()])? == 0 {
                return Err(LedgerError::NotInstalled);
            }
            prune_calendar_hidden(&tx, id)?;
            disk::failpoint(Step::Commit).map_err(LedgerError::Disk)?;
            tx.commit()?;
            Ok(())
        })?;
        self.runtime.uninstall(id);
        Ok(Uninstalled {
            files_removed: disk::remove_mod(self.layout, id).is_ok(),
        })
    }

    /// Load every row into the registry, parking (never deleting) a row whose record or files don't
    /// check out, then make the sweep's moves from the same listing. Run only by the writer, right
    /// after its store opens ([`after_writer_open`]); [`Ledger::recover_missing`] and
    /// [`Ledger::expire`] run after it on a blocking thread. The files are checked by presence, type
    /// and size here; their bytes are hashed on every serve (`runtime::read_verified`), so a file
    /// changed in place is refused there rather than read twice at every launch. A row whose files
    /// are missing first looks for an exact copy in this store's quarantine
    /// ([`disk::take_from_quarantine`]), so a sweep made from an older copy of this ledger costs
    /// nothing once the newer one is back. One still missing is parked, and
    /// [`Ledger::recover_missing`] then looks in the vault's other stores. `check` can hold a row
    /// back from being served (slice 5's blacklist).
    pub fn startup(&self, now: DateTime<Utc>, check: &dyn ServeCheck) -> StartupReport {
        let _ledger = self.runtime.ledger_lock();
        // A test folder lives for the session that loaded it: whatever a crash or a closed store
        // left of one is gone before anything is read, so no folder row is ever loaded or swept for
        // (`ledger/folder.rs`). A failure is logged and harmless: a folder row with no session is
        // never served ([`Ledger::load`]), and the next open tries again.
        self.runtime.folders().forget_all();
        match self.with_conn(folder::purge_rows) {
            Ok(gone) if !gone.is_empty() => {
                eprintln!(
                    "mods: unloaded {} test mod(s) from a closed session",
                    gone.len()
                );
            }
            Ok(_) => {}
            Err(e) => eprintln!("mods: leftover test mods couldn't be cleared ({e})"),
        }
        let rows = match self.with_conn(|c| Ok(read_rows(c)?)) {
            Ok(rows) => rows,
            Err(e) => {
                // Without the listing nothing can be served or proved stale.
                self.runtime.clear();
                return StartupReport {
                    skipped: Some(e.to_string()),
                    ..StartupReport::default()
                };
            }
        };
        let mut report = self.load(&rows, now, check);
        // A row the safety list removed has no files to keep: a folder that reappears for it is
        // quarantined like any other with no row.
        let known = rows
            .iter()
            .filter(|row| !row.removed() && !row.folder())
            .map(|row| (row.id.clone(), VersionName::parse(&row.version).ok()))
            .collect();
        report.sweep = disk::sweep(self.layout, &known, now);
        report
    }

    /// The copying half of a startup load. A row whose files are still missing gets an exact copy
    /// from another store of the vault on this machine ([`disk::recover`]), such as the store a
    /// restored copy was made from, so a same-machine restore's mods come back as they were; when
    /// any came back, the rows are loaded again. Run after [`Ledger::startup`] on a blocking thread
    /// ([`after_writer_open`]), never inline in an open path: a copy reads, writes and fsyncs whole
    /// packages; and before other stores' folders go ([`Ledger::remove_other_stores`], on the
    /// restore's blocking thread). Under the ledger lock, and only while the store it was begun for
    /// is still the open one, checked before each copy and again before the reload. Otherwise it
    /// stops there and leaves the registry alone, since the store open now has its own load, before
    /// or after this one; what it already copied stays, as files that store's own rows name.
    pub fn recover_missing(
        &self,
        now: DateTime<Utc>,
        check: &dyn ServeCheck,
    ) -> Result<StartupReport, LedgerError> {
        let _ledger = self.runtime.ledger_lock();
        self.recover_held(now, check)
    }

    /// [`Ledger::recover_missing`]'s pass, for a caller that holds the ledger lock.
    fn recover_held(
        &self,
        now: DateTime<Utc>,
        check: &dyn ServeCheck,
    ) -> Result<StartupReport, LedgerError> {
        let rows = self.with_conn(|c| Ok(read_rows(c)?))?;
        let mut recovered = Vec::new();
        let mut unrecovered = Vec::new();
        for row in &rows {
            // Never copy a removed mod's files back from another store of the vault, and a test
            // folder has no files in any store: they are the developer's own.
            if row.removed() || row.folder() {
                continue;
            }
            let Ok(ok) = read_row(self.layout, row) else {
                continue;
            };
            if check_files(&ok) != Err(store::REASON_FILES_MISSING) {
                continue;
            }
            // Before every copy, not just the first: a store closed mid-pass gets no more than the
            // copy in progress, and whatever closed it (which takes this lock) waits no longer.
            self.with_conn(|_| Ok(()))?;
            match disk::recover(self.layout, &ok.id, &ok.version, &ok.files, now) {
                Ok(Some(from)) => {
                    recovered.push(format!("{}/{} from {from}", ok.id, ok.version.as_str()));
                }
                Ok(None) => {}
                Err(e) => unrecovered.push(format!("{}/{}: {e}", ok.id, ok.version.as_str())),
            }
        }
        if recovered.is_empty() {
            return Ok(StartupReport {
                unrecovered,
                ..StartupReport::default()
            });
        }
        // The copies took a while: load only into the store they were made for.
        self.with_conn(|_| Ok(()))?;
        let mut report = self.load(&rows, now, check);
        recovered.append(&mut report.recovered);
        report.recovered = recovered;
        report.unrecovered = unrecovered;
        Ok(report)
    }

    /// Remove the mod folders of this vault's other stores on this machine, which PM has just
    /// deleted with their ledgers ([`remove_vacated_store`]), but only once this store has its
    /// copies of what they hold. A restore of a store whose own ledger is gone ("Start a new empty
    /// store" replaced it) finds its rows' only copies in that store's folder, and the
    /// [`Ledger::recover_missing`] its open spawned may not have run yet; so that pass runs here
    /// first, in the caller's hold of the ledger lock. Nothing goes when it can't finish, when a
    /// row's copy couldn't be looked for or made (the report's `unrecovered`), or when a row was
    /// never looked for: one this PM can't read back (an API range it is too old for, a record it
    /// can't parse) or whose files are there but differ. One of those folders may hold the copy a
    /// later PM, or a repair, needs. Only a row whose files are missing everywhere lets them go.
    fn remove_other_stores(
        &self,
        data_dir: &Path,
        now: DateTime<Utc>,
        check: &dyn ServeCheck,
    ) -> Result<StartupReport, LedgerError> {
        let report = self.recover_held(now, check)?;
        let rows = self.with_conn(|c| Ok(read_rows(c)?))?;
        let all_looked_for = rows.iter().filter(|row| !row.folder()).all(|row| {
            read_row(self.layout, row)
                .is_ok_and(|ok| check_files(&ok) != Err(store::REASON_FILES_CHANGED))
        });
        if report.unrecovered.is_empty() && all_looked_for {
            let keep = Some(self.layout.store_id());
            if let Err(e) = disk::remove_stores(data_dir, self.layout.vault_id(), keep) {
                eprintln!("mods: a removed store's mods couldn't all be removed ({e})");
            }
        }
        Ok(report)
    }

    /// Check `rows`, record the parks and unparks they need, and make the registry exactly the rows
    /// that serve. The caller holds the ledger lock and read `rows` under it.
    fn load(&self, rows: &[Row], now: DateTime<Utc>, check: &dyn ServeCheck) -> StartupReport {
        let mut report = StartupReport::default();
        let mut changes = Vec::new();
        let mut serve = Vec::new();
        for row in rows {
            // Removed by the safety list: no files to look for or put back, nothing to serve. The
            // row waits, paused, for the uninstall dialog (I-16).
            if row.removed() {
                continue;
            }
            // A test folder is served from its session or not at all: its files are the
            // developer's own, so the load neither checks them against this store's folder, parks
            // the row for them, nor writes anything about it (a row with no session is a crash's
            // leftover, cleared at the next writer open).
            if row.folder() {
                if let Some(m) = self.folder_served(row, check) {
                    report.loaded.push(row.id.clone());
                    serve.push(m);
                }
                continue;
            }
            let checked = read_row(self.layout, row).and_then(|ok| {
                match check_files(&ok) {
                    Err(why) if why == store::REASON_FILES_MISSING => {
                        match disk::take_from_quarantine(
                            self.layout,
                            &ok.id,
                            &ok.version,
                            &ok.files,
                            now,
                        ) {
                            Ok(Some(from)) => {
                                report.recovered.push(format!(
                                    "{}/{} from {from}",
                                    ok.id,
                                    ok.version.as_str()
                                ));
                                check_files(&ok)
                            }
                            Ok(None) => Err(why),
                            Err(e) => {
                                eprintln!("mods: {}'s files not recovered: {e}", ok.id);
                                Err(why)
                            }
                        }
                    }
                    other => other,
                }
                .map(|()| ok)
            });
            let problem = checked.as_ref().err().copied();
            let change = policy::startup_state(&row.state, row.reason.as_deref(), problem);
            if let Some((_, why)) = change {
                match why {
                    Some(why) => report.parked.push((row.id.clone(), why)),
                    None => report.unparked.push(row.id.clone()),
                }
            }
            // Developer mode, on the row as the park above leaves it: an unsigned mod is served only
            // while it is on for this device in this vault. This is in the load, not only in the
            // switch, so every path that reloads the registry (a startup, a resume, a recovery, a
            // restore's open) pauses an unsigned row that developer mode being off would let serve,
            // whatever else changed it (a park lifted, a user's resume).
            let (after_park, reason_after) =
                change.map_or((row.state.as_str(), row.reason.as_deref()), |(s, r)| (s, r));
            let dev = match policy::dev_mode_state(
                row.signed,
                self.runtime.unsigned_allowed(),
                after_park,
                reason_after,
            ) {
                // Not turned back on while its files don't check out: the park comes first, and the
                // row says what stopped it meanwhile.
                Some((store::STATE_ACTIVE, None)) if checked.is_err() => None,
                // Nor past a safety-list entry that names it: that list owns the stop now.
                Some((store::STATE_ACTIVE, None))
                    if check
                        .hold(&row.id, &row.version, row.package_sha256.as_deref())
                        .is_some() =>
                {
                    Some((store::STATE_PAUSED, Some(store::REASON_BLACKLIST)))
                }
                other => other,
            };
            match dev {
                Some((store::STATE_PAUSED, Some(store::REASON_DEV_MODE))) => {
                    report.dev_paused.push(row.id.clone());
                }
                Some((store::STATE_ACTIVE, None)) => report.dev_resumed.push(row.id.clone()),
                _ => {}
            }
            let change = dev.or(change);
            let state = change.map_or(row.state.as_str(), |(s, _)| s);
            if let Some((to, why)) = change {
                changes.push((row, to, why));
            }
            let Ok(ok) = checked else { continue };
            if !policy::serves(state, row.consented) {
                continue;
            }
            if let Some(why) = check.hold(&row.id, &row.version, row.package_sha256.as_deref()) {
                report.held.push((row.id.clone(), why));
                continue;
            }
            match InstalledMod::from_ledger(ok.id, &ok.root, &ok.entry, ok.files) {
                Ok(m) => {
                    report.loaded.push(row.id.clone());
                    serve.push(m);
                }
                Err(e) => eprintln!("mods: {} not loaded: {e}", row.id),
            }
        }
        if !changes.is_empty() {
            let at = time_text(now);
            let written = self.with_conn(|conn| {
                let tx = conn.transaction()?;
                for (row, to, why) in &changes {
                    // Compare-and-set against what was read, so another owner's change since wins.
                    tx.execute(
                        "UPDATE mods SET state = ?2, state_reason = ?3, updated_at = ?4 \
                         WHERE id = ?1 AND state = ?5 AND state_reason IS ?6",
                        params![row.id, to, why, at, row.state, row.reason],
                    )?;
                }
                tx.commit()?;
                Ok(())
            });
            if let Err(e) = written {
                // Not served either way; the next launch records it.
                eprintln!("mods: couldn't record a park ({e})");
            }
        }
        // A row that is still here but doesn't serve (parked, held, paused, unconsented) is paused as
        // far as a live frame is concerned, not uninstalled. A blacklist-removed row isn't named:
        // its mod is gone as far as the user is concerned, and the goodbye already said so.
        let parked = rows
            .iter()
            .filter(|row| !row.removed())
            .filter_map(|row| ModId::parse(&row.id).ok())
            .collect();
        self.runtime.replace_all(serve, &parked);
        report
    }

    /// The boot sweep's deletions: abandoned staging, and quarantine past its 30 days. Run after
    /// [`Ledger::startup`], on a blocking thread ([`after_writer_open`]), since either can be a large
    /// tree. Under the ledger lock, so no install is writing into staging, and only while the store
    /// it was begun for is still the open one; otherwise nothing (the next writer open does it).
    pub fn expire(&self, now: DateTime<Utc>) -> Result<SweepReport, LedgerError> {
        let _ledger = self.runtime.ledger_lock();
        self.with_conn(|_| Ok(()))?;
        Ok(disk::expire(self.layout, now))
    }
}

/// The `files` column: every verified file as `{path, sha256, size}`.
fn file_records_json(pkg: &VerifiedPackage) -> Result<String, LedgerError> {
    let files: Vec<FileRecord> = pkg
        .files
        .iter()
        .map(|(path, f)| FileRecord {
            path: path.clone(),
            sha256: hex::encode(f.sha256),
            size: f.bytes.len() as u64,
        })
        .collect();
    serde_json::to_string(&files).map_err(|e| LedgerError::Disk(io::Error::other(e)))
}

/// The `project_refs` column: the manifest's `data.projectRefs`.
fn project_refs_json(m: &package::Manifest) -> String {
    let refs: Vec<Value> = m
        .data
        .project_refs
        .iter()
        .map(|r| serde_json::json!({"key": r.key, "path": r.path}))
        .collect();
    Value::Array(refs).to_string()
}

/// The row's values for an install's commit.
struct RowWrite<'a> {
    id: &'a str,
    name: &'a str,
    version: &'a str,
    tier: &'a str,
    author: &'a str,
    source_repo: &'a str,
    source_commit: &'a str,
    /// Whether an Atlas key signed it (an unsigned file's row says no: policy reads it).
    signed: bool,
    installed_from: &'a str,
    /// The `.pmmod`'s own hash; `None` only for a test folder (v60's CHECK), which has no package.
    package_sha256: Option<&'a str>,
    manifest: &'a str,
    files: String,
    project_refs: String,
    /// Set when the user consented with this install; a row's earlier consent is kept.
    consented_at: Option<String>,
    /// Written by a fresh install only; an update or a repair keeps the row's (v60's `mod_ledger`
    /// names an unsigned maker by it).
    install_id: String,
    now: String,
}

/// What the commit wrote, from the row as its own transaction read it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Committed {
    state: String,
    consented: bool,
}

fn read_current(conn: &Connection, id: &ModId) -> rusqlite::Result<Option<Current>> {
    conn.query_row(
        "SELECT version, package_sha256, tier, author, source_repo, state, state_reason, \
                consented_at IS NOT NULL, manifest \
         FROM mods WHERE id = ?1",
        [id.as_str()],
        |r| {
            Ok(Current {
                pinned: Pinned {
                    version: r.get(0)?,
                    package_sha256: r.get(1)?,
                    tier: r.get(2)?,
                    author: r.get(3)?,
                    source_repo: r.get(4)?,
                },
                state: r.get(5)?,
                reason: r.get(6)?,
                consented: r.get(7)?,
                manifest: r.get(8)?,
            })
        },
    )
    .optional()
}

fn read_granted(conn: &Connection, id: &ModId) -> rusqlite::Result<BTreeSet<String>> {
    let mut stmt =
        conn.prepare("SELECT permission FROM mod_grants WHERE mod_id = ?1 AND decision = ?2")?;
    let rows = stmt.query_map(params![id.as_str(), store::GRANTED], |r| r.get(0))?;
    rows.collect()
}

fn read_rows(conn: &Connection) -> rusqlite::Result<Vec<Row>> {
    let mut stmt = conn.prepare(
        "SELECT id, version, package_sha256, state, state_reason, consented_at IS NOT NULL, \
                manifest, files, signed, installed_from \
         FROM mods ORDER BY id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(Row {
            id: r.get(0)?,
            version: r.get(1)?,
            package_sha256: r.get(2)?,
            state: r.get(3)?,
            reason: r.get(4)?,
            consented: r.get(5)?,
            manifest: r.get(6)?,
            files: r.get(7)?,
            signed: r.get(8)?,
            installed_from: r.get(9)?,
        })
    })?;
    rows.collect()
}

/// What an install runs inside its own transaction after the row is written, told which kind of
/// install it is (see [`Ledger::install_package_with`]).
pub(crate) type InTx<'a> = &'a dyn Fn(&Connection, InstallKind) -> Result<(), LedgerError>;

/// Write the row, after checking the pin is still what the checks read: a fresh install needs no
/// row, an update or repair the very row it was decided against. The state is decided here, from
/// the row this transaction reads: another owner's pause written since the checks (the blacklist,
/// the user) is kept, never overwritten from the earlier read. `in_tx` runs between the row and the
/// commit: its failure drops the transaction, so the row goes with it.
fn commit_with(
    conn: &mut Connection,
    w: &RowWrite<'_>,
    kind: InstallKind,
    expected: Option<&Pinned>,
    in_tx: InTx<'_>,
) -> Result<Committed, LedgerError> {
    let tx = conn.transaction()?;
    let committed = write_row(&tx, w, kind, expected)?;
    in_tx(&tx, kind)?;
    disk::failpoint(Step::Commit).map_err(LedgerError::Disk)?;
    tx.commit()?;
    Ok(committed)
}

/// [`commit_with`]'s writes, in the caller's transaction (the unsigned install adds its own rule to the
/// same one).
fn write_row(
    tx: &Connection,
    w: &RowWrite<'_>,
    kind: InstallKind,
    expected: Option<&Pinned>,
) -> Result<Committed, LedgerError> {
    let id = ModId::parse(w.id).map_err(|_| Refusal::UnreadableRecord)?;
    let now = read_current(tx, &id)?;
    if now.as_ref().map(|c| &c.pinned) != expected {
        return Err(Refusal::Changed.into());
    }
    let (state, reason) = policy::state_after_install(
        now.as_ref()
            .map(|c| (c.state.as_str(), c.reason.as_deref())),
    );
    let consented = w.consented_at.is_some() || now.as_ref().is_some_and(|c| c.consented);
    let values = params![
        w.id,
        w.name,
        w.version,
        w.tier,
        w.author,
        w.source_repo,
        w.source_commit,
        w.installed_from,
        w.package_sha256,
        w.manifest,
        w.files,
        w.project_refs,
        state,
        reason,
        w.consented_at,
        w.now,
    ];
    match kind {
        InstallKind::Fresh => tx.execute(
            "INSERT INTO mods(id, name, version, tier, author, source_repo, source_commit, signed, \
                              installed_from, package_sha256, manifest, files, project_refs, state, \
                              state_reason, consented_at, installed_at, updated_at, install_id) \
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?18, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?16, \
                     ?17)",
            [
                values,
                &[
                    &w.install_id as &dyn rusqlite::ToSql,
                    &w.signed as &dyn rusqlite::ToSql,
                ],
            ]
            .concat()
            .as_slice(),
        )?,
        // Tier, author and source repo are the pin, unchanged by construction; the install id is
        // the row's, kept.
        InstallKind::Update | InstallKind::Repair => tx.execute(
            "UPDATE mods SET name = ?2, version = ?3, source_commit = ?7, signed = 1, \
                             installed_from = ?8, package_sha256 = ?9, manifest = ?10, files = ?11, \
                             project_refs = ?12, state = ?13, state_reason = ?14, \
                             consented_at = COALESCE(consented_at, ?15), updated_at = ?16 \
             WHERE id = ?1 AND tier = ?4 AND author = ?5 AND source_repo = ?6",
            values,
        )?,
    };
    // A key whose slot this version moved or dropped goes now, in this transaction: its value was
    // typed for another host or header, and must not wait in the store for a later slot to match
    // (`secrets` also refuses to read a row whose pair is not the slot's). The approval that let
    // the move land was about the new destination, never about the old key.
    if matches!(kind, InstallKind::Update | InstallKind::Repair) {
        secrets::prune_to_slots(tx, w.id, w.manifest)?;
    }
    // A waiting update this install satisfies is no longer waiting.
    blacklist::clear_waiting(tx, w.id, Some(w.version))?;
    Ok(Committed { state, consented })
}

/// Take the mod's overlay ([`overlay_source`]) out of the calendar's hidden set, leaving the blob
/// otherwise as it was. A blob that isn't the shape the calendar writes is left alone. The
/// uninstall's, and a backup's strip of a folder mod's (`backup::strip`).
pub(crate) fn prune_calendar_hidden(conn: &Connection, id: &ModId) -> rusqlite::Result<()> {
    let blob: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [CALENDAR_UI_KEY],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(pruned) = blob.as_deref().and_then(|b| without_hidden(b, id)) {
        conn.execute(
            "UPDATE settings SET value = ?2 WHERE key = ?1",
            params![CALENDAR_UI_KEY, pruned],
        )?;
    }
    Ok(())
}

/// The `calendar_ui` blob without the mod's overlay in its `hidden` list, or `None` when it has none.
fn without_hidden(blob: &str, id: &ModId) -> Option<String> {
    let mut value: Value = serde_json::from_str(blob).ok()?;
    let hidden = value.get_mut("hidden")?.as_array_mut()?;
    let name = overlay_source(id);
    let before = hidden.len();
    hidden.retain(|e| e.as_str() != Some(name.as_str()));
    (hidden.len() != before).then(|| value.to_string())
}

/// A row read back into a mod, ready to check against the disk and load.
struct Loadable {
    id: ModId,
    version: VersionName,
    root: PathBuf,
    entry: String,
    files: Vec<(String, ServedFile)>,
}

fn decode_sha(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    hex::decode(s).ok()?.try_into().ok()
}

/// Read a row back: its record (every name back through the same grammar that let it in, so a
/// hand-edited row can't name a path), then its API range. The ledger reason it fails with, if it
/// does; [`check_files`] then checks it against the disk.
fn read_row(layout: &Layout, row: &Row) -> Result<Loadable, &'static str> {
    let record = store::REASON_RECORD;
    let id = ModId::parse(&row.id).map_err(|_| record)?;
    let version = VersionName::parse(&row.version).map_err(|_| record)?;
    let records: Vec<FileRecord> = serde_json::from_str(&row.files).map_err(|_| record)?;
    let manifest: Value = serde_json::from_str(&row.manifest).map_err(|_| record)?;
    let entry = manifest["entry"]["page"].as_str().ok_or(record)?.to_owned();
    let api = manifest["apiVersion"]["min"]
        .as_str()
        .and_then(policy::parse_major_minor)
        .zip(
            manifest["apiVersion"]["target"]
                .as_str()
                .and_then(policy::parse_major_minor),
        )
        .map(|(min, target)| ApiRange { min, target })
        .ok_or(record)?;
    let mut files = Vec::with_capacity(records.len());
    for r in records {
        package::check_path(&r.path).map_err(|_| record)?;
        let sha256 = decode_sha(&r.sha256).ok_or(record)?;
        files.push((
            r.path,
            ServedFile {
                sha256,
                size: r.size,
            },
        ));
    }
    if !entry.starts_with(APP_DIR) || !files.iter().any(|(p, _)| *p == entry) {
        return Err(record);
    }
    if !policy::api_fit(api, MOD_API_VERSION, MOD_API_OLDEST_MAJOR).runs() {
        return Err(store::REASON_API);
    }
    Ok(Loadable {
        root: layout.version_dir(&id, &version),
        id,
        version,
        entry,
        files,
    })
}

/// Check every file a row lists against the disk, by presence, type and size.
fn check_files(ok: &Loadable) -> Result<(), &'static str> {
    for (path, want) in &ok.files {
        let mut full = ok.root.clone();
        full.extend(path.split('/'));
        match std::fs::symlink_metadata(&full) {
            Ok(meta) if meta.is_file() && meta.len() == want.size => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(store::REASON_FILES_MISSING)
            }
            _ => return Err(store::REASON_FILES_CHANGED),
        }
    }
    Ok(())
}

// ---- Wiring into the app -----------------------------------------------------------------------

/// The open store's mod layout, or `None` when it has no folder (no id, and `mint` didn't need
/// one) or can't be named (logged). The DB lock is held only to read or mint the store id.
fn active_layout(app: &AppHandle, state: &AppState, mint: Mint) -> Option<Layout> {
    let vault_id = state.active_vault_id()?;
    let data_dir = crate::paths::data_dir(app)
        .map_err(|e| eprintln!("mods: no data folder ({e})"))
        .ok()?;
    let store_id = {
        let conn = state
            .conn()
            .map_err(|e| eprintln!("mods: no store ({e})"))
            .ok()?;
        store_id_for(&conn, mint)
            .map_err(|e| eprintln!("mods: this store's mods folder can't be named ({e})"))
            .ok()??
    };
    Layout::new(&data_dir, &vault_id, &store_id)
        .map_err(|e| eprintln!("mods: not loaded: {e}"))
        .ok()
}

/// Run `f` on a ledger for the open store's mods folder: the safety list's refresh, the background
/// updater and the Mods tab's commands (slice 6) go through this. `None` when no store is open (the
/// vault is locked, or another profile is the writer), or it has no mods folder and `mint` didn't
/// ask for one. `f` runs synchronously on the caller's thread: call this from `spawn_blocking`, never
/// on the async runtime and never while holding the DB lock.
pub fn with_active_ledger<T>(
    app: &AppHandle,
    mint: Mint,
    f: impl FnOnce(&Ledger<'_, AppState>) -> T,
) -> Option<T> {
    let runtime = app.try_state::<ModRuntime>()?;
    let state = app.state::<AppState>();
    let layout = active_layout(app, state.inner(), mint)?;
    Some(f(&Ledger::new(&layout, state.inner(), runtime.inner())))
}

/// Load the open store's mods and run the boot sweep. Called by the lock session once this
/// instance holds the store as its writer (`lock_session::engage`, a take-over), never by a
/// profile that stepped back, whose store is closed. A store that has no folder, or can't be named,
/// loads nothing.
///
/// The cached safety list's blacklist is applied first ([`Ledger::apply_blacklist`]: a few row
/// writes and, for a removed mod, its folder; the cached file is one small read), so a row it
/// pauses or removes never reaches the registry, and no frame can mount before it. A restore's
/// store comes through here too, and the startup load's [`SafetyCheck`] holds back anything the
/// apply could not write. Developer mode for this device is looked up before anything loads
/// ([`dev_switch::on_here`], from the store's own record) and given to the registry, so the load
/// pauses an unsigned row while it is off and serves none (INVARIANTS.md I-34).
///
/// The load and the sweep's moves run inline, so two loads can't race across a vault switch: one
/// query, a stat per file, a rename per stray folder, and the hashing of a quarantined copy only
/// when a row's files are missing. Whatever reads or writes whole packages or trees runs after it on
/// a blocking thread, and only when the listing was read, because the callers include async
/// commands and the main thread: copying a missing row's files from another store of the vault
/// ([`Ledger::recover_missing`], which loads again if any came back), then the sweep's deletions
/// ([`Ledger::expire`]). Each checks under the ledger lock that its store is still the open one.
/// Every ledger change waits for that lock: a load of the same store waits for the whole copy (so
/// `switch_to_vault` runs its second engage off the async runtime), and a close of the store waits
/// for the package being copied, since the copy stops at its next check.
pub fn after_writer_open(app: &AppHandle) {
    let Some(runtime) = app.try_state::<ModRuntime>() else {
        return;
    };
    let state = app.state::<AppState>();
    // Developer mode for this device in this vault decides whether an unsigned row may serve: it is
    // looked up from the store's own record before anything loads, and a lookup that fails is off.
    let dev_on = dev_switch::on_here(app, state.inner());
    let Some(layout) = active_layout(app, state.inner(), Mint::IfRows) else {
        let _ledger = runtime.ledger_lock();
        runtime.clear();
        // A store with no mods folder yet can still have it on: the first unsigned install into it
        // writes the folder, and a later load must not pause that mod.
        runtime.set_unsigned_allowed(dev_on);
        return;
    };
    runtime.set_unsigned_allowed(dev_on);
    // The cached safety list is applied before the load, so a row it pauses or removes never
    // reaches the registry, and so before any frame can mount (spec §1.10). A restored store's rows
    // meet it here too: a restore commit opens its store through this same path.
    let cached = super::refresh::cached_index(app);
    let ledger = Ledger::new(&layout, state.inner(), runtime.inner());
    if let Some(index) = &cached {
        super::refresh::report_applied(app, &ledger.apply_blacklist(index, Utc::now()));
    }
    let check = SafetyCheck::serve(cached.as_ref());
    let report = ledger.startup(Utc::now(), &check);
    if let Some(why) = &report.skipped {
        eprintln!("mods: nothing loaded or swept, the ledger couldn't be read ({why})");
    }
    for (id, why) in &report.parked {
        eprintln!("mods: {id} parked ({why})");
    }
    for recovered in &report.recovered {
        eprintln!("mods: put back {recovered}");
    }
    if !report.sweep.is_empty() {
        eprintln!("mods: boot sweep {:?}", report.sweep);
    }
    // The registry was just made from the rows (or, when they couldn't be read, emptied): a Mods
    // tab that is already up reads them again.
    super::refresh::emit_changed(app, super::refresh::CHANGED_LOADED);
    // The startup forgot every test folder's session (and deleted their rows): so go their watches.
    super::folder_watch::resync(app);
    if report.skipped.is_some() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (Some(runtime), Some(state)) =
            (app.try_state::<ModRuntime>(), app.try_state::<AppState>())
        else {
            return;
        };
        let ledger = Ledger::new(&layout, state.inner(), runtime.inner());
        let check = SafetyCheck::serve(cached.as_ref());
        match ledger.recover_missing(Utc::now(), &check) {
            Ok(report) => {
                log_recovery(&report);
                // This pass lands after the open that started it has returned, so a page that
                // already read the rows saw these mods parked: tell it they are back.
                if !report.recovered.is_empty() {
                    super::refresh::emit_changed(&app, super::refresh::CHANGED_RECOVERED);
                }
            }
            Err(e) => eprintln!("mods: other stores of this vault not looked in ({e})"),
        }
        match ledger.expire(Utc::now()) {
            Ok(report) if !report.is_empty() => eprintln!("mods: boot sweep {report:?}"),
            Ok(_) => {}
            Err(e) => eprintln!("mods: boot sweep's clean-up skipped ({e})"),
        }
    });
}

/// Finish, now and in the caller's thread, the recovery [`after_writer_open`] only starts: copy in
/// what the vault's other stores hold for rows whose files are missing, and reload when any came
/// back. `switch_to_vault` runs it so its answer about the restored mods is the settled one, not
/// the state at the instant the open returned (spec §1.11). It waits for the ledger lock the
/// background pass holds, and is a no-op for what that pass already did. Returns whether any mod's
/// files came back. Blocking: call from `spawn_blocking`, never on the async runtime.
pub fn recover_now(app: &AppHandle) -> bool {
    let cached = super::refresh::cached_index(app);
    let check = SafetyCheck::serve(cached.as_ref());
    let recovered = with_active_ledger(app, Mint::IfRows, |ledger| {
        match ledger.recover_missing(Utc::now(), &check) {
            Ok(report) => {
                log_recovery(&report);
                !report.recovered.is_empty()
            }
            Err(e) => {
                eprintln!("mods: other stores of this vault not looked in ({e})");
                false
            }
        }
    })
    .unwrap_or(false);
    if recovered {
        super::refresh::emit_changed(app, super::refresh::CHANGED_RECOVERED);
    }
    recovered
}

/// Remove the mod files of a store PM has just deleted: the pristine home a restore re-homed over
/// (`commands::backups`), whose ledger went with it (spec §6: no residue), or every store of a
/// deleted vault ([`remove_deleted_vault`]). Every store folder of `vault_id` goes but the open
/// store's own, which shares the vault id when the open vault is a copy of that very vault; that
/// store first takes from them the copies its rows need ([`Ledger::remove_other_stores`]). Under
/// the ledger lock; nothing is removed when the open store's id can't be read or minted, since its
/// folder couldn't then be told apart.
pub fn remove_vacated_store(app: &AppHandle, data_dir: &Path, vault_id: &str) {
    let Some(runtime) = app.try_state::<ModRuntime>() else {
        return;
    };
    let _ledger = runtime.ledger_lock();
    let state = app.state::<AppState>();
    if state.active_vault_id().as_deref() != Some(vault_id) {
        if let Err(e) = disk::remove_stores(data_dir, vault_id, None) {
            eprintln!("mods: a removed store's mods couldn't all be removed ({e})");
        }
        return;
    }
    // Minted when it has rows, as the startup load mints it: such a store needs its folder to copy
    // into before the others go. The DB guard is dropped before the ledger takes it again.
    let open = state
        .conn()
        .map_err(|e| LedgerError::Store(e.to_string()))
        .and_then(|conn| store_id_for(&conn, Mint::IfRows))
        .and_then(|id| {
            id.map(|id| Layout::new(data_dir, vault_id, &id))
                .transpose()
                .map_err(LedgerError::from)
        });
    match open {
        Ok(Some(layout)) => {
            let ledger = Ledger::new(&layout, state.inner(), runtime.inner());
            let cached = super::refresh::cached_index(app);
            let check = SafetyCheck::serve(cached.as_ref());
            match ledger.remove_other_stores(data_dir, Utc::now(), &check) {
                Ok(report) => {
                    log_recovery(&report);
                    if !report.unrecovered.is_empty() {
                        eprintln!("mods: a removed store's mods were left, for those files");
                    }
                }
                Err(e) => eprintln!("mods: a removed store's mods were left ({e})"),
            }
        }
        // The open store has no rows and no folder: none of the folders is its.
        Ok(None) => {
            if let Err(e) = disk::remove_stores(data_dir, vault_id, None) {
                eprintln!("mods: a removed store's mods couldn't all be removed ({e})");
            }
        }
        Err(e) => eprintln!("mods: a removed store's mods were left ({e})"),
    }
}

/// Log what a recovery pass copied in, and what it couldn't.
fn log_recovery(report: &StartupReport) {
    for recovered in &report.recovered {
        eprintln!("mods: put back {recovered}");
    }
    for row in &report.unrecovered {
        eprintln!("mods: files not recovered for {row}");
    }
}

/// Remove the mods of a vault that is gone for this profile (spec §6: no residue): the shared vault
/// it has just deleted, or one whose deletion by its owner it has just acknowledged
/// (`commands::vaults`). Every store folder of `vault_id` goes, since the ledgers that named them
/// went with the vault. Called with the store closed, once the vault this profile goes back to is
/// known (`resident`, from its metadata): should that be a copy of the deleted vault (a restore keeps
/// the id), one of the folders is its store's, which can't be told apart before it opens, so nothing
/// goes.
pub fn remove_deleted_vault(app: &AppHandle, data_dir: &Path, vault_id: &str, resident: &str) {
    if resident == vault_id {
        eprintln!("mods: the deleted vault's mods were left: this profile's own vault is a copy");
        return;
    }
    remove_vacated_store(app, data_dir, vault_id);
}

/// The lock session has settled (`lock_session::engage`, or a take-over): load the writer's mods
/// when `writes`, or serve none. Whatever closed the store (a hand-off, or a detach whose next vault
/// is locked), the registry then matches it.
pub fn after_settle(app: &AppHandle, writes: impl FnOnce() -> bool) {
    let Some(runtime) = app.try_state::<ModRuntime>() else {
        return;
    };
    if settle_registry(runtime.inner(), writes) {
        after_writer_open(app);
        // A pilot owed to this store (a marker; nothing without one) with the index held on this
        // machine. After the load, not inside it: `after_writer_open` returns early for a store with
        // no mods yet, which is exactly the store that is owed one.
        super::autoinstall::spawn_after_open(app);
    }
    // Either way the sessions of test folders are gone (a close forgot them, a writer's open
    // deleted their rows): so are their watches.
    super::folder_watch::resync(app);
}

/// [`after_settle`]'s registry half: `writes` is asked, and a `false` acted on, under the ledger
/// lock, so a take-over's load on the watcher thread can't land between the answer and the clear.
/// Whether to load.
fn settle_registry(runtime: &ModRuntime, writes: impl FnOnce() -> bool) -> bool {
    let _ledger = runtime.ledger_lock();
    let writes = writes();
    if !writes {
        runtime.clear();
    }
    writes
}

/// The store closed under this instance (another profile became the writer, or a vault command
/// took the connection): serve nothing.
pub fn on_store_closed(app: &AppHandle) {
    if let Some(runtime) = app.try_state::<ModRuntime>() {
        {
            let _ledger = runtime.ledger_lock();
            runtime.clear();
        }
        // `clear` forgot every test folder's session: their watches go with them.
        super::folder_watch::resync(app);
    }
}

/// For "Remove PM data" (`wipe.rs`): nothing is served, and no ledger change runs, while the guard
/// lives. Synchronous only: drop it before any `.await`.
pub fn quiesce(app: &AppHandle) -> Option<MutexGuard<'_, ()>> {
    let runtime: &ModRuntime = app.try_state::<ModRuntime>()?.inner();
    let guard = runtime.ledger_lock();
    runtime.clear();
    // The watcher takes no ledger lock, so this is safe under the guard.
    super::folder_watch::resync(app);
    Some(guard)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::engine::Platform;
    use crate::mods::package::tests::signed_package;
    use crate::mods::runtime::FrameError;
    use minisign_verify::PublicKey;
    use std::fs;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;

    // The safety list against the ledger (slice 5).
    mod slice5;
    // The pilots' auto-install (PR B, B8).
    mod pilots;
    // The ledger's writers against the registry's frames (slice 6, seams).
    mod seams;
    // The Mods tab's writers: pause, resume, turn on, consented installs, uninstall (slice 6).
    mod slice6;
    // The unsigned file install, and developer mode as the ledger carries it out (slice 6, part 3).
    mod dev_unsigned;
    // The folder test-mod loader and the frozen-page watchdog's pause (slice 6, part 4).
    mod test_folder;
    mod watchdog_pause;

    const VAULT: &str = "11111111-2222-3333-4444-555555555555";
    const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    /// A store bound to one vault, that can be made unreadable or switched to another vault, and
    /// can run some SQL just before its `n`th step (another owner writing between two steps).
    struct TestStore {
        conn: Mutex<Connection>,
        vault_id: Mutex<String>,
        closed: AtomicBool,
        steps: AtomicUsize,
        before_step: Mutex<Option<(usize, String)>>,
    }

    impl LedgerStore for TestStore {
        fn with_conn<T>(
            &self,
            layout: &Layout,
            f: impl FnOnce(&mut Connection) -> Result<T, LedgerError>,
        ) -> Result<T, LedgerError> {
            if self.closed.load(Ordering::SeqCst) {
                return Err(LedgerError::Store("the vault is locked".into()));
            }
            let mut conn = self.conn.lock().unwrap();
            let step = self.steps.fetch_add(1, Ordering::SeqCst);
            if let Some((at, sql)) = &*self.before_step.lock().unwrap() {
                if *at == step {
                    conn.execute_batch(sql).unwrap();
                }
            }
            if *self.vault_id.lock().unwrap() != layout.vault_id() {
                return Err(LedgerError::VaultChanged);
            }
            same_store(&conn, layout)?;
            f(&mut conn)
        }
    }

    struct World {
        dir: tempfile::TempDir,
        layout: Layout,
        store: TestStore,
        runtime: ModRuntime,
    }

    /// A store of `VAULT` whose mods live under `data`, with its store id minted as a first install
    /// mints it.
    fn world_over(data: Option<&Path>) -> World {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        let store_id = store_id_for(&conn, Mint::Always).unwrap().unwrap();
        let data = data.map_or_else(|| dir.path().join("data"), Path::to_path_buf);
        let layout = Layout::new(&data, VAULT, &store_id).unwrap();
        World {
            layout,
            store: TestStore {
                conn: Mutex::new(conn),
                vault_id: Mutex::new(VAULT.into()),
                closed: AtomicBool::new(false),
                steps: AtomicUsize::new(0),
                before_step: Mutex::new(None),
            },
            runtime: ModRuntime::new(Platform::Linux, vec!["tauri://localhost".into()]),
            dir,
        }
    }

    fn world() -> World {
        world_over(None)
    }

    fn allow(_: &VerifiedPackage, _: &str, _: Source) -> Result<(), String> {
        Ok(())
    }

    fn serve_all(_: &str, _: &str, _: Option<&str>) -> Option<String> {
        None
    }

    fn opts(approval: Approval) -> InstallOptions<'static> {
        InstallOptions {
            source: Source::Catalogue,
            consented: true,
            approval,
            catalogue: &allow,
        }
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-05T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn id(s: &str) -> ModId {
        ModId::parse(s).unwrap()
    }

    /// The fixture package at `version`, with `change` applied to its manifest.
    fn pkg(version: &str, change: impl FnOnce(&mut Value)) -> (Vec<u8>, PublicKey) {
        signed_package(|m, _| {
            m["version"] = Value::String(version.into());
            change(m);
        })
    }

    impl World {
        fn ledger(&self) -> Ledger<'_, TestStore> {
            Ledger::new(&self.layout, &self.store, &self.runtime)
        }

        fn install(
            &self,
            (bytes, key): &(Vec<u8>, PublicKey),
            approval: Approval,
        ) -> Result<Installed, LedgerError> {
            self.ledger().install_package(
                bytes,
                &TrustedKeys::of_test_keys(std::slice::from_ref(key)),
                &opts(approval),
                now(),
            )
        }

        fn sql<T: rusqlite::types::FromSql>(&self, q: &str) -> T {
            self.store
                .conn
                .lock()
                .unwrap()
                .query_row(q, [], |r| r.get(0))
                .unwrap()
        }

        fn exec(&self, q: &str) {
            self.store.conn.lock().unwrap().execute_batch(q).unwrap();
        }

        fn rows(&self) -> i64 {
            self.sql("SELECT count(*) FROM mods")
        }

        /// Everything in this store's mods folder but its quarantine, sorted.
        fn installed_tree(&self) -> Vec<String> {
            let mut out = Vec::new();
            walk(self.layout.store_dir(), self.layout.store_dir(), &mut out);
            out.retain(|p| !p.starts_with(disk::QUARANTINE_DIR));
            out
        }

        fn staging_is_empty(&self) -> bool {
            fs::read_dir(self.layout.staging()).map_or(true, |mut rd| rd.next().is_none())
        }

        fn version_dir(&self, v: &str) -> PathBuf {
            self.layout
                .version_dir(&id("atlas.map"), &VersionName::parse(v).unwrap())
        }

        /// The folder as the registry holds it: canonical, which on Windows is the `\\?\` long
        /// form, not the temp folder's 8.3 name.
        fn canonical_version_dir(&self, v: &str) -> PathBuf {
            fs::canonicalize(self.version_dir(v)).unwrap()
        }

        fn served_from(&self) -> Option<PathBuf> {
            self.runtime.installed_root(&id("atlas.map"))
        }

        /// `from`'s rows as a restore carries them into this store: every column, none of the files.
        fn restore_rows_from(&self, from: &World) {
            let cols =
                "id, name, version, tier, author, source_repo, source_commit, signed, install_id, \
                        installed_from, package_sha256, manifest, files, project_refs, state, \
                        consented_at, installed_at, updated_at";
            let rows: Vec<Vec<rusqlite::types::Value>> = {
                let conn = from.store.conn.lock().unwrap();
                let mut stmt = conn.prepare(&format!("SELECT {cols} FROM mods")).unwrap();
                let n = stmt.column_count();
                stmt.query_map([], |r| (0..n).map(|i| r.get(i)).collect())
                    .unwrap()
                    .collect::<rusqlite::Result<_>>()
                    .unwrap()
            };
            let conn = self.store.conn.lock().unwrap();
            for row in &rows {
                let marks = vec!["?"; row.len()].join(", ");
                conn.execute(
                    &format!("INSERT INTO mods({cols}) VALUES ({marks})"),
                    rusqlite::params_from_iter(row),
                )
                .unwrap();
            }
        }
    }

    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let rel = e
                .path()
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if e.file_type().unwrap().is_dir() {
                out.push(format!("{rel}/"));
                walk(base, &e.path(), out);
            } else {
                out.push(rel);
            }
        }
        out.sort();
    }

    #[test]
    fn a_fresh_install_writes_the_files_then_the_row_then_serves() {
        let w = world();
        assert!(
            !w.layout.root().exists(),
            "no mod files before the first install"
        );
        let package = pkg("1.0.0", |_| {});
        let got = w.install(&package, Approval::Approved).unwrap();
        assert_eq!(
            got,
            Installed {
                kind: InstallKind::Fresh,
                version: "1.0.0".into(),
                state: "active".into(),
                pruned: vec![]
            }
        );
        // Exactly the package's files, as verified.
        let verified = package::verify(&package.0, std::slice::from_ref(&package.1)).unwrap();
        let dir = w.version_dir("1.0.0");
        for (path, f) in &verified.files {
            assert_eq!(fs::read(dir.join(path)).unwrap(), f.bytes, "{path}");
        }
        let on_disk: Vec<String> = w
            .installed_tree()
            .into_iter()
            .filter(|p| !p.ends_with('/'))
            .collect();
        let expected: Vec<String> = verified
            .files
            .keys()
            .map(|p| format!("atlas.map/1.0.0/{p}"))
            .collect();
        assert_eq!(on_disk, expected);
        assert!(w.staging_is_empty());
        // The row.
        let row: (String, String, String, String, i64, String, String, String, String, Option<String>) = w
            .store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT version, tier, author, source_repo, signed, installed_from, package_sha256, \
                        manifest, state, consented_at FROM mods WHERE id = 'atlas.map'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?)),
            )
            .unwrap();
        assert_eq!(
            (
                &row.0[..],
                &row.1[..],
                &row.2[..],
                &row.3[..],
                row.4,
                &row.5[..]
            ),
            (
                "1.0.0",
                "first",
                "Atlas",
                "itsAtlas-org/Personal-Manager",
                1,
                "catalogue"
            )
        );
        assert_eq!(row.6, hex::encode(Sha256::digest(&package.0)));
        assert_eq!(
            row.7.as_bytes(),
            verified.files[package::MANIFEST].bytes,
            "the manifest verbatim"
        );
        assert_eq!(row.8, "active");
        assert_eq!(row.9.as_deref(), Some("2026-11-05T12:00:00.000Z"));
        let files: Vec<FileRecord> =
            serde_json::from_str(&w.sql::<String>("SELECT files FROM mods")).unwrap();
        assert_eq!(files.len(), verified.files.len());
        assert!(files
            .iter()
            .all(|f| verified.files[&f.path].sha256 == decode_sha(&f.sha256).unwrap()));
        // Served from the version folder.
        assert_eq!(w.served_from(), Some(fs::canonicalize(&dir).unwrap()));
        assert!(w.runtime.open_frame(&id("atlas.map")).is_ok());
    }

    /// Each refusal comes before anything is written: no folder, no row change, nothing served.
    #[test]
    fn every_refusal_writes_nothing() {
        let w = world();
        let deny = |_: &VerifiedPackage, _: &str, _: Source| -> Result<(), String> {
            Err("it's on the blacklist".to_owned())
        };
        let blacklisted = InstallOptions {
            catalogue: &deny,
            ..opts(Approval::Approved)
        };
        let package = pkg("1.0.0", |_| {});
        match w.ledger().install_package(
            &package.0,
            &TrustedKeys::of_test_keys(std::slice::from_ref(&package.1)),
            &blacklisted,
            now(),
        ) {
            Err(LedgerError::Refused(Refusal::Catalogue(why))) => {
                assert_eq!(why, "it's on the blacklist")
            }
            other => panic!("{other:?}"),
        }
        let refused =
            |package: (Vec<u8>, PublicKey)| w.install(&package, Approval::Approved).unwrap_err();
        assert!(matches!(
            refused(pkg("1.0.0-RC1", |_| {})),
            LedgerError::Refused(Refusal::Path(PathRefusal::Version(_)))
        ));
        assert!(matches!(
            refused(pkg("1.0.0", |m| m["apiVersion"] =
                serde_json::json!({"min": "1.1", "target": "1.1"}))),
            LedgerError::Refused(Refusal::NeedsNewerPm { min: (1, 1) })
        ));
        let unsigned = signed_package(|_, _| {});
        assert!(matches!(
            w.ledger().install_package(
                &unsigned.0,
                &TrustedKeys::of_test_keys(&[]),
                &opts(Approval::Approved),
                now()
            ),
            Err(LedgerError::Package(_))
        ));
        // A key this machine has seen revoked signs nothing it installs (the key-id gate).
        let signer = crate::mods::keys::KeyId::parse("00000000000000AA").unwrap();
        let ring = crate::mods::keys::KeyRing::of([crate::mods::keys::AtlasKey {
            role: crate::mods::keys::KeyRole::Main,
            id: signer,
            key: package.1.clone(),
        }]);
        match w.ledger().install_package(
            &package.0,
            &ring.trusted(&BTreeSet::from([signer])),
            &opts(Approval::Approved),
            now(),
        ) {
            Err(LedgerError::Package(e)) => assert_eq!(e.refusal, package::Refusal::UntrustedKey),
            other => panic!("{other:?}"),
        }
        // The tier is checked again here, whatever the verifier said.
        let mut forged = package::verify(&package.0, std::slice::from_ref(&package.1)).unwrap();
        forged.manifest.publisher.tier = ids::Tier::Second;
        assert!(matches!(
            w.ledger()
                .install_verified(&forged, "00", &opts(Approval::Approved), now()),
            Err(LedgerError::Refused(Refusal::Tier(_)))
        ));
        assert_eq!(w.rows(), 0);
        assert!(w.installed_tree().is_empty() && w.staging_is_empty());

        // Against an installed 1.1.0: older, the same number as another file, another publisher.
        w.install(&pkg("1.1.0", |_| {}), Approval::Approved)
            .unwrap();
        let before = w.installed_tree();
        let manifest_before: String =
            w.sql("SELECT manifest || version || package_sha256 FROM mods");
        assert!(matches!(
            refused(pkg("1.0.9", |_| {})),
            LedgerError::Refused(Refusal::Downgrade { .. })
        ));
        assert!(matches!(
            refused(pkg("1.1.0", |m| m["description"] = "rebuilt".into())),
            LedgerError::Refused(Refusal::SameVersionDifferentFile)
        ));
        for change in [
            Box::new(|m: &mut Value| m["publisher"]["author"] = "Someone".into())
                as Box<dyn Fn(&mut Value)>,
            Box::new(|m: &mut Value| m["publisher"]["sourceRepo"] = "someone/fork".into()),
        ] {
            assert!(matches!(
                refused(pkg("2.0.0", |m| change(m))),
                LedgerError::Refused(Refusal::Publisher)
            ));
        }
        assert_eq!(w.installed_tree(), before);
        assert_eq!(
            w.sql::<String>("SELECT manifest || version || package_sha256 FROM mods"),
            manifest_before
        );
        assert!(w.staging_is_empty());
    }

    #[test]
    fn an_unattended_update_that_asks_for_more_waits_and_an_approved_one_installs() {
        let w = world();
        w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
            .unwrap();
        let wider = pkg("1.1.0", |m| {
            m["permissions"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({"id": "tasks.read", "why": "x"}));
        });
        match w.install(&wider, Approval::Unattended) {
            Err(LedgerError::Refused(Refusal::NeedsApproval(what))) => {
                assert_eq!(
                    what,
                    vec![policy::Widening::Permission("tasks.read".into())]
                )
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(w.sql::<String>("SELECT version FROM mods"), "1.0.0");
        assert!(!w.version_dir("1.1.0").exists());
        // Granted already: nothing new to ask.
        w.exec("INSERT INTO mod_grants(mod_id, permission, decision) VALUES ('atlas.map', 'tasks.read', 'granted')");
        assert_eq!(
            w.install(&wider, Approval::Unattended).unwrap().kind,
            InstallKind::Update
        );
        // A plain update needs nobody.
        assert_eq!(
            w.install(&pkg("1.2.0", |_| {}), Approval::Unattended)
                .unwrap()
                .version,
            "1.2.0"
        );
        // Adding a host widens: unattended, it waits; approved, it installs.
        let host = pkg("1.3.0", |m| {
            m["permissions"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({"id": "internet", "why": "x"}));
            m["internetHosts"] = serde_json::json!(["a.example.com"]);
        });
        match w.install(&host, Approval::Unattended) {
            Err(LedgerError::Refused(Refusal::NeedsApproval(what))) => assert_eq!(
                what,
                vec![
                    policy::Widening::Permission("internet".into()),
                    policy::Widening::Host("a.example.com".into())
                ]
            ),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            w.install(&host, Approval::Approved).unwrap().version,
            "1.3.0"
        );
    }

    /// A key is bound to its slot's host and header: an approved update that moves or drops the
    /// slot deletes the key in the install's own transaction, one that leaves the slot alone keeps
    /// it, and a refused or failed install deletes nothing.
    #[test]
    fn an_approved_update_that_moves_a_slot_deletes_its_key_and_the_rest_stay() {
        use serde_json::json;
        let w = world();
        let with = |version: &str, k_host: &str| {
            let k_host = k_host.to_owned();
            pkg(version, move |m| {
                m["permissions"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"id": "internet", "why": "x"}));
                m["internetHosts"] = json!(["a.example.com", "b.example.com"]);
                m["secrets"] = json!([
                    {"name": "k", "host": k_host, "header": "X-Key"},
                    {"name": "j", "host": "a.example.com", "header": "X-J"},
                ]);
            })
        };
        let keys = |w: &World| {
            w.sql::<String>(
                "SELECT group_concat(name, ',') FROM (SELECT name FROM mod_secrets ORDER BY name)",
            )
        };
        w.install(&with("1.0.0", "a.example.com"), Approval::Approved)
            .unwrap();
        w.exec(
            "INSERT INTO mod_secrets(mod_id, name, host, header, value) VALUES \
             ('atlas.map', 'k', 'a.example.com', 'X-Key', 'v1'), \
             ('atlas.map', 'j', 'a.example.com', 'X-J', 'v2')",
        );
        // The same slots: both keys stay.
        w.install(&with("1.1.0", "a.example.com"), Approval::Unattended)
            .unwrap();
        assert_eq!(keys(&w), "j,k");
        // `k` goes to another host: unattended it waits, and nothing is deleted by a refusal.
        let moved = with("1.2.0", "b.example.com");
        assert!(w.install(&moved, Approval::Unattended).is_err());
        assert_eq!(keys(&w), "j,k");
        // Approved, it installs and `k`'s old key is gone; `j` is untouched.
        w.install(&moved, Approval::Approved).unwrap();
        assert_eq!(keys(&w), "j");
        // A repair keeps what is exactly a slot and deletes a row that is not one.
        w.exec(
            "INSERT INTO mod_secrets(mod_id, name, host, header, value) VALUES \
             ('atlas.map', 'k', 'b.example.com', 'X-Key', 'v3'), \
             ('atlas.map', 'stale', 'a.example.com', 'X-S', 'v4')",
        );
        w.install(&moved, Approval::Approved).unwrap();
        assert_eq!(keys(&w), "j,k");
        // Dropping the slot altogether.
        let dropped = pkg("1.3.0", |_| {});
        w.install(&dropped, Approval::Approved).unwrap();
        assert_eq!(w.sql::<i64>("SELECT count(*) FROM mod_secrets"), 0);
    }

    /// Write, fsync (file and folder), rename and the row's commit: each failure leaves no row, no
    /// folder outside staging and quarantine, nothing in staging, and nothing served.
    #[test]
    fn each_failure_point_of_a_fresh_install_leaves_nothing() {
        for step in [
            (Step::Write, 0),
            (Step::Write, 3),
            (Step::SyncFile, 2),
            (Step::SyncDir, 0),
            (Step::SyncDir, 3),
            (Step::Rename, 0),
            (Step::Commit, 0),
        ] {
            let w = world();
            disk::fail_at(Some(step));
            let got = w.install(&pkg("1.0.0", |_| {}), Approval::Approved);
            disk::fail_at(None);
            assert!(got.is_err(), "{step:?} fails the install");
            assert_eq!(w.rows(), 0, "{step:?}");
            assert!(
                w.installed_tree().is_empty(),
                "{step:?} left {:?}",
                w.installed_tree()
            );
            assert!(w.staging_is_empty(), "{step:?}");
            assert_eq!(
                w.runtime.open_frame(&id("atlas.map")).unwrap_err(),
                FrameError::NotInstalled
            );
            // And the same install then succeeds.
            w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
                .unwrap();
        }
    }

    #[test]
    fn a_failed_update_keeps_the_old_version_running() {
        for step in [
            (Step::Write, 1),
            (Step::SyncDir, 1),
            (Step::Rename, 0),
            (Step::Commit, 0),
        ] {
            let w = world();
            w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
                .unwrap();
            let before = w.installed_tree();
            disk::fail_at(Some(step));
            let got = w.install(&pkg("1.1.0", |_| {}), Approval::Approved);
            disk::fail_at(None);
            assert!(got.is_err(), "{step:?}");
            assert_eq!(w.sql::<String>("SELECT version FROM mods"), "1.0.0");
            assert_eq!(w.installed_tree(), before, "{step:?}");
            assert!(w.staging_is_empty());
            assert_eq!(
                w.served_from(),
                Some(fs::canonicalize(w.version_dir("1.0.0")).unwrap())
            );
        }
    }

    #[test]
    fn an_update_moves_to_its_own_folder_and_prunes_the_old_one_after() {
        let w = world();
        w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
            .unwrap();
        let session = w.runtime.open_frame(&id("atlas.map")).unwrap();
        // From a file this time: the source is recorded, the pin is the publisher's either way.
        let (bytes, key) = pkg("1.1.0", |_| {});
        let from_file = InstallOptions {
            source: Source::File,
            ..opts(Approval::Approved)
        };
        let got = w
            .ledger()
            .install_package(
                &bytes,
                &TrustedKeys::of_test_keys(&[key]),
                &from_file,
                now(),
            )
            .unwrap();
        assert_eq!(
            (got.kind, got.pruned),
            (InstallKind::Update, vec!["1.0.0".to_owned()])
        );
        assert_eq!(
            w.sql::<String>("SELECT version || ':' || installed_from FROM mods"),
            "1.1.0:file"
        );
        assert!(!w.version_dir("1.0.0").exists());
        assert_eq!(
            w.served_from(),
            Some(fs::canonicalize(w.version_dir("1.1.0")).unwrap())
        );
        assert!(
            w.runtime.session_mod(&session.session).is_err(),
            "the old version's frames end"
        );
    }

    #[test]
    fn a_repair_puts_back_exactly_the_pinned_package() {
        let w = world();
        let package = pkg("1.0.0", |_| {});
        w.install(&package, Approval::Approved).unwrap();
        let main = w.version_dir("1.0.0").join("app/main.js");
        fs::write(&main, b"tampered, and longer than before").unwrap();
        // A fresh runtime's startup parks it; the repair brings it back.
        let report = w.ledger().startup(now(), &serve_all);
        assert_eq!(
            report.parked,
            vec![("atlas.map".to_owned(), store::REASON_FILES_CHANGED)]
        );
        assert_eq!(w.sql::<String>("SELECT state FROM mods"), "parked");
        assert_eq!(w.served_from(), None);
        let got = w.install(&package, Approval::Approved).unwrap();
        assert_eq!((got.kind, &got.state[..]), (InstallKind::Repair, "active"));
        assert_eq!(fs::read(&main).unwrap(), b"console.log('map');");
        assert_eq!(
            w.sql::<Option<String>>("SELECT state_reason FROM mods"),
            None
        );
        assert!(w.served_from().is_some());
        // The broken copy waits in quarantine, not deleted.
        let mut q = Vec::new();
        walk(&w.layout.quarantine(), &w.layout.quarantine(), &mut q);
        assert!(q.iter().any(|p| p.ends_with("/1.0.0/app/main.js")), "{q:?}");
    }

    /// The commit re-reads the pin under its transaction: a row that appeared, or changed, since the
    /// checks read it refuses the commit rather than overwriting it.
    #[test]
    fn the_commit_refuses_a_row_that_changed_since_the_checks() {
        let w = world();
        w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
            .unwrap();
        let pin = read_current(&w.store.conn.lock().unwrap(), &id("atlas.map"))
            .unwrap()
            .unwrap()
            .pinned;
        let write = RowWrite {
            id: "atlas.map",
            name: "Map",
            version: "1.1.0",
            tier: "first",
            author: "Atlas",
            source_repo: "itsAtlas-org/Personal-Manager",
            source_commit: &"0".repeat(40),
            signed: true,
            installed_from: store::FROM_CATALOGUE,
            package_sha256: Some(&"a".repeat(64)),
            manifest: "{}",
            files: "[]".into(),
            project_refs: "[]".into(),
            consented_at: None,
            install_id: "fedcba9876543210".into(),
            now: time_text(now()),
        };
        let mut conn = w.store.conn.lock().unwrap();
        let stale = Pinned {
            version: "0.9.0".into(),
            ..pin.clone()
        };
        for (kind, expected) in [
            (InstallKind::Fresh, None),
            (InstallKind::Update, Some(&stale)),
        ] {
            assert!(matches!(
                commit_with(&mut conn, &write, kind, expected, &|_, _| Ok(())),
                Err(LedgerError::Refused(Refusal::Changed))
            ));
        }
        drop(conn);
        assert_eq!(w.sql::<String>("SELECT version FROM mods"), "1.0.0");
        w.exec("DELETE FROM mods");
        let mut conn = w.store.conn.lock().unwrap();
        assert!(
            matches!(
                commit_with(
                    &mut conn,
                    &write,
                    InstallKind::Update,
                    Some(&pin),
                    &|_, _| Ok(())
                ),
                Err(LedgerError::Refused(Refusal::Changed))
            ),
            "an uninstall since the checks isn't updated back into being"
        );
    }

    /// A pause another owner writes while an update is staging (the blacklist, the user) survives
    /// the update's commit, and the registry follows the row the commit read, not the earlier one.
    #[test]
    fn a_pause_written_during_an_install_survives_its_commit() {
        let w = world();
        w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
            .unwrap();
        assert!(w.served_from().is_some());
        // The update's steps: 0 reads the row for the checks, 1 commits.
        let at = w.store.steps.load(Ordering::SeqCst) + 1;
        *w.store.before_step.lock().unwrap() = Some((
            at,
            "UPDATE mods SET state = 'paused', state_reason = 'blacklist'".into(),
        ));
        let got = w
            .install(&pkg("1.1.0", |_| {}), Approval::Approved)
            .unwrap();
        assert_eq!((got.kind, &got.state[..]), (InstallKind::Update, "paused"));
        assert_eq!(
            w.sql::<String>("SELECT version || ':' || state || ':' || state_reason FROM mods"),
            "1.1.0:paused:blacklist"
        );
        assert_eq!(w.served_from(), None, "a paused mod isn't served");
    }

    #[test]
    fn a_change_begun_in_one_vault_never_lands_in_another() {
        let w = world();
        *w.store.vault_id.lock().unwrap() = "another-vault".into();
        assert!(matches!(
            w.install(&pkg("1.0.0", |_| {}), Approval::Approved),
            Err(LedgerError::VaultChanged)
        ));
        assert!(w.installed_tree().is_empty() && w.staging_is_empty());
        assert!(matches!(
            w.ledger().uninstall(&id("atlas.map")),
            Err(LedgerError::VaultChanged)
        ));
        // Nor in another store of this vault (a restored copy switched in): same vault id, another
        // store id.
        *w.store.vault_id.lock().unwrap() = VAULT.into();
        w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
            .unwrap();
        w.exec("UPDATE settings SET value = 'fedcba9876543210' WHERE key = 'mods_store_id'");
        assert!(matches!(
            w.install(&pkg("1.1.0", |_| {}), Approval::Approved),
            Err(LedgerError::VaultChanged)
        ));
        assert!(matches!(
            w.ledger().uninstall(&id("atlas.map")),
            Err(LedgerError::VaultChanged)
        ));
        assert!(matches!(
            w.ledger().expire(now()),
            Err(LedgerError::VaultChanged)
        ));
        let report = w.ledger().startup(now(), &serve_all);
        assert!(report.skipped.is_some() && report.sweep.is_empty());
        assert_eq!(w.sql::<String>("SELECT version FROM mods"), "1.0.0");
        assert!(w.version_dir("1.0.0").exists());
        // A store id that isn't one PM mints can't name a folder, and is never minted over.
        w.exec("UPDATE settings SET value = '../../elsewhere' WHERE key = 'mods_store_id'");
        let conn = w.store.conn.lock().unwrap();
        for mint in [Mint::Always, Mint::IfRows] {
            assert!(store_id_for(&conn, mint).is_err());
        }
        assert_eq!(
            conn.query_row(
                "SELECT value FROM settings WHERE key = 'mods_store_id'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "../../elsewhere"
        );
    }

    /// A store gets its id when it first needs a folder: an install, or a startup with rows (a
    /// restored copy, whose id the restore cleared). One that never installed a mod writes nothing.
    #[test]
    fn a_store_is_named_only_when_it_needs_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        assert_eq!(store_id_for(&conn, Mint::IfRows).unwrap(), None);
        let settings = |c: &Connection| -> i64 {
            c.query_row(
                "SELECT count(*) FROM settings WHERE key = 'mods_store_id'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(settings(&conn), 0);
        let minted = store_id_for(&conn, Mint::Always).unwrap().unwrap();
        assert_eq!(disk::check_store_id(&minted), Ok(()));
        assert_eq!(
            store_id_for(&conn, Mint::Always).unwrap().as_deref(),
            Some(minted.as_str()),
            "minted once, then kept"
        );
        forget_store_id(&conn).unwrap();
        assert_eq!(store_id_for(&conn, Mint::IfRows).unwrap(), None);
        conn.execute_batch(
            "INSERT INTO mods(id, name, version, tier, author, source_repo, source_commit, signed, \
                              install_id, installed_from, package_sha256, manifest, files) \
             VALUES ('atlas.map', 'Map', '1.0.0', 'first', 'Atlas', 'r', 'c', 1, \
                     '0123456789abcdef', 'catalogue', \
                     '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef', '{}', '[]')",
        )
        .unwrap();
        let again = store_id_for(&conn, Mint::IfRows).unwrap().unwrap();
        assert_ne!(again, minted, "a restored copy is a store of its own");
    }

    /// Two stores of one vault on one machine (a restore keeps the vault id, and its copy can run
    /// beside the vault it came from) never touch each other's files: not by startup, sweep, update
    /// prune or uninstall.
    #[test]
    fn two_stores_of_one_vault_keep_to_their_own_files() {
        let a = world();
        let data = a.layout.root().parent().unwrap().to_path_buf();
        let b = world_over(Some(&data));
        assert_ne!(a.layout.store_id(), b.layout.store_id());
        a.install(&pkg("1.0.0", |_| {}), Approval::Approved)
            .unwrap();
        a.install(&pkg("2.0.0", |_| {}), Approval::Approved)
            .unwrap();
        a.install(
            &signed_package(|m, _| m["id"] = "atlas.pinboard".into()),
            Approval::Approved,
        )
        .unwrap();
        let a_tree = a.installed_tree();
        // B: a restored copy of A from when the Map was at 1.0.0 and the Pinboard not installed.
        let row: (String, String) = a
            .store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT manifest, files FROM mods WHERE id = 'atlas.map'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        b.store
            .conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO mods(id, name, version, tier, author, source_repo, source_commit, \
                                  signed, install_id, installed_from, package_sha256, manifest, \
                                  files, state, consented_at) \
                 VALUES ('atlas.map', 'Map', '1.0.0', 'first', 'Atlas', \
                         'itsAtlas-org/Personal-Manager', 'c', 1, '0123456789abcdef', 'catalogue', \
                         ?1, ?2, ?3, 'active', '2026-11-05T12:00:00.000Z')",
                params!["0".repeat(64), row.0, row.1],
            )
            .unwrap();
        // A no longer holds 1.0.0 (its update pruned it), so there is nothing exact to copy.
        let report = b.ledger().startup(now(), &serve_all);
        assert_eq!(
            report.parked,
            vec![("atlas.map".to_owned(), store::REASON_FILES_MISSING)]
        );
        assert!(report.sweep.is_empty(), "{:?}", report.sweep);
        b.ledger()
            .expire(now() + chrono::Duration::days(31))
            .unwrap();
        // B installs and uninstalls on its own.
        b.install(
            &signed_package(|m, _| m["id"] = "atlas.pinboard".into()),
            Approval::Approved,
        )
        .unwrap();
        b.ledger().uninstall(&id("atlas.pinboard")).unwrap();
        b.ledger().uninstall(&id("atlas.map")).unwrap();
        assert_eq!(a.installed_tree(), a_tree, "A's files are as A left them");
        let report = a.ledger().startup(now(), &serve_all);
        assert_eq!(report.loaded, ["atlas.map", "atlas.pinboard"]);
        assert!(report.parked.is_empty() && report.sweep.is_empty());
    }

    /// A restored copy is a store of its own, so its rows name files its own folder doesn't hold.
    /// When the store it was made from holds them exactly, they are copied in (never moved) and its
    /// mods come back as they were; a copy that isn't exact stays where it is and the row parks. The
    /// copy is `recover_missing`'s, never the startup load's, which an open path runs inline; and it
    /// is made only into the store that is still open.
    #[test]
    fn a_restored_copy_gets_its_mods_from_the_store_it_was_made_from() {
        let a = world();
        let data = a.layout.root().parent().unwrap().to_path_buf();
        let b = world_over(Some(&data));
        a.install(&pkg("1.0.0", |_| {}), Approval::Approved)
            .unwrap();
        let a_tree = a.installed_tree();
        b.restore_rows_from(&a);
        // A's copy with a changed byte: not taken.
        let main = a.version_dir("1.0.0").join("app/main.js");
        let original = fs::read(&main).unwrap();
        let mut changed = original.clone();
        changed[0] ^= 1;
        fs::write(&main, &changed).unwrap();
        let report = b.ledger().startup(now(), &serve_all);
        assert!(report.recovered.is_empty());
        assert_eq!(
            report.parked,
            vec![("atlas.map".to_owned(), store::REASON_FILES_MISSING)]
        );
        let report = b.ledger().recover_missing(now(), &serve_all).unwrap();
        assert_eq!(report, StartupReport::default());
        assert!(b.installed_tree().is_empty());
        // The exact copy: the startup load leaves it where it is, and the row parked.
        fs::write(&main, &original).unwrap();
        let report = b.ledger().startup(now(), &serve_all);
        assert!(report.recovered.is_empty() && report.loaded.is_empty());
        assert!(b.installed_tree().is_empty());
        assert_eq!(b.served_from(), None);
        // `recover_missing` copies it in: unparked, served from B's own folder, A's left as it was.
        let report = b.ledger().recover_missing(now(), &serve_all).unwrap();
        assert_eq!(
            report.recovered,
            [format!(
                "atlas.map/1.0.0 from {}/atlas.map/1.0.0",
                a.layout.store_id()
            )]
        );
        assert_eq!(report.unparked, ["atlas.map"]);
        assert_eq!(report.loaded, ["atlas.map"]);
        assert_eq!(
            b.sql::<String>("SELECT state FROM mods"),
            store::STATE_ACTIVE
        );
        assert_eq!(b.served_from(), Some(b.canonical_version_dir("1.0.0")));
        assert_eq!(b.installed_tree(), a_tree);
        assert_eq!(a.installed_tree(), a_tree, "copied, never moved");
        assert!(b.staging_is_empty());
        // A pass that finds nothing missing copies nothing and leaves the registry as it is.
        let report = b.ledger().recover_missing(now(), &serve_all).unwrap();
        assert_eq!(report, StartupReport::default());
        let served = b.canonical_version_dir("1.0.0");
        assert_eq!(b.served_from(), Some(served.clone()));
        // Begun for B while another store is open, it copies nothing and leaves the registry alone
        // (the open store's own load decides it): B's files go missing again, and stay missing.
        fs::remove_dir_all(b.version_dir("1.0.0")).unwrap();
        *b.store.vault_id.lock().unwrap() = "another-vault".into();
        assert!(matches!(
            b.ledger().recover_missing(now(), &serve_all),
            Err(LedgerError::VaultChanged)
        ));
        assert!(!b.version_dir("1.0.0").exists());
        assert_eq!(b.served_from(), Some(served));
        *b.store.vault_id.lock().unwrap() = VAULT.into();
        let report = a.ledger().startup(now(), &serve_all);
        assert_eq!(report.loaded, ["atlas.map"]);
        assert!(report.parked.is_empty() && report.sweep.is_empty());
    }

    /// A store that stops being the open one partway through `recover_missing` gets no copy after
    /// the one in progress, and no reload: the pass stops at the next check, leaving the registry
    /// to the load of whichever store is open now.
    #[test]
    fn a_store_closed_mid_recovery_gets_no_further_copy_and_no_reload() {
        let a = world();
        let data = a.layout.root().parent().unwrap().to_path_buf();
        let b = world_over(Some(&data));
        a.install(&pkg("1.0.0", |_| {}), Approval::Approved)
            .unwrap();
        let pinboard = signed_package(|m, _| m["id"] = "atlas.pinboard".into());
        a.install(&pinboard, Approval::Approved).unwrap();
        b.restore_rows_from(&a);
        assert_eq!(b.ledger().startup(now(), &serve_all).parked.len(), 2);
        let pinboard_dir = b.layout.mod_dir(&id("atlas.pinboard"));
        let pinboard_served = || b.runtime.installed_root(&id("atlas.pinboard"));
        let other_store =
            "UPDATE settings SET value = 'fedcba9876543210' WHERE key = 'mods_store_id'";
        let reopen_b = format!(
            "UPDATE settings SET value = '{}' WHERE key = 'mods_store_id'",
            b.layout.store_id()
        );
        // Another store is open from the Pinboard's copy on. The pass's steps: 0 reads the rows,
        // 1 begins the Map's copy, 2 the Pinboard's.
        let at = b.store.steps.load(Ordering::SeqCst) + 2;
        *b.store.before_step.lock().unwrap() = Some((at, other_store.into()));
        assert!(matches!(
            b.ledger().recover_missing(now(), &serve_all),
            Err(LedgerError::VaultChanged)
        ));
        assert!(b.version_dir("1.0.0").exists(), "copied while B was open");
        assert!(!pinboard_dir.exists());
        assert_eq!((b.served_from(), pinboard_served()), (None, None));
        // Another store is open by the reload. Steps: 0 reads, 1 begins the Pinboard's copy (the
        // Map's files are there now), 2 checks before the reload.
        b.exec(&reopen_b);
        let at = b.store.steps.load(Ordering::SeqCst) + 2;
        *b.store.before_step.lock().unwrap() = Some((at, other_store.into()));
        assert!(matches!(
            b.ledger().recover_missing(now(), &serve_all),
            Err(LedgerError::VaultChanged)
        ));
        assert!(pinboard_dir.exists(), "copied while B was open");
        assert_eq!((b.served_from(), pinboard_served()), (None, None));
        assert_eq!(
            b.sql::<i64>("SELECT count(*) FROM mods WHERE state = 'parked'"),
            2
        );
        // B's own next load finds both copies and serves them.
        b.exec(&reopen_b);
        *b.store.before_step.lock().unwrap() = None;
        let report = b.ledger().startup(now(), &serve_all);
        assert_eq!(report.unparked, ["atlas.map", "atlas.pinboard"]);
        assert_eq!(b.served_from(), Some(b.canonical_version_dir("1.0.0")));
        assert!(pinboard_served().is_some());
    }

    /// When PM deletes a store whose vault the open store is a copy of (the pristine home a
    /// restore re-homed over), the vault's other store folders go, but only after the open store
    /// has copied from them what its rows need, in the same hold of the ledger lock. A restore of
    /// a store whose ledger is gone has its only copies there, and the recovery its open spawned
    /// may not have run yet. Nothing goes when that pass can't finish or a copy fails.
    #[test]
    fn removing_other_stores_takes_their_copies_first_and_nothing_when_it_cant() {
        let a = world();
        let data = a.layout.root().parent().unwrap().to_path_buf();
        let b = world_over(Some(&data));
        a.install(&pkg("1.0.0", |_| {}), Approval::Approved)
            .unwrap();
        let a_tree = a.installed_tree();
        b.restore_rows_from(&a);
        assert_eq!(b.ledger().startup(now(), &serve_all).parked.len(), 1);
        let remove = || {
            let _held = b.runtime.ledger_lock();
            b.ledger().remove_other_stores(&data, now(), &serve_all)
        };
        // A copy that fails (staging can't be made): every folder is left.
        if b.layout.staging().exists() {
            fs::remove_dir(b.layout.staging()).unwrap();
        }
        fs::write(b.layout.staging(), b"").unwrap();
        let report = remove().unwrap();
        assert_eq!(report.unrecovered.len(), 1, "{report:?}");
        assert!(report.recovered.is_empty());
        assert_eq!(a.installed_tree(), a_tree);
        fs::remove_file(b.layout.staging()).unwrap();
        // A store that isn't open any more: nothing copied, nothing removed.
        b.store.closed.store(true, Ordering::SeqCst);
        assert!(remove().is_err());
        assert_eq!(a.installed_tree(), a_tree);
        assert!(b.installed_tree().is_empty());
        b.store.closed.store(false, Ordering::SeqCst);
        // The copy, then the other folders go; B's own is kept, and serves.
        let report = remove().unwrap();
        assert_eq!(
            report.recovered,
            [format!(
                "atlas.map/1.0.0 from {}/atlas.map/1.0.0",
                a.layout.store_id()
            )]
        );
        assert!(report.unrecovered.is_empty());
        assert_eq!(b.installed_tree(), a_tree);
        assert_eq!(b.served_from(), Some(b.canonical_version_dir("1.0.0")));
        assert!(!a.layout.store_dir().exists());
    }

    /// The other stores' folders stay for a row the pass never looked for, since one of them may
    /// hold the copy it needs: a row this PM can't read back (an API range it is too old for,
    /// which an update of PM serves again) or one whose files are there but differ (a repair's).
    #[test]
    fn removing_other_stores_keeps_them_for_a_row_it_never_looked_for() {
        let a = world();
        let data = a.layout.root().parent().unwrap().to_path_buf();
        let b = world_over(Some(&data));
        a.install(&pkg("1.0.0", |_| {}), Approval::Approved)
            .unwrap();
        let a_tree = a.installed_tree();
        b.restore_rows_from(&a);
        let remove = || {
            let _held = b.runtime.ledger_lock();
            b.ledger().remove_other_stores(&data, now(), &serve_all)
        };
        let set_manifest = |m: &str| {
            b.store
                .conn
                .lock()
                .unwrap()
                .execute("UPDATE mods SET manifest = ?1", [m])
                .unwrap();
        };
        // A row that needs a newer PM: nothing copied, nothing removed.
        let original: String = b.sql("SELECT manifest FROM mods");
        let mut newer: Value = serde_json::from_str(&original).unwrap();
        newer["apiVersion"] = serde_json::json!({"min": "1.9", "target": "1.9"});
        set_manifest(&newer.to_string());
        let report = remove().unwrap();
        assert!(report.recovered.is_empty(), "{report:?}");
        assert!(report.unrecovered.is_empty(), "{report:?}");
        assert_eq!(a.installed_tree(), a_tree);
        assert!(b.installed_tree().is_empty());
        // A row whose files are there but differ: A's copy stays.
        set_manifest(&original);
        b.ledger().recover_missing(now(), &serve_all).unwrap();
        assert_eq!(b.installed_tree(), a_tree);
        let entry: String = b.sql("SELECT json_extract(manifest, '$.entry.page') FROM mods");
        let file = b.version_dir("1.0.0").join(&entry);
        let bytes = fs::read(&file).unwrap();
        fs::write(&file, [&bytes[..], b"!"].concat()).unwrap();
        remove().unwrap();
        assert_eq!(a.installed_tree(), a_tree);
        // Put right, the row is complete, and the other folders go.
        fs::write(&file, &bytes).unwrap();
        remove().unwrap();
        assert!(!a.layout.store_dir().exists());
        assert_eq!(b.installed_tree(), a_tree);
    }

    /// A fresh install mints the row's install id, an update and a repair keep it, and an install
    /// after an uninstall gets a new one: an unsigned maker's gifts are its install's
    /// (`policy::made_it`), so a reinstall starts with no claim on them.
    #[test]
    fn an_install_id_lasts_through_updates_and_repairs_and_not_a_reinstall() {
        let w = world();
        let v1 = pkg("1.0.0", |_| {});
        w.install(&v1, Approval::Approved).unwrap();
        let first: String = w.sql("SELECT install_id FROM mods");
        assert_eq!(disk::check_store_id(&first), Ok(()), "16 lowercase hex");
        assert_eq!(
            w.install(&v1, Approval::Approved).unwrap().kind,
            InstallKind::Repair
        );
        assert_eq!(w.sql::<String>("SELECT install_id FROM mods"), first);
        let update = w.install(&pkg("2.0.0", |_| {}), Approval::Approved);
        assert_eq!(update.unwrap().kind, InstallKind::Update);
        assert_eq!(w.sql::<String>("SELECT install_id FROM mods"), first);
        w.ledger().uninstall(&id("atlas.map")).unwrap();
        assert_eq!(
            w.install(&v1, Approval::Approved).unwrap().kind,
            InstallKind::Fresh
        );
        assert_ne!(w.sql::<String>("SELECT install_id FROM mods"), first);
    }

    /// The safety net works both ways: a sweep made from an older copy of this store's own ledger
    /// (a sync glitch, a store file put back) quarantines files the newer ledger still names, and
    /// when that ledger is back its exact files come back with it. A copy that isn't exact stays.
    #[test]
    fn a_ledger_that_comes_back_gets_its_quarantined_files_back() {
        let w = world();
        w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
            .unwrap();
        w.exec("CREATE TABLE saved AS SELECT * FROM mods; DELETE FROM mods;");
        let report = w.ledger().startup(now(), &serve_all);
        assert_eq!(report.sweep.quarantined, ["atlas.map"]);
        assert!(w.installed_tree().is_empty());
        w.exec("INSERT INTO mods SELECT * FROM saved; DROP TABLE saved;");
        // A changed byte in the quarantined copy: not put back, so the row parks.
        let quarantined = w
            .layout
            .quarantine()
            .join("20261105T120000Z-atlas.map/1.0.0/app/main.js");
        let original = fs::read(&quarantined).unwrap();
        let mut changed = original.clone();
        changed[0] ^= 1;
        fs::write(&quarantined, &changed).unwrap();
        let report = w.ledger().startup(now(), &serve_all);
        assert!(report.recovered.is_empty());
        assert_eq!(
            report.parked,
            vec![("atlas.map".to_owned(), store::REASON_FILES_MISSING)]
        );
        assert!(quarantined.exists());
        // The exact copy comes back, unparked and served.
        fs::write(&quarantined, &original).unwrap();
        let report = w.ledger().startup(now(), &serve_all);
        assert_eq!(
            report.recovered,
            ["atlas.map/1.0.0 from 20261105T120000Z-atlas.map"]
        );
        assert_eq!(report.unparked, ["atlas.map"]);
        assert_eq!(report.loaded, ["atlas.map"]);
        assert!(w.served_from().is_some());
        assert!(fs::read_dir(w.layout.quarantine())
            .unwrap()
            .next()
            .is_none());
    }

    /// Slice 5's hook: a held row isn't served, and its row and files are left as they were.
    #[test]
    fn a_row_the_serve_check_holds_is_not_served_and_keeps_its_files() {
        let w = world();
        w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
            .unwrap();
        let sha: String = w.sql("SELECT package_sha256 FROM mods");
        let seen = Mutex::new(Vec::new());
        let blacklist = |id: &str, version: &str, sha: Option<&str>| {
            seen.lock()
                .unwrap()
                .push(format!("{id} {version} {}", sha.unwrap_or("-")));
            Some("on the blacklist".to_owned())
        };
        let report = w.ledger().startup(now(), &blacklist);
        assert_eq!(
            report.held,
            vec![("atlas.map".to_owned(), "on the blacklist".to_owned())]
        );
        assert!(report.loaded.is_empty() && report.sweep.is_empty());
        assert_eq!(*seen.lock().unwrap(), [format!("atlas.map 1.0.0 {sha}")]);
        assert_eq!(w.served_from(), None);
        assert_eq!(w.sql::<String>("SELECT state FROM mods"), "active");
        assert!(w.version_dir("1.0.0").exists());
    }

    /// Once the lock session settles, the registry matches the store: the writer of an open store
    /// keeps its mods (and loads), and anything else serves none, however the store was closed.
    #[test]
    fn a_settled_lock_that_isnt_an_open_stores_writer_serves_nothing() {
        let w = world();
        w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
            .unwrap();
        assert!(settle_registry(&w.runtime, || true));
        assert!(w.served_from().is_some());
        assert!(!settle_registry(&w.runtime, || false));
        assert_eq!(w.served_from(), None);
    }

    #[test]
    fn uninstall_takes_the_mod_and_its_data_and_leaves_its_gifts_and_spend() {
        let w = world();
        w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
            .unwrap();
        w.exec(
            "INSERT INTO tasks(title) VALUES ('call Sam');
             INSERT INTO mod_grants(mod_id, permission, decision) VALUES ('atlas.map', 'storage', 'granted');
             INSERT INTO mod_kv(mod_id, key, class, value) VALUES ('atlas.map', 'settings', 'content', '{}');
             INSERT INTO mod_secrets(mod_id, name, host, header, value) VALUES ('atlas.map', 't', 'h.example', 'X', 's');
             INSERT INTO mod_ledger(mod_id, mod_name, tier, author, source_repo, signed, install_id, task_id)
                 SELECT 'atlas.map', 'Map', tier, author, source_repo, signed, install_id, (SELECT id FROM tasks)
                 FROM mods WHERE id = 'atlas.map';
             INSERT INTO usage_log(model, kind, mod_id) VALUES ('m', 'background', 'atlas.map');
             INSERT INTO settings(key, value) VALUES ('calendar_ui',
                 '{\"hidden\":[\"mod:atlas.map\",\"work@example.com\",\"mod:atlas.mapper\"],\"last_create\":\"work@example.com\"}');",
        );
        let got = w.ledger().uninstall(&id("atlas.map")).unwrap();
        assert!(got.files_removed);
        assert_eq!(w.rows(), 0);
        for table in ["mod_grants", "mod_kv", "mod_secrets"] {
            assert_eq!(
                w.sql::<i64>(&format!("SELECT count(*) FROM {table}")),
                0,
                "{table}"
            );
        }
        assert_eq!(
            w.sql::<i64>("SELECT count(*) FROM mod_ledger WHERE mod_name = 'Map'"),
            1,
            "\"made by Map\" stays"
        );
        assert_eq!(
            w.sql::<i64>("SELECT count(*) FROM tasks"),
            1,
            "the gift itself stays"
        );
        assert_eq!(
            w.sql::<i64>("SELECT count(*) FROM usage_log WHERE mod_id = 'atlas.map'"),
            1,
            "spend history stays"
        );
        let ui: Value = serde_json::from_str(
            &w.sql::<String>("SELECT value FROM settings WHERE key = 'calendar_ui'"),
        )
        .unwrap();
        assert_eq!(
            ui["hidden"],
            serde_json::json!(["work@example.com", "mod:atlas.mapper"])
        );
        assert_eq!(ui["last_create"], "work@example.com");
        assert!(
            !w.layout.root().exists(),
            "the last uninstall leaves no mods folder"
        );
        assert_eq!(
            w.runtime.open_frame(&id("atlas.map")).unwrap_err(),
            FrameError::NotInstalled
        );
        assert!(matches!(
            w.ledger().uninstall(&id("atlas.map")),
            Err(LedgerError::NotInstalled)
        ));
    }

    #[test]
    fn a_failed_uninstall_changes_nothing() {
        let w = world();
        w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
            .unwrap();
        w.exec("INSERT INTO mod_kv(mod_id, key, class, value) VALUES ('atlas.map', 'settings', 'content', '{}')");
        disk::fail_at(Some((Step::Commit, 0)));
        assert!(w.ledger().uninstall(&id("atlas.map")).is_err());
        disk::fail_at(None);
        assert_eq!(w.rows(), 1);
        assert_eq!(w.sql::<i64>("SELECT count(*) FROM mod_kv"), 1);
        assert!(w.version_dir("1.0.0").exists());
        assert!(w.served_from().is_some());
    }

    /// Spec §4: the Pinboard's overlay keeps its pre-mods source id, so a hidden set saved before
    /// the move still applies and its uninstall still prunes it; every other mod's is `mod:<id>`.
    #[test]
    fn overlay_sources_keep_the_pinboards_old_name() {
        assert_eq!(overlay_source(&id("atlas.map")), "mod:atlas.map");
        assert_eq!(overlay_source(&id("atlas.pinboard")), "pm:pinboard");
        assert_eq!(overlay_source(&id("acme.pinboard")), "mod:acme.pinboard");
        let p = id("atlas.pinboard");
        assert_eq!(
            without_hidden(r#"{"hidden":["pm:pinboard","mod:atlas.pinboard"]}"#, &p).as_deref(),
            Some(r#"{"hidden":["mod:atlas.pinboard"]}"#)
        );
    }

    #[test]
    fn calendar_hidden_pruning_leaves_any_other_shape_alone() {
        let m = id("atlas.map");
        assert_eq!(
            without_hidden(r#"{"hidden":["mod:atlas.map"],"x":1}"#, &m).as_deref(),
            Some(r#"{"hidden":[],"x":1}"#)
        );
        for untouched in [
            r#"{"hidden":["cal"]}"#,
            r#"{"hidden":"mod:atlas.map"}"#,
            r#"{}"#,
            "not json",
            "[]",
        ] {
            assert_eq!(without_hidden(untouched, &m), None, "{untouched}");
        }
    }

    /// Startup loads what checks out and parks the rest with its reason, never deleting a row; the
    /// files coming back unparks it.
    #[test]
    fn startup_loads_rows_and_parks_what_is_missing() {
        let w = world();
        w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
            .unwrap();
        w.install(
            &signed_package(|m, _| m["id"] = "atlas.pinboard".into()),
            Approval::Approved,
        )
        .unwrap();
        let held = w
            .layout
            .version_dir(&id("atlas.map"), &VersionName::parse("1.0.0").unwrap())
            .join("app/main.js");
        let aside = w.dir.path().join("main.js");
        fs::rename(&held, &aside).unwrap();

        let fresh = ModRuntime::new(Platform::Linux, vec![]);
        let report = Ledger::new(&w.layout, &w.store, &fresh).startup(now(), &serve_all);
        assert_eq!(report.loaded, ["atlas.pinboard"]);
        assert_eq!(
            report.parked,
            vec![("atlas.map".to_owned(), store::REASON_FILES_MISSING)]
        );
        assert!(report.sweep.is_empty(), "{:?}", report.sweep);
        assert_eq!(w.rows(), 2, "a broken mod is parked, never deleted");
        assert_eq!(
            w.sql::<String>("SELECT state || ':' || state_reason FROM mods WHERE id = 'atlas.map'"),
            "parked:files-missing"
        );
        assert!(fresh.installed_root(&id("atlas.map")).is_none());
        assert!(w.version_dir("1.0.0").exists(), "its folder stays too");

        fs::rename(&aside, &held).unwrap();
        let report = Ledger::new(&w.layout, &w.store, &fresh).startup(now(), &serve_all);
        assert_eq!(report.unparked, ["atlas.map"]);
        assert_eq!(report.loaded, ["atlas.map", "atlas.pinboard"]);
        assert_eq!(
            w.sql::<String>("SELECT state FROM mods WHERE id = 'atlas.map'"),
            "active"
        );
    }

    #[test]
    fn startup_serves_only_active_consented_rows_and_leaves_other_owners_states() {
        let w = world();
        w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
            .unwrap();
        w.exec("UPDATE mods SET state = 'paused', state_reason = 'blacklist'");
        let report = w.ledger().startup(now(), &serve_all);
        assert!(report.loaded.is_empty() && report.parked.is_empty());
        assert_eq!(
            w.sql::<String>("SELECT state || ':' || state_reason FROM mods"),
            "paused:blacklist"
        );
        assert_eq!(w.served_from(), None);
        w.exec("UPDATE mods SET state = 'active', state_reason = NULL, consented_at = NULL");
        assert!(
            w.ledger().startup(now(), &serve_all).loaded.is_empty(),
            "an auto-installed pilot waits for its first-open screen"
        );
        w.exec("UPDATE mods SET consented_at = '2026-11-05T12:00:00.000Z'");
        assert_eq!(w.ledger().startup(now(), &serve_all).loaded, ["atlas.map"]);
    }

    /// A hand-edited row can't steer a path: its id, version and file paths go back through the
    /// grammar, and it parks.
    #[test]
    fn a_row_that_cant_be_read_back_parks_and_names_no_path() {
        let w = world();
        w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
            .unwrap();
        let outside = w.dir.path().join("escape.txt");
        fs::write(&outside, b"x").unwrap();
        for edit in [
            "UPDATE mods SET version = '../../..'",
            "UPDATE mods SET files = '[{\"path\":\"../../../escape.txt\",\"sha256\":\"00\",\"size\":1}]'",
            "UPDATE mods SET files = 'nope'",
            "UPDATE mods SET manifest = '{}'",
        ] {
            let w2 = world();
            w2.install(&pkg("1.0.0", |_| {}), Approval::Approved).unwrap();
            w2.exec(edit);
            let report = w2.ledger().startup(now(), &serve_all);
            assert_eq!(report.parked, vec![("atlas.map".to_owned(), store::REASON_RECORD)], "{edit}");
            assert!(report.loaded.is_empty());
        }
        // A newer API than this PM's parks too (a store written by a newer PM's install).
        let mut manifest: Value =
            serde_json::from_str(&w.sql::<String>("SELECT manifest FROM mods")).unwrap();
        manifest["apiVersion"] = serde_json::json!({"min": "1.9", "target": "1.9"});
        w.store
            .conn
            .lock()
            .unwrap()
            .execute("UPDATE mods SET manifest = ?1", [manifest.to_string()])
            .unwrap();
        assert_eq!(
            w.ledger().startup(now(), &serve_all).parked,
            vec![("atlas.map".to_owned(), store::REASON_API)]
        );
        assert!(outside.exists());
    }

    #[test]
    fn startup_sweeps_from_the_rows_it_read() {
        let w = world();
        w.install(&pkg("1.1.0", |_| {}), Approval::Approved)
            .unwrap();
        let stray = w.layout.store_dir().join("acme.gone/1.0.0");
        fs::create_dir_all(&stray).unwrap();
        fs::create_dir_all(w.version_dir("1.0.0")).unwrap();
        fs::create_dir_all(w.layout.staging().join("left-over")).unwrap();
        let mut report = w.ledger().startup(now(), &serve_all);
        report.sweep.quarantined.sort();
        // A version folder the row doesn't name is quarantined like a mod no row names: the boot
        // sweep deletes nothing on sight (only an update's own prune does, after its commit).
        assert_eq!(report.sweep.quarantined, ["acme.gone", "atlas.map/1.0.0"]);
        assert_eq!(report.sweep.staging_cleared, 0, "staging waits for expire");
        assert!(w.version_dir("1.1.0").exists());
        for kept in [
            "20261105T120000Z-acme.gone/1.0.0",
            "20261105T120000Z-atlas.map/1.0.0",
        ] {
            assert!(w.layout.quarantine().join(kept).exists(), "{kept}");
        }
        let report = w.ledger().expire(now()).unwrap();
        assert_eq!(report.staging_cleared, 1);
        assert!(report.expired.is_empty());
        // Thirty days on, the quarantine is emptied.
        let later = now() + chrono::Duration::days(30);
        let mut expired = w.ledger().expire(later).unwrap().expired;
        expired.sort();
        assert_eq!(
            expired,
            ["20261105T120000Z-acme.gone", "20261105T120000Z-atlas.map"]
        );
    }

    /// The rule the whole sweep rests on: a ledger that can't be read deletes, moves and loads
    /// nothing.
    #[test]
    fn a_ledger_that_cant_be_read_sweeps_nothing_and_serves_nothing() {
        let w = world();
        w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
            .unwrap();
        fs::create_dir_all(w.version_dir("0.9.0")).unwrap();
        fs::create_dir_all(w.layout.staging().join("left-over")).unwrap();
        let before = w.installed_tree();
        w.store.closed.store(true, Ordering::SeqCst);
        let report = w.ledger().startup(now(), &serve_all);
        assert!(report.skipped.is_some());
        assert!(report.sweep.is_empty());
        assert!(w.ledger().expire(now()).is_err(), "nor its deletions");
        assert!(
            w.ledger().recover_missing(now(), &serve_all).is_err(),
            "nor its copies"
        );
        assert_eq!(w.installed_tree(), before);
        assert!(w.layout.staging().join("left-over").exists());
        assert_eq!(w.served_from(), None);
        // A table that can't be read is the same as a closed store.
        w.store.closed.store(false, Ordering::SeqCst);
        w.exec("ALTER TABLE mods RENAME TO mods_gone");
        assert!(w.ledger().startup(now(), &serve_all).skipped.is_some());
        assert_eq!(w.installed_tree(), before);
    }
}
