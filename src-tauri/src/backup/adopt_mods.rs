// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What adopting a restored vault does about mods, what joining a shared one doesn't, and what a
//! restore tells the page about them (Mods spec §1.11).
//!
//! **The restore's reset** ([`reset_for_adopt`]) runs on the restored store's own connection
//! before it is attached, so before anything serves from it. A backup comes from somewhere else
//! (another computer, or this one at another time), and three things in it describe that other
//! place, not this one: developer mode (so a restore never enables unsigned installs on a machine
//! whose owner didn't: [`devmode::all_devices_off`] empties the per-device record and turns the old
//! row off, and the load that follows the store's open pauses the unsigned mods, INVARIANTS.md
//! I-34), the connected apps (`mcp_clients`: their tokens belong to the device that issued them),
//! and the MCP switch. The cached safety list is the fourth thing a
//! restore must meet before a mod runs, and it needs no step here: the restore hands the backup's
//! copy to the acceptance rules (`mods::refresh::take_restored`), and the writer's open applies the
//! cached list before it loads a row (`ledger::after_writer_open`), which is before any frame can
//! mount.
//!
//! **Joining a shared vault resets almost nothing.** Its store is the vault's, in use by the vault's
//! other accounts and machines at the same time, so switching dev mode off or deleting the
//! connected apps there would do it for them. Dev mode is per device (a joiner's device has no
//! entry, so it reads off), and the connected apps have a seam where PR C makes that true
//! ([`joiner_accepts_only_its_own_clients`], which takes no store, so it cannot write one). The one
//! write is [`joiner_inherits_no_dev_mode`]: a store with no per-device record yet and an old
//! vault-wide "on" would hand that "on" to the joiner's device when it first looks, so the join
//! switches everyone off first, before the store is attached.
//!
//! **The report** ([`describe`]) is one row per mod the restored store lists, in the words the
//! restore card shows. It is pending until the open and the same-machine recovery have run (a
//! mod whose files sit in another store of this vault on this computer comes back by itself, after
//! the open command returns), and settled after ([`settled`]), where "back" means the registry
//! serves it, not merely that its row says `active`.

use rusqlite::Connection;
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::error::Result;
use crate::mods::devmode;
use crate::mods::ids::ModId;
use crate::mods::ledger::ServeCheck;
use crate::mods::runtime::ModRuntime;
use crate::mods::safety::SafetyCheck;
use crate::mods::store;
use crate::AppState;

/// Reset what a restored store carries from where it came from (module doc). One transaction:
/// either the store is safe to serve or the caller refuses to adopt it. Any failure is the
/// caller's to treat as "don't adopt". A restore's commit only: never a shared vault's join.
pub fn reset_for_adopt(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    // Dev mode off for EVERY device: an empty per-device record (so the old row can't be carried
    // over to this one later) and the old row off. The unsigned rows need no write here: the load
    // that follows the store's open reads the record first and pauses each active unsigned row
    // with the reason `dev-mode` (`ledger::after_writer_open`), the same pause switching it off
    // makes; the page asks the backend (`dev_mode_state`), not its own copy.
    devmode::all_devices_off(&tx)?;
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

/// A joiner's dev mode is off: it is per device (`mods::devmode`), so the device that has just
/// joined has no entry in the shared store's record and reads as off, while the vault's other
/// devices keep theirs. The one thing that would break that is a store with no record yet: the
/// first device to look gets the old vault-wide row's "on" (`devmode::ensure`), and the joiner's
/// device would be it, whether the old row says `"true"` now or only turns so later (a member on an
/// earlier build writes it). So a store with no record gets the empty one, and the old row is
/// turned off ([`devmode::drop_inherited_on_at_join`]; the owner's device switches it on again).
/// Says whether it wrote. A store that has a record is not written at all, and no write lock is
/// taken for it.
///
/// Called by `commands::vaults::adopt_shared_vault` on the opened store BEFORE it is attached, so
/// before the writer's open (`ledger::after_writer_open`) looks developer mode up. A failure
/// refuses the join: continuing would let the open hand the old "on" over.
pub fn joiner_inherits_no_dev_mode(conn: &Connection) -> Result<bool> {
    Ok(devmode::drop_inherited_on_at_join(conn)?)
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

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn set_legacy(conn: &Connection, value: &str) {
        conn.execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![devmode::LEGACY_ROW, value],
        )
        .unwrap();
    }

    fn legacy(conn: &Connection) -> Option<String> {
        conn.query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [devmode::LEGACY_ROW],
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
        devmode::write(&mut conn, A, true).unwrap();
        devmode::write(&mut conn, B, true).unwrap();
        conn.execute(
            "INSERT INTO mcp_clients(name, token_hash) VALUES ('one', 'h1'), ('two', 'h2')",
            [],
        )
        .unwrap();
        add(&conn, "atlas.board", true, "active", None);

        reset_for_adopt(&conn).unwrap();

        // The restored store's record named this machine (or the one the backup came from): both
        // are off now, whatever the backup said.
        assert!(!devmode::is_on(&conn, A).unwrap());
        assert!(!devmode::is_on(&conn, B).unwrap());
        assert_eq!(legacy(&conn).as_deref(), Some("false"));
        let clients: i64 = conn
            .query_row("SELECT count(*) FROM mcp_clients", [], |r| r.get(0))
            .unwrap();
        assert_eq!(clients, 0);
        // Nothing else is touched: the installed mod and its state stay.
        assert_eq!(read_rows(&conn).unwrap().len(), 1);
    }

    #[test]
    fn adopting_a_store_that_never_had_developer_mode_leaves_nothing_to_carry_over() {
        let (_d, conn) = store_conn();
        assert_eq!(record(&conn), None);
        reset_for_adopt(&conn).unwrap();
        assert!(record(&conn).is_some(), "an empty record is written");
        // Even an old "on" that turns up afterwards is never handed to a device.
        set_legacy(&conn, "true");
        assert!(!devmode::is_on(&conn, A).unwrap());
    }

    #[test]
    fn a_reset_that_fails_changes_nothing() {
        // Without the table the clear fails: developer mode must not end up switched off in one
        // half of a reset that the caller then treats as failed.
        let (_d, mut conn) = store_conn();
        set_legacy(&conn, "true");
        devmode::write(&mut conn, A, true).unwrap();
        let before = record(&conn);
        conn.execute_batch("DROP TABLE mcp_clients").unwrap();
        assert!(reset_for_adopt(&conn).is_err());
        assert_eq!(record(&conn), before, "rolled back");
        assert_eq!(legacy(&conn).as_deref(), Some("true"), "rolled back");
        assert!(devmode::is_on(&conn, A).unwrap());
    }

    #[test]
    fn a_join_of_a_store_with_no_record_and_an_old_on_switches_everyone_off() {
        let (_d, conn) = store_conn();
        set_legacy(&conn, "true");
        assert!(joiner_inherits_no_dev_mode(&conn).unwrap());
        // The joiner's device would have been the first to look, and the store would have given it
        // the vault's old "on".
        assert!(!devmode::is_on(&conn, A).unwrap());
        assert!(!devmode::is_on(&conn, B).unwrap());
        assert!(record(&conn).is_some(), "the record is written, empty");
    }

    #[test]
    fn a_join_of_a_store_that_has_a_record_writes_nothing() {
        let (_d, mut conn) = store_conn();
        set_legacy(&conn, "true");
        devmode::write(&mut conn, B, true).unwrap();
        let before = (record(&conn), legacy(&conn));
        assert!(!joiner_inherits_no_dev_mode(&conn).unwrap());
        assert_eq!((record(&conn), legacy(&conn)), before);
        // The owner's device keeps its switch and the joiner's device was never on.
        assert!(devmode::is_on(&conn, B).unwrap());
        assert!(!devmode::is_on(&conn, A).unwrap());
    }

    /// Production code of a source file: everything before its test MODULE, less comment lines.
    /// The cut is at a column-0 `#[cfg(test)]` whose item (past any further attributes) is a
    /// `mod`, never at the first `#[cfg(test)]` of any kind: a file with an early test-only
    /// function or `use` (`#[cfg(test)] pub fn installed_root`) would otherwise count everything
    /// after it as test code and never be scanned.
    fn production(source: &str) -> String {
        let lines: Vec<&str> = source.lines().collect();
        let cut = (0..lines.len())
            .find(|&i| {
                lines[i] == "#[cfg(test)]"
                    && lines[i + 1..]
                        .iter()
                        .find(|l| !l.starts_with("#["))
                        .is_some_and(|l| l.starts_with("mod ") || l.starts_with("pub mod "))
            })
            .unwrap_or(lines.len());
        lines[..cut]
            .iter()
            .filter(|l| !l.trim_start().starts_with("//"))
            .copied()
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_join_writes_only_through_devmode_before_it_attaches_and_a_restore_resets_before_it_attaches(
    ) {
        // The join's command can't run without an app, so its source is read: it must not run the
        // restore's reset on the shared store (that disconnected the apps of the vault's other
        // accounts and machines), its one dev-mode write goes through the devmode function and
        // comes before the attach (whose engage is the first look at developer mode), and it names
        // no settings key itself.
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
        assert!(!join.contains("mcp_clients") && !join.contains("\"dev_mode\""));
        assert!(join.contains("adopt_mods::joiner_accepts_only_its_own_clients()"));
        let drop_on = join
            .find("adopt_mods::joiner_inherits_no_dev_mode(&conn)")
            .expect("the join drops an inherited developer-mode on");
        let attach = join
            .find("attach_profile_here(")
            .expect("and attaches the store");
        assert!(drop_on < attach, "before anything looks developer mode up");
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

    /// The files (path under `src/`, production code) that name the old vault-wide `dev_mode` row,
    /// other than the ones allowed to: `mods/devmode.rs` owns it (the carry-over and the two resets
    /// that go through `devmode::all_devices_off` / `drop_inherited_on_at_join`), and the two key
    /// lists that already mention it (the webview's allow-list in `settings.rs`, which asserts it
    /// is NOT readable from the page, and the Dev tab's list of values safe to show). A file under
    /// a `tests/` directory is all test code (it has no `#[cfg(test)]` line to cut at), so it may
    /// plant the row to prove what the production code does with it.
    fn readers_of_the_old_row(files: &[(String, String)]) -> Vec<String> {
        const ALLOWED: [&str; 3] = ["mods/devmode.rs", "settings.rs", "commands_dev.rs"];
        files
            .iter()
            .filter(|(rel, _)| !ALLOWED.contains(&rel.as_str()) && !rel.contains("/tests/"))
            .filter(|(_, source)| {
                let prod = production(source);
                prod.contains("\"dev_mode\"") || prod.contains("LEGACY_ROW")
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
    fn only_devmode_names_the_old_dev_mode_row() {
        // Developer mode is per device, decided by `mods::devmode` from its own record. The old
        // row is the vault's, written by whichever device last switched it on, so a Rust reader of
        // it anywhere else would gate on another device's choice.
        let files = source_tree();
        for allowed in ["mods/devmode.rs", "settings.rs", "commands_dev.rs"] {
            assert!(
                files.iter().any(|(rel, _)| rel == allowed),
                "{allowed} still exists"
            );
        }
        let readers = readers_of_the_old_row(&files);
        assert!(
            readers.is_empty(),
            "{readers:?} read the old dev_mode row, which is the vault's and not this device's: \
             ask `mods::devmode`"
        );
    }

    #[test]
    fn the_old_row_check_fails_for_a_new_reader() {
        // Teeth: the same check over the real tree plus one more file that reads the row, by its
        // key or through the constant, in production code (a test or a comment doesn't count).
        let mut files = source_tree();
        assert!(readers_of_the_old_row(&files).is_empty());
        for (path, body) in [
            ("mods/policy.rs", "fn f() { get(\"dev_mode\"); }"),
            (
                "mods/tab/x.rs",
                "fn f() { read(conn, devmode::LEGACY_ROW); }",
            ),
        ] {
            files.push((path.into(), body.into()));
            assert_eq!(readers_of_the_old_row(&files), [path]);
            files.pop();
        }
        files.push((
            "mods/policy.rs".into(),
            "// \"dev_mode\" is the old row\nfn f() {}\n#[cfg(test)]\nmod t { const K: &str = \"dev_mode\"; }".into(),
        ));
        assert!(readers_of_the_old_row(&files).is_empty());
        // An early test-only item must not hide the production code after it (the first
        // `#[cfg(test)]` of many files is a helper, not the test module).
        files.push((
            "mods/policy.rs".into(),
            "#[cfg(test)]\npub fn helper() {}\n\nfn f() { get(\"dev_mode\"); }\n\n#[cfg(test)]\n#[allow(clippy::x)]\nmod tests {}\n".into(),
        ));
        assert_eq!(readers_of_the_old_row(&files), ["mods/policy.rs"]);
        files.pop();
        // ...and a read inside the test module itself is still not production.
        files.push((
            "mods/policy.rs".into(),
            "#[cfg(test)]\nuse x::y;\nfn f() {}\n#[cfg(test)]\nmod tests { const K: &str = \"dev_mode\"; }\n".into(),
        ));
        assert!(readers_of_the_old_row(&files).is_empty());
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
