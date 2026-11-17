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
//! `backup::adopt_mods` fails on any other mention, and on any read here. [`is_on`] is a pure read:
//! it writes nothing, so merely asking can never change an answer. Upgrading from a PM that
//! carried developer mode over turns it off once; each person switches it on again in Settings,
//! under Developer.
//!
//! **Fails toward off.** A record that isn't what PM writes reads as nobody being on; a device
//! whose id can't be read is off; the list is capped (the oldest switch-on falls off first, and that
//! device is then simply off).
//!
//! The pure rules are here; the ledger carries the switch out against the mods
//! (`ledger/dev_switch.rs`: pause every unsigned mod, or resume exactly those it paused).

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

/// Whether developer mode is on for `device` (its id, from [`super::tab::device::device_id`]) in
/// this vault: only when the record lists that device's [`credential`]. A pure read: it writes
/// nothing, and no record is off.
pub fn is_on(conn: &Connection, device: &str) -> rusqlite::Result<bool> {
    Ok(on_by(
        read(conn, store::SETTING_DEV_DEVICES)?.as_deref(),
        &credential(device),
    ))
}

/// Switch developer mode on or off for `device` in this vault, in one transaction, and say what it
/// is now. Other devices' entries are untouched. This writes the record only: the ledger's
/// `set_dev_mode_for_device` is the one function that also pauses or resumes the unsigned mods.
pub fn write(conn: &mut Connection, device: &str, on: bool) -> rusqlite::Result<bool> {
    let credential = credential(device);
    let tx = conn.transaction()?;
    let next = with_device(
        read(&tx, store::SETTING_DEV_DEVICES)?.as_deref(),
        &credential,
        on,
    );
    tx.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![store::SETTING_DEV_DEVICES, next],
    )?;
    tx.commit()?;
    Ok(on_by(Some(&next), &credential))
}

/// Switch developer mode off for every device in this vault (the restore reset: §1.11 says a restore
/// never enables unsigned installs on another machine). Writes an empty record, and turns the old
/// row off if it is there (an older PM sharing the vault then starts off too; this PM never reads
/// it). The mods it affects are paused by the load that follows the store's open (nothing is paused
/// here: the load reads the record first and pauses an active unsigned row with the reason
/// `dev-mode`).
///
/// Called by the restore commit (`backup::adopt_mods::reset_for_adopt`, inside its transaction),
/// and by nothing else: a shared-vault join writes nothing.
pub fn all_devices_off(conn: &Connection) -> rusqlite::Result<()> {
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
            assert!(!is_on(&conn, device).unwrap(), "{device}");
        }
    }

    #[test]
    fn a_deleted_record_and_an_old_on_row_do_not_switch_a_device_on_again() {
        // The attack: a member of a shared vault holds the store key. A device was on; they delete
        // the record row and write the old row "true". Asking (any number of times, for any
        // device) must not hand the "on" back.
        let mut conn = store();
        assert!(write(&mut conn, A, true).unwrap());
        assert!(is_on(&conn, A).unwrap());
        conn.execute(
            "DELETE FROM settings WHERE key = ?1",
            [store::SETTING_DEV_DEVICES],
        )
        .unwrap();
        set_raw(&conn, LEGACY_ROW, "true");
        for _ in 0..2 {
            assert!(!is_on(&conn, A).unwrap());
            assert!(!is_on(&conn, B).unwrap());
        }
        // Writing the old row again after the asks changes nothing either.
        set_raw(&conn, LEGACY_ROW, "false");
        set_raw(&conn, LEGACY_ROW, "true");
        assert!(!is_on(&conn, A).unwrap());
    }

    #[test]
    fn a_record_that_exists_is_the_only_thing_that_says_on_whatever_the_old_row_says() {
        let mut conn = store();
        write(&mut conn, B, true).unwrap();
        for legacy in ["true", "false", "TRUE", ""] {
            set_raw(&conn, LEGACY_ROW, legacy);
            assert!(is_on(&conn, B).unwrap(), "{legacy:?}");
            assert!(!is_on(&conn, A).unwrap(), "{legacy:?}");
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
                    write(&mut conn, B, true).unwrap();
                    set_raw(&conn, LEGACY_ROW, "true");
                }
                _ => {
                    set_raw(&conn, store::SETTING_DEV_DEVICES, "garbage");
                    set_raw(&conn, LEGACY_ROW, "true");
                }
            }
            let before = all_rows(&conn);
            for device in [A, B] {
                is_on(&conn, device).unwrap();
            }
            assert_eq!(all_rows(&conn), before, "setup {setup}");
        }
    }

    #[test]
    fn a_vault_with_nothing_starts_off_for_everyone_and_stays_unwritten() {
        let conn = store();
        assert!(!is_on(&conn, A).unwrap());
        assert!(!is_on(&conn, B).unwrap());
        assert_eq!(read(&conn, store::SETTING_DEV_DEVICES).unwrap(), None);
    }

    #[test]
    fn a_junk_record_is_off_even_with_an_old_on_row() {
        let conn = store();
        set_raw(&conn, LEGACY_ROW, "true");
        set_raw(&conn, store::SETTING_DEV_DEVICES, "garbage");
        assert!(!is_on(&conn, A).unwrap());
    }

    #[test]
    fn writing_changes_this_device_only_and_says_what_it_is_now() {
        let mut conn = store();
        assert!(write(&mut conn, A, true).unwrap());
        assert!(write(&mut conn, B, true).unwrap());
        assert!(!write(&mut conn, A, false).unwrap());
        assert!(!is_on(&conn, A).unwrap());
        assert!(is_on(&conn, B).unwrap(), "B's switch is its own");
        // Off again is a no-op that still says off.
        assert!(!write(&mut conn, A, false).unwrap());
    }

    #[test]
    fn a_write_into_a_vault_with_an_old_on_row_switches_only_that_device() {
        let mut conn = store();
        set_raw(&conn, LEGACY_ROW, "true");
        // B switches on: A (who never asked) is not on, and the old row is left as it was.
        assert!(write(&mut conn, B, true).unwrap());
        assert!(!is_on(&conn, A).unwrap());
        assert_eq!(read(&conn, LEGACY_ROW).unwrap().as_deref(), Some("true"));
    }

    #[test]
    fn the_restore_reset_turns_every_device_and_the_old_row_off_and_stays_off() {
        let mut conn = store();
        set_raw(&conn, LEGACY_ROW, "true");
        write(&mut conn, A, true).unwrap();
        write(&mut conn, B, true).unwrap();
        all_devices_off(&conn).unwrap();
        assert!(!is_on(&conn, A).unwrap() && !is_on(&conn, B).unwrap());
        assert_eq!(read(&conn, LEGACY_ROW).unwrap().as_deref(), Some("false"));
        // The empty record is there, and an old "true" that comes back is still never read.
        let stored = read(&conn, store::SETTING_DEV_DEVICES).unwrap();
        assert_eq!(
            devices_of(stored.as_deref().expect("the empty record")).unwrap(),
            Vec::<String>::new()
        );
        set_raw(&conn, LEGACY_ROW, "true");
        assert!(!is_on(&conn, A).unwrap());
    }

    #[test]
    fn a_device_id_in_either_record_in_its_raw_form_is_not_a_switch() {
        // What a hostile member can do: read the id from the disclaimer's list and write it into
        // the developer-mode record. Both records name the RAW id here.
        let conn = store();
        let raw = format!(r#"{{"v":1,"devices":["{A}"]}}"#);
        set_raw(&conn, store::SETTING_DISCLAIMER, &raw);
        set_raw(&conn, store::SETTING_DEV_DEVICES, &raw);
        assert!(!is_on(&conn, A).unwrap());
        assert!(!crate::mods::tab::disclaimer::is_accepted(&conn, A).unwrap());
    }

    #[test]
    fn no_row_of_the_store_holds_the_raw_device_id_and_one_records_value_is_not_anothers() {
        let mut conn = store();
        crate::mods::tab::disclaimer::record(&conn, A).unwrap();
        assert!(write(&mut conn, A, true).unwrap());
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
        assert!(!is_on(&fresh, A).unwrap());
    }

    #[test]
    fn the_restore_reset_on_a_vault_with_no_record_writes_the_empty_record_and_the_old_row_off() {
        let conn = store();
        set_raw(&conn, LEGACY_ROW, "true");
        all_devices_off(&conn).unwrap();
        assert!(!is_on(&conn, A).unwrap());
        assert_eq!(read(&conn, LEGACY_ROW).unwrap().as_deref(), Some("false"));
        assert!(read(&conn, store::SETTING_DEV_DEVICES).unwrap().is_some());
        // And it didn't invent an old row where there was none.
        let bare = store();
        all_devices_off(&bare).unwrap();
        assert_eq!(read(&bare, LEGACY_ROW).unwrap(), None);
    }
}
