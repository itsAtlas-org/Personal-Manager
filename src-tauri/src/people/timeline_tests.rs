// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tests for the timeline. Every person is invented: example.com addresses, made-up ASCII names.

use super::tests::{count, draft, person, store, text};
use super::timeline::{
    add_interaction, delete_interaction, first_page, last_met, list_timeline, update_interaction,
    EventLink, InteractionDraft,
};
use super::*;

fn entry(kind: &str, on_date: &str, people: &[i64]) -> InteractionDraft {
    InteractionDraft {
        kind: kind.into(),
        on_date: on_date.into(),
        people: people.to_vec(),
        ..Default::default()
    }
}

fn notes(page: &timeline::TimelinePage) -> Vec<String> {
    page.entries.iter().map(|e| e.note.clone()).collect()
}

#[test]
fn last_met_follows_the_newest_met_entry() {
    let (_d, conn) = store();
    let mia = person(&conn, "Mia", "Moore");
    assert_eq!(last_met(&conn, mia).unwrap(), None);

    add_interaction(&conn, &entry("met", "2026-10-01", &[mia])).unwrap();
    assert_eq!(last_met(&conn, mia).unwrap().as_deref(), Some("2026-10-01"));

    // A call, a message and a note are not meetings, however new.
    for kind in ["call", "message", "note"] {
        add_interaction(&conn, &entry(kind, "2026-11-20", &[mia])).unwrap();
    }
    assert_eq!(last_met(&conn, mia).unwrap().as_deref(), Some("2026-10-01"));

    // An older meeting added later doesn't move it back; a newer one moves it forward.
    add_interaction(&conn, &entry("met", "2026-09-01", &[mia])).unwrap();
    assert_eq!(last_met(&conn, mia).unwrap().as_deref(), Some("2026-10-01"));
    let newest = add_interaction(&conn, &entry("met", "2026-11-12", &[mia])).unwrap();
    assert_eq!(last_met(&conn, mia).unwrap().as_deref(), Some("2026-11-12"));
    assert_eq!(
        get_person(&conn, mia).unwrap().last_met.as_deref(),
        Some("2026-11-12")
    );

    // Deleting it falls back to the one before, because it is a query, never a stored value.
    delete_interaction(&conn, newest).unwrap();
    assert_eq!(last_met(&conn, mia).unwrap().as_deref(), Some("2026-10-01"));
}

#[test]
fn the_timeline_pages_newest_first_without_gaps_or_repeats() {
    let (_d, conn) = store();
    let kay = person(&conn, "Kay", "Kept");
    // Out of order on purpose, with two on one day and a time on one of them.
    for (i, day) in [
        "2026-03-01",
        "2026-05-09",
        "2026-05-09",
        "2026-01-15",
        "2026-05-09",
        "2026-07-04",
        "2026-02-02",
    ]
    .iter()
    .enumerate()
    {
        let mut d = entry("met", day, &[kay]);
        d.note = format!("n{i}");
        if i == 2 {
            d.at = Some("2026-05-09T15:00:00Z".into());
        }
        add_interaction(&conn, &d).unwrap();
    }
    let mut seen = Vec::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let page = list_timeline(&conn, kay, cursor.as_ref(), 3).unwrap();
        pages += 1;
        seen.extend(notes(&page));
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(pages, 3);
    // 2026-07-04 first; then the three on 05-09 (the timed one first, then newest id first);
    // then March, February and January.
    assert_eq!(seen, ["n5", "n2", "n4", "n1", "n0", "n6", "n3"]);

    // The record carries the first page; a limit past the cap is clamped.
    let first = first_page(&conn, kay).unwrap();
    assert_eq!(first.entries.len(), 7);
    assert!(first.next.is_none());
    assert_eq!(
        list_timeline(&conn, kay, None, 100_000)
            .unwrap()
            .entries
            .len(),
        7
    );
    assert_eq!(get_person(&conn, kay).unwrap().timeline.entries.len(), 7);
}

#[test]
fn interactions_never_move_people_updated_at() {
    let (_d, conn) = store();
    let a = person(&conn, "Ann", "Able");
    let b = person(&conn, "Bob", "Baker");
    let stamp = |id: i64| {
        text(
            &conn,
            &format!("SELECT updated_at FROM people WHERE id = {id}"),
        )
    };
    let (before_a, before_b) = (stamp(a), stamp(b));

    let id = add_interaction(&conn, &entry("met", "2026-11-01", &[a, b])).unwrap();
    let mut edit = entry("call", "2026-11-02", &[a]);
    edit.note = "changed".into();
    update_interaction(&conn, id, &edit).unwrap();
    assert_eq!((stamp(a), stamp(b)), (before_a.clone(), before_b.clone()));
    delete_interaction(&conn, id).unwrap();
    assert_eq!((stamp(a), stamp(b)), (before_a, before_b));

    // So an editor opened before the entry was logged still saves.
    let p = get_person(&conn, a).unwrap();
    add_interaction(&conn, &entry("note", "2026-11-03", &[a])).unwrap();
    update_person(&conn, a, &draft("Ann", "Able"), &p.updated_at).unwrap();
}

#[test]
fn an_entry_names_several_people_and_others_are_listed_without_the_viewer() {
    let (_d, conn) = store();
    let a = person(&conn, "Ann", "Able");
    let b = person(&conn, "Bob", "Baker");
    let c = person(&conn, "Cy", "Cole");
    add_interaction(&conn, &entry("met", "2026-11-01", &[a, b, c, a])).unwrap();
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM interaction_people"), 3);
    let page = list_timeline(&conn, a, None, 10).unwrap();
    let others: Vec<&str> = page.entries[0]
        .others
        .iter()
        .map(|p| p.display_name.as_str())
        .collect();
    assert_eq!(others, ["Bob Baker", "Cy Cole"]);
}

#[test]
fn a_trashed_person_is_left_off_other_peoples_entries_and_comes_back_with_a_restore() {
    let (_d, conn) = store();
    let a = person(&conn, "Ann", "Able");
    let gone = person(&conn, "Tess", "Trashed");
    let id = add_interaction(&conn, &entry("met", "2026-11-01", &[a, gone])).unwrap();
    trash_people(&conn, &[gone], &[]).unwrap();

    let page = list_timeline(&conn, a, None, 10).unwrap();
    assert!(
        page.entries[0].others.is_empty(),
        "the shared entry names the others only"
    );

    // An edit made without seeing them keeps their link.
    let mut edit = entry("met", "2026-11-01", &[a]);
    edit.note = "added a note".into();
    update_interaction(&conn, id, &edit).unwrap();
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM interaction_people"), 2);

    restore_people(&conn, &[gone]).unwrap();
    let page = list_timeline(&conn, a, None, 10).unwrap();
    assert_eq!(page.entries[0].others.len(), 1);

    // Nothing can be logged against someone in Trash.
    trash_people(&conn, &[gone], &[]).unwrap();
    assert!(add_interaction(&conn, &entry("note", "2026-11-02", &[gone])).is_err());
}

#[test]
fn an_edit_replaces_the_visible_people_and_the_text() {
    let (_d, conn) = store();
    let a = person(&conn, "Ann", "Able");
    let b = person(&conn, "Bob", "Baker");
    let c = person(&conn, "Cy", "Cole");
    let id = add_interaction(&conn, &entry("met", "2026-11-01", &[a, b])).unwrap();
    let mut edit = entry("message", "2026-11-05", &[b, c]);
    edit.note = "  line one\r\nline two  ".into();
    edit.at = Some("2026-11-05T09:30:00+01:00".into());
    update_interaction(&conn, id, &edit).unwrap();

    assert_eq!(list_timeline(&conn, a, None, 5).unwrap().entries.len(), 0);
    let page = list_timeline(&conn, c, None, 5).unwrap();
    let e = &page.entries[0];
    assert_eq!(e.kind, "message");
    assert_eq!(e.on_date, "2026-11-05");
    assert_eq!(e.note, "line one\nline two");
    assert_eq!(e.at.as_deref(), Some("2026-11-05T08:30:00Z"));
    assert!(update_interaction(&conn, id + 99, &edit).is_err());
}

#[test]
fn an_edit_leaves_the_meeting_link_alone() {
    let (_d, conn) = store();
    let a = person(&conn, "Ann", "Able");
    let mut d = entry("met", "2026-11-01", &[a]);
    d.event = Some(EventLink {
        ical: "evt-1@example.com".into(),
        start: "2026-11-01T10:00:00Z".into(),
        title: "Planning".into(),
    });
    let id = add_interaction(&conn, &d).unwrap();
    // The edit names no event; the link stays.
    update_interaction(&conn, id, &entry("met", "2026-11-01", &[a])).unwrap();
    let e = &list_timeline(&conn, a, None, 5).unwrap().entries[0];
    assert_eq!(e.event_ical.as_deref(), Some("evt-1@example.com"));
    assert_eq!(e.event_start.as_deref(), Some("2026-11-01T10:00:00Z"));
    assert_eq!(e.event_title.as_deref(), Some("Planning"));
}

#[test]
fn a_bad_entry_is_refused_in_fixed_words() {
    let (_d, conn) = store();
    let a = person(&conn, "Ann", "Able");
    let sentinel = "SENTINEL-NOTE-ZQX";
    let mut bad = vec![
        entry("coffee", "2026-11-01", &[a]),
        entry("met", "2026-02-30", &[a]),
        entry("met", "2026-11", &[a]),
        entry("met", "2026-11-01", &[]),
        entry("met", "2026-11-01", &[424_242]),
    ];
    let mut late = entry("met", "2026-11-01", &[a]);
    late.at = Some(format!("{sentinel} o'clock"));
    bad.push(late);
    let mut big = entry("met", "2026-11-01", &[a]);
    big.note = format!("{sentinel}{}", "x".repeat(NOTES_MAX));
    bad.push(big);
    let mut half = entry("met", "2026-11-01", &[a]);
    half.event = Some(EventLink {
        ical: sentinel.into(),
        start: String::new(),
        title: String::new(),
    });
    bad.push(half);
    for d in &bad {
        let err = add_interaction(&conn, d).expect_err("refused").to_string();
        assert!(!err.contains(sentinel), "an error quoted a value: {err}");
    }
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM interactions"), 0);
}

#[test]
fn a_long_event_title_is_clipped_not_refused() {
    let (_d, conn) = store();
    let a = person(&conn, "Ann", "Able");
    let mut d = entry("met", "2026-11-01", &[a]);
    d.event = Some(EventLink {
        ical: "evt-2@example.com".into(),
        start: "2026-11-01".into(),
        title: "T".repeat(VALUE_MAX + 500),
    });
    add_interaction(&conn, &d).unwrap();
    let title = text(&conn, "SELECT event_title FROM interactions");
    assert_eq!(title.chars().count(), VALUE_MAX);
}

#[test]
fn deleting_an_entry_removes_it_for_everyone_and_a_missing_one_says_so() {
    let (_d, conn) = store();
    let a = person(&conn, "Ann", "Able");
    let b = person(&conn, "Bob", "Baker");
    let id = add_interaction(&conn, &entry("met", "2026-11-01", &[a, b])).unwrap();
    delete_interaction(&conn, id).unwrap();
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM interaction_people"), 0);
    assert!(delete_interaction(&conn, id).is_err());
    assert!(list_timeline(&conn, 424_242, None, 5).is_err());
}
