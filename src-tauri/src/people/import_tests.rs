// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tests for vCard import and export against a real store: the round trip, re-importing PM's own
//! file, matching by id, Trash, the held preview, and what an export may and may not write. Every
//! person is invented: example.com addresses, 555-01xx style numbers, made-up names.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use rusqlite::Connection;
use serde_json::json;

use super::card::Card;
use super::export::{self, ExportScope, TIMELINE_FILE};
use super::import::{
    self, commit_held, commit_in, prepare, preview_counts, read_capped, ImportHold, ImportOptions,
    ImportResult, STALE, TOO_BIG,
};
use super::merge::MergeChoices;
use super::tests::{add_meeting, count, detail, draft, leapling, person, store, stored_leap, text};
use super::vcard::parse_vcf;
use super::*;

fn d(kind: &str, label: &str, value: &str) -> DetailDraft {
    DetailDraft {
        label: label.into(),
        ..detail(kind, value)
    }
}

/// A person with a little of everything a vCard can carry, written to be awkward: separators in
/// values, a custom label that tries to be a parameter, accents, a long multi-byte note.
fn rich() -> PersonDraft {
    PersonDraft {
        given_name: "Ada".into(),
        middle_name: "Q".into(),
        family_name: "Éxample-Ünicode".into(),
        name_prefix: "Dr.".into(),
        name_suffix: "Jr.".into(),
        born_on: Some("1990-04-15".into()),
        met_where: "A conference, in Leeds; with; semicolons".into(),
        met_on: Some("2020-05".into()),
        notes: format!(
            "Line one\nLine two, with comma; and semicolon \\ backslash\n{}",
            "é".repeat(120)
        ),
        details: vec![
            d("email", "work", "ada@example.com"),
            d("email", "x;TYPE=EVIL:y", "ada.two@example.com"),
            d("phone", "mobile", "+44 20 7946 0101"),
            d("phone", "home fax", "+44 20 7946 0102"),
            DetailDraft {
                parts: Some(json!({
                    "street": "1 Example Road",
                    "city": "Leeds",
                    "postal_code": "LS1 4AB",
                    "country": "United Kingdom"
                })),
                ..d(
                    "address",
                    "work",
                    "1 Example Road\nLeeds LS1 4AB\nUnited Kingdom",
                )
            },
            d("address", "home", "5 Plain Road\nLeeds"),
            d("web", "LinkedIn", "https://www.linkedin.com/in/ada-example"),
            d("web", "Website", "https://example.com/ada"),
            d("web", "My blog", "https://example.com/blog"),
            // A street-only address is its text, on export and on the way back.
            DetailDraft {
                parts: Some(json!({"street": "9 Only Street"})),
                ..d("address", "home", "9 Only Street")
            },
            DetailDraft {
                parts: Some(json!({"title": "Engineer", "department": "R&D"})),
                from_date: Some("2020-01".into()),
                is_current: true,
                ..d("work", "", "Example Widgets")
            },
            // A past job: its dates and "not current" must survive a file.
            DetailDraft {
                parts: Some(json!({"title": "Advisor"})),
                from_date: Some("2015-03".into()),
                to_date: Some("2019-12".into()),
                is_current: false,
                ..d("work", "", "Second Co")
            },
            DetailDraft {
                parts: Some(json!({"field": "Maths"})),
                from_date: Some("2008-09".into()),
                to_date: Some("2011-06".into()),
                is_current: false,
                ..d("school", "", "Example University")
            },
            d("former_name", "", "Ada Maiden"),
            d("custom", "Pet; name", "Rex"),
            // The importer's own row for a birthday it couldn't read: it must survive a file too.
            d("custom", "Birthday (as written)", "circa 1800"),
            d("nickname", "", "Addie"),
            d("nickname", "", "Lady A"),
            d("date", "Anniversary", "2010-06-01"),
            d("date", "Graduation", "--06-30"),
            d("date", "", "2001-01-02"),
            d("relation", "Spouse", "Charles Example"),
            DetailDraft {
                from_date: Some("2026-03".into()),
                ..d("location", "", "Lisbon")
            },
        ],
        ..Default::default()
    }
}

fn card_of(conn: &Connection, id: i64) -> Card {
    Card::from_person(&get_person(conn, id).unwrap())
}

fn export_vcf(conn: &Connection, scope: &ExportScope) -> String {
    String::from_utf8(export::build(conn, scope, false).unwrap().vcf).unwrap()
}

fn import_text(conn: &Connection, vcf: &str, options: &ImportOptions) -> ImportResult {
    let parsed = prepare(parse_vcf(vcf.as_bytes()));
    let tx = conn.unchecked_transaction().unwrap();
    let result = commit_in(&tx, &parsed.cards, options).unwrap();
    tx.commit().unwrap();
    result
}

fn plain() -> ImportOptions {
    ImportOptions::default()
}

fn all() -> ExportScope {
    ExportScope::All
}

#[test]
fn export_then_import_is_identity() {
    let (_dir, conn) = store();
    let id = create_person(&conn, &rich()).unwrap();
    let family = create_label(&conn, "Family").unwrap();
    let conf = create_label(&conn, "Conference 2026").unwrap();
    label_people(&conn, &[id], family).unwrap();
    label_people(&conn, &[id], conf).unwrap();
    let original = card_of(&conn, id);

    let vcf = export_vcf(&conn, &all());
    // What was written reads back as exactly the card it was written from.
    let parsed = parse_vcf(vcf.as_bytes());
    assert!(parsed.complete);
    assert_eq!(parsed.cards.len(), 1);
    assert!(
        parsed.cards[0] == original,
        "the file reads back differently"
    );

    // And importing it into another store makes the same person (with an id of its own).
    let (_dir2, other) = store();
    let result = import_text(&other, &vcf, &plain());
    assert_eq!(result.added, 1);
    let mut again = card_of(&other, result.touched[0]);
    let mut expected = original;
    again.ident = None;
    expected.ident = None;
    assert!(again == expected, "the imported person differs");
}

#[test]
fn a_year_less_birthday_is_written_without_a_year_and_reads_back() {
    let (_dir, conn) = store();
    for (name, born) in [("Yearless", "--04-15"), ("Leapling", "--02-29")] {
        let id = create_person(
            &conn,
            &PersonDraft {
                born_on: Some(born.into()),
                ..draft(name, "Example")
            },
        )
        .unwrap();
        let vcf = export_vcf(&conn, &ExportScope::Ids(vec![id]));
        assert!(
            vcf.contains(&format!("BDAY:{born}\r\n")),
            "the birthday was written with a year"
        );
        let parsed = parse_vcf(vcf.as_bytes());
        assert!(parsed.cards[0] == card_of(&conn, id));
        assert_eq!(parsed.cards[0].born_on.as_deref(), Some(born));
    }
}

/// A one-card file, CRLF, as another app (or a hand-edited PM export) might write it.
fn card_file(lines: &[&str]) -> String {
    let mut out = String::from("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Leap Example\r\n");
    for l in lines {
        out.push_str(l);
        out.push_str("\r\n");
    }
    out.push_str("END:VCARD\r\n");
    out
}

#[test]
fn a_vcard_round_trip_keeps_the_leap_day() {
    for (born, answer, word) in [
        ("--02-29", LeapDay::Mar1, "mar1"),
        ("2000-02-29", LeapDay::Feb28, "feb28"),
    ] {
        let (_dir, conn) = store();
        let id = leapling(&conn, born, Some(answer));
        let vcf = export_vcf(&conn, &ExportScope::Ids(vec![id]));
        assert!(
            vcf.contains(&format!("X-PM-LEAP-DAY:{word}\r\n")),
            "the answer was not written"
        );
        let parsed = parse_vcf(vcf.as_bytes());
        assert_eq!(parsed.cards[0].leap_day, Some(answer));
        assert!(parsed.cards[0] == card_of(&conn, id));
        // Into another store: the same birthday and the same answer.
        let (_dir2, other) = store();
        let result = import_text(&other, &vcf, &plain());
        assert_eq!(result.added, 1);
        let p = get_person(&other, result.touched[0]).unwrap();
        assert_eq!(p.born_on.as_deref(), Some(born));
        assert_eq!(p.leap_day, Some(answer));
    }

    // An unanswered person writes nothing, and reads back unanswered.
    let (_dir, conn) = store();
    let id = leapling(&conn, "--02-29", None);
    let vcf = export_vcf(&conn, &ExportScope::Ids(vec![id]));
    assert!(!vcf.contains("X-PM-LEAP-DAY"));
    assert_eq!(parse_vcf(vcf.as_bytes()).cards[0].leap_day, None);
}

#[test]
fn a_leap_day_in_a_file_counts_only_on_a_29_february_birthday() {
    // On a 15 March birthday, on no birthday, or with a word PM doesn't know: ignored.
    for lines in [
        vec!["BDAY:--03-15", "X-PM-LEAP-DAY:mar1"],
        vec!["X-PM-LEAP-DAY:feb28"],
        vec!["BDAY:--02-29", "X-PM-LEAP-DAY:feb29"],
        vec!["BDAY:--02-29", "X-PM-LEAP-DAY:"],
        vec!["BDAY:--02-29", "X-PM-LEAP-DAY:MAR1"],
    ] {
        let file = card_file(&lines);
        let parsed = parse_vcf(file.as_bytes());
        assert_eq!(parsed.cards.len(), 1);
        assert_eq!(parsed.cards[0].leap_day, None, "{lines:?}");
        // And an import stores none.
        let (_dir, conn) = store();
        let result = import_text(&conn, &file, &plain());
        assert_eq!(stored_leap(&conn, result.touched[0]), None, "{lines:?}");
    }
    // The word on a 29 February birthday is read, with surrounding space ignored.
    let file = card_file(&["BDAY:--02-29", "X-PM-LEAP-DAY: mar1 "]);
    assert_eq!(
        parse_vcf(file.as_bytes()).cards[0].leap_day,
        Some(LeapDay::Mar1)
    );
    // The file does not report its own X-PM word as an unknown property.
    assert!(parse_vcf(file.as_bytes()).ignored.is_empty());
}

#[test]
fn a_reimport_fills_an_answer_pm_does_not_hold_and_never_overwrites_one() {
    let (_dir, conn) = store();
    let unanswered = leapling(&conn, "--02-29", None);
    let answered = leapling(&conn, "--02-29", Some(LeapDay::Feb28));
    // Same people by their own ids, as a file exported from this store would carry them.
    for (id, file_word, expect) in [
        (unanswered, "mar1", Some(LeapDay::Mar1)),
        (answered, "mar1", Some(LeapDay::Feb28)),
    ] {
        let ident = get_person(&conn, id).unwrap().ident;
        let uid = format!("UID:{ident}");
        let file = card_file(&["BDAY:--02-29", &format!("X-PM-LEAP-DAY:{file_word}"), &uid]);
        import_text(&conn, &file, &plain());
        assert_eq!(get_person(&conn, id).unwrap().leap_day, expect);
    }
}

#[test]
fn reimporting_pms_own_export_adds_nothing() {
    let (_dir, conn) = store();
    let id = create_person(&conn, &rich()).unwrap();
    let other = create_person(
        &conn,
        &PersonDraft {
            details: vec![
                d("email", "", "boris@example.com"),
                // A link with no label is written as a plain URL, which reads back as "Website".
                d("web", "", "https://example.com/boris"),
                // A blank-label link to a known network reads back as that network's name.
                d("web", "", "https://www.linkedin.com/in/boris-example"),
            ],
            ..draft("Boris", "Example")
        },
    )
    .unwrap();
    let label = create_label(&conn, "Family").unwrap();
    label_people(&conn, &[id, other], label).unwrap();
    let before = (
        count(&conn, "SELECT COUNT(*) FROM people"),
        count(&conn, "SELECT COUNT(*) FROM person_details"),
        count(&conn, "SELECT COUNT(*) FROM person_labels"),
        count(&conn, "SELECT COUNT(*) FROM contact_labels"),
        count(&conn, "SELECT COUNT(*) FROM person_sources"),
    );
    let stamp = text(
        &conn,
        &format!("SELECT updated_at FROM people WHERE id = {id}"),
    );

    let vcf = export_vcf(&conn, &all());
    let result = import_text(&conn, &vcf, &plain());
    assert_eq!((result.added, result.updated, result.unchanged), (0, 0, 2));
    assert!(result.kept.is_empty());
    let after = (
        count(&conn, "SELECT COUNT(*) FROM people"),
        count(&conn, "SELECT COUNT(*) FROM person_details"),
        count(&conn, "SELECT COUNT(*) FROM person_labels"),
        count(&conn, "SELECT COUNT(*) FROM contact_labels"),
        count(&conn, "SELECT COUNT(*) FROM person_sources"),
    );
    assert_eq!(before, after);
    // Nothing was saved, so the editor's "changed elsewhere" check has nothing to trip on.
    assert_eq!(
        text(
            &conn,
            &format!("SELECT updated_at FROM people WHERE id = {id}")
        ),
        stamp
    );
}

#[test]
fn a_repeated_uid_in_one_file_does_not_abort_the_import() {
    let (_dir, conn) = store();
    let vcf = "BEGIN:VCARD\nUID:dup-1\nFN:First Copy\nEMAIL:first@example.com\nEND:VCARD\n\
               BEGIN:VCARD\nUID:dup-1\nFN:Second Copy\nEMAIL:second@example.com\nEND:VCARD\n\
               BEGIN:VCARD\nUID:solo-1\nFN:Third Person\nEND:VCARD\n";
    let parsed = prepare(parse_vcf(vcf.as_bytes()));
    assert_eq!(parsed.repeated, 1);
    let tx = conn.unchecked_transaction().unwrap();
    let result = commit_in(&tx, &parsed.cards, &plain()).unwrap();
    tx.commit().unwrap();
    assert_eq!(result.added, 3);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM people"), 3);
    // The first keeps the id; the repeat has none, so the unique index is never hit.
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM person_sources WHERE provider = 'vcard' AND external_id = 'dup-1'"
        ),
        1
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM person_sources WHERE provider = 'vcard' AND external_id IS NULL"
        ),
        1
    );
    // The same file again matches by id and adds nobody (the repeat, having no id, is new again).
    let again = import_text(&conn, vcf, &plain());
    assert_eq!(again.added, 1);
}

#[test]
fn a_folded_uid_resolves_to_the_survivor() {
    let (_dir, conn) = store();
    let a = create_person(&conn, &draft("Alpha", "Example")).unwrap();
    let b = create_person(&conn, &draft("Alpha", "Example")).unwrap();
    let folded_ident = text(&conn, &format!("SELECT uid FROM people WHERE id = {b}"));
    merge::merge_people(&conn, a, b, &MergeChoices::default()).unwrap();
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM people"), 1);

    let vcf = format!(
        "BEGIN:VCARD\nUID:{folded_ident}\nFN:Alpha Example\nEMAIL:alpha@example.com\nEND:VCARD\n"
    );
    let result = import_text(&conn, &vcf, &plain());
    // The card is the survivor, who gains the email; nobody new.
    assert_eq!((result.added, result.updated), (0, 1));
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM people"), 1);
    assert_eq!(
        count(
            &conn,
            &format!(
                "SELECT COUNT(*) FROM person_details WHERE person_id = {a} AND kind = 'email'"
            )
        ),
        1
    );
}

#[test]
fn a_reimport_by_uid_only_adds_and_lists_what_pm_kept() {
    let (_dir, conn) = store();
    let first = "BEGIN:VCARD\nUID:file-uid-1\nFN:Cleo Example\nBDAY:1990-04-15\nEMAIL:cleo@example.com\nEND:VCARD\n";
    let result = import_text(&conn, first, &plain());
    assert_eq!(result.added, 1);
    let id = result.touched[0];

    // The user corrects the birthday in PM.
    let p = get_person(&conn, id).unwrap();
    let mut edited = draft("Cleo", "Example");
    edited.born_on = Some("1991-01-01".into());
    edited.details = p
        .details
        .iter()
        .map(|x| DetailDraft {
            id: Some(x.id),
            kind: x.kind.clone(),
            label: x.label.clone(),
            value: x.value.clone(),
            ..Default::default()
        })
        .collect();
    update_person(&conn, id, &edited, &p.updated_at).unwrap();

    // The file comes again, with a new email and a new phone and the old birthday.
    let second = "BEGIN:VCARD\nUID:file-uid-1\nFN:Cleo Example\nBDAY:1990-04-15\nEMAIL:cleo@example.com\nEMAIL:cleo.new@example.com\nTEL:+44 20 7946 0103\nEND:VCARD\n";
    let result = import_text(&conn, second, &plain());
    assert_eq!((result.added, result.updated), (0, 1));
    assert_eq!(result.kept.len(), 1);
    assert_eq!(result.kept[0].name, "Cleo Example");
    assert_eq!(result.kept[0].fields, vec!["birthday"]);
    let after = get_person(&conn, id).unwrap();
    assert_eq!(after.born_on.as_deref(), Some("1991-01-01"));
    assert_eq!(
        after.details.iter().filter(|x| x.kind == "email").count(),
        2
    );
    assert_eq!(
        after.details.iter().filter(|x| x.kind == "phone").count(),
        1
    );
    // The source row's copy moved on to the newest card.
    let snapshot = text(
        &conn,
        "SELECT snapshot FROM person_sources WHERE external_id = 'file-uid-1'",
    );
    assert!(snapshot.contains("cleo.new@example.com"));
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM person_sources"), 1);
}

#[test]
fn an_import_match_on_a_trashed_person_restores_them() {
    let (_dir, conn) = store();
    let file = "BEGIN:VCARD\nUID:trash-uid-1\nFN:Dana Example\nEMAIL:dana@example.com\nEND:VCARD\n";
    let id = import_text(&conn, file, &plain()).touched[0];
    trash_people(&conn, &[id], &[]).unwrap();
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM people WHERE trashed_at IS NOT NULL"
        ),
        1
    );

    let result = import_text(&conn, file, &plain());
    assert_eq!(result.restored, vec!["Dana Example"]);
    assert_eq!(result.added, 0);
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM people WHERE trashed_at IS NOT NULL"
        ),
        0
    );

    // An email that only a trashed person has is not a match: a different id makes a new person.
    trash_people(&conn, &[id], &[]).unwrap();
    let other = "BEGIN:VCARD\nUID:someone-else\nFN:Dana Twin\nEMAIL:dana@example.com\nEND:VCARD\n";
    let parsed = prepare(parse_vcf(other.as_bytes()));
    let counts = preview_counts(&conn, &parsed.cards).unwrap();
    assert_eq!((counts.already, counts.look_alike), (0, 0));
    let result = import_text(&conn, other, &plain());
    assert_eq!((result.added, result.restored.len()), (1, 0));
}

#[test]
fn the_preview_counts_who_is_already_here_and_who_looks_like_them() {
    let (_dir, conn) = store();
    let ada = create_person(
        &conn,
        &PersonDraft {
            details: vec![
                d("email", "", "ada@example.com"),
                d("phone", "", "+44 20 7946 0101"),
            ],
            ..draft("Ada", "Example")
        },
    )
    .unwrap();
    let ada_ident = text(&conn, &format!("SELECT uid FROM people WHERE id = {ada}"));
    let vcf = format!(
        "BEGIN:VCARD\nUID:{ada_ident}\nFN:Ada Example\nEND:VCARD\n\
         BEGIN:VCARD\nFN:Ada By Email\nEMAIL:ADA@example.com\nEND:VCARD\n\
         BEGIN:VCARD\nFN:Ada By Phone\nTEL:020 7946 0101\nEND:VCARD\n\
         BEGIN:VCARD\nFN:Ada By Phone\nTEL:+44 20 7946 0101\nEND:VCARD\n\
         BEGIN:VCARD\nFN:Nobody Known\nEMAIL:nobody@example.com\nEND:VCARD\n\
         BEGIN:VCARD\nEND:VCARD\n"
    );
    let parsed = prepare(parse_vcf(vcf.as_bytes()));
    let counts = preview_counts(&conn, &parsed.cards).unwrap();
    assert_eq!(parsed.cards.len(), 5);
    assert_eq!(counts.already, 1);
    // The email matches (case folded), and so do both phone forms: the national number with a
    // trunk zero is the same number as the international one, by the duplicate finder's own rule.
    assert_eq!(counts.look_alike, 3);
    let preview = import::ImportPreview::of("x".into(), "f.vcf".into(), &parsed, counts);
    // The last card has nothing to call anyone by.
    assert_eq!(preview.no_name, 1);
    assert!(preview.complete);
}

#[test]
fn new_people_get_a_source_row_the_label_and_their_own_categories() {
    let (_dir, conn) = store();
    let vcf = "BEGIN:VCARD\nUID:src-1\nFN:Eve Example\nCATEGORIES:Friends,Book club\nEND:VCARD\n\
               BEGIN:VCARD\nFN:Finn Example\nEND:VCARD\n";
    let options = ImportOptions {
        label: Some("Conference 2026".into()),
    };
    let result = import_text(&conn, vcf, &options);
    assert_eq!(result.added, 2);
    let labels: Vec<String> = list_labels(&conn)
        .unwrap()
        .into_iter()
        .map(|l| l.name)
        .collect();
    assert_eq!(labels, vec!["Book club", "Conference 2026", "Friends"]);
    // Everyone in the file carries the option's label; only Eve carries her categories.
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM person_labels"), 4);
    let snapshot = text(
        &conn,
        "SELECT snapshot FROM person_sources WHERE external_id = 'src-1'",
    );
    assert!(snapshot.contains("Eve Example"));
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM person_sources WHERE external_id IS NULL AND provider = 'vcard'"
        ),
        1
    );
}

#[test]
fn a_store_switch_mid_job_writes_nothing() {
    let (_dir, conn) = store();
    let hold = Mutex::new(ImportHold::default());
    let parsed = prepare(parse_vcf(b"BEGIN:VCARD\nFN:Gail Example\nEND:VCARD\n"));
    let now = Instant::now();
    let id = hold.lock().unwrap().put(1, parsed.cards, now).unwrap();

    // The store epoch moved (another vault opened, or this one locked and unlocked).
    let err = commit_held(&conn, &hold, 2, &id, &plain(), now).unwrap_err();
    assert_eq!(err.to_string(), STALE);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM people"), 0);

    // The dialog recognises this refusal by its opening words (STALE_PREVIEW_START in
    // src/lib/peopleTransfer.ts): rewording it must change both together.
    assert!(STALE.starts_with("That file was read a while ago"));

    // The same store, in time: it commits, once.
    let done = commit_held(&conn, &hold, 1, &id, &plain(), now).unwrap();
    assert_eq!(done.added, 1);
    let again = commit_held(&conn, &hold, 1, &id, &plain(), now).unwrap_err();
    assert_eq!(again.to_string(), STALE);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM people"), 1);
}

#[test]
fn an_old_or_replaced_preview_refuses_and_a_clear_forgets_it() {
    let (_dir, conn) = store();
    let hold = Mutex::new(ImportHold::default());
    let cards = || prepare(parse_vcf(b"BEGIN:VCARD\nFN:Hana Example\nEND:VCARD\n")).cards;
    let t0 = Instant::now();
    let first = hold.lock().unwrap().put(1, cards(), t0).unwrap();
    let second = hold.lock().unwrap().put(1, cards(), t0).unwrap();
    // A new preview replaces the old one.
    assert!(commit_held(&conn, &hold, 1, &first, &plain(), t0).is_err());
    // Ten minutes is the limit.
    let late = t0 + Duration::from_secs(10 * 60 + 1);
    assert!(commit_held(&conn, &hold, 1, &second, &plain(), late).is_err());
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM people"), 0);
    hold.lock().unwrap().clear();
    assert!(commit_held(&conn, &hold, 1, &second, &plain(), t0).is_err());
}

#[test]
fn a_dismissed_preview_is_forgotten() {
    let (_dir, conn) = store();
    let hold = Mutex::new(ImportHold::default());
    let cards = || prepare(parse_vcf(b"BEGIN:VCARD\nFN:Hana Example\nEND:VCARD\n")).cards;
    let t0 = Instant::now();
    let first = hold.lock().unwrap().put(1, cards(), t0).unwrap();
    import::discard_held(&hold, &first);
    // The dialog closed: the id no longer commits, and nothing is held.
    assert!(format!("{:?}", hold.lock().unwrap()).contains("held: false"));
    assert_eq!(
        commit_held(&conn, &hold, 1, &first, &plain(), t0)
            .unwrap_err()
            .to_string(),
        STALE
    );
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM people"), 0);

    // A discard from a dialog that was already replaced must not drop the newer preview; nor does
    // an id nothing holds.
    let second = hold.lock().unwrap().put(1, cards(), t0).unwrap();
    import::discard_held(&hold, &first);
    import::discard_held(&hold, "no-such-preview");
    assert_eq!(
        commit_held(&conn, &hold, 1, &second, &plain(), t0)
            .unwrap()
            .added,
        1
    );
    // Discarding after the import went through is nothing to do.
    import::discard_held(&hold, &second);
}

#[test]
fn an_expired_preview_is_gone_after_a_get_and_replaced_by_the_next_put() {
    let (_dir, conn) = store();
    let hold = Mutex::new(ImportHold::default());
    let cards = || prepare(parse_vcf(b"BEGIN:VCARD\nFN:Hana Example\nEND:VCARD\n")).cards;
    let held = |h: &Mutex<ImportHold>| format!("{:?}", h.lock().unwrap()).contains("held: true");
    let t0 = Instant::now();
    let late = t0 + Duration::from_secs(10 * 60 + 1);

    // A get past the ten minutes lets go of it (and refuses).
    let old = hold.lock().unwrap().put(1, cards(), t0).unwrap();
    assert!(held(&hold));
    assert!(commit_held(&conn, &hold, 1, &old, &plain(), late).is_err());
    assert!(!held(&hold), "an expired parse is not kept after a get");

    // A put replaces what is held, expired or not (put does not evict on its own): the old id
    // refuses, the new one commits.
    let stale = hold.lock().unwrap().put(1, cards(), t0).unwrap();
    let fresh = hold.lock().unwrap().put(1, cards(), late).unwrap();
    assert!(commit_held(&conn, &hold, 1, &stale, &plain(), late).is_err());
    assert_eq!(
        commit_held(&conn, &hold, 1, &fresh, &plain(), late)
            .unwrap()
            .added,
        1
    );
}

#[test]
fn a_refused_import_leaves_nothing_behind() {
    let (_dir, conn) = store();
    // A label the import can't make (too long) refuses the whole file after the people before it.
    let vcf = "BEGIN:VCARD\nFN:Ivy Example\nEND:VCARD\nBEGIN:VCARD\nFN:Jon Example\nEND:VCARD\n";
    let parsed = prepare(parse_vcf(vcf.as_bytes()));
    let options = ImportOptions {
        label: Some("x".repeat(2000)),
    };
    let tx = conn.unchecked_transaction().unwrap();
    assert!(commit_in(&tx, &parsed.cards, &options).is_err());
    drop(tx);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM people"), 0);
}

#[test]
fn a_file_over_the_cap_is_refused_in_plain_words() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.vcf");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(super::vcard::FILE_CAP as u64 + 1).unwrap();
    let err = read_capped(&path).unwrap_err();
    assert_eq!(err.to_string(), TOO_BIG);
    let small = dir.path().join("small.vcf");
    let body = b"BEGIN:VCARD\nFN:Kit\nEND:VCARD\n";
    std::fs::write(&small, body).unwrap();
    assert_eq!(read_capped(&small).unwrap(), body);
    assert!(read_capped(dir.path()).is_err());
}

#[test]
fn people_in_trash_are_never_exported() {
    let (_dir, conn) = store();
    let keep = person(&conn, "Kept", "Example");
    let gone = person(&conn, "Trashed", "Example");
    trash_people(&conn, &[gone], &[]).unwrap();
    let label = create_label(&conn, "Team").unwrap();
    label_people(&conn, &[keep, gone], label).unwrap();
    for scope in [
        all(),
        ExportScope::Label(label),
        ExportScope::Ids(vec![keep, gone]),
    ] {
        let vcf = export_vcf(&conn, &scope);
        assert!(vcf.contains("FN:Kept Example"));
        assert!(!vcf.contains("Trashed"));
    }
    trash_people(&conn, &[keep], &[]).unwrap();
    assert!(export::build(&conn, &all(), true).is_err());
}

#[test]
fn the_file_is_folded_at_75_octets_on_character_boundaries_with_crlf() {
    let (_dir, conn) = store();
    let mut p = draft("Lena", "Example");
    p.notes = "é漢字😀".repeat(80);
    create_person(&conn, &p).unwrap();
    let vcf = export_vcf(&conn, &all());
    assert!(vcf.contains("\r\n"));
    assert!(!vcf.replace("\r\n", "").contains('\n'));
    for line in vcf.split("\r\n").filter(|l| !l.is_empty()) {
        assert!(line.len() <= 75, "a line is {} octets", line.len());
    }
    // Reading it back (unfolding on bytes) recovers the note whole.
    let back = parse_vcf(vcf.as_bytes());
    assert_eq!(back.cards[0].notes, "é漢字😀".repeat(80));
}

#[test]
fn a_label_cannot_inject_a_vcard_parameter() {
    let (_dir, conn) = store();
    let evil = "x;TYPE=EVIL:y";
    let id = create_person(
        &conn,
        &PersonDraft {
            details: vec![
                d("email", evil, "mal@example.com"),
                d("phone", "work\nTEL:+1 555 0100", "+44 20 7946 0104"),
                d("web", "a\"b;c", "https://example.com/x"),
            ],
            ..draft("Mal", "Example")
        },
    )
    .unwrap();
    let vcf = export_vcf(&conn, &all());
    // The label travels only as an escaped value of its own X-ABLabel property.
    assert!(vcf.contains("X-ABLabel:x\\;TYPE=EVIL:y\r\n"));
    for line in vcf.split("\r\n") {
        assert!(
            !line.starts_with("TYPE=") && !line.replace("\\;", "").contains(";TYPE=EVIL"),
            "a label became structure"
        );
        let upper = line.to_ascii_uppercase();
        assert!(!upper.starts_with("TEL:+1 555"), "a label started a line");
    }
    let email_line = vcf.split("\r\n").find(|l| l.contains(".EMAIL")).unwrap();
    assert!(email_line.ends_with(";TYPE=INTERNET:mal@example.com"));
    // And it reads back as the label it was.
    let back = parse_vcf(vcf.as_bytes());
    assert!(back.cards[0] == card_of(&conn, id));
}

#[test]
fn a_custom_label_cannot_inject_a_vcard_parameter() {
    let (_dir, conn) = store();
    let evil = "x;TYPE=EVIL:y";
    let id = create_person(
        &conn,
        &PersonDraft {
            details: vec![
                d("custom", evil, "Rex"),
                d("custom", "line\nX-PM-CUSTOM:forged", "Fido"),
                d("custom", "Birthday (as written)", "circa 1800"),
                d("custom", "", "no label"),
                d("school", "", "Example; University\nFN:Forged"),
                d("former_name", "", "Ada\r\nTEL:+1 555 0100 Maiden"),
            ],
            ..draft("Mal", "Example")
        },
    )
    .unwrap();
    let vcf = export_vcf(&conn, &all());
    // The label is the escaped value of its own X-ABLabel, never a parameter or a line of its own.
    assert!(vcf.contains("X-ABLabel:x\\;TYPE=EVIL:y\r\n"));
    for line in vcf.split("\r\n") {
        assert!(
            !line.replace("\\;", "").contains(";TYPE=EVIL"),
            "a custom label became a parameter"
        );
        assert!(
            !line.starts_with("FN:Forged")
                && !line.starts_with("TEL:")
                && !line.starts_with("X-PM-CUSTOM:forged"),
            "a value or label started a line"
        );
    }
    assert_eq!(
        vcf.split("\r\n")
            .filter(|l| l.starts_with("X-PM-CUSTOM:") || l.contains(".X-PM-CUSTOM:"))
            .count(),
        4,
        "four custom rows, no forged fifth"
    );
    // Every one reads back as what it was, including the importer's own "as written" row.
    let back = parse_vcf(vcf.as_bytes());
    assert!(back.cards[0] == card_of(&conn, id));
}

#[test]
fn a_formula_in_a_name_is_neutralised_in_the_csv() {
    let (_dir, conn) = store();
    let sam1 = create_person(
        &conn,
        &PersonDraft {
            display_name: "=HYPERLINK(\"https://example.com\",\"x\")".into(),
            ..draft("", "")
        },
    )
    .unwrap();
    let sam2 = create_person(
        &conn,
        &PersonDraft {
            display_name: "@SUM(1+1)".into(),
            ..draft("", "")
        },
    )
    .unwrap();
    let twin_a = create_person(&conn, &draft("Sam", "Example")).unwrap();
    let twin_b = create_person(&conn, &draft("Sam", "Example")).unwrap();
    let meeting = add_meeting(
        &conn,
        "2026-03-02",
        "-2+3 follow up, \"quoted\"",
        &[sam1, sam2, twin_a, twin_b],
    );
    conn.execute(
        "UPDATE interactions SET event_title = '+cmd|calc' WHERE id = ?1",
        params![meeting],
    )
    .unwrap();
    let out = export::build(&conn, &all(), true).unwrap();
    assert_eq!(out.people, 4);
    let csv = String::from_utf8(out.csv.unwrap()).unwrap();
    let mut lines = csv.split("\r\n");
    assert_eq!(
        lines.next().unwrap(),
        "\"date\",\"kind\",\"person_uid\",\"person_name\",\"note\",\"event_title\""
    );
    // One row per person per entry: the two Sams stay apart by id.
    let rows: Vec<&str> = lines.filter(|l| !l.is_empty()).collect();
    assert_eq!(rows.len(), 4);
    assert!(csv.contains("\"'=HYPERLINK(\"\"https://example.com\"\",\"\"x\"\")\""));
    assert!(csv.contains("\"'@SUM(1+1)\""));
    assert!(csv.contains("\"'-2+3 follow up, \"\"quoted\"\"\""));
    assert!(csv.contains("\"'+cmd|calc\""));
    // No cell starts with a character a spreadsheet reads as a formula.
    for bad in ["=", "+", "-", "@", "\t"] {
        assert!(
            !csv.contains(&format!("\"{bad}")),
            "a cell starts with {bad:?}"
        );
        assert!(
            !csv.contains(&format!(",\"{bad}")),
            "a cell starts with {bad:?}"
        );
    }
    let idents: std::collections::HashSet<&str> = rows
        .iter()
        .filter(|r| r.contains("Sam Example"))
        .map(|r| r.split(',').nth(2).unwrap())
        .collect();
    assert_eq!(idents.len(), 2);
}

#[test]
fn the_timeline_leaves_out_people_in_trash_and_only_comes_when_asked() {
    let (_dir, conn) = store();
    let a = person(&conn, "Ann", "Example");
    let b = person(&conn, "Ben", "Example");
    add_meeting(&conn, "2026-03-02", "lunch", &[a, b]);
    trash_people(&conn, &[b], &[]).unwrap();
    assert!(export::build(&conn, &all(), false).unwrap().csv.is_none());
    let csv = String::from_utf8(export::build(&conn, &all(), true).unwrap().csv.unwrap()).unwrap();
    assert!(csv.contains("Ann Example"));
    assert!(!csv.contains("Ben Example"));
}

#[test]
fn writing_puts_the_spreadsheet_beside_the_file_and_never_replaces_one() {
    let (_dir, conn) = store();
    let a = person(&conn, "Ann", "Example");
    add_meeting(&conn, "2026-03-02", "lunch", &[a]);
    let out = export::build(&conn, &all(), true).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("people");
    let first = export::write(&dest, &out).unwrap();
    assert_eq!(first.people, 1);
    assert_eq!(first.files.len(), 2);
    assert!(dir.path().join("people.vcf").is_file());
    assert!(dir.path().join(TIMELINE_FILE).is_file());
    let second = export::write(&dir.path().join("people.vcf"), &out).unwrap();
    assert!(second.files[1].ends_with("people-timeline-2.csv"));
    assert!(dir.path().join(TIMELINE_FILE).is_file());
}

#[test]
fn a_hostile_file_cannot_make_an_import_refuse_for_its_content() {
    let (_dir, conn) = store();
    let long = "z".repeat(5000);
    let vcf = format!(
        "BEGIN:VCARD\nFN:{long}\nBDAY:1990-02-31\nEMAIL:{long}\nNOTE:{long}\nCATEGORIES:{long}\nORG:{long};{long}\nX-PM-MET-ON:not a date\nEND:VCARD\n"
    );
    let result = import_text(&conn, &vcf, &plain());
    assert_eq!(result.added, 1);
    let p = get_person(&conn, result.touched[0]).unwrap();
    assert!(p.display_name.chars().count() <= VALUE_MAX);
    assert_eq!(p.met_on, None);
    assert_eq!(p.born_on, None);
}

#[test]
fn no_error_or_debug_print_names_a_contact() {
    let (_dir, conn) = store();
    let sentinel = "SENTINEL-PERSON-5521";
    let vcf =
        format!("BEGIN:VCARD\nUID:s-1\nFN:{sentinel}\nEMAIL:{sentinel}@example.com\nEND:VCARD\n");
    let parsed = prepare(parse_vcf(vcf.as_bytes()));
    let hold = Mutex::new(ImportHold::default());
    let id = hold
        .lock()
        .unwrap()
        .put(1, parsed.cards, Instant::now())
        .unwrap();
    let mut shown = format!("{:?}", hold.lock().unwrap());
    let refusal = commit_held(&conn, &hold, 9, &id, &plain(), Instant::now()).unwrap_err();
    shown.push_str(&refusal.to_string());
    let done = commit_held(&conn, &hold, 1, &id, &plain(), Instant::now()).unwrap();
    shown.push_str(&format!("{done:?}"));
    let preview_text = format!(
        "{:?}",
        import::ImportPreview::of(
            id,
            "f.vcf".into(),
            &prepare(parse_vcf(vcf.as_bytes())),
            Default::default()
        )
    );
    shown.push_str(&preview_text);
    assert!(!shown.contains(sentinel));
}

#[test]
fn long_multibyte_address_parts_cannot_refuse_the_whole_file() {
    let (_dir, conn) = store();
    // Four parts of 1,024 four-byte characters: each is within its cap, together they pass what a
    // save accepts for one detail's extra fields.
    let wide = "😀".repeat(1024);
    let vcf = format!(
        "BEGIN:VCARD\nFN:Wide Example\nADR:;;{wide};{wide};{wide};{wide};\nEND:VCARD\n\
         BEGIN:VCARD\nFN:Next Example\nEND:VCARD\n"
    );
    let result = import_text(&conn, &vcf, &plain());
    assert_eq!(result.added, 2);
    let first = get_person(&conn, result.touched[0]).unwrap();
    assert!(first.details.iter().any(|d| d.kind == "address"));
}

#[test]
fn a_title_with_no_company_is_not_made_a_company() {
    let (_dir, conn) = store();
    let vcf = "BEGIN:VCARD\nFN:Titled Example\nTITLE:Chief Example Officer\nEND:VCARD\n";
    let result = import_text(&conn, vcf, &plain());
    let p = get_person(&conn, result.touched[0]).unwrap();
    assert!(!p.details.iter().any(|d| d.kind == "work"));
    let custom = p.details.iter().find(|d| d.kind == "custom").unwrap();
    assert_eq!(
        (custom.label.as_str(), custom.value.as_str()),
        ("Job title", "Chief Example Officer")
    );
    // And it goes out and comes back as the same field.
    let vcf = export_vcf(&conn, &all());
    assert!(!vcf.contains("ORG"));
    assert!(parse_vcf(vcf.as_bytes()).cards[0] == card_of(&conn, result.touched[0]));
}

#[test]
fn a_label_alone_makes_a_matched_person_updated_and_a_full_person_makes_no_label() {
    let (_dir, conn) = store();
    let id = create_person(&conn, &draft("Lena", "Example")).unwrap();
    let ident = text(&conn, &format!("SELECT uid FROM people WHERE id = {id}"));
    let vcf =
        format!("BEGIN:VCARD\nUID:{ident}\nFN:Lena Example\nCATEGORIES:Neighbours\nEND:VCARD\n");
    let result = import_text(&conn, &vcf, &plain());
    assert_eq!((result.updated, result.unchanged), (1, 0));
    // The same file again gives her nothing new.
    let again = import_text(&conn, &vcf, &plain());
    assert_eq!((again.updated, again.unchanged), (0, 1));

    // A person already at the most labels gets none, and the label is not made for nobody.
    for n in 0..LABELS_PER_PERSON_MAX {
        let label = create_label(&conn, &format!("Full {n}")).unwrap();
        label_people(&conn, &[id], label).ok();
    }
    let before = count(&conn, "SELECT COUNT(*) FROM contact_labels");
    let vcf =
        format!("BEGIN:VCARD\nUID:{ident}\nFN:Lena Example\nCATEGORIES:Never Made\nEND:VCARD\n");
    let result = import_text(&conn, &vcf, &plain());
    assert_eq!(result.unchanged, 1);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM contact_labels"), before);
}

#[test]
fn a_vcf_added_to_a_name_never_replaces_a_file_that_is_there() {
    let (_dir, conn) = store();
    create_person(&conn, &draft("Writer", "Example")).unwrap();
    let exported = export::build(&conn, &all(), false).unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("people.vcf"), b"keep me").unwrap();
    let done = export::write(&dir.path().join("people"), &exported).unwrap();
    assert_eq!(
        std::fs::read(dir.path().join("people.vcf")).unwrap(),
        b"keep me"
    );
    assert!(done.files[0].ends_with("people-2.vcf"));
}
