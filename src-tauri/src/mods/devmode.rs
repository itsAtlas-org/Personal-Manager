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

/// The pre-per-device boolean row (`"true"`/`"false"`), once webview-written. Read once, to carry an
/// old "on" over to the device that first reads it; never an authority after that
/// ([`store::SETTING_DEV_DEVICES`] exists), and a restore turns it off too. **This is the only
/// place in Rust that names it** (besides the webview allow-list's and the Dev tab's key lists), so
/// nothing can gate on a vault-wide switch by accident; `adopt_mods`'s tripwire test fails on a
/// reader anywhere else.
pub(crate) const LEGACY_ROW: &str = "dev_mode";

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

/// The record a vault with none gets when its first device asks, from the old boolean row (pure):
/// that device alone is on when the old row said `"true"`, and nobody is otherwise.
pub fn migrated(legacy: Option<&str>, credential: &str) -> String {
    with_device(None, credential, legacy == Some("true"))
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

/// Make sure the vault has its record, carrying the old row's meaning over to `device` when it has
/// none (module doc). One statement, so two devices asking together can't both write: the first
/// record stands, and each then reads it.
fn ensure(conn: &Connection, credential: &str) -> rusqlite::Result<()> {
    if read(conn, store::SETTING_DEV_DEVICES)?.is_some() {
        return Ok(());
    }
    let legacy = read(conn, LEGACY_ROW)?;
    conn.execute(
        "INSERT OR IGNORE INTO settings(key, value) VALUES (?1, ?2)",
        params![
            store::SETTING_DEV_DEVICES,
            migrated(legacy.as_deref(), credential)
        ],
    )?;
    Ok(())
}

/// Whether developer mode is on for `device` (its id, from [`super::tab::device::device_id`]) in
/// this vault. The first read of a vault with no record writes it ([`ensure`]).
pub fn is_on(conn: &Connection, device: &str) -> rusqlite::Result<bool> {
    let credential = credential(device);
    ensure(conn, &credential)?;
    Ok(on_by(
        read(conn, store::SETTING_DEV_DEVICES)?.as_deref(),
        &credential,
    ))
}

/// Switch developer mode on or off for `device` in this vault, in one transaction, and say what it
/// is now. Other devices' entries are untouched. This writes the record only: the ledger's
/// `set_dev_mode_for_device` is the one function that also pauses or resumes the unsigned mods.
pub fn write(conn: &mut Connection, device: &str, on: bool) -> rusqlite::Result<bool> {
    let credential = credential(device);
    let tx = conn.transaction()?;
    ensure(&tx, &credential)?;
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
/// never enables unsigned installs on another machine). Writes an empty record, so an old "on" row
/// can't be carried over afterwards, and turns the old row off if it is there. The mods it affects
/// are paused by the load that follows the store's open (nothing is paused here: the load reads the
/// record first and pauses an active unsigned row with the reason `dev-mode`).
///
/// Called by the restore commit (`backup::adopt_mods::reset_for_adopt`, inside its transaction) and
/// by a join of a store with no record ([`drop_inherited_on_at_join`]).
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

/// A shared-vault join's one write (§1.15, decided 2026-11-12): a joined store with no record gets
/// the empty one (nobody on) and its old row turned off if it says `"true"`. Without that, the
/// carry-over in [`ensure`] would hand an old "on" to the joiner's device, which never chose it, and
/// would stay armed for a store whose old row only turns `"true"` later (a member on an earlier
/// build writing it) once this device takes over as the writer. Sealing the record closes the
/// carry-over for good; the owner's device switches theirs on again, which is the fail-safe
/// direction. A store that already has a record is never touched: the old row is never read, the
/// joiner has no entry, and **no write lock is taken** (a plain read decides), so a busy writer
/// can't make the join fail. Only a store with no record opens the immediate transaction, and
/// re-checks inside it, so a record another machine writes meanwhile is never overwritten.
/// Says whether it wrote.
///
/// Must run on the joined store before anything on the joiner reads developer mode (the writer's
/// open: [`is_on`] is what writes the record).
pub fn drop_inherited_on_at_join(conn: &Connection) -> rusqlite::Result<bool> {
    if read(conn, store::SETTING_DEV_DEVICES)?.is_some() {
        return Ok(false);
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    let sealed = read(&tx, store::SETTING_DEV_DEVICES)?.is_none();
    if sealed {
        all_devices_off(&tx)?;
    }
    tx.commit()?;
    Ok(sealed)
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
        set_raw(&conn, LEGACY_ROW, "true");
        assert!(is_on(&conn, A).unwrap());
        // The old row said "on", and it is still "true": no other device inherits it.
        assert!(!is_on(&conn, B).unwrap());
        // Flipping the old row afterwards (the webview could once write it) changes nothing.
        set_raw(&conn, LEGACY_ROW, "false");
        assert!(is_on(&conn, A).unwrap());
        set_raw(&conn, LEGACY_ROW, "true");
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
    fn a_write_into_a_vault_with_an_old_on_row_keeps_that_devices_on_until_it_switches_off() {
        let mut conn = store();
        set_raw(&conn, LEGACY_ROW, "true");
        // B switches on first: A (who never asked) is not turned on by the old row afterwards,
        // since the record now exists and was made for B.
        assert!(write(&mut conn, B, true).unwrap());
        assert!(!is_on(&conn, A).unwrap());
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
        // Nothing to carry over afterwards, even if an old "true" comes back.
        set_raw(&conn, LEGACY_ROW, "true");
        assert!(!is_on(&conn, A).unwrap());
    }

    #[test]
    fn a_join_of_a_store_with_no_record_and_an_old_on_switches_everyone_off_first() {
        let conn = store();
        set_raw(&conn, LEGACY_ROW, "true");
        assert!(drop_inherited_on_at_join(&conn).unwrap());
        // The joiner's device is off, and so is every other: the owner switches theirs on again.
        assert!(!is_on(&conn, A).unwrap() && !is_on(&conn, B).unwrap());
        // The record was written, empty, so the old row is never read again.
        let stored = read(&conn, store::SETTING_DEV_DEVICES).unwrap();
        assert_eq!(devices_of(stored.as_deref().unwrap()).unwrap().len(), 0);
        set_raw(&conn, LEGACY_ROW, "true");
        assert!(!is_on(&conn, A).unwrap());
    }

    #[test]
    fn a_join_of_a_store_that_already_has_a_record_writes_nothing() {
        let mut conn = store();
        set_raw(&conn, LEGACY_ROW, "true");
        write(&mut conn, B, true).unwrap();
        let before = read(&conn, store::SETTING_DEV_DEVICES).unwrap();
        let legacy_before = read(&conn, LEGACY_ROW).unwrap();
        assert!(!drop_inherited_on_at_join(&conn).unwrap());
        assert_eq!(read(&conn, store::SETTING_DEV_DEVICES).unwrap(), before);
        assert_eq!(read(&conn, LEGACY_ROW).unwrap(), legacy_before);
        // The owner's device keeps its switch; the joiner never had one.
        assert!(is_on(&conn, B).unwrap() && !is_on(&conn, A).unwrap());
    }

    #[test]
    fn a_join_of_a_store_with_no_record_and_no_old_on_still_seals_an_empty_record() {
        // Sealed, not left armed: an old row that only turns "true" later (a member on an earlier
        // build writes it) must not be carried to the joiner's device when it becomes the writer.
        for legacy in [None, Some("false"), Some("TRUE"), Some("")] {
            let conn = store();
            if let Some(v) = legacy {
                set_raw(&conn, LEGACY_ROW, v);
            }
            assert!(drop_inherited_on_at_join(&conn).unwrap(), "{legacy:?}");
            let stored = read(&conn, store::SETTING_DEV_DEVICES).unwrap();
            assert_eq!(
                devices_of(stored.as_deref().expect("the empty record")).unwrap(),
                Vec::<String>::new(),
                "{legacy:?}"
            );
            // An old row that was absent is not invented.
            if legacy.is_none() {
                assert_eq!(read(&conn, LEGACY_ROW).unwrap(), None);
            }
            // Nobody is on, and the late old "true" is never read.
            set_raw(&conn, LEGACY_ROW, "true");
            assert!(!is_on(&conn, A).unwrap(), "{legacy:?}");
        }
    }

    #[test]
    fn a_join_of_a_store_that_has_a_record_never_waits_for_the_writers_lock() {
        // One file, two connections: the active writer is mid-transaction, holding the write lock.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pm.sqlite");
        let key = "00".repeat(32);
        let mut writer = crate::db::open(&path, &key).unwrap();
        write(&mut writer, B, true).unwrap();
        let joiner = crate::db::open(&path, &key).unwrap();
        joiner
            .busy_timeout(std::time::Duration::from_millis(50))
            .unwrap();
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        // A plain read decides: nothing to write, so the lock is never asked for.
        assert!(!drop_inherited_on_at_join(&joiner).unwrap());
        writer.execute_batch("COMMIT").unwrap();
        assert!(is_on(&joiner, B).unwrap() && !is_on(&joiner, A).unwrap());
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
    fn the_restore_reset_on_a_vault_with_no_record_leaves_no_old_on_to_migrate() {
        let conn = store();
        set_raw(&conn, LEGACY_ROW, "true");
        all_devices_off(&conn).unwrap();
        assert!(!is_on(&conn, A).unwrap());
        // And it didn't invent an old row where there was none.
        let bare = store();
        all_devices_off(&bare).unwrap();
        assert_eq!(read(&bare, LEGACY_ROW).unwrap(), None);
    }
}
