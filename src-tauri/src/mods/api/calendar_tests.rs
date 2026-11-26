// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `calendar.agenda` through the real dispatcher over a real store (PR C3): who may ask, the seven
//! fields and nothing else, a uid that is null rather than PM's row id, quiet calendars and declined
//! events left out, and the argument ranges.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::calendar_calls::{event_dto, Query};
use super::call_fixtures::*;
use super::projection::Projection;
use super::test_support::TestHost;
use super::Code;
use crate::calendar::{AgendaEvent, CalendarEvent};

const CAL_MOD: &str = "atlas.agenda";
const READ: &[&str] = &["calendar.read"];

/// Text no event may ever show: it sits in every column the agenda must not read.
const MARKER: &str = "zz-marker@example.invalid";

/// A calendar and its account, both carrying `address` (the account's own email is what a calendar
/// id embeds).
fn seed_calendar(host: &TestHost, id: &str, address: &str, quiet: bool) {
    host.sql(|c| {
        crate::calendar::upsert_source(
            c,
            &format!("gcal:{address}"),
            "google",
            Some(address),
            address,
        )
        .unwrap();
        c.execute(
            "INSERT INTO calendars(id, source_id, provider, remote_id, name, quiet) \
             VALUES (?1, ?2, 'google', ?3, 'Work', ?4)",
            rusqlite::params![id, format!("gcal:{address}"), address, quiet as i64],
        )
        .unwrap();
    });
}

/// One event `days` from today at noon, 30 minutes long, with `uid` (or none).
fn seed_event(
    host: &TestHost,
    id: &str,
    calendar: &str,
    summary: &str,
    days: i64,
    uid: Option<&str>,
) {
    host.sql(|c| {
        c.execute(
            "INSERT INTO calendar_events(id, calendar_id, summary, start, end, uid) \
             VALUES (?1, ?2, ?3, datetime(date('now'), ?4, '+12 hours'), \
                     datetime(date('now'), ?4, '+12 hours', '+30 minutes'), ?5)",
            rusqlite::params![id, calendar, summary, format!("{days:+} days"), uid],
        )
        .unwrap();
    });
}

async fn agenda(host: &TestHost, s: &str, args: Value) -> Value {
    value(call(host, s, "calendar.agenda", args).await)
}

fn titles(v: &Value) -> Vec<String> {
    v["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["title"].as_str().unwrap().to_owned())
        .collect()
}

// --- who may ask --------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agenda_is_empty_without_calendar_read() {
    // A mod with other reads but not `calendar.read` is refused, and the refusal carries nothing.
    let (host, s) = first(CAL_MOD, &["tasks.read", "projects.read"]);
    seed_calendar(&host, "cal-1", "me@example.com", false);
    seed_event(
        &host,
        "e1",
        "cal-1",
        "Board meeting",
        1,
        Some("u1@example.com"),
    );
    let reply = call(&host, &s, "calendar.agenda", json!({})).await;
    let e = error(reply);
    assert_eq!(e.code, Code::NotGranted);
    let said = serde_json::to_string(&e).unwrap();
    assert!(!said.contains("Board meeting") && !said.contains("u1@example.com"));
}

// --- what an event tells ------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agenda_dto_has_exactly_the_seven_fields() {
    let (host, s) = first(CAL_MOD, READ);
    seed_calendar(&host, "cal-1", "me@example.com", false);
    seed_event(&host, "e1", "cal-1", "Standup", 1, Some("u1@example.com"));
    host.sql(|c| {
        c.execute(
            "UPDATE calendar_events SET location = 'Room 4', recurring = 1 WHERE id = 'e1'",
            [],
        )
        .unwrap()
    });
    let v = agenda(&host, &s, json!({})).await;
    let events = v["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    let keys: BTreeSet<&str> = events[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from([
            "uid",
            "title",
            "start",
            "end",
            "allDay",
            "location",
            "recurring"
        ])
    );
    let e = &events[0];
    assert_eq!(e["uid"], "u1@example.com");
    assert_eq!(e["title"], "Standup");
    assert_eq!(e["location"], "Room 4");
    assert_eq!(e["allDay"], false);
    assert_eq!(e["recurring"], true);
    assert!(e["start"].is_string() && e["end"].is_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agenda_never_carries_an_email() {
    // Every column the agenda must not read holds the marker: the guests, the organiser, the
    // description, both links, the calendar's and the account's addresses, and the row ids that embed
    // them. (A Google iCalUID legitimately holds an `@`, so the test looks for the marker, not an `@`.)
    let (host, s) = first(CAL_MOD, &["calendar.read", "projects.read", "tasks.read"]);
    let calendar = format!("gcal:{MARKER}:primary");
    seed_calendar(&host, &calendar, MARKER, false);
    seed_event(
        &host,
        &format!("{calendar}:e1"),
        &calendar,
        "Review",
        1,
        None,
    );
    host.sql(|c| {
        c.execute(
            "UPDATE calendar_events SET description = ?1, html_link = ?1, \
                    attendees = ?2, organizer = ?1, conference_url = ?1 WHERE calendar_id = ?3",
            rusqlite::params![
                format!("https://{MARKER}"),
                format!(r#"[{{"email":"{MARKER}","response":"accepted","self":false}}]"#),
                calendar,
            ],
        )
        .unwrap()
    });
    let v = agenda(&host, &s, json!({})).await;
    assert_eq!(titles(&v), ["Review"]);
    assert!(
        !v.to_string().contains(MARKER),
        "the agenda leaked what it must not read: {v}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agenda_uid_is_null_when_the_event_has_none() {
    let (host, s) = first(CAL_MOD, READ);
    seed_calendar(&host, "cal-1", "me@example.com", false);
    seed_event(&host, "cal-1:no-uid", "cal-1", "No uid", 1, None);
    seed_event(&host, "cal-1:blank-uid", "cal-1", "Blank uid", 2, Some(""));
    let v = agenda(&host, &s, json!({})).await;
    assert_eq!(titles(&v), ["No uid", "Blank uid"]);
    for e in v["events"].as_array().unwrap() {
        // Null, and never the row's own id (which embeds the calendar and account).
        assert_eq!(e["uid"], Value::Null, "{e}");
        assert!(!e.to_string().contains("cal-1"), "{e}");
    }
}

// --- what is left out ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agenda_leaves_out_quiet_calendars() {
    let (host, s) = first(CAL_MOD, READ);
    seed_calendar(&host, "cal-loud", "me@example.com", false);
    seed_calendar(&host, "cal-quiet", "quiet@example.com", true);
    seed_event(&host, "e1", "cal-loud", "Loud", 1, Some("u1"));
    seed_event(&host, "e2", "cal-quiet", "Quiet", 1, Some("u2"));
    let v = agenda(&host, &s, json!({})).await;
    assert_eq!(titles(&v), ["Loud"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agenda_leaves_out_declined_events() {
    let (host, s) = first(CAL_MOD, READ);
    seed_calendar(&host, "cal-me", "me@example.com", false);
    seed_event(&host, "going", "cal-me", "Going", 1, Some("u1"));
    seed_event(&host, "declined", "cal-me", "Declined", 2, Some("u2"));
    seed_event(&host, "unanswered", "cal-me", "Unanswered", 3, Some("u3"));
    let guest = |response: &str| {
        format!(r#"[{{"email":"me@example.com","response":"{response}","self":true}}]"#)
    };
    host.sql(|c| {
        for (id, response) in [("going", "accepted"), ("declined", "declined")] {
            c.execute(
                "UPDATE calendar_events SET attendees = ?2 WHERE id = ?1",
                rusqlite::params![id, guest(response)],
            )
            .unwrap();
        }
    });
    let v = agenda(&host, &s, json!({})).await;
    assert_eq!(titles(&v), ["Going", "Unanswered"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agenda_is_what_has_not_ended_within_the_days_asked_soonest_first() {
    let (host, s) = first(CAL_MOD, READ);
    seed_calendar(&host, "cal-1", "me@example.com", false);
    seed_event(&host, "late", "cal-1", "In a week", 7, Some("u3"));
    seed_event(&host, "soon", "cal-1", "Tomorrow", 1, Some("u1"));
    seed_event(&host, "past", "cal-1", "Last week", -7, Some("u0"));
    seed_event(&host, "far", "cal-1", "In a month", 30, Some("u4"));
    // The default is three weeks.
    assert_eq!(
        titles(&agenda(&host, &s, json!({})).await),
        ["Tomorrow", "In a week"]
    );
    assert_eq!(
        titles(&agenda(&host, &s, json!({"days": 3})).await),
        ["Tomorrow"]
    );
    assert_eq!(
        titles(&agenda(&host, &s, json!({"days": 60})).await),
        ["Tomorrow", "In a week", "In a month"]
    );
    assert_eq!(
        titles(&agenda(&host, &s, json!({"limit": 1})).await),
        ["Tomorrow"]
    );
}

/// The query's LIMIT runs before the declined filter, so a declined series ahead of a real event must
/// not eat the page: the call asks for the most rows a page can be and cuts after the filter.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agenda_page_is_not_spent_on_declined_events_ahead_of_a_real_one() {
    let (host, s) = first(CAL_MOD, READ);
    seed_calendar(&host, "cal-me", "me@example.com", false);
    for day in 1..=6 {
        seed_event(
            &host,
            &format!("d{day}"),
            "cal-me",
            "Daily",
            day,
            Some(&format!("ud{day}")),
        );
    }
    seed_event(&host, "real1", "cal-me", "Real one", 10, Some("ur1"));
    seed_event(&host, "real2", "cal-me", "Real two", 11, Some("ur2"));
    seed_event(&host, "real3", "cal-me", "Real three", 12, Some("ur3"));
    host.sql(|c| {
        c.execute(
            "UPDATE calendar_events SET attendees = \
             '[{\"email\":\"me@example.com\",\"response\":\"declined\",\"self\":true}]' \
             WHERE id LIKE 'd%'",
            [],
        )
        .unwrap();
    });
    // Three asked for, three real events behind six declined ones.
    assert_eq!(
        titles(&agenda(&host, &s, json!({"limit": 3})).await),
        ["Real one", "Real two", "Real three"]
    );
    // And `limit` still caps what comes back.
    assert_eq!(
        titles(&agenda(&host, &s, json!({"limit": 2})).await),
        ["Real one", "Real two"]
    );
}

/// A feed controls an event's title and location: both are cut, so a page of them can't outgrow the
/// result cap and fail the call for every mod.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agenda_cuts_a_long_title_and_location() {
    let (host, s) = first(CAL_MOD, READ);
    seed_calendar(&host, "cal-me", "me@example.com", false);
    seed_event(&host, "e1", "cal-me", &"é".repeat(40_000), 1, Some("u1"));
    host.sql(|c| {
        c.execute(
            "UPDATE calendar_events SET location = ?1",
            rusqlite::params!["é".repeat(40_000)],
        )
        .unwrap();
    });
    let v = agenda(&host, &s, json!({})).await;
    let event = &v["events"][0];
    for field in ["title", "location"] {
        assert_eq!(
            event[field].as_str().unwrap().chars().count(),
            super::calendar_calls::TEXT_MAX_CHARS,
            "{field}"
        );
    }
}

/// The start and the end are text a store's writer controls as well: a hostile one is cut, so it can't
/// make the call fail with "too large" for everyone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agenda_cuts_a_hostile_start_and_end() {
    let (host, s) = first(CAL_MOD, READ);
    seed_calendar(&host, "cal-me", "me@example.com", false);
    seed_event(&host, "e1", "cal-me", "Hostile", 1, Some("u1"));
    // Trailing spaces still parse as a date, so the row stays in the agenda's window.
    host.sql(|c| {
        c.execute(
            "UPDATE calendar_events SET start = start || ?1, end = end || ?1",
            rusqlite::params![" ".repeat(2 << 20)],
        )
        .unwrap();
    });
    let v = agenda(&host, &s, json!({})).await;
    let event = &v["events"][0];
    for field in ["start", "end"] {
        assert_eq!(
            event[field].as_str().unwrap().chars().count(),
            super::calendar_calls::TIME_MAX_CHARS,
            "{field}"
        );
    }
}

/// The uid comes uncut from the feed too: a hostile 40,000-character one is sent as no uid, and a
/// normal one (even at the cap) is kept whole.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agenda_drops_an_oversized_uid() {
    let (host, s) = first(CAL_MOD, READ);
    seed_calendar(&host, "cal-me", "me@example.com", false);
    let at_cap = "u".repeat(super::calendar_calls::UID_MAX_CHARS);
    seed_event(
        &host,
        "e1",
        "cal-me",
        "Long uid",
        1,
        Some(&"é".repeat(40_000)),
    );
    seed_event(&host, "e2", "cal-me", "At the cap", 2, Some(&at_cap));
    let v = agenda(&host, &s, json!({})).await;
    let events = v["events"].as_array().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events[0]["uid"].is_null(), "{}", events[0]);
    assert_eq!(events[1]["uid"].as_str(), Some(at_cap.as_str()));
}

// --- the arguments ------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agenda_days_and_limit_are_range_checked() {
    let (host, s) = first(CAL_MOD, READ);
    for args in [
        json!({"days": 0}),
        json!({"days": 61}),
        json!({"days": -1}),
        json!({"days": 1.5}),
        json!({"days": "7"}),
        json!({"limit": 0}),
        json!({"limit": 251}),
        json!({"limit": -3}),
        json!({"limit": 2.5}),
        json!({"limit": null, "days": [1]}),
        json!({"horizon": 5}),
        json!([]),
    ] {
        assert_eq!(
            code(call(&host, &s, "calendar.agenda", args.clone()).await),
            Code::BadArgs,
            "{args}"
        );
    }
    // The edges are fine.
    for args in [
        json!({}),
        json!(null),
        json!({"days": 1, "limit": 1}),
        json!({"days": 60, "limit": 250}),
    ] {
        value(call(&host, &s, "calendar.agenda", args).await);
    }
    assert_eq!(
        Query::parse(&json!({})).unwrap(),
        Query {
            days: 21,
            limit: 50
        }
    );
}

// --- the builder the MCP server reuses ----------------------------------------------------------

#[test]
fn the_event_builder_takes_a_projection_and_reads_no_other_field() {
    let event = AgendaEvent {
        event: CalendarEvent {
            id: format!("cal:{MARKER}:1"),
            calendar_id: format!("cal:{MARKER}"),
            summary: "Lunch".into(),
            description: Some(MARKER.into()),
            html_link: Some(MARKER.into()),
            organizer: Some(MARKER.into()),
            conference_url: Some(MARKER.into()),
            start: "2026-12-01T12:00:00Z".into(),
            uid: Some("u@example.com".into()),
            ..Default::default()
        },
        ended: false,
        declined: false,
        description_cut: false,
    };
    // The same fields whatever the projection: an event is `calendar.read`'s whole, and the other axes
    // (projects) change nothing here.
    for projection in [Projection::NONE, Projection::scoped(true, true)] {
        let dto = event_dto(&event, &projection);
        assert!(!dto.to_string().contains(MARKER), "{dto}");
        assert_eq!(dto["end"], Value::Null);
        assert_eq!(dto["title"], "Lunch");
    }
}
