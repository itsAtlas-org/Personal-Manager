// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tests for the vCard parser: the synthetic corpus of the research notes (Apple 3.0, Android and
//! Outlook 2.1, 4.0, and hostile files). Every person is invented: example.com addresses, 555-01xx
//! style numbers and made-up names. The parser takes bytes, so nothing here touches a disk.

use super::card::{Card, CardDetail};
use super::vcard::{parse_vcf, parse_vcf_capped, Caps, VcfParse, Warning};

fn crlf(lines: &[&str]) -> Vec<u8> {
    let mut out = lines.join("\r\n");
    out.push_str("\r\n");
    out.into_bytes()
}

fn parse_text(text: &str) -> VcfParse {
    parse_vcf(text.as_bytes())
}

fn detail<'a>(card: &'a Card, kind: &str) -> Vec<&'a CardDetail> {
    card.details.iter().filter(|d| d.kind == kind).collect()
}

fn labels_of(card: &Card, kind: &str) -> Vec<String> {
    detail(card, kind).iter().map(|d| d.label.clone()).collect()
}

fn values_of(card: &Card, kind: &str) -> Vec<String> {
    detail(card, kind).iter().map(|d| d.value.clone()).collect()
}

fn warned(p: &VcfParse, w: Warning) -> usize {
    p.warnings.get(&w).copied().unwrap_or(0)
}

const APPLE: &[&str] = &[
    "BEGIN:VCARD",
    "VERSION:3.0",
    "PRODID:-//Apple Inc.//iOS 17//EN",
    "N:Lovelace;Ada;Augusta;Countess;",
    "FN:Ada Lovelace",
    "ORG:Analytical Engines;Research",
    "TITLE:Head of Sales",
    "item1.EMAIL;type=INTERNET;type=pref:ada@example.com",
    "item1.X-ABLabel:_$!<Work>!$_",
    "item2.EMAIL;type=INTERNET:ada.home@example.com",
    "item2.X-ABLabel:Spouse's work",
    "TEL;type=CELL;type=VOICE;type=pref:+44 7700 900101",
    "item3.URL;type=pref:https://www.linkedin.com/in/ada-example",
    "item3.X-ABLabel:_$!<HomePage>!$_",
    "URL:https://github.com/ada-example",
    "BDAY;X-APPLE-OMIT-YEAR=1604:1604-04-15",
    "item4.X-ABDATE;type=pref:2010-06-01",
    "item4.X-ABLabel:_$!<Anniversary>!$_",
    "item5.X-ABRELATEDNAMES;type=pref:Charles Example",
    "item5.X-ABLabel:_$!<Spouse>!$_",
    "X-SOCIALPROFILE;type=linkedin;x-user=ada:https://www.linkedin.com/in/ada",
    "IMPP;X-SERVICE-TYPE=Skype;type=HOME:skype:ada.example",
    "NOTE:First line\\nSecond\\, with comma\\; and semi",
    "  folded",
    "X-ABUID:ABC123:ABPerson",
    "PHOTO;ENCODING=b;TYPE=JPEG:/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAMCAgMCAgMDAwMEAwMEBQgFBQQEBQoH",
    " BwYIDAoMDAsKCwsNDhIQDQ4RDgsLEBYQERMUFRUVDA8XGBYUGBIUFRT/2wBDAQMEBAUEBQkFBQkUDQsNFBQUFBQU",
    " FBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBT/wAARCAABAAEDASIAAhEBAxEB/8QA",
    "UID:11111111-2222-3333-4444-555555555555",
    "END:VCARD",
    "BEGIN:VCARD",
    "VERSION:3.0",
    "FN:Boris Example",
    "EMAIL:boris@example.com",
    "END:VCARD",
    "BEGIN:VCARD",
    "VERSION:3.0",
    "N:Example;Cleo;;;",
    "FN:Cleo Example",
    "TEL;type=HOME:020 7946 0103",
    "END:VCARD",
];

#[test]
fn an_apple_3_0_file_maps_grouped_labels_dates_and_socials() {
    let p = parse_vcf(&crlf(APPLE));
    assert!(p.complete);
    assert_eq!(p.cards.len(), 3);
    let ada = &p.cards[0];
    assert_eq!(ada.display_name, "Ada Lovelace");
    assert_eq!(ada.family_name, "Lovelace");
    assert_eq!(ada.given_name, "Ada");
    assert_eq!(ada.middle_name, "Augusta");
    assert_eq!(ada.name_prefix, "Countess");
    assert_eq!(
        ada.ident.as_deref(),
        Some("11111111-2222-3333-4444-555555555555")
    );
    // The grouped label beats TYPE; Apple's wrapper is gone; the preferred email is first.
    assert_eq!(values_of(ada, "email")[0], "ada@example.com");
    assert_eq!(labels_of(ada, "email"), vec!["work", "Spouse's work"]);
    assert_eq!(labels_of(ada, "phone"), vec!["mobile"]);
    // A label on the URL overrides the network; the others read as networks and services.
    assert_eq!(
        labels_of(ada, "web"),
        vec!["Homepage", "GitHub", "LinkedIn", "Skype"]
    );
    assert_eq!(values_of(ada, "web")[3], "ada.example");
    // OMIT-YEAR 1604 is no year.
    assert_eq!(ada.born_on.as_deref(), Some("--04-15"));
    assert_eq!(labels_of(ada, "date"), vec!["Anniversary"]);
    assert_eq!(values_of(ada, "date"), vec!["2010-06-01"]);
    assert_eq!(labels_of(ada, "relation"), vec!["Spouse"]);
    assert_eq!(values_of(ada, "relation"), vec!["Charles Example"]);
    let job = detail(ada, "work")[0];
    assert_eq!(job.value, "Analytical Engines");
    assert_eq!(job.parts.as_ref().unwrap()["title"], "Head of Sales");
    assert_eq!(job.parts.as_ref().unwrap()["department"], "Research");
    assert!(job.is_current);
    // Escapes undone, a fold's one space dropped and the second kept.
    assert_eq!(ada.notes, "First line\nSecond, with comma; and semi folded");
    // Counted by name only.
    assert_eq!(p.ignored.get("PHOTO"), Some(&1));
    assert_eq!(p.ignored.get("X-ABUID"), Some(&1));
    assert_eq!(p.ignored.get("PRODID"), Some(&1));
    assert_eq!(p.cards[1].display_name, "Boris Example");
    assert_eq!(p.cards[2].display_name, "Cleo Example");
}

#[test]
fn an_android_2_1_file_reads_quoted_printable_bare_params_and_lowercase_framing() {
    let text = [
        "BEGIN:VCARD",
        "VERSION:2.1",
        "N;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:=E5=BC=A0;=E4=B8=89;;;",
        "FN;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:=E5=BC=A0=E4=B8=89",
        "TEL;CELL;VOICE:+44 7700 900102",
        "TEL;HOME:020 7946 0102",
        "ADR;HOME:;;12 Example Street;Leeds;;LS1 1AA;United Kingdom",
        "NOTE;ENCODING=QUOTED-PRINTABLE:First line=0D=0A=",
        " second line kept=",
        " and more",
        "X-ANDROID-CUSTOM:vnd.android.cursor.item/nickname;Nick;1;;;;;;;;;;;;;",
        "EMAIL;INTERNET;WORK:zhang@example.com",
        "END:VCARD",
        "begin:vcard",
        "version:2.1",
        "fn:Lowercase Person",
        "end:vcard",
        "",
    ]
    .join("\n");
    let p = parse_text(&text);
    assert!(p.complete);
    assert_eq!(p.cards.len(), 2);
    let zhang = &p.cards[0];
    assert_eq!(zhang.display_name, "张三");
    assert_eq!(zhang.family_name, "张");
    assert_eq!(zhang.given_name, "三");
    assert_eq!(labels_of(zhang, "phone"), vec!["mobile", "home"]);
    assert_eq!(labels_of(zhang, "email"), vec!["work"]);
    let adr = detail(zhang, "address")[0];
    assert_eq!(adr.label, "home");
    assert_eq!(
        adr.value,
        "12 Example Street\nLeeds LS1 1AA\nUnited Kingdom"
    );
    assert_eq!(adr.parts.as_ref().unwrap()["postal_code"], "LS1 1AA");
    // The soft break keeps the continuation's leading space.
    assert_eq!(zhang.notes, "First line\n second line kept and more");
    assert_eq!(p.ignored.get("X-ANDROID-CUSTOM"), Some(&1));
    assert_eq!(p.cards[1].display_name, "Lowercase Person");
}

#[test]
fn a_utf16_file_with_a_bom_reads_and_so_does_one_with_bare_cr() {
    let text = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Zoë Example\r\nEND:VCARD\r\n";
    let mut le = vec![0xFF, 0xFE];
    le.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    let mut be = vec![0xFE, 0xFF];
    be.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
    for bytes in [le, be] {
        let p = parse_vcf(&bytes);
        assert_eq!(p.cards.len(), 1);
        assert_eq!(p.cards[0].display_name, "Zoë Example");
    }
    let cr = parse_text("BEGIN:VCARD\rFN:Old Mac\rEND:VCARD\r");
    assert_eq!(cr.cards[0].display_name, "Old Mac");
    let mut bom = vec![0xEF, 0xBB, 0xBF];
    bom.extend(text.as_bytes());
    assert_eq!(parse_vcf(&bom).cards.len(), 1);
}

#[test]
fn a_4_0_file_reads_year_less_dates_company_cards_and_standard_socials() {
    let p = parse_vcf(&crlf(&[
        "BEGIN:VCARD",
        "VERSION:4.0",
        "KIND:org",
        "FN:Example Widgets Ltd",
        "ORG:Example Widgets Ltd",
        "TEL;VALUE=uri;TYPE=cell:tel:+44-7700-900103",
        "BDAY:--0415",
        "ANNIVERSARY:19960415",
        "SOCIALPROFILE;SERVICE-TYPE=Mastodon:https://example.com/@widgets",
        "RELATED;TYPE=friend:urn:uuid:99999999-0000-0000-0000-000000000000",
        "END:VCARD",
        "BEGIN:VCARD",
        "VERSION:4.0",
        "KIND:group",
        "FN:Book Club",
        "END:VCARD",
        "BEGIN:VCARD",
        "VERSION:4.0",
        "KIND:location",
        "FN:Example Hall",
        "END:VCARD",
    ]));
    // The company card is a person (E19); the group and the place are not.
    assert_eq!(p.cards.len(), 1);
    let c = &p.cards[0];
    assert_eq!(c.display_name, "Example Widgets Ltd");
    assert_eq!(values_of(c, "phone"), vec!["+44-7700-900103"]);
    assert_eq!(labels_of(c, "phone"), vec!["mobile"]);
    assert_eq!(c.born_on.as_deref(), Some("--04-15"));
    assert_eq!(labels_of(c, "date"), vec!["Anniversary"]);
    assert_eq!(labels_of(c, "web"), vec!["Mastodon"]);
    assert!(
        detail(c, "relation").is_empty(),
        "a link to a card isn't a name"
    );
    assert_eq!(p.ignored.get("KIND"), Some(&2));
}

#[test]
fn a_birthday_that_is_not_a_date_is_kept_as_written() {
    let p = parse_text("BEGIN:VCARD\nFN:Dee Example\nBDAY;VALUE=text:circa 1800\nEND:VCARD\n");
    let c = &p.cards[0];
    assert_eq!(c.born_on, None);
    assert_eq!(labels_of(c, "custom"), vec!["Birthday (as written)"]);
    assert_eq!(values_of(c, "custom"), vec!["circa 1800"]);
    assert_eq!(warned(&p, Warning::BadDate), 1);
}

#[test]
fn a_photo_is_skipped_without_being_buffered() {
    // A line cap far below the photo: had the photo been built as a line, it would have been cut
    // and warned about. It is stepped over, and the property after it still reads.
    let mut text = String::from("BEGIN:VCARD\r\nFN:Fay Example\r\nPHOTO;ENCODING=b;TYPE=JPEG:");
    for _ in 0..400 {
        text.push_str("\r\n ");
        text.push_str(&"QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVo".repeat(2));
    }
    text.push_str("\r\nEMAIL:fay@example.com\r\nEND:VCARD\r\n");
    let caps = Caps {
        line: 256,
        ..Caps::default()
    };
    let p = parse_vcf_capped(text.as_bytes(), caps);
    assert_eq!(p.cards.len(), 1);
    assert_eq!(warned(&p, Warning::ValueCut), 0);
    assert_eq!(p.ignored.get("PHOTO"), Some(&1));
    assert_eq!(values_of(&p.cards[0], "email"), vec!["fay@example.com"]);
}

#[test]
fn a_2_1_photo_with_unindented_base64_and_a_blank_line_is_stepped_over() {
    let p = parse_text(
        "BEGIN:VCARD\nVERSION:2.1\nFN:Gus Example\nPHOTO;ENCODING=BASE64;JPEG:\n/9j/4AAQSkZJRgABAQ\nAAAQABAAD/2wBDAAMC\n\nEMAIL:gus@example.com\nEND:VCARD\n",
    );
    assert_eq!(p.cards.len(), 1);
    assert_eq!(values_of(&p.cards[0], "email"), vec!["gus@example.com"]);
    assert_eq!(warned(&p, Warning::UnreadableLine), 0);
}

#[test]
fn a_card_with_no_end_is_not_imported_and_the_file_says_it_was_cut() {
    let p = parse_text(
        "BEGIN:VCARD\nFN:Whole Person\nEND:VCARD\nBEGIN:VCARD\nFN:Cut Off\nEMAIL:cut@example.com",
    );
    assert_eq!(p.cards.len(), 1);
    assert_eq!(p.cards[0].display_name, "Whole Person");
    assert!(!p.complete);
}

#[test]
fn a_begin_inside_a_card_does_not_nest_and_does_not_lose_the_first() {
    let p = parse_text(
        "BEGIN:VCARD\nFN:Outer Person\nBEGIN:VCARD\nFN:Inner Person\nEND:VCARD\nEND:VCARD\n",
    );
    assert_eq!(p.cards.len(), 2);
    assert_eq!(p.cards[0].display_name, "Outer Person");
    assert_eq!(p.cards[1].display_name, "Inner Person");
    assert_eq!(warned(&p, Warning::UnfinishedCard), 1);
    assert!(p.complete);
}

#[test]
fn an_oversized_note_and_a_two_mib_line_are_cut_and_never_panic() {
    let big = "a".repeat(200 * 1024);
    let p = parse_text(&format!(
        "BEGIN:VCARD\nFN:Big Note\nNOTE:{big}\nEND:VCARD\n"
    ));
    assert_eq!(p.cards.len(), 1);
    assert!(p.cards[0].notes.len() <= super::vcard::NOTE_MAX);
    assert!(warned(&p, Warning::ValueCut) >= 1);

    let huge = "b".repeat(2 * 1024 * 1024);
    let p = parse_text(&format!(
        "BEGIN:VCARD\nFN:Long Line\nNOTE:{huge}\nEND:VCARD\nBEGIN:VCARD\nFN:After\nEND:VCARD\n"
    ));
    assert_eq!(p.cards.len(), 2);
    assert!(p.cards[0].notes.len() <= super::vcard::NOTE_MAX);
    assert!(warned(&p, Warning::ValueCut) >= 1);
    assert!(p.complete);

    // A line with no colon at all is just unreadable, however long.
    let p = parse_text(&format!("BEGIN:VCARD\nFN:Odd\n{huge}\nEND:VCARD\n"));
    assert_eq!(p.cards.len(), 1);
    assert_eq!(warned(&p, Warning::UnreadableLine), 1);
}

#[test]
fn huge_counts_are_clamped_and_say_so() {
    let mut text = String::from("BEGIN:VCARD\nFN:Many Emails\n");
    for i in 0..40 {
        text.push_str(&format!("EMAIL:e{i}@example.com\n"));
    }
    text.push_str("END:VCARD\n");
    let caps = Caps {
        props_per_card: 6,
        ..Caps::default()
    };
    let p = parse_vcf_capped(text.as_bytes(), caps);
    assert_eq!(p.cards.len(), 1);
    // The name is one of the six properties kept.
    assert_eq!(detail(&p.cards[0], "email").len(), 5);
    assert_eq!(warned(&p, Warning::TooManyProperties), 1);

    // A real card with far more than a person may hold is cut to the person's cap.
    let mut text = String::from("BEGIN:VCARD\nFN:Absurd\n");
    for i in 0..600 {
        text.push_str(&format!("TEL:+44 20 7946 {i:04}\n"));
    }
    text.push_str("END:VCARD\n");
    let p = parse_text(&text);
    assert_eq!(detail(&p.cards[0], "phone").len(), 499);
    assert_eq!(warned(&p, Warning::TooManyProperties), 1);

    let many_cards: String = (0..5)
        .map(|i| format!("BEGIN:VCARD\nFN:Person {i}\nEND:VCARD\n"))
        .collect();
    let caps = Caps {
        cards: 2,
        ..Caps::default()
    };
    let p = parse_vcf_capped(many_cards.as_bytes(), caps);
    assert_eq!(p.cards.len(), 2);
    assert!(!p.complete);

    let caps = Caps {
        props_total: 3,
        ..Caps::default()
    };
    let p = parse_vcf_capped(many_cards.as_bytes(), caps);
    assert!(p.cards.len() < 5);
    assert!(!p.complete);
}

#[test]
fn hostile_text_stays_inert_text() {
    let p = parse_text(
        "BEGIN:VCARD\nFN:Sam\u{202e}Reed\nURL:javascript:alert(1)\nURL:file:///etc/passwd\nEND:VCARD\n",
    );
    let c = &p.cards[0];
    // The spoofing control is gone from the name; the URLs are kept as text, never fetched or run.
    assert_eq!(c.display_name, "SamReed");
    assert_eq!(
        values_of(c, "web"),
        vec!["javascript:alert(1)", "file:///etc/passwd"]
    );
    assert_eq!(labels_of(c, "web"), vec!["Website", "Website"]);
}

#[test]
fn a_card_with_nothing_to_call_the_person_by_is_skipped_and_counted() {
    let p = parse_text(
        "BEGIN:VCARD\nFN:\nEND:VCARD\nBEGIN:VCARD\nNOTE:only a note\nEND:VCARD\nBEGIN:VCARD\nFN:\nEMAIL:composed@example.com\nEND:VCARD\n",
    );
    assert_eq!(warned(&p, Warning::NoName), 2);
    assert_eq!(p.cards.len(), 1);
    // No name given: PM calls the person by their email, as it does for a typed one.
    assert_eq!(p.cards[0].display_name, "composed@example.com");
}

#[test]
fn odd_inputs_never_panic_and_never_drop_a_good_card() {
    // None of these may panic, closed or not.
    let rough: &[&[u8]] = &[
        b"",
        b"BEGIN:VCARD",
        b"BEGIN:VCARD\nBEGIN:VCARD\nEND:VCARD",
        b"BEGIN:VCARD\nNOTE;ENCODING=QUOTED-PRINTABLE:abc=",
        b"\0\0\0\0",
        b"END:VCARD\nEND:VCARD\n",
    ];
    for bytes in rough {
        let _ = parse_vcf(bytes);
    }
    // These close their own card, so the good card after each is read whatever was wrong in it.
    let closed: &[&[u8]] = &[
        b"BEGIN:VCARD\nFN:Lone\\\nEND:VCARD\n",
        b"BEGIN:VCARD\nNOTE;ENCODING=QUOTED-PRINTABLE:a=ZZb=4\nFN:Bad Hex\nEND:VCARD\n",
        b"BEGIN:VCARD\nFN:Nul\0Byte\nEND:VCARD\n",
        b"BEGIN:VCARD\nFN;:Empty Param\nEND:VCARD\n",
        b"BEGIN:VCARD\nFN;X=\"unclosed:Quote\nEND:VCARD\n",
        b"BEGIN:VCARD\nitem1.EMAIL:a@example.com\nEND:VCARD\n",
        b"BEGIN:VCARD\n.FN:x\n..:y\nEND:VCARD\n",
        b"BEGIN:VCARD\nFN:Caf\xe9 Example\nEND:VCARD\n",
    ];
    for bytes in closed {
        let mut both = bytes.to_vec();
        both.extend_from_slice(b"BEGIN:VCARD\nFN:Good Person\nEND:VCARD\n");
        let p = parse_vcf(&both);
        assert!(p.cards.iter().any(|c| c.display_name == "Good Person"));
    }
    // Every truncation of a real file parses without panicking.
    let file = crlf(APPLE);
    for end in (0..file.len()).step_by(7) {
        let _ = parse_vcf(&file[..end]);
    }
}

#[test]
fn invalid_utf8_falls_back_to_windows_1252_with_a_warning_and_bad_hex_stays_literal() {
    let p = parse_vcf(b"BEGIN:VCARD\nFN:Caf\xe9 Example\nEND:VCARD\n");
    assert_eq!(p.cards[0].display_name, "Café Example");
    assert_eq!(warned(&p, Warning::BadEncoding), 1);
    let p =
        parse_vcf(b"BEGIN:VCARD\nFN:Hex\nNOTE;ENCODING=QUOTED-PRINTABLE:a=ZZb=3Dc\nEND:VCARD\n");
    assert_eq!(p.cards[0].notes, "a=ZZb=c");
    let p = parse_vcf(b"BEGIN:VCARD\nFN;CHARSET=ISO-8859-1:Ren\xe9 Example\nEND:VCARD\n");
    assert_eq!(p.cards[0].display_name, "René Example");
    assert_eq!(warned(&p, Warning::BadEncoding), 0);
}

#[test]
fn a_value_that_looks_like_a_group_is_not_one() {
    let p = parse_text("BEGIN:VCARD\nFN:Group Test\nNOTE:item1.EMAIL:not@example.com\nEND:VCARD\n");
    assert!(detail(&p.cards[0], "email").is_empty());
    assert_eq!(p.cards[0].notes, "item1.EMAIL:not@example.com");
}

#[test]
fn quoted_parameters_may_hold_colons_and_semicolons() {
    let p = parse_text(
        "BEGIN:VCARD\nFN:Quoted Param\nADR;LABEL=\"1 Main St^nTown: X; Y\":;;;;;;\nEND:VCARD\n",
    );
    let adr = detail(&p.cards[0], "address")[0];
    assert_eq!(adr.value, "1 Main St\nTown: X; Y");
}

#[test]
fn the_parser_never_fetches() {
    // By construction: nothing in the module's shipped code names a network or process type, and
    // every import is one of the pure ones below.
    let source = include_str!("vcard.rs");
    let shipped = source
        .split("#[cfg(test)]\nmod tests {")
        .next()
        .unwrap_or_default();
    for forbidden in [
        "reqwest",
        "hyper",
        "TcpStream",
        "UdpSocket",
        "std::net",
        "tokio::net",
        "std::process",
        "std::fs",
        "tauri",
        "open::",
        "ureq",
    ] {
        assert!(
            !shipped.contains(forbidden),
            "vcard.rs must not reach for {forbidden}"
        );
    }
    let allowed = [
        "use std::borrow::Cow;",
        "use std::collections::{BTreeMap, HashMap};",
        "use serde::Serialize;",
        "use serde_json::{Map, Value};",
        "use super::card::{truncate_bytes, truncate_chars, Card, CardDetail};",
        "use super::partial_date::PartialDate;",
    ];
    for line in shipped.lines().filter(|l| l.starts_with("use ")) {
        assert!(allowed.contains(&line), "an unexpected import: {line}");
    }
}

#[test]
fn no_value_reaches_a_warning_or_a_debug_print() {
    let sentinel = "SENTINEL-NAME-7731";
    let p = parse_text(&format!(
        "BEGIN:VCARD\nFN:{sentinel}\nX-NOTE:{sentinel}\nNOTE;ENCODING=QUOTED-PRINTABLE:=ZZ\nBDAY:{sentinel}\nEND:VCARD\nBEGIN:VCARD\nFN:\nEND:VCARD\n"
    ));
    assert!(!format!("{p:?}").contains(sentinel));
    for w in p.warnings.keys() {
        assert!(!w.as_str().contains(sentinel));
    }
    // An unknown property is listed by its name, never by its value.
    assert!(p.ignored.keys().all(|k| !k.contains(sentinel)));
    assert_eq!(p.ignored.get("X-NOTE"), Some(&1));
}
