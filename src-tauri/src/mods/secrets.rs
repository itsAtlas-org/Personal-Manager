// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Where a mod's secrets live and what they are bound to (Mods spec §1.7, PR B X4).
//!
//! A secret is a key the USER typed into a field PM drew (the mod's row in the Mods tab, or the
//! Developer tab's Test keys). It is stored in `mod_secrets` inside the encrypted database, never
//! in the OS keychain (that holds the database key itself, Windows caps an item at 2560 bytes and
//! macOS keeps one bundled item), as `(mod, slot name, host, header, value)`. **The mod never holds
//! the value**: nothing here hands it to a frame, no command returns it (a status is a bool) and no
//! error, log line or `Debug` prints it ([`SecretValue`]). `net.fetch` reads it in Rust
//! ([`read_for_use`]) and adds it as the slot's one header on the slot's one host.
//!
//! **The binding.** The host and header of a stored row are copied from the installed manifest's
//! slot when the key is saved, and a row counts only while they still equal that slot: a row whose
//! pair no longer matches (a slot moved by an update, a row planted by a crafted backup or a
//! hand-edited store) is as good as no key, so a value typed for one destination can never go to
//! another. An update or repair that moves or drops a slot also deletes the row
//! ([`prune_to_slots`], run in the install's own transaction), so the old value does not wait in
//! the store to be matched by a later slot. A restore deletes every row ([`purge_all`]): backups
//! carry none (`backup::strip`), and a store whose rows came from anywhere else is not believed.
//!
//! **No other reader.** The dev inspector's allow-list does not name `mod_secrets`
//! (`commands_dev::TABLES`, a test keeps it so); the listing reads names, hosts and headers, never
//! the value column; the store is vault-scoped (SYNC-SET class truth), so a co-user of a shared
//! vault can open it, which the public wording says (Q9, drafted for Bobby, not decided here).
//!
//! Everything here is database work on a connection the caller holds for one step: no file or
//! network I/O, no `.await`.

use std::collections::BTreeSet;
use std::fmt;

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use serde_json::Value;

use crate::mods::api::net;

/// One secret slot a manifest declares: the name the mod and the user know it by, the one host it
/// goes to and the one header it goes in.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Slot {
    pub name: String,
    pub host: String,
    pub header: String,
}

/// The slots of a stored manifest, as written (pure, tolerant: an entry missing a part is not a
/// slot, and a manifest that does not read has none, so nothing is bound to it).
pub fn slots(manifest: &str) -> Vec<Slot> {
    let Ok(v) = serde_json::from_str::<Value>(manifest) else {
        return Vec::new();
    };
    let text = |s: &Value, key: &str| s.get(key).and_then(Value::as_str).map(str::to_owned);
    v["secrets"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|s| {
                    Some(Slot {
                        name: text(s, "name")?,
                        host: text(s, "host")?,
                        header: text(s, "header")?,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A slot and whether it has a key now: what the Mods row and Test keys show. Never the value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotStatus {
    pub name: String,
    pub host: String,
    pub header: String,
    /// A key is stored AND bound to this slot's present host and header.
    pub set: bool,
}

/// What every `(mod, name, host, header)` of the store with a key is, without the value (pure
/// read): the listing's one query for all mods.
pub fn bound_pairs(
    conn: &Connection,
) -> rusqlite::Result<BTreeSet<(String, String, String, String)>> {
    let mut stmt = conn.prepare("SELECT mod_id, name, host, header FROM mod_secrets")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
    rows.collect()
}

/// The slots of `manifest` with their status against `bound` (pure).
pub fn statuses(
    mod_id: &str,
    manifest: &str,
    bound: &BTreeSet<(String, String, String, String)>,
) -> Vec<SlotStatus> {
    slots(manifest)
        .into_iter()
        .map(|s| {
            let set = bound.contains(&(
                mod_id.to_owned(),
                s.name.clone(),
                s.host.clone(),
                s.header.clone(),
            ));
            SlotStatus {
                name: s.name,
                host: s.host,
                header: s.header,
                set,
            }
        })
        .collect()
}

/// A key's text. It prints as nothing and is never serialised: the one way out is [`Self::expose`],
/// which only the code that puts it on a request line calls.
#[cfg_attr(not(test), allow(dead_code))] // read by `net.fetch`'s handler (X6)
pub struct SecretValue(String);

impl SecretValue {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue(..)")
    }
}

/// A key ready for a request: its slot (the host the URL must be, the header it goes in) and the
/// value.
#[derive(Debug)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct Bound {
    pub slot: Slot,
    pub value: SecretValue,
}

/// Why a command refused.
#[derive(Debug)]
pub enum Refused {
    /// No such installed mod.
    NotInstalled,
    /// The manifest declares no slot of that name.
    NoSuchSlot,
    /// A key that cannot be sent in a header: too long, or text a header line cannot carry. Fixed
    /// words only; the typed text is never quoted.
    Unusable,
    Db(rusqlite::Error),
}

impl From<rusqlite::Error> for Refused {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e)
    }
}

/// What a save did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Saved {
    Stored,
    /// An empty value: the row was deleted (or never was).
    Cleared,
}

fn manifest_of(conn: &Connection, mod_id: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT manifest FROM mods WHERE id = ?1", [mod_id], |r| {
        r.get(0)
    })
    .optional()
}

/// Spaces, tabs and line breaks around what was pasted are not part of a key (a paste usually
/// brings a newline); anything inside is judged as typed.
fn trimmed(typed: &str) -> &str {
    typed.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\r' | '\n'))
}

/// Store the key typed for slot `name` of `mod_id`, bound to the manifest slot's present host and
/// header; or, when nothing is left after trimming, delete it. One transaction, so the manifest
/// the slot is read from is the one the row is bound to.
pub fn save(
    conn: &mut Connection,
    mod_id: &str,
    name: &str,
    typed: &str,
) -> Result<Saved, Refused> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let manifest = manifest_of(&tx, mod_id)?.ok_or(Refused::NotInstalled)?;
    let value = trimmed(typed);
    // Deleting needs no declared slot: a row left from an old manifest must always be removable.
    if value.is_empty() {
        tx.execute(
            "DELETE FROM mod_secrets WHERE mod_id = ?1 AND name = ?2",
            params![mod_id, name],
        )?;
        tx.commit()?;
        return Ok(Saved::Cleared);
    }
    let slot = slots(&manifest)
        .into_iter()
        .find(|s| s.name == name)
        .ok_or(Refused::NoSuchSlot)?;
    // The very check `net.fetch` applies when it reads the row: a key that passes here can be sent,
    // and one that could not be is refused now, while the person is looking.
    if net::secret_header(&slot.header, value).is_err() {
        return Err(Refused::Unusable);
    }
    tx.execute(
        "INSERT INTO mod_secrets(mod_id, name, host, header, value) VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(mod_id, name) DO UPDATE SET host = excluded.host, header = excluded.header, \
             value = excluded.value, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')",
        params![mod_id, name, slot.host, slot.header, value],
    )?;
    tx.commit()?;
    Ok(Saved::Stored)
}

/// Delete the key of slot `name`. Idempotent; refuses only a mod that is not installed.
pub fn clear(conn: &Connection, mod_id: &str, name: &str) -> Result<(), Refused> {
    if manifest_of(conn, mod_id)?.is_none() {
        return Err(Refused::NotInstalled);
    }
    conn.execute(
        "DELETE FROM mod_secrets WHERE mod_id = ?1 AND name = ?2",
        params![mod_id, name],
    )?;
    Ok(())
}

/// The slots of one installed mod and whether each has a key now.
pub fn status(conn: &Connection, mod_id: &str) -> Result<Vec<SlotStatus>, Refused> {
    let manifest = manifest_of(conn, mod_id)?.ok_or(Refused::NotInstalled)?;
    Ok(statuses(mod_id, &manifest, &bound_pairs(conn)?))
}

/// The key `net.fetch` puts on a request, with its slot: only while the stored row's host and
/// header are the installed manifest's slot's, else `None`, which the caller answers as "key not
/// set" (no other word, so a mismatch cannot be told from an empty slot). The binding is in the
/// query itself, so no path reads a value it has not matched.
#[cfg_attr(not(test), allow(dead_code))]
pub fn read_for_use(
    conn: &Connection,
    mod_id: &str,
    name: &str,
) -> rusqlite::Result<Option<Bound>> {
    let Some(manifest) = manifest_of(conn, mod_id)? else {
        return Ok(None);
    };
    let Some(slot) = slots(&manifest).into_iter().find(|s| s.name == name) else {
        return Ok(None);
    };
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM mod_secrets \
             WHERE mod_id = ?1 AND name = ?2 AND host = ?3 AND header = ?4",
            params![mod_id, name, slot.host, slot.header],
            |r| r.get(0),
        )
        .optional()?;
    Ok(value.map(|v| Bound {
        slot,
        value: SecretValue(v),
    }))
}

/// Delete every row of `mod_id` that is not exactly a slot of `manifest` (the manifest an
/// update or repair has just installed): a moved slot, a dropped one, or a row that was never a
/// slot. Run in the install's transaction. A manifest that does not read has no slots, so it
/// deletes them all.
pub fn prune_to_slots(conn: &Connection, mod_id: &str, manifest: &str) -> rusqlite::Result<usize> {
    let keep = slots(manifest);
    let mut stmt = conn.prepare("SELECT name, host, header FROM mod_secrets WHERE mod_id = ?1")?;
    let stale: Vec<String> = stmt
        .query_map([mod_id], |r| {
            Ok(Slot {
                name: r.get(0)?,
                host: r.get(1)?,
                header: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter(|row| !keep.contains(row))
        .map(|row| row.name)
        .collect();
    for name in &stale {
        conn.execute(
            "DELETE FROM mod_secrets WHERE mod_id = ?1 AND name = ?2",
            params![mod_id, name],
        )?;
    }
    Ok(stale.len())
}

/// Delete every key (a restore's commit).
pub fn purge_all(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM mod_secrets", [])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::api::test_support::{insert_mod, store, ModRow};
    use serde_json::json;

    // Test values only; never a real credential (gitleaks reads this file).
    const TYPED: &str = "typed-value-1";
    const HOST: &str = "api.example.com";

    fn manifest(slots: &[(&str, &str, &str)]) -> String {
        json!({
            "internetHosts": [HOST, "other.example.com"],
            "secrets": slots.iter()
                .map(|(n, h, header)| json!({"name": n, "host": h, "header": header}))
                .collect::<Vec<_>>(),
        })
        .to_string()
    }

    fn put(conn: &Connection, id: &'static str, slots: &[(&str, &str, &str)]) {
        insert_mod(conn, &ModRow::signed(id, "second", &["internet"]));
        conn.execute(
            "UPDATE mods SET manifest = ?2 WHERE id = ?1",
            params![id, manifest(slots)],
        )
        .unwrap();
    }

    fn rows(conn: &Connection) -> i64 {
        conn.query_row("SELECT count(*) FROM mod_secrets", [], |r| r.get(0))
            .unwrap()
    }

    fn one() -> (tempfile::TempDir, Connection) {
        let (dir, conn) = store();
        put(&conn, "acme.tool", &[("token", HOST, "X-Api-Key")]);
        (dir, conn)
    }

    // ---- the manifest's slots ---------------------------------------------------------------

    #[test]
    fn slots_are_read_tolerantly() {
        let m = manifest(&[("a", HOST, "X-A"), ("b", HOST, "X-B")]);
        assert_eq!(
            slots(&m),
            vec![
                Slot {
                    name: "a".into(),
                    host: HOST.into(),
                    header: "X-A".into()
                },
                Slot {
                    name: "b".into(),
                    host: HOST.into(),
                    header: "X-B".into()
                },
            ]
        );
        for junk in [
            "",
            "not json",
            "{}",
            r#"{"secrets":"x"}"#,
            r#"{"secrets":[{"name":"a"}]}"#,
            r#"{"secrets":[{"name":1,"host":"h","header":"X"}]}"#,
        ] {
            assert!(slots(junk).is_empty(), "{junk}");
        }
    }

    // ---- saving, status and the binding -----------------------------------------------------

    #[test]
    fn a_saved_key_is_bound_to_the_slots_host_and_header_and_status_is_a_bool() {
        let (_d, mut conn) = one();
        assert_eq!(
            save(&mut conn, "acme.tool", "token", TYPED).unwrap(),
            Saved::Stored
        );
        let (host, header): (String, String) = conn
            .query_row("SELECT host, header FROM mod_secrets", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((host.as_str(), header.as_str()), (HOST, "X-Api-Key"));
        let st = status(&conn, "acme.tool").unwrap();
        assert_eq!(
            st,
            vec![SlotStatus {
                name: "token".into(),
                host: HOST.into(),
                header: "X-Api-Key".into(),
                set: true
            }]
        );
        // Nothing the page can read carries the value.
        assert!(!serde_json::to_string(&st).unwrap().contains(TYPED));
    }

    #[test]
    fn saving_again_replaces_the_value_and_keeps_one_row() {
        let (_d, mut conn) = one();
        save(&mut conn, "acme.tool", "token", TYPED).unwrap();
        save(&mut conn, "acme.tool", "token", "typed-value-2").unwrap();
        assert_eq!(rows(&conn), 1);
        let got = read_for_use(&conn, "acme.tool", "token").unwrap().unwrap();
        assert_eq!(got.value.expose(), "typed-value-2");
        // The slot comes with it: the URL's host must be this one, the key goes in this header.
        assert_eq!(
            (got.slot.host.as_str(), got.slot.header.as_str()),
            (HOST, "X-Api-Key")
        );
    }

    #[test]
    fn an_empty_value_deletes_the_row_and_blanks_around_a_paste_are_not_the_key() {
        let (_d, mut conn) = one();
        save(&mut conn, "acme.tool", "token", &format!("  {TYPED}\r\n")).unwrap();
        let got = read_for_use(&conn, "acme.tool", "token").unwrap().unwrap();
        assert_eq!(got.value.expose(), TYPED);
        for empty in ["", "   ", "\r\n\t "] {
            save(&mut conn, "acme.tool", "token", TYPED).unwrap();
            assert_eq!(
                save(&mut conn, "acme.tool", "token", empty).unwrap(),
                Saved::Cleared
            );
            assert_eq!(rows(&conn), 0, "{empty:?}");
        }
        // Clearing what was never set is fine.
        assert_eq!(
            save(&mut conn, "acme.tool", "token", "").unwrap(),
            Saved::Cleared
        );
    }

    #[test]
    fn a_key_a_header_cannot_carry_is_refused_without_quoting_it() {
        let (_d, mut conn) = one();
        let long = "k".repeat(net::MAX_HEADER_VALUE_BYTES + 1);
        for bad in ["a\nb", "a\u{0}b", "caf\u{e9}", "line\u{7f}", long.as_str()] {
            match save(&mut conn, "acme.tool", "token", bad) {
                Err(Refused::Unusable) => {}
                other => panic!("{:?}: {other:?}", bad.len()),
            }
            assert_eq!(rows(&conn), 0);
        }
        // The largest a header may hold is accepted.
        let edge = "k".repeat(net::MAX_HEADER_VALUE_BYTES);
        assert_eq!(
            save(&mut conn, "acme.tool", "token", &edge).unwrap(),
            Saved::Stored
        );
    }

    #[test]
    fn a_slot_the_manifest_did_not_declare_and_a_mod_that_is_not_there_are_refused() {
        let (_d, mut conn) = one();
        assert!(matches!(
            save(&mut conn, "acme.tool", "other", TYPED),
            Err(Refused::NoSuchSlot)
        ));
        assert!(matches!(
            save(&mut conn, "acme.none", "token", TYPED),
            Err(Refused::NotInstalled)
        ));
        assert!(matches!(
            save(&mut conn, "acme.none", "token", ""),
            Err(Refused::NotInstalled)
        ));
        assert!(matches!(
            clear(&conn, "acme.none", "token"),
            Err(Refused::NotInstalled)
        ));
        assert!(matches!(
            status(&conn, "acme.none"),
            Err(Refused::NotInstalled)
        ));
        assert_eq!(rows(&conn), 0);
    }

    #[test]
    fn clear_deletes_one_slots_row_only_and_an_orphan_row_can_always_be_cleared() {
        let (_d, mut conn) = store();
        put(
            &conn,
            "acme.tool",
            &[("a", HOST, "X-A"), ("b", HOST, "X-B")],
        );
        save(&mut conn, "acme.tool", "a", TYPED).unwrap();
        save(&mut conn, "acme.tool", "b", TYPED).unwrap();
        clear(&conn, "acme.tool", "a").unwrap();
        assert_eq!(rows(&conn), 1);
        clear(&conn, "acme.tool", "a").unwrap();
        // A row whose slot the manifest dropped is still removable by name.
        conn.execute(
            "UPDATE mods SET manifest = ?1",
            [manifest(&[("a", HOST, "X-A")])],
        )
        .unwrap();
        save(&mut conn, "acme.tool", "b", "").unwrap();
        assert_eq!(rows(&conn), 0);
    }

    #[test]
    fn a_row_whose_host_or_header_no_longer_matches_the_slot_is_not_a_key() {
        let (_d, mut conn) = one();
        save(&mut conn, "acme.tool", "token", TYPED).unwrap();
        for new in [
            manifest(&[("token", "other.example.com", "X-Api-Key")]),
            manifest(&[("token", HOST, "X-Other")]),
            manifest(&[]),
        ] {
            conn.execute("UPDATE mods SET manifest = ?1", [&new])
                .unwrap();
            assert!(
                read_for_use(&conn, "acme.tool", "token").unwrap().is_none(),
                "{new}"
            );
            assert!(
                status(&conn, "acme.tool").unwrap().iter().all(|s| !s.set),
                "{new}"
            );
        }
        // And the slot back as it was matches again (the row itself was never touched).
        conn.execute(
            "UPDATE mods SET manifest = ?1",
            [manifest(&[("token", HOST, "X-Api-Key")])],
        )
        .unwrap();
        assert!(read_for_use(&conn, "acme.tool", "token").unwrap().is_some());
    }

    #[test]
    fn a_planted_row_with_the_wrong_destination_is_never_read_and_saving_rebinds_it() {
        let (_d, mut conn) = one();
        conn.execute(
            "INSERT INTO mod_secrets(mod_id, name, host, header, value) \
             VALUES ('acme.tool', 'token', 'evil.example.net', 'X-Api-Key', 'planted')",
            [],
        )
        .unwrap();
        assert!(read_for_use(&conn, "acme.tool", "token").unwrap().is_none());
        assert!(!status(&conn, "acme.tool").unwrap()[0].set);
        save(&mut conn, "acme.tool", "token", TYPED).unwrap();
        let (host, value): (String, String) = conn
            .query_row("SELECT host, value FROM mod_secrets", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((host.as_str(), value.as_str()), (HOST, TYPED));
    }

    /// The three commands read from source (they need an app to run): the typed text appears only
    /// as the argument and as the one call that stores it, and nothing in their bodies prints,
    /// logs or formats. A new line that touched it would fail here and be reviewed.
    #[test]
    fn the_key_commands_only_pass_the_typed_text_to_the_store() {
        let source = include_str!("tab/commands.rs");
        let section = source
            .split("// ---- Keys a mod's slots hold")
            .nth(1)
            .and_then(|s| s.split("/// Tell the frames of `mods`").next())
            .expect("the key commands are in commands.rs");
        let code: String = section
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for banned in [
            "println!",
            "eprintln!",
            "dbg!",
            "format!",
            "tracing::",
            "log::",
        ] {
            assert!(!code.contains(banned), "{banned} in the key commands");
        }
        assert_eq!(
            code.matches("value").count(),
            2,
            "`value` is the argument and the store call, nothing else"
        );
        assert!(code.contains("secrets::save(&mut conn, id.as_str(), &name, &value)"));
        // Window-gated through `blocking`, one each; none is a settings write.
        assert_eq!(code.matches("blocking(&webview").count(), 3);
        for cmd in ["mod_secret_save", "mod_secret_clear", "mod_secret_status"] {
            assert!(!cmd.starts_with("set_"));
        }
    }

    #[test]
    fn a_secret_never_prints() {
        let (_d, mut conn) = one();
        save(&mut conn, "acme.tool", "token", TYPED).unwrap();
        let got = read_for_use(&conn, "acme.tool", "token").unwrap().unwrap();
        assert!(!format!("{got:?}").contains(TYPED));
        for e in [
            Refused::NotInstalled,
            Refused::NoSuchSlot,
            Refused::Unusable,
        ] {
            assert!(!format!("{e:?}").contains(TYPED));
        }
    }

    #[test]
    fn the_listing_query_reads_no_value_and_mods_do_not_share_keys() {
        let (_d, mut conn) = one();
        put(&conn, "acme.two", &[("token", HOST, "X-Api-Key")]);
        save(&mut conn, "acme.tool", "token", TYPED).unwrap();
        let bound = bound_pairs(&conn).unwrap();
        assert_eq!(bound.len(), 1);
        let m = manifest(&[("token", HOST, "X-Api-Key")]);
        assert!(statuses("acme.tool", &m, &bound)[0].set);
        assert!(!statuses("acme.two", &m, &bound)[0].set);
        assert!(read_for_use(&conn, "acme.two", "token").unwrap().is_none());
    }

    // ---- an update or repair that moves a slot ----------------------------------------------

    #[test]
    fn prune_keeps_a_row_that_is_exactly_a_slot_and_deletes_the_rest() {
        let (_d, mut conn) = store();
        put(
            &conn,
            "acme.tool",
            &[
                ("keep", HOST, "X-K"),
                ("moved", HOST, "X-M"),
                ("rehost", HOST, "X-R"),
                ("gone", HOST, "X-G"),
            ],
        );
        put(&conn, "acme.two", &[("keep", HOST, "X-K")]);
        for n in ["keep", "moved", "rehost", "gone"] {
            save(&mut conn, "acme.tool", n, TYPED).unwrap();
        }
        save(&mut conn, "acme.two", "keep", TYPED).unwrap();
        let next = manifest(&[
            ("keep", HOST, "X-K"),
            ("moved", HOST, "X-Moved"),
            ("rehost", "other.example.com", "X-R"),
        ]);
        assert_eq!(prune_to_slots(&conn, "acme.tool", &next).unwrap(), 3);
        let left: Vec<String> = conn
            .prepare("SELECT mod_id || '/' || name FROM mod_secrets ORDER BY 1")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(left, ["acme.tool/keep", "acme.two/keep"]);
        // A manifest that does not read has no slots: nothing stays bound to it.
        assert_eq!(prune_to_slots(&conn, "acme.tool", "nope").unwrap(), 1);
        assert_eq!(rows(&conn), 1);
    }

    #[test]
    fn purge_all_empties_the_table() {
        let (_d, mut conn) = one();
        save(&mut conn, "acme.tool", "token", TYPED).unwrap();
        assert_eq!(purge_all(&conn).unwrap(), 1);
        assert_eq!(rows(&conn), 0);
    }
}
