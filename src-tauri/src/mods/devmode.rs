// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Developer mode, per device and per vault (Mods spec §1.15, decided 2026-11-12): "on for this
//! device in this vault", not a vault-wide switch. It gates the one thing that matters, unsigned
//! mods (an unsigned file installs only while it is on, and an unsigned mod runs only while it is
//! on), so it has to be something a restore onto another machine, a joined shared vault or a copied
//! database can never carry in: the record lists the devices that switched it on
//! ([`store::SETTING_DEV_DEVICES`]), and a device that isn't in the list is off whatever else the
//! database says.
//!
//! **The list holds [`credential`]s, never device ids.** A device is named by
//! [`super::tab::device::device_id`], and that id is kept out of every row (the file in `data_dir`
//! is its only home): the list holds a domain-separated one-way value of it. Anyone who can write
//! the store (a hostile member of a shared vault, a crafted copy) can read every row, so a raw id
//! sitting in any of them (the disclaimer's list used to have one) would let them write a
//! "developer mode on" entry for someone else's device and open the unsigned-install gate. The
//! disclaimer's list holds a value of a different domain, so nothing read from it works here. A
//! record in the old raw form names nobody (it reads as off).
//!
//! **Only Rust reads it, and only this record.** The old boolean row (`dev_mode`) was a webview
//! preference, which is why it can't be believed: it is no longer in the page's allow-list, and the
//! answer the page shows is the `dev_mode_state` command's, from this record.
//!
//! **The old row is never read, by anything.** There is no carry-over: a vault whose record is
//! missing, or whose old row says `"true"`, is off for every device. (A carry-over once handed an
//! old "on" to the first device that asked. It could be re-armed by anyone who holds the store key,
//! a member of a shared vault, by deleting the record and writing the old row, which switched the
//! victim's developer mode on without a click; so nothing but a record that lists the device ever
//! switches it on.) The one place Rust names the old row is [`all_devices_off`], which WRITES it
//! `"false"` so an older PM sharing the vault also starts off; the tripwire test in
//! `backup::adopt_mods` fails on any other mention, and on any read here. [`is_on`] writes only
//! toward off (the device's own record, below), which never changes an answer, so merely asking can
//! never switch anything on. Upgrading from a PM that carried developer mode over turns it off
//! once; each person switches it on again in Settings, under Developer.
//!
//! **The device remembers too.** A credential in the store's record is a bearer value: nothing the
//! device alone holds stands behind it (a one-way hash of the id, no secret key), so whoever can
//! write the store and has ever read it (it sits in the record while the device is on; an old copy
//! of the store file keeps it) could write it back after the device was switched off or reset. So
//! the store's record is only half the answer. The other half is [`DEVICE_FILE`]
//! (`data_dir/mods-dev-mode.json`, versioned, the ids of the vaults this device switched on): developer
//! mode is on for this device in this vault **only if both say so**, the store's record lists this
//! device's credential AND this device's file lists this vault ([`Here`]). A copied database, a
//! restore, a joined vault or another machine has no device file entry, so it is off there.
//! - **Order.** Switching on writes the store's record first and the device's file second; switching
//!   off writes the device's file first and the store's record second. A crash between the two
//!   leaves it off either way.
//! - **The store can only ever turn it off.** Whenever a read finds that the store's record does not
//!   list this device (a record without it, or no record), this vault is cleared from the device's
//!   file (best effort: a failed clear still answers off). So a restore or a reset that removed the
//!   device from the record can't be undone by someone writing an old credential back later: the
//!   device had already forgotten. The restore reset ([`all_devices_off`]) clears it directly, and
//!   fails if it can't.
//! - **A bad device file is off.** A device file that is missing, unreadable, not what PM writes, over the
//!   size cap ([`MAX_FILE_BYTES`]) or over the entry cap ([`MAX_VAULTS`]) is off for every vault; a
//!   write caps it (the oldest switch-on falls off first, and that vault is then simply off).
//!
//! What this holds: a store writer, however much they have read or kept, can never switch this
//! device on, because the device must also say so, and the store can only ever clear the device's
//! entry. What it does not: someone who can write this computer's `data_dir` can write the device
//! file and the device id file as they like, as with every other file kept there (out of scope; the
//! person owns that folder). Upgrading from a build that had developer mode on turns it off once
//! more: there is no device file yet, so each person switches it on again in Settings > Developer.
//!
//! **Fails toward off.** A record that isn't what PM writes reads as nobody being on; a device
//! whose id can't be read is off; the list is capped (the oldest switch-on falls off first, and that
//! device is then simply off).
//!
//! The pure rules are here; the ledger carries the switch out against the mods
//! (`ledger/dev_switch.rs`: pause every unsigned mod, or resume exactly those it paused).

use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::store;
use super::tab::device;

/// Most devices one vault remembers.
pub const MAX_DEVICES: usize = 32;

const FORMAT: u32 = 1;

/// The domain this record's [`credential`]s are made under (the disclaimer's differs).
const CREDENTIAL_DOMAIN: &str = "pm-mods-dev-mode-v1";

/// What the record holds for `device` in place of its id (module doc).
pub fn credential(device: &str) -> String {
    device::credential(CREDENTIAL_DOMAIN, device)
}

/// The pre-per-device boolean row (`"true"`/`"false"`), once webview-written. **Never read, never an
/// authority.** The restore reset writes it `"false"` ([`all_devices_off`]) for the sake of an older
/// PM sharing the vault, and that is the only use. **This is the only place in Rust that names it**,
/// so nothing can gate on a vault-wide switch by accident; it is private on purpose, and
/// `adopt_mods`'s tripwire test fails on any other mention and on any read here.
const LEGACY_ROW: &str = "dev_mode";

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    v: u32,
    devices: Vec<String>,
}

/// The entries a stored record names, or `None` when it isn't a record PM wrote (pure).
fn devices_of(stored: &str) -> Option<Vec<String>> {
    serde_json::from_str::<Record>(stored)
        .ok()
        .filter(|r| r.v == FORMAT)
        .map(|r| r.devices)
}

/// Whether the device whose [`credential`] this is, is on, by the stored record (pure). No record,
/// or one that isn't ours, is off.
pub fn on_by(stored: Option<&str>, credential: &str) -> bool {
    stored
        .and_then(devices_of)
        .is_some_and(|d| d.iter().any(|x| x == credential))
}

/// The record with the device whose [`credential`] this is switched on or off (pure): once in the
/// list at most, newest last, no longer than [`MAX_DEVICES`]. A record that isn't ours is replaced.
pub fn with_device(stored: Option<&str>, credential: &str, on: bool) -> String {
    let mut devices = stored.and_then(devices_of).unwrap_or_default();
    devices.retain(|d| d != credential);
    if on {
        devices.push(credential.to_owned());
    }
    let over = devices.len().saturating_sub(MAX_DEVICES);
    devices.drain(..over);
    serde_json::to_string(&Record { v: FORMAT, devices }).unwrap_or_default()
}

/// The record that switches every device off.
fn nobody() -> String {
    with_device(None, "", false)
}

fn read(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
        r.get(0)
    })
    .optional()
}

/// The device's file in `data_dir`: the vaults this device switched developer mode on in.
pub const DEVICE_FILE: &str = "mods-dev-mode.json";

/// Most vaults the device file remembers (the oldest switch-on falls off first). A file that lists
/// more is not one PM wrote and reads as off for every vault.
pub const MAX_VAULTS: usize = 64;

/// The most the device file may weigh (read and written). A larger file is off for every vault.
pub const MAX_FILE_BYTES: u64 = 16 * 1024;

/// The longest vault id the device file keeps; a longer entry makes the file not ours.
const MAX_VAULT_ID_LEN: usize = 128;

/// Reading and rewriting the device file are one step at a time (the store's connection already
/// orders the callers that matter; this keeps two threads' rewrites from losing one another's).
static DEVICE_FILE_LOCK: Mutex<()> = Mutex::new(());

/// This device in this vault: who is asking, which vault's answer, and where the device keeps its
/// half of the record. Built where the open vault and `data_dir` are known (the commands), and
/// passed down to everything that asks or switches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Here {
    id: String,
    vault: String,
    dir: PathBuf,
}

impl Here {
    /// `id`: this device's id ([`device::device_id`]); `vault`: the open vault's id; `dir`: `data_dir`.
    pub fn new(id: &str, vault: &str, dir: &Path) -> Self {
        Self {
            id: id.to_owned(),
            vault: vault.to_owned(),
            dir: dir.to_owned(),
        }
    }

    /// This device in `vault`, naming the device by the file in `data_dir` (minted on first use).
    pub fn open(data_dir: &Path, vault: &str) -> io::Result<Self> {
        Ok(Self::new(&device::device_id(data_dir)?, vault, data_dir))
    }

    /// Where this device keeps its record (`data_dir`).
    #[cfg(test)]
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// The vault this is the device in.
    pub fn vault(&self) -> &str {
        &self.vault
    }

    fn credential(&self) -> String {
        credential(&self.id)
    }
}

/// Why a switch didn't happen.
#[derive(Debug)]
pub enum Error {
    /// The store couldn't be read or written.
    Db(rusqlite::Error),
    /// The device's file couldn't be read or written.
    Disk(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Db(e) => write!(f, "{e}"),
            Self::Disk(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e)
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Disk(e)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceRecord {
    v: u32,
    vaults: Vec<String>,
}

/// The vaults a device file names, or `None` when it isn't a file PM wrote: wrong shape or version,
/// more than [`MAX_VAULTS`] entries, an entry that is empty or over-long (pure).
fn vaults_of(text: &str) -> Option<Vec<String>> {
    let r: DeviceRecord = serde_json::from_str(text).ok()?;
    let sane = r.v == FORMAT
        && r.vaults.len() <= MAX_VAULTS
        && r.vaults
            .iter()
            .all(|v| !v.is_empty() && v.len() <= MAX_VAULT_ID_LEN);
    sane.then_some(r.vaults)
}

/// The device file's text with `vault` switched on or off (pure): once in the list at most, newest
/// last, no longer than [`MAX_VAULTS`].
fn with_vault(mut vaults: Vec<String>, vault: &str, on: bool) -> String {
    vaults.retain(|v| v != vault);
    if on {
        vaults.push(vault.to_owned());
    }
    let over = vaults.len().saturating_sub(MAX_VAULTS);
    vaults.drain(..over);
    serde_json::to_string(&DeviceRecord { v: FORMAT, vaults }).unwrap_or_default()
}

/// The vaults the device file lists. A missing file, or one that isn't what PM writes (corrupt,
/// over a cap), lists none; only a file that can't be *read* is an error.
fn device_vaults(dir: &Path) -> io::Result<Vec<String>> {
    let file = match File::open(dir.join(DEVICE_FILE)) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Ok(Vec::new());
    }
    Ok(std::str::from_utf8(&bytes)
        .ok()
        .and_then(vaults_of)
        .unwrap_or_default())
}

/// Switch `vault` on or off in the device file. Off in a vault the file doesn't list writes nothing.
/// On replaces a file that isn't ours (the other vaults then read as off).
fn set_device(dir: &Path, vault: &str, on: bool) -> io::Result<()> {
    let _guard = DEVICE_FILE_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let vaults = device_vaults(dir)?;
    if !on && !vaults.iter().any(|v| v == vault) {
        return Ok(());
    }
    let text = with_vault(vaults, vault, on);
    crate::vault::write_atomic(&dir.join(DEVICE_FILE), text.as_bytes())
}

/// Whether the device's file lists `vault`. Any failure is off.
fn device_says_on(dir: &Path, vault: &str) -> bool {
    let _guard = DEVICE_FILE_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    device_vaults(dir).is_ok_and(|vaults| vaults.iter().any(|v| v == vault))
}

/// The store's record in `conn`, written with `credential` switched on or off, in one transaction.
fn write_store(conn: &mut Connection, credential: &str, on: bool) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    let next = with_device(
        read(&tx, store::SETTING_DEV_DEVICES)?.as_deref(),
        credential,
        on,
    );
    tx.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![store::SETTING_DEV_DEVICES, next],
    )?;
    tx.commit()
}

/// Whether developer mode is on for `here` (this device in this vault): only when the store's record
/// lists this device's [`credential`] AND the device's own file lists this vault (module doc). Either
/// missing, unreadable, corrupt or capped out is off. Writes only toward off: a store whose record
/// doesn't list the device clears this vault from the device's file (best effort), which never
/// changes the answer, so asking can never switch anything on.
pub fn is_on(conn: &Connection, here: &Here) -> rusqlite::Result<bool> {
    let listed = on_by(
        read(conn, store::SETTING_DEV_DEVICES)?.as_deref(),
        &here.credential(),
    );
    if !listed {
        if let Err(e) = set_device(&here.dir, &here.vault, false) {
            eprintln!("mods: the device's developer-mode record couldn't be cleared ({e})");
        }
        return Ok(false);
    }
    Ok(device_says_on(&here.dir, &here.vault))
}

/// Switch developer mode on or off for `here` in this vault, and say what it is now. Other devices'
/// entries are untouched. On writes the store's record, then the device's file; off writes the
/// device's file, then the store's record (a crash between the two leaves it off). A step that fails
/// stops the switch with an error; neither order can leave it on. This writes the records only: the
/// ledger's `set_dev_mode_for_device` is the one function that also pauses or resumes the unsigned
/// mods.
pub fn write(conn: &mut Connection, here: &Here, on: bool) -> Result<bool, Error> {
    let credential = here.credential();
    if on {
        write_store(conn, &credential, true)?;
        set_device(&here.dir, &here.vault, true)?;
    } else {
        set_device(&here.dir, &here.vault, false)?;
        write_store(conn, &credential, false)?;
    }
    Ok(on)
}

/// Switch developer mode off for every device in this vault (the restore reset: §1.11 says a restore
/// never enables unsigned installs on another machine). Forgets this vault in this device's file
/// (`dir` is `data_dir`; a restore keeps its vault id, so the file would otherwise still say on),
/// then writes an empty record, and turns the old row off if it is there (an older PM sharing the
/// vault then starts off too; this PM never reads it). The mods it affects are paused by the load
/// that follows the store's open (nothing is paused here: the load reads the record first and
/// pauses an active unsigned row with the reason `dev-mode`).
///
/// Called by the restore commit (`backup::adopt_mods::reset_for_adopt`, inside its transaction),
/// and by nothing else: a shared-vault join writes nothing. A device file that can't be cleared is
/// an error, so the restore is refused.
pub fn all_devices_off(conn: &Connection, dir: &Path, vault: &str) -> Result<(), Error> {
    set_device(dir, vault, false)?;
    conn.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![store::SETTING_DEV_DEVICES, nobody()],
    )?;
    conn.execute(
        "UPDATE settings SET value = 'false' WHERE key = ?1",
        [LEGACY_ROW],
    )?;
    Ok(())
}

/// The vault every ledger test's world is for.
#[cfg(test)]
pub(crate) const TEST_VAULT: &str = "11111111-2222-3333-4444-555555555555";

/// A device named `id` in [`TEST_VAULT`] with a data folder of its own, for tests that drive the
/// ledger. Every test runs on its own thread, so each test gets its own folder (dropped with the
/// thread) and no test's device file can reach another's; asking twice in one test gives the same
/// device. The reference is leaked (a few bytes per test) so options can hold it.
#[cfg(test)]
pub(crate) fn test_here(id: &str) -> &'static Here {
    use std::cell::RefCell;
    use std::collections::HashMap;
    thread_local! {
        static DIR: tempfile::TempDir = tempfile::tempdir().unwrap();
        static HERES: RefCell<HashMap<String, &'static Here>> = RefCell::new(HashMap::new());
    }
    HERES.with(|m| {
        *m.borrow_mut().entry(id.to_owned()).or_insert_with(|| {
            let dir = DIR.with(|d| d.path().join(id));
            std::fs::create_dir_all(&dir).unwrap();
            Box::leak(Box::new(Here::new(id, TEST_VAULT, &dir)))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn store() -> Connection {
        let dir = tempfile::tempdir().unwrap();
        crate::db::open(&dir.path().join("pm.sqlite"), &"00".repeat(32)).unwrap()
    }

    fn set_raw(conn: &Connection, key: &str, value: &str) {
        conn.execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
        .unwrap();
    }

    #[test]
    fn a_device_is_on_only_when_the_record_names_it() {
        assert!(!on_by(None, A));
        let one = with_device(None, A, true);
        assert!(on_by(Some(&one), A));
        // Another device of the same vault, or a copy of it on another machine: off.
        assert!(!on_by(Some(&one), B));
        let two = with_device(Some(&one), B, true);
        assert!(on_by(Some(&two), A) && on_by(Some(&two), B));
        // Switching one off leaves the other as it was.
        let back = with_device(Some(&two), A, false);
        assert!(!on_by(Some(&back), A) && on_by(Some(&back), B));
    }

    #[test]
    fn switching_on_twice_lists_the_device_once() {
        let once = with_device(None, A, true);
        let twice = with_device(Some(&once), A, true);
        assert_eq!(devices_of(&twice).unwrap(), [A]);
    }

    #[test]
    fn a_record_that_isnt_ours_reads_as_nobody_and_is_replaced_by_the_next_write() {
        for junk in [
            "",
            "not json",
            "true",
            "[]",
            r#"{"v":2,"devices":["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]}"#,
            r#"{"v":1,"devices":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
            r#"{"v":1,"devices":[],"extra":1}"#,
        ] {
            assert!(!on_by(Some(junk), A), "{junk}");
            let fixed = with_device(Some(junk), B, true);
            assert_eq!(devices_of(&fixed).unwrap(), [B], "{junk}");
        }
    }

    #[test]
    fn the_list_is_capped_and_the_oldest_switch_on_falls_off_first() {
        let ids: Vec<String> = (0..MAX_DEVICES + 3).map(|i| format!("{i:032x}")).collect();
        let mut stored = None;
        for id in &ids {
            stored = Some(with_device(stored.as_deref(), id, true));
        }
        let kept = devices_of(stored.as_deref().unwrap()).unwrap();
        assert_eq!(kept.len(), MAX_DEVICES);
        assert_eq!(kept.first(), Some(&ids[3]));
        assert!(!on_by(stored.as_deref(), &ids[0]), "the oldest is off");
        assert!(on_by(stored.as_deref(), ids.last().unwrap()));
    }

    /// Every row of the settings table, in key order (what "left alone" is measured against).
    fn all_rows(conn: &Connection) -> Vec<(String, String)> {
        conn.prepare("SELECT key, value FROM settings ORDER BY key")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn no_record_and_an_old_on_row_is_off_for_this_device_and_for_any_other() {
        // The old row is never read: a vault that has only it, "true", is off for everyone.
        let conn = store();
        set_raw(&conn, LEGACY_ROW, "true");
        assert_eq!(read(&conn, store::SETTING_DEV_DEVICES).unwrap(), None);
        for device in [A, B, "cccccccccccccccccccccccccccccccc"] {
            assert!(!is_on(&conn, test_here(device)).unwrap(), "{device}");
        }
    }

    #[test]
    fn a_deleted_record_and_an_old_on_row_do_not_switch_a_device_on_again() {
        // The attack: a member of a shared vault holds the store key. A device was on; they delete
        // the record row and write the old row "true". Asking (any number of times, for any
        // device) must not hand the "on" back.
        let mut conn = store();
        assert!(write(&mut conn, test_here(A), true).unwrap());
        assert!(is_on(&conn, test_here(A)).unwrap());
        conn.execute(
            "DELETE FROM settings WHERE key = ?1",
            [store::SETTING_DEV_DEVICES],
        )
        .unwrap();
        set_raw(&conn, LEGACY_ROW, "true");
        for _ in 0..2 {
            assert!(!is_on(&conn, test_here(A)).unwrap());
            assert!(!is_on(&conn, test_here(B)).unwrap());
        }
        // Writing the old row again after the asks changes nothing either.
        set_raw(&conn, LEGACY_ROW, "false");
        set_raw(&conn, LEGACY_ROW, "true");
        assert!(!is_on(&conn, test_here(A)).unwrap());
    }

    #[test]
    fn a_record_that_exists_is_the_only_thing_that_says_on_whatever_the_old_row_says() {
        let mut conn = store();
        write(&mut conn, test_here(B), true).unwrap();
        for legacy in ["true", "false", "TRUE", ""] {
            set_raw(&conn, LEGACY_ROW, legacy);
            assert!(is_on(&conn, test_here(B)).unwrap(), "{legacy:?}");
            assert!(!is_on(&conn, test_here(A)).unwrap(), "{legacy:?}");
        }
    }

    #[test]
    fn asking_never_writes() {
        // No record, a record, a junk record, with and without an old row: the settings table is
        // byte for byte what it was after any number of reads.
        for setup in 0..4 {
            let mut conn = store();
            match setup {
                0 => {}
                1 => set_raw(&conn, LEGACY_ROW, "true"),
                2 => {
                    write(&mut conn, test_here(B), true).unwrap();
                    set_raw(&conn, LEGACY_ROW, "true");
                }
                _ => {
                    set_raw(&conn, store::SETTING_DEV_DEVICES, "garbage");
                    set_raw(&conn, LEGACY_ROW, "true");
                }
            }
            let before = all_rows(&conn);
            for device in [A, B] {
                is_on(&conn, test_here(device)).unwrap();
            }
            assert_eq!(all_rows(&conn), before, "setup {setup}");
        }
    }

    #[test]
    fn a_vault_with_nothing_starts_off_for_everyone_and_stays_unwritten() {
        let conn = store();
        assert!(!is_on(&conn, test_here(A)).unwrap());
        assert!(!is_on(&conn, test_here(B)).unwrap());
        assert_eq!(read(&conn, store::SETTING_DEV_DEVICES).unwrap(), None);
    }

    #[test]
    fn a_junk_record_is_off_even_with_an_old_on_row() {
        let conn = store();
        set_raw(&conn, LEGACY_ROW, "true");
        set_raw(&conn, store::SETTING_DEV_DEVICES, "garbage");
        assert!(!is_on(&conn, test_here(A)).unwrap());
    }

    #[test]
    fn writing_changes_this_device_only_and_says_what_it_is_now() {
        let mut conn = store();
        assert!(write(&mut conn, test_here(A), true).unwrap());
        assert!(write(&mut conn, test_here(B), true).unwrap());
        assert!(!write(&mut conn, test_here(A), false).unwrap());
        assert!(!is_on(&conn, test_here(A)).unwrap());
        assert!(is_on(&conn, test_here(B)).unwrap(), "B's switch is its own");
        // Off again is a no-op that still says off.
        assert!(!write(&mut conn, test_here(A), false).unwrap());
    }

    #[test]
    fn a_write_into_a_vault_with_an_old_on_row_switches_only_that_device() {
        let mut conn = store();
        set_raw(&conn, LEGACY_ROW, "true");
        // B switches on: A (who never asked) is not on, and the old row is left as it was.
        assert!(write(&mut conn, test_here(B), true).unwrap());
        assert!(!is_on(&conn, test_here(A)).unwrap());
        assert_eq!(read(&conn, LEGACY_ROW).unwrap().as_deref(), Some("true"));
    }

    #[test]
    fn the_restore_reset_turns_every_device_and_the_old_row_off_and_stays_off() {
        let mut conn = store();
        set_raw(&conn, LEGACY_ROW, "true");
        write(&mut conn, test_here(A), true).unwrap();
        write(&mut conn, test_here(B), true).unwrap();
        all_devices_off(&conn, test_here(A).dir(), TEST_VAULT).unwrap();
        assert!(!is_on(&conn, test_here(A)).unwrap() && !is_on(&conn, test_here(B)).unwrap());
        assert_eq!(read(&conn, LEGACY_ROW).unwrap().as_deref(), Some("false"));
        // The empty record is there, and an old "true" that comes back is still never read.
        let stored = read(&conn, store::SETTING_DEV_DEVICES).unwrap();
        assert_eq!(
            devices_of(stored.as_deref().expect("the empty record")).unwrap(),
            Vec::<String>::new()
        );
        set_raw(&conn, LEGACY_ROW, "true");
        assert!(!is_on(&conn, test_here(A)).unwrap());
    }

    #[test]
    fn a_device_id_in_either_record_in_its_raw_form_is_not_a_switch() {
        // What a hostile member can do: read the id from the disclaimer's list and write it into
        // the developer-mode record. Both records name the RAW id here.
        let conn = store();
        let raw = format!(r#"{{"v":1,"devices":["{A}"]}}"#);
        set_raw(&conn, store::SETTING_DISCLAIMER, &raw);
        set_raw(&conn, store::SETTING_DEV_DEVICES, &raw);
        assert!(!is_on(&conn, test_here(A)).unwrap());
        assert!(!crate::mods::tab::disclaimer::is_accepted(&conn, A).unwrap());
    }

    #[test]
    fn no_row_of_the_store_holds_the_raw_device_id_and_one_records_value_is_not_anothers() {
        let mut conn = store();
        crate::mods::tab::disclaimer::record(&conn, A).unwrap();
        assert!(write(&mut conn, test_here(A), true).unwrap());
        let rows: Vec<String> = conn
            .prepare("SELECT key || '=' || value FROM settings")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(rows.iter().any(|r| r.contains(store::SETTING_DISCLAIMER)));
        assert!(
            rows.iter().all(|r| !r.contains(A)),
            "the raw id is in a row: {rows:?}"
        );
        // The value the disclaimer keeps for this device, written into the developer-mode record
        // for another device's switch, switches nothing on.
        let theirs = read(&conn, store::SETTING_DISCLAIMER).unwrap().unwrap();
        assert_ne!(
            theirs,
            read(&conn, store::SETTING_DEV_DEVICES).unwrap().unwrap()
        );
        let fresh = store();
        set_raw(&fresh, store::SETTING_DEV_DEVICES, &theirs);
        assert!(!is_on(&fresh, test_here(A)).unwrap());
    }

    #[test]
    fn the_restore_reset_on_a_vault_with_no_record_writes_the_empty_record_and_the_old_row_off() {
        let conn = store();
        set_raw(&conn, LEGACY_ROW, "true");
        all_devices_off(&conn, test_here(A).dir(), TEST_VAULT).unwrap();
        assert!(!is_on(&conn, test_here(A)).unwrap());
        assert_eq!(read(&conn, LEGACY_ROW).unwrap().as_deref(), Some("false"));
        assert!(read(&conn, store::SETTING_DEV_DEVICES).unwrap().is_some());
        // And it didn't invent an old row where there was none.
        let bare = store();
        all_devices_off(&bare, test_here(A).dir(), TEST_VAULT).unwrap();
        assert_eq!(read(&bare, LEGACY_ROW).unwrap(), None);
    }

    // ---- the device's own record (module doc: "The device remembers too") -------------------------

    const VAULT: &str = "vault-1";

    fn here_in(dir: &Path, id: &str) -> Here {
        Here::new(id, VAULT, dir)
    }

    fn record_row(conn: &Connection) -> Option<String> {
        read(conn, store::SETTING_DEV_DEVICES).unwrap()
    }

    fn device_file(dir: &Path) -> Option<String> {
        std::fs::read_to_string(dir.join(DEVICE_FILE)).ok()
    }

    fn lists_vault(dir: &Path, vault: &str) -> bool {
        device_vaults(dir).unwrap().iter().any(|v| v == vault)
    }

    /// A device folder and a store of one vault.
    fn rig() -> (tempfile::TempDir, Connection) {
        (tempfile::tempdir().unwrap(), store())
    }

    #[test]
    fn a_credential_written_back_after_the_device_switched_off_is_not_a_switch() {
        // The replay the store's record alone can't stop: whoever kept the bytes of the record
        // while the device was on writes them back after the device switched off.
        let (dir, mut conn) = rig();
        let me = here_in(dir.path(), A);
        assert!(write(&mut conn, &me, true).unwrap());
        assert!(is_on(&conn, &me).unwrap());
        let kept = record_row(&conn).unwrap();
        assert!(!write(&mut conn, &me, false).unwrap());
        assert!(!is_on(&conn, &me).unwrap());
        set_raw(&conn, store::SETTING_DEV_DEVICES, &kept);
        assert!(!is_on(&conn, &me).unwrap(), "the replayed credential");
        // Again, and after other asks: still off, and the device never remembered it was on.
        assert!(!is_on(&conn, &me).unwrap());
        assert!(!lists_vault(dir.path(), VAULT));
    }

    #[test]
    fn a_credential_written_back_after_a_restore_is_not_a_switch() {
        let (dir, mut conn) = rig();
        let me = here_in(dir.path(), A);
        write(&mut conn, &me, true).unwrap();
        let kept = record_row(&conn).unwrap();
        // The restore's reset: it ran before anything asked, so the asks can't be what cleared the
        // device's entry.
        all_devices_off(&conn, dir.path(), VAULT).unwrap();
        assert!(!lists_vault(dir.path(), VAULT), "the restore cleared it");
        set_raw(&conn, store::SETTING_DEV_DEVICES, &kept);
        assert!(!is_on(&conn, &me).unwrap());
    }

    #[test]
    fn a_credential_written_back_after_the_record_was_deleted_is_not_a_switch() {
        // Another member of a shared vault deletes the record (or a reset empties it) and, after
        // the device has looked once, writes the kept credential back.
        let (dir, mut conn) = rig();
        let me = here_in(dir.path(), A);
        write(&mut conn, &me, true).unwrap();
        let kept = record_row(&conn).unwrap();
        for gone in [None, Some(nobody()), Some("garbage".to_owned())] {
            write(&mut conn, &me, true).unwrap();
            match &gone {
                None => {
                    conn.execute(
                        "DELETE FROM settings WHERE key = ?1",
                        [store::SETTING_DEV_DEVICES],
                    )
                    .unwrap();
                }
                Some(text) => set_raw(&conn, store::SETTING_DEV_DEVICES, text),
            }
            assert!(!is_on(&conn, &me).unwrap(), "{gone:?}");
            set_raw(&conn, store::SETTING_DEV_DEVICES, &kept);
            assert!(!is_on(&conn, &me).unwrap(), "replayed after {gone:?}");
        }
    }

    #[test]
    fn the_store_listing_the_device_is_not_enough_without_the_devices_own_record() {
        let (dir, conn) = rig();
        let me = here_in(dir.path(), A);
        set_raw(
            &conn,
            store::SETTING_DEV_DEVICES,
            &with_device(None, &me.credential(), true),
        );
        assert!(!is_on(&conn, &me).unwrap());
        // A copied database, a restore or a joined vault on another machine: the same store, a
        // device folder that has never heard of it.
        let other_machine = tempfile::tempdir().unwrap();
        assert!(!is_on(&conn, &here_in(other_machine.path(), A)).unwrap());
        assert_eq!(
            device_file(other_machine.path()),
            None,
            "nothing is invented"
        );
    }

    #[test]
    fn the_devices_record_is_not_enough_without_the_store_listing_the_device_and_is_cleared() {
        let (dir, mut conn) = rig();
        let me = here_in(dir.path(), A);
        write(&mut conn, &me, true).unwrap();
        // The store's record loses the device (a reset, a member's edit): a successfully read
        // record without it, and no record at all.
        for store_says in [Some(with_device(None, &credential(B), true)), None] {
            write(&mut conn, &me, true).unwrap();
            assert!(lists_vault(dir.path(), VAULT));
            match &store_says {
                Some(text) => set_raw(&conn, store::SETTING_DEV_DEVICES, text),
                None => {
                    conn.execute(
                        "DELETE FROM settings WHERE key = ?1",
                        [store::SETTING_DEV_DEVICES],
                    )
                    .unwrap();
                }
            }
            assert!(!is_on(&conn, &me).unwrap(), "{store_says:?}");
            assert!(
                !lists_vault(dir.path(), VAULT),
                "the device forgot it: {store_says:?}"
            );
        }
    }

    #[test]
    fn on_needs_both_halves() {
        let (dir, mut conn) = rig();
        let me = here_in(dir.path(), A);
        let listed = with_device(None, &me.credential(), true);
        for (store_lists, device_lists) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            conn.execute(
                "DELETE FROM settings WHERE key = ?1",
                [store::SETTING_DEV_DEVICES],
            )
            .unwrap();
            if store_lists {
                set_raw(&conn, store::SETTING_DEV_DEVICES, &listed);
            }
            let _ = std::fs::remove_file(dir.path().join(DEVICE_FILE));
            if device_lists {
                set_device(dir.path(), VAULT, true).unwrap();
            }
            assert_eq!(
                is_on(&conn, &me).unwrap(),
                store_lists && device_lists,
                "store {store_lists}, device {device_lists}"
            );
        }
        // And a write that leaves both halves reads on.
        assert!(write(&mut conn, &me, true).unwrap());
        assert!(is_on(&conn, &me).unwrap());
    }

    #[test]
    fn a_device_file_that_is_corrupt_oversized_or_over_the_entry_cap_is_off() {
        let (dir, mut conn) = rig();
        let me = here_in(dir.path(), A);
        write(&mut conn, &me, true).unwrap();
        assert!(is_on(&conn, &me).unwrap());
        let listing = format!(r#"{{"v":1,"vaults":["{VAULT}"]}}"#);
        let crowd: Vec<String> = (0..=MAX_VAULTS).map(|i| format!("v{i}")).collect();
        let too_many = format!(
            r#"{{"v":1,"vaults":["{VAULT}",{}]}}"#,
            crowd
                .iter()
                .map(|v| format!("\"{v}\""))
                .collect::<Vec<_>>()
                .join(",")
        );
        let long_id = format!(r#"{{"v":1,"vaults":["{VAULT}","{}"]}}"#, "x".repeat(200));
        let padded = format!("{listing}{}", " ".repeat(MAX_FILE_BYTES as usize));
        for (what, bad) in [
            ("empty", String::new()),
            ("not json", "garbage".to_owned()),
            ("another version", listing.replace("\"v\":1", "\"v\":2")),
            ("an extra field", listing.replace('}', r#","extra":1}"#)),
            ("not a list", format!(r#"{{"v":1,"vaults":"{VAULT}"}}"#)),
            ("too many entries", too_many),
            ("an over-long id", long_id),
            (
                "an empty id",
                format!(r#"{{"v":1,"vaults":["{VAULT}",""]}}"#),
            ),
            ("over the size cap", padded),
        ] {
            std::fs::write(dir.path().join(DEVICE_FILE), &bad).unwrap();
            assert!(!is_on(&conn, &me).unwrap(), "{what}");
        }
        // The same file at the cap is fine: only a bigger one is refused.
        let at_cap = format!(
            "{listing}{}",
            " ".repeat(MAX_FILE_BYTES as usize - listing.len())
        );
        std::fs::write(dir.path().join(DEVICE_FILE), &at_cap).unwrap();
        assert!(is_on(&conn, &me).unwrap());
        // Switching on replaces a file that isn't ours.
        std::fs::write(dir.path().join(DEVICE_FILE), "garbage").unwrap();
        assert!(write(&mut conn, &me, true).unwrap());
        assert!(is_on(&conn, &me).unwrap());
    }

    #[test]
    fn a_device_file_that_cannot_be_read_is_off_and_a_switch_that_needs_it_fails() {
        let (dir, mut conn) = rig();
        let me = here_in(dir.path(), A);
        write(&mut conn, &me, true).unwrap();
        // A folder where the file belongs: opening works on Linux, reading fails; either way it is
        // an error, not "nothing listed".
        std::fs::remove_file(dir.path().join(DEVICE_FILE)).unwrap();
        std::fs::create_dir(dir.path().join(DEVICE_FILE)).unwrap();
        assert!(!is_on(&conn, &me).unwrap());
        assert!(write(&mut conn, &me, false).is_err(), "off can't clear it");
        assert!(
            record_row(&conn).is_some_and(|r| on_by(Some(&r), &me.credential())),
            "off writes the device first: the store was not touched"
        );
        assert!(all_devices_off(&conn, dir.path(), VAULT).is_err());
        // A store that doesn't list the device still answers off when the clear fails.
        conn.execute(
            "DELETE FROM settings WHERE key = ?1",
            [store::SETTING_DEV_DEVICES],
        )
        .unwrap();
        assert!(!is_on(&conn, &me).unwrap());
    }

    #[test]
    fn switching_on_writes_the_store_first_and_off_writes_the_device_first() {
        // A switch that fails between its two writes leaves it off, in both directions.
        let (dir, mut conn) = rig();
        let me = here_in(dir.path(), A);
        std::fs::create_dir(dir.path().join(DEVICE_FILE)).unwrap();
        // On: the store's record is already written when the device's file fails.
        assert!(write(&mut conn, &me, true).is_err());
        assert!(on_by(record_row(&conn).as_deref(), &me.credential()));
        assert!(!is_on(&conn, &me).unwrap(), "a crash between leaves it off");
        // Off: the device's file fails first, so the store is as it was.
        let before = record_row(&conn);
        assert!(write(&mut conn, &me, false).is_err());
        assert_eq!(record_row(&conn), before);
        // And with the device's file in place, off writes both.
        std::fs::remove_dir(dir.path().join(DEVICE_FILE)).unwrap();
        assert!(write(&mut conn, &me, true).unwrap());
        assert!(!write(&mut conn, &me, false).unwrap());
        assert!(!on_by(record_row(&conn).as_deref(), &me.credential()));
        assert!(!lists_vault(dir.path(), VAULT));
    }

    #[test]
    fn two_vaults_on_one_device_are_independent() {
        let dir = tempfile::tempdir().unwrap();
        let (mut one, mut two) = (store(), store());
        let (here_one, here_two) = (
            Here::new(A, "vault-one", dir.path()),
            Here::new(A, "vault-two", dir.path()),
        );
        assert!(write(&mut one, &here_one, true).unwrap());
        assert!(is_on(&one, &here_one).unwrap());
        assert!(!is_on(&two, &here_two).unwrap(), "the other vault is off");
        assert!(write(&mut two, &here_two, true).unwrap());
        assert!(is_on(&one, &here_one).unwrap() && is_on(&two, &here_two).unwrap());
        // Off in one leaves the other.
        assert!(!write(&mut one, &here_one, false).unwrap());
        assert!(!is_on(&one, &here_one).unwrap());
        assert!(is_on(&two, &here_two).unwrap());
        // A restore of one clears only that vault.
        write(&mut one, &here_one, true).unwrap();
        all_devices_off(&one, dir.path(), "vault-one").unwrap();
        assert!(!is_on(&one, &here_one).unwrap());
        assert!(is_on(&two, &here_two).unwrap());
    }

    #[test]
    fn one_vaults_entry_is_not_another_vaults_answer() {
        // The device lists vault one; a store of vault two that lists the device is still off.
        let dir = tempfile::tempdir().unwrap();
        let mut one = store();
        let two = store();
        write(&mut one, &Here::new(A, "vault-one", dir.path()), true).unwrap();
        set_raw(
            &two,
            store::SETTING_DEV_DEVICES,
            &with_device(None, &credential(A), true),
        );
        assert!(!is_on(&two, &Here::new(A, "vault-two", dir.path())).unwrap());
        assert!(is_on(&one, &Here::new(A, "vault-one", dir.path())).unwrap());
    }

    #[test]
    fn the_device_file_is_small_versioned_atomic_and_capped_on_write() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..MAX_VAULTS + 3 {
            set_device(dir.path(), &format!("v{i}"), true).unwrap();
        }
        let kept = device_vaults(dir.path()).unwrap();
        assert_eq!(kept.len(), MAX_VAULTS);
        assert_eq!(
            kept.first().map(String::as_str),
            Some("v3"),
            "oldest falls off"
        );
        assert!(!lists_vault(dir.path(), "v0"));
        assert!(device_file(dir.path()).unwrap().starts_with(r#"{"v":1,"#));
        // Switching on twice lists it once; off in a vault it doesn't list writes nothing.
        set_device(dir.path(), "v10", true).unwrap();
        assert_eq!(
            device_vaults(dir.path())
                .unwrap()
                .iter()
                .filter(|v| *v == "v10")
                .count(),
            1
        );
        let before = device_file(dir.path());
        set_device(dir.path(), "not-listed", false).unwrap();
        assert_eq!(device_file(dir.path()), before);
        // No temp file is left beside it.
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, [DEVICE_FILE]);
        // A missing folder is an error for a write and off for a read, not a panic.
        let gone = dir.path().join("gone");
        assert!(set_device(&gone, "v", true).is_err());
        assert!(!device_says_on(&gone, "v"));
    }

    #[test]
    fn asking_writes_only_toward_off() {
        // The device's file is byte for byte what it was after asking when the answer is on, and
        // asking in a vault the device never switched on creates nothing.
        let (dir, mut conn) = rig();
        let me = here_in(dir.path(), A);
        assert!(!is_on(&conn, &me).unwrap());
        assert_eq!(device_file(dir.path()), None);
        write(&mut conn, &me, true).unwrap();
        let (file, rows) = (device_file(dir.path()), all_rows(&conn));
        for _ in 0..3 {
            assert!(is_on(&conn, &me).unwrap());
        }
        assert_eq!((device_file(dir.path()), all_rows(&conn)), (file, rows));
    }
}
