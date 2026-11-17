// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What names this device (plan decision 7: the Mods disclaimer is per device AND per vault, and
//! developer mode is per device too). PM had no device id: the lock session's `instance_id` is a
//! fresh uuid every launch, and the vault, the database and a backup all travel to other machines.
//!
//! So this is a random 128-bit id in `data_dir/mods-device-id`, minted by the first call and kept
//! for the life of the data folder. It is outside the database on purpose, so that nothing which
//! copies the database (a backup, a restore, a shared vault) can carry one machine's answer to
//! another. **It never goes into a row either**: the per-device records (the disclaimer's,
//! developer mode's) hold a [`credential`] of it, a different one-way value for each, so reading
//! one record gives nothing that works in another, and never the id itself. A "Remove PM
//! completely" takes it with the folder; "Remove PM data" for one vault leaves it, since the
//! device is still the same device.
//!
//! **Fails toward asking.** A file that is missing or isn't the shape PM writes (a hand edit, a
//! half-written copy) is replaced with a fresh id, so the device then reads as a new one: the
//! disclaimer asks again and developer mode is off. A file that can't be *read* (an I/O error) is
//! never written over: the caller gets the error and treats the device as unknown.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use sha2::{Digest, Sha256};

use crate::mods::disk;

/// The file's name in `data_dir`.
pub const DEVICE_FILE: &str = "mods-device-id";

/// An id is 32 lowercase hex digits: 128 random bits.
const ID_LEN: usize = 32;

/// What a database row may hold in place of this device's id: the hex SHA-256 of
/// `domain || 0x00 || device`. The raw id never goes into any row, because a shared vault's other
/// members (and anything that writes a crafted copy of the store) can read every row: with the raw
/// id in one, they could write it into another row that means something else (a developer-mode
/// entry from a disclaimer's list). Each record uses its own `domain`, so a value read from one
/// can't be replayed into another, and 128 random bits can't be recovered from the hash. A record
/// written in the old raw form just doesn't match, which is the fail-toward-asking direction.
pub fn credential(domain: &str, device: &str) -> String {
    let mut h = Sha256::new();
    h.update(domain.as_bytes());
    h.update([0u8]);
    h.update(device.as_bytes());
    hex::encode(h.finalize())
}

/// Reading, minting and replacing the file are one step at a time.
static LOCK: Mutex<()> = Mutex::new(());

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(DEVICE_FILE)
}

/// Whether `s` is an id PM would have written (pure).
pub fn is_device_id(s: &str) -> bool {
    s.len() == ID_LEN
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// This device's id, minted on first use. Blocking: a small file read, and on first use a write.
/// Small and reusable: the disclaimer reads it, and developer mode (`mods::devmode`) is per device
/// with it.
pub fn device_id(data_dir: &Path) -> io::Result<String> {
    let _guard = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let file = path(data_dir);
    match fs::read(&file) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let text = text.trim();
            if is_device_id(text) {
                return Ok(text.to_owned());
            }
            // Not ours: a new device, never a guess at the old one.
            mint(&file, true)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => mint(&file, false),
        Err(e) => Err(e),
    }
}

/// Write a fresh id. `replace`: there is a file whose content isn't an id; a temp file beside it is
/// written and fsynced, then renamed over it, so a crash leaves the old file or the new one. A
/// first mint creates the file itself (`create_new`: it never overwrites one a racing PM just made,
/// and loses to it by reading its id); a crash part way leaves a file that isn't an id, which the
/// next call replaces, so the worst case is one more disclaimer.
fn mint(file: &Path, replace: bool) -> io::Result<String> {
    let mut raw = [0u8; ID_LEN / 2];
    getrandom::fill(&mut raw).map_err(|e| io::Error::other(format!("no randomness: {e}")))?;
    let id = hex::encode(raw);
    let dir = file.parent().ok_or_else(|| io::Error::other("no folder"))?;
    let write = |to: &Path| -> io::Result<()> {
        let mut f = OpenOptions::new().write(true).create_new(true).open(to)?;
        f.write_all(id.as_bytes())?;
        f.sync_all()
    };
    if replace {
        let temp = dir.join(format!(".{DEVICE_FILE}.{}", &id[..8]));
        let placed = write(&temp).and_then(|()| fs::rename(&temp, file));
        if placed.is_err() {
            let _ = fs::remove_file(&temp);
        }
        placed?;
    } else {
        match write(file) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let theirs = fs::read_to_string(file)?;
                return if is_device_id(theirs.trim()) {
                    Ok(theirs.trim().to_owned())
                } else {
                    Err(io::Error::other("the device id file isn't an id"))
                };
            }
            Err(e) => return Err(e),
        }
    }
    let _ = disk::sync_dir(dir);
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_32_lowercase_hex_and_nothing_else() {
        assert!(is_device_id(&"a0".repeat(16)));
        for bad in [
            "",
            "abc",
            &"A0".repeat(16),
            &"g0".repeat(16),
            &"a0".repeat(17),
            &format!("{}\n", "a0".repeat(16)),
        ] {
            assert!(!is_device_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_credential_is_one_way_stable_and_different_for_each_record_and_device() {
        let id = "ab".repeat(16);
        let c = credential("pm-test-v1", &id);
        assert_eq!(c.len(), 64);
        assert!(!c.contains(&id), "the raw id is not in it");
        assert_eq!(c, credential("pm-test-v1", &id));
        assert_ne!(
            c,
            credential("pm-test-v2", &id),
            "a record's value is its own"
        );
        assert_ne!(c, credential("pm-test-v1", &"cd".repeat(16)));
        // The separator keeps `a || bc` and `ab || c` apart.
        assert_ne!(credential("a", "bc"), credential("ab", "c"));
    }

    #[test]
    fn the_first_call_mints_and_every_later_call_gives_the_same_id() {
        let dir = tempfile::tempdir().unwrap();
        let a = device_id(dir.path()).unwrap();
        assert!(is_device_id(&a));
        assert_eq!(device_id(dir.path()).unwrap(), a);
        assert_eq!(fs::read_to_string(path(dir.path())).unwrap(), a);
        // No temp file is left beside it.
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, [DEVICE_FILE]);
    }

    #[test]
    fn two_data_folders_are_two_devices() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        assert_ne!(device_id(a.path()).unwrap(), device_id(b.path()).unwrap());
    }

    #[test]
    fn a_file_that_isnt_an_id_is_replaced_so_the_device_reads_as_new() {
        let dir = tempfile::tempdir().unwrap();
        let first = device_id(dir.path()).unwrap();
        for junk in ["", "not an id", "{\"id\":1}", &first[..31]] {
            fs::write(path(dir.path()), junk).unwrap();
            let fresh = device_id(dir.path()).unwrap();
            assert!(is_device_id(&fresh), "{junk:?}");
            assert_ne!(fresh, first, "{junk:?} must not resurrect the old id");
            assert_eq!(device_id(dir.path()).unwrap(), fresh);
        }
    }

    #[test]
    fn an_id_with_a_trailing_newline_is_still_that_id() {
        // A hand-edited copy: the trimmed text is read, and the file is left as it is.
        let dir = tempfile::tempdir().unwrap();
        let id = "ab".repeat(16);
        fs::write(path(dir.path()), format!("{id}\n")).unwrap();
        assert_eq!(device_id(dir.path()).unwrap(), id);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_that_cannot_be_read_is_an_error_and_is_never_written_over() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let id = device_id(dir.path()).unwrap();
        fs::set_permissions(path(dir.path()), fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read(path(dir.path())).is_ok() {
            return; // running as root: nothing is unreadable
        }
        assert!(device_id(dir.path()).is_err());
        fs::set_permissions(path(dir.path()), fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(device_id(dir.path()).unwrap(), id);
    }

    #[test]
    fn a_missing_folder_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        assert!(device_id(&dir.path().join("gone")).is_err());
    }
}
