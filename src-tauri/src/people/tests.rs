// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tests for the people core. Every person here is invented: example.com addresses, 555-01xx
//! numbers, made-up names.

use rusqlite::{params, Connection};
use serde_json::{json, Value};

use super::sources::{retire, tombstone_snapshot};
use super::*;

const KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

fn store() -> (tempfile::TempDir, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::db::open(&dir.path().join("t.sqlite"), KEY).unwrap();
    (dir, conn)
}

fn draft(given: &str, family: &str) -> PersonDraft {
    PersonDraft {
        given_name: given.into(),
        family_name: family.into(),
        ..Default::default()
    }
}

fn detail(kind: &str, value: &str) -> DetailDraft {
    DetailDraft {
        kind: kind.into(),
        value: value.into(),
        ..Default::default()
    }
}

fn person(conn: &Connection, given: &str, family: &str) -> i64 {
    create_person(conn, &draft(given, family)).unwrap()
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

fn text(conn: &Connection, sql: &str) -> String {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

fn names(rows: &[PersonRow]) -> Vec<String> {
    rows.iter().map(|r| r.display_name.clone()).collect()
}

fn age_in_trash(conn: &Connection, id: i64, days: i64) {
    conn.execute(
        "UPDATE people SET trashed_at = strftime('%Y-%m-%dT%H:%M:%fZ','now', ?2) WHERE id = ?1",
        params![id, format!("-{days} days")],
    )
    .unwrap();
}

fn add_source(
    conn: &Connection,
    person_id: i64,
    provider: &str,
    external: Option<&str>,
    state: &str,
    snapshot: Option<&str>,
) -> i64 {
    conn.execute(
        "INSERT INTO person_sources(person_id, provider, account, external_id, etag, snapshot, \
                                    conflicts, state, dirty_at, synced_at) \
         VALUES (?1, ?2, 'me@example.com', ?3, 'etag-1', ?4, '[{\"field\":\"x\"}]', ?5, \
                 '2026-11-01T00:00:00.000Z', '2026-11-01T00:00:00.000Z')",
        params![person_id, provider, external, snapshot, state],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn add_meeting(conn: &Connection, on_date: &str, note: &str, people: &[i64]) -> i64 {
    conn.execute(
        "INSERT INTO interactions(kind, on_date, note) VALUES ('met', ?1, ?2)",
        params![on_date, note],
    )
    .unwrap();
    let id = conn.last_insert_rowid();
    for p in people {
        conn.execute(
            "INSERT INTO interaction_people(interaction_id, person_id) VALUES (?1, ?2)",
            params![id, p],
        )
        .unwrap();
    }
    id
}

/// Every `table.column` whose text holds `needle` (case-insensitively), over every real table.
fn where_found(conn: &Connection, needle: &str) -> Vec<String> {
    let tables: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' \
                   AND name NOT LIKE 'sqlite_%' AND sql NOT LIKE 'CREATE VIRTUAL%'",
            )
            .unwrap();
        let rows = stmt.query_map([], |r| r.get(0)).unwrap();
        rows.map(|r| r.unwrap()).collect()
    };
    let mut hits = Vec::new();
    for table in tables {
        let cols: Vec<String> = {
            let mut stmt = conn
                .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
                .unwrap();
            let rows = stmt.query_map([], |r| r.get(0)).unwrap();
            rows.map(|r| r.unwrap()).collect()
        };
        for col in cols {
            let n: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM \"{table}\" WHERE CAST(\"{col}\" AS TEXT) LIKE ?1"
                    ),
                    params![format!("%{needle}%")],
                    |r| r.get(0),
                )
                .unwrap();
            if n > 0 {
                hits.push(format!("{table}.{col}"));
            }
        }
    }
    hits
}

// --- identity and names -----------------------------------------------------------------------------------

#[test]
fn two_people_with_one_name_coexist() {
    let (_d, conn) = store();
    let a = person(&conn, "Sam", "Reed");
    let b = person(&conn, "Sam", "Reed");
    assert_ne!(a, b);
    let (pa, pb) = (get_person(&conn, a).unwrap(), get_person(&conn, b).unwrap());
    assert_ne!(pa.ident, pb.ident, "each is minted its own uid");
    assert_eq!(pa.ident.len(), 32);
    assert!(pa
        .ident
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
    let all = list_people(&conn, ListFilter::All, None, ListSort::First).unwrap();
    assert_eq!(names(&all), ["Sam Reed", "Sam Reed"]);
}

#[test]
fn every_write_refreshes_the_search_and_sort_keys() {
    let (_d, conn) = store();
    let mut d = draft("Élodie", "Martin");
    d.details = vec![
        detail("email", " Elodie.M@Example.COM "),
        detail("phone", "+44 (0)20 7946 0123"),
        detail("nickname", "Ellie"),
        detail("work", "Ünited Widgets"),
        detail("web", "Not In The Key"),
    ];
    let id = create_person(&conn, &d).unwrap();
    let (sort, search): (String, String) = conn
        .query_row(
            "SELECT sort_key, search_key FROM people WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(sort, "elodie martin");
    let (words, phones) = search.split_once(fold::KEY_SPLIT).unwrap();
    for want in [
        "elodie martin",
        "ellie",
        "united widgets",
        "elodie.m@example.com",
    ] {
        assert!(words.contains(want), "{want}");
    }
    assert!(!words.contains("not in the key"));
    assert_eq!(phones, "+4402079460123");
    let keys: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare("SELECT kind, value_key FROM person_details WHERE person_id = ?1 ORDER BY id")
            .unwrap();
        let rows = stmt
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        rows.map(|r| r.unwrap()).collect()
    };
    assert_eq!(keys[0], ("email".into(), "elodie.m@example.com".into()));
    assert_eq!(keys[1], ("phone".into(), "+4402079460123".into()));
    assert_eq!(keys[3], ("work".into(), "united widgets".into()));

    // A save changes every key with it.
    let p = get_person(&conn, id).unwrap();
    let mut next = draft("Zoë", "Martin");
    next.display_name = "Zoë Martin".into();
    next.details = vec![detail("email", "zoe@example.com")];
    update_person(&conn, id, &next, &p.updated_at).unwrap();
    let (sort, search): (String, String) = conn
        .query_row(
            "SELECT sort_key, search_key FROM people WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(sort, "zoe martin");
    assert!(search.contains("zoe@example.com"));
    assert!(!search.contains("elodie") && !search.contains("4402079460123"));
}

#[test]
fn a_blank_name_is_composed_from_the_details() {
    let (_d, conn) = store();
    let make = |details: Vec<DetailDraft>| {
        let id = create_person(
            &conn,
            &PersonDraft {
                details,
                ..Default::default()
            },
        )
        .unwrap();
        get_person(&conn, id).unwrap().display_name
    };
    // The order: nickname, then company, then email, then phone.
    let all = vec![
        detail("phone", "555 0101"),
        detail("email", "p@example.com"),
        detail("work", "Example Works"),
        detail("nickname", "Pip"),
    ];
    assert_eq!(make(all.clone()), "Pip");
    assert_eq!(make(all[..3].to_vec()), "Example Works");
    assert_eq!(make(all[..2].to_vec()), "p@example.com");
    assert_eq!(make(all[..1].to_vec()), "555 0101");
    // Parts win over every detail, joined prefix given middle family suffix.
    let mut d = PersonDraft {
        name_prefix: "Dr.".into(),
        given_name: "Ada".into(),
        middle_name: "M".into(),
        family_name: "Quill".into(),
        name_suffix: "PhD".into(),
        details: all,
        ..Default::default()
    };
    let id = create_person(&conn, &d).unwrap();
    assert_eq!(
        get_person(&conn, id).unwrap().display_name,
        "Dr. Ada M Quill PhD"
    );
    // Nothing to go on is refused, and writes nothing.
    d = PersonDraft::default();
    let before = count(&conn, "SELECT COUNT(*) FROM people");
    assert!(create_person(&conn, &d).is_err());
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM people"), before);
}

#[test]
fn a_rename_of_the_given_name_updates_the_display_name() {
    let (_d, conn) = store();
    let id = person(&conn, "Sam", "Reed");
    let p = get_person(&conn, id).unwrap();
    assert_eq!(p.display_name, "Sam Reed");
    // The editor sends the display name it was shown, unchanged.
    let mut edit = draft("Samuel", "Reed");
    edit.display_name = p.display_name.clone();
    let saved = update_person(&conn, id, &edit, &p.updated_at).unwrap();
    assert_eq!(saved.display_name, "Samuel Reed");
    assert_eq!(get_person(&conn, id).unwrap().display_name, "Samuel Reed");
    let found = search_people(&conn, "samuel", 10).unwrap();
    assert_eq!(names(&found), ["Samuel Reed"]);
    // The old spelling left the key (words match as substrings, so "sam" alone would still hit).
    let key = text(&conn, "SELECT search_key FROM people");
    assert!(key.starts_with("samuel reed samuel reed"));
}

#[test]
fn a_typed_display_name_survives_a_rename() {
    let (_d, conn) = store();
    let mut d = draft("Margaret", "Reed");
    d.display_name = "Mum".into();
    let id = create_person(&conn, &d).unwrap();
    let p = get_person(&conn, id).unwrap();
    assert_eq!(p.display_name, "Mum");
    let mut edit = draft("Maggie", "Reed");
    edit.display_name = "Mum".into();
    let saved = update_person(&conn, id, &edit, &p.updated_at).unwrap();
    assert_eq!(saved.display_name, "Mum");
    // A new typed name replaces it, and then that one is the typed one.
    let mut edit = draft("Maggie", "Reed");
    edit.display_name = "Mother".into();
    let saved = update_person(&conn, id, &edit, &saved.updated_at).unwrap();
    assert_eq!(saved.display_name, "Mother");
    // Clearing it hands the name back to the parts.
    let saved = update_person(&conn, id, &draft("Maggie", "Reed"), &saved.updated_at).unwrap();
    assert_eq!(saved.display_name, "Maggie Reed");
}

// --- folding ---------------------------------------------------------------------------------------------

#[test]
fn an_unaccented_query_finds_an_accented_name() {
    let (_d, conn) = store();
    person(&conn, "Élodie", "Martin");
    person(&conn, "Eloise", "Brun");
    for q in ["elodie", "ELODIE", "Élodie", "  elodie   mart ", "lodie"] {
        assert_eq!(
            names(&search_people(&conn, q, 10).unwrap()),
            ["Élodie Martin"],
            "{q}"
        );
    }
    assert_eq!(search_people(&conn, "e", 10).unwrap().len(), 2);
    assert!(search_people(&conn, "elodie nobody", 10)
        .unwrap()
        .is_empty());
}

#[test]
fn dakuten_names_do_not_fold_together() {
    let (_d, conn) = store();
    person(&conn, "ばば", "テスト");
    person(&conn, "はは", "テスト");
    assert_ne!(fold::fold_key("ば"), fold::fold_key("は"));
    assert_eq!(
        names(&search_people(&conn, "ばば", 10).unwrap()),
        ["ばば テスト"]
    );
    assert_eq!(
        names(&search_people(&conn, "はは", 10).unwrap()),
        ["はは テスト"]
    );
}

#[test]
fn a_devanagari_name_keeps_its_marks() {
    let (_d, conn) = store();
    let id = person(&conn, "कुमार", "शर्मा");
    let p = get_person(&conn, id).unwrap();
    assert_eq!(p.given_name, "कुमार");
    assert_eq!(p.family_name, "शर्मा");
    assert_eq!(
        names(&search_people(&conn, "कुमार", 10).unwrap()),
        ["कुमार शर्मा"]
    );
    // The bare consonant is a different letter, not the same word without its vowel sign.
    assert!(search_people(&conn, "कमार", 10).unwrap().is_empty());
    assert_ne!(fold::fold_key("कु"), fold::fold_key("क"));
}

#[test]
fn a_zwnj_name_round_trips() {
    let (_d, conn) = store();
    let zwnj = "می\u{200c}خواهم";
    let id = person(&conn, zwnj, "Example");
    assert_eq!(get_person(&conn, id).unwrap().given_name, zwnj);
    let joiner = "A\u{200d}B";
    let id = person(&conn, joiner, "Example");
    assert_eq!(get_person(&conn, id).unwrap().given_name, joiner);
    assert_eq!(names(&search_people(&conn, zwnj, 10).unwrap()).len(), 1);
}

#[test]
fn invisible_bidi_characters_never_reach_a_name() {
    let (_d, conn) = store();
    let mut d = draft("Sa\u{202e}m", "Re\u{200f}ed\u{feff}");
    d.notes = "line one\r\nline\u{202e} two\u{0007}".into();
    d.details = vec![
        detail("email", "a\u{2066}b@example.com"),
        DetailDraft {
            kind: "address".into(),
            value: "1 Example Road\nLeeds\u{2028}".into(),
            parts: Some(json!({"street": "1 Example\u{202e} Road", "extra_key": "kept"})),
            ..Default::default()
        },
    ];
    let id = create_person(&conn, &d).unwrap();
    let p = get_person(&conn, id).unwrap();
    assert_eq!(p.display_name, "Sam Reed");
    assert_eq!(p.given_name, "Sam");
    assert_eq!(p.notes, "line one\nline two");
    assert_eq!(
        p.details[0].value, "1 Example Road\nLeeds",
        "an address keeps its newline"
    );
    assert_eq!(
        p.details[0].parts.as_ref().unwrap()["street"],
        "1 Example Road"
    );
    assert_eq!(p.details[0].parts.as_ref().unwrap()["extra_key"], "kept");
    assert_eq!(p.details[1].value, "ab@example.com");
    // A label is a value too.
    let label = create_label(&conn, "Fam\u{202a}ily").unwrap();
    assert_eq!(list_labels(&conn).unwrap()[0].name, "Family");
    assert!(label > 0);
    assert!(where_found(&conn, "\u{202e}").is_empty());
}

// --- dates ---------------------------------------------------------------------------------------------------

#[test]
fn a_year_less_birthday_round_trips() {
    let (_d, conn) = store();
    let mut d = draft("Ivy", "Hale");
    d.born_on = Some("--04-15".into());
    let id = create_person(&conn, &d).unwrap();
    assert_eq!(
        get_person(&conn, id).unwrap().born_on.as_deref(),
        Some("--04-15")
    );
    let p = get_person(&conn, id).unwrap();
    let mut edit = draft("Ivy", "Hale");
    edit.born_on = Some("1990-04-15".into());
    update_person(&conn, id, &edit, &p.updated_at).unwrap();
    assert_eq!(
        get_person(&conn, id).unwrap().born_on.as_deref(),
        Some("1990-04-15")
    );
    // A partial birthday is no birthday.
    let mut bad = draft("Ivy", "Hale");
    bad.born_on = Some("1990-04".into());
    assert!(create_person(&conn, &bad).is_err());
}

#[test]
fn the_list_rows_carry_the_birthday_for_the_upcoming_strip() {
    let (_d, conn) = store();
    let mut d = draft("Ivy", "Hale");
    d.born_on = Some("--04-15".into());
    create_person(&conn, &d).unwrap();
    person(&conn, "Noel", "Park");
    let rows = list_people(&conn, ListFilter::All, None, ListSort::First).unwrap();
    let born: Vec<Option<&str>> = rows.iter().map(|r| r.born_on.as_deref()).collect();
    assert_eq!(born, vec![Some("--04-15"), None]);
}

#[test]
fn a_year_less_29_february_is_valid() {
    let (_d, conn) = store();
    let mut d = draft("Leap", "Day");
    d.born_on = Some("--02-29".into());
    let id = create_person(&conn, &d).unwrap();
    assert_eq!(
        get_person(&conn, id).unwrap().born_on.as_deref(),
        Some("--02-29")
    );
    d.born_on = Some("2024-02-29".into());
    assert!(create_person(&conn, &d).is_ok());
    d.born_on = Some("2023-02-29".into());
    assert!(create_person(&conn, &d).is_err());
    d.born_on = Some("--02-30".into());
    assert!(create_person(&conn, &d).is_err());
}

#[test]
fn a_date_you_met_is_checked_for_shape_not_for_the_clock() {
    let (_d, conn) = store();
    let mut d = draft("Ivy", "Hale");
    d.met_on = Some("2019-06".into());
    assert!(create_person(&conn, &d).is_ok());
    // A clock or zone ahead of the real date must never lock a record.
    d.met_on = Some("2999".into());
    assert!(create_person(&conn, &d).is_ok());
    d.met_on = Some("2019-13".into());
    assert!(create_person(&conn, &d).is_err());
}

// --- limits -------------------------------------------------------------------------------------------------

#[test]
fn values_over_google_limits_are_refused_before_saving() {
    let (_d, conn) = store();
    let before = count(&conn, "SELECT COUNT(*) FROM people");

    let mut d = draft("Sam", "Reed");
    d.details = vec![detail("email", &"x".repeat(1_024))];
    assert!(
        create_person(&conn, &d).is_ok(),
        "exactly the limit is fine"
    );
    d.details = vec![detail("email", &"x".repeat(1_025))];
    assert!(create_person(&conn, &d).is_err());
    d.details = vec![];
    d.given_name = "y".repeat(1_025);
    assert!(create_person(&conn, &d).is_err());
    let mut d = draft("Sam", "Reed");
    d.notes = "n".repeat(65_536);
    assert!(create_person(&conn, &d).is_ok());
    d.notes = "n".repeat(65_537);
    assert!(create_person(&conn, &d).is_err());
    let mut d = draft("Sam", "Reed");
    d.details = (0..500)
        .map(|i| detail("custom", &format!("v{i}")))
        .collect();
    assert!(create_person(&conn, &d).is_ok());
    d.details.push(detail("custom", "one too many"));
    assert!(create_person(&conn, &d).is_err());
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM people"),
        before + 3,
        "a refusal writes nothing, not even a half-made person"
    );
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM person_details"),
        1 + 500,
        "the refused ones left no details behind"
    );

    // 64 labels per person.
    let id = person(&conn, "Label", "Lover");
    for i in 0..64 {
        let l = create_label(&conn, &format!("label {i}")).unwrap();
        assert_eq!(label_people(&conn, &[id], l).unwrap(), 1);
    }
    let one_more = create_label(&conn, "one more").unwrap();
    assert!(label_people(&conn, &[id], one_more).is_err());
    assert_eq!(get_person(&conn, id).unwrap().labels.len(), 64);

    // 25,000 people per store.
    let room = 25_000 - count(&conn, "SELECT COUNT(*) FROM people");
    conn.execute(
        "WITH RECURSIVE c(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM c WHERE n < ?1) \
         INSERT INTO people(uid, display_name) SELECT printf('fill%028d', n), 'Filler' FROM c",
        params![room],
    )
    .unwrap();
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM people"), 25_000);
    assert!(create_person(&conn, &draft("Over", "Cap")).is_err());
}

// --- errors never carry a value ------------------------------------------------------------------------------

#[test]
fn a_sentinel_name_never_reaches_an_error_or_debug_string() {
    let (_d, conn) = store();
    let s = "Zqvxsentinelname";
    let id = {
        let mut d = draft(s, s);
        d.notes = s.into();
        d.met_where = s.into();
        d.details = vec![
            detail("email", &format!("{s}@example.com")),
            detail("phone", s),
        ];
        create_person(&conn, &d).unwrap()
    };
    let p = get_person(&conn, id).unwrap();

    let mut failures: Vec<Error> = Vec::new();
    let with = |f: &dyn Fn(&mut PersonDraft)| {
        let mut d = draft(s, s);
        f(&mut d);
        d
    };
    for d in [
        with(&|d| d.born_on = Some(s.into())),
        with(&|d| d.met_on = Some(s.into())),
        with(&|d| d.details = vec![detail(s, s)]),
        with(&|d| d.details = vec![detail("email", &format!("{s}{}", "x".repeat(1_100)))]),
        with(&|d| d.given_name = format!("{s}{}", "x".repeat(1_100))),
        with(&|d| d.notes = format!("{s}{}", "x".repeat(70_000))),
        with(&|d| d.details = vec![detail("date", s)]),
        with(&|d| {
            d.details = vec![DetailDraft {
                kind: "address".into(),
                value: s.into(),
                parts: Some(Value::String(s.into())),
                ..Default::default()
            }]
        }),
        with(&|d| {
            d.details = vec![DetailDraft {
                id: Some(999_999),
                kind: "email".into(),
                value: s.into(),
                ..Default::default()
            }]
        }),
        with(&|d| {
            d.details = vec![
                DetailDraft {
                    id: Some(1),
                    kind: "email".into(),
                    value: s.into(),
                    ..Default::default()
                },
                DetailDraft {
                    id: Some(1),
                    kind: "email".into(),
                    value: s.into(),
                    ..Default::default()
                },
            ]
        }),
    ] {
        failures.push(create_person(&conn, &d).unwrap_err());
        failures.push(update_person(&conn, id, &d, &p.updated_at).unwrap_err());
    }
    failures.push(update_person(&conn, id, &draft(s, s), "2000-01-01T00:00:00.000Z").unwrap_err());
    failures.push(update_person(&conn, 987_654, &draft(s, s), "x").unwrap_err());
    failures.push(get_person(&conn, 987_654).unwrap_err());
    failures.push(create_label(&conn, "").unwrap_err());
    create_label(&conn, s).unwrap();
    failures.push(create_label(&conn, s).unwrap_err());
    failures.push(create_label(&conn, &s.to_uppercase()).unwrap_err());
    failures.push(rename_label(&conn, 987_654, s).unwrap_err());
    failures
        .push(link_person_project(&conn, id, &format!("{s}{}", "x".repeat(1_100))).unwrap_err());
    failures.push(link_person_project(&conn, 987_654, s).unwrap_err());
    failures.push(trash_people(&conn, &[id], &[987_654]).unwrap_err());
    for e in &failures {
        for shown in [e.to_string(), format!("{e:?}")] {
            assert!(
                !shown.to_lowercase().contains("sentinel"),
                "a value reached an error"
            );
        }
    }
    assert!(failures.len() > 20, "the table above ran");

    // The Debug of everything that holds a value prints ids and counts.
    let row = &list_people(&conn, ListFilter::All, None, ListSort::First).unwrap()[0];
    let mut shown = vec![
        format!("{p:?}"),
        format!("{row:?}"),
        format!("{:?}", p.details),
        format!("{:?}", p.labels),
        format!("{:?}", draft(s, s)),
        format!("{:?}", detail("email", s)),
        format!("{:?}", list_labels(&conn).unwrap()),
        format!(
            "{:?}",
            update_person(&conn, id, &draft(s, s), &p.updated_at).unwrap()
        ),
    ];
    let source = add_source(&conn, id, "vcard", None, "linked", Some("{}"));
    shown.push(format!("{:?}", sources::sources_of(&conn, id).unwrap()));
    assert!(source > 0);
    for one in shown {
        assert!(
            !one.to_lowercase().contains("sentinel"),
            "a value reached a Debug"
        );
    }
}

// --- trash, restore, purge -----------------------------------------------------------------------------------

#[test]
fn a_trashed_person_is_invisible_to_search_and_lists() {
    let (_d, conn) = store();
    let keep = person(&conn, "Kay", "Kept");
    let gone = person(&conn, "Tess", "Trashed");
    let label = create_label(&conn, "Friends").unwrap();
    label_people(&conn, &[keep, gone], label).unwrap();
    star_people(&conn, &[keep, gone], true).unwrap();
    assert_eq!(trash_people(&conn, &[gone], &[]).unwrap(), 1);

    assert_eq!(names(&search_people(&conn, "tess", 10).unwrap()).len(), 0);
    for filter in [
        ListFilter::All,
        ListFilter::Starred,
        ListFilter::Label(label),
    ] {
        assert_eq!(
            names(&list_people(&conn, filter, None, ListSort::First).unwrap()),
            ["Kay Kept"],
            "{filter:?}"
        );
    }
    assert!(
        list_people(&conn, ListFilter::Unlabelled, None, ListSort::First)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        names(&list_people(&conn, ListFilter::Trash, None, ListSort::First).unwrap()),
        ["Tess Trashed"]
    );
    assert_eq!(
        list_labels(&conn).unwrap()[0].people,
        1,
        "the label counts only people outside Trash"
    );
    // Their record still opens from Trash, and an edit refuses.
    let p = get_person(&conn, gone).unwrap();
    assert!(p.trashed_at.is_some());
    assert!(update_person(&conn, gone, &draft("Tess", "Trashed"), &p.updated_at).is_err());
}

#[test]
fn a_restore_from_trash_returns_the_person_to_search_and_lists() {
    let (_d, conn) = store();
    let id = person(&conn, "Tess", "Trashed");
    trash_people(&conn, &[id], &[]).unwrap();
    assert!(search_people(&conn, "tess", 10).unwrap().is_empty());
    assert_eq!(restore_people(&conn, &[id, id, 424_242]).unwrap(), 1);
    assert_eq!(
        names(&search_people(&conn, "tess", 10).unwrap()),
        ["Tess Trashed"]
    );
    assert_eq!(
        names(&list_people(&conn, ListFilter::All, None, ListSort::First).unwrap()),
        ["Tess Trashed"]
    );
    assert!(list_people(&conn, ListFilter::Trash, None, ListSort::First)
        .unwrap()
        .is_empty());
    assert_eq!(restore_people(&conn, &[id]).unwrap(), 0, "already back");
}

#[test]
fn trashing_twice_keeps_the_first_date() {
    let (_d, conn) = store();
    let id = person(&conn, "Tess", "Trashed");
    trash_people(&conn, &[id], &[]).unwrap();
    age_in_trash(&conn, id, 10);
    let first = text(&conn, "SELECT trashed_at FROM people");
    assert_eq!(trash_people(&conn, &[id], &[]).unwrap(), 0);
    assert_eq!(text(&conn, "SELECT trashed_at FROM people"), first);
}

#[test]
fn a_purged_person_leaves_no_personal_data() {
    let (_d, conn) = store();
    let mut d = draft("Zqvx", "Sentinelname");
    d.notes = "zqvx notes".into();
    d.met_where = "zqvx cafe".into();
    d.born_on = Some("--04-15".into());
    d.details = vec![
        detail("email", "zqvx@example.com"),
        detail("phone", "+1 555 0142"),
        DetailDraft {
            kind: "address".into(),
            value: "1 Zqvx Road".into(),
            parts: Some(json!({"street": "1 Zqvx Road"})),
            ..Default::default()
        },
        detail("nickname", "Zq"),
        detail("work", "Zqvx Co"),
    ];
    let doomed = create_person(&conn, &d).unwrap();
    let survivor = person(&conn, "Una", "Survives");
    let doomed_ident: String = text(
        &conn,
        &format!("SELECT uid FROM people WHERE id = {doomed}"),
    );
    let survivor_ident: String = text(
        &conn,
        &format!("SELECT uid FROM people WHERE id = {survivor}"),
    );
    let folded = "f".repeat(32);
    conn.execute(
        "UPDATE people SET merged_uids = ?2 WHERE id = ?1",
        params![doomed, json!([folded]).to_string()],
    )
    .unwrap();
    let label = create_label(&conn, "Friends").unwrap();
    label_people(&conn, &[doomed, survivor], label).unwrap();
    link_person_project(&conn, doomed, "Atlas").unwrap();
    conn.execute(
        "INSERT INTO person_distinct_pairs(low_uid, high_uid) VALUES (?1, ?2), (?3, ?2)",
        params![doomed_ident, survivor_ident, folded],
    )
    .unwrap();
    // Two more pairs that do not name the purged person survive.
    conn.execute(
        "INSERT INTO person_distinct_pairs(low_uid, high_uid) VALUES ('aaa', 'bbb')",
        [],
    )
    .unwrap();
    let shared = add_meeting(&conn, "2026-10-01", "lunch together", &[doomed, survivor]);
    let alone = add_meeting(&conn, "2026-10-02", "zqvx one to one", &[doomed]);
    let nobody = add_meeting(&conn, "2026-10-03", "an entry that never had a person", &[]);
    add_source(
        &conn,
        doomed,
        "vcard",
        Some("zqvx-card-uid"),
        "linked",
        Some(r#"{"name":"Zqvx Sentinelname"}"#),
    );
    add_source(
        &conn,
        doomed,
        "google",
        Some("people/c777"),
        "linked",
        Some(
            &json!({"names": [{"givenName": "Zqvx"}], "emailAddresses": [{"value": "zqvx@example.com"}],
                    "metadata": {"sources": [{"type": "CONTACT", "id": "src777"}]}})
            .to_string(),
        ),
    );
    trash_people(&conn, &[doomed], &[]).unwrap();
    age_in_trash(&conn, doomed, 31);
    assert!(
        !where_found(&conn, "zqvx").is_empty(),
        "the fixture holds the name"
    );

    // A recent Trash entry is not due.
    let recent = person(&conn, "Recent", "Trash");
    trash_people(&conn, &[recent], &[]).unwrap();

    let now_text = crate::ingest::iso_now(&conn).unwrap();
    assert_eq!(purge_expired(&conn, &now_text).unwrap(), 1);

    assert_eq!(
        count(
            &conn,
            &format!("SELECT COUNT(*) FROM people WHERE id = {doomed}")
        ),
        0
    );
    for needle in ["zqvx", "5550142", "sentinel"] {
        assert_eq!(where_found(&conn, needle), Vec::<String>::new(), "{needle}");
    }
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM person_details"), 0);
    // The label, the project and the survivor remain; the survivor still carries the label.
    assert_eq!(list_labels(&conn).unwrap()[0].people, 1);
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM projects WHERE name = 'Atlas'"),
        1
    );
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM person_projects"), 0);
    assert!(get_person(&conn, survivor).is_ok());
    assert!(
        get_person(&conn, recent).is_ok(),
        "a Trash entry under 30 days stays"
    );
    // Interactions: the shared one stays with the survivor, the lone one goes, the personless one stays.
    assert_eq!(
        count(
            &conn,
            &format!("SELECT COUNT(*) FROM interactions WHERE id = {shared}")
        ),
        1
    );
    assert_eq!(
        count(
            &conn,
            &format!("SELECT COUNT(*) FROM interactions WHERE id = {alone}")
        ),
        0
    );
    assert_eq!(
        count(
            &conn,
            &format!("SELECT COUNT(*) FROM interactions WHERE id = {nobody}")
        ),
        1
    );
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM interaction_people"), 1);
    // The distinct pairs naming the purged uids are gone; the unrelated pair is not.
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM person_distinct_pairs"),
        1
    );
    // The vCard link is gone; the Google link is a tombstone of ids.
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM person_sources WHERE provider = 'vcard'"
        ),
        0
    );
    type TombstoneRow = (
        Option<i64>,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        String,
    );
    let (person_id, state, external, etag, conflicts, dirty, synced, acct, snapshot): TombstoneRow = conn
        .query_row(
            "SELECT person_id, state, external_id, etag, conflicts, dirty_at, synced_at, account, snapshot \
               FROM person_sources",
            [],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(person_id, None);
    assert_eq!(state, "ignored");
    assert_eq!(external.as_deref(), Some("people/c777"));
    assert_eq!((etag, conflicts, dirty), (None, None, None));
    assert!(synced.is_some());
    assert_eq!(acct, "me@example.com");
    let snap: Value = serde_json::from_str(&snapshot).unwrap();
    assert_eq!(snap.as_object().unwrap().len(), 2, "just the two id keys");
    assert_eq!(snap["source_id"], "src777");
    assert_eq!(snap["pm_uids"], json!([doomed_ident, folded]));
}

#[test]
fn a_purged_vcard_person_leaves_no_source() {
    let (_d, conn) = store();
    let id = person(&conn, "Card", "Holder");
    add_source(
        &conn,
        id,
        "vcard",
        Some("card-1"),
        "linked",
        Some(r#"{"fn":"Card Holder"}"#),
    );
    add_source(
        &conn,
        id,
        "vcard",
        None,
        "linked",
        Some(r#"{"fn":"Card Holder"}"#),
    );
    trash_people(&conn, &[id], &[]).unwrap();
    assert_eq!(delete_people_now(&conn, &[id]).unwrap(), 1);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM person_sources"), 0);
    assert!(where_found(&conn, "holder").is_empty());
}

#[test]
fn a_purged_pending_create_person_is_not_recreated() {
    let (_d, conn) = store();
    let id = person(&conn, "Not", "Yet");
    let ident = text(&conn, "SELECT uid FROM people");
    add_source(&conn, id, "google", None, "pending_create", None);
    trash_people(&conn, &[id], &[]).unwrap();
    age_in_trash(&conn, id, 40);
    assert_eq!(purge_on_open(&conn).unwrap(), 1);
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM person_sources WHERE state = 'pending_create'"
        ),
        0
    );
    let (person_id, state, snapshot): (Option<i64>, String, String) = conn
        .query_row(
            "SELECT person_id, state, snapshot FROM person_sources",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((person_id, state.as_str()), (None, "ignored"));
    let snap: Value = serde_json::from_str(&snapshot).unwrap();
    assert_eq!(snap, json!({"pm_uids": [ident], "source_id": null}));
}

#[test]
fn a_purge_keeps_a_pending_delete_so_it_is_still_sent() {
    let (_d, conn) = store();
    let id = person(&conn, "Send", "Delete");
    let source = add_source(&conn, id, "google", Some("people/c1"), "linked", Some("{}"));
    trash_people(&conn, &[id], &[source]).unwrap();
    age_in_trash(&conn, id, 45);
    purge_on_open(&conn).unwrap();
    assert_eq!(
        text(&conn, "SELECT state FROM person_sources"),
        "pending_delete"
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM person_sources WHERE person_id IS NULL"
        ),
        1
    );
}

#[test]
fn stripping_a_tombstone_twice_keeps_its_source_id() {
    let (_d, conn) = store();
    let id = person(&conn, "Twice", "Stripped");
    let first_ident = text(&conn, "SELECT uid FROM people");
    add_source(
        &conn,
        id,
        "google",
        Some("people/c9"),
        "linked",
        Some(&json!({"metadata": {"sources": [{"type": "CONTACT", "id": "src9"}]}}).to_string()),
    );
    let tx = conn.unchecked_transaction().unwrap();
    retire(&tx, &[id]).unwrap();
    // A merge folds another uid in between (or a crash retries): the second pass unions.
    let folded = "e".repeat(32);
    tx.execute(
        "UPDATE people SET merged_uids = ?2 WHERE id = ?1",
        params![id, json!([folded]).to_string()],
    )
    .unwrap();
    retire(&tx, &[id]).unwrap();
    retire(&tx, &[id]).unwrap();
    tx.commit().unwrap();
    let snap: Value =
        serde_json::from_str(&text(&conn, "SELECT snapshot FROM person_sources")).unwrap();
    assert_eq!(snap["source_id"], "src9");
    assert_eq!(snap["pm_uids"], json!([first_ident, folded]));
    // The pure function agrees, and a retire outside a transaction is refused.
    let again = tombstone_snapshot(Some(&snap.to_string()), std::slice::from_ref(&folded));
    assert_eq!(serde_json::from_str::<Value>(&again).unwrap(), snap);
    assert!(retire(&conn, &[id]).is_err());
}

#[test]
fn a_restore_cancels_an_unsent_pending_delete() {
    let (_d, conn) = store();
    let id = person(&conn, "Change", "Mind");
    let other = person(&conn, "Other", "One");
    let linked = add_source(&conn, id, "google", Some("people/c1"), "linked", Some("{}"));
    let unlinked = add_source(&conn, id, "google", None, "pending_create", None);
    let foreign = add_source(
        &conn,
        other,
        "google",
        Some("people/c2"),
        "linked",
        Some("{}"),
    );
    // A link that is not theirs refuses the whole move.
    assert!(trash_people(&conn, &[id], &[foreign]).is_err());
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM people WHERE trashed_at IS NOT NULL"
        ),
        0
    );
    trash_people(&conn, &[id], &[linked, unlinked]).unwrap();
    let state = |sid: i64| {
        text(
            &conn,
            &format!("SELECT state FROM person_sources WHERE id = {sid}"),
        )
    };
    assert_eq!(state(linked), "pending_delete");
    assert_eq!(
        state(unlinked),
        "pending_create",
        "nothing to delete at Google for a link never sent"
    );
    restore_people(&conn, &[id]).unwrap();
    assert_eq!(state(linked), "linked");
    assert_eq!(state(unlinked), "pending_create");
    // A delete Google already confirmed left a tombstone; a restore does not revive it.
    conn.execute(
        "UPDATE person_sources SET state = 'ignored' WHERE id = ?1",
        params![linked],
    )
    .unwrap();
    trash_people(&conn, &[id], &[]).unwrap();
    restore_people(&conn, &[id]).unwrap();
    assert_eq!(state(linked), "ignored");
}

#[test]
fn deleting_now_works_in_one_transaction_and_ignores_unknown_ids() {
    let (_d, conn) = store();
    let a = person(&conn, "A", "One");
    let b = person(&conn, "B", "Two");
    trash_people(&conn, &[a], &[]).unwrap();
    assert_eq!(delete_people_now(&conn, &[a, a, b, 31_337]).unwrap(), 1);
    assert!(get_person(&conn, a).is_err());
    assert!(
        get_person(&conn, b).is_ok(),
        "a person not in Trash is never deleted now"
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM sqlite_temp_master WHERE name LIKE 'people_doomed%'"
        ),
        2
    );
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM people_doomed"),
        0,
        "scratch rows never linger"
    );
    assert!(
        delete_people_in(&conn, &[b]).is_err(),
        "the _in form needs a transaction"
    );
}

#[test]
fn a_store_open_purges_the_expired_trash() {
    use crate::sidecar::{SidecarManager, SidecarPaths};
    let (dir, conn) = store();
    let old = person(&conn, "Old", "Entry");
    let recent = person(&conn, "Recent", "Entry");
    trash_people(&conn, &[old, recent], &[]).unwrap();
    age_in_trash(&conn, old, 31);
    age_in_trash(&conn, recent, 29);
    let sidecar = SidecarManager::new(SidecarPaths {
        source_dir: dir.path().join("sidecar"),
        venv_dir: dir.path().join("venv"),
    });
    let state = crate::AppState::new(Some(conn), None, sidecar, None, None);
    state.after_store_open();
    let conn = state.conn().unwrap();
    assert!(
        get_person(&conn, old).is_err(),
        "31 days in Trash: purged on open"
    );
    assert!(get_person(&conn, recent).is_ok(), "29 days: kept");

    // A locked vault has no store to purge, and opening nothing never panics.
    let sidecar = SidecarManager::new(SidecarPaths {
        source_dir: dir.path().join("sidecar2"),
        venv_dir: dir.path().join("venv2"),
    });
    let locked = crate::AppState::new(None, None, sidecar, None, None);
    locked.after_store_open();
    assert!(locked.conn().is_err());
}

// --- projects -------------------------------------------------------------------------------------------------

#[test]
fn linking_a_case_variant_uses_the_existing_project() {
    let (_d, conn) = store();
    crate::projects::ensure_named(&conn, "Atlas").unwrap();
    let id = person(&conn, "Pat", "Planner");
    assert_eq!(link_person_project(&conn, id, "atlas").unwrap(), "Atlas");
    assert_eq!(link_person_project(&conn, id, " ATLAS ").unwrap(), "Atlas");
    assert_eq!(get_person(&conn, id).unwrap().projects, ["Atlas"]);
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM projects WHERE lower(name) = 'atlas'"
        ),
        1
    );
    unlink_person_project(&conn, id, "Atlas").unwrap();
    assert!(get_person(&conn, id).unwrap().projects.is_empty());
    unlink_person_project(&conn, id, "Atlas").unwrap();
}

#[test]
fn linking_an_unknown_project_creates_its_row() {
    let (_d, conn) = store();
    let id = person(&conn, "Pat", "Planner");
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM projects WHERE name = 'Brand New'"
        ),
        0
    );
    assert_eq!(
        link_person_project(&conn, id, "  Brand New ").unwrap(),
        "Brand New"
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM projects WHERE name = 'Brand New'"
        ),
        1
    );
    assert_eq!(get_person(&conn, id).unwrap().projects, ["Brand New"]);
    assert!(link_person_project(&conn, id, "   ").is_err());
}

// --- labels, stars, lists ---------------------------------------------------------------------------------------

#[test]
fn labels_are_unique_by_folded_name_and_deleting_one_keeps_the_people() {
    let (_d, conn) = store();
    let a = person(&conn, "A", "One");
    let b = person(&conn, "B", "Two");
    let family = create_label(&conn, " Famille ").unwrap();
    assert!(create_label(&conn, "FAMILLE").is_err());
    assert!(
        create_label(&conn, "Fámille").is_err(),
        "accents fold, like Google's duplicate check"
    );
    assert_eq!(ensure_label(&conn, "famille").unwrap(), family);
    let work = ensure_label(&conn, "Work").unwrap();
    assert!(rename_label(&conn, work, "famille").is_err());
    rename_label(&conn, work, "WORK").unwrap();
    assert_eq!(label_people(&conn, &[a, b, a], family).unwrap(), 2);
    assert_eq!(
        label_people(&conn, &[a], family).unwrap(),
        0,
        "already carried"
    );
    assert_eq!(
        names(&list_people(&conn, ListFilter::Label(family), None, ListSort::First).unwrap()),
        ["A One", "B Two"]
    );
    assert_eq!(unlabel_people(&conn, &[b], family).unwrap(), 1);
    assert_eq!(
        names(&list_people(&conn, ListFilter::Unlabelled, None, ListSort::First).unwrap()),
        ["B Two"]
    );
    // A label filter and a search word combine: the id binds before the words.
    assert_eq!(
        names(
            &list_people(
                &conn,
                ListFilter::Label(family),
                Some("a one"),
                ListSort::First
            )
            .unwrap()
        ),
        ["A One"]
    );
    assert!(list_people(
        &conn,
        ListFilter::Label(family),
        Some("two"),
        ListSort::First
    )
    .unwrap()
    .is_empty());
    // Renaming a label is a change to the people who carry it.
    let stamp = |id: i64| {
        text(
            &conn,
            &format!("SELECT updated_at FROM people WHERE id = {id}"),
        )
    };
    let before = stamp(a);
    rename_label(&conn, family, "Famille proche").unwrap();
    assert_ne!(stamp(a), before);
    assert_eq!(delete_label(&conn, family).unwrap(), 1);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM people"), 2);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM person_labels"), 0);
    assert!(
        label_people(&conn, &[a], family).is_err(),
        "the label is gone"
    );
}

#[test]
fn the_list_sorts_by_first_name_last_name_and_recent_meeting() {
    let (_d, conn) = store();
    let zed = person(&conn, "Zed", "Adams");
    let amy = person(&conn, "Amy", "Zhou");
    let mia = person(&conn, "Mia", "Moore");
    let single = create_person(
        &conn,
        &PersonDraft {
            display_name: "Cher".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let by = |sort| names(&list_people(&conn, ListFilter::All, None, sort).unwrap());
    assert_eq!(
        by(ListSort::First),
        ["Amy Zhou", "Cher", "Mia Moore", "Zed Adams"]
    );
    // No family name falls back to the display name.
    assert_eq!(
        by(ListSort::Last),
        ["Zed Adams", "Cher", "Mia Moore", "Amy Zhou"]
    );
    add_meeting(&conn, "2026-09-01", "", &[mia]);
    add_meeting(&conn, "2026-10-01", "", &[zed, mia]);
    // Only a `met` entry counts, and a call does not.
    conn.execute(
        "INSERT INTO interactions(kind, on_date) VALUES ('call', '2026-11-20')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO interaction_people(interaction_id, person_id) VALUES (last_insert_rowid(), ?1)",
        params![amy],
    )
    .unwrap();
    assert_eq!(
        by(ListSort::RecentlyMet),
        ["Mia Moore", "Zed Adams", "Amy Zhou", "Cher"]
    );
    let rows = list_people(&conn, ListFilter::All, None, ListSort::RecentlyMet).unwrap();
    assert_eq!(rows[0].last_met.as_deref(), Some("2026-10-01"));
    assert_eq!(rows[2].last_met, None);
    assert_eq!(
        get_person(&conn, mia).unwrap().last_met.as_deref(),
        Some("2026-10-01")
    );
    assert!(single > 0 && amy > 0);
}

#[test]
fn a_row_carries_its_secondary_line_labels_and_google_state() {
    let (_d, conn) = store();
    let mut d = draft("Row", "Reader");
    d.details = vec![
        detail("phone", "555 0111"),
        detail("work", "Example Works"),
        detail("email", "second@example.com"),
        detail("email", "first@example.com"),
    ];
    let id = create_person(&conn, &d).unwrap();
    let lab = create_label(&conn, "Pals").unwrap();
    label_people(&conn, &[id], lab).unwrap();
    add_source(&conn, id, "google", Some("people/c5"), "local_only", None);
    let row = &list_people(&conn, ListFilter::All, None, ListSort::First).unwrap()[0];
    assert_eq!(
        row.primary.as_deref(),
        Some("second@example.com"),
        "position 0 is the primary"
    );
    assert_eq!(row.company.as_deref(), Some("Example Works"));
    assert_eq!(row.labels.len(), 1);
    assert_eq!(row.google_state.as_deref(), Some("local_only"));
}

#[test]
fn a_query_of_three_digits_also_matches_phone_numbers() {
    let (_d, conn) = store();
    let mut d = draft("Dial", "Tone");
    d.details = vec![detail("phone", "+1 (555) 010-0123")];
    create_person(&conn, &d).unwrap();
    create_person(&conn, &draft("Other", "Person")).unwrap();
    let found = |q: &str| names(&search_people(&conn, q, 10).unwrap());
    assert_eq!(found("0100"), ["Dial Tone"]);
    assert_eq!(found("555-010"), ["Dial Tone"]);
    assert_eq!(found("dial 0123"), ["Dial Tone"]);
    // Typed with spaces, a number's groups can be shorter than three digits.
    assert_eq!(found("+1 555 010"), ["Dial Tone"]);
    assert_eq!(found("+1 555 010 0123"), ["Dial Tone"]);
    assert_eq!(found("5 55 01"), ["Dial Tone"]);
    assert!(found("+44 555 010").is_empty());
    assert!(
        found("1 2").is_empty(),
        "fewer than three digits in all never reach phones"
    );
    assert!(
        found("12").is_empty(),
        "two digits never reach the phone numbers"
    );
    assert!(found("0199").is_empty());
}

#[test]
fn a_phone_search_combines_with_a_label_filter() {
    let (_d, conn) = store();
    let mut d = draft("Dial", "Tone");
    d.details = vec![detail("phone", "+1 555 010 0123")];
    let a = create_person(&conn, &d).unwrap();
    let mut e = draft("Ring", "Back");
    e.details = vec![detail("phone", "+1 555 010 0199")];
    let b = create_person(&conn, &e).unwrap();
    let l = create_label(&conn, "Phones").unwrap();
    label_people(&conn, &[a], l).unwrap();
    let found = |q: &str| {
        names(&list_people(&conn, ListFilter::Label(l), Some(q), ListSort::First).unwrap())
    };
    assert_eq!(found("0123"), ["Dial Tone"]);
    assert_eq!(
        found("+1 555 010"),
        ["Dial Tone"],
        "the unlabelled one is out"
    );
    assert!(found("0199").is_empty());
    assert!(get_person(&conn, b).is_ok());
}

#[test]
fn label_trash_star_and_link_writes_join_a_callers_transaction() {
    let (_d, conn) = store();
    assert!(
        ensure_label_in(&conn, "Imported").is_err(),
        "the _in forms need a transaction"
    );
    let gone = person(&conn, "Back", "Again");
    trash_people(&conn, &[gone], &[]).unwrap();
    let tx = conn.unchecked_transaction().unwrap();
    let fresh = create_person_in(&tx, &draft("Fresh", "Import")).unwrap();
    let label = ensure_label_in(&tx, "Imported").unwrap();
    assert_eq!(label_people_in(&tx, &[fresh, gone], label).unwrap(), 2);
    assert_eq!(restore_people_in(&tx, &[gone]).unwrap(), 1);
    assert_eq!(star_people_in(&tx, &[fresh], true).unwrap(), 1);
    link_person_project_in(&tx, fresh, "Import Project").unwrap();
    // Dropping the transaction takes all of it back.
    drop(tx);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM people"), 1);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM contact_labels"), 0);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM person_projects"), 0);
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM people WHERE trashed_at IS NOT NULL"
        ),
        1
    );
}

#[test]
fn a_search_matches_nicknames_emails_and_companies() {
    let (_d, conn) = store();
    let mut d = draft("Sam", "Reed");
    d.details = vec![
        detail("nickname", "Sammy"),
        detail("email", "sam.reed@example.com"),
        detail("work", "Example Works"),
        detail("former_name", "Samantha Brook"),
        detail("web", "https://example.com/hidden-word"),
    ];
    create_person(&conn, &d).unwrap();
    for q in [
        "sammy",
        "sam.reed@",
        "works",
        "brook",
        "example works sammy",
    ] {
        assert_eq!(search_people(&conn, q, 10).unwrap().len(), 1, "{q}");
    }
    assert!(search_people(&conn, "hidden-word", 10).unwrap().is_empty());
    assert_eq!(
        search_people(&conn, "", 10).unwrap().len(),
        1,
        "a blank query lists"
    );
}

// --- saving -------------------------------------------------------------------------------------------------------

#[test]
fn a_save_refuses_when_the_person_changed_elsewhere() {
    let (_d, conn) = store();
    let id = person(&conn, "Sam", "Reed");
    let opened = get_person(&conn, id).unwrap();
    // Somebody else (a sync, another window) saves first.
    update_person(&conn, id, &draft("Samuel", "Reed"), &opened.updated_at).unwrap();
    let err = update_person(&conn, id, &draft("Sammy", "Reed"), &opened.updated_at).unwrap_err();
    assert_eq!(err.to_string(), CHANGED_ELSEWHERE);
    assert_eq!(
        get_person(&conn, id).unwrap().given_name,
        "Samuel",
        "nothing was written"
    );
}

#[test]
fn every_change_moves_updated_at_strictly_forward() {
    let (_d, conn) = store();
    let id = person(&conn, "Sam", "Reed");
    let label = create_label(&conn, "Pals").unwrap();
    let stamp = |c: &Connection| text(c, "SELECT updated_at FROM people");
    let mut seen = vec![stamp(&conn)];
    star_people(&conn, &[id], true).unwrap();
    seen.push(stamp(&conn));
    label_people(&conn, &[id], label).unwrap();
    seen.push(stamp(&conn));
    link_person_project(&conn, id, "Atlas").unwrap();
    seen.push(stamp(&conn));
    unlink_person_project(&conn, id, "Atlas").unwrap();
    seen.push(stamp(&conn));
    unlabel_people(&conn, &[id], label).unwrap();
    seen.push(stamp(&conn));
    trash_people(&conn, &[id], &[]).unwrap();
    seen.push(stamp(&conn));
    restore_people(&conn, &[id]).unwrap();
    seen.push(stamp(&conn));
    for pair in seen.windows(2) {
        assert!(
            pair[0] < pair[1],
            "every write moved it, even inside one millisecond"
        );
    }
    // A change that changes nothing does not.
    let before = stamp(&conn);
    star_people(&conn, &[id], true).unwrap();
    assert_eq!(stamp(&conn), before);
    // A timeline entry alone does not move it.
    add_meeting(&conn, "2026-11-01", "", &[id]);
    assert_eq!(stamp(&conn), before);
}

#[test]
fn a_save_keeps_a_details_row_and_its_sync_key_when_its_value_is_edited() {
    let (_d, conn) = store();
    let mut d = draft("Sam", "Reed");
    d.details = vec![
        detail("email", "old@example.com"),
        detail("phone", "555 0100"),
    ];
    let id = create_person(&conn, &d).unwrap();
    let p = get_person(&conn, id).unwrap();
    let email = p.details.iter().find(|x| x.kind == "email").unwrap().id;
    conn.execute(
        "UPDATE person_details SET src_key = 'old@example.com' WHERE id = ?1",
        params![email],
    )
    .unwrap();
    let mut edit = draft("Sam", "Reed");
    edit.details = vec![
        DetailDraft {
            id: Some(email),
            ..detail("email", "new@example.com")
        },
        detail("email", "extra@example.com"),
    ];
    update_person(&conn, id, &edit, &p.updated_at).unwrap();
    let after = get_person(&conn, id).unwrap();
    assert_eq!(
        after.details.len(),
        2,
        "the phone was removed, the extra email added"
    );
    let kept = after.details.iter().find(|x| x.id == email).unwrap();
    assert_eq!(kept.value, "new@example.com");
    assert_eq!(kept.position, 0);
    assert_eq!(
        text(
            &conn,
            &format!("SELECT src_key FROM person_details WHERE id = {email}")
        ),
        "old@example.com",
        "the sync's bookkeeping survives an edit"
    );
    assert_eq!(
        text(
            &conn,
            &format!("SELECT value_key FROM person_details WHERE id = {email}")
        ),
        "new@example.com"
    );
    // A detail id from another person is refused.
    let other = person(&conn, "Other", "Person");
    let o = get_person(&conn, other).unwrap();
    let mut steal = draft("Other", "Person");
    steal.details = vec![DetailDraft {
        id: Some(email),
        ..detail("email", "x@example.com")
    }];
    assert!(update_person(&conn, other, &steal, &o.updated_at).is_err());
}

#[test]
fn identifiers_resolve_through_folded_idents_and_sources() {
    let (_d, conn) = store();
    let id = person(&conn, "Find", "Me");
    let ident = text(&conn, "SELECT uid FROM people");
    let folded = "c".repeat(32);
    conn.execute(
        "UPDATE people SET merged_uids = ?1",
        params![json!([folded]).to_string()],
    )
    .unwrap();
    assert_eq!(
        sources::person_for_ident(&conn, &ident).unwrap(),
        Some((id, false))
    );
    assert_eq!(
        sources::person_for_ident(&conn, &folded).unwrap(),
        Some((id, false))
    );
    assert_eq!(sources::person_for_ident(&conn, "nope").unwrap(), None);
    trash_people(&conn, &[id], &[]).unwrap();
    assert_eq!(
        sources::person_for_ident(&conn, &folded).unwrap(),
        Some((id, true))
    );
    add_source(&conn, id, "google", Some("people/c3"), "linked", None);
    assert_eq!(
        sources::person_for_source(&conn, "google", "me@example.com", "people/c3").unwrap(),
        Some(id)
    );
    assert_eq!(
        sources::person_for_source(&conn, "google", "me@example.com", "people/c4").unwrap(),
        None
    );
}

#[test]
fn the_consts_are_the_spec_vocabulary() {
    assert_eq!(DETAIL_KINDS.len(), 12);
    assert!(DETAIL_KINDS.contains(&"former_name") && DETAIL_KINDS.contains(&"location"));
    assert_eq!(PROVIDERS, ["vcard", "google"]);
    assert_eq!(SOURCE_STATES.len(), 6);
    assert_eq!(INTERACTION_KINDS, ["met", "call", "message", "note"]);
    assert_eq!(PROMPT_ANSWERS, ["logged", "skipped", "series_skipped"]);
}
