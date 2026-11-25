// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The `projectRefs` grammar against its shared vector table, the walk over hostile values, and the
//! rewrite's rules over a real store: whose data it touches, what it writes, what it leaves alone.

use rusqlite::{params, Connection};
use serde_json::{json, Value};

use super::*;
use crate::mods::api::test_support::{insert_mod, store, ModRow};

// ---- the grammar ---------------------------------------------------------------------------------

/// [`rewrite_text`] as the new text or nothing.
fn rewrite_opt(text: &str, paths: &[RefPath], change: &Change<'_>) -> Option<String> {
    match rewrite_text(text, paths, change) {
        Rewrite::Done(out) => Some(out),
        _ => None,
    }
}

fn table() -> Value {
    serde_json::from_str(include_str!("../../../../tests/mod-project-ref-paths.json")).unwrap()
}

#[test]
fn the_shared_vector_table_is_read_the_same_way_here() {
    let table = table();
    let valid = table["valid"].as_array().unwrap();
    let invalid = table["invalid"].as_array().unwrap();
    assert!(
        valid.len() >= 10 && invalid.len() >= 20,
        "a table, not a stub"
    );
    for path in valid {
        let path = path.as_str().unwrap();
        assert!(parse(path).is_ok(), "{path:?} should parse");
    }
    for case in invalid {
        let (path, why) = (case["path"].as_str().unwrap(), &case["why"]);
        assert!(parse(path).is_err(), "{path:?} ({why}) should not parse");
    }
}

// ---- the walk --------------------------------------------------------------------------------------

fn rename(value: &mut Value, path: &str, from: &str, to: &str) -> bool {
    let change = Change::Rename { from, to };
    parse(path)
        .unwrap()
        .each_member(value, &mut |v| change.apply(v))
}

#[test]
fn a_path_reaches_the_pinboards_widgets_and_the_children_of_its_folders_and_nothing_else() {
    let mut board = json!({
        "version": 3,
        "title": "Atlas",
        "widgets": [
            {"id": "a", "project": "Atlas", "title": "Atlas"},
            {"id": "b", "project": "Other"},
            {"id": "f", "kind": "folder", "project": null, "children": [
                {"id": "c", "project": "Atlas"},
                {"id": "d", "note": "Atlas"}
            ]}
        ]
    });
    for path in ["$.widgets[*].project", "$.widgets[*].children[*].project"] {
        assert!(rename(&mut board, path, "Atlas", "Atlas 2"), "{path}");
    }
    assert_eq!(
        board,
        json!({
            "version": 3,
            "title": "Atlas",
            "widgets": [
                {"id": "a", "project": "Atlas 2", "title": "Atlas"},
                {"id": "b", "project": "Other"},
                {"id": "f", "kind": "folder", "project": null, "children": [
                    {"id": "c", "project": "Atlas 2"},
                    {"id": "d", "note": "Atlas"}
                ]}
            ]
        }),
        "the title, the note and the other project's widget are not the paths' members"
    );
}

#[test]
fn a_string_is_a_project_only_when_it_equals_the_name_exactly_and_sits_on_the_path() {
    let mut value =
        json!({"p": "atlas", "q": "Atlas ", "r": ["Atlas"], "s": 7, "t": {"x": "Atlas"}});
    for key in ["p", "q", "r", "s", "t"] {
        assert!(
            !rename(&mut value, &format!("$.{key}"), "Atlas", "New"),
            "{key}"
        );
    }
    assert_eq!(value["p"], "atlas");
    assert!(rename(&mut value, "$.t.x", "Atlas", "New"));
    assert_eq!(value["t"]["x"], "New");
}

#[test]
fn a_recursive_segment_looks_eight_levels_down_and_no_further() {
    // `levels` objects around a holder: the holder sits `levels` below the value the segment starts at.
    fn nested(levels: usize) -> Value {
        let mut v = json!({"project": "Old"});
        for _ in 0..levels {
            v = json!({ "a": v });
        }
        v
    }
    let mut at_eight = nested(MAX_DEPTH);
    assert!(rename(&mut at_eight, "$..project", "Old", "New"));
    let mut at_nine = nested(MAX_DEPTH + 1);
    let before = at_nine.clone();
    assert!(!rename(&mut at_nine, "$..project", "Old", "New"));
    assert_eq!(at_nine, before, "past the cap nothing is touched");
    // Through arrays too, and every hit.
    let mut mixed = json!({"x": [{"project": "Old"}, {"y": {"project": "Old"}}], "project": "Old"});
    assert!(rename(&mut mixed, "$..project", "Old", "New"));
    assert_eq!(
        mixed,
        json!({"x": [{"project": "New"}, {"y": {"project": "New"}}], "project": "New"})
    );
    // A recursive segment in the middle of a path, then a plain key.
    let mut deep =
        json!({"a": {"b": {"widgets": [{"project": "Old"}]}}, "widgets": [{"project": "Old"}]});
    assert!(rename(&mut deep, "$..widgets[*].project", "Old", "New"));
    assert_eq!(deep["a"]["b"]["widgets"][0]["project"], "New");
    assert_eq!(deep["widgets"][0]["project"], "New");
}

#[test]
fn a_root_that_is_an_array_and_a_value_of_the_wrong_shape_are_handled() {
    let mut list = json!([{"project": "Old"}, {"project": "Else"}, 5, "Old"]);
    assert!(rename(&mut list, "$[*].project", "Old", "New"));
    assert_eq!(
        list,
        json!([{"project": "New"}, {"project": "Else"}, 5, "Old"])
    );
    for wrong in [
        json!(null),
        json!("Old"),
        json!(3),
        json!({"widgets": "Old"}),
    ] {
        let mut v = wrong.clone();
        assert!(!rename(&mut v, "$.widgets[*].project", "Old", "New"));
        assert_eq!(v, wrong);
    }
}

#[test]
fn names_with_quotes_unicode_and_json_syntax_survive_a_round_trip() {
    let hostile = [
        "He said \"hi\"",
        "back\\slash",
        "Zoë 🙂 北京",
        "line\nbreak\ttab",
        "{\"project\":\"x\"}",
        "[*]",
        "$.widgets",
        "ends with a quote\"",
    ];
    for name in hostile {
        let text = json!({"widgets": [{"project": name}]}).to_string();
        let change = Change::Rename {
            from: name,
            to: "Plain",
        };
        let out = rewrite_opt(&text, &[parse("$.widgets[*].project").unwrap()], &change).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&out).unwrap(),
            json!({"widgets": [{"project": "Plain"}]}),
            "{name:?}"
        );
        // And into a name like that: written as a string, never as JSON of its own.
        let into = Change::Rename {
            from: "Plain",
            to: name,
        };
        let back = rewrite_opt(&out, &[parse("$.widgets[*].project").unwrap()], &into).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&back).unwrap(),
            json!({"widgets": [{"project": name}]}),
            "{name:?}"
        );
    }
}

#[test]
fn a_delete_sets_the_key_to_null_and_keeps_everything_else() {
    let text = json!({"widgets": [{"id": "a", "project": "Gone", "n": 1}, {"id": "b", "project": "Kept"}]})
        .to_string();
    let out = rewrite_opt(
        &text,
        &[parse("$.widgets[*].project").unwrap()],
        &Change::Unbind { name: "Gone" },
    )
    .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap(),
        json!({"widgets": [{"id": "a", "project": null, "n": 1}, {"id": "b", "project": "Kept"}]})
    );
}

#[test]
fn nothing_matching_or_not_json_is_no_rewrite() {
    let paths = [parse("$.a").unwrap()];
    let change = Change::Rename {
        from: "Old",
        to: "New",
    };
    assert_eq!(rewrite_opt("{\"a\":\"Else\"}", &paths, &change), None);
    assert_eq!(rewrite_opt("not json", &paths, &change), None);
    assert_eq!(rewrite_opt("", &paths, &change), None);
    // Rule 5: serde_json's own depth limit. 200 levels don't parse, and are left as they are.
    let deep = format!("{}\"Old\"{}", "[".repeat(200), "]".repeat(200));
    assert_eq!(rewrite_opt(&deep, &paths, &change), None);
}

// ---- the rewrite, over a store -----------------------------------------------------------------------

const MOD: &str = "acme.board";

fn row(permissions: &[&'static str], consented: bool, state: &'static str) -> ModRow {
    ModRow {
        keys: vec![("board", "content")],
        state,
        consented,
        ..ModRow::signed(MOD, "second", permissions)
    }
}

/// An installed mod whose manifest declares `row`'s permissions and the `board` key, holding
/// `refs` in its `project_refs` column and `value` under `board` (revision 1).
fn install(conn: &Connection, row: &ModRow, refs: Value, value: &str) {
    insert_mod(conn, row);
    conn.execute(
        "UPDATE mods SET project_refs = ?2 WHERE id = ?1",
        params![row.id, refs.to_string()],
    )
    .unwrap();
    put(conn, row.id, "board", value);
}

fn put(conn: &Connection, id: &str, key: &str, value: &str) {
    conn.execute(
        "INSERT INTO mod_kv(mod_id, key, class, value, revision, updated_at) \
         VALUES (?1, ?2, 'content', ?3, 1, '2026-01-01T00:00:00.000Z') \
         ON CONFLICT(mod_id, key) DO UPDATE SET value = ?3, revision = 1, \
                                               updated_at = '2026-01-01T00:00:00.000Z'",
        params![id, key, value],
    )
    .unwrap();
}

fn grant(conn: &Connection, id: &str, permission: &str, decision: &str) {
    conn.execute(
        "INSERT INTO mod_grants(mod_id, permission, decision) VALUES (?1, ?2, ?3) \
         ON CONFLICT(mod_id, permission) DO UPDATE SET decision = ?3",
        params![id, permission, decision],
    )
    .unwrap();
}

/// The value, revision and stamp under `board`.
fn read(conn: &Connection) -> (Value, i64, String) {
    conn.query_row(
        "SELECT value, revision, updated_at FROM mod_kv WHERE mod_id = ?1 AND key = 'board'",
        [MOD],
        |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?)),
    )
    .map(|(v, r, t)| (serde_json::from_str(&v).unwrap(), r, t))
    .unwrap()
}

fn board_refs() -> Value {
    json!([
        {"key": "board", "path": "$.widgets[*].project"},
        {"key": "board", "path": "$.widgets[*].children[*].project"}
    ])
}

fn board(project: &str) -> String {
    json!({"widgets": [
        {"id": "a", "project": project},
        {"id": "f", "children": [{"id": "c", "project": project}]},
        {"id": "z", "project": "Other"}
    ]})
    .to_string()
}

fn renamed(conn: &Connection) -> Report {
    rewrite_all(
        conn,
        &Change::Rename {
            from: "Atlas",
            to: "Atlas 2",
        },
    )
    .unwrap()
}

fn unbound(conn: &Connection) -> Report {
    rewrite_all(conn, &Change::Unbind { name: "Atlas" }).unwrap()
}

#[test]
fn a_rename_rewrites_the_granted_mods_refs_and_bumps_the_revision() {
    let (_dir, conn) = store();
    install(
        &conn,
        &row(&["storage", PERMISSION], true, "active"),
        board_refs(),
        &board("Atlas"),
    );
    grant(&conn, MOD, PERMISSION, "granted");
    let (_, revision, stamp) = read(&conn);

    assert_eq!(
        renamed(&conn),
        Report {
            rewritten: 1,
            skipped_large: 0,
            ..Report::default()
        }
    );
    let (value, next, stamped) = read(&conn);
    assert_eq!(
        value,
        serde_json::from_str::<Value>(&board("Atlas 2")).unwrap()
    );
    assert_eq!(
        next,
        revision + 1,
        "one bump for the value, however many paths hit"
    );
    assert_ne!(stamped, stamp, "updated_at is stamped");
}

#[test]
fn a_delete_nulls_the_binding_and_bumps_the_revision() {
    let (_dir, conn) = store();
    install(
        &conn,
        &row(&["storage", PERMISSION], true, "active"),
        board_refs(),
        &board("Atlas"),
    );
    grant(&conn, MOD, PERMISSION, "granted");

    assert_eq!(unbound(&conn).rewritten, 1);
    let (value, revision, _) = read(&conn);
    assert_eq!(value["widgets"][0]["project"], Value::Null);
    assert_eq!(value["widgets"][1]["children"][0]["project"], Value::Null);
    assert_eq!(value["widgets"][2]["project"], "Other");
    assert_eq!(value["widgets"][0]["id"], "a", "the widget stays");
    assert_eq!(revision, 2);
}

#[test]
fn a_mod_that_never_declared_projects_read_keeps_its_string() {
    let (_dir, conn) = store();
    // Granted rows for what it did declare, and refs in its column (a column edited by hand, or a
    // manifest from before the install check): the rewrite reads the declaration, not the column.
    install(
        &conn,
        &row(&["storage"], true, "active"),
        board_refs(),
        &board("Atlas"),
    );
    grant(&conn, MOD, "storage", "granted");
    grant(&conn, MOD, PERMISSION, "granted");
    assert_eq!(renamed(&conn), Report::default());
    assert_eq!(
        read(&conn).0,
        serde_json::from_str::<Value>(&board("Atlas")).unwrap()
    );
    assert_eq!(read(&conn).1, 1);
}

#[test]
fn a_mod_that_was_asked_and_denied_keeps_its_string() {
    let (_dir, conn) = store();
    install(
        &conn,
        &row(&["storage", PERMISSION], true, "active"),
        board_refs(),
        &board("Atlas"),
    );
    grant(&conn, MOD, PERMISSION, "denied");
    assert_eq!(renamed(&conn), Report::default());
    assert_eq!(unbound(&conn), Report::default());
    let (value, revision, _) = read(&conn);
    assert_eq!(
        value,
        serde_json::from_str::<Value>(&board("Atlas")).unwrap()
    );
    assert_eq!(revision, 1);
}

#[test]
fn a_consented_mod_with_no_decision_on_record_is_not_held_and_keeps_its_string() {
    let (_dir, conn) = store();
    // Second party: projects.read is asked at install, so no granted row is no grant.
    install(
        &conn,
        &row(&["storage", PERMISSION], true, "active"),
        board_refs(),
        &board("Atlas"),
    );
    assert_eq!(renamed(&conn), Report::default());
    assert_eq!(read(&conn).1, 1);
}

#[test]
fn a_paused_mod_that_holds_the_grant_is_still_rewritten() {
    let (_dir, conn) = store();
    install(
        &conn,
        &row(&["storage", PERMISSION], true, "paused"),
        board_refs(),
        &board("Atlas"),
    );
    grant(&conn, MOD, PERMISSION, "granted");
    assert_eq!(renamed(&conn).rewritten, 1);
    assert_eq!(read(&conn).0["widgets"][0]["project"], "Atlas 2");
}

#[test]
fn a_mod_that_has_not_consented_yet_is_rewritten_with_no_grant_and_turning_it_on_changes_nothing() {
    let (_dir, conn) = store();
    // A pilot as an auto-install leaves it: declared, unconsented, nothing granted.
    install(
        &conn,
        &row(&["storage", PERMISSION], false, "active"),
        board_refs(),
        &board("Atlas"),
    );
    assert_eq!(renamed(&conn).rewritten, 1);
    let (value, revision, _) = read(&conn);
    assert_eq!(value["widgets"][0]["project"], "Atlas 2");
    assert_eq!(revision, 2);

    // 'Turn on': the person consents and grants what the screen named.
    conn.execute(
        "UPDATE mods SET consented_at = '2026-11-20T00:00:00.000Z'",
        [],
    )
    .unwrap();
    grant(&conn, MOD, PERMISSION, "granted");
    // The same rename again finds nothing, and the revision stays where it was.
    assert_eq!(renamed(&conn).rewritten, 0);
    let (after, revision, _) = read(&conn);
    assert_eq!((after, revision), (value, 2));
}

#[test]
fn an_unconsented_mod_that_never_declared_projects_read_keeps_its_string() {
    let (_dir, conn) = store();
    install(
        &conn,
        &row(&["storage"], false, "active"),
        board_refs(),
        &board("Atlas"),
    );
    assert_eq!(renamed(&conn), Report::default());
    assert_eq!(read(&conn).1, 1);
}

#[test]
fn a_value_over_eight_mebibytes_is_skipped_and_counted_never_parsed() {
    let (_dir, conn) = store();
    let pad = "a".repeat(9 * 1024 * 1024);
    let big = json!({"widgets": [{"project": "Atlas"}], "pad": pad}).to_string();
    install(
        &conn,
        &row(&["storage", PERMISSION], true, "active"),
        board_refs(),
        &big,
    );
    grant(&conn, MOD, PERMISSION, "granted");
    assert_eq!(
        renamed(&conn),
        Report {
            rewritten: 0,
            skipped_large: 1,
            ..Report::default()
        }
    );
    let stored: String = conn
        .query_row("SELECT value FROM mod_kv WHERE mod_id = ?1", [MOD], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(stored, big, "left exactly as it was");
    assert_eq!(read(&conn).1, 1);
    // One just inside the cap is rewritten.
    let ok =
        json!({"widgets": [{"project": "Atlas"}], "pad": "a".repeat(7 * 1024 * 1024)}).to_string();
    put(&conn, MOD, "board", &ok);
    assert_eq!(
        renamed(&conn),
        Report {
            rewritten: 1,
            skipped_large: 0,
            ..Report::default()
        }
    );
}

/// A second mod holding the same board under its own id, for the tests of a pass over several.
fn install_other(conn: &Connection) {
    let other = ModRow {
        keys: vec![("board", "content")],
        ..ModRow::signed("acme.other", "second", &["storage", PERMISSION])
    };
    install(conn, &other, board_refs(), &board("Atlas"));
    grant(conn, "acme.other", PERMISSION, "granted");
}

fn other_project(conn: &Connection) -> Value {
    let v: String = conn
        .query_row(
            "SELECT value FROM mod_kv WHERE mod_id = 'acme.other'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    serde_json::from_str::<Value>(&v).unwrap()["widgets"][0]["project"].clone()
}

#[test]
fn a_cell_that_is_not_the_text_it_should_be_costs_that_one_mod_and_never_the_rename() {
    // SQLite keeps a BLOB, an integer or bytes that are not UTF-8 in a TEXT column as they are, and
    // a shared vault's member can plant one: reading it as a String is an error, and an error here
    // rolls the whole rename back. Each plant below is on the first mod; the second must still be
    // rewritten and the call must succeed.
    for (what, sql) in [
        (
            "a BLOB value",
            "UPDATE mod_kv SET value = X'FFFE' WHERE mod_id = 'acme.board'",
        ),
        (
            "an invalid UTF-8 text value",
            "UPDATE mod_kv SET value = CAST(X'FFFE' AS TEXT) WHERE mod_id = 'acme.board'",
        ),
        (
            "a BLOB project_refs",
            "UPDATE mods SET project_refs = X'FFFE' WHERE id = 'acme.board'",
        ),
        (
            "an invalid UTF-8 project_refs",
            "UPDATE mods SET project_refs = CAST(X'FFFE' AS TEXT) WHERE id = 'acme.board'",
        ),
        (
            "a BLOB manifest",
            "UPDATE mods SET manifest = X'FFFE' WHERE id = 'acme.board'",
        ),
    ] {
        let (_dir, conn) = store();
        install(
            &conn,
            &row(&["storage", PERMISSION], true, "active"),
            board_refs(),
            &board("Atlas"),
        );
        grant(&conn, MOD, PERMISSION, "granted");
        install_other(&conn);
        conn.execute(sql, []).unwrap();
        let report = rewrite_all(
            &conn,
            &Change::Rename {
                from: "Atlas",
                to: "Atlas 2",
            },
        )
        .unwrap_or_else(|e| panic!("{what} broke the rewrite: {e}"));
        assert_eq!(other_project(&conn), "Atlas 2", "{what}: the other mod");
        assert_eq!(report.rewritten, 1, "{what}");
        assert_eq!(report.skipped_unreadable, 1, "{what} is counted");
    }
}

#[test]
fn a_stored_column_is_read_for_the_manifests_64_entries_and_a_repeat_is_one_path() {
    let declared = |k: &str| k == "board";
    let same: Vec<Value> = (0..5000)
        .map(|_| json!({"key": "board", "path": "$..a..b.c"}))
        .collect();
    let paths = stored_paths(&Value::Array(same).to_string(), declared);
    assert_eq!(paths["board"].len(), 1, "5000 copies are one path's work");

    let many: Vec<Value> = (0..200)
        .map(|i| json!({"key": "board", "path": format!("$.k{i}")}))
        .collect();
    let paths = stored_paths(&Value::Array(many).to_string(), declared);
    assert_eq!(
        paths["board"].len(),
        MAX_REFS,
        "past 64 entries are not read"
    );
    assert!(paths["board"].contains(&parse("$.k63").unwrap()));
    assert!(!paths["board"].contains(&parse("$.k64").unwrap()));
}

#[test]
fn a_pass_stops_parsing_once_its_budget_is_spent_and_counts_what_it_left() {
    let (_dir, conn) = store();
    install(
        &conn,
        &row(&["storage", PERMISSION], true, "active"),
        board_refs(),
        &board("Atlas"),
    );
    grant(&conn, MOD, PERMISSION, "granted");
    install_other(&conn);
    let one = board("Atlas").len() as u64;
    let report = rewrite_within(
        &conn,
        &Change::Rename {
            from: "Atlas",
            to: "Atlas 2",
        },
        one + one / 2,
    )
    .unwrap();
    assert_eq!(
        report,
        Report {
            rewritten: 1,
            skipped_large: 1,
            ..Report::default()
        },
        "the first value fits the budget and the second does not"
    );
}

#[test]
fn a_big_value_planted_ahead_of_a_small_one_cannot_use_the_budget_up_before_it() {
    let (_dir, conn) = store();
    // The hostile mod is installed (so scanned) first and holds one value that fits the pass's budget
    // on its own but leaves no room for the honest board after it.
    let hostile = ModRow {
        keys: vec![("board", "content")],
        ..ModRow::signed("acme.aaa", "second", &["storage", PERMISSION])
    };
    let pad = "x".repeat(99_000);
    let big = json!({"widgets": [{"project": "Atlas"}], "pad": pad}).to_string();
    install(&conn, &hostile, board_refs(), &big);
    grant(&conn, "acme.aaa", PERMISSION, "granted");
    install(
        &conn,
        &row(&["storage", PERMISSION], true, "active"),
        board_refs(),
        &board("Atlas"),
    );
    grant(&conn, MOD, PERMISSION, "granted");
    // Room for the big value, not for both.
    let budget = big.len() as u64 + 10;
    assert!(board("Atlas").len() > 10);
    let report = rewrite_within(
        &conn,
        &Change::Rename {
            from: "Atlas",
            to: "Atlas 2",
        },
        budget,
    )
    .unwrap();
    assert_eq!(
        report,
        Report {
            rewritten: 1,
            skipped_large: 1,
            ..Report::default()
        }
    );
    assert_eq!(
        read(&conn).0["widgets"][0]["project"],
        "Atlas 2",
        "the small honest board is reached first"
    );
    let stored: String = conn
        .query_row(
            "SELECT value FROM mod_kv WHERE mod_id = 'acme.aaa'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, big, "the big one is the one left whole");
}

#[test]
fn a_rewrite_that_would_grow_a_value_past_the_cap_leaves_it_whole() {
    let (_dir, conn) = store();
    // About 7 MiB of one-letter project names: 450,000 matches, each of which a long new name would
    // grow by 100 bytes, some 45 MB written under the mutex.
    let item = "{\"project\":\"a\"},";
    let body = item.repeat(7 * 1024 * 1024 / item.len());
    let big = format!("{{\"widgets\":[{}{{}}]}}", body);
    assert!(big.len() as u64 <= MAX_VALUE_BYTES);
    install(
        &conn,
        &row(&["storage", PERMISSION], true, "active"),
        board_refs(),
        &big,
    );
    grant(&conn, MOD, PERMISSION, "granted");
    let report = rewrite_all(
        &conn,
        &Change::Rename {
            from: "a",
            to: &"b".repeat(100),
        },
    )
    .unwrap();
    assert_eq!(
        report,
        Report {
            skipped_large: 1,
            ..Report::default()
        }
    );
    let stored: String = conn
        .query_row("SELECT value FROM mod_kv WHERE mod_id = ?1", [MOD], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(stored, big, "left exactly as it was");
    assert_eq!(read(&conn).1, 1);
    // The same value renamed to a name of the same length is fine.
    let report = rewrite_all(&conn, &Change::Rename { from: "a", to: "b" }).unwrap();
    assert_eq!(report.rewritten, 1);
}

#[test]
fn a_bad_entry_in_the_stored_column_is_skipped_and_the_good_ones_still_apply() {
    let (_dir, conn) = store();
    let refs = json!([
        {"key": "board", "path": "$.widgets[*].project"},
        {"key": "board", "path": "$.a b"},
        {"key": "board", "path": "$.widgets[*]"},
        {"key": "board", "path": ""},
        {"key": "undeclared", "path": "$.widgets[*].project"},
        {"key": 7, "path": "$.x"},
        "not an object",
        {"key": "board"}
    ]);
    install(
        &conn,
        &row(&["storage", PERMISSION], true, "active"),
        refs,
        &board("Atlas"),
    );
    grant(&conn, MOD, PERMISSION, "granted");
    put(&conn, MOD, "undeclared", &board("Atlas"));
    assert_eq!(renamed(&conn).rewritten, 1);
    let (value, _, _) = read(&conn);
    assert_eq!(value["widgets"][0]["project"], "Atlas 2");
    assert_eq!(
        value["widgets"][1]["children"][0]["project"], "Atlas",
        "the children path wasn't in the column"
    );
    let undeclared: String = conn
        .query_row(
            "SELECT value FROM mod_kv WHERE key = 'undeclared'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        undeclared,
        board("Atlas"),
        "a key the manifest doesn't declare is never touched"
    );

    // A column that isn't a list at all is nothing to do.
    conn.execute("UPDATE mods SET project_refs = 'garbage'", [])
        .unwrap();
    assert_eq!(renamed(&conn), Report::default());
    conn.execute("UPDATE mods SET project_refs = '{\"key\":\"board\"}'", [])
        .unwrap();
    assert_eq!(renamed(&conn), Report::default());
}

#[test]
fn a_value_that_is_not_json_or_has_no_row_is_left_and_the_pass_goes_on() {
    let (_dir, conn) = store();
    install(
        &conn,
        &row(&["storage", PERMISSION], true, "active"),
        board_refs(),
        "not json {",
    );
    grant(&conn, MOD, PERMISSION, "granted");
    assert_eq!(renamed(&conn), Report::default());
    conn.execute("DELETE FROM mod_kv", []).unwrap();
    assert_eq!(renamed(&conn), Report::default());
}

#[test]
fn only_the_mods_that_hold_the_right_are_touched_among_several() {
    let (_dir, conn) = store();
    install(
        &conn,
        &row(&["storage", PERMISSION], true, "active"),
        board_refs(),
        &board("Atlas"),
    );
    grant(&conn, MOD, PERMISSION, "granted");
    // Another mod that says it reads projects and was refused.
    let other = ModRow {
        keys: vec![("board", "content")],
        ..ModRow::signed("acme.other", "second", &["storage", PERMISSION])
    };
    install(&conn, &other, board_refs(), &board("Atlas"));
    grant(&conn, "acme.other", PERMISSION, "denied");
    assert_eq!(renamed(&conn).rewritten, 1);
    let theirs: String = conn
        .query_row(
            "SELECT value FROM mod_kv WHERE mod_id = 'acme.other'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(theirs, board("Atlas"));
}

// ---- through the satellites the four project commands call ----------------------------------------------

/// The legacy board as the old Pinboard stored it, with a timeline bound to `project` and a folder
/// holding another.
fn legacy(project: &str) -> String {
    json!({"widgets": [
        {"id": "a", "kind": "timeline", "project": project},
        {"id": "f", "kind": "folder", "children": [{"id": "c", "kind": "timeline", "project": project}]}
    ]})
    .to_string()
}

/// The two homes of a board: the legacy row and a mod that holds the right to have its refs rewritten.
fn both_boards(conn: &Connection, project: &str) {
    crate::db::set_setting(conn, "pinboard", &legacy(project)).unwrap();
    install(
        conn,
        &row(&["storage", PERMISSION], true, "active"),
        board_refs(),
        &board(project),
    );
    grant(conn, MOD, PERMISSION, "granted");
}

fn legacy_row(conn: &Connection) -> String {
    crate::db::get_setting(conn, "pinboard").unwrap().unwrap()
}

fn project(conn: &Connection, name: &str) -> i64 {
    crate::entities::resolve_project(conn, name, true)
        .unwrap()
        .unwrap()
}

#[test]
fn a_rename_through_rename_entity_rewrites_the_legacy_board_and_the_mods_refs() {
    let (_dir, conn) = store();
    both_boards(&conn, "Atlas");
    let id = project(&conn, "Atlas");
    // What `rename_entity` does inside its transaction.
    let old = crate::entities::canonical_name(&conn, id).unwrap();
    let canonical = crate::entities::rename_entity(&conn, id, "Atlas 2").unwrap();
    crate::projects::rename_project_satellites(&conn, &old, &canonical).unwrap();

    assert_eq!(legacy_row(&conn), legacy("Atlas 2"));
    let (value, revision, _) = read(&conn);
    assert_eq!(
        value,
        serde_json::from_str::<Value>(&board("Atlas 2")).unwrap()
    );
    assert_eq!(revision, 2, "a frame holding the old copy is now stale");
}

#[test]
fn a_merge_through_merge_entities_or_merge_projects_rewrites_both_boards() {
    // `merge_projects` resolves two names and `merge_entities` takes two ids; both then run this.
    let (_dir, conn) = store();
    both_boards(&conn, "Atlas");
    let from = project(&conn, "Atlas");
    let into = project(&conn, "Marketing");
    let old = crate::entities::canonical_name(&conn, from).unwrap();
    crate::entities::merge_entities(&conn, from, into).unwrap();
    let canonical = crate::entities::canonical_name(&conn, into).unwrap();
    assert_eq!(canonical, "Marketing");
    crate::projects::rename_project_satellites(&conn, &old, &canonical).unwrap();

    assert_eq!(legacy_row(&conn), legacy("Marketing"));
    let (value, revision, _) = read(&conn);
    assert_eq!(
        value,
        serde_json::from_str::<Value>(&board("Marketing")).unwrap()
    );
    assert_eq!(revision, 2);
}

#[test]
fn a_delete_nulls_both_boards_bindings_and_bumps_the_revision() {
    let (_dir, conn) = store();
    both_boards(&conn, "Atlas");
    project(&conn, "Atlas");
    crate::projects::delete_project_satellites(&conn, "Atlas").unwrap();

    assert_eq!(
        serde_json::from_str::<Value>(&legacy_row(&conn)).unwrap(),
        json!({"widgets": [
            {"id": "a", "kind": "timeline", "project": null},
            {"id": "f", "kind": "folder", "children": [{"id": "c", "kind": "timeline", "project": null}]}
        ]})
    );
    let (value, revision, _) = read(&conn);
    assert_eq!(value["widgets"][0]["project"], Value::Null);
    assert_eq!(value["widgets"][1]["children"][0]["project"], Value::Null);
    assert_eq!(revision, 2);
}

#[test]
fn the_legacy_row_is_rewritten_before_the_move_and_left_alone_after_it() {
    let (_dir, conn) = store();
    both_boards(&conn, "Atlas");
    project(&conn, "Atlas");

    // Before the move is recorded: today's behaviour, and the mod's copy too.
    crate::projects::rename_project_satellites(&conn, "Atlas", "Atlas 2").unwrap();
    assert_eq!(legacy_row(&conn), legacy("Atlas 2"));
    assert_eq!(read(&conn).1, 2);

    // The move is recorded: the legacy row is a frozen snapshot, byte for byte, through a rename and
    // a delete; the mod's copy keeps being rewritten.
    crate::db::set_setting(&conn, "pinboard_moved_sha", &"a".repeat(64)).unwrap();
    let frozen = legacy_row(&conn);
    crate::projects::rename_project_satellites(&conn, "Atlas 2", "Atlas 3").unwrap();
    assert_eq!(
        legacy_row(&conn),
        frozen,
        "a rename leaves the snapshot alone"
    );
    assert_eq!(read(&conn).0["widgets"][0]["project"], "Atlas 3");
    // The snapshot still binds "Atlas 2": a delete of that name must not touch it (and the mod, which
    // holds "Atlas 3" now, has nothing to change).
    crate::projects::delete_project_satellites(&conn, "Atlas 2").unwrap();
    assert_eq!(legacy_row(&conn), frozen, "and so does a delete");
    assert_eq!(read(&conn).1, 3);
    crate::projects::delete_project_satellites(&conn, "Atlas 3").unwrap();
    assert_eq!(read(&conn).0["widgets"][0]["project"], Value::Null);
    assert_eq!(read(&conn).1, 4);

    // A blank marker is no record of a move.
    crate::db::set_setting(&conn, "pinboard_moved_sha", "  ").unwrap();
    crate::projects::rename_project_satellites(&conn, "Atlas 2", "Atlas 4").unwrap();
    assert_eq!(legacy_row(&conn), legacy("Atlas 4"));
}

#[test]
fn a_mod_row_with_no_board_key_yet_costs_the_legacy_rewrite_nothing() {
    let (_dir, conn) = store();
    crate::db::set_setting(&conn, "pinboard", &legacy("Atlas")).unwrap();
    // The mod is installed (refs declared, grant held) but has stored nothing.
    install(
        &conn,
        &row(&["storage", PERMISSION], true, "active"),
        board_refs(),
        &board("Atlas"),
    );
    grant(&conn, MOD, PERMISSION, "granted");
    conn.execute("DELETE FROM mod_kv", []).unwrap();
    project(&conn, "Atlas");
    crate::projects::rename_project_satellites(&conn, "Atlas", "Atlas 2").unwrap();
    assert_eq!(legacy_row(&conn), legacy("Atlas 2"));
    assert_eq!(
        conn.query_row("SELECT count(*) FROM mod_kv", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0,
        "nothing is made up for a mod that stored nothing"
    );
}
