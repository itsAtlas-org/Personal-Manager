// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What adopting a restored vault (and joining a shared one) does about mods, and what a restore
//! tells the page about them (Mods spec §1.11).
//!
//! **The reset** ([`reset_for_adopt`]) runs on the adopted store's own connection before anything
//! serves from it. A store arrives from somewhere else (a backup from another computer, a vault
//! another account made), and three things in it describe that other place, not this one: the
//! developer-mode switch (so a restore never enables unsigned installs on a machine whose owner
//! didn't), the connected apps (`mcp_clients`: their tokens belong to the device that issued them),
//! and the MCP switch. The cached safety list is the fourth thing a restore must meet before a mod
//! runs, and it needs no step here: the restore hands the backup's copy to the acceptance rules
//! (`mods::refresh::take_restored`), and the writer's open applies the cached list before it loads a
//! row (`ledger::after_writer_open`), which is before any frame can mount.
//!
//! **The report** ([`describe`]) is one row per mod the restored store lists, in the words the
//! restore card shows. It is pending until the open and the same-machine recovery have run (a
//! mod whose files sit in another store of this vault on this computer comes back by itself, after
//! the open command returns), and settled after.

use rusqlite::Connection;
use serde::Serialize;

use crate::db;
use crate::error::Result;
use crate::mods::store;

/// `settings` key of developer mode (`settings::WEBVIEW_PREFS`, `"true"` or `"false"`).
pub const DEV_MODE_KEY: &str = "dev_mode";

/// Reset what a store carries from where it came from (module doc). One transaction: either the
/// store is safe to serve or the caller refuses to adopt it. Any failure is the caller's to treat
/// as "don't adopt".
pub fn reset_for_adopt(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    // Dev mode OFF, written as the row (not just deleted), so the UI's hydration reads "off".
    // Slice 6 part dev-mode: pause the unsigned rows (`state_reason = dev-mode`) and make the UI's
    // authority this row (capabilities.tsx still trusts localStorage over it) right after this.
    db::set_bool(&tx, DEV_MODE_KEY, false)?;
    tx.execute("DELETE FROM mcp_clients", [])?;
    switch_mcp_off(&tx)?;
    tx.commit()?;
    Ok(())
}

/// Switch the MCP server off. PR C adds the switch (its `settings` key, §1.16); until it does there
/// is nothing to turn off, and none is invented here. PR C: write the switch's off value in this
/// function and add its test beside `adopting_turns_dev_mode_off_and_clears_connected_apps`.
fn switch_mcp_off(_conn: &Connection) -> Result<()> {
    Ok(())
}

/// One installed mod as a restored store lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModRow {
    pub id: String,
    pub name: String,
    pub version: String,
    pub tier: String,
    pub signed: bool,
    pub source: String,
    pub state: String,
    pub reason: Option<String>,
}

/// Every mod row of the store, in id order. A failure is the caller's to log: the page then shows
/// no mod list, which is what a vault with no mods shows.
pub fn read_rows(conn: &Connection) -> Result<Vec<ModRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, version, tier, signed, installed_from, state, state_reason \
         FROM mods ORDER BY id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(ModRow {
            id: r.get(0)?,
            name: r.get(1)?,
            version: r.get(2)?,
            tier: r.get(3)?,
            signed: r.get::<_, i64>(4)? != 0,
            source: r.get(5)?,
            state: r.get(6)?,
            reason: r.get(7)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Where a restored mod stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Standing {
    /// Not known yet: the store isn't open, so its files haven't been looked for.
    Pending,
    /// Its files are here and it is on.
    Back,
    /// Its files are here and it is switched off (the user's pause, the safety list's, dev mode's).
    Paused,
    /// It can't run, and `note` says what gets it back.
    Parked,
    /// An install that hadn't finished.
    Installing,
}

/// A restored mod, as the restore card lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RestoredMod {
    pub id: String,
    pub name: String,
    pub version: String,
    pub tier: String,
    pub signed: bool,
    /// `catalogue` or `file`.
    pub source: String,
    pub standing: Standing,
    /// What to do or what happened, or none when it is simply back. For `pending` it is what the
    /// card says should the files not be on this computer.
    pub note: Option<String>,
}

/// Spec §1.11's words for a mod whose files aren't here: a signed one is installed again from the
/// catalogue, an unsigned one only from its file.
pub fn missing_files_note(signed: bool, name: &str) -> String {
    if signed {
        format!("Install {name} to get this back")
    } else {
        "Install the file again to get this back".to_string()
    }
}

/// Why a parked mod is parked, in words, and what gets it back. `reason` is the row's
/// `state_reason` (`store::LEDGER_REASONS` for the ledger's own parks).
fn parked_note(signed: bool, name: &str, reason: Option<&str>) -> String {
    match reason {
        None | Some(store::REASON_FILES_MISSING) => missing_files_note(signed, name),
        Some(store::REASON_FILES_CHANGED) => {
            format!("{name}'s files aren't what was saved. Install it again to get this back")
        }
        Some(store::REASON_RECORD) => {
            format!("{name}'s saved record can't be read. Remove it, then install it again")
        }
        Some(store::REASON_API) => format!("{name} needs a newer PM. Update PM to use it"),
        // A closed room or another owner's reason: its own words.
        Some(other) => format!("{name} is parked: {other}"),
    }
}

/// One row as the card shows it. `settled` is whether the store has been opened and the recovery
/// run; before that nothing is known, only what would apply if the files aren't found.
pub fn describe(row: &ModRow, settled: bool) -> RestoredMod {
    let (standing, note) = if !settled {
        (
            Standing::Pending,
            Some(missing_files_note(row.signed, &row.name)),
        )
    } else {
        match row.state.as_str() {
            store::STATE_ACTIVE => (Standing::Back, None),
            store::STATE_PAUSED => (Standing::Paused, paused_note(row.reason.as_deref())),
            store::STATE_INSTALLING => (
                Standing::Installing,
                Some(format!("{} hadn't finished installing", row.name)),
            ),
            // `parked`, and anything a future state adds: it isn't running, say why.
            _ => (
                Standing::Parked,
                Some(parked_note(row.signed, &row.name, row.reason.as_deref())),
            ),
        }
    };
    RestoredMod {
        id: row.id.clone(),
        name: row.name.clone(),
        version: row.version.clone(),
        tier: row.tier.clone(),
        signed: row.signed,
        source: row.source.clone(),
        standing,
        note,
    }
}

fn paused_note(reason: Option<&str>) -> Option<String> {
    match reason {
        Some(store::REASON_BLACKLIST) => Some("Paused by the safety list".into()),
        Some(store::REASON_BLACKLIST_REMOVED) => Some("Removed by the safety list".into()),
        _ => None,
    }
}

/// [`describe`] for every row.
pub fn describe_all(rows: &[ModRow], settled: bool) -> Vec<RestoredMod> {
    rows.iter().map(|r| describe(r, settled)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_conn() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let key = "ab".repeat(32);
        let conn = db::open(&dir.path().join("pm.sqlite"), &key).unwrap();
        (dir, conn)
    }

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn add(conn: &Connection, id: &str, signed: bool, state: &str, reason: Option<&str>) {
        conn.execute(
            "INSERT INTO mods(id, name, version, tier, author, source_repo, source_commit, signed, \
                              install_id, installed_from, package_sha256, manifest, files, state, \
                              state_reason) \
             VALUES (?1, ?2, '1.0.0', ?3, 'a', 'itsAtlas-org/PM-Mods', \
                     '0123456789abcdef0123456789abcdef01234567', ?4, '0123456789abcdef', ?5, ?6, \
                     '{}', '[]', ?7, ?8)",
            rusqlite::params![
                id,
                format!("{id} name"),
                if signed { "first" } else { "third" },
                signed as i64,
                if signed { "catalogue" } else { "file" },
                SHA,
                state,
                reason
            ],
        )
        .unwrap();
    }

    #[test]
    fn adopting_turns_dev_mode_off_and_clears_connected_apps() {
        let (_d, conn) = store_conn();
        db::set_bool(&conn, DEV_MODE_KEY, true).unwrap();
        conn.execute(
            "INSERT INTO mcp_clients(name, token_hash) VALUES ('one', 'h1'), ('two', 'h2')",
            [],
        )
        .unwrap();
        add(&conn, "atlas.board", true, "active", None);

        reset_for_adopt(&conn).unwrap();

        assert_eq!(
            db::get_setting(&conn, DEV_MODE_KEY).unwrap().as_deref(),
            Some("false"),
            "the row says off, so the page's hydration reads off"
        );
        let clients: i64 = conn
            .query_row("SELECT count(*) FROM mcp_clients", [], |r| r.get(0))
            .unwrap();
        assert_eq!(clients, 0);
        // Nothing else is touched: the installed mod and its state stay.
        assert_eq!(read_rows(&conn).unwrap().len(), 1);
    }

    #[test]
    fn adopting_a_store_that_never_had_dev_mode_still_writes_the_off_row() {
        let (_d, conn) = store_conn();
        assert_eq!(db::get_setting(&conn, DEV_MODE_KEY).unwrap(), None);
        reset_for_adopt(&conn).unwrap();
        assert!(!db::get_bool(&conn, DEV_MODE_KEY, true).unwrap());
    }

    #[test]
    fn a_reset_that_fails_changes_nothing() {
        // Without the table the clear fails: dev mode must not end up switched off in one half of
        // a reset that the caller then treats as failed.
        let (_d, conn) = store_conn();
        db::set_bool(&conn, DEV_MODE_KEY, true).unwrap();
        conn.execute_batch("DROP TABLE mcp_clients").unwrap();
        assert!(reset_for_adopt(&conn).is_err());
        assert!(
            db::get_bool(&conn, DEV_MODE_KEY, false).unwrap(),
            "rolled back"
        );
    }

    fn row(signed: bool, state: &str, reason: Option<&str>) -> ModRow {
        ModRow {
            id: "x.y".into(),
            name: "Board".into(),
            version: "1.0.0".into(),
            tier: "first".into(),
            signed,
            source: "catalogue".into(),
            state: state.into(),
            reason: reason.map(Into::into),
        }
    }

    #[test]
    fn a_missing_signed_mod_says_install_it_and_an_unsigned_one_says_install_the_file() {
        let signed = describe(&row(true, "parked", Some("files-missing")), true);
        assert_eq!(signed.standing, Standing::Parked);
        assert_eq!(
            signed.note.as_deref(),
            Some("Install Board to get this back")
        );
        let unsigned = describe(&row(false, "parked", Some("files-missing")), true);
        assert_eq!(unsigned.standing, Standing::Parked);
        assert_eq!(
            unsigned.note.as_deref(),
            Some("Install the file again to get this back")
        );
        // A parked row with no recorded reason is read as missing files, the common case.
        assert_eq!(
            describe(&row(true, "parked", None), true).note.as_deref(),
            Some("Install Board to get this back")
        );
    }

    #[test]
    fn the_other_parks_and_pauses_each_have_their_own_words() {
        let note = |state: &str, reason: Option<&str>| describe(&row(true, state, reason), true);
        assert!(note("parked", Some("files-changed"))
            .note
            .unwrap()
            .contains("aren't what was saved"));
        assert!(note("parked", Some("record-unreadable"))
            .note
            .unwrap()
            .contains("can't be read"));
        assert!(note("parked", Some("api-unsupported"))
            .note
            .unwrap()
            .contains("newer PM"));
        assert_eq!(
            note("parked", Some("requires: localai")).note.as_deref(),
            Some("Board is parked: requires: localai")
        );
        let back = note("active", None);
        assert_eq!((back.standing, back.note), (Standing::Back, None));
        let paused = note("paused", Some("user"));
        assert_eq!((paused.standing, paused.note), (Standing::Paused, None));
        assert_eq!(
            note("paused", Some("blacklist")).note.as_deref(),
            Some("Paused by the safety list")
        );
        assert_eq!(
            note("paused", Some("blacklist-removed")).note.as_deref(),
            Some("Removed by the safety list")
        );
        assert_eq!(note("installing", None).standing, Standing::Installing);
    }

    #[test]
    fn before_the_open_every_mod_is_pending_with_the_text_for_missing_files() {
        // The card can't know yet whether a mod's files are on this computer (the recovery runs
        // after the open), so it never claims a mod is back, or parked, before then.
        for state in ["active", "parked", "paused"] {
            let m = describe(&row(true, state, None), false);
            assert_eq!(m.standing, Standing::Pending, "{state}");
            assert_eq!(m.note.as_deref(), Some("Install Board to get this back"));
        }
        let m = describe(&row(false, "active", None), false);
        assert_eq!(
            m.note.as_deref(),
            Some("Install the file again to get this back")
        );
    }

    #[test]
    fn the_rows_are_read_with_their_state_and_in_id_order() {
        let (_d, conn) = store_conn();
        add(&conn, "z.last", false, "parked", Some("files-missing"));
        add(&conn, "a.first", true, "active", None);
        let rows = read_rows(&conn).unwrap();
        assert_eq!(
            rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["a.first", "z.last"]
        );
        assert!(rows[0].signed && !rows[1].signed);
        assert_eq!(rows[1].reason.as_deref(), Some("files-missing"));
        assert_eq!(describe_all(&rows, true).len(), 2);
    }

    #[test]
    fn the_standing_words_are_the_wire_words() {
        assert_eq!(
            serde_json::to_string(&Standing::Pending).unwrap(),
            "\"pending\""
        );
        assert_eq!(serde_json::to_string(&Standing::Back).unwrap(), "\"back\"");
        assert_eq!(
            serde_json::to_string(&Standing::Paused).unwrap(),
            "\"paused\""
        );
        assert_eq!(
            serde_json::to_string(&Standing::Parked).unwrap(),
            "\"parked\""
        );
        assert_eq!(
            serde_json::to_string(&Standing::Installing).unwrap(),
            "\"installing\""
        );
    }
}
