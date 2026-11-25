// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The legacy reader: the shared fixtures against the Rust port, the clamp rule on its own, and the
//! Calendar command's switch on `pinboard_moved_sha` over a real store.

use rusqlite::{params, Connection};
use serde_json::{json, Value};

use super::*;
use crate::mods::api::test_support::{insert_mod, store, ModRow};

const FIXTURES: &str = include_str!("../../../tests/pinboard-overlay-fixtures.json");

fn entry(id: &str, date: &str, label: &str) -> Entry {
    Entry {
        id: id.into(),
        date: date.into(),
        label: label.into(),
    }
}

fn entry_of(v: &Value) -> Entry {
    entry(
        v["id"].as_str().unwrap(),
        v["date"].as_str().unwrap(),
        v["label"].as_str().unwrap(),
    )
}

/// `{"$repeat": N, "item": {...}}` stands for N items, `{n}` in their strings read 1..N (the same
/// expansion the TypeScript test makes).
fn expand(v: &Value) -> Value {
    match v {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .flat_map(|x| match x.get("$repeat").and_then(Value::as_u64) {
                    Some(n) => (1..=n)
                        .map(|i| {
                            let mut item = x["item"].clone();
                            for (_, s) in item.as_object_mut().unwrap() {
                                if let Value::String(t) = s {
                                    *t = t.replace("{n}", &i.to_string());
                                }
                            }
                            item
                        })
                        .collect::<Vec<_>>(),
                    None => vec![expand(x)],
                })
                .collect(),
        ),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), expand(x))).collect()),
        other => other.clone(),
    }
}

pub(crate) struct Case {
    pub(crate) name: String,
    pub(crate) raws: Vec<String>,
    pub(crate) expected: Option<Vec<Entry>>,
    pub(crate) count: Option<usize>,
    pub(crate) first: Option<Entry>,
    pub(crate) last: Option<Entry>,
}

/// The shared fixtures, as the move's tests read them too (`mods::pinboard_move`).
pub(crate) fn cases() -> Vec<Case> {
    let fixtures: Value = serde_json::from_str(FIXTURES).unwrap();
    fixtures["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| Case {
            name: c["name"].as_str().unwrap().to_owned(),
            raws: match c.get("raws") {
                Some(r) => r
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| s.as_str().unwrap().to_owned())
                    .collect(),
                None => vec![expand(&c["board"]).to_string()],
            },
            expected: c
                .get("expected")
                .map(|e| e.as_array().unwrap().iter().map(entry_of).collect()),
            count: c
                .get("expectedCount")
                .map(|n| usize::try_from(n.as_u64().unwrap()).unwrap()),
            first: c.get("expectedFirst").map(entry_of),
            last: c.get("expectedLast").map(entry_of),
        })
        .collect()
}

pub(crate) fn check(case: &Case, got: &[Entry], what: &str) {
    if let Some(expected) = &case.expected {
        assert_eq!(got, expected.as_slice(), "{what}: {}", case.name);
    }
    if let Some(n) = case.count {
        assert_eq!(got.len(), n, "{what}: {}", case.name);
    }
    if let Some(first) = &case.first {
        assert_eq!(got.first(), Some(first), "{what}: {}", case.name);
    }
    if let Some(last) = &case.last {
        assert_eq!(got.last(), Some(last), "{what}: {}", case.name);
    }
}

#[test]
fn every_shared_fixture_gives_the_same_entries_as_the_typescript_original() {
    let all = cases();
    assert!(all.len() >= 20, "the fixtures were read");
    for case in &all {
        for raw in &case.raws {
            check(case, &derive_overlay(raw), "derive_overlay");
            check(case, &clamp_overlay(pinboard_entries(raw)), "clamp(port)");
        }
    }
}

#[test]
fn the_fixtures_hold_every_case_the_plan_names() {
    let names = cases()
        .iter()
        .map(|c| c.name.to_lowercase())
        .collect::<Vec<_>>()
        .join("\n");
    for word in [
        "undated",
        "bound to a project",
        "showoncalendar",
        "blank label",
        "folders",
        "unicode",
        "2,001-entry",
        "bad date",
        "datetime",
        "141-character",
        "repeated id",
    ] {
        assert!(names.contains(word), "no fixture case for {word}");
    }
}

#[test]
fn the_port_leaves_a_bad_date_for_the_clamp_to_refuse() {
    // `pinboard_entries` is the walk alone: the date is its first ten characters, unchecked.
    let raw = json!({"version": 1, "widgets": [{"id": "w", "kind": "timeline", "items": [
        {"id": "a", "date": "2026-13-45", "label": "  x  "},
        {"id": "b", "date": "2026-12-01T10:00", "label": ""},
    ]}]})
    .to_string();
    assert_eq!(
        pinboard_entries(&raw),
        vec![
            entry("w:a", "2026-13-45", "x"),
            entry("w:b", "2026-12-01", "deadline")
        ]
    );
    assert_eq!(
        derive_overlay(&raw),
        vec![entry("w:b", "2026-12-01", "deadline")]
    );
}

#[test]
fn clamp_overlay_never_raises_on_what_an_old_board_can_hold() {
    let multibyte = "é".repeat(12);
    let got = clamp_overlay(vec![
        entry("", "2026-12-01", "no id"),
        entry("a", &multibyte, "x"),
        entry("b", "", "x"),
        entry("c", "2026-12-01\u{1F680}", "\u{1F680}".repeat(300).as_str()),
        entry("d", "\u{1F680}2026-12-0", "x"),
        entry("e", "2026-12-01", "\n\t"),
    ]);
    assert_eq!(
        got,
        // The empty id is dropped: `calendar.overlay.set` could not publish it again.
        vec![
            entry("c", "2026-12-01", &"\u{1F680}".repeat(140)),
            entry("e", "2026-12-01", "deadline"),
        ]
    );
}

#[test]
fn everything_the_clamp_keeps_is_something_the_mod_can_publish_again() {
    use crate::mods::api::overlay_writer_accepts as accepts;
    // Every fixture's clamped output goes through the real writer rule (`calendar.overlay.set`'s
    // `check_entries`): the mod's first republish over seeded rows must not be refused.
    for case in cases() {
        for raw in &case.raws {
            let kept = derive_overlay(raw);
            let sent: Vec<(&str, &str, &str)> = kept
                .iter()
                .map(|e| (e.id.as_str(), e.date.as_str(), e.label.as_str()))
                .collect();
            assert!(accepts(&sent), "{}", case.name);
        }
    }
    // The Pinboard's real shape: two random UUIDs (73 characters), and a label with an emoji's
    // variation selector or a no-break space.
    let id = "3f2b8c1e-6a4d-4f0b-9d2e-1c7a5b8e9f01:9a7e5d3c-2b1f-4e8a-8c6d-0f4b2a1e7d35";
    assert_eq!(id.chars().count(), 73);
    let kept = clamp_overlay(vec![
        entry(id, "2026-12-01", "\u{26a0}\u{fe0f} deploy"),
        entry("n", "2026-12-02", "a\u{a0}b"),
        entry("t", "2026-12-03", "tab\there"),
    ]);
    assert_eq!(kept.len(), 3);
    assert_eq!(kept[0], entry(id, "2026-12-01", "\u{26a0}\u{fe0f} deploy"));
    assert_eq!(kept[2].label, "tab here");
    let sent: Vec<(&str, &str, &str)> = kept
        .iter()
        .map(|e| (e.id.as_str(), e.date.as_str(), e.label.as_str()))
        .collect();
    assert!(accepts(&sent));
}

#[test]
fn a_skipped_entry_does_not_hold_its_id() {
    // The first entry with an id that the table could hold is the one kept: a bad date does not
    // reserve the id against a later good one.
    let got = clamp_overlay(vec![
        entry("a", "2026-02-30", "bad"),
        entry("a", "2026-02-28", "good"),
        entry("a", "2026-03-01", "repeat"),
    ]);
    assert_eq!(got, vec![entry("a", "2026-02-28", "good")]);
}

#[test]
fn the_label_is_cut_after_trimming_and_by_characters() {
    let long = format!("  {}  ", "ab".repeat(100));
    let got = clamp_overlay(vec![entry("a", "2026-12-01", &long)]);
    assert_eq!(got[0].label.chars().count(), 140);
    assert!(got[0].label.starts_with("abab"));
}

// ---- the Calendar's command: the switch on `pinboard_moved_sha` ----

fn board_with(entries: &[(&str, &str, &str)]) -> String {
    let items: Vec<Value> = entries
        .iter()
        .map(|(id, date, label)| json!({"id": id, "date": date, "label": label}))
        .collect();
    json!({"version": 1, "widgets": [{"id": "w1", "kind": "timeline", "items": items}]}).to_string()
}

fn install_pinboard(conn: &Connection) {
    insert_mod(
        conn,
        &ModRow::signed(PINBOARD_MOD_ID, "first", &["storage"]),
    );
}

fn seed(conn: &Connection, mod_id: &str, entries: &[Entry]) {
    for e in entries {
        conn.execute(
            "INSERT INTO mod_overlay(mod_id, id, date, label) VALUES (?1, ?2, ?3, ?4)",
            params![mod_id, e.id, e.date, e.label],
        )
        .unwrap();
    }
}

fn mark_moved(conn: &Connection) {
    crate::db::set_setting(conn, "pinboard_moved_sha", &"a".repeat(64)).unwrap();
}

#[test]
fn before_the_move_the_stored_board_is_what_draws() {
    let (_dir, conn) = store();
    assert_eq!(
        calendar_entries(&conn).unwrap(),
        vec![],
        "no board, no dots"
    );
    crate::db::set_setting(
        &conn,
        BOARD_KEY,
        &board_with(&[("i1", "2026-12-01", "ship"), ("i2", "2026-13-01", "bad")]),
    )
    .unwrap();
    assert_eq!(
        calendar_entries(&conn).unwrap(),
        vec![entry("w1:i1", "2026-12-01", "ship")]
    );
}

#[test]
fn before_the_move_the_overlay_table_is_not_read() {
    let (_dir, conn) = store();
    install_pinboard(&conn);
    seed(
        &conn,
        PINBOARD_MOD_ID,
        &[entry("w9:x", "2027-01-01", "from the table")],
    );
    crate::db::set_setting(
        &conn,
        BOARD_KEY,
        &board_with(&[("i1", "2026-12-01", "ship")]),
    )
    .unwrap();
    assert_eq!(
        calendar_entries(&conn).unwrap(),
        vec![entry("w1:i1", "2026-12-01", "ship")]
    );
    // A blank marker is no record of a move.
    crate::db::set_setting(&conn, "pinboard_moved_sha", "  ").unwrap();
    assert_eq!(calendar_entries(&conn).unwrap().len(), 1);
    assert_eq!(calendar_entries(&conn).unwrap()[0].label, "ship");
}

#[test]
fn after_the_move_the_mods_rows_are_what_draw_and_the_legacy_row_is_not_read() {
    let (_dir, conn) = store();
    install_pinboard(&conn);
    insert_mod(&conn, &ModRow::signed("atlas.other", "first", &["storage"]));
    crate::db::set_setting(
        &conn,
        BOARD_KEY,
        &board_with(&[("i1", "2026-12-01", "legacy")]),
    )
    .unwrap();
    seed(
        &conn,
        PINBOARD_MOD_ID,
        &[
            entry("w2:b", "2027-02-02", "second"),
            entry("w2:a", "2027-01-01", "first"),
        ],
    );
    seed(
        &conn,
        "atlas.other",
        &[entry("o:1", "2027-03-03", "not mine")],
    );
    mark_moved(&conn);
    assert_eq!(
        calendar_entries(&conn).unwrap(),
        vec![
            entry("w2:b", "2027-02-02", "second"),
            entry("w2:a", "2027-01-01", "first")
        ],
        "the mod's rows, in the order they were written, and no other mod's"
    );
}

#[test]
fn after_the_move_rows_planted_past_what_the_call_allows_never_reach_the_calendar() {
    let (_dir, conn) = store();
    install_pinboard(&conn);
    seed(
        &conn,
        PINBOARD_MOD_ID,
        &[entry("good:1", "2026-12-01", "kept")],
    );
    // What a shared vault's member can write straight to the table: the CHECK is only the date's
    // shape, so these all pass it.
    conn.execute_batch(
        "INSERT INTO mod_overlay(mod_id, id, date, label) VALUES
            ('atlas.pinboard', 'month13', '2026-13-45', 'no such day'),
            ('atlas.pinboard', 'zero', '0000-00-00', 'year zero'),
            ('atlas.pinboard', 'tab\tid', '2026-12-02', 'control in id'),
            ('atlas.pinboard', '', '2026-12-02', 'empty id');
         INSERT INTO mod_overlay(mod_id, id, date, label)
            SELECT 'atlas.pinboard', 'long' || printf('%0300d', 0), '2026-12-03', 'id too long';
         INSERT INTO mod_overlay(mod_id, id, date, label)
            SELECT 'atlas.pinboard', 'huge', '2026-12-04', replace(hex(zeroblob(3000000)), '00', 'ab');
         INSERT INTO mod_overlay(mod_id, id, date, label)
            VALUES ('atlas.pinboard', 'blob', '2026-12-05', X'FFFE');",
    )
    .unwrap();
    // A run of valid-looking rows past the cap.
    conn.execute(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 12000)
         INSERT INTO mod_overlay(mod_id, id, date, label)
         SELECT 'atlas.pinboard', 'bulk:' || i, '2027-01-01', 'row ' || i FROM n",
        [],
    )
    .unwrap();
    mark_moved(&conn);
    let got = calendar_entries(&conn).unwrap();
    assert_eq!(
        got.len(),
        OVERLAY_MAX_ENTRIES,
        "no more than the call allows"
    );
    assert_eq!(got[0], entry("good:1", "2026-12-01", "kept"));
    let huge = got.iter().find(|e| e.id == "huge").unwrap();
    assert_eq!(huge.label.chars().count(), OVERLAY_LABEL_MAX_CHARS);
    for e in &got {
        assert!(
            e.id != "month13"
                && e.id != "zero"
                && e.id != "blob"
                && !e.id.is_empty()
                && !e.id.starts_with("long")
                && !e.id.contains('\t'),
            "{}",
            e.id
        );
        assert!(crate::mods::api::real_overlay_date(&e.date), "{}", e.date);
    }
}

/// A cell that is not text costs only its own row (the test above plants one); a database that
/// fails halfway through the read is an error the Calendar can show, never a quietly shorter list.
#[test]
fn after_the_move_a_read_that_fails_is_an_error_and_not_a_shorter_list_of_dots() {
    let (_dir, conn) = store();
    install_pinboard(&conn);
    seed(
        &conn,
        PINBOARD_MOD_ID,
        &[
            entry("ok:1", "2026-12-01", "first"),
            entry("boom", "2026-12-02", "the row the scan fails on"),
            entry("ok:2", "2026-12-03", "third"),
        ],
    );
    mark_moved(&conn);
    assert_eq!(calendar_entries(&conn).unwrap().len(), 3);
    // `substr` is what the reader cuts each cell with: a stand-in that fails on one id is an I/O
    // error in the middle of the scan.
    conn.create_scalar_function(
        "substr",
        3,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8,
        |ctx| {
            let text: String = ctx.get(0)?;
            if text == "boom" {
                return Err(rusqlite::Error::UserFunctionError("disk I/O".into()));
            }
            let n: i64 = ctx.get(2)?;
            Ok(text.chars().take(n as usize).collect::<String>())
        },
    )
    .unwrap();
    assert!(calendar_entries(&conn).is_err());
}

/// The read is bounded in rows as well as in what it keeps: a run of planted rows the clamp drops,
/// longer than the room the reader allows itself, ends the read, so a row past it is not drawn.
#[test]
fn after_the_move_the_read_stops_after_a_bounded_number_of_rows() {
    let (_dir, conn) = store();
    install_pinboard(&conn);
    conn.execute(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i <= ?1)
         INSERT INTO mod_overlay(mod_id, id, date, label)
         SELECT 'atlas.pinboard', 'bad:' || i, '2026-13-45', 'no such day' FROM n",
        [READ_LIMIT as i64],
    )
    .unwrap();
    seed(
        &conn,
        PINBOARD_MOD_ID,
        &[entry("late", "2026-12-01", "past the room")],
    );
    mark_moved(&conn);
    assert_eq!(calendar_entries(&conn).unwrap(), vec![]);
}

#[test]
fn after_the_move_an_empty_overlay_means_no_dots_never_the_legacy_row() {
    let (_dir, conn) = store();
    install_pinboard(&conn);
    crate::db::set_setting(
        &conn,
        BOARD_KEY,
        &board_with(&[("i1", "2026-12-01", "legacy")]),
    )
    .unwrap();
    mark_moved(&conn);
    assert_eq!(calendar_entries(&conn).unwrap(), vec![]);
    // Even with no mod installed at all (uninstalled after the move): nothing, and no error.
    conn.execute("DELETE FROM mods", []).unwrap();
    assert_eq!(calendar_entries(&conn).unwrap(), vec![]);
}

#[test]
fn a_corrupt_stored_board_draws_nothing_and_does_not_fail_the_calendar() {
    let (_dir, conn) = store();
    for raw in ["{not json", "", "null", "[1,2]"] {
        crate::db::set_setting(&conn, BOARD_KEY, raw).unwrap();
        assert_eq!(calendar_entries(&conn).unwrap(), vec![], "{raw:?}");
    }
}

#[test]
fn the_table_takes_every_fixtures_clamped_entries_and_reads_them_back_as_the_legacy_reader_drew_them(
) {
    // The seeding's own INSERT is the table's CHECK and primary key: the clamp must satisfy both
    // for every fixture, and the Calendar must draw the same dots before and after the move.
    for case in cases() {
        for raw in &case.raws {
            let (_dir, conn) = store();
            install_pinboard(&conn);
            crate::db::set_setting(&conn, BOARD_KEY, raw).unwrap();
            let before = calendar_entries(&conn).unwrap();
            check(&case, &before, "before the move");
            seed(&conn, PINBOARD_MOD_ID, &derive_overlay(raw));
            mark_moved(&conn);
            assert_eq!(
                calendar_entries(&conn).unwrap(),
                before,
                "after the move: {}",
                case.name
            );
        }
    }
}

#[test]
fn the_command_is_registered_and_wrapped() {
    let lib = include_str!("../lib.rs");
    assert!(lib.contains("commands::calendar_pinboard_entries,"));
    let ipc = include_str!("../../../src/lib/ipc.ts");
    assert!(ipc.contains("invoke<PinboardEntry[]>(\"calendar_pinboard_entries\")"));
}

// ---- the notes (K5b): the same fixtures, read for the notes an uninstall keeps ----

fn note_of(v: &Value) -> LegacyNote {
    LegacyNote {
        key: v["key"].as_str().unwrap().to_owned(),
        title: v["title"].as_str().unwrap().to_owned(),
        text: v["text"].as_str().unwrap().to_owned(),
        recorded_current: v["recordedCurrent"].as_bool().unwrap(),
    }
}

#[test]
fn every_shared_fixture_with_notes_gives_the_notes_the_uninstall_keeps() {
    let fixtures: Value = serde_json::from_str(FIXTURES).unwrap();
    let mut checked = 0;
    for c in fixtures["cases"].as_array().unwrap() {
        let Some(expected) = c.get("expectedNotes") else {
            continue;
        };
        let expected: Vec<LegacyNote> = expected.as_array().unwrap().iter().map(note_of).collect();
        let raws: Vec<String> = match c.get("raws") {
            Some(r) => r
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().to_owned())
                .collect(),
            None => vec![expand(&c["board"]).to_string()],
        };
        for raw in raws {
            assert_eq!(
                pinboard_notes(&raw),
                expected,
                "{}",
                c["name"].as_str().unwrap()
            );
            checked += 1;
        }
    }
    assert!(checked >= 8, "the note fixtures were read ({checked})");
}

#[test]
fn the_notes_of_a_board_never_depend_on_its_timelines_and_the_entries_never_on_its_notes() {
    // The same board feeds both readers; neither is affected by what only the other reads.
    let raw = json!({"version": 1, "widgets": [
        {"id": "n1", "kind": "note", "text": "a note"},
        {"id": "t1", "kind": "timeline", "items": [{"id": "i1", "date": "2026-12-01", "label": "x"}]},
    ]})
    .to_string();
    assert_eq!(pinboard_notes(&raw).len(), 1);
    assert_eq!(
        pinboard_entries(&raw),
        vec![entry("t1:i1", "2026-12-01", "x")]
    );
}

/// The Pinboard's `cheapHash`, from the TypeScript function run in node over these strings.
#[test]
fn the_cheap_hash_is_the_pinboards_own() {
    for (text, hash) in [
        ("", "45h"),
        ("hello", "2y0dev"),
        ("- a\n- b", "169yung"),
        ("\u{e9}\u{1F680} note", "o0ezb5"),
        (&"a".repeat(500), "k40eo5"),
    ] {
        assert_eq!(cheap_hash(text), hash, "{text:?}");
    }
}

fn install_mod_row(conn: &Connection) {
    insert_mod(
        conn,
        &ModRow::signed(PINBOARD_MOD_ID, "first", &["storage"]),
    );
}

fn put_board_row(conn: &Connection, value: &str) {
    conn.execute(
        "INSERT OR REPLACE INTO mod_kv(mod_id, key, class, value) VALUES (?1, 'board', 'content', ?2)",
        params![PINBOARD_MOD_ID, value],
    )
    .unwrap();
}

#[test]
fn before_the_move_the_stored_board_is_the_old_row_and_its_bytes_are_exact() {
    let (_dir, conn) = store();
    install_mod_row(&conn);
    assert_eq!(stored_board(&conn).unwrap(), None, "no row");
    put_board_row(&conn, "{\"from\":\"the mod\"}");
    let raw = "{\"version\":1,\n  \"widgets\":[] }  \u{e9}";
    crate::db::set_setting(&conn, BOARD_KEY, raw).unwrap();
    let got = stored_board(&conn).unwrap().unwrap();
    assert_eq!(got.source, BoardSource::Legacy);
    assert_eq!(
        got.bytes,
        raw.as_bytes(),
        "the bytes as stored, never re-written"
    );
}

#[test]
fn after_the_move_the_stored_board_is_the_mods_and_only_the_mods() {
    let (_dir, conn) = store();
    install_mod_row(&conn);
    crate::db::set_setting(&conn, BOARD_KEY, "{\"legacy\":true}").unwrap();
    mark_moved(&conn);
    assert_eq!(
        stored_board(&conn).unwrap(),
        None,
        "a moved store whose mod wrote no board has none: the frozen row is not a fallback"
    );
    put_board_row(&conn, "{\"version\":1,\"widgets\":[]}");
    let got = stored_board(&conn).unwrap().unwrap();
    assert_eq!(got.source, BoardSource::Mod);
    assert_eq!(got.bytes, b"{\"version\":1,\"widgets\":[]}");
}

#[test]
fn a_blank_board_row_is_no_board() {
    let (_dir, conn) = store();
    for raw in ["", "  ", "\n\t"] {
        crate::db::set_setting(&conn, BOARD_KEY, raw).unwrap();
        assert_eq!(stored_board(&conn).unwrap(), None, "{raw:?}");
    }
}
