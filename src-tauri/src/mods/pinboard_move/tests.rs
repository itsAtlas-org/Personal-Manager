// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Pinboard's move over a real store: the bytes, the guards, the strips' half, the device prefs,
//! and the rename that races the install. The install itself (the transaction, the marker, the manual
//! install) is `ledger/tests/pinboard_move.rs`.
//!
//! Every board is a stand-in for what a person or a hostile vault member could have stored in
//! `settings['pinboard']`. The oracles are fixed values (the board's own bytes, the shared fixtures'
//! expected arrays), never what the code under test would say.

use rusqlite::params;
use serde_json::json;

use super::*;
use crate::mods::api::test_support::{insert_mod, store, ModRow};
use crate::pinboard_legacy::tests as shared;
use crate::pinboard_legacy::{calendar_entries, Entry};

/// A signed catalogue install of the Pinboard with its three declared keys, as the ledger leaves it
/// (`insert_mod` makes a file install; the catalogue is what the adopt requires).
fn install(conn: &Connection) {
    insert_mod(
        conn,
        &ModRow {
            keys: vec![
                ("board", "content"),
                ("prefs", "content"),
                ("board-before-move", "content"),
            ],
            ..ModRow::signed(PINBOARD_MOD_ID, "first", &["storage", "projects.read"])
        },
    );
    conn.execute(
        "UPDATE mods SET installed_from = ?1 WHERE id = ?2",
        params![store::FROM_CATALOGUE, PINBOARD_MOD_ID],
    )
    .unwrap();
}

/// The store the move runs in: the mod installed, and triggers that abort any write to the legacy
/// row, so a path that touches `settings['pinboard']` fails where it does.
fn world() -> (tempfile::TempDir, Connection) {
    let (dir, conn) = store();
    install(&conn);
    conn.execute_batch(
        "CREATE TRIGGER legacy_no_update BEFORE UPDATE ON settings WHEN old.key = 'pinboard' \
         BEGIN SELECT RAISE(ABORT, 'the legacy row was written'); END;
         CREATE TRIGGER legacy_no_delete BEFORE DELETE ON settings WHEN old.key = 'pinboard' \
         BEGIN SELECT RAISE(ABORT, 'the legacy row was deleted'); END;",
    )
    .unwrap();
    (dir, conn)
}

/// The legacy row as the old Pinboard left it, and the move owed (what v62 writes in C2).
fn owed(conn: &Connection, board: &[u8]) {
    // Text where it is text (the Pinboard stored a string), a BLOB where it is not.
    match std::str::from_utf8(board) {
        Ok(text) => conn.execute(
            "INSERT OR REPLACE INTO settings(key, value) VALUES ('pinboard', ?1)",
            [text],
        ),
        Err(_) => conn.execute(
            "INSERT OR REPLACE INTO settings(key, value) VALUES ('pinboard', ?1)",
            [board],
        ),
    }
    .unwrap();
    db::set_setting(conn, PENDING, "1").unwrap();
}

fn legacy_row(conn: &Connection) -> Option<(Vec<u8>, i64)> {
    conn.query_row(
        "SELECT CAST(value AS BLOB), rowid FROM settings WHERE key = 'pinboard'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
    .unwrap()
}

fn setting(conn: &Connection, key: &str) -> Option<String> {
    text_of(conn, key).unwrap()
}

fn board_of(conn: &Connection, key: &str) -> Option<(Vec<u8>, i64)> {
    mod_value(conn, key)
        .unwrap()
        .map(|(bytes, revision)| (bytes, revision.expect("a plain integer revision")))
}

fn put_mod_board(conn: &Connection, key: &str, value: &str, revision: i64) {
    conn.execute(
        "INSERT INTO mod_kv(mod_id, key, class, value, revision) \
         VALUES (?1, ?2, 'content', ?3, ?4) \
         ON CONFLICT(mod_id, key) DO UPDATE SET value = excluded.value, revision = excluded.revision",
        params![PINBOARD_MOD_ID, key, value, revision],
    )
    .unwrap();
}

/// Run the handover as the install does: in a transaction that an error takes back.
fn run(conn: &mut Connection) -> Result<(), LedgerError> {
    let tx = conn.transaction().unwrap();
    hand_over_pinboard(&tx, PINBOARD_MOD_ID)?;
    tx.commit().unwrap();
    Ok(())
}

fn overlay(conn: &Connection) -> Vec<Entry> {
    let mut stmt = conn
        .prepare("SELECT id, date, label FROM mod_overlay WHERE mod_id = ?1 ORDER BY rowid")
        .unwrap();
    stmt.query_map([PINBOARD_MOD_ID], |r| {
        Ok(Entry {
            id: r.get(0)?,
            date: r.get(1)?,
            label: r.get(2)?,
        })
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

/// A board with three widgets, written the way JavaScript's `JSON.stringify` would NOT re-write it:
/// keys out of order, a space before a colon, an escape for a character that needs none, a trailing
/// newline. Parsing and re-serialising it would change its bytes.
fn three_widget_board() -> String {
    "{\"widgets\" : [\n  {\"kind\":\"note\",\"id\":\"n1\",\"text\":\"caf\\u00e9 \u{1F680} \\\"quoted\\\"\"},\n  \
     {\"id\":\"t1\", \"kind\":\"timeline\",\"items\":[{\"label\":\"Launch\",\"id\":\"i1\",\"date\":\"2026-12-01\"}]},\n  \
     {\"id\":\"l1\",\"kind\":\"list\",\"items\":[]}\n],\"version\":1}\n"
        .to_owned()
}

// ---- The bytes cross unchanged -----------------------------------------------------------------

#[test]
fn a_board_crosses_byte_for_byte_and_the_old_row_is_never_written() {
    let (_dir, mut conn) = world();
    let board = three_widget_board();
    owed(&conn, board.as_bytes());
    let before = legacy_row(&conn).unwrap();

    run(&mut conn).unwrap();

    let (stored, revision) = board_of(&conn, BOARD).unwrap();
    assert_eq!(
        stored,
        board.as_bytes(),
        "the very bytes, not a re-serialised copy"
    );
    assert_eq!(sha_hex(&stored), sha_hex(board.as_bytes()));
    assert_eq!(revision, 1);
    let class: String = conn
        .query_row(
            "SELECT class FROM mod_kv WHERE mod_id = ?1 AND key = 'board'",
            [PINBOARD_MOD_ID],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(class, "content");
    // The old row: the same bytes in the same row.
    assert_eq!(legacy_row(&conn).unwrap(), before);
    assert_eq!(before.0, board.as_bytes());
    // What the move records.
    assert_eq!(setting(&conn, MOVED), Some(sha_hex(board.as_bytes())));
    assert_eq!(setting(&conn, PENDING), None);
    assert_eq!(setting(&conn, CONFLICT), None);
    assert_eq!(setting(&conn, REPAIRED), None);
    assert_eq!(setting(&conn, ADOPT_FLAG), Some("1".into()));
    // The dots do not vanish for a user who never opens the tab.
    assert_eq!(
        overlay(&conn),
        vec![Entry {
            id: "t1:i1".into(),
            date: "2026-12-01".into(),
            label: "Launch".into()
        }]
    );
    // Nothing else of the mod's was made.
    assert_eq!(board_of(&conn, PREFS), None);
    assert_eq!(board_of(&conn, BEFORE_MOVE), None);
}

#[test]
fn a_900_kib_board_crosses_byte_for_byte() {
    let (_dir, mut conn) = world();
    let note = "x".repeat(900 * 1024);
    let board = format!(
        "{{\"version\":1,\"widgets\":[{{\"id\":\"n\",\"kind\":\"note\",\"text\":\"{note}\"}}]}}"
    );
    assert!(board.len() > 900 * 1024 && board.len() < MOVE_MAX_BYTES);
    owed(&conn, board.as_bytes());
    run(&mut conn).unwrap();
    let (stored, _) = board_of(&conn, BOARD).unwrap();
    assert!(stored == board.as_bytes(), "900 KiB, byte for byte");
    assert_eq!(setting(&conn, MOVED), Some(sha_hex(board.as_bytes())));
}

/// A valid board of exactly `len` bytes.
fn board_of_len(len: usize) -> String {
    let head = "{\"version\":1,\"widgets\":[{\"id\":\"n\",\"kind\":\"note\",\"text\":\"";
    let tail = "\"}]}";
    let fill = len - head.len() - tail.len();
    format!("{head}{}{tail}", "y".repeat(fill))
}

#[test]
fn the_size_test_is_the_cap_minus_a_kib_and_a_byte_over_is_refused() {
    assert_eq!(MOVE_MAX_BYTES, (1 << 20) - 1024);
    let (_dir, mut conn) = world();
    let exactly = board_of_len(MOVE_MAX_BYTES);
    assert_eq!(exactly.len(), MOVE_MAX_BYTES);
    owed(&conn, exactly.as_bytes());
    run(&mut conn).unwrap();
    assert!(board_of(&conn, BOARD).unwrap().0 == exactly.as_bytes());

    let (_dir, mut conn) = world();
    let over = board_of_len(MOVE_MAX_BYTES + 1);
    owed(&conn, over.as_bytes());
    let err = run(&mut conn).unwrap_err().to_string();
    assert!(err.contains("bigger than a mod can save"), "{err}");
    // Refused the way every refusal is: nothing written, the marker kept, the old row as it was.
    assert_eq!(board_of(&conn, BOARD), None);
    assert_eq!(setting(&conn, PENDING), Some("1".into()));
    assert_eq!(setting(&conn, MOVED), None);
    assert!(legacy_row(&conn).unwrap().0 == over.as_bytes());
    // The strip: the mod is installed here, so the person is told.
    assert_eq!(state(&conn).unwrap().refused, Some("tooBig"));
    // With no install asked for (no marker, no mod), there is nothing to warn about yet.
    let (_dir, bare) = store();
    owed(&bare, over.as_bytes());
    assert_eq!(state(&bare).unwrap().refused, None);
    db::set_setting(&bare, autoinstall::PILOTS[1].marker, "1").unwrap();
    assert_eq!(state(&bare).unwrap().refused, Some("tooBig"));
}

// ---- The read-back -----------------------------------------------------------------------------

#[test]
fn a_row_that_does_not_read_back_as_written_fails_the_move_and_takes_it_back() {
    let (_dir, mut conn) = world();
    let board = three_widget_board();
    owed(&conn, board.as_bytes());
    fault::FIRED.with(|n| n.set(0));
    fault::TAMPER_AFTER_WRITE.with(|f| f.set(true));
    let result = run(&mut conn);
    fault::TAMPER_AFTER_WRITE.with(|f| f.set(false));

    assert_eq!(fault::FIRED.with(|n| n.get()), 1, "the fault fired");
    assert!(result.is_err(), "a stored board that differs is an error");
    // The transaction went with it: no board in the mod, the move still owed, the old row as it was.
    assert_eq!(board_of(&conn, BOARD), None);
    assert_eq!(setting(&conn, PENDING), Some("1".into()));
    assert_eq!(setting(&conn, MOVED), None);
    assert_eq!(setting(&conn, ADOPT_FLAG), None);
    assert_eq!(overlay(&conn), vec![]);
    assert_eq!(legacy_row(&conn).unwrap().0, board.as_bytes());
    // And with no fault the next pass moves it.
    run(&mut conn).unwrap();
    assert!(board_of(&conn, BOARD).unwrap().0 == board.as_bytes());
}

// ---- A board the mod could not read -----------------------------------------------------------------

#[test]
fn a_lone_surrogate_gets_a_repaired_copy_and_the_old_bytes_stay() {
    let (_dir, mut conn) = world();
    // JavaScript wrote `\ud83d` alone (half an emoji), a stray low half, a good pair, and an
    // escaped backslash followed by the letters of an escape, which is just text.
    let board = r#"{"version":1,"widgets":[{"id":"n","kind":"note","text":"a\ud83d b \udc00 c \ud83d\ude80 d \\ud83d e \uD83D"}]}"#;
    assert!(
        serde_json::from_str::<Value>(board).is_err(),
        "serde_json refuses it"
    );
    owed(&conn, board.as_bytes());
    let before = legacy_row(&conn).unwrap();
    run(&mut conn).unwrap();

    let (stored, revision) = board_of(&conn, BOARD).unwrap();
    let stored = String::from_utf8(stored).unwrap();
    assert_eq!(revision, 1);
    assert_eq!(
        stored,
        r#"{"version":1,"widgets":[{"id":"n","kind":"note","text":"a\ufffd b \ufffd c \ud83d\ude80 d \\ud83d e \ufffd"}]}"#
    );
    let parsed: Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(
        parsed["widgets"][0]["text"],
        "a\u{FFFD} b \u{FFFD} c \u{1F680} d \\ud83d e \u{FFFD}"
    );
    assert_eq!(setting(&conn, REPAIRED), Some("1".into()));
    // The legacy row is the person's own bytes, exactly; the recorded sha is theirs, not the copy's.
    assert_eq!(legacy_row(&conn).unwrap(), before);
    assert_eq!(setting(&conn, MOVED), Some(sha_hex(board.as_bytes())));
    assert_eq!(setting(&conn, PENDING), None);
    assert!(state(&conn).unwrap().repaired);
}

#[test]
fn a_board_that_is_not_json_is_refused_with_the_marker_kept_and_nothing_written() {
    let hostile: Vec<(&str, Vec<u8>)> = vec![
        ("a plain note", b"- a note".to_vec()),
        ("cut short", br#"{"version":1,"widgets":["#.to_vec()),
        ("two documents", br#"{"widgets":[]}{"widgets":[]}"#.to_vec()),
        ("not UTF-8", b"{\"version\":1,\"x\":\"\xff\xfe\"}".to_vec()),
        ("a BOM", "\u{FEFF}{\"version\":1}".as_bytes().to_vec()),
        ("far too deep", "[".repeat(5000).into_bytes()),
        (
            "a surrogate the repair cannot save",
            br#"{"version":1,"widgets":[ "\ud83d"#.to_vec(),
        ),
    ];
    for (what, bytes) in hostile {
        let (_dir, mut conn) = world();
        owed(&conn, &bytes);
        let before = legacy_row(&conn).unwrap();
        let err = run(&mut conn).unwrap_err().to_string();
        assert!(err.contains("safe in PM's store"), "{what}: {err}");
        assert!(err.contains("couldn't be read"), "{what}: {err}");
        assert_eq!(board_of(&conn, BOARD), None, "{what}");
        assert_eq!(setting(&conn, PENDING), Some("1".into()), "{what}");
        assert_eq!(setting(&conn, MOVED), None, "{what}");
        assert_eq!(legacy_row(&conn).unwrap(), before, "{what}");
        // The strip's half: with an install asked for, the page is told, in a word.
        db::set_setting(&conn, autoinstall::PILOTS[1].marker, "1").unwrap();
        assert_eq!(state(&conn).unwrap().refused, Some("unreadable"), "{what}");
    }
}

#[test]
fn the_repair_changes_only_surrogate_escapes_inside_strings() {
    assert_eq!(
        repair_surrogates(r#"{"a":"\ud83d\ude80"}"#),
        None,
        "a pair is whole"
    );
    assert_eq!(
        repair_surrogates(r#"{"a":"\\ud83d"}"#),
        None,
        "a backslash, then text"
    );
    assert_eq!(repair_surrogates(r#"{"a":"\u00e9\n\""}"#), None);
    assert_eq!(
        repair_surrogates(r#"{"\udc00":"\ud83d\n"}"#).as_deref(),
        Some(r#"{"\ufffd":"\ufffd\n"}"#),
        "a key too, and the escape after it stays"
    );
    // A high half followed by another high half: the first is alone, the second pairs or not by itself.
    assert_eq!(
        repair_surrogates(r#""\ud83d\ud83d\ude80""#).as_deref(),
        Some(r#""\ufffd\ud83d\ude80""#)
    );
    // Outside a string it is not an escape at all.
    assert_eq!(repair_surrogates(r"\ud83d"), None);
    // A multi-byte character right after a backslash is not split.
    assert_eq!(
        repair_surrogates("\"\\\u{e9} \\ud83d\"").as_deref(),
        Some("\"\\\u{e9} \\ufffd\"")
    );
}

// ---- A board in the mod already ----------------------------------------------------------------------

#[test]
fn an_empty_default_in_the_mod_is_moved_over_and_one_with_content_is_never_touched() {
    for default in [
        r#"{"version":1,"widgets":[]}"#,
        r#"{"widgets":[]}"#,
        r#"{"version":1}"#,
        "{}",
        "  \n",
        "",
    ] {
        let (_dir, mut conn) = world();
        put_mod_board(&conn, BOARD, default, 3);
        let board = three_widget_board();
        owed(&conn, board.as_bytes());
        run(&mut conn).unwrap();
        let (stored, revision) = board_of(&conn, BOARD).unwrap();
        assert!(stored == board.as_bytes(), "{default:?}: moved over");
        assert_eq!(
            revision, 4,
            "{default:?}: one revision on, so a frame holding the old one is stale"
        );
        assert_eq!(setting(&conn, CONFLICT), None, "{default:?}");
        assert_eq!(
            board_of(&conn, BEFORE_MOVE),
            None,
            "{default:?}: the empty bytes are worth nothing"
        );
    }
    for content in [
        r#"{"version":1,"widgets":[{"id":"n","kind":"note","text":"mine"}]}"#,
        r#"{"version":1,"widgets":[],"futureField":{"keep":true}}"#,
        r#"{"version":2,"widgets":[],"layout":[1]}"#,
        "not json but somebody's words",
        "[]",
        "null",
    ] {
        let (_dir, mut conn) = world();
        put_mod_board(&conn, BOARD, content, 3);
        let board = three_widget_board();
        owed(&conn, board.as_bytes());
        let legacy = legacy_row(&conn).unwrap();
        run(&mut conn).unwrap();
        // Both kept, byte for byte.
        let (stored, revision) = board_of(&conn, BOARD).unwrap();
        assert_eq!(
            (stored.as_slice(), revision),
            (content.as_bytes(), 3),
            "{content}: never overwritten"
        );
        assert_eq!(legacy_row(&conn).unwrap(), legacy, "{content}");
        // The note says which old board the strip is about; the pending copy is settled; the board
        // the mod keeps is what the Calendar and an uninstall read from here.
        assert_eq!(
            setting(&conn, CONFLICT),
            Some(sha_hex(board.as_bytes())),
            "{content}"
        );
        assert_eq!(
            setting(&conn, MOVED),
            Some(sha_hex(board.as_bytes())),
            "{content}"
        );
        assert_eq!(setting(&conn, PENDING), None, "{content}");
        assert_eq!(
            overlay(&conn),
            vec![],
            "{content}: nothing seeded over the mod's own"
        );
        assert!(state(&conn).unwrap().conflict);
    }
}

#[test]
fn use_my_old_board_saves_the_mods_board_first_and_deletes_nothing() {
    let (_dir, mut conn) = world();
    let mine = r#"{"version":1,"widgets":[{"id":"n","kind":"note","text":"made in the mod"}]}"#;
    put_mod_board(&conn, BOARD, mine, 5);
    let old = three_widget_board();
    owed(&conn, old.as_bytes());
    let legacy = legacy_row(&conn).unwrap();
    run(&mut conn).unwrap();
    assert!(state(&conn).unwrap().conflict);

    use_old_board(&mut conn).unwrap();

    // The old board is the mod's now, one revision on; the one that was there is saved under the
    // declared key, byte for byte; the old row is untouched; the strip is gone.
    let (board, revision) = board_of(&conn, BOARD).unwrap();
    assert!(board == old.as_bytes());
    assert_eq!(revision, 6);
    let (kept, kept_revision) = board_of(&conn, BEFORE_MOVE).unwrap();
    assert_eq!((kept.as_slice(), kept_revision), (mine.as_bytes(), 1));
    assert_eq!(legacy_row(&conn).unwrap(), legacy);
    assert_eq!(setting(&conn, CONFLICT), None);
    assert_eq!(setting(&conn, MOVED), Some(sha_hex(old.as_bytes())));
    assert_eq!(setting(&conn, ADOPT_FLAG), Some("1".into()));
    assert_eq!(
        overlay(&conn).len(),
        1,
        "the Calendar draws the board that is shown"
    );
    assert!(!state(&conn).unwrap().conflict);
    // Nothing to choose now.
    assert!(matches!(
        use_old_board(&mut conn),
        Err(UseOldRefusal::NoConflict)
    ));
}

#[test]
fn use_my_old_board_refuses_whatever_it_cannot_do_whole_and_changes_nothing() {
    let mine = r#"{"version":1,"widgets":[{"id":"n","kind":"note","text":"mine"}]}"#;
    // The old row is not what the strip was raised for.
    let (_dir, mut conn) = world();
    put_mod_board(&conn, BOARD, mine, 2);
    owed(&conn, three_widget_board().as_bytes());
    run(&mut conn).unwrap();
    // (an older build writing the row, as a restore or a shared vault could)
    conn.execute_batch("DROP TRIGGER legacy_no_update").unwrap();
    conn.execute(
        "UPDATE settings SET value = '{\"version\":1,\"widgets\":[]}' WHERE key = 'pinboard'",
        [],
    )
    .unwrap();
    assert!(matches!(
        use_old_board(&mut conn),
        Err(UseOldRefusal::Changed)
    ));
    assert!(board_of(&conn, BOARD).unwrap().0 == mine.as_bytes());
    assert_eq!(board_of(&conn, BEFORE_MOVE), None);
    assert!(state(&conn).unwrap().conflict, "the strip stays");

    // A copy of another board already sits under board-before-move: it is never replaced.
    let (_dir, mut conn) = world();
    put_mod_board(&conn, BOARD, mine, 2);
    put_mod_board(
        &conn,
        BEFORE_MOVE,
        r#"{"version":1,"widgets":[{"id":"x"}]}"#,
        1,
    );
    owed(&conn, three_widget_board().as_bytes());
    run(&mut conn).unwrap();
    assert!(matches!(
        use_old_board(&mut conn),
        Err(UseOldRefusal::CopyInTheWay)
    ));
    assert!(board_of(&conn, BOARD).unwrap().0 == mine.as_bytes());
    assert_eq!(
        board_of(&conn, BEFORE_MOVE).unwrap().0,
        br#"{"version":1,"widgets":[{"id":"x"}]}"#
    );
    // ... unless it is this very board (an earlier try that stopped after the copy).
    put_mod_board(&conn, BEFORE_MOVE, mine, 1);
    use_old_board(&mut conn).unwrap();
    assert!(board_of(&conn, BOARD).unwrap().0 == three_widget_board().as_bytes());

    // An old board that cannot move is refused as the move would refuse it.
    let (_dir, mut conn) = world();
    put_mod_board(&conn, BOARD, mine, 2);
    owed(&conn, b"- not json");
    run(&mut conn).unwrap();
    assert!(state(&conn).unwrap().conflict);
    assert!(matches!(
        use_old_board(&mut conn),
        Err(UseOldRefusal::Unreadable)
    ));
    assert!(board_of(&conn, BOARD).unwrap().0 == mine.as_bytes());
    assert_eq!(board_of(&conn, BEFORE_MOVE), None);

    // The mod is not installed here from the catalogue.
    let (_dir, mut conn) = world();
    put_mod_board(&conn, BOARD, mine, 2);
    owed(&conn, three_widget_board().as_bytes());
    run(&mut conn).unwrap();
    conn.execute("UPDATE mods SET installed_from = 'file'", [])
        .unwrap();
    assert!(matches!(
        use_old_board(&mut conn),
        Err(UseOldRefusal::NotInstalled)
    ));
}

#[test]
fn keep_this_one_and_ok_clear_only_their_own_note() {
    let (_dir, mut conn) = world();
    put_mod_board(&conn, BOARD, r#"{"version":1,"widgets":[{"id":"n"}]}"#, 2);
    owed(&conn, three_widget_board().as_bytes());
    run(&mut conn).unwrap();
    db::set_setting(&conn, REPAIRED, "1").unwrap();
    let legacy = legacy_row(&conn).unwrap();

    assert!(
        !dismiss(&conn, "pinboard").unwrap(),
        "no other row is a note"
    );
    assert!(!dismiss(&conn, "").unwrap());
    assert!(legacy_row(&conn).is_some());
    assert!(dismiss(&conn, "conflict").unwrap());
    assert_eq!(
        state(&conn).unwrap(),
        MoveState {
            conflict: false,
            repaired: true,
            refused: None
        }
    );
    assert!(dismiss(&conn, "repaired").unwrap());
    assert_eq!(setting(&conn, REPAIRED), None);
    assert_eq!(
        legacy_row(&conn).unwrap(),
        legacy,
        "the old row is never touched"
    );
    assert!(board_of(&conn, BOARD).is_some());
    assert_eq!(
        setting(&conn, MOVED),
        Some(sha_hex(three_widget_board().as_bytes()))
    );
}

// ---- Nothing owed, nothing to move ---------------------------------------------------------------

#[test]
fn with_no_pending_copy_nothing_happens_even_over_an_empty_board() {
    // A restore of a store that moved: the legacy row is a snapshot, the mod holds the person's
    // later board (here an emptied one), and a pass that finds the marker again must not move.
    let (_dir, mut conn) = world();
    let board = three_widget_board();
    conn.execute(
        "INSERT INTO settings(key, value) VALUES ('pinboard', ?1)",
        [&board],
    )
    .unwrap();
    db::set_setting(&conn, MOVED, &sha_hex(board.as_bytes())).unwrap();
    put_mod_board(&conn, BOARD, r#"{"version":1,"widgets":[]}"#, 7);
    run(&mut conn).unwrap();
    let (stored, revision) = board_of(&conn, BOARD).unwrap();
    assert_eq!(
        (stored.as_slice(), revision),
        (br#"{"version":1,"widgets":[]}"#.as_slice(), 7)
    );
    assert_eq!(setting(&conn, CONFLICT), None);
    assert_eq!(setting(&conn, ADOPT_FLAG), None);
    assert_eq!(overlay(&conn), vec![]);
    assert_eq!(setting(&conn, MOVED), Some(sha_hex(board.as_bytes())));
}

#[test]
fn a_blank_or_missing_board_clears_the_pending_copy_and_stops() {
    for raw in [None, Some(""), Some("  \n\t "), Some("\u{a0}\u{2003}")] {
        let (_dir, mut conn) = world();
        db::set_setting(&conn, PENDING, "1").unwrap();
        if let Some(raw) = raw {
            conn.execute(
                "INSERT INTO settings(key, value) VALUES ('pinboard', ?1)",
                [raw],
            )
            .unwrap();
        }
        let legacy = legacy_row(&conn);
        run(&mut conn).unwrap();
        assert_eq!(setting(&conn, PENDING), None, "{raw:?}");
        assert_eq!(board_of(&conn, BOARD), None, "{raw:?}");
        assert_eq!(setting(&conn, MOVED), None, "{raw:?}: nothing moved");
        assert_eq!(setting(&conn, ADOPT_FLAG), None, "{raw:?}");
        assert_eq!(legacy_row(&conn), legacy, "{raw:?}");
    }
}

// ---- The overlay ----------------------------------------------------------------------------

#[test]
fn the_seeded_overlay_equals_the_shared_fixtures_for_every_board_they_hold() {
    let mut moved = 0;
    // The only fixture boards that cannot move, named here by their text: a blank board (nothing to
    // copy) and text that is not JSON. Every other raw must move, so a wrong refusal of one is a
    // failure here, not a skip. (The list is not asked of `plan`: it is the code under test.)
    const UNMOVABLE: [&str; 2] = ["", "{not json"];
    let mut refused = 0;
    for case in shared::cases() {
        for raw in &case.raws {
            let (_dir, mut conn) = world();
            owed(&conn, raw.as_bytes());
            let unmovable = UNMOVABLE.contains(&raw.as_str());
            let result = run(&mut conn);
            if unmovable {
                refused += 1;
                // Nothing a mod could read moves; the fixtures expect no dots either.
                assert!(board_of(&conn, BOARD).is_none(), "{}", case.name);
                assert_eq!(overlay(&conn), vec![], "{}", case.name);
                assert!(
                    case.expected.as_ref().is_none_or(Vec::is_empty),
                    "{}",
                    case.name
                );
                continue;
            }
            result.unwrap_or_else(|e| panic!("{}: refused: {e}", case.name));
            moved += 1;
            assert!(
                board_of(&conn, BOARD).unwrap().0 == raw.as_bytes(),
                "{}",
                case.name
            );
            // What the fixtures say the dots are: the 2,001-entry board stops at 2,000, the bad
            // dates are skipped, the 141-character label is cut to 140.
            shared::check(&case, &overlay(&conn), "the move's seeding");
            // And it is what the Calendar draws from here (the reader over the same rows).
            shared::check(
                &case,
                &calendar_entries(&conn).unwrap(),
                "the Calendar after the move",
            );
        }
    }
    assert!(moved >= 25, "the fixtures' boards moved ({moved})");
    assert!(refused >= 2, "the unmovable boards were seen ({refused})");
}

#[test]
fn a_seeding_that_fails_is_rolled_back_and_the_move_carries_on() {
    let (_dir, mut conn) = world();
    let board = json!({"version": 1, "widgets": [{"id": "t", "kind": "timeline", "items": [
        {"id": "a", "date": "2026-12-01", "label": "one"},
        {"id": "b", "date": "2026-12-02", "label": "two"}
    ]}]})
    .to_string();
    owed(&conn, board.as_bytes());
    // A mod's own earlier rows are replaced by the seeding, so a half-done one would show.
    conn.execute(
        "INSERT INTO mod_overlay(mod_id, id, date, label) VALUES (?1, 'old', '2026-01-01', 'old')",
        [PINBOARD_MOD_ID],
    )
    .unwrap();
    fault::FIRED.with(|n| n.set(0));
    fault::SEEDING_FAILS.with(|f| f.set(true));
    let result = run(&mut conn);
    fault::SEEDING_FAILS.with(|f| f.set(false));

    assert_eq!(fault::FIRED.with(|n| n.get()), 1, "the fault fired");
    result.expect("a seeding error never stops the move");
    assert!(board_of(&conn, BOARD).unwrap().0 == board.as_bytes());
    assert_eq!(setting(&conn, MOVED), Some(sha_hex(board.as_bytes())));
    assert_eq!(setting(&conn, PENDING), None);
    assert_eq!(setting(&conn, ADOPT_FLAG), Some("1".into()));
    // The savepoint took the whole seeding back: not the first row, and not the deletion of the old.
    let ids: Vec<String> = overlay(&conn).into_iter().map(|e| e.id).collect();
    assert_eq!(ids, ["old"]);
}

// ---- The old row is never written -----------------------------------------------------------------

#[test]
fn the_old_row_survives_every_path() {
    // The triggers every other test's store has abort a write; this one checks the plain bytes and
    // rowid on a store WITHOUT them, for each path that ends in a commit and for a refusal.
    let board = three_widget_board();
    let content = r#"{"version":1,"widgets":[{"id":"n"}]}"#;
    for path in 0..4 {
        let (_dir, mut conn) = store();
        install(&conn);
        owed(&conn, board.as_bytes());
        let before = legacy_row(&conn).unwrap();
        match path {
            0 => run(&mut conn).unwrap(),
            1 => {
                put_mod_board(&conn, BOARD, content, 1);
                run(&mut conn).unwrap();
                use_old_board(&mut conn).unwrap();
            }
            2 => {
                put_mod_board(&conn, BOARD, content, 1);
                run(&mut conn).unwrap();
                dismiss(&conn, "conflict").unwrap();
            }
            _ => {
                owed(&conn, b"- x");
                let before = legacy_row(&conn).unwrap();
                assert!(run(&mut conn).is_err());
                assert_eq!(legacy_row(&conn).unwrap(), before);
                continue;
            }
        }
        assert_eq!(legacy_row(&conn).unwrap(), before, "path {path}");
    }
}

#[test]
fn nothing_in_the_product_writes_the_legacy_row_or_the_pending_key() {
    // Source guard for the two C1 promises: `settings['pinboard']` is only ever read here, and the
    // move is dormant (the pending key is only ever deleted; C2's migration writes it).
    let source = include_str!("../pinboard_move.rs");
    let production = source.split("#[cfg(test)]\nmod tests;").next().unwrap();
    for (n, line) in production.lines().enumerate() {
        let code = line.split("//").next().unwrap();
        for target in ["LEGACY_KEY", "PENDING"] {
            assert!(
                !code.contains(&format!("set_setting(conn, {target}"))
                    && !code.contains(&format!("set_setting(&tx, {target}")),
                "line {}: writes {target}",
                n + 1
            );
        }
    }
    let code_only: String = production
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    for sql in code_only.split('"').filter(|s| s.contains("settings")) {
        let sql = sql.to_ascii_uppercase();
        assert!(
            !sql.contains("INSERT") && !sql.contains("UPDATE"),
            "a statement that writes settings in the move: {sql}"
        );
        if sql.contains("DELETE") {
            assert!(sql.contains("KEY = ?1"), "a delete by key only: {sql}");
        }
    }
    // Nothing else in the crate names the pending key outside this file and the tests (the PILOTS
    // entry uses the constant), so nothing in C1 writes it.
    fn walk(dir: &std::path::Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                let production = text.split("#[cfg(test)]\nmod tests").next().unwrap();
                let own = path.ends_with("mods/pinboard_move.rs")
                    || path.ends_with("mods/autoinstall.rs")
                    || path.components().any(|c| c.as_os_str() == "tests");
                if !own && production.contains(concat!("pinboard_move_", "pending")) {
                    out.push(path.display().to_string());
                }
            }
        }
    }
    let mut found = Vec::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut found,
    );
    assert!(found.is_empty(), "{found:?}");
}

// ---- Hostile stores ---------------------------------------------------------------------------

#[test]
fn a_planted_type_in_any_row_is_never_an_error_that_blocks_the_install() {
    // A BLOB at the pending key and as the legacy board.
    let (_dir, mut conn) = world();
    conn.execute(
        "INSERT INTO settings(key, value) VALUES (?1, x'00ff00')",
        [PENDING],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO settings(key, value) VALUES ('pinboard', x'7b7d')",
        [],
    )
    .unwrap();
    run(&mut conn).unwrap();
    assert_eq!(board_of(&conn, BOARD).unwrap().0, b"{}");

    // A BLOB that is not text as the mod's own board: content, kept.
    let (_dir, mut conn) = world();
    owed(&conn, three_widget_board().as_bytes());
    conn.execute(
        "INSERT INTO mod_kv(mod_id, key, class, value, revision) \
         VALUES (?1, 'board', 'content', x'ff00', 1)",
        [PINBOARD_MOD_ID],
    )
    .unwrap();
    run(&mut conn).unwrap();
    assert_eq!(
        board_of(&conn, BOARD).unwrap().0,
        [0xff, 0x00],
        "a planted BLOB is content: kept"
    );
    assert!(state(&conn).unwrap().conflict);
}

/// A shared vault's member can plant a `revision` the column's `>= 1` check lets through: text, a BLOB or a
/// fraction. Each is read as "no plain integer", which makes the board content (kept), never an error.
const PLANTED_REVISIONS: [&str; 3] = ["'x'", "1.5", "x'01'"];

fn plant_revision(conn: &Connection, key: &str, value: &str, revision: &str) {
    conn.execute(
        &format!(
            "INSERT OR REPLACE INTO mod_kv(mod_id, key, class, value, revision) \
             VALUES (?1, ?2, 'content', ?3, {revision})"
        ),
        params![PINBOARD_MOD_ID, key, value],
    )
    .unwrap();
}

fn stored_revision(conn: &Connection, key: &str) -> rusqlite::types::Value {
    conn.query_row(
        "SELECT revision FROM mod_kv WHERE mod_id = ?1 AND key = ?2",
        params![PINBOARD_MOD_ID, key],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn a_planted_revision_makes_the_board_content_that_is_kept_and_never_an_error() {
    for revision in PLANTED_REVISIONS {
        let (_dir, mut conn) = world();
        // Bytes that are only a default, so an honest revision would have been moved over.
        plant_revision(&conn, BOARD, r#"{"version":1,"widgets":[]}"#, revision);
        let was = stored_revision(&conn, BOARD);
        assert!(
            !matches!(was, rusqlite::types::Value::Integer(_)),
            "{revision}: the column kept the planted type ({was:?})"
        );
        let board = three_widget_board();
        owed(&conn, board.as_bytes());
        let legacy = legacy_row(&conn).unwrap();

        run(&mut conn).unwrap_or_else(|e| panic!("{revision}: the move failed: {e}"));

        // Both kept: the mod's row exactly as planted, the old row in place, the strip raised.
        assert_eq!(
            mod_value(&conn, BOARD).unwrap().unwrap().0,
            br#"{"version":1,"widgets":[]}"#,
            "{revision}"
        );
        assert_eq!(stored_revision(&conn, BOARD), was, "{revision}");
        assert_eq!(legacy_row(&conn).unwrap(), legacy, "{revision}");
        assert_eq!(setting(&conn, CONFLICT), Some(sha_hex(board.as_bytes())));
        assert_eq!(setting(&conn, PENDING), None, "{revision}");

        // "Use my old board" has no revision to write against: it changes nothing, and both boards
        // stay (the strip's other action is still there).
        let err = use_old_board(&mut conn).unwrap_err();
        assert!(matches!(err, UseOldRefusal::Failed), "{revision}: {err:?}");
        assert_eq!(
            mod_value(&conn, BOARD).unwrap().unwrap().0,
            br#"{"version":1,"widgets":[]}"#,
            "{revision}"
        );
        assert_eq!(board_of(&conn, BEFORE_MOVE), None, "{revision}");
        assert!(setting(&conn, CONFLICT).is_some(), "{revision}");
        assert!(dismiss(&conn, "conflict").unwrap());
    }
}

#[test]
fn a_planted_revision_on_the_prefs_row_does_not_stop_the_device_prefs() {
    for revision in PLANTED_REVISIONS {
        let (_dir, mut conn) = world();
        plant_revision(&conn, PREFS, r#"{"showPast":false}"#, revision);
        db::set_setting(&conn, ADOPT_FLAG, "1").unwrap();
        let handed = adopt_pinboard_prefs(&mut conn, Some(true), Some(true), None)
            .unwrap_or_else(|e| panic!("{revision}: {e:?}"));
        assert!(handed, "{revision}");
        let (bytes, _) = mod_value(&conn, PREFS).unwrap().unwrap();
        let object: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            object,
            json!({"showPast": false, "confirmDelete": true}),
            "{revision}: only the missing key was filled"
        );
        assert_eq!(setting(&conn, ADOPT_FLAG), None, "{revision}");
    }
}

// ---- The device prefs ---------------------------------------------------------------------------------

fn prefs(conn: &Connection) -> Option<(Value, i64)> {
    board_of(conn, PREFS).map(|(b, r)| (serde_json::from_slice(&b).unwrap(), r))
}

#[test]
fn the_device_prefs_fill_only_what_the_mod_lacks_and_clear_the_flag() {
    let (_dir, mut conn) = world();
    // Dormant: nothing is written without the flag the move sets.
    assert!(!adopt_pinboard_prefs(&mut conn, Some(false), Some(false), Some(false)).unwrap());
    assert_eq!(prefs(&conn), None);

    db::set_setting(&conn, ADOPT_FLAG, "1").unwrap();
    // This device set nothing: nothing to hand over, and the flag stays for a device that did.
    assert!(!adopt_pinboard_prefs(&mut conn, None, None, None).unwrap());
    assert_eq!(setting(&conn, ADOPT_FLAG), Some("1".into()));

    // The mod has no prefs yet: the device's become revision 1, and only the ones it set.
    assert!(adopt_pinboard_prefs(&mut conn, Some(false), None, Some(true)).unwrap());
    assert_eq!(
        prefs(&conn),
        Some((json!({"confirmDelete": false, "showCompleted": true}), 1))
    );
    assert_eq!(setting(&conn, ADOPT_FLAG), None);

    // What the person changed in the mod is never overwritten, and a missing one is added.
    db::set_setting(&conn, ADOPT_FLAG, "1").unwrap();
    put_mod_board(&conn, PREFS, r#"{"confirmDelete":true,"unknown":7}"#, 4);
    assert!(adopt_pinboard_prefs(&mut conn, Some(false), Some(false), Some(false)).unwrap());
    assert_eq!(
        prefs(&conn),
        Some((
            json!({"confirmDelete": true, "unknown": 7, "showPast": false, "showCompleted": false}),
            5
        ))
    );
    // Nothing missing: nothing written, revision unchanged.
    db::set_setting(&conn, ADOPT_FLAG, "1").unwrap();
    assert!(adopt_pinboard_prefs(&mut conn, Some(true), Some(true), Some(true)).unwrap());
    assert_eq!(prefs(&conn).unwrap().1, 5);
    assert_eq!(setting(&conn, ADOPT_FLAG), None);
}

#[test]
fn prefs_the_mod_holds_as_something_else_are_left_alone_and_a_test_folder_gets_nothing() {
    let (_dir, mut conn) = world();
    for odd in ["[1]", "\"x\"", "null", "not json"] {
        put_mod_board(&conn, PREFS, odd, 2);
        db::set_setting(&conn, ADOPT_FLAG, "1").unwrap();
        assert!(adopt_pinboard_prefs(&mut conn, Some(false), None, None).unwrap());
        assert_eq!(
            board_of(&conn, PREFS).unwrap(),
            (odd.as_bytes().to_vec(), 2),
            "{odd}"
        );
        assert_eq!(setting(&conn, ADOPT_FLAG), None, "{odd}");
    }
    conn.execute("UPDATE mods SET installed_from = 'file'", [])
        .unwrap();
    db::set_setting(&conn, ADOPT_FLAG, "1").unwrap();
    assert!(matches!(
        adopt_pinboard_prefs(&mut conn, Some(false), None, None),
        Err(AdoptRefusal::NotInstalled)
    ));
    assert_eq!(setting(&conn, ADOPT_FLAG), Some("1".into()));
}

// ---- A rename that races the install ----------------------------------------------------------------

/// A board with a timeline bound to `project`, in the Pinboard's own shape.
fn bound(project: &str) -> Value {
    json!({"version": 1, "widgets": [
        {"id": "a", "kind": "timeline", "project": project},
        {"id": "f", "kind": "folder", "children": [{"id": "c", "kind": "timeline", "project": project}]}
    ]})
}

/// The Pinboard mod with its `projectRefs`, installed but not yet turned on (consent pending: a
/// rewrite does not wait for a grant in that state, K3), and a project to rename.
fn store_with_refs() -> (tempfile::TempDir, Connection) {
    let (dir, conn) = store();
    install(&conn);
    conn.execute(
        "UPDATE mods SET project_refs = ?2, consented_at = NULL WHERE id = ?1",
        params![
            PINBOARD_MOD_ID,
            json!([{"key": "board", "path": "$.widgets..project"}]).to_string()
        ],
    )
    .unwrap();
    crate::entities::resolve_project(&conn, "Atlas", true)
        .unwrap()
        .unwrap();
    (dir, conn)
}

#[test]
fn a_rename_before_the_move_is_copied_and_one_after_it_goes_through_the_mods_refs() {
    // Both orders happen in one store under one lock, so one runs whole before the other.
    // Rename first: the old row is rewritten (the move is not recorded yet), and the move copies it.
    let (_dir, mut conn) = store_with_refs();
    owed(&conn, bound("Atlas").to_string().as_bytes());
    crate::projects::rename_project_satellites(&conn, "Atlas", "Atlas 2").unwrap();
    let renamed: Value = serde_json::from_slice(&legacy_row(&conn).unwrap().0).unwrap();
    assert_eq!(renamed, bound("Atlas 2"), "rewritten before the move");
    run(&mut conn).unwrap();
    let moved: Value = serde_json::from_slice(&board_of(&conn, BOARD).unwrap().0).unwrap();
    assert_eq!(moved, bound("Atlas 2"), "the move copies the renamed board");

    // Move first: the old row is a snapshot from then on, and the rename reaches the mod's board
    // through its declared refs, bumping the revision.
    let (_dir, mut conn) = store_with_refs();
    owed(&conn, bound("Atlas").to_string().as_bytes());
    run(&mut conn).unwrap();
    let snapshot = legacy_row(&conn).unwrap();
    crate::projects::rename_project_satellites(&conn, "Atlas", "Atlas 2").unwrap();
    assert_eq!(
        legacy_row(&conn).unwrap(),
        snapshot,
        "the snapshot is frozen"
    );
    let (bytes, revision) = board_of(&conn, BOARD).unwrap();
    let moved: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(moved, bound("Atlas 2"));
    assert_eq!(revision, 2, "a frame holding the first copy is stale");
}
