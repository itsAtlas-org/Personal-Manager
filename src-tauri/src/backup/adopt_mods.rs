// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What adopting a restored vault does about mods, what joining a shared one doesn't, and what a
//! restore tells the page about them (Mods spec §1.11).
//!
//! **The restore's reset** ([`reset_for_adopt`]) runs on the restored store's own connection
//! before it is attached, so before anything serves from it. A backup comes from somewhere else
//! (another computer, or this one at another time), and three things in it describe that other
//! place, not this one: the developer-mode switch (so a restore never enables unsigned installs on
//! a machine whose owner didn't), the connected apps (`mcp_clients`: their tokens belong to the
//! device that issued them), and the MCP switch. The cached safety list is the fourth thing a
//! restore must meet before a mod runs, and it needs no step here: the restore hands the backup's
//! copy to the acceptance rules (`mods::refresh::take_restored`), and the writer's open applies the
//! cached list before it loads a row (`ledger::after_writer_open`), which is before any frame can
//! mount.
//!
//! **Joining a shared vault resets nothing.** Its store is the vault's, in use by the vault's other
//! accounts and machines at the same time, so writing the dev-mode row or deleting the connected
//! apps there would switch dev mode off for them and disconnect their apps. Dev mode and connected
//! apps are per device, and each has a seam where its owner makes that true:
//! [`joiner_dev_mode_is_per_device`] and [`joiner_accepts_only_its_own_clients`]. Neither takes the
//! store, so neither can write it.
//!
//! **The report** ([`describe`]) is one row per mod the restored store lists, in the words the
//! restore card shows. It is pending until the open and the same-machine recovery have run (a
//! mod whose files sit in another store of this vault on this computer comes back by itself, after
//! the open command returns), and settled after ([`settled`]), where "back" means the registry
//! serves it, not merely that its row says `active`.

use rusqlite::Connection;
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::db;
use crate::error::Result;
use crate::mods::ids::ModId;
use crate::mods::ledger::ServeCheck;
use crate::mods::runtime::ModRuntime;
use crate::mods::safety::SafetyCheck;
use crate::mods::store;
use crate::AppState;

/// `settings` key of developer mode (`settings::WEBVIEW_PREFS`, `"true"` or `"false"`).
pub const DEV_MODE_KEY: &str = "dev_mode";

/// Reset what a restored store carries from where it came from (module doc). One transaction:
/// either the store is safe to serve or the caller refuses to adopt it. Any failure is the
/// caller's to treat as "don't adopt". A restore's commit only: never a shared vault's join.
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

/// Slice 6 part dev-mode: a joiner's dev mode is off because dev mode is per device. That part makes
/// dev mode "on for this device in this vault", so a device that has just joined has no entry and
/// reads as off, while the vault's other devices keep theirs; nothing in the shared store is
/// written at the join. Until it lands the `dev_mode` row is the vault's, and the page's switch
/// reads this device's own `localStorage` first (`capabilities.tsx`). Called by
/// `commands::vaults::adopt_shared_vault`; takes no store on purpose.
pub fn joiner_dev_mode_is_per_device() {}

/// PR C: `mcp_clients` are per device; a joiner never accepts another device's clients. PR C binds
/// each connected app to the device that issued its token, and the MCP server here accepts only
/// this device's, so the rows other devices made in the shared store stay theirs and are never
/// deleted by a join. Called by `commands::vaults::adopt_shared_vault`; takes no store on purpose.
pub fn joiner_accepts_only_its_own_clients() {}

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
    /// `consented_at` is set: false only for an auto-installed pilot whose first-open screen hasn't
    /// been accepted (D17), whose frame waits for it.
    pub consented: bool,
    /// What the safety list's entries are matched against, with the id and version.
    pub package_sha256: Option<String>,
}

/// Every mod row of the store, in id order. A failure is the caller's to log: the page then shows
/// no mod list, which is what a vault with no mods shows.
pub fn read_rows(conn: &Connection) -> Result<Vec<ModRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, version, tier, signed, installed_from, state, state_reason, \
                consented_at IS NOT NULL, package_sha256 \
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
            consented: r.get(8)?,
            package_sha256: r.get(9)?,
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
    /// Its files are here and it runs: the registry serves it.
    Back,
    /// Its files are here and it is switched off (the user's pause, the safety list's, dev mode's).
    Paused,
    /// It can't run, and `note` says what gets it back.
    Parked,
    /// An install that hadn't finished.
    Installing,
    /// Its files are here and it is on, but it waits for the user to open it the first time (an
    /// auto-installed pilot, D17).
    Waiting,
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
    /// What to do or what happened, or none when it is simply back, or still pending (nothing is
    /// known before the open, and the card says so in its own words).
    pub note: Option<String>,
}

/// What the open store's load made of a row, read after it and the same-machine recovery.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Live {
    /// The registry serves it.
    pub loaded: bool,
    /// The safety list holds it back, in the list's words (`safety::blacklist_words`).
    pub held: Option<String>,
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

/// An `active` row, by what the load made of it: back only when the registry serves it. Held by the
/// safety list first (that is the reason it would not run even once opened), then waiting for its
/// first open, then a load that didn't take it for a reason the row doesn't record.
fn active_standing(row: &ModRow, live: &Live) -> (Standing, Option<String>) {
    if live.loaded {
        (Standing::Back, None)
    } else if let Some(why) = &live.held {
        (Standing::Paused, Some(format!("Held back by {why}")))
    } else if !row.consented {
        (
            Standing::Waiting,
            Some("Waiting for you to open it".to_string()),
        )
    } else {
        (
            Standing::Parked,
            Some(format!(
                "{} didn't start on this computer. Restarting PM tries it again",
                row.name
            )),
        )
    }
}

/// One row as the card shows it. `live` is what the open store's load made of it, `None` before
/// the store is opened, when nothing is known.
pub fn describe(row: &ModRow, live: Option<&Live>) -> RestoredMod {
    let (standing, note) = match live {
        None => (Standing::Pending, None),
        Some(live) => match row.state.as_str() {
            store::STATE_ACTIVE => active_standing(row, live),
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
        },
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

/// Every row before the restored store is opened: all pending.
pub fn pending(rows: &[ModRow]) -> Vec<RestoredMod> {
    rows.iter().map(|r| describe(r, None)).collect()
}

/// Every row of the open store, settled: read once its load and the same-machine recovery have run
/// (`switch_to_vault`, after `ledger::recover_now`), each against the registry and the cached safety
/// list. Under the ledger lock (the ledger's own order: that lock, then the DB's), so the rows and
/// the registry are the ones a single load left; the safety list is read before either lock.
/// Blocking.
pub fn settled(app: &AppHandle) -> Result<Vec<RestoredMod>> {
    let cached = crate::mods::refresh::cached_index(app);
    let check = SafetyCheck::serve(cached.as_ref());
    let runtime = app.try_state::<ModRuntime>();
    let _ledger = runtime.as_ref().map(|r| r.ledger_lock());
    let rows = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        read_rows(&conn)?
    };
    Ok(rows
        .iter()
        .map(|row| {
            let loaded = runtime
                .as_ref()
                .is_some_and(|r| ModId::parse(&row.id).is_ok_and(|id| r.is_installed(&id)));
            let held = check.hold(&row.id, &row.version, row.package_sha256.as_deref());
            describe(row, Some(&Live { loaded, held }))
        })
        .collect())
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
                              state_reason, consented_at) \
             VALUES (?1, ?2, '1.0.0', ?3, 'a', 'itsAtlas-org/PM-Mods', \
                     '0123456789abcdef0123456789abcdef01234567', ?4, '0123456789abcdef', ?5, ?6, \
                     '{}', '[]', ?7, ?8, ?9)",
            rusqlite::params![
                id,
                format!("{id} name"),
                if signed { "first" } else { "third" },
                signed as i64,
                if signed { "catalogue" } else { "file" },
                SHA,
                state,
                reason,
                // The signed one is consented, the unsigned one an unaccepted pilot's row.
                signed.then_some("2026-11-11T00:00:00.000Z"),
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

    /// Production code of a source file: everything before its test module, less comment lines.
    fn production(source: &str) -> String {
        source
            .split("#[cfg(test)]")
            .next()
            .unwrap()
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn joining_a_shared_vault_writes_nothing_and_a_restore_resets_before_it_attaches() {
        // The join's command can't run without an app, so its source is read: it must not reset the
        // shared store (that switched dev mode off for, and disconnected the apps of, the vault's
        // other accounts and machines), and it goes through the two per-device seams instead.
        let vaults = production(include_str!("../commands/vaults.rs"));
        let join = vaults
            .split("pub fn adopt_shared_vault(")
            .nth(1)
            .and_then(|rest| rest.split("\n}\n").next())
            .expect("adopt_shared_vault is in vaults.rs");
        assert!(
            !vaults.contains("reset_for_adopt("),
            "no vault command resets a store it shares"
        );
        assert!(!join.contains("mcp_clients") && !join.contains("DEV_MODE_KEY"));
        assert!(join.contains("adopt_mods::joiner_dev_mode_is_per_device()"));
        assert!(join.contains("adopt_mods::joiner_accepts_only_its_own_clients()"));
        // The restore's commit resets, once, and before the store is attached (so before the
        // engage that loads its mods).
        let backups = production(include_str!("../commands/backups.rs"));
        let switch = backups
            .split("pub async fn switch_to_vault(")
            .nth(1)
            .and_then(|rest| rest.split("\n}\n").next())
            .expect("switch_to_vault is in backups.rs");
        assert_eq!(backups.matches("reset_for_adopt(").count(), 1);
        let reset = switch.find("reset_for_adopt(").expect("the restore resets");
        let attach = switch.find("attach_profile_here(").expect("and attaches");
        assert!(reset < attach, "the reset comes before the attach");
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
            consented: true,
            package_sha256: Some(SHA.into()),
        }
    }

    /// What a load makes of a row that isn't served (every state but a running `active` one).
    fn not_loaded() -> Live {
        Live::default()
    }

    fn running() -> Live {
        Live {
            loaded: true,
            held: None,
        }
    }

    #[test]
    fn a_missing_signed_mod_says_install_it_and_an_unsigned_one_says_install_the_file() {
        let live = not_loaded();
        let signed = describe(&row(true, "parked", Some("files-missing")), Some(&live));
        assert_eq!(signed.standing, Standing::Parked);
        assert_eq!(
            signed.note.as_deref(),
            Some("Install Board to get this back")
        );
        let unsigned = describe(&row(false, "parked", Some("files-missing")), Some(&live));
        assert_eq!(unsigned.standing, Standing::Parked);
        assert_eq!(
            unsigned.note.as_deref(),
            Some("Install the file again to get this back")
        );
        // A parked row with no recorded reason is read as missing files, the common case.
        assert_eq!(
            describe(&row(true, "parked", None), Some(&live))
                .note
                .as_deref(),
            Some("Install Board to get this back")
        );
    }

    #[test]
    fn the_other_parks_and_pauses_each_have_their_own_words() {
        let note = |state: &str, reason: Option<&str>| {
            describe(&row(true, state, reason), Some(&not_loaded()))
        };
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
        let back = describe(&row(true, "active", None), Some(&running()));
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
    fn an_active_row_is_back_only_when_the_registry_serves_it() {
        // `active` in the row is not "it runs": an unaccepted pilot (no consent yet), a row the
        // safety list holds at load, or one the load didn't take are all active and not served.
        let active = row(true, "active", None);
        let back = describe(&active, Some(&running()));
        assert_eq!((back.standing, back.note), (Standing::Back, None));

        let pilot = ModRow {
            consented: false,
            ..active.clone()
        };
        let waiting = describe(&pilot, Some(&not_loaded()));
        assert_eq!(waiting.standing, Standing::Waiting);
        assert_eq!(waiting.note.as_deref(), Some("Waiting for you to open it"));

        let held = Live {
            loaded: false,
            held: Some("PM's safety list: leaks data".into()),
        };
        let m = describe(&active, Some(&held));
        assert_eq!(m.standing, Standing::Paused);
        assert_eq!(
            m.note.as_deref(),
            Some("Held back by PM's safety list: leaks data")
        );
        // The safety list's hold is said even for a pilot not yet opened: it wouldn't run then.
        assert_eq!(describe(&pilot, Some(&held)).standing, Standing::Paused);

        let unknown = describe(&active, Some(&not_loaded()));
        assert_eq!(unknown.standing, Standing::Parked);
        assert!(unknown.note.unwrap().contains("didn't start"));
    }

    #[test]
    fn before_the_open_every_mod_is_pending_and_says_nothing_of_its_own() {
        // The card can't know yet whether a mod's files are on this computer (the recovery runs
        // after the open), so it never claims a mod is back, or that it needs installing, before
        // then: a same-machine restore brings most of them back by themselves.
        for state in ["active", "parked", "paused"] {
            for signed in [true, false] {
                let m = describe(&row(signed, state, None), None);
                assert_eq!(m.standing, Standing::Pending, "{state}");
                assert_eq!(m.note, None, "{state}");
            }
        }
        assert!(pending(&[row(true, "active", None)])
            .iter()
            .all(|m| m.standing == Standing::Pending));
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
        assert!(rows[0].consented && !rows[1].consented);
        assert_eq!(rows[0].package_sha256.as_deref(), Some(SHA));
        assert_eq!(pending(&rows).len(), 2);
    }

    #[test]
    fn the_standing_words_are_the_wire_words() {
        for (standing, word) in [
            (Standing::Pending, "pending"),
            (Standing::Back, "back"),
            (Standing::Paused, "paused"),
            (Standing::Parked, "parked"),
            (Standing::Installing, "installing"),
            (Standing::Waiting, "waiting"),
        ] {
            assert_eq!(
                serde_json::to_string(&standing).unwrap(),
                format!("\"{word}\"")
            );
        }
    }
}
