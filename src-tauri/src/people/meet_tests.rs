// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tests for the meetings owed an answer and answering them. Every person is invented: example.com
//! addresses, made-up ASCII names. The clock is pinned and the table runs under a sweep of zones
//! (an all-day event's end is the user's midnight, so the answer must not depend on the machine's).

use std::collections::BTreeSet;
use std::str::FromStr;

use chrono::{DateTime, Duration, SecondsFormat, TimeZone, Utc};
use chrono_tz::Tz;
use rusqlite::{params, Connection};
use serde_json::{json, Value};

use super::meet::{
    answer_meeting, event_guests, owed_meetings_at, MeetingAnswer, MeetingAnswered, NewGuest,
    OwedMeeting, PersonRef,
};
use super::tests::{count, detail, draft, person, store, text};
use super::timeline::list_timeline;
use super::*;
use crate::calendar::Attendee;

fn pinned() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 26, 12, 0, 0).unwrap()
}

fn iso(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn zone(name: &str) -> Tz {
    Tz::from_str(name).unwrap()
}

fn gmt(hours_east: i32) -> Tz {
    let name = if hours_east >= 0 {
        format!("Etc/GMT-{hours_east}")
    } else {
        format!("Etc/GMT+{}", -hours_east)
    };
    zone(&name)
}

// --- the calendar fixture ---------------------------------------------------------------------------

const GOOGLE_MAIN: &str = "gcal:me@example.com:main";
const GOOGLE_ALT: &str = "gcal:me@example.com:alt";
const GOOGLE_QUIET: &str = "gcal:me@example.com:quiet";
const OUTLOOK: &str = "outlook:me@work.example.com:cal";
const ICS_FEED: &str = "ical:0123456789abcdef";

fn calendars(conn: &Connection) {
    for (id, provider, email) in [
        ("gcal:me@example.com", "google", Some("me@example.com")),
        (
            "outlook:me@work.example.com",
            "microsoft",
            Some("me@work.example.com"),
        ),
        ("ical:0123456789abcdef", "other", None),
    ] {
        conn.execute(
            "INSERT INTO connector_sources(id, provider, service, label, account_email) \
             VALUES (?1, ?2, 'calendar', 'cal', ?3)",
            params![id, provider, email],
        )
        .unwrap();
    }
    // A Drive sign-in is an address of the user's too.
    conn.execute(
        "INSERT INTO connector_sources(id, provider, service, label, account_email) \
         VALUES ('gdrive:me@drive.example.com', 'google', 'drive', 'drive', 'me@drive.example.com')",
        [],
    )
    .unwrap();
    for (id, source, provider, quiet) in [
        (GOOGLE_MAIN, "gcal:me@example.com", "google", 0),
        (GOOGLE_ALT, "gcal:me@example.com", "google", 0),
        (GOOGLE_QUIET, "gcal:me@example.com", "google", 1),
        (OUTLOOK, "outlook:me@work.example.com", "microsoft", 0),
        (ICS_FEED, "ical:0123456789abcdef", "other", 0),
    ] {
        conn.execute(
            "INSERT INTO calendars(id, source_id, provider, name, quiet) VALUES (?1, ?2, ?3, 'c', ?4)",
            params![id, source, provider, quiet],
        )
        .unwrap();
    }
}

/// One mirrored event row.
struct Ev {
    calendar: &'static str,
    uid: Option<String>,
    title: String,
    start: String,
    end: Option<String>,
    all_day: bool,
    recurring: bool,
    series: Option<String>,
    kind: Option<&'static str>,
    guests: Vec<Value>,
    packed: bool,
}

fn ev(uid: &str, start: &str, end: Option<&str>, all_day: bool) -> Ev {
    Ev {
        calendar: GOOGLE_MAIN,
        uid: (!uid.is_empty()).then(|| uid.to_string()),
        title: format!("Meeting {uid}"),
        start: start.into(),
        end: end.map(str::to_string),
        all_day,
        recurring: false,
        series: None,
        kind: None,
        guests: vec![guest("Bob Baker", "bob@example.com")],
        packed: false,
    }
}

fn guest(name: &str, email: &str) -> Value {
    json!({ "name": name, "email": email, "response": "accepted", "optional": false,
            "organizer": false, "self": false })
}

fn google_me(response: &str) -> Value {
    json!({ "name": "Me", "email": "me@example.com", "response": response, "optional": false,
            "organizer": false, "self": true })
}

/// An Outlook guest list carries no `self`: the user is the guest with the account's address.
fn outlook_me(response: &str) -> Value {
    json!({ "name": "Me", "email": "me@work.example.com", "response": response, "optional": false,
            "organizer": false, "self": false })
}

fn put(conn: &Connection, e: &Ev) {
    let attendees = (!e.guests.is_empty()).then(|| Value::Array(e.guests.clone()).to_string());
    let id = format!(
        "{}:{}:{}",
        e.calendar,
        e.uid.clone().unwrap_or_else(|| e.title.clone()),
        e.start
    );
    let stored: rusqlite::types::Value = match attendees {
        None => rusqlite::types::Value::Null,
        Some(t) if e.packed => rusqlite::types::Value::Blob(crate::calendar_history::pack(&t)),
        Some(t) => rusqlite::types::Value::Text(t),
    };
    conn.execute(
        "INSERT INTO calendar_events(id, calendar_id, summary, start, end, all_day, uid, recurring, \
                                     series_id, attendees, event_type) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            id,
            e.calendar,
            e.title,
            e.start,
            e.end,
            e.all_day,
            e.uid,
            e.recurring,
            e.series,
            stored,
            e.kind
        ],
    )
    .unwrap();
}

fn owed_uids(conn: &Connection, now: DateTime<Utc>, z: Tz) -> BTreeSet<String> {
    owed_meetings_at(conn, now, z)
        .unwrap()
        .into_iter()
        .map(|m| m.event_ical)
        .collect()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

/// A day relative to the user's civil today in `z` at `now`: `YYYY-MM-DD`.
fn day(now: DateTime<Utc>, z: Tz, offset: i64) -> String {
    let today = crate::clock::today_in_at(z, now);
    (today + Duration::days(offset))
        .format("%Y-%m-%d")
        .to_string()
}

/// Every case of the table: the events, and the ones that must be owed. Built relative to the zone's
/// civil today (all-day) and to the pinned instant (timed), so the expected set holds in every zone.
fn table(conn: &Connection, now: DateTime<Utc>, z: Tz) -> BTreeSet<String> {
    let hours = |h: i64| iso(now + Duration::hours(h));
    let days = |d: i64| iso(now - Duration::days(d));
    let all_day = |uid: &str, last_day_offset: i64| {
        ev(
            uid,
            &day(now, z, last_day_offset),
            Some(&day(now, z, last_day_offset + 1)),
            true,
        )
    };
    let owed = std::cell::RefCell::new(Vec::new());
    let add = |name: &str, owed_expected: bool, e: Ev| {
        put(conn, &e);
        if owed_expected {
            owed.borrow_mut().push(name.to_string());
        }
    };

    // Timed: ended hours ago is owed; still running, 13 days and 15 days.
    add(
        "t-ended",
        true,
        ev("t-ended", &hours(-4), Some(&hours(-3)), false),
    );
    add(
        "t-running",
        false,
        ev("t-running", &hours(-1), Some(&hours(1)), false),
    );
    add(
        "t-13d",
        true,
        ev(
            "t-13d",
            &days(13),
            Some(&iso(now - Duration::days(13) + Duration::hours(1))),
            false,
        ),
    );
    add(
        "t-15d",
        false,
        ev(
            "t-15d",
            &days(15),
            Some(&iso(now - Duration::days(15) + Duration::hours(1))),
            false,
        ),
    );
    // A timed event with no end is judged at its start.
    add("t-no-end", true, ev("t-no-end", &hours(-5), None, false));

    // All-day: the last civil day is yesterday (ended at the user's midnight), today (not yet),
    // 14 days ago (still asked about) and 15 days ago (too old).
    add("a-yesterday", true, all_day("a-yesterday", -1));
    add("a-today", false, all_day("a-today", 0));
    add("a-14d", true, all_day("a-14d", -14));
    add("a-15d", false, all_day("a-15d", -15));
    // A two-day conference is owed once its last day has passed, not before.
    add(
        "a-conf-over",
        true,
        ev(
            "a-conf-over",
            &day(now, z, -3),
            Some(&day(now, z, -1)),
            true,
        ),
    );
    add(
        "a-conf-on",
        false,
        ev("a-conf-on", &day(now, z, -1), Some(&day(now, z, 1)), true),
    );

    // Declined: on Google by the self guest; on Outlook by the account's address; accepted is owed.
    let mut e = ev("g-declined", &hours(-4), Some(&hours(-3)), false);
    e.guests.push(google_me("declined"));
    add("g-declined", false, e);
    let mut e = ev("g-accepted", &hours(-4), Some(&hours(-3)), false);
    e.guests.push(google_me("accepted"));
    add("g-accepted", true, e);
    let mut e = ev("o-declined", &hours(-4), Some(&hours(-3)), false);
    e.calendar = OUTLOOK;
    e.guests.push(outlook_me("declined"));
    add("o-declined", false, e);
    let mut e = ev("o-accepted", &hours(-4), Some(&hours(-3)), false);
    e.calendar = OUTLOOK;
    e.guests.push(outlook_me("accepted"));
    add("o-accepted", true, e);
    // An Outlook invite someone else sent to the user alone: the parser lists its organiser the way
    // Google does (`graph_attendees`), with Graph's "organizer" status, so it is owed.
    let organiser = json!({ "name": "Olive Organiser", "email": "olive.organiser@example.com",
                            "response": "organizer", "optional": false, "organizer": true,
                            "self": false });
    let mut e = ev("o-invited", &hours(-4), Some(&hours(-3)), false);
    e.calendar = OUTLOOK;
    e.guests = vec![outlook_me("accepted"), organiser.clone()];
    add("o-invited", true, e);
    let mut e = ev("o-invited-declined", &hours(-4), Some(&hours(-3)), false);
    e.calendar = OUTLOOK;
    e.guests = vec![outlook_me("declined"), organiser];
    add("o-invited-declined", false, e);
    // ICS: nobody can be identified as the user, so a "declined" guest is somebody else's answer
    // and the meeting is asked about.
    let mut e = ev("ics-nobody", &hours(-4), Some(&hours(-3)), false);
    e.calendar = ICS_FEED;
    e.guests.push(
        json!({ "name": "Cat", "email": "cat@example.com", "response": "declined",
                          "optional": false, "organizer": false, "self": false }),
    );
    add("ics-nobody", true, e);

    // A quiet calendar, and event types that are not meetings.
    let mut e = ev("quiet", &hours(-4), Some(&hours(-3)), false);
    e.calendar = GOOGLE_QUIET;
    add("quiet", false, e);
    for (name, kind) in [
        ("k-birthday", "birthday"),
        ("k-focus", "focusTime"),
        ("k-ooo", "outOfOffice"),
        ("k-where", "workingLocation"),
        ("k-gmail", "fromGmail"),
    ] {
        let mut e = ev(name, &hours(-4), Some(&hours(-3)), false);
        e.kind = Some(kind);
        add(name, false, e);
    }
    let mut e = ev("k-default", &hours(-4), Some(&hours(-3)), false);
    e.kind = Some("default");
    add("k-default", true, e);

    // Guests: rooms and the user's own addresses are not guests; a room with a human is.
    let mut e = ev("room-only", &hours(-4), Some(&hours(-3)), false);
    e.guests = vec![
        guest("Room 4", "c_123@resource.calendar.google.com"),
        google_me("accepted"),
    ];
    add("room-only", false, e);
    let mut e = ev("only-me", &hours(-4), Some(&hours(-3)), false);
    e.guests = vec![
        google_me("accepted"),
        guest("Me again", "ME@drive.example.com"),
        guest("Me at work", "me@work.example.com"),
    ];
    add("only-me", false, e);
    let mut e = ev("room-and-human", &hours(-4), Some(&hours(-3)), false);
    e.guests = vec![
        guest("Room 4", "c_123@resource.calendar.google.com"),
        guest("Dee Dale", "dee@example.com"),
    ];
    add("room-and-human", true, e);
    let mut e = ev("no-guests", &hours(-4), Some(&hours(-3)), false);
    e.guests = Vec::new();
    add("no-guests", false, e);
    let mut e = ev("nameless", &hours(-4), Some(&hours(-3)), false);
    e.guests = vec![json!({ "name": "No Address" })];
    add("nameless", false, e);

    // The same occurrence on two calendars is one entry; it is declined only when every copy that
    // names the user says so.
    add(
        "two-cals",
        true,
        ev("two-cals", &hours(-4), Some(&hours(-3)), false),
    );
    let mut twin = ev("two-cals", &hours(-4), Some(&hours(-3)), false);
    twin.calendar = GOOGLE_ALT;
    put(conn, &twin);
    let mut e = ev("two-one-declined", &hours(-4), Some(&hours(-3)), false);
    e.guests.push(google_me("declined"));
    put(conn, &e);
    let mut twin = ev("two-one-declined", &hours(-4), Some(&hours(-3)), false);
    twin.calendar = GOOGLE_ALT;
    twin.guests.push(google_me("accepted"));
    add("two-one-declined", true, twin);
    let mut e = ev("two-both-declined", &hours(-4), Some(&hours(-3)), false);
    e.guests.push(google_me("declined"));
    put(conn, &e);
    let mut twin = ev("two-both-declined", &hours(-4), Some(&hours(-3)), false);
    twin.calendar = GOOGLE_ALT;
    twin.guests.push(google_me("declined"));
    add("two-both-declined", false, twin);

    // A series shares one uid: each occurrence is its own card.
    for (i, hours_ago) in [26_i64, 50].into_iter().enumerate() {
        let mut e = ev(
            "series",
            &hours(-hours_ago),
            Some(&hours(-hours_ago + 1)),
            false,
        );
        e.recurring = true;
        e.series = Some("series-master".into());
        e.title = format!("Weekly {i}");
        put(conn, &e);
    }
    owed.borrow_mut().push("series".into());
    // ...and a skipped series hides every occurrence, past and to come.
    for hours_ago in [26_i64, 50] {
        let mut e = ev(
            "series-skipped",
            &hours(-hours_ago),
            Some(&hours(-hours_ago + 1)),
            false,
        );
        e.recurring = true;
        put(conn, &e);
    }
    conn.execute(
        "INSERT INTO meet_prompt_answers(event_uid, event_start, answer) \
         VALUES ('series-skipped', '', 'series_skipped')",
        [],
    )
    .unwrap();

    // No uid: never prompted (a null one and an empty one).
    add("no-uid", false, ev("", &hours(-4), Some(&hours(-3)), false));
    let mut e = ev("empty-uid", &hours(-4), Some(&hours(-3)), false);
    e.uid = Some(String::new());
    add("empty-uid", false, e);

    // Answered: logged, skipped, and an entry already linked with no answer row.
    for (name, answer) in [("answered", "logged"), ("skipped", "skipped")] {
        let e = ev(name, &hours(-4), Some(&hours(-3)), false);
        conn.execute(
            "INSERT INTO meet_prompt_answers(event_uid, event_start, answer) VALUES (?1, ?2, ?3)",
            params![name, e.start, answer],
        )
        .unwrap();
        add(name, false, e);
    }
    let e = ev("linked", &hours(-4), Some(&hours(-3)), false);
    conn.execute(
        "INSERT INTO interactions(kind, on_date, event_uid, event_start) \
         VALUES ('met', '2026-11-26', 'linked', ?1)",
        params![e.start],
    )
    .unwrap();
    add("linked", false, e);
    // An answer for another occurrence of the same uid leaves this one owed.
    let e = ev("other-occurrence", &hours(-4), Some(&hours(-3)), false);
    conn.execute(
        "INSERT INTO meet_prompt_answers(event_uid, event_start, answer) \
         VALUES ('other-occurrence', '2026-01-01T00:00:00Z', 'skipped')",
        [],
    )
    .unwrap();
    add("other-occurrence", true, e);

    // A guest list stored packed (a row the sync has aged) still reads through text_at.
    let mut e = ev("packed", &hours(-4), Some(&hours(-3)), false);
    e.packed = true;
    add("packed", true, e);

    let all = owed.into_inner();
    all.into_iter().collect()
}

#[test]
fn the_owed_table_holds_at_a_pinned_clock() {
    let (_d, conn) = store();
    calendars(&conn);
    let z = zone("UTC");
    let want = table(&conn, pinned(), z);
    assert_eq!(owed_uids(&conn, pinned(), z), want);
    // The series is two cards, one per occurrence.
    let series: Vec<OwedMeeting> = owed_meetings_at(&conn, pinned(), z)
        .unwrap()
        .into_iter()
        .filter(|m| m.event_ical == "series")
        .collect();
    assert_eq!(series.len(), 2);
    assert!(series.iter().all(|m| m.recurs));
    assert!(series[0].start > series[1].start, "newest first");
    // The same occurrence on two calendars is one card.
    let two = owed_meetings_at(&conn, pinned(), z)
        .unwrap()
        .into_iter()
        .filter(|m| m.event_ical == "two-cals")
        .count();
    assert_eq!(two, 1);
}

#[test]
fn the_owed_table_holds_in_every_zone() {
    // Each zone has its own civil today, so each gets its own store: the all-day rows are built
    // from that zone's calendar.
    let mut zones: Vec<Tz> = (-12..=14).map(gmt).collect();
    for name in [
        "Pacific/Auckland",
        "Asia/Kolkata",
        "Pacific/Kiritimati",
        "America/New_York",
        "America/Los_Angeles",
        "Europe/London",
    ] {
        zones.push(zone(name));
    }
    // Instants either side of midnight in the busiest offsets, as well as the pinned noon.
    let clocks = [
        pinned(),
        Utc.with_ymd_and_hms(2026, 11, 26, 0, 5, 0).unwrap(),
        Utc.with_ymd_and_hms(2026, 11, 26, 23, 55, 0).unwrap(),
    ];
    for z in zones {
        for now in clocks {
            let (_d, conn) = store();
            calendars(&conn);
            let want = table(&conn, now, z);
            assert_eq!(
                owed_uids(&conn, now, z),
                want,
                "zone {} at {}",
                z.name(),
                iso(now)
            );
        }
    }
}

#[test]
fn an_all_day_event_ends_at_the_users_midnight_not_utc() {
    let (_d, conn) = store();
    calendars(&conn);
    // 23:30 UTC on the 26th is already the 27th in UTC+2, and still the 26th in UTC.
    let now = Utc.with_ymd_and_hms(2026, 11, 26, 23, 30, 0).unwrap();
    put(&conn, &ev("day", "2026-11-26", Some("2026-11-27"), true));
    assert!(owed_uids(&conn, now, gmt(2)).contains("day"));
    assert!(!owed_uids(&conn, now, zone("UTC")).contains("day"));
    assert!(!owed_uids(&conn, now, gmt(-5)).contains("day"));
}

#[test]
fn an_all_day_event_in_a_zone_that_skips_midnight_still_sorts_by_its_day() {
    let (_d, conn) = store();
    calendars(&conn);
    // Havana's clocks jump from 00:00 to 01:00 on 8 March 2026, so that midnight never exists.
    let now = Utc.with_ymd_and_hms(2026, 3, 10, 12, 0, 0).unwrap();
    put(&conn, &ev("early", "2026-03-05", Some("2026-03-06"), true));
    put(&conn, &ev("gap", "2026-03-07", Some("2026-03-08"), true));
    let order: Vec<String> = owed_meetings_at(&conn, now, zone("America/Havana"))
        .unwrap()
        .into_iter()
        .map(|m| m.event_ical)
        .collect();
    assert_eq!(
        order,
        ["gap", "early"],
        "newest first, not the 1970 epoch last"
    );
}

// --- the guests -------------------------------------------------------------------------------------

fn only(conn: &Connection, uid: &str) -> OwedMeeting {
    owed_meetings_at(conn, pinned(), zone("UTC"))
        .unwrap()
        .into_iter()
        .find(|m| m.event_ical == uid)
        .expect("owed")
}

fn ago(h: i64) -> String {
    iso(pinned() - Duration::hours(h))
}

#[test]
fn only_known_guests_are_pre_ticked() {
    let (_d, conn) = store();
    calendars(&conn);
    let mut d = draft("Ann", "Able");
    d.details = vec![detail("email", "Ann@Example.com")];
    let ann = create_person(&conn, &d).unwrap();
    let mut e = ev("m1", &ago(4), Some(&ago(3)), false);
    e.guests = vec![
        guest("Ann A.", "ANN@example.com"),
        guest("Newt Nobody", "newt@example.com"),
        guest("Room", "r@resource.calendar.google.com"),
        google_me("accepted"),
    ];
    put(&conn, &e);
    let m = only(&conn, "m1");
    assert_eq!(m.known.len(), 1);
    assert_eq!(m.known[0].id, ann);
    assert_eq!(m.known[0].display_name, "Ann Able");
    assert_eq!(m.unknown.len(), 1);
    assert_eq!(m.unknown[0].name, "Newt Nobody");
    assert_eq!(m.unknown[0].email, "newt@example.com");
    assert_eq!(m.unknown[0].trashed_id, None);
    assert_eq!(m.title, "Meeting m1");
    assert!(!m.all_day && !m.recurs);
}

#[test]
fn a_person_invited_twice_is_one_tick() {
    let (_d, conn) = store();
    calendars(&conn);
    let mut d = draft("Ann", "Able");
    d.details = vec![
        detail("email", "ann@example.com"),
        detail("email", "ann@work.example.com"),
    ];
    create_person(&conn, &d).unwrap();
    let mut e = ev("m1", &ago(4), Some(&ago(3)), false);
    e.guests = vec![
        guest("Ann", "ann@example.com"),
        guest("Ann", "ann@work.example.com"),
        guest("Ann again", "ANN@example.com"),
    ];
    put(&conn, &e);
    let m = only(&conn, "m1");
    assert_eq!((m.known.len(), m.unknown.len()), (1, 0));
}

#[test]
fn a_guest_in_trash_is_offered_unticked_and_ticking_restores_them() {
    let (_d, conn) = store();
    calendars(&conn);
    let mut d = draft("Tess", "Trashed");
    d.details = vec![detail("email", "tess@example.com")];
    let tess = create_person(&conn, &d).unwrap();
    trash_people(&conn, &[tess], &[]).unwrap();
    let mut e = ev("m1", &ago(4), Some(&ago(3)), false);
    e.guests = vec![guest("Tess T.", "tess@example.com")];
    put(&conn, &e);

    let m = only(&conn, "m1");
    assert!(m.known.is_empty(), "never pre-ticked");
    assert_eq!(m.unknown.len(), 1);
    assert_eq!(m.unknown[0].trashed_id, Some(tess));

    let done = answer_meeting(
        &conn,
        "m1",
        &e.start,
        &[PersonRef::Existing { id: tess }],
        MeetingAnswer::Logged,
        None,
        None,
    )
    .unwrap();
    assert_eq!((done.created, done.restored), (0, 1));
    let back = get_person(&conn, tess).unwrap();
    assert!(back.trashed_at.is_none());
    assert_eq!(back.timeline.entries.len(), 1);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM people"), 1);
}

#[test]
fn ticking_a_new_guest_whose_address_is_in_trash_restores_that_person_instead() {
    let (_d, conn) = store();
    calendars(&conn);
    let mut d = draft("Tess", "Trashed");
    d.details = vec![detail("email", "tess@example.com")];
    let tess = create_person(&conn, &d).unwrap();
    trash_people(&conn, &[tess], &[]).unwrap();
    let done = answer_meeting(
        &conn,
        "m1",
        &ago(4),
        &[PersonRef::Guest {
            new: NewGuest {
                name: "Tess T.".into(),
                email: "TESS@example.com".into(),
            },
        }],
        MeetingAnswer::Logged,
        None,
        Some("Sync"),
    )
    .unwrap();
    assert_eq!((done.created, done.restored), (0, 1));
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM people"), 1);
    assert!(get_person(&conn, tess).unwrap().trashed_at.is_none());
}

#[test]
fn an_events_guests_are_sorted_the_way_the_owed_list_sorts_them() {
    let (_d, conn) = store();
    calendars(&conn);
    let mut d = draft("Ann", "Able");
    d.details = vec![detail("email", "Ann@Example.com")];
    let ann = create_person(&conn, &d).unwrap();
    let mut d = draft("Tess", "Trashed");
    d.details = vec![detail("email", "tess@example.com")];
    let tess = create_person(&conn, &d).unwrap();
    trash_people(&conn, &[tess], &[]).unwrap();
    let list: Vec<Attendee> = serde_json::from_value(json!([
        guest("Ann A.", "ANN@example.com"),
        guest("Ann again", "ann@example.com"),
        guest("Newt Nobody", "newt@example.com"),
        guest("Tess T.", "tess@example.com"),
        guest("Room", "r@resource.calendar.google.com"),
        // The user, by their own address (no `self` flag, as an Outlook invite has none) and by it.
        guest("Me", "ME@example.com"),
        google_me("accepted"),
        json!({ "name": "No Address" }),
    ]))
    .unwrap();
    let got = event_guests(&conn, &list).unwrap();
    assert_eq!(got.known.len(), 1);
    assert_eq!(
        (got.known[0].id, got.known[0].email.as_str()),
        (ann, "ANN@example.com")
    );
    let unknown: Vec<(&str, Option<i64>)> = got
        .unknown
        .iter()
        .map(|g| (g.email.as_str(), g.trashed_id))
        .collect();
    assert_eq!(
        unknown,
        [("newt@example.com", None), ("tess@example.com", Some(tess))]
    );
    // An event with no guests is a card with nobody on it, which "Someone else…" fills.
    let none = event_guests(&conn, &[]).unwrap();
    assert!(none.known.is_empty() && none.unknown.is_empty());
}

// --- answering --------------------------------------------------------------------------------------

fn answer_row(conn: &Connection, uid: &str, start: &str) -> Option<String> {
    conn.query_row(
        "SELECT answer FROM meet_prompt_answers WHERE event_uid = ?1 AND event_start = ?2",
        params![uid, start],
        |r| r.get(0),
    )
    .ok()
}

#[test]
fn saving_logs_one_entry_for_everyone_and_makes_the_new_guests_people() {
    let (_d, conn) = store();
    calendars(&conn);
    let ann = person(&conn, "Ann", "Able");
    let e = ev("m1", &ago(4), Some(&ago(3)), false);
    put(&conn, &e);
    let done = answer_meeting(
        &conn,
        "m1",
        &e.start,
        &[
            PersonRef::Existing { id: ann },
            PersonRef::Guest {
                new: NewGuest {
                    name: "Newt Nobody".into(),
                    email: "newt@example.com".into(),
                },
            },
            PersonRef::Guest {
                new: NewGuest {
                    name: "Newt again".into(),
                    email: "NEWT@example.com".into(),
                },
            },
            PersonRef::Existing { id: ann },
        ],
        MeetingAnswer::Logged,
        Some("Went well"),
        None,
    )
    .unwrap();
    assert_eq!((done.created, done.restored), (1, 0));
    assert_eq!(
        done.interaction,
        Some(count(&conn, "SELECT id FROM interactions"))
    );
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM people"), 2);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM interactions"), 1);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM interaction_people"), 2);
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM person_sources"),
        0,
        "a new guest has no Google row"
    );
    assert_eq!(answer_row(&conn, "m1", &e.start).as_deref(), Some("logged"));
    // The entry is linked, titled from the mirror and dated on the user's day.
    let page = list_timeline(&conn, ann, None, 5).unwrap();
    let entry = &page.entries[0];
    assert_eq!(entry.kind, "met");
    assert_eq!(entry.on_date, "2026-11-26");
    assert_eq!(entry.note, "Went well");
    assert_eq!(entry.event_ical.as_deref(), Some("m1"));
    assert_eq!(entry.event_start.as_deref(), Some(e.start.as_str()));
    assert_eq!(entry.event_title.as_deref(), Some("Meeting m1"));
    assert_eq!(entry.at.as_deref(), Some(e.start.as_str()));
    assert_eq!(entry.others.len(), 1);
    // The new person has their address, and the meeting is no longer owed.
    assert_eq!(
        text(
            &conn,
            "SELECT value FROM person_details WHERE kind = 'email'"
        ),
        "newt@example.com"
    );
    assert!(!owed_uids(&conn, pinned(), zone("UTC")).contains("m1"));
}

#[test]
fn a_blank_title_falls_back_to_the_mirrors_and_a_long_skip_start_is_refused() {
    let (_d, conn) = store();
    calendars(&conn);
    let e = ev("m1", &ago(4), Some(&ago(3)), false);
    put(&conn, &e);
    let ann = person(&conn, "Ann", "Able");
    answer_meeting(
        &conn,
        "m1",
        &e.start,
        &[PersonRef::Existing { id: ann }],
        MeetingAnswer::Logged,
        None,
        Some("  "),
    )
    .unwrap();
    let page = list_timeline(&conn, ann, None, 5).unwrap();
    assert_eq!(page.entries[0].event_title.as_deref(), Some("Meeting m1"));
    // A skip's start is bounded like a logged entry's.
    let long = "2".repeat(65);
    assert!(answer_meeting(&conn, "m1", &long, &[], MeetingAnswer::Skipped, None, None).is_err());
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM meet_prompt_answers WHERE event_start = ''"
        ),
        0
    );
}

#[test]
fn skipping_and_skipping_a_series_write_their_answers() {
    let (_d, conn) = store();
    calendars(&conn);
    let a = ev("a", &ago(4), Some(&ago(3)), false);
    let b = ev("b", &ago(4), Some(&ago(3)), false);
    put(&conn, &a);
    put(&conn, &b);
    answer_meeting(
        &conn,
        "a",
        &a.start,
        &[],
        MeetingAnswer::Skipped,
        None,
        None,
    )
    .unwrap();
    answer_meeting(
        &conn,
        "b",
        &b.start,
        &[],
        MeetingAnswer::SeriesSkipped,
        None,
        None,
    )
    .unwrap();
    assert_eq!(answer_row(&conn, "a", &a.start).as_deref(), Some("skipped"));
    assert_eq!(
        answer_row(&conn, "b", "").as_deref(),
        Some("series_skipped")
    );
    assert!(owed_uids(&conn, pinned(), zone("UTC")).is_empty());
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM interactions"), 0);
    // Without an iCal id there is nothing to remember an answer against.
    assert!(answer_meeting(&conn, "", &a.start, &[], MeetingAnswer::Skipped, None, None).is_err());
    assert!(answer_meeting(&conn, "", "", &[], MeetingAnswer::SeriesSkipped, None, None).is_err());
}

#[test]
fn logging_a_skipped_meeting_replaces_the_skip() {
    let (_d, conn) = store();
    calendars(&conn);
    let ann = person(&conn, "Ann", "Able");
    let e = ev("m1", &ago(4), Some(&ago(3)), false);
    put(&conn, &e);
    answer_meeting(
        &conn,
        "m1",
        &e.start,
        &[],
        MeetingAnswer::Skipped,
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        answer_row(&conn, "m1", &e.start).as_deref(),
        Some("skipped")
    );

    answer_meeting(
        &conn,
        "m1",
        &e.start,
        &[PersonRef::Existing { id: ann }],
        MeetingAnswer::Logged,
        None,
        None,
    )
    .unwrap();
    assert_eq!(answer_row(&conn, "m1", &e.start).as_deref(), Some("logged"));
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM meet_prompt_answers"), 1);

    // A later skip (the card opened again) never turns a logged meeting back into a skipped one.
    answer_meeting(
        &conn,
        "m1",
        &e.start,
        &[],
        MeetingAnswer::Skipped,
        None,
        None,
    )
    .unwrap();
    assert_eq!(answer_row(&conn, "m1", &e.start).as_deref(), Some("logged"));
}

#[test]
fn an_event_with_no_uid_logs_and_writes_no_answer_row() {
    let (_d, conn) = store();
    calendars(&conn);
    let ann = person(&conn, "Ann", "Able");
    let done = answer_meeting(
        &conn,
        "",
        &ago(4),
        &[PersonRef::Existing { id: ann }],
        MeetingAnswer::Logged,
        None,
        Some("Coffee with a feed event"),
    )
    .unwrap();
    assert!(done.interaction.is_some());
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM meet_prompt_answers"), 0);
    let entry = &list_timeline(&conn, ann, None, 5).unwrap().entries[0];
    assert_eq!(
        entry.event_title.as_deref(),
        Some("Coffee with a feed event")
    );
    assert_eq!(entry.event_ical, None, "no id, so no link");
    assert_eq!(entry.event_start, None);
}

#[test]
fn the_entry_is_dated_on_the_users_day_not_utc() {
    let (_d, conn) = store();
    calendars(&conn);
    crate::db::set_setting(&conn, crate::settings::TIME_ZONE_KEY, "Etc/GMT-2").unwrap();
    let ann = person(&conn, "Ann", "Able");
    // 23:30 UTC on the 25th is 01:30 on the 26th in UTC+2.
    answer_meeting(
        &conn,
        "late",
        "2026-11-25T23:30:00Z",
        &[PersonRef::Existing { id: ann }],
        MeetingAnswer::Logged,
        None,
        Some("Late"),
    )
    .unwrap();
    answer_meeting(
        &conn,
        "allday",
        "2026-11-20",
        &[PersonRef::Existing { id: ann }],
        MeetingAnswer::Logged,
        None,
        Some("Offsite"),
    )
    .unwrap();
    let page = list_timeline(&conn, ann, None, 5).unwrap();
    let dates: Vec<(&str, Option<&str>)> = page
        .entries
        .iter()
        .map(|e| (e.on_date.as_str(), e.at.as_deref()))
        .collect();
    assert_eq!(
        dates,
        [
            ("2026-11-26", Some("2026-11-25T23:30:00Z")),
            ("2026-11-20", None)
        ]
    );
}

#[test]
fn a_failed_answer_writes_nothing() {
    let (_d, conn) = store();
    calendars(&conn);
    let sentinel = "SENTINEL-GUEST-ZQX";
    let e = ev("m1", &ago(4), Some(&ago(3)), false);
    put(&conn, &e);
    let result = answer_meeting(
        &conn,
        "m1",
        &e.start,
        &[
            PersonRef::Guest {
                new: NewGuest {
                    name: sentinel.into(),
                    email: "sentinel@example.com".into(),
                },
            },
            PersonRef::Existing { id: 424_242 },
        ],
        MeetingAnswer::Logged,
        Some(sentinel),
        None,
    );
    let err = result.expect_err("refused").to_string();
    assert!(!err.contains(sentinel), "an error quoted a guest: {err}");
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM people"), 0);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM interactions"), 0);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM meet_prompt_answers"), 0);
    // A log with nobody ticked is refused too: Skip is the way to say nobody.
    assert!(answer_meeting(
        &conn,
        "m1",
        &e.start,
        &[],
        MeetingAnswer::Logged,
        None,
        None
    )
    .is_err());
    let blank = PersonRef::Guest {
        new: NewGuest::default(),
    };
    assert!(answer_meeting(
        &conn,
        "m1",
        &e.start,
        &[blank],
        MeetingAnswer::Logged,
        None,
        None
    )
    .is_err());
}

#[test]
fn the_wire_shapes_match_the_spec() {
    let words = [
        MeetingAnswer::Logged,
        MeetingAnswer::Skipped,
        MeetingAnswer::SeriesSkipped,
    ]
    .map(MeetingAnswer::word);
    assert_eq!(words, PROMPT_ANSWERS);
    for (a, word) in [
        (MeetingAnswer::Logged, "logged"),
        (MeetingAnswer::Skipped, "skipped"),
        (MeetingAnswer::SeriesSkipped, "series_skipped"),
    ] {
        let parsed: MeetingAnswer = serde_json::from_value(json!(word)).unwrap();
        assert_eq!(parsed, a);
    }
    let refs: Vec<PersonRef> = serde_json::from_value(json!([
        { "id": 7 },
        { "new": { "name": "Newt Nobody", "email": "newt@example.com" } },
        { "new": { "email": "only@example.com" } }
    ]))
    .unwrap();
    assert!(matches!(refs[0], PersonRef::Existing { id: 7 }));
    assert!(matches!(&refs[1], PersonRef::Guest { new } if new.name == "Newt Nobody"));
    assert!(matches!(&refs[2], PersonRef::Guest { new } if new.name.is_empty()));
    let done = serde_json::to_value(MeetingAnswered::default()).unwrap();
    assert_eq!(
        done,
        json!({ "interaction": null, "created": 0, "restored": 0 })
    );
}

#[test]
fn an_owed_meeting_serialises_with_the_spec_names() {
    let (_d, conn) = store();
    calendars(&conn);
    put(&conn, &ev("m1", &ago(4), Some(&ago(3)), false));
    let wire = serde_json::to_value(only(&conn, "m1")).unwrap();
    for key in [
        "uid", "start", "end", "title", "all_day", "recurs", "known", "unknown",
    ] {
        assert!(wire.get(key).is_some(), "missing {key}");
    }
}
