// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Developer mode, per device and per vault (Mods spec §1.15, decided 2026-11-12): "on for this
//! device in this vault", not a vault-wide switch. It gates the one thing that matters, unsigned
//! mods (an unsigned file installs only while it is on, and an unsigned mod runs only while it is
//! on), so it has to be something a restore onto another machine, a joined shared vault or a copied
//! database can never carry in: the record lists the devices that switched it on
//! ([`store::SETTING_DEV_DEVICES`]), named by [`super::tab::device::device_id`], and a device that
//! isn't in the list is off whatever else the database says.
//!
//! **Only Rust reads it.** The old boolean row (`dev_mode`) was a webview preference, which is why
//! it can't be believed: it is no longer in the page's allow-list, and the answer the page shows is
//! the `dev_mode_state` command's, from this record.
//!
//! **The old row's meaning, carried over once.** A vault with no record yet and an old row of
//! `"true"` was switched on, before there were devices, by the person using it: the first device
//! to ask gets that "on" (and only that device), by writing the record, so nothing is believed
//! twice. From then on the old row is never read. A restore writes an empty record (so a restored
//! old "on" can't be carried over to the restoring machine) and turns the old row off too
//! ([`all_devices_off`]).
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

/// Most devices one vault remembers.
pub const MAX_DEVICES: usize = 32;

const FORMAT: u32 = 1;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    v: u32,
    devices: Vec<String>,
}

/// The devices a stored record names, or `None` when it isn't a record PM wrote (pure).
fn devices_of(stored: &str) -> Option<Vec<String>> {
    serde_json::from_str::<Record>(stored)
        .ok()
        .filter(|r| r.v == FORMAT)
        .map(|r| r.devices)
}

/// Whether `device` is on, by the stored record (pure). No record, or one that isn't ours, is off.
pub fn on_by(stored: Option<&str>, device: &str) -> bool {
    stored
        .and_then(devices_of)
        .is_some_and(|d| d.iter().any(|x| x == device))
}

/// The record with `device` switched on or off (pure): once in the list at most, newest last, no
/// longer than [`MAX_DEVICES`]. A record that isn't ours is replaced.
pub fn with_device(stored: Option<&str>, device: &str, on: bool) -> String {
    let mut devices = stored.and_then(devices_of).unwrap_or_default();
    devices.retain(|d| d != device);
    if on {
        devices.push(device.to_owned());
    }
    let over = devices.len().saturating_sub(MAX_DEVICES);
    devices.drain(..over);
    serde_json::to_string(&Record { v: FORMAT, devices }).unwrap_or_default()
}

/// The record a vault with none gets when its first device asks, from the old boolean row (pure):
/// that device alone is on when the old row said `"true"`, and nobody is otherwise.
pub fn migrated(legacy: Option<&str>, device: &str) -> String {
    with_device(None, device, legacy == Some("true"))
}

/// The record that switches every device off.
#[cfg_attr(not(test), allow(dead_code))] // [`all_devices_off`]'s, see there
fn nobody() -> String {
    with_device(None, "", false)
}

fn read(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
        r.get(0)
    })
    .optional()
}

/// Make sure the vault has its record, carrying the old row's meaning over to `device` when it has
/// none (module doc). One statement, so two devices asking together can't both write: the first
/// record stands, and each then reads it.
fn ensure(conn: &Connection, device: &str) -> rusqlite::Result<()> {
    if read(conn, store::SETTING_DEV_DEVICES)?.is_some() {
        return Ok(());
    }
    let legacy = read(conn, store::SETTING_DEV_MODE_LEGACY)?;
    conn.execute(
        "INSERT OR IGNORE INTO settings(key, value) VALUES (?1, ?2)",
        params![
            store::SETTING_DEV_DEVICES,
            migrated(legacy.as_deref(), device)
        ],
    )?;
    Ok(())
}

/// Whether developer mode is on for `device` in this vault. The first read of a vault with no
/// record writes it ([`ensure`]).
pub fn is_on(conn: &Connection, device: &str) -> rusqlite::Result<bool> {
    ensure(conn, device)?;
    Ok(on_by(
        read(conn, store::SETTING_DEV_DEVICES)?.as_deref(),
        device,
    ))
}

/// Switch developer mode on or off for `device` in this vault, in one transaction, and say what it
/// is now. Other devices' entries are untouched. This writes the record only: the ledger's
/// `set_dev_mode_for_device` is the one function that also pauses or resumes the unsigned mods.
pub fn write(conn: &mut Connection, device: &str, on: bool) -> rusqlite::Result<bool> {
    let tx = conn.transaction()?;
    ensure(&tx, device)?;
    let next = with_device(
        read(&tx, store::SETTING_DEV_DEVICES)?.as_deref(),
        device,
        on,
    );
    tx.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![store::SETTING_DEV_DEVICES, next],
    )?;
    tx.commit()?;
    Ok(on_by(Some(&next), device))
}

/// Switch developer mode off for every device in this vault (the restore reset: §1.11 says a restore
/// never enables unsigned installs on another machine). Writes an empty record, so an old "on" row
/// can't be carried over afterwards, and turns the old row off if it is there. The mods it affects
/// are paused by the load that follows the store's open.
///
/// Nothing on this branch calls it: it is the restore commit's (track B's
/// `backup::adopt_mods::reset_for_adopt`, which writes the old row off today), wired when the two
/// branches merge.
#[cfg_attr(not(test), allow(dead_code))] // the restore reset's, at the merge with track B
pub fn all_devices_off(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![store::SETTING_DEV_DEVICES, nobody()],
    )?;
    conn.execute(
        "UPDATE settings SET value = 'false' WHERE key = ?1",
        [store::SETTING_DEV_MODE_LEGACY],
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

    #[test]
    fn an_old_on_row_is_carried_to_the_first_device_that_asks_and_to_nobody_else() {
        assert!(on_by(Some(&migrated(Some("true"), A)), A));
        assert!(!on_by(Some(&migrated(Some("true"), A)), B));
        // Off, absent and anything but the exact word: nobody.
        for legacy in [Some("false"), None, Some(""), Some("TRUE"), Some("1")] {
            assert!(!on_by(Some(&migrated(legacy, A)), A), "{legacy:?}");
        }
    }

    #[test]
    fn the_first_read_writes_the_record_and_the_old_row_is_never_read_again() {
        let conn = store();
        set_raw(&conn, store::SETTING_DEV_MODE_LEGACY, "true");
        assert!(is_on(&conn, A).unwrap());
        // The old row said "on", and it is still "true": no other device inherits it.
        assert!(!is_on(&conn, B).unwrap());
        // Flipping the old row afterwards (the webview could once write it) changes nothing.
        set_raw(&conn, store::SETTING_DEV_MODE_LEGACY, "false");
        assert!(is_on(&conn, A).unwrap());
        set_raw(&conn, store::SETTING_DEV_MODE_LEGACY, "true");
        assert!(!is_on(&conn, B).unwrap());
    }

    #[test]
    fn a_vault_with_no_old_row_starts_off_for_everyone() {
        let conn = store();
        assert!(!is_on(&conn, A).unwrap());
        assert!(!is_on(&conn, B).unwrap());
        assert!(read(&conn, store::SETTING_DEV_DEVICES).unwrap().is_some());
    }

    #[test]
    fn a_junk_record_is_off_and_is_not_migrated_over() {
        let conn = store();
        set_raw(&conn, store::SETTING_DEV_MODE_LEGACY, "true");
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
    fn a_write_into_a_vault_with_an_old_on_row_keeps_that_devices_on_until_it_switches_off() {
        let mut conn = store();
        set_raw(&conn, store::SETTING_DEV_MODE_LEGACY, "true");
        // B switches on first: A (who never asked) is not turned on by the old row afterwards,
        // since the record now exists and was made for B.
        assert!(write(&mut conn, B, true).unwrap());
        assert!(!is_on(&conn, A).unwrap());
    }

    #[test]
    fn the_restore_reset_turns_every_device_and_the_old_row_off_and_stays_off() {
        let mut conn = store();
        set_raw(&conn, store::SETTING_DEV_MODE_LEGACY, "true");
        write(&mut conn, A, true).unwrap();
        write(&mut conn, B, true).unwrap();
        all_devices_off(&conn).unwrap();
        assert!(!is_on(&conn, A).unwrap() && !is_on(&conn, B).unwrap());
        assert_eq!(
            read(&conn, store::SETTING_DEV_MODE_LEGACY)
                .unwrap()
                .as_deref(),
            Some("false")
        );
        // Nothing to carry over afterwards, even if an old "true" comes back.
        set_raw(&conn, store::SETTING_DEV_MODE_LEGACY, "true");
        assert!(!is_on(&conn, A).unwrap());
    }

    #[test]
    fn the_restore_reset_on_a_vault_with_no_record_leaves_no_old_on_to_migrate() {
        let conn = store();
        set_raw(&conn, store::SETTING_DEV_MODE_LEGACY, "true");
        all_devices_off(&conn).unwrap();
        assert!(!is_on(&conn, A).unwrap());
        // And it didn't invent an old row where there was none.
        let bare = store();
        all_devices_off(&bare).unwrap();
        assert_eq!(read(&bare, store::SETTING_DEV_MODE_LEGACY).unwrap(), None);
    }
}
