// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What adopting a restored vault does about mods, what joining a shared one doesn't, and what a
//! restore tells the page about them (Mods spec §1.11).
//!
//! **The restore's reset** ([`reset_for_adopt`]) runs on the restored store's own connection
//! before it is attached, so before anything serves from it. A backup comes from somewhere else
//! (another computer, or this one at another time), and three things in it describe that other
//! place, not this one: developer mode (so a restore never enables unsigned installs on a machine
//! whose owner didn't: [`devmode::all_devices_off`] empties the per-device record, forgets the
//! vault in this device's own record and turns the old row off, and the load that follows the store's open pauses the unsigned mods, INVARIANTS.md
//! I-34), the connected apps (`mcp_clients`: their tokens belong to the device that issued them),
//! and the MCP switch. A fourth is not believed at all: the mods' typed keys (`mod_secrets`; a
//! backup is written without them, so any row in a restored store was planted, and the restore
//! deletes it). The cached safety list is the fifth thing a
//! restore must meet before a mod runs, and it needs no step here: the restore hands the backup's
//! copy to the acceptance rules (`mods::refresh::take_restored`), and the writer's open applies the
//! cached list before it loads a row (`ledger::after_writer_open`), which is before any frame can
//! mount.
//!
//! **Joining a shared vault writes nothing.** Its store is the vault's, in use by the vault's
//! other accounts and machines at the same time, so switching dev mode off or deleting the
//! connected apps or the mods' keys there would do it for them (the keys are the vault's own, typed
//! by whoever can open it). Dev mode is per device (a joiner's device has no
//! entry in the record, so it reads off, and nothing else ever switches it on: the old row is
//! never read, `mods::devmode`), and the connected apps have a seam where PR C makes that true
//! ([`joiner_accepts_only_its_own_clients`], which takes no store, so it cannot write one). The
//! join has no dev-mode step at all; `a_join_names_no_developer_mode_code_and_a_restore_resets_before_it_attaches`
//! reads its source to keep it so.
//!
//! **The report** ([`describe`]) is one row per mod the restored store lists, in the words the
//! restore card shows. It is pending until the open and the same-machine recovery have run (a
//! mod whose files sit in another store of this vault on this computer comes back by itself, after
//! the open command returns), and settled after ([`settled`]), where "back" means the registry
//! serves it, not merely that its row says `active`.

use std::path::Path;

use rusqlite::Connection;
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::error::Result;
use crate::mods::autoinstall;
use crate::mods::devmode;
use crate::mods::ids::ModId;
use crate::mods::ledger::ServeCheck;
use crate::mods::runtime::ModRuntime;
use crate::mods::safety::SafetyCheck;
use crate::mods::secrets;
use crate::mods::store;
use crate::AppState;

/// Reset what a restored store carries from where it came from (module doc). One transaction:
/// either the store is safe to serve or the caller refuses to adopt it. Any failure is the
/// caller's to treat as "don't adopt". A restore's commit only: never a shared vault's join.
pub fn reset_for_adopt(conn: &Connection, data_dir: &Path, vault_id: &str) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM mcp_clients", [])?;
    // The typed keys. A backup is written without them (`backup::strip`), so a row here came from
    // somewhere that is not this person's own typing: a crafted file, or a copy made before the
    // strip existed. Either way it would be sent to a host as the user's credential, so none is
    // believed; the person types the keys again, as the restore card says.
    secrets::purge_all(&tx)?;
    switch_mcp_off(&tx)?;
    // The mod's files are per computer and a backup carries none. A pilot (the Document Map) whose
    // catalogue row came with the backup is owed its files here, as it was owed them at the
    // migration: the marker says so, and the open's pass puts them back (`autoinstall`), so a
    // restore onto a new computer gets the Map back without a click, as it did when it was built in.
    autoinstall::mark_restored(&tx)?;
    // A backup taken after the Pinboard moved into its mod, with the mod's data left out, holds the
    // board only as the legacy snapshot of when it moved. Owing the move again, in this same
    // transaction, is what brings that board back into the mod (`autoinstall::rearm_pinboard` says
    // when it fires and what it writes). Only a restore does this; a join writes nothing.
    autoinstall::rearm_pinboard(&tx)?;
    // Dev mode off for EVERY device: an empty per-device record and the old row off (this PM never
    // reads the old row; the write is for an older PM sharing the vault). The unsigned rows need no
    // write here: the load that follows the store's open reads the record first and pauses each
    // active unsigned row with the reason `dev-mode` (`ledger::after_writer_open`), the same pause
    // switching it off makes; the page asks the backend (`dev_mode_state`), not its own copy. This
    // device's own record of the vault goes too (`data_dir`: a restore keeps the vault id, so the
    // file would otherwise still say on). Last, so a reset that fails earlier changes nothing here;
    // one that can't forget fails and is rolled back, and the restore is refused.
    devmode::all_devices_off(&tx, data_dir, vault_id).map_err(|e| {
        crate::error::Error::Other(format!("developer mode couldn't be reset ({e})"))
    })?;
    tx.commit()?;
    Ok(())
}

/// Switch the MCP server off. PR C adds the switch (its `settings` key, §1.16); until it does there
/// is nothing to turn off, and none is invented here. PR C: write the switch's off value in this
/// function and add its test beside `adopting_turns_dev_mode_off_and_clears_connected_apps`.
fn switch_mcp_off(_conn: &Connection) -> Result<()> {
    Ok(())
}

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
        // Every restored unsigned mod that was running ends here (the restore turns developer mode
        // off), so the card has to say what brings it back: the switch is Settings > Developer.
        Some(store::REASON_DEV_MODE) => Some(
            "Paused because developer mode is off on this computer. Switch it on in Settings, under Developer, to use it again"
                .into(),
        ),
        Some(store::REASON_FROZEN) => Some(
            "Paused because it stopped responding. Resume it from the Mods tab when you want to try it again"
                .into(),
        ),
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
    use crate::db;
    use crate::mods::pinboard_move;

    fn store_conn() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let key = "ab".repeat(32);
        let conn = db::open(&dir.path().join("pm.sqlite"), &key).unwrap();
        (dir, conn)
    }

    const VAULT: &str = "vault-1";

    /// Where device `id` keeps its record: a folder of its own (two devices are two machines).
    fn dir_of(dir: &tempfile::TempDir, id: &str) -> std::path::PathBuf {
        let own = dir.path().join(id);
        std::fs::create_dir_all(&own).unwrap();
        own
    }

    /// A device of the test vault.
    fn here(dir: &tempfile::TempDir, id: &str) -> devmode::Here {
        devmode::Here::new(id, VAULT, &dir_of(dir, id))
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

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    /// The old webview-written row. Named by its key here, in test code only (the tripwire below
    /// reads production code).
    const OLD_ROW: &str = "dev_mode";

    fn set_legacy(conn: &Connection, value: &str) {
        conn.execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![OLD_ROW, value],
        )
        .unwrap();
    }

    fn legacy(conn: &Connection) -> Option<String> {
        conn.query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [OLD_ROW],
            |r| r.get(0),
        )
        .ok()
    }

    fn record(conn: &Connection) -> Option<String> {
        conn.query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [store::SETTING_DEV_DEVICES],
            |r| r.get(0),
        )
        .ok()
    }

    #[test]
    fn adopting_turns_developer_mode_off_for_every_device_and_clears_connected_apps() {
        let (_d, mut conn) = store_conn();
        set_legacy(&conn, "true");
        devmode::write(&mut conn, &here(&_d, A), true).unwrap();
        devmode::write(&mut conn, &here(&_d, B), true).unwrap();
        conn.execute(
            "INSERT INTO mcp_clients(name, token_hash) VALUES ('one', 'h1'), ('two', 'h2')",
            [],
        )
        .unwrap();
        add(&conn, "atlas.board", true, "active", None);

        reset_for_adopt(&conn, &dir_of(&_d, A), VAULT).unwrap();

        // The restored store's record named this machine (or the one the backup came from): both
        // are off now, whatever the backup said.
        assert!(!devmode::is_on(&conn, &here(&_d, A)).unwrap());
        assert!(!devmode::is_on(&conn, &here(&_d, B)).unwrap());
        assert_eq!(legacy(&conn).as_deref(), Some("false"));
        let clients: i64 = conn
            .query_row("SELECT count(*) FROM mcp_clients", [], |r| r.get(0))
            .unwrap();
        assert_eq!(clients, 0);
        // Nothing else is touched: the installed mod and its state stay.
        assert_eq!(read_rows(&conn).unwrap().len(), 1);
    }

    /// The restore keeps the vault id, so the machine that switched developer mode on still holds
    /// its own record of it: the reset forgets it, and a credential written back into the restored
    /// store afterwards (from a copy of the backup, say) switches nothing on.
    #[test]
    fn a_restore_makes_the_device_forget_so_a_replayed_credential_stays_off() {
        let (_d, mut conn) = store_conn();
        let me = here(&_d, A);
        devmode::write(&mut conn, &me, true).unwrap();
        let kept = record(&conn).unwrap();
        reset_for_adopt(&conn, &dir_of(&_d, A), VAULT).unwrap();
        conn.execute(
            "UPDATE settings SET value = ?1 WHERE key = ?2",
            rusqlite::params![kept, store::SETTING_DEV_DEVICES],
        )
        .unwrap();
        assert!(!devmode::is_on(&conn, &me).unwrap());
    }

    /// A reset that cannot make this device forget is refused, and the store is left as it was.
    #[test]
    fn a_reset_that_cannot_clear_the_devices_record_fails_and_changes_nothing() {
        let (_d, mut conn) = store_conn();
        let me = here(&_d, A);
        devmode::write(&mut conn, &me, true).unwrap();
        let before = record(&conn);
        let file = dir_of(&_d, A).join(devmode::DEVICE_FILE);
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        assert!(reset_for_adopt(&conn, &dir_of(&_d, A), VAULT).is_err());
        assert_eq!(record(&conn), before, "rolled back");
    }

    /// A backup carries a mod's row and none of its files, so a restore owes the Document Map its
    /// files on this computer: the marker is written for a signed catalogue row of the pilot, and
    /// for nothing else (a mod the person removed, an unsigned file, another mod).
    #[test]
    fn adopting_marks_a_restored_signed_map_row_as_owed_its_files_and_nothing_else() {
        let marker = |conn: &Connection| -> bool {
            conn.query_row(
                "SELECT count(*) FROM settings WHERE key = 'mods_autoinstall_map'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
                > 0
        };
        let (d, conn) = store_conn();
        add(&conn, "atlas.map", true, "active", None);
        reset_for_adopt(&conn, &dir_of(&d, A), VAULT).unwrap();
        assert!(marker(&conn), "a signed catalogue row of the pilot");

        let (d, conn) = store_conn();
        reset_for_adopt(&conn, &dir_of(&d, A), VAULT).unwrap();
        assert!(
            !marker(&conn),
            "no row: the person removed it, or never had it"
        );

        let (d, conn) = store_conn();
        add(&conn, "atlas.board", true, "active", None);
        reset_for_adopt(&conn, &dir_of(&d, A), VAULT).unwrap();
        assert!(!marker(&conn), "another mod");

        let (d, conn) = store_conn();
        add(&conn, "atlas.map", false, "active", None);
        reset_for_adopt(&conn, &dir_of(&d, A), VAULT).unwrap();
        assert!(!marker(&conn), "an unsigned file is not the pilot");
    }

    // ---- The Pinboard's re-arm (Q4): a restore brings back the pre-move board ---------------------

    const PIN: &str = "atlas.pinboard";
    /// The board as the legacy row holds it: a timeline bound to a project, and a freeform one with
    /// one dated entry (the Calendar's dots come from freeform timelines).
    const OLD_BOARD: &str = r#"{"version":1,"widgets":[{"id":"t","kind":"timeline","project":"Atlas"},{"id":"f","kind":"timeline","items":[{"id":"i","label":"Ship","date":"2026-12-01"}]}]}"#;
    const MOVED_SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn put(conn: &Connection, key: &str, value: &str) {
        conn.execute(
            "INSERT OR REPLACE INTO settings(key, value) VALUES (?1, ?2)",
            rusqlite::params![key, value],
        )
        .unwrap();
    }

    fn setting_text(conn: &Connection, key: &str) -> Option<String> {
        use rusqlite::OptionalExtension;
        // Read as bytes: a planted BLOB that is not UTF-8 must not panic the helper.
        conn.query_row(
            "SELECT CAST(value AS BLOB) FROM settings WHERE key = ?1",
            [key],
            |r| r.get::<_, Vec<u8>>(0),
        )
        .optional()
        .unwrap()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    fn legacy_board_bytes(conn: &Connection) -> Option<Vec<u8>> {
        use rusqlite::OptionalExtension;
        conn.query_row(
            "SELECT CAST(value AS BLOB) FROM settings WHERE key = 'pinboard'",
            [],
            |r| r.get(0),
        )
        .optional()
        .unwrap()
    }

    /// A restored store as a mod-data-off backup leaves it: the board moved once (the record is
    /// there, with stale strip notes), the legacy row holds the snapshot, the signed catalogue
    /// Pinboard row came along, and the mod's own `board` did not.
    fn moved_without_mod_data() -> (tempfile::TempDir, Connection) {
        let (d, conn) = store_conn();
        add(&conn, PIN, true, "active", None);
        put(&conn, "pinboard", OLD_BOARD);
        put(&conn, pinboard_move::MOVED, MOVED_SHA);
        put(&conn, pinboard_move::CONFLICT, MOVED_SHA);
        put(&conn, pinboard_move::REPAIRED, "1");
        (d, conn)
    }

    fn fired(conn: &Connection) -> bool {
        setting_text(conn, pinboard_move::PENDING).is_some()
    }

    /// Every key the re-arm may write or delete, as the store holds them.
    fn rearm_keys(conn: &Connection) -> Vec<Option<String>> {
        [
            pinboard_move::PENDING,
            pinboard_move::MOVED,
            pinboard_move::CONFLICT,
            pinboard_move::REPAIRED,
            pinboard_move::RESTORED,
        ]
        .iter()
        .map(|k| setting_text(conn, k))
        .collect()
    }

    #[test]
    fn a_restore_without_the_mods_data_owes_the_move_again_and_changes_nothing_else() {
        let (d, conn) = moved_without_mod_data();
        reset_for_adopt(&conn, &dir_of(&d, A), VAULT).unwrap();
        assert_eq!(
            setting_text(&conn, pinboard_move::PENDING).as_deref(),
            Some("1"),
            "the copy is owed again"
        );
        assert_eq!(setting_text(&conn, pinboard_move::MOVED), None);
        assert_eq!(
            setting_text(&conn, pinboard_move::CONFLICT),
            None,
            "a stale conflict describes a board that is not in this copy"
        );
        assert_eq!(setting_text(&conn, pinboard_move::REPAIRED), None);
        assert_eq!(
            setting_text(&conn, pinboard_move::RESTORED).as_deref(),
            Some("1")
        );
        assert_eq!(
            setting_text(&conn, "mods_autoinstall_pinboard").as_deref(),
            Some("1"),
            "the marker the pass acts on is there too"
        );
        assert_eq!(
            legacy_board_bytes(&conn).as_deref(),
            Some(OLD_BOARD.as_bytes()),
            "the legacy row is byte for byte what it was"
        );
        let kv: i64 = conn
            .query_row("SELECT count(*) FROM mod_kv", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kv, 0, "nothing was written to the mod's storage");
    }

    #[test]
    fn the_rearm_returns_the_store_to_the_pre_move_state_for_renames_and_the_calendar() {
        // With the record of the move gone, the legacy row is live again: a rename rewrites it (so
        // the copy takes the renamed board) and the Calendar reads its dots, as before the move.
        let (d, conn) = moved_without_mod_data();
        reset_for_adopt(&conn, &dir_of(&d, A), VAULT).unwrap();
        assert!(!crate::projects::pinboard_move_recorded(&conn).unwrap());
        let dots = crate::pinboard_legacy::calendar_entries(&conn).unwrap();
        assert_eq!(dots.len(), 1, "the Calendar reads the legacy board again");
        crate::projects::rename_project_satellites(&conn, "Atlas", "Atlas 2").unwrap();
        let text = String::from_utf8(legacy_board_bytes(&conn).unwrap()).unwrap();
        let board: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(board["widgets"][0]["project"], "Atlas 2");
    }

    /// Each store a restore meets that must NOT be re-armed: the record of the move, the strip
    /// notes and the legacy row all stay as they were, and nothing is pending.
    #[test]
    fn the_rearm_stays_off_unless_every_condition_holds() {
        type Break = fn(&Connection);
        let cases: [(&str, Break); 12] = [
            ("a backup with the mod's data: the board is there", |c| {
                c.execute(
                    "INSERT INTO mod_kv(mod_id, key, class, value) \
                     VALUES ('atlas.pinboard', 'board', 'content', '{}')",
                    [],
                )
                .unwrap();
            }),
            ("a backup from before the move: no record", |c| {
                c.execute("DELETE FROM settings WHERE key = 'pinboard_moved_sha'", [])
                    .unwrap();
            }),
            ("the person removed the mod: no row", |c| {
                c.execute("DELETE FROM mods WHERE id = 'atlas.pinboard'", [])
                    .unwrap();
            }),
            ("an unsigned file install is not the Pinboard", |c| {
                c.execute(
                    "UPDATE mods SET signed = 0, installed_from = 'file' WHERE id = 'atlas.pinboard'",
                    [],
                )
                .unwrap();
            }),
            (
                "a catalogue row that is not signed is not the Pinboard",
                |c| {
                    c.execute(
                        "UPDATE mods SET signed = 0 WHERE id = 'atlas.pinboard' \
                     AND installed_from = 'catalogue'",
                        [],
                    )
                    .unwrap();
                },
            ),
            ("a developer's test folder is not the Pinboard", |c| {
                c.execute(
                    "UPDATE mods SET installed_from = 'folder', package_sha256 = NULL \
                     WHERE id = 'atlas.pinboard'",
                    [],
                )
                .unwrap();
            }),
            ("no legacy row: no board to give back", |c| {
                c.execute("DELETE FROM settings WHERE key = 'pinboard'", [])
                    .unwrap();
            }),
            ("a blank legacy row", |c| put(c, "pinboard", " \n\t ")),
            // The move would refuse these as unreadable, so the copy could never land: the record of
            // the move must stay, and no note is owed.
            ("a legacy row that is not text", |c| {
                c.execute(
                    "UPDATE settings SET value = x'ff00fe' WHERE key = 'pinboard'",
                    [],
                )
                .unwrap();
            }),
            ("a legacy row that is not JSON", |c| {
                put(c, "pinboard", "this is not a board");
            }),
            ("a blank record of the move", |c| {
                put(c, pinboard_move::MOVED, "  ");
            }),
            ("a record of the move that is not text", |c| {
                c.execute(
                    "UPDATE settings SET value = x'ff00fe' WHERE key = 'pinboard_moved_sha'",
                    [],
                )
                .unwrap();
            }),
        ];
        for (what, break_it) in cases {
            let (d, conn) = moved_without_mod_data();
            break_it(&conn);
            let before = rearm_keys(&conn);
            let legacy_before = legacy_board_bytes(&conn);
            reset_for_adopt(&conn, &dir_of(&d, A), VAULT)
                .unwrap_or_else(|e| panic!("{what}: the restore must not be blocked: {e}"));
            assert!(!fired(&conn), "{what}: nothing is owed");
            assert_eq!(rearm_keys(&conn), before, "{what}: nothing was touched");
            assert_eq!(
                setting_text(&conn, pinboard_move::RESTORED),
                None,
                "{what}: no note"
            );
            assert_eq!(legacy_board_bytes(&conn), legacy_before, "{what}");
        }
    }

    #[test]
    fn a_planted_integer_as_the_record_of_the_move_reads_as_recorded() {
        // The same rule as `pinboard_move_recorded`: a value that is text once cast (an integer)
        // is a record, so the restore owes the copy; only a blank or non-UTF-8 value is not one.
        let (d, conn) = moved_without_mod_data();
        conn.execute(
            "UPDATE settings SET value = 7 WHERE key = 'pinboard_moved_sha'",
            [],
        )
        .unwrap();
        reset_for_adopt(&conn, &dir_of(&d, A), VAULT).unwrap();
        assert!(fired(&conn));
        assert_eq!(setting_text(&conn, pinboard_move::MOVED), None);
        assert_eq!(
            legacy_board_bytes(&conn).as_deref(),
            Some(OLD_BOARD.as_bytes())
        );
    }

    #[test]
    fn a_planted_legacy_row_never_blocks_a_restore_and_is_never_changed() {
        // A BLOB that is not text and an integer where the board should be: the restore goes
        // through, the row is byte for byte as planted (the move, when it runs, refuses an
        // unreadable board and says so in its strip).
        for plant in ["x'ff00fe'", "7"] {
            let (d, conn) = moved_without_mod_data();
            conn.execute(
                &format!("UPDATE settings SET value = {plant} WHERE key = 'pinboard'"),
                [],
            )
            .unwrap();
            let before = legacy_board_bytes(&conn);
            reset_for_adopt(&conn, &dir_of(&d, A), VAULT)
                .unwrap_or_else(|e| panic!("{plant}: {e}"));
            assert_eq!(legacy_board_bytes(&conn), before, "{plant}");
        }
    }

    #[test]
    fn a_second_reset_changes_nothing() {
        let (d, conn) = moved_without_mod_data();
        reset_for_adopt(&conn, &dir_of(&d, A), VAULT).unwrap();
        let after_first = rearm_keys(&conn);
        assert!(fired(&conn));
        reset_for_adopt(&conn, &dir_of(&d, A), VAULT).unwrap();
        assert_eq!(
            rearm_keys(&conn),
            after_first,
            "the record is gone, so no re-arm"
        );
        assert_eq!(
            legacy_board_bytes(&conn).as_deref(),
            Some(OLD_BOARD.as_bytes())
        );
    }

    #[test]
    fn the_rearm_names_the_pending_key_only_by_its_constant() {
        // The pending-key guard (`pinboard_move/tests.rs`) forbids the literal outside the files it
        // lists; this file reaches the key only through `autoinstall::rearm_pinboard`.
        let source = production(include_str!("adopt_mods.rs"));
        assert!(!source.contains(concat!("pinboard_move_", "pending")));
        assert!(!source.contains("pinboard_move::PENDING"));
        assert!(source.contains("autoinstall::rearm_pinboard(&tx)"));
    }

    /// A crafted backup can carry a `mod_secrets` row (the strip writes none, but nothing stops a
    /// hand-made file): the restore's commit deletes every row, and keeps the mod itself.
    #[test]
    fn adopting_deletes_every_mod_secret_a_restored_store_carries() {
        let (_d, conn) = store_conn();
        add(&conn, "atlas.board", true, "active", None);
        add(&conn, "acme.tool", false, "active", None);
        conn.execute(
            "INSERT INTO mod_secrets(mod_id, name, host, header, value) VALUES \
             ('atlas.board', 'a', 'h.example.com', 'X-A', 'planted-1'), \
             ('acme.tool', 'b', 'h.example.com', 'X-B', 'planted-2')",
            [],
        )
        .unwrap();
        reset_for_adopt(&conn, &dir_of(&_d, A), VAULT).unwrap();
        let left: i64 = conn
            .query_row("SELECT count(*) FROM mod_secrets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0);
        assert_eq!(read_rows(&conn).unwrap().len(), 2, "the mods stay");
    }

    #[test]
    fn a_reset_that_cannot_delete_the_secrets_changes_nothing_and_fails() {
        let (_d, mut conn) = store_conn();
        set_legacy(&conn, "true");
        devmode::write(&mut conn, &here(&_d, A), true).unwrap();
        conn.execute_batch("DROP TABLE mod_secrets").unwrap();
        assert!(reset_for_adopt(&conn, &dir_of(&_d, A), VAULT).is_err());
        assert!(devmode::is_on(&conn, &here(&_d, A)).unwrap(), "rolled back");
    }

    #[test]
    fn adopting_a_store_that_never_had_developer_mode_writes_the_empty_record_and_no_old_row() {
        let (_d, conn) = store_conn();
        assert_eq!(record(&conn), None);
        reset_for_adopt(&conn, &dir_of(&_d, A), VAULT).unwrap();
        assert!(record(&conn).is_some(), "an empty record is written");
        assert_eq!(legacy(&conn), None, "an old row is not invented");
        // Even an old "on" that turns up afterwards is never read.
        set_legacy(&conn, "true");
        assert!(!devmode::is_on(&conn, &here(&_d, A)).unwrap());
    }

    #[test]
    fn a_reset_that_fails_changes_nothing() {
        // Without the table the clear fails: developer mode must not end up switched off in one
        // half of a reset that the caller then treats as failed.
        let (_d, mut conn) = store_conn();
        set_legacy(&conn, "true");
        devmode::write(&mut conn, &here(&_d, A), true).unwrap();
        let before = record(&conn);
        conn.execute_batch("DROP TABLE mcp_clients").unwrap();
        assert!(reset_for_adopt(&conn, &dir_of(&_d, A), VAULT).is_err());
        assert_eq!(record(&conn), before, "rolled back");
        assert_eq!(legacy(&conn).as_deref(), Some("true"), "rolled back");
        assert!(devmode::is_on(&conn, &here(&_d, A)).unwrap());
    }

    #[test]
    fn what_a_join_does_with_developer_mode_leaves_both_rows_as_they_were() {
        // A join does nothing about developer mode (the command can't run without an app, so
        // `a_join_names_no_developer_mode_code...` reads its source). Everything a joiner's device
        // does with the store on the way in is asking whether it is on: that, for a store with no
        // record plus an old "true", for one whose record names only another device, and for one
        // with a record and no old row, changes neither row.
        for (record_for, old) in [
            (None, Some("true")),
            (Some(B), Some("true")),
            (Some(B), None),
        ] {
            let (_d, mut conn) = store_conn();
            if let Some(old) = old {
                set_legacy(&conn, old);
            }
            if let Some(device) = record_for {
                devmode::write(&mut conn, &here(&_d, device), true).unwrap();
            }
            let before = (record(&conn), legacy(&conn));
            assert!(
                !devmode::is_on(&conn, &here(&_d, A)).unwrap(),
                "the joiner is off"
            );
            assert_eq!((record(&conn), legacy(&conn)), before);
            // The owner's device keeps its switch.
            assert_eq!(
                devmode::is_on(&conn, &here(&_d, B)).unwrap(),
                record_for.is_some()
            );
        }
    }

    /// Production code of a source file: every line outside a test MODULE, less comment lines.
    /// Each column-0 `#[cfg(test)]` whose item (past any further attributes) is a `mod` is cut out
    /// by itself (a `mod x;` line, a one-line `mod x { .. }`, or a block up to its column-0 `}`),
    /// and the scan goes on after it. Cutting the file at the first test module instead would
    /// never scan the production code that follows an EARLY one (`mods/api/mod.rs` declares
    /// `mod tests;` at line 55, with production code to line 416); cutting at the first
    /// `#[cfg(test)]` of any kind would hide everything after an early test-only fn or `use`.
    fn production(source: &str) -> String {
        let lines: Vec<&str> = source.lines().collect();
        let is_mod = |l: &str| l.starts_with("mod ") || l.starts_with("pub mod ");
        let mut kept = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            if lines[i] == "#[cfg(test)]" {
                let item = (i + 1..lines.len()).find(|&j| !lines[j].starts_with("#["));
                if let Some(j) = item.filter(|&j| is_mod(lines[j])) {
                    let head = lines[j].trim_end();
                    let one_line =
                        head.ends_with(';') || (head.contains('{') && head.ends_with('}'));
                    i = if one_line {
                        j + 1
                    } else {
                        (j + 1..lines.len())
                            .find(|&k| lines[k] == "}")
                            .map_or(lines.len(), |k| k + 1)
                    };
                    continue;
                }
            }
            if !lines[i].trim_start().starts_with("//") {
                kept.push(lines[i]);
            }
            i += 1;
        }
        kept.join("\n")
    }

    /// What in a join's code (production, comments cut) touches developer mode or the connected
    /// apps: the devmode module, the old row, the per-device record, the restore's reset, or the
    /// apps' table. Empty is the rule: a join writes none of it.
    fn join_touches(join: &str) -> Vec<&'static str> {
        [
            "devmode",
            "dev_mode",
            "mods_dev_devices",
            "SETTING_DEV_DEVICES",
            "all_devices_off",
            "reset_for_adopt",
            "joiner_inherits",
            "drop_inherited",
            "mcp_clients",
            "mod_secrets",
            "purge_all",
        ]
        .into_iter()
        .filter(|name| join.contains(name))
        .collect()
    }

    #[test]
    fn a_join_names_no_developer_mode_code_and_a_restore_resets_before_it_attaches() {
        // The join's command can't run without an app, so its source is read: it must not run the
        // restore's reset on the shared store (that disconnected the apps of the vault's other
        // accounts and machines), and it must not touch developer mode at all (a join writes
        // nothing: the joiner's device has no entry in the record, so it is off, and the old row is
        // never read).
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
        assert_eq!(join_touches(join), Vec::<&str>::new());
        assert!(join.contains("adopt_mods::joiner_accepts_only_its_own_clients()"));
        assert!(join.contains("attach_profile_here("));
        // Nothing but the restore's reset writes the per-device record wholesale, anywhere.
        for (rel, source) in source_tree() {
            if rel.contains("/tests/") {
                continue;
            }
            let prod = production(&source);
            let callers = prod.matches("all_devices_off(").count();
            match rel.as_str() {
                // Its definition, and the restore's one call.
                "mods/devmode.rs" => assert_eq!(callers, 1, "{rel}"),
                "backup/adopt_mods.rs" => assert_eq!(callers, 1, "{rel}"),
                _ => assert_eq!(callers, 0, "{rel} calls the restore's reset"),
            }
        }
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

    /// The WRITE-OFF, the one use of the old row there is: the `UPDATE` in `all_devices_off`.
    const WRITE_OFF: &str = "UPDATE settings SET value = 'false' WHERE key = ?1";

    /// Whether `text` names the old row: the bare key `dev_mode` as a whole word, in any quoting
    /// (`"dev_mode"` in Rust, `'dev_mode'` or `key = dev_mode` inside a SQL string), or the
    /// constant `LEGACY_ROW`. A longer identifier (`dev_mode_state`, `set_dev_mode_for_device`)
    /// is a different name and does not count.
    fn names_the_old_row(text: &str) -> bool {
        text.contains("LEGACY_ROW") || names_the_old_key(text)
    }

    /// [`names_the_old_row`] without the constant: only the key as a whole word.
    fn names_the_old_key(text: &str) -> bool {
        const KEY: &str = "dev_mode";
        let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
        text.match_indices(KEY).any(|(at, _)| {
            !text[..at].chars().next_back().is_some_and(word)
                && !text[at + KEY.len()..].chars().next().is_some_and(word)
        })
    }

    /// Whether `devmode.rs`'s production code uses the old row in any way but the write-off: it
    /// may define the constant and name it once, as the parameter of [`WRITE_OFF`] inside
    /// `all_devices_off`; any other mention (a read, a second use, a `SELECT` in that function, the
    /// key as a literal) fails, and so does a file that no longer has the write-off.
    fn devmode_uses_the_old_row_beyond_the_write_off(source: &str) -> bool {
        let prod = production(source);
        let mut rest = Vec::new();
        let mut body = String::new();
        let mut in_body = false;
        for line in prod.lines() {
            if line.contains("const LEGACY_ROW") {
                continue;
            }
            if line.starts_with("pub fn all_devices_off(") {
                in_body = true;
            }
            if in_body {
                body.push_str(line);
                body.push('\n');
                if line == "}" {
                    in_body = false;
                }
            } else {
                rest.push(line);
            }
        }
        names_the_old_row(&rest.join("\n"))
            || body.matches("LEGACY_ROW").count() != 1
            || names_the_old_key(&body)
            || body.contains("SELECT")
            || !body.contains(WRITE_OFF)
    }

    /// The files (path under `src/`, production code) that use the old vault-wide `dev_mode` row at
    /// all, other than `mods/devmode.rs` and only for its write-off (see above). No other file may
    /// name it, not even the webview's key lists (the page's allow-list in `settings.rs` is
    /// asserted not to hold it, in test code; the Dev tab shows its value redacted like any other
    /// unlisted key). A file under a `tests/` directory is all test code (it has no
    /// `#[cfg(test)]` line to cut at), so it may plant the row to prove what the production code
    /// does with it.
    fn misusers_of_the_old_row(files: &[(String, String)]) -> Vec<String> {
        files
            .iter()
            .filter(|(rel, _)| !rel.contains("/tests/"))
            .filter(|(rel, source)| {
                if rel == "mods/devmode.rs" {
                    devmode_uses_the_old_row_beyond_the_write_off(source)
                } else {
                    names_the_old_row(&production(source))
                }
            })
            .map(|(rel, _)| rel.clone())
            .collect()
    }

    fn source_tree() -> Vec<(String, String)> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![root.clone()];
        let mut files = Vec::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let rel = path
                        .strip_prefix(&root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    files.push((rel, std::fs::read_to_string(&path).unwrap()));
                }
            }
        }
        files
    }

    #[test]
    fn the_old_dev_mode_row_is_named_only_by_devmode_and_only_to_write_it_off() {
        // Developer mode is per device, decided by `mods::devmode` from its own record, and only
        // a record that lists the device switches it on. The old row is the vault's, and anyone
        // who holds a shared store's key can write it, so a Rust READER of it anywhere (a
        // carry-over, a "migration", a display that gates on it) is an attack surface. The one
        // thing Rust does with it is write it "false" in the restore reset, in `devmode.rs`.
        let files = source_tree();
        assert!(
            files.iter().any(|(rel, _)| rel == "mods/devmode.rs"),
            "mods/devmode.rs still exists"
        );
        let misusers = misusers_of_the_old_row(&files);
        assert!(
            misusers.is_empty(),
            "{misusers:?} use the old dev_mode row, which is the vault's and not this device's, \
             and is never read: ask `mods::devmode`"
        );
    }

    /// A reader in the form it would most likely take: the key in a single-quoted SQL literal.
    const SQL_READ: &str = "SELECT value FROM settings WHERE key = 'dev_mode'";

    #[test]
    fn the_old_row_is_found_in_any_quoting_but_not_inside_a_longer_name() {
        for text in [
            SQL_READ,
            "get(\"dev_mode\")",
            "WHERE key = dev_mode",
            "'dev_mode'",
            "dev_mode",
            "x(LEGACY_ROW)",
        ] {
            assert!(names_the_old_row(text), "{text}");
        }
        for text in [
            "dev_mode_state(conn)",
            "set_dev_mode_for_device()",
            "is_dev_mode",
            "unsigned_dev_mode2",
            "",
        ] {
            assert!(!names_the_old_row(text), "{text}");
        }
    }

    #[test]
    fn the_old_row_check_fails_for_a_change_in_devmode_itself() {
        // Teeth for the one file that may name the row: the real devmode.rs passes, and each of
        // these edits of it (a reader, a second use inside the write-off, a read in place of the
        // write, the key as a literal, the write-off gone) is caught.
        let files = source_tree();
        let real = files
            .iter()
            .find(|(rel, _)| rel == "mods/devmode.rs")
            .map(|(_, s)| s.clone())
            .expect("mods/devmode.rs exists");
        assert!(!devmode_uses_the_old_row_beyond_the_write_off(&real));
        assert!(real.contains(WRITE_OFF), "the write-off is there to mutate");
        let bad = [
            // A reader beside the others.
            format!("{real}\nfn peek(conn: &Connection) {{ let _ = read(conn, LEGACY_ROW); }}\n"),
            // The key spelled out.
            format!("{real}\nfn peek(conn: &Connection) {{ let _ = read(conn, \"dev_mode\"); }}\n"),
            // The key inside a SQL string (single-quoted), beside the others.
            format!("{real}\nfn peek(c: &Connection) {{ c.query_row(\"{SQL_READ}\", [], |r| r.get::<_, String>(0)); }}\n"),
            // ...and inside the write-off function itself.
            real.replace(
                "[LEGACY_ROW],\n    )?;",
                &format!("[LEGACY_ROW],\n    )?;\n    conn.query_row(\"{SQL_READ}\", [], |r| r.get::<_, String>(0))?;"),
            ),
            // A read inside the write-off function.
            real.replace(
                "[LEGACY_ROW],\n    )?;",
                "[LEGACY_ROW],\n    )?;\n    let _ = read(conn, LEGACY_ROW);",
            ),
            // The write-off turned into a SELECT.
            real.replace(WRITE_OFF, "SELECT value FROM settings WHERE key = ?1"),
            // The write-off removed.
            real.replace(
                WRITE_OFF,
                "UPDATE settings SET value = 'true' WHERE key = ?1",
            ),
        ];
        for (i, source) in bad.iter().enumerate() {
            assert_ne!(source, &real, "edit {i} applied");
            assert!(devmode_uses_the_old_row_beyond_the_write_off(source), "{i}");
            let mut tree = files.clone();
            tree.retain(|(rel, _)| rel != "mods/devmode.rs");
            tree.push(("mods/devmode.rs".into(), source.clone()));
            assert_eq!(misusers_of_the_old_row(&tree), ["mods/devmode.rs"], "{i}");
        }
    }

    #[test]
    fn the_join_check_fails_for_a_join_that_touches_developer_mode() {
        assert!(join_touches("fn j() { attach_profile_here(); }").is_empty());
        for body in [
            "devmode::write(&mut c, d, false)",
            "crate::backup::adopt_mods::joiner_inherits_no_dev_mode(&conn)",
            "devmode::drop_inherited_on_at_join(&conn)",
            "devmode::all_devices_off(&conn)",
            "conn.execute(\"DELETE FROM settings WHERE key = 'dev_mode'\", [])",
            "conn.execute(\"UPDATE settings SET value = '' WHERE key = 'mods_dev_devices'\", [])",
            "store::SETTING_DEV_DEVICES",
            "reset_for_adopt(&conn, &dir_of(&_d, A), VAULT)",
            "conn.execute(\"DELETE FROM mcp_clients\", [])",
        ] {
            assert!(!join_touches(body).is_empty(), "{body}");
        }
    }

    #[test]
    fn the_old_row_check_fails_for_a_new_reader() {
        // Teeth: the same check over the real tree plus one more file that reads the row, by its
        // key or through the constant, in production code (a test or a comment doesn't count).
        let mut files = source_tree();
        assert!(misusers_of_the_old_row(&files).is_empty());
        for (path, body) in [
            ("mods/policy.rs", "fn f() { get(\"dev_mode\"); }"),
            (
                "mods/policy.rs",
                "fn f() { q(\"SELECT value FROM settings WHERE key = 'dev_mode'\"); }",
            ),
            (
                "mods/tab/x.rs",
                "fn f() { read(conn, devmode::LEGACY_ROW); }",
            ),
        ] {
            files.push((path.into(), body.into()));
            assert_eq!(misusers_of_the_old_row(&files), [path]);
            files.pop();
        }
        // A longer name is another thing (the page's `dev_mode_state` command is not the row).
        files.push((
            "mods/policy.rs".into(),
            "fn f() { dev_mode_state(conn); }".into(),
        ));
        assert!(misusers_of_the_old_row(&files).is_empty());
        files.pop();
        files.push((
            "mods/policy.rs".into(),
            "// \"dev_mode\" is the old row\nfn f() {}\n#[cfg(test)]\nmod t { const K: &str = \"dev_mode\"; }".into(),
        ));
        assert!(misusers_of_the_old_row(&files).is_empty());
        // An early test-only item must not hide the production code after it (the first
        // `#[cfg(test)]` of many files is a helper, not the test module).
        files.push((
            "mods/policy.rs".into(),
            "#[cfg(test)]\npub fn helper() {}\n\nfn f() { get(\"dev_mode\"); }\n\n#[cfg(test)]\n#[allow(clippy::x)]\nmod tests {}\n".into(),
        ));
        assert_eq!(misusers_of_the_old_row(&files), ["mods/policy.rs"]);
        files.pop();
        // An early test MODULE (a `mod tests;` line, or a block) must not hide the production code
        // after it either: `mods/api/mod.rs`, `paths.rs` and `mods/retag.rs` all have one.
        for early in [
            "#[cfg(test)]\nmod test_support;\n#[cfg(test)]\nmod tests;\n\nfn f() { get(\"dev_mode\"); }\n",
            "#[cfg(test)]\nmod a {\n    fn t() {}\n}\n\nfn f() { read(devmode::LEGACY_ROW); }\n",
            "#[cfg(test)]\nmod a {}\nfn f() { get(\"dev_mode\"); }\n",
        ] {
            files.push(("mods/policy.rs".into(), early.into()));
            assert_eq!(
                misusers_of_the_old_row(&files),
                ["mods/policy.rs"],
                "{early}"
            );
            files.pop();
        }
        // ...and the test module is still cut out when production code follows it.
        files.push((
            "mods/policy.rs".into(),
            "#[cfg(test)]\nmod a {\n    const K: &str = \"dev_mode\";\n}\n\nfn f() {}\n".into(),
        ));
        assert!(misusers_of_the_old_row(&files).is_empty());
        files.pop();
        // ...and a read inside the test module itself is still not production.
        files.push((
            "mods/policy.rs".into(),
            "#[cfg(test)]\nuse x::y;\nfn f() {}\n#[cfg(test)]\nmod tests { const K: &str = \"dev_mode\"; }\n".into(),
        ));
        assert!(misusers_of_the_old_row(&files).is_empty());
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
        // An unsigned mod the restore left paused (it turns developer mode off), and one the
        // frozen-page watchdog paused: each says what brings it back, not just "is paused".
        let unsigned = describe(
            &row(false, "paused", Some(store::REASON_DEV_MODE)),
            Some(&not_loaded()),
        );
        assert_eq!(unsigned.standing, Standing::Paused);
        let words = unsigned.note.unwrap();
        assert!(words.contains("developer mode is off"), "{words}");
        assert!(
            words.contains("Settings") && words.contains("Developer"),
            "{words}"
        );
        let frozen = note("paused", Some(store::REASON_FROZEN)).note.unwrap();
        assert!(
            frozen.contains("stopped responding") && frozen.contains("Resume"),
            "{frozen}"
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
