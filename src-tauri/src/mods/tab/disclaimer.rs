// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Mods disclaimer's record (spec §1.13, plan decision 7): accepted once, but per device AND per
//! vault. The vault half is a `settings` row listing the devices that accepted it in this vault
//! ([`store::SETTING_DISCLAIMER`]); the device half is [`super::device::device_id`]. A person is
//! asked again on a new machine (it isn't in the list, even when the database was restored
//! there), and again in a vault they join or restore (its list has never heard of this device).
//!
//! **The list holds [`credential`]s, not device ids**: the raw id is kept out of every row (see
//! `device.rs`), so a reader of a shared store can't lift it from here and write a "developer mode
//! on" entry for that device. This record's value is made under its own domain, so one copied
//! from developer mode's record isn't an acceptance either. A record in the old raw form names
//! nobody (people are asked again).
//!
//! The first-open "Turn on" of an auto-installed pilot counts as accepting (D17): its consent
//! writes the same record in the same transaction ([`record`]).
//!
//! A record that isn't what PM writes reads as nobody having accepted: it fails toward asking, and
//! the next acceptance replaces it. The list is capped (the oldest acceptance falls off first, and
//! that device simply asks once more), so a shared vault can't grow it without bound.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::device;
use crate::mods::store;

/// Most devices one vault remembers.
pub const MAX_DEVICES: usize = 32;

const FORMAT: u32 = 1;

/// The domain this record's [`credential`]s are made under (developer mode's differs).
const CREDENTIAL_DOMAIN: &str = "pm-mods-disclaimer-v1";

/// What the record holds for `device` in place of its id (module doc).
pub fn credential(device: &str) -> String {
    device::credential(CREDENTIAL_DOMAIN, device)
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    v: u32,
    devices: Vec<String>,
}

/// The accepted devices a stored record names; empty for none or a record that isn't ours (pure).
fn devices_of(stored: Option<&str>) -> Vec<String> {
    stored
        .and_then(|s| serde_json::from_str::<Record>(s).ok())
        .filter(|r| r.v == FORMAT)
        .map(|r| r.devices)
        .unwrap_or_default()
}

/// Whether the device whose [`credential`] this is accepted, by the stored record (pure).
pub fn accepted(stored: Option<&str>, credential: &str) -> bool {
    devices_of(stored).iter().any(|d| d == credential)
}

/// The record with the device whose [`credential`] this is added: at the end (newest last), once,
/// and no longer than [`MAX_DEVICES`] (pure).
pub fn with_device(stored: Option<&str>, credential: &str) -> String {
    let mut devices = devices_of(stored);
    devices.retain(|d| d != credential);
    devices.push(credential.to_owned());
    let over = devices.len().saturating_sub(MAX_DEVICES);
    devices.drain(..over);
    serde_json::to_string(&Record { v: FORMAT, devices }).unwrap_or_default()
}

fn read(conn: &Connection) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        [store::SETTING_DISCLAIMER],
        |r| r.get(0),
    )
    .optional()
}

/// Whether `device` has accepted the disclaimer in this vault.
pub fn is_accepted(conn: &Connection, device: &str) -> rusqlite::Result<bool> {
    Ok(accepted(read(conn)?.as_deref(), &credential(device)))
}

/// Record that `device` accepted, in this vault. Read and written in the caller's step (a
/// transaction, for the consent that counts as accepting), so two devices of a shared vault
/// accepting together don't lose one another's answer.
pub fn record(conn: &Connection, device: &str) -> rusqlite::Result<()> {
    let next = with_device(read(conn)?.as_deref(), &credential(device));
    conn.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![store::SETTING_DISCLAIMER, next],
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

    #[test]
    fn nobody_has_accepted_until_a_device_does_and_only_that_device() {
        assert!(!accepted(None, A));
        let one = with_device(None, A);
        assert!(accepted(Some(&one), A));
        // Another device, with the same vault's record: asks again.
        assert!(!accepted(Some(&one), B));
        let two = with_device(Some(&one), B);
        assert!(accepted(Some(&two), A) && accepted(Some(&two), B));
    }

    #[test]
    fn accepting_twice_lists_the_device_once() {
        let once = with_device(None, A);
        let twice = with_device(Some(&once), A);
        assert_eq!(devices_of(Some(&twice)), [A]);
    }

    #[test]
    fn a_record_that_isnt_ours_reads_as_nobody_and_is_replaced_by_the_next_acceptance() {
        for junk in [
            "",
            "not json",
            "[]",
            r#"{"v":2,"devices":["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]}"#,
            r#"{"v":1,"devices":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
            r#"{"v":1,"devices":[],"extra":1}"#,
        ] {
            assert!(!accepted(Some(junk), A), "{junk}");
            let fixed = with_device(Some(junk), B);
            assert_eq!(devices_of(Some(&fixed)), [B], "{junk}");
        }
    }

    #[test]
    fn the_list_is_capped_and_the_oldest_acceptance_falls_off_first() {
        let mut stored = None;
        let ids: Vec<String> = (0..MAX_DEVICES + 3).map(|i| format!("{i:032x}")).collect();
        for id in &ids {
            stored = Some(with_device(stored.as_deref(), id));
        }
        let kept = devices_of(stored.as_deref());
        assert_eq!(kept.len(), MAX_DEVICES);
        assert_eq!(kept.first(), Some(&ids[3]));
        assert_eq!(kept.last(), ids.last());
        assert!(!accepted(stored.as_deref(), &ids[0]));
    }

    #[test]
    fn the_record_holds_a_credential_so_neither_the_raw_id_nor_developer_modes_value_accepts() {
        let conn = store();
        record(&conn, A).unwrap();
        let stored = read(&conn).unwrap().unwrap();
        assert!(
            !stored.contains(A),
            "the raw id is not in the row: {stored}"
        );
        assert!(stored.contains(&credential(A)));
        for lookalike in [A.to_owned(), crate::mods::devmode::credential(A)] {
            let forged = format!(r#"{{"v":1,"devices":["{lookalike}"]}}"#);
            assert!(!accepted(Some(&forged), &credential(A)), "{lookalike}");
        }
        // And through the store: a record naming the raw id is nobody's acceptance.
        let other = store();
        other
            .execute(
                "INSERT INTO settings(key, value) VALUES (?1, ?2)",
                params![
                    store::SETTING_DISCLAIMER,
                    format!(r#"{{"v":1,"devices":["{A}"]}}"#)
                ],
            )
            .unwrap();
        assert!(!is_accepted(&other, A).unwrap());
    }

    #[test]
    fn the_record_lives_in_this_vaults_settings_and_a_second_vault_has_none() {
        let here = store();
        let elsewhere = store();
        assert!(!is_accepted(&here, A).unwrap());
        record(&here, A).unwrap();
        assert!(is_accepted(&here, A).unwrap());
        assert!(
            !is_accepted(&elsewhere, A).unwrap(),
            "a joined vault asks again"
        );
        // A restore of `here` on another machine carries the row, not the device.
        assert!(!is_accepted(&here, B).unwrap(), "a new machine asks again");
        record(&here, B).unwrap();
        assert!(is_accepted(&here, A).unwrap() && is_accepted(&here, B).unwrap());
    }
}
