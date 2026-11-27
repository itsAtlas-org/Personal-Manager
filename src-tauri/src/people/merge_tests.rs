// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tests for the duplicate finder, merging and its Undo. Every person is invented: example.com
//! addresses, 555-01xx and Ofcom drama-range numbers, made-up ASCII names.

use std::time::{Duration, Instant};

use rusqlite::{params, Connection};
use serde_json::{json, Value};

use super::dupes::{self, DupesReport, Reason, GROUP_MAX, PAIRS_MAX};
use super::merge::{
    self, MergeChoices, NotesChoice, Side, UndoShelf, UndoStep, NO_LONGER, UNDO_HOLD,
};
use super::tests::{
    add_meeting, add_source, count, detail, draft, leapling, person, store, stored_leap, text,
};
use super::*;

fn with(given: &str, family: &str, details: Vec<DetailDraft>) -> PersonDraft {
    PersonDraft {
        details,
        ..draft(given, family)
    }
}

fn job(org: &str, title: &str) -> DetailDraft {
    DetailDraft {
        parts: Some(json!({ "title": title })),
        ..detail("work", org)
    }
}

fn labelled(kind: &str, value: &str, label: &str) -> DetailDraft {
    DetailDraft {
        label: label.into(),
        ..detail(kind, value)
    }
}

fn report(conn: &Connection) -> DupesReport {
    dupes::scan(&dupes::load(conn).unwrap())
}

fn merge_ab(conn: &Connection, into: i64, from: i64) -> UndoStep {
    merge::merge_people(conn, into, from, &MergeChoices::default())
        .unwrap()
        .1
}

fn as_json(conn: &Connection, id: i64) -> Value {
    serde_json::to_value(get_person(conn, id).unwrap()).unwrap()
}

fn pairs_on_disk(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT low_uid || '|' || high_uid FROM person_distinct_pairs ORDER BY 1")
        .unwrap();
    let rows = stmt.query_map([], |r| r.get(0)).unwrap();
    rows.map(|r| r.unwrap()).collect()
}

fn sources_on_disk(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            "SELECT id || '|' || IFNULL(person_id, '-') || '|' || IFNULL(etag, '-') || '|' || \
                    IFNULL(snapshot, '-') || '|' || IFNULL(dirty_at, '-') \
               FROM person_sources ORDER BY id",
        )
        .unwrap();
    let rows = stmt.query_map([], |r| r.get(0)).unwrap();
    rows.map(|r| r.unwrap()).collect()
}

/// The reasons the finder gives for the one pair `a` and `b` make, in a store of their own.
fn reasons_for(a: PersonDraft, b: PersonDraft) -> Vec<Reason> {
    let (_d, conn) = store();
    person_from(&conn, &a);
    person_from(&conn, &b);
    let r = report(&conn);
    assert!(r.pairs.len() <= 1);
    r.pairs
        .first()
        .map(|p| p.reasons.clone())
        .unwrap_or_default()
}

fn person_from(conn: &Connection, d: &PersonDraft) -> i64 {
    create_person(conn, d).unwrap()
}

// --- finding them ---------------------------------------------------------------------------------

#[test]
fn the_finder_gives_each_reason_apart() {
    let email = |e: &str| vec![detail("email", e)];
    let phone = |p: &str| vec![detail("phone", p)];
    // Same email, different names.
    assert_eq!(
        reasons_for(
            with("Ann", "Lee", email("ann@example.com")),
            with("Bea", "Cho", email("ANN@example.com"))
        ),
        [Reason::Email]
    );
    // A national number against its international form.
    assert_eq!(
        reasons_for(
            with("Ann", "Lee", phone("+44 20 7946 0000")),
            with("Bea", "Cho", phone("020 7946 0000"))
        ),
        [Reason::Phone]
    );
    // The same name is one finding, never also "similar".
    assert_eq!(
        reasons_for(draft("Sam", "Reed"), draft("Sam", "Reed")),
        [Reason::Name]
    );
    // Sam and Samuel.
    assert_eq!(
        reasons_for(draft("Sam", "Reed"), draft("Samuel", "Reed")),
        [Reason::Similar]
    );
    // The same given names in another order.
    assert_eq!(
        reasons_for(draft("Mary Ann", "Lee"), draft("Ann Mary", "Lee")),
        [Reason::Similar]
    );
    // An accented twin.
    assert_eq!(
        reasons_for(draft("\u{c9}lodie", "Martin"), draft("Elodie", "Martin")),
        [Reason::Name]
    );
    // Several reasons at once, strongest first.
    assert_eq!(
        reasons_for(
            with("Sam", "Reed", email("sam@example.com")),
            with("Sam", "Reed", email("sam@example.com"))
        ),
        [Reason::Email, Reason::Name]
    );
    // Nothing alike.
    assert!(reasons_for(draft("Ann", "Lee"), draft("Bea", "Cho")).is_empty());
    // Initials are not enough for "similar".
    assert!(reasons_for(draft("S", "Reed"), draft("Samuel", "Reed")).is_empty());
}

#[test]
fn a_distinct_pair_and_a_trashed_person_are_skipped() {
    let (_d, conn) = store();
    let a = person(&conn, "Sam", "Reed");
    let b = person(&conn, "Sam", "Reed");
    assert_eq!(report(&conn).pairs.len(), 1);
    dupes::mark_distinct(&conn, b, a).unwrap();
    let r = report(&conn);
    assert!(r.pairs.is_empty());
    assert_eq!(r.hidden, 1, "the pair is hidden, and said so");
    assert!(r.complete());
    assert_eq!(dupes::restore_distinct(&conn).unwrap(), 1);
    assert_eq!(report(&conn).pairs.len(), 1);
    trash_people(&conn, &[b], &[]).unwrap();
    assert!(report(&conn).pairs.is_empty());
}

#[test]
fn a_group_over_the_limit_is_skipped_and_never_reported_clean() {
    let (_d, conn) = store();
    for i in 0..=GROUP_MAX {
        create_person(
            &conn,
            &with(
                &format!("Person{i}"),
                "Crowd",
                vec![detail("email", "shared@example.com")],
            ),
        )
        .unwrap();
    }
    let r = report(&conn);
    assert!(r.pairs.iter().all(|p| !p.reasons.contains(&Reason::Email)));
    assert!(r
        .skipped
        .iter()
        .any(|s| s.reason == Reason::Email && s.people == GROUP_MAX + 1));
    assert!(!r.complete(), "a skipped group is not a clean result");
}

#[test]
fn a_scan_stops_at_the_pair_cap_and_says_so() {
    let (_d, conn) = store();
    // PAIRS_MAX + 100 two-person email groups: many pairs, no group anywhere near GROUP_MAX, and
    // names that never match each other.
    let n = PAIRS_MAX + 100;
    for i in 0..n {
        for side in ["a", "b"] {
            create_person(
                &conn,
                &with(
                    &format!("Given{i}{side}"),
                    &format!("Family{i}{side}"),
                    vec![detail("email", &format!("pair{i}@example.com"))],
                ),
            )
            .unwrap();
        }
    }
    let r = report(&conn);
    assert_eq!(r.pairs.len(), PAIRS_MAX);
    assert!(r.cut_short);
    assert!(r.skipped.is_empty());
    assert!(!r.complete(), "a cut-short scan is never reported clean");
}

#[test]
fn a_card_keeps_the_email_and_phone_that_flagged_the_pair() {
    let (_d, conn) = store();
    // More than the card's cap of "address" rows, which sort before "email" and "phone" by kind.
    let mut rows: Vec<DetailDraft> = (0..25)
        .map(|i| detail("address", &format!("{i} Example Road")))
        .collect();
    rows.push(detail("email", "crowded@example.com"));
    rows.push(labelled("phone", "+44 20 7946 0002", ""));
    let a = create_person(&conn, &with("Ida", "Crowded", rows)).unwrap();
    let b = create_person(
        &conn,
        &with(
            "Ida",
            "Crowded",
            vec![detail("email", "crowded@example.com")],
        ),
    )
    .unwrap();
    let r = report(&conn);
    let card = r.cards.iter().find(|c| c.id == a).unwrap();
    assert!(card.details.iter().any(|d| d.kind == "email"));
    assert!(card.details.iter().any(|d| d.kind == "phone"));
    assert!(r.cards.iter().any(|c| c.id == b));
}

#[test]
fn a_pair_marked_not_the_same_person_is_never_merged_in_a_batch() {
    let (_d, conn) = store();
    let a = person(&conn, "Sam", "Reed");
    let b = person(&conn, "Sam", "Reed");
    dupes::mark_distinct(&conn, a, b).unwrap();
    // A webview (or a snapshot read just before the click) that still lists the pair.
    let (done, steps) = merge::merge_identical_pairs(&conn, &[(a, b)]).unwrap();
    assert_eq!(done.merged, 0);
    assert!(steps.is_empty());
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM people"), 2);
}

#[test]
fn a_chain_in_a_batch_never_merges_a_pair_the_user_said_is_not_the_same() {
    let (_d, conn) = store();
    let a = person(&conn, "Sam", "Reed");
    let b = person(&conn, "Sam", "Reed");
    let c = person(&conn, "Sam", "Reed");
    // The user said b and c are different people. a~b and a~c are both on offer: folding b into a
    // rewrites the (b, c) answer to (a, c), so a must not then absorb c.
    dupes::mark_distinct(&conn, b, c).unwrap();
    let (done, steps) = merge::merge_identical_pairs(&conn, &[(a, b), (a, c)]).unwrap();
    assert_eq!(done.merged, 1);
    assert_eq!(steps.len(), 1);
    assert!(get_person(&conn, b).is_err());
    assert!(get_person(&conn, c).is_ok(), "the answer is still honoured");
    assert_eq!(pairs_on_disk(&conn).len(), 1, "and still on record");
}

#[test]
fn nothing_differs_pairs_merge_in_one_action() {
    let (_d, conn) = store();
    let a = create_person(
        &conn,
        &with(
            "Sam",
            "Reed",
            vec![
                detail("email", "sam@example.com"),
                labelled("phone", "+44 20 7946 0001", ""),
            ],
        ),
    )
    .unwrap();
    // The same email (a different case would be a different spelling, so it is exact here), a
    // label the first lacks, and a link the first lacks: all fills, nothing lost.
    let b = create_person(
        &conn,
        &with(
            "Sam",
            "Reed",
            vec![
                detail("email", "sam@example.com"),
                labelled("phone", "+44 20 7946 0001", "mobile"),
                detail("web", "https://example.com/sam"),
            ],
        ),
    )
    .unwrap();
    // A pair that differs (a birthday each), and one that only looks alike by name.
    let c = create_person(
        &conn,
        &PersonDraft {
            born_on: Some("1990-01-01".into()),
            ..draft("Tia", "Wong")
        },
    )
    .unwrap();
    let d = create_person(
        &conn,
        &PersonDraft {
            born_on: Some("1991-02-02".into()),
            ..draft("Tia", "Wong")
        },
    )
    .unwrap();
    let e = person(&conn, "Una", "Park");
    let f = person(&conn, "Unas", "Park");

    let r = report(&conn);
    let flagged = |x: i64, y: i64| r.pairs.iter().find(|p| (p.a, p.b) == (x, y)).unwrap();
    assert!(flagged(a, b).nothing_differs);
    assert!(!flagged(c, d).nothing_differs);
    assert!(flagged(c, d).differs.born_on);
    assert!(
        !flagged(e, f).nothing_differs,
        "a similar name never joins the group"
    );

    let (done, steps) = merge::merge_identical_pairs(&conn, &[(a, b), (c, d), (e, f)]).unwrap();
    assert_eq!(done.merged, 1, "only the pair that loses nothing");
    assert_eq!(steps.len(), 1);
    assert!(get_person(&conn, b).is_err());
    let kept = get_person(&conn, a).unwrap();
    assert_eq!(kept.details.iter().filter(|d| d.kind == "email").count(), 1);
    assert_eq!(kept.details.iter().filter(|d| d.kind == "web").count(), 1);
    let phone = kept.details.iter().find(|d| d.kind == "phone").unwrap();
    assert_eq!(
        phone.label, "mobile",
        "the dropped copy's label fills the blank"
    );
    assert!(
        get_person(&conn, d).is_ok(),
        "the pair that differs is left alone"
    );
}

#[test]
fn identical_pairs_follow_a_chain_of_copies_to_one_person() {
    let (_d, conn) = store();
    let a = person(&conn, "Sam", "Reed");
    let b = person(&conn, "Sam", "Reed");
    let c = person(&conn, "Sam", "Reed");
    let (done, steps) = merge::merge_identical_pairs(&conn, &[(a, b), (b, c), (a, c)]).unwrap();
    assert_eq!(done.merged, 2);
    assert_eq!(steps.len(), 2);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM people"), 1);
    // One Undo takes both back, newest first.
    assert_eq!(merge::undo_steps(&conn, &steps).unwrap(), 2);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM people"), 3);
}

#[test]
fn identical_pairs_are_judged_again_by_the_backend() {
    // A webview that lists a pair that differs, or someone in Trash, gets nothing merged.
    let (_d, conn) = store();
    let a = person(&conn, "Sam", "Reed");
    let b = create_person(
        &conn,
        &PersonDraft {
            notes: "met at the fair".into(),
            ..draft("Sam", "Reed")
        },
    )
    .unwrap();
    let c = create_person(
        &conn,
        &PersonDraft {
            notes: "a different note".into(),
            ..draft("Sam", "Reed")
        },
    )
    .unwrap();
    let t = person(&conn, "Sam", "Reed");
    trash_people(&conn, &[t], &[]).unwrap();
    let (done, _) = merge::merge_identical_pairs(&conn, &[(b, c), (a, t)]).unwrap();
    assert_eq!(done.merged, 0);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM people"), 4);
    // A blank note on one side only fills, so it is fine.
    let (done, _) = merge::merge_identical_pairs(&conn, &[(a, b)]).unwrap();
    assert_eq!(done.merged, 1);
    assert_eq!(get_person(&conn, a).unwrap().notes, "met at the fair");
}

// --- merging --------------------------------------------------------------------------------------------

#[test]
fn a_merge_keeps_every_source_link() {
    let (_d, conn) = store();
    let a = person(&conn, "Sam", "Reed");
    let b = person(&conn, "Sam", "Reed");
    let one = add_source(
        &conn,
        a,
        "vcard",
        Some("card-1"),
        "linked",
        Some("{\"x\":1}"),
    );
    let two = add_source(
        &conn,
        b,
        "google",
        Some("people/c1"),
        "linked",
        Some("{\"y\":2}"),
    );
    let before = sources_on_disk(&conn);
    let step = merge_ab(&conn, a, b);
    let kept = get_person(&conn, a).unwrap();
    let mut ids: Vec<i64> = kept.sources.iter().map(|s| s.id).collect();
    ids.sort_unstable();
    assert_eq!(ids, [one, two], "the survivor keeps every provider link");
    // Nothing about the link changed but whose it is: etag, snapshot and dirty_at are as they were.
    let after = sources_on_disk(&conn);
    assert_eq!(after.len(), before.len());
    assert!(after[1].starts_with(&format!("{two}|{a}|")));
    assert_eq!(
        after[1]
            .split_once('|')
            .unwrap()
            .1
            .split_once('|')
            .unwrap()
            .1,
        before[1]
            .split_once('|')
            .unwrap()
            .1
            .split_once('|')
            .unwrap()
            .1
    );
    // Undo moves it back by row id and touches nothing else.
    merge::undo_steps(&conn, &[step]).unwrap();
    assert_eq!(sources_on_disk(&conn), before);
}

#[test]
fn a_merge_keeps_two_jobs_at_one_company() {
    let (_d, conn) = store();
    let a = create_person(
        &conn,
        &with("Sam", "Reed", vec![job("Example Ltd", "Engineer")]),
    )
    .unwrap();
    let b = create_person(
        &conn,
        &with("Sam", "Reed", vec![job("Example Ltd", "Manager")]),
    )
    .unwrap();
    merge_ab(&conn, a, b);
    let kept = get_person(&conn, a).unwrap();
    let titles: Vec<&str> = kept
        .details
        .iter()
        .filter(|d| d.kind == "work")
        .map(|d| d.parts.as_ref().unwrap()["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Engineer", "Manager"]);

    // The same job twice is one.
    let c = create_person(
        &conn,
        &with("Tia", "Wong", vec![job("Example Ltd", "Chair")]),
    )
    .unwrap();
    let d = create_person(
        &conn,
        &with("Tia", "Wong", vec![job("Example Ltd", "Chair")]),
    )
    .unwrap();
    merge_ab(&conn, c, d);
    assert_eq!(
        get_person(&conn, c)
            .unwrap()
            .details
            .iter()
            .filter(|d| d.kind == "work")
            .count(),
        1
    );
}

#[test]
fn a_merge_keeps_the_label_of_the_dropped_duplicate() {
    let (_d, conn) = store();
    let a = create_person(
        &conn,
        &with(
            "Sam",
            "Reed",
            vec![labelled("email", "sam@example.com", "")],
        ),
    )
    .unwrap();
    let b = create_person(
        &conn,
        &with(
            "Sam",
            "Reed",
            vec![labelled("email", "sam@example.com", "work")],
        ),
    )
    .unwrap();
    merge_ab(&conn, a, b);
    let emails: Vec<_> = get_person(&conn, a)
        .unwrap()
        .details
        .into_iter()
        .filter(|d| d.kind == "email")
        .collect();
    assert_eq!(emails.len(), 1);
    assert_eq!(emails[0].label, "work");

    // Two different non-blank labels keep both rows.
    let c = create_person(
        &conn,
        &with(
            "Tia",
            "Wong",
            vec![labelled("email", "tia@example.com", "home")],
        ),
    )
    .unwrap();
    let d = create_person(
        &conn,
        &with(
            "Tia",
            "Wong",
            vec![labelled("email", "tia@example.com", "work")],
        ),
    )
    .unwrap();
    merge_ab(&conn, c, d);
    assert_eq!(
        get_person(&conn, c)
            .unwrap()
            .details
            .iter()
            .filter(|d| d.kind == "email")
            .count(),
        2
    );
}

#[test]
fn a_detail_with_no_key_is_never_deduplicated() {
    let (_d, conn) = store();
    let a = create_person(&conn, &with("Sam", "Reed", vec![detail("custom", "")])).unwrap_or(0);
    // An empty value is refused or dropped by the editor rules; a stored empty key is forced here.
    let a = if a == 0 {
        person(&conn, "Sam", "Reed")
    } else {
        a
    };
    let b = person(&conn, "Sam", "Reed");
    for id in [a, b] {
        conn.execute(
            "INSERT INTO person_details(person_id, kind, value, value_key) VALUES (?1, 'custom', 'x', '')",
            params![id],
        )
        .unwrap();
    }
    let n = count(
        &conn,
        "SELECT COUNT(*) FROM person_details WHERE kind = 'custom'",
    );
    merge_ab(&conn, a, b);
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM person_details WHERE kind = 'custom'"
        ),
        n
    );
}

#[test]
fn a_merge_asks_only_where_a_field_differs_and_the_other_is_dropped() {
    let (_d, conn) = store();
    let mk = |born: &str, note: &str, met: &str| PersonDraft {
        born_on: Some(born.into()),
        notes: note.into(),
        met_where: met.into(),
        ..draft("Sam", "Reed")
    };
    let a = create_person(&conn, &mk("1990-01-01", "first", "the fair")).unwrap();
    let b = create_person(&conn, &mk("1991-02-02", "second", "the pub")).unwrap();
    let p = merge::preview(&conn, a, b).unwrap();
    assert!(p.differs.born_on && p.differs.notes && p.differs.met && !p.differs.name);
    // No answer: nothing is dropped unasked, and nothing is written.
    assert!(merge::merge_people(&conn, a, b, &MergeChoices::default()).is_err());
    assert!(get_person(&conn, b).is_ok());
    let choices = MergeChoices {
        name: None,
        born_on: Some(Side::Folded),
        met: Some(Side::Survivor),
        notes: Some(NotesChoice::Both),
    };
    merge::merge_people(&conn, a, b, &choices).unwrap();
    let kept = get_person(&conn, a).unwrap();
    assert_eq!(kept.born_on.as_deref(), Some("1991-02-02"));
    assert_eq!(kept.met_where, "the fair");
    assert_eq!(kept.notes, "first\n\nsecond");
    assert_eq!(kept.folded_idents.len(), 1, "the folded uid is recorded");
}

#[test]
fn a_merge_keeps_the_leap_day_of_the_kept_birthday() {
    let kept = |conn: &Connection, id: i64| {
        let p = get_person(conn, id).unwrap();
        (p.born_on, p.leap_day)
    };
    let pick = |side: Side| MergeChoices {
        born_on: Some(side),
        ..Default::default()
    };

    // Both answered, differently: the answer is part of the birthday's choice, so it is asked.
    let (_d, conn) = store();
    let a = leapling(&conn, "--02-29", Some(LeapDay::Feb28));
    let b = leapling(&conn, "--02-29", Some(LeapDay::Mar1));
    assert!(merge::preview(&conn, a, b).unwrap().differs.born_on);
    assert!(merge::merge_people(&conn, a, b, &MergeChoices::default()).is_err());
    assert!(get_person(&conn, b).is_ok(), "nothing is dropped unasked");
    merge::merge_people(&conn, a, b, &pick(Side::Folded)).unwrap();
    assert_eq!(
        kept(&conn, a),
        (Some("--02-29".into()), Some(LeapDay::Mar1))
    );
    assert_eq!(stored_leap(&conn, a).as_deref(), Some("mar1"));

    // The survivor's answer, when the survivor's is picked.
    let (_d, conn) = store();
    let a = leapling(&conn, "--02-29", Some(LeapDay::Feb28));
    let b = leapling(&conn, "--02-29", Some(LeapDay::Mar1));
    merge::merge_people(&conn, a, b, &pick(Side::Survivor)).unwrap();
    assert_eq!(kept(&conn, a).1, Some(LeapDay::Feb28));

    // Only one answered, same birthday: nothing differs, and the answer is kept either way round.
    for (a_answer, b_answer) in [(Some(LeapDay::Mar1), None), (None, Some(LeapDay::Mar1))] {
        let (_d, conn) = store();
        let a = leapling(&conn, "--02-29", a_answer);
        let b = leapling(&conn, "--02-29", b_answer);
        assert!(!merge::preview(&conn, a, b).unwrap().differs.born_on);
        merge_ab(&conn, a, b);
        assert_eq!(kept(&conn, a).1, Some(LeapDay::Mar1));
    }

    // One side has no birthday: the other's birthday and answer fill it.
    let (_d, conn) = store();
    let a = person(&conn, "Leap", "Day");
    let b = leapling(&conn, "2000-02-29", Some(LeapDay::Mar1));
    merge_ab(&conn, a, b);
    assert_eq!(
        kept(&conn, a),
        (Some("2000-02-29".into()), Some(LeapDay::Mar1))
    );

    // Neither answered: still not answered.
    let (_d, conn) = store();
    let a = leapling(&conn, "--02-29", None);
    let b = leapling(&conn, "--02-29", None);
    merge_ab(&conn, a, b);
    assert_eq!(kept(&conn, a), (Some("--02-29".into()), None));

    // Different birthdays: the answer goes with the one kept, and a kept date that isn't 29
    // February leaves no answer behind.
    let (_d, conn) = store();
    let a = leapling(&conn, "2000-02-29", Some(LeapDay::Mar1));
    let mut other = draft("Leap", "Day");
    other.born_on = Some("--03-04".into());
    let b = create_person(&conn, &other).unwrap();
    merge::merge_people(&conn, a, b, &pick(Side::Folded)).unwrap();
    assert_eq!(kept(&conn, a), (Some("--03-04".into()), None));
    let (_d, conn) = store();
    let a = leapling(&conn, "2000-02-29", Some(LeapDay::Mar1));
    let b = create_person(&conn, &other).unwrap();
    merge::merge_people(&conn, a, b, &pick(Side::Survivor)).unwrap();
    assert_eq!(
        kept(&conn, a),
        (Some("2000-02-29".into()), Some(LeapDay::Mar1))
    );

    // Two 29 February birthdays that differ only in the year: the kept one has no answer, so the
    // other's is not lost.
    let (_d, conn) = store();
    let a = leapling(&conn, "2000-02-29", None);
    let b = leapling(&conn, "--02-29", Some(LeapDay::Feb28));
    merge::merge_people(&conn, a, b, &pick(Side::Survivor)).unwrap();
    assert_eq!(
        kept(&conn, a),
        (Some("2000-02-29".into()), Some(LeapDay::Feb28))
    );

    // Undo puts the answers back as they were.
    let (_d, conn) = store();
    let a = leapling(&conn, "--02-29", Some(LeapDay::Feb28));
    let b = leapling(&conn, "--02-29", Some(LeapDay::Mar1));
    let step = merge::merge_people(&conn, a, b, &pick(Side::Folded))
        .unwrap()
        .1;
    assert_eq!(merge::undo_steps(&conn, &[step]).unwrap(), 1);
    assert_eq!(stored_leap(&conn, a).as_deref(), Some("feb28"));
    assert_eq!(stored_leap(&conn, b).as_deref(), Some("mar1"));
}

#[test]
fn a_merge_refuses_someone_in_trash_or_the_same_person_twice() {
    let (_d, conn) = store();
    let a = person(&conn, "Sam", "Reed");
    let b = person(&conn, "Sam", "Reed");
    assert!(merge::merge_people(&conn, a, a, &MergeChoices::default()).is_err());
    trash_people(&conn, &[b], &[]).unwrap();
    assert!(merge::preview(&conn, a, b).is_err());
    assert!(merge::merge_people(&conn, a, b, &MergeChoices::default()).is_err());
    assert!(merge::merge_people(&conn, a, 9_999, &MergeChoices::default()).is_err());
}

#[test]
fn merge_preview_counts_what_will_move() {
    let (_d, conn) = store();
    let a = create_person(
        &conn,
        &with(
            "Sam",
            "Reed",
            vec![
                detail("email", "sam@example.com"),
                detail("phone", "+44 20 7946 0001"),
            ],
        ),
    )
    .unwrap();
    let b = create_person(
        &conn,
        &with(
            "Sam",
            "Reed",
            vec![
                detail("email", "sam@example.com"),
                detail("web", "https://example.com/sam"),
            ],
        ),
    )
    .unwrap();
    let friends = create_label(&conn, "Friends").unwrap();
    let team = create_label(&conn, "Team").unwrap();
    label_people(&conn, &[a, b], friends).unwrap();
    label_people(&conn, &[b], team).unwrap();
    let shared = add_meeting(&conn, "2026-10-01", "lunch", &[a, b]);
    let _ = shared;
    add_meeting(&conn, "2026-10-02", "walk", &[b]);
    link_person_project(&conn, a, "Garden").unwrap();
    link_person_project(&conn, b, "Garden").unwrap();
    link_person_project(&conn, b, "Allotment").unwrap();
    add_source(&conn, a, "vcard", Some("c1"), "linked", None);
    add_source(&conn, b, "vcard", Some("c2"), "linked", None);

    let p = merge::preview(&conn, a, b).unwrap();
    assert_eq!(p.details, 3, "the email both have is counted once");
    assert_eq!(p.details_dropped, 1);
    assert_eq!(p.labels, 2);
    assert_eq!(p.timeline, 2, "a meeting both were at counts once");
    assert_eq!(p.projects, 2);
    assert_eq!(p.sources, 2);
    assert!(!p.differs.any());
    assert!(p.lossless);
    // A preview writes nothing.
    assert!(get_person(&conn, b).is_ok());
}

#[test]
fn a_merge_of_two_people_marked_distinct_does_not_self_pair() {
    let (_d, conn) = store();
    let a = person(&conn, "Sam", "Reed");
    let b = person(&conn, "Sam", "Reed");
    dupes::mark_distinct(&conn, a, b).unwrap();
    let before = pairs_on_disk(&conn);
    let step = merge_ab(&conn, a, b);
    assert!(
        pairs_on_disk(&conn).is_empty(),
        "no pair names one person twice"
    );
    merge::undo_steps(&conn, &[step]).unwrap();
    assert_eq!(pairs_on_disk(&conn), before);
}

#[test]
fn a_merge_moves_a_distinct_pair_onto_the_survivor() {
    let (_d, conn) = store();
    let a = person(&conn, "Sam", "Reed");
    let b = person(&conn, "Sam", "Reed");
    let x = person(&conn, "Xia", "Chen");
    dupes::mark_distinct(&conn, b, x).unwrap();
    merge_ab(&conn, a, b);
    let ident_a = text(&conn, &format!("SELECT uid FROM people WHERE id = {a}"));
    let ident_x = text(&conn, &format!("SELECT uid FROM people WHERE id = {x}"));
    let (low, high) = if ident_a < ident_x {
        (ident_a, ident_x)
    } else {
        (ident_x, ident_a)
    };
    assert_eq!(pairs_on_disk(&conn), [format!("{low}|{high}")]);
}

// --- Undo -----------------------------------------------------------------------------------------------

/// Two people with something of every kind, a pair naming each, and a meeting both were at.
fn rich_pair(conn: &Connection) -> (i64, i64, i64) {
    let a = create_person(
        conn,
        &PersonDraft {
            notes: "kept".into(),
            ..with(
                "Sam",
                "Reed",
                vec![
                    detail("email", "sam@example.com"),
                    labelled("phone", "+44 20 7946 0001", "home"),
                ],
            )
        },
    )
    .unwrap();
    let b = create_person(
        conn,
        &with(
            "Sam",
            "Reed",
            vec![
                detail("email", "sam@example.com"),
                labelled("phone", "+44 20 7946 0001", "work"),
                job("Example Ltd", "Chair"),
            ],
        ),
    )
    .unwrap();
    let x = person(conn, "Xia", "Chen");
    let label = create_label(conn, "Friends").unwrap();
    label_people(conn, &[b], label).unwrap();
    add_meeting(conn, "2026-10-01", "lunch", &[a, b]);
    add_meeting(conn, "2026-10-02", "walk", &[b]);
    link_person_project(conn, b, "Garden").unwrap();
    add_source(
        conn,
        b,
        "google",
        Some("people/c9"),
        "linked",
        Some("{\"s\":1}"),
    );
    dupes::mark_distinct(conn, b, x).unwrap();
    dupes::mark_distinct(conn, a, x).unwrap();
    (a, b, x)
}

#[test]
fn a_merge_undo_restores_the_folded_uid_and_rows() {
    let (_d, conn) = store();
    let (a, b, _x) = rich_pair(&conn);
    let (a_before, b_before) = (as_json(&conn, a), as_json(&conn, b));
    let (pairs_before, sources_before) = (pairs_on_disk(&conn), sources_on_disk(&conn));
    let b_ident = b_before["uid"].as_str().unwrap().to_string();
    let b_folded_before = get_person(&conn, a).unwrap().folded_idents.len();

    let step = merge_ab(&conn, a, b);
    assert!(get_person(&conn, b).is_err());
    assert_eq!(
        get_person(&conn, a).unwrap().folded_idents,
        std::slice::from_ref(&b_ident)
    );

    assert_eq!(merge::undo_steps(&conn, &[step]).unwrap(), 1);
    assert_eq!(
        as_json(&conn, a),
        a_before,
        "the survivor is as the merge found them"
    );
    assert_eq!(
        as_json(&conn, b),
        b_before,
        "the folded person is back with their uid and rows"
    );
    assert_eq!(
        get_person(&conn, a).unwrap().folded_idents.len(),
        b_folded_before
    );
    assert_eq!(
        pairs_on_disk(&conn),
        pairs_before,
        "including the pair naming the folded person"
    );
    assert_eq!(sources_on_disk(&conn), sources_before);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM interaction_people"), 3);
}

#[test]
fn a_merge_undo_after_an_edit_is_refused() {
    let (_d, conn) = store();
    let (a, b, _) = rich_pair(&conn);
    let step = merge_ab(&conn, a, b);
    let p = get_person(&conn, a).unwrap();
    let saved = update_person(
        &conn,
        a,
        &PersonDraft {
            notes: "edited".into(),
            given_name: "Sam".into(),
            family_name: "Reed".into(),
            ..Default::default()
        },
        &p.updated_at,
    );
    assert!(saved.is_ok());
    let e = merge::undo_steps(&conn, &[step]).unwrap_err().to_string();
    assert!(e.contains(NO_LONGER));
    assert!(
        get_person(&conn, b).is_err(),
        "a refused Undo writes nothing"
    );
    assert_eq!(get_person(&conn, a).unwrap().notes, "edited");
}

#[test]
fn a_label_added_after_a_merge_refuses_the_undo() {
    let (_d, conn) = store();
    let (a, b, _) = rich_pair(&conn);
    let step = merge_ab(&conn, a, b);
    let label = create_label(&conn, "Later").unwrap();
    label_people(&conn, &[a], label).unwrap();
    assert!(merge::undo_steps(&conn, &[step]).is_err());
    assert!(get_person(&conn, b).is_err());
}

#[test]
fn a_meeting_logged_after_a_merge_does_not_refuse_the_undo_and_stays() {
    let (_d, conn) = store();
    let (a, b, _) = rich_pair(&conn);
    let step = merge_ab(&conn, a, b);
    let later = add_meeting(&conn, "2026-11-01", "coffee", &[a]);
    merge::undo_steps(&conn, &[step]).unwrap();
    assert_eq!(
        count(
            &conn,
            &format!("SELECT COUNT(*) FROM interaction_people WHERE interaction_id = {later} AND person_id = {a}")
        ),
        1
    );
    assert_eq!(
        count(
            &conn,
            &format!("SELECT COUNT(*) FROM interaction_people WHERE person_id = {b}")
        ),
        2
    );
}

#[test]
fn undo_is_refused_when_the_folded_uid_is_taken() {
    let (_d, conn) = store();
    let (a, b, _) = rich_pair(&conn);
    let b_ident = get_person(&conn, b).unwrap().ident;
    let step = merge_ab(&conn, a, b);
    conn.execute(
        "INSERT INTO people(uid, display_name) VALUES (?1, 'Someone')",
        params![b_ident],
    )
    .unwrap();
    assert!(merge::undo_steps(&conn, &[step]).is_err());
}

#[test]
fn a_store_switch_between_merge_and_undo_refuses_the_undo() {
    let (_d, conn) = store();
    let (a, b, _) = rich_pair(&conn);
    let t0 = Instant::now();
    let mut shelf = UndoShelf::default();
    let token = shelf.put(1, vec![merge_ab(&conn, a, b)], t0).unwrap();
    // Another store is open now (a vault switch bumped the epoch).
    let e = shelf.take(&token, 2, t0).unwrap_err().to_string();
    assert!(e.contains(NO_LONGER));
    // Nothing is left to try again with.
    assert!(shelf.take(&token, 1, t0).is_err());
}

#[test]
fn the_shelf_hands_a_merge_back_once_and_only_while_fresh() {
    let (_d, conn) = store();
    let (a, b, _) = rich_pair(&conn);
    let t0 = Instant::now();
    let mut shelf = UndoShelf::default();
    let token = shelf.put(7, vec![merge_ab(&conn, a, b)], t0).unwrap();
    assert!(shelf.take("not-a-token", 7, t0).is_err());
    let steps = shelf.take(&token, 7, t0 + Duration::from_secs(29)).unwrap();
    assert_eq!(merge::undo_steps(&conn, &steps).unwrap(), 1);
    assert!(shelf.take(&token, 7, t0).is_err(), "used once");

    let (a2, b2, _) = (a, b, 0);
    let token = shelf.put(7, vec![merge_ab(&conn, a2, b2)], t0).unwrap();
    assert!(
        shelf
            .take(&token, 7, t0 + UNDO_HOLD + Duration::from_secs(1))
            .is_err(),
        "too late"
    );
    assert!(shelf.put(7, Vec::new(), t0).is_none(), "nothing to hold");
}

#[test]
fn a_merge_put_back_after_a_failed_undo_can_be_tried_again() {
    let (_d, conn) = store();
    let (a, b, _) = rich_pair(&conn);
    let t0 = Instant::now();
    let mut shelf = UndoShelf::default();
    let token = shelf.put(3, vec![merge_ab(&conn, a, b)], t0).unwrap();
    let steps = shelf.take(&token, 3, t0).unwrap();
    // The database failed (not a refusal), so the caller hands the steps back under their token.
    shelf.put_back(token.clone(), 3, steps, t0);
    let steps = shelf.take(&token, 3, t0).unwrap();
    assert_eq!(merge::undo_steps(&conn, &steps).unwrap(), 1);
}

#[test]
fn clearing_the_shelf_forgets_every_merge() {
    let (_d, conn) = store();
    let (a, b, _) = rich_pair(&conn);
    let t0 = Instant::now();
    let mut shelf = UndoShelf::default();
    let token = shelf.put(3, vec![merge_ab(&conn, a, b)], t0).unwrap();
    shelf.clear();
    assert!(shelf.take(&token, 3, t0).is_err());
}

#[test]
fn a_merge_in_a_store_never_names_a_value_in_its_errors() {
    let (_d, conn) = store();
    let a = create_person(
        &conn,
        &PersonDraft {
            notes: "sentinel-note-alpha".into(),
            ..draft("Zed", "Sentinelson")
        },
    )
    .unwrap();
    let b = create_person(
        &conn,
        &PersonDraft {
            notes: "sentinel-note-beta".into(),
            ..draft("Zed", "Sentinelson")
        },
    )
    .unwrap();
    let e = merge::merge_people(&conn, a, b, &MergeChoices::default())
        .unwrap_err()
        .to_string();
    for needle in ["Sentinelson", "sentinel-note"] {
        assert!(!e.contains(needle));
    }
    let (done, step) = merge::merge_people(
        &conn,
        a,
        b,
        &MergeChoices {
            notes: Some(NotesChoice::Folded),
            ..Default::default()
        },
    )
    .unwrap();
    for shown in [format!("{done:?}"), format!("{step:?}")] {
        assert!(!shown.contains("Sentinelson") && !shown.contains("sentinel-note"));
    }
}
