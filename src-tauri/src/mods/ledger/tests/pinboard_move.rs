// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Pinboard's move as the install carries it out (PR C, K8): through the pilots' auto-install
//! (`land_pilot`, `settle_pilot`) and through a person's own install from the Mods tab
//! (`install_verified`), against the real ledger, store and files. What the move itself does with a
//! board is `pinboard_move/tests.rs`; this file is about the transaction round it: the marker, the
//! rollback, and the frame that must never get to write first.
//!
//! The markers are seeded by hand: these tests start from a store the ladder has not marked and
//! pick the state they need (v62 writes the same two keys for a store that used the Pinboard).

use super::*;
use crate::mods::autoinstall::{self, Step, PILOTS};
use crate::mods::index::Index;
use crate::mods::package;
use crate::mods::pinboard_move::{self, fault};
use crate::mods::safety::tests::{entry_for, index_listing, now as list_now};
use serde_json::json;
use sha2::{Digest, Sha256};

const MARKER: &str = "mods_autoinstall_pinboard";
const PENDING: &str = "pinboard_move_pending";
const ID: &str = "atlas.pinboard";

/// The legacy board as the old Pinboard wrote it: not what a re-serialiser would write.
const BOARD: &str = "{\"widgets\" : [{\"kind\":\"timeline\",\"id\":\"t\",\"items\":[{\"label\":\"Ship\",\"id\":\"i\",\"date\":\"2026-12-01\"}]}],\n\"version\":1}\n";

struct Built {
    bytes: Vec<u8>,
    keys: TrustedKeys,
    sha: String,
    verified: VerifiedPackage,
}

fn pinboard(m: &mut Value) {
    m["id"] = ID.into();
    m["name"] = "Pinboard".into();
}

fn built(version: &str) -> Built {
    built_with(version, pinboard)
}

/// A signed package whose manifest `change` edits (the fixture's own id is the Map's).
fn built_with(version: &str, change: impl FnOnce(&mut Value)) -> Built {
    let (bytes, key) = pkg(version, change);
    let verified = package::verify(&bytes, std::slice::from_ref(&key)).unwrap();
    Built {
        sha: hex::encode(Sha256::digest(&bytes)),
        keys: TrustedKeys::of_test_keys(std::slice::from_ref(&key)),
        verified,
        bytes,
    }
}

impl Built {
    fn index(&self) -> Index {
        let mut entry = entry_for(&self.verified, &self.sha);
        entry["id"] = ID.into();
        entry["name"] = "Pinboard".into();
        index_listing(entry, json!([]))
    }
}

fn pilot() -> &'static autoinstall::Pilot {
    &PILOTS[1]
}

fn setting(w: &World, key: &str) -> Option<String> {
    w.store
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT CAST(value AS TEXT) FROM settings WHERE key = ?1",
            [key],
            |r| r.get(0),
        )
        .optional()
        .unwrap()
}

/// What the Pinboard's store looks like when a migration owes it the move.
fn owe(w: &World, marker: bool) {
    w.exec(&format!(
        "INSERT INTO settings(key, value) VALUES ('pinboard', '{}');
         INSERT INTO settings(key, value) VALUES ('{PENDING}', '1');",
        BOARD.replace('\'', "''")
    ));
    if marker {
        w.exec(&format!(
            "INSERT INTO settings(key, value) VALUES ('{MARKER}', '1')"
        ));
    }
}

fn board(w: &World) -> Option<(String, i64)> {
    w.store
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT CAST(value AS TEXT), revision FROM mod_kv WHERE mod_id = ?1 AND key = 'board'",
            [ID],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .unwrap()
}

fn legacy(w: &World) -> (String, i64) {
    w.store
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT CAST(value AS TEXT), rowid FROM settings WHERE key = 'pinboard'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
}

fn land(w: &World, b: &Built, index: &Index) -> PilotLanded {
    let plan = w.ledger().pilot_plan(Some(index), list_now()).unwrap();
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].0.id, ID);
    let Step::Download(d) = plan.into_iter().next().unwrap().1 else {
        panic!("not a download")
    };
    w.ledger()
        .land_pilot(pilot(), &d, &b.bytes, &b.keys, index, list_now())
}

/// A person's own install from the Mods tab: the catalogue's file, consent given.
fn install_by_hand(w: &World, b: &Built, source: Source) -> Result<Installed, LedgerError> {
    let opts = InstallOptions {
        source,
        consented: true,
        approval: Approval::Approved,
        catalogue: &allow,
    };
    w.ledger()
        .install_verified(&b.verified, &b.sha, &opts, now())
}

fn mod_rows(w: &World) -> i64 {
    w.sql("SELECT count(*) FROM mods")
}

fn settled(w: &World, bytes_on_disk: &str) {
    // The move's whole effect, once, in the row's own transaction.
    let (stored, revision) = board(w).expect("the mod holds the board");
    assert!(stored == bytes_on_disk, "byte for byte");
    assert_eq!(revision, 1);
    assert_eq!(
        setting(w, "pinboard_moved_sha").as_deref(),
        Some(hex::encode(Sha256::digest(bytes_on_disk.as_bytes())).as_str())
    );
    assert_eq!(setting(w, PENDING), None);
    assert_eq!(
        setting(w, "mods_pilot_landed_pinboard").as_deref(),
        Some("1")
    );
    assert_eq!(
        setting(w, "mods_adopt_pinboard_prefs").as_deref(),
        Some("1")
    );
    assert_eq!(w.sql::<i64>("SELECT count(*) FROM mod_overlay"), 1);
}

// ---- The auto-install -----------------------------------------------------------------------------

#[test]
fn an_auto_install_moves_the_board_in_the_rows_own_transaction_and_leaves_the_old_row_alone() {
    let w = world();
    let b = built("1.0.0");
    let index = b.index();
    owe(&w, true);
    let before = legacy(&w);

    assert!(matches!(land(&w, &b, &index), PilotLanded::Installed(_)));

    settled(&w, BOARD);
    assert_eq!(setting(&w, MARKER), None, "the marker goes with it");
    assert_eq!(
        legacy(&w),
        before,
        "the old row, byte for byte and in place"
    );
    // D17: it is installed unconsented, and nothing runs before "Turn on".
    assert!(!w.sql::<bool>("SELECT consented_at IS NOT NULL FROM mods"));
    assert_eq!(w.runtime.installed_root(&id(ID)), None);
    // A later pass has nothing to do.
    assert!(w
        .ledger()
        .pilot_plan(Some(&index), list_now())
        .unwrap()
        .is_empty());
}

#[test]
fn a_move_that_does_not_read_back_rolls_the_install_back_and_the_next_pass_retries() {
    let w = world();
    let b = built("1.0.0");
    let index = b.index();
    owe(&w, true);
    fault::FIRED.with(|n| n.set(0));
    fault::TAMPER_AFTER_WRITE.with(|f| f.set(true));
    let landed = land(&w, &b, &index);
    fault::TAMPER_AFTER_WRITE.with(|f| f.set(false));

    assert_eq!(fault::FIRED.with(|n| n.get()), 1, "the fault fired");
    assert!(matches!(landed, PilotLanded::Failed(_)), "{landed:?}");
    // The mod never exists without its board: no row, no board, the marker and the copy still owed.
    assert_eq!(mod_rows(&w), 0);
    assert_eq!(board(&w), None);
    assert!(setting(&w, MARKER).is_some());
    assert_eq!(setting(&w, PENDING).as_deref(), Some("1"));
    assert_eq!(setting(&w, "pinboard_moved_sha"), None);
    assert!(w.installed_tree().is_empty(), "the files went back out");
    assert!(w.staging_is_empty());
    assert_eq!(legacy(&w).0, BOARD);
    // The next pass retries, and now it holds.
    assert!(matches!(land(&w, &b, &index), PilotLanded::Installed(_)));
    settled(&w, BOARD);
}

#[test]
fn a_board_that_cannot_move_stops_the_install_with_the_marker_kept_and_the_board_where_it_was() {
    // Each says its own reason: a valid board one byte over the cap must say "bigger", not "couldn't
    // be read" (a text of "x"s would be refused as unreadable with the size test gone).
    let over_the_cap = {
        let (head, tail) = (
            "{\"version\":1,\"widgets\":[{\"id\":\"n\",\"kind\":\"note\",\"text\":\"",
            "\"}]}",
        );
        let fill = pinboard_move::MOVE_MAX_BYTES + 1 - head.len() - tail.len();
        format!("{head}{}{tail}", "y".repeat(fill))
    };
    assert_eq!(over_the_cap.len(), pinboard_move::MOVE_MAX_BYTES + 1);
    assert!(serde_json::from_str::<Value>(&over_the_cap).is_ok());
    for (what, legacy_text, reason) in [
        (
            "not JSON",
            "- a note".to_owned(),
            "couldn't be read as it is",
        ),
        ("over the cap", over_the_cap, "bigger than a mod can save"),
    ] {
        let w = world();
        let b = built("1.0.0");
        let index = b.index();
        w.exec(&format!(
            "INSERT INTO settings(key, value) VALUES ('pinboard', '{legacy_text}');
             INSERT INTO settings(key, value) VALUES ('{PENDING}', '1');
             INSERT INTO settings(key, value) VALUES ('{MARKER}', '1');"
        ));
        let landed = land(&w, &b, &index);
        let PilotLanded::Failed(why) = landed else {
            panic!("{what}: installed")
        };
        assert!(why.contains("safe in PM's store"), "{what}: {why}");
        assert!(why.contains(reason), "{what}: {why}");
        assert_eq!(mod_rows(&w), 0, "{what}");
        assert!(setting(&w, MARKER).is_some(), "{what}");
        assert_eq!(setting(&w, PENDING).as_deref(), Some("1"), "{what}");
        assert!(w.installed_tree().is_empty(), "{what}");
        assert!(legacy(&w).0 == legacy_text, "{what}");
    }
}

#[test]
fn a_seeding_that_fails_never_rolls_the_install_back() {
    let w = world();
    let b = built("1.0.0");
    let index = b.index();
    owe(&w, true);
    fault::FIRED.with(|n| n.set(0));
    fault::SEEDING_FAILS.with(|f| f.set(true));
    let landed = land(&w, &b, &index);
    fault::SEEDING_FAILS.with(|f| f.set(false));

    assert_eq!(fault::FIRED.with(|n| n.get()), 1, "the fault fired");
    assert!(matches!(landed, PilotLanded::Installed(_)), "{landed:?}");
    let (stored, _) = board(&w).unwrap();
    assert!(stored == BOARD);
    assert_eq!(setting(&w, MARKER), None);
    assert_eq!(setting(&w, PENDING), None);
    assert_eq!(
        w.sql::<i64>("SELECT count(*) FROM mod_overlay"),
        0,
        "derived: the mod republishes"
    );
}

#[test]
fn a_blank_board_and_a_store_with_nothing_owed_install_without_moving_anything() {
    // Blank: the copy owed is cleared, nothing is written for the mod.
    let w = world();
    let b = built("1.0.0");
    w.exec(&format!(
        "INSERT INTO settings(key, value) VALUES ('pinboard', '  \n ');
         INSERT INTO settings(key, value) VALUES ('{PENDING}', '1');
         INSERT INTO settings(key, value) VALUES ('{MARKER}', '1');"
    ));
    assert!(matches!(
        land(&w, &b, &b.index()),
        PilotLanded::Installed(_)
    ));
    assert_eq!(board(&w), None);
    assert_eq!(setting(&w, PENDING), None);
    assert_eq!(setting(&w, "pinboard_moved_sha"), None);
    assert_eq!(legacy(&w).0, "  \n ");

    // Nothing owed (only the marker, as a restore writes it): the old row is not copied.
    let w = world();
    w.exec(&format!(
        "INSERT INTO settings(key, value) VALUES ('pinboard', '{}');
         INSERT INTO settings(key, value) VALUES ('{MARKER}', '1');",
        BOARD
    ));
    assert!(matches!(
        land(&w, &b, &b.index()),
        PilotLanded::Installed(_)
    ));
    assert_eq!(
        board(&w),
        None,
        "a restore of a store that moved does not move again"
    );
    assert_eq!(
        setting(&w, "mods_pilot_landed_pinboard").as_deref(),
        Some("1")
    );
    assert_eq!(legacy(&w).0, BOARD);
}

#[test]
fn a_restore_of_a_post_move_backup_does_not_move_again() {
    // The vault moved once. The person has worked in the mod since (here: emptied the board, and the
    // old row is a snapshot). A restore writes the marker again (the files are owed, not the rows):
    // settling it must not copy the snapshot over their later board.
    let w = world();
    let b = built("1.0.0");
    owe(&w, true);
    assert!(matches!(
        land(&w, &b, &b.index()),
        PilotLanded::Installed(_)
    ));
    w.exec(
        "UPDATE mod_kv SET value = '{\"version\":1,\"widgets\":[]}', revision = 9 WHERE key = 'board'",
    );
    w.exec(&format!(
        "INSERT INTO settings(key, value) VALUES ('{MARKER}', '1')"
    ));
    w.ledger().settle_pilot(pilot()).unwrap();
    assert_eq!(
        board(&w),
        Some(("{\"version\":1,\"widgets\":[]}".to_owned(), 9)),
        "their later board stands"
    );
    assert_eq!(setting(&w, "pinboard_move_conflict"), None);
    assert_eq!(setting(&w, MARKER), None);
    assert_eq!(legacy(&w).0, BOARD);
}

// ---- A person's own install -----------------------------------------------------------------------

#[test]
fn a_manual_install_with_a_pending_copy_moves_the_board_before_the_frame_can_write_one() {
    let w = world();
    let b = built("1.0.0");
    // Only the pending copy (D-9: it is written whenever a board exists; the marker only when the
    // store used the Pinboard), so no auto-install pass is coming either.
    owe(&w, false);
    install_by_hand(&w, &b, Source::Catalogue).unwrap();

    // The moment the row exists, so does the board: a frame started now reads it. This is the race:
    // without the hook the install returned with no board, and a frame that wrote its own empty
    // default first made the move keep both boards.
    settled(&w, BOARD);
    assert_eq!(setting(&w, "pinboard_move_conflict"), None);
    assert_eq!(
        w.sql::<i64>("SELECT count(*) FROM mod_kv WHERE key = 'board'"),
        1
    );
}

#[test]
fn a_manual_install_with_the_marker_settles_it_too_and_one_with_neither_touches_nothing() {
    let w = world();
    let b = built("1.0.0");
    owe(&w, true);
    install_by_hand(&w, &b, Source::Catalogue).unwrap();
    settled(&w, BOARD);
    assert_eq!(setting(&w, MARKER), None);

    // Neither marker nor pending copy: a plain install, no board, nothing recorded.
    let w = world();
    w.exec(&format!(
        "INSERT INTO settings(key, value) VALUES ('pinboard', '{BOARD}')"
    ));
    install_by_hand(&w, &b, Source::Catalogue).unwrap();
    assert_eq!(board(&w), None);
    assert_eq!(setting(&w, "pinboard_moved_sha"), None);
    assert_eq!(setting(&w, "mods_pilot_landed_pinboard"), None);
}

#[test]
fn a_manual_install_that_the_move_refuses_installs_nothing() {
    let w = world();
    let b = built("1.0.0");
    w.exec(&format!(
        "INSERT INTO settings(key, value) VALUES ('pinboard', '- a note');
         INSERT INTO settings(key, value) VALUES ('{PENDING}', '1');"
    ));
    let err = install_by_hand(&w, &b, Source::Catalogue).unwrap_err();
    assert!(err.to_string().contains("safe in PM's store"), "{err}");
    assert_eq!(mod_rows(&w), 0);
    assert_eq!(setting(&w, PENDING).as_deref(), Some("1"));
    assert!(w.installed_tree().is_empty());
}

#[test]
fn only_a_fresh_catalogue_install_of_a_pilot_moves_anything() {
    // A file the person picked is no catalogue install: no pass would settle it, and the board stays.
    let w = world();
    let b = built("1.0.0");
    owe(&w, true);
    install_by_hand(&w, &b, Source::File).unwrap();
    assert_eq!(board(&w), None);
    assert_eq!(setting(&w, PENDING).as_deref(), Some("1"));
    assert!(setting(&w, MARKER).is_some());

    // An update is not a fresh install: a copy owed after the mod was here is the pass's, not a
    // version change's.
    let w = world();
    install_by_hand(&w, &b, Source::Catalogue).unwrap();
    owe(&w, false);
    let newer = built("1.1.0");
    let installed = install_by_hand(&w, &newer, Source::Catalogue).unwrap();
    assert_eq!(installed.kind, InstallKind::Update);
    assert_eq!(board(&w), None);
    assert_eq!(setting(&w, PENDING).as_deref(), Some("1"));

    // Another mod is never a pilot, whatever markers are in the store.
    let w = world();
    owe(&w, true);
    w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
        .unwrap();
    assert_eq!(board(&w), None);
    assert_eq!(setting(&w, PENDING).as_deref(), Some("1"));
}

// ---- The landed note never decides whether a copy is owed ----------------------------------------

#[test]
fn a_pending_copy_moves_whether_or_not_the_landed_note_is_already_there() {
    // The note is written by every settle (even one that moved nothing) and a backup keeps it
    // (`settings` is never stripped), so a store that is owed the copy AGAIN carries it: a restore
    // of a backup taken with the mod's data off brings the old board back this way. The pending
    // key is the only thing that says a copy is owed.
    //
    // 1. The auto-install (a fresh mod, the note already there).
    let w = world();
    let b = built("1.0.0");
    let index = b.index();
    owe(&w, true);
    w.exec("INSERT INTO settings(key, value) VALUES ('mods_pilot_landed_pinboard', '1')");
    assert!(matches!(land(&w, &b, &index), PilotLanded::Installed(_)));
    settled(&w, BOARD);
    assert_eq!(setting(&w, MARKER), None);

    // 2. The pass's settle of a row that is already here: moved once, then the board is gone from
    // the mod (its data was off in the backup) and the key and the marker are written again.
    w.exec(&format!(
        "DELETE FROM mod_kv WHERE key = 'board';
         DELETE FROM settings WHERE key = 'pinboard_moved_sha';
         INSERT INTO settings(key, value) VALUES ('{PENDING}', '1');
         INSERT INTO settings(key, value) VALUES ('{MARKER}', '1');"
    ));
    assert_eq!(board(&w), None);
    w.ledger().settle_pilot(pilot()).unwrap();
    settled(&w, BOARD);
    assert_eq!(setting(&w, MARKER), None, "the marker goes with it");
    assert_eq!(legacy(&w).0, BOARD, "the old row is untouched");

    // 3. A person's own install (the note already there, only the key owed).
    let w = world();
    owe(&w, false);
    w.exec("INSERT INTO settings(key, value) VALUES ('mods_pilot_landed_pinboard', '1')");
    install_by_hand(&w, &b, Source::Catalogue).unwrap();
    settled(&w, BOARD);
}

// ---- A restore without the mod's data brings the pre-move board back ------------------------------

#[test]
fn a_restore_that_re_arms_the_move_and_the_real_settle_put_the_old_board_back_in_the_mod() {
    // The Pinboard moved once. A backup taken with the mod's data off holds the signed row, the
    // legacy snapshot and the move's record, but not the mod's board (the strip drops `mod_kv`).
    let w = world();
    let b = built("1.0.0");
    owe(&w, true);
    assert!(matches!(
        land(&w, &b, &b.index()),
        PilotLanded::Installed(_)
    ));
    settled(&w, BOARD);
    // What the backup lacks: the mod's board. (The overlay stays in a backup; it is removed here so
    // the settle's own seeding is what the test sees.) The person's edits after the move are not
    // in the snapshot, which is the point of the strip's words.
    w.exec(
        "DELETE FROM mod_kv WHERE key = 'board'; DELETE FROM mod_overlay;
         INSERT INTO settings(key, value) VALUES ('pinboard_move_repaired', '1');",
    );
    let before = legacy(&w);

    // The restore's reset, as `switch_to_vault` runs it before the store is attached.
    let dir = tempfile::tempdir().unwrap();
    {
        let conn = w.store.conn.lock().unwrap();
        crate::backup::adopt_mods::reset_for_adopt(&conn, dir.path(), "vault-1").unwrap();
    }
    assert_eq!(setting(&w, PENDING).as_deref(), Some("1"));
    assert_eq!(setting(&w, "pinboard_moved_sha"), None);
    assert_eq!(setting(&w, "pinboard_move_repaired"), None);
    assert_eq!(setting(&w, MARKER).as_deref(), Some("1"));
    assert_eq!(board(&w), None, "nothing is copied until the pass settles");

    // The pass settles the row that is already there.
    w.ledger().settle_pilot(pilot()).unwrap();
    settled(&w, BOARD);
    assert_eq!(setting(&w, MARKER), None);
    assert_eq!(
        legacy(&w),
        before,
        "the old row, byte for byte and in place"
    );
    let state = {
        let conn = w.store.conn.lock().unwrap();
        pinboard_move::state(&conn).unwrap()
    };
    assert!(state.restored, "the strip says what came back");
    assert!(!state.conflict && !state.repaired && state.refused.is_none());
}

#[test]
fn the_rearm_is_not_run_for_a_backup_that_kept_the_mods_board() {
    // A toggle-on backup has the board the person edited since the move: the reset must not owe the
    // copy again, and the settle that follows must change nothing.
    let w = world();
    let b = built("1.0.0");
    owe(&w, true);
    assert!(matches!(
        land(&w, &b, &b.index()),
        PilotLanded::Installed(_)
    ));
    w.exec(
        "UPDATE mod_kv SET value = '{\"version\":1,\"widgets\":[]}', revision = 9 WHERE key = 'board'",
    );
    let dir = tempfile::tempdir().unwrap();
    {
        let conn = w.store.conn.lock().unwrap();
        crate::backup::adopt_mods::reset_for_adopt(&conn, dir.path(), "vault-1").unwrap();
    }
    assert_eq!(setting(&w, PENDING), None);
    assert!(setting(&w, "pinboard_moved_sha").is_some());
    w.ledger().settle_pilot(pilot()).unwrap();
    assert_eq!(
        board(&w),
        Some(("{\"version\":1,\"widgets\":[]}".to_owned(), 9)),
        "their later board stands"
    );
    let conn = w.store.conn.lock().unwrap();
    assert!(!pinboard_move::state(&conn).unwrap().restored);
}

// ---- The Map, which a person's own install now settles too -----------------------------------------

const MAP_MARKER: &str = "mods_autoinstall_map";

fn owe_the_map(w: &World) {
    w.exec(&format!(
        "INSERT INTO settings(key, value) VALUES ('{MAP_MARKER}', '1');
         INSERT INTO settings(key, value) VALUES ('map', '{{\"nodeCap\":2500,\"tsneEnabled\":false}}');
         INSERT INTO settings(key, value) VALUES ('doc_layout_meta', '{{\"x\":1}}');
         INSERT INTO settings(key, value) VALUES ('project_layout_cache', '[1,2]');
         INSERT INTO settings(key, value) VALUES ('unrelated', 'kept');"
    ));
}

#[test]
fn a_manual_install_of_the_map_with_its_marker_runs_the_maps_handover_in_the_rows_transaction() {
    let w = world();
    let b = built_with("1.0.0", |_| {});
    owe_the_map(&w);
    install_by_hand(&w, &b, Source::Catalogue).unwrap();

    assert_eq!(
        setting(&w, MAP_MARKER),
        None,
        "the marker goes with the row"
    );
    assert_eq!(
        setting(&w, "mods_pilot_landed_map").as_deref(),
        Some("1"),
        "the landed note is written"
    );
    assert_eq!(
        setting(&w, "mods_adopt_map_prefs").as_deref(),
        Some("1"),
        "the page is asked for the device prefs"
    );
    for key in ["map", "doc_layout_meta", "project_layout_cache"] {
        assert_eq!(setting(&w, key), None, "{key}: dead once the Map is a mod");
    }
    assert_eq!(
        setting(&w, "unrelated").as_deref(),
        Some("kept"),
        "nothing else in settings"
    );
    let carried: Value =
        serde_json::from_str(&w.sql::<String>(
            "SELECT value FROM mod_kv WHERE mod_id = 'atlas.map' AND key = 'settings'",
        ))
        .unwrap();
    assert_eq!(carried, json!({"nodeCap": 2500, "enhanced": false}));
}

#[test]
fn a_manual_install_of_the_map_whose_handover_fails_installs_nothing_and_keeps_the_marker() {
    let w = world();
    let b = built_with("1.0.0", |_| {});
    owe_the_map(&w);
    // The handover's last write fails: the whole install goes back with it.
    w.exec(
        "CREATE TRIGGER no_note BEFORE INSERT ON settings WHEN NEW.key = 'mods_pilot_landed_map' \
         BEGIN SELECT RAISE(ABORT, 'no note'); END;",
    );
    install_by_hand(&w, &b, Source::Catalogue).unwrap_err();

    assert_eq!(mod_rows(&w), 0);
    assert!(w.installed_tree().is_empty(), "the files went back out");
    assert!(setting(&w, MAP_MARKER).is_some(), "the marker stays");
    for key in ["map", "doc_layout_meta", "project_layout_cache"] {
        assert!(setting(&w, key).is_some(), "{key}: kept");
    }
    assert_eq!(setting(&w, "mods_adopt_map_prefs"), None);
    assert_eq!(
        w.sql::<i64>("SELECT count(*) FROM mod_kv WHERE mod_id = 'atlas.map'"),
        0
    );
}
