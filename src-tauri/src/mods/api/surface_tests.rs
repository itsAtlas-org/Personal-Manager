// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `secrets.set` and `notify` through the dispatcher over a real store: what the page is asked to
//! draw and with what, every refusal, the per-session ask limit and the per-day notice limit (and
//! that each is Rust's, counted in one step), the local date the day is judged by, and that a call
//! that stops being the mod's counts nothing.

use rusqlite::params;
use serde_json::{json, Value};

use super::test_support::{insert_mod, live_session, serve, ModRow, TestHost};
use super::*;
use crate::mods::policy::MOD_API_VERSION;
use crate::mods::secrets;

const MOD: &str = "acme.rates";
const HOST: &str = "api.example.com";
// A test value, never a credential (gitleaks reads this file).
const KEY: &str = "typed-value-9";

fn manifest(permissions: &[&str]) -> String {
    json!({
        "permissions": permissions.iter().map(|p| json!({"id": p, "why": "test"})).collect::<Vec<_>>(),
        "data": {"keys": []},
        "entry": {"page": "app/index.html"},
        "internetHosts": [HOST],
        "secrets": [
            {"name": "token", "host": HOST, "header": "X-Api-Key"},
            {"name": "other", "host": HOST, "header": "Authorization"},
        ],
    })
    .to_string()
}

/// A mod of `tier` declaring `internet` and `notify`, both granted, with a live session.
fn setup_tier(tier: &'static str) -> (TestHost, String) {
    let host = TestHost::new();
    host.sql(|c| {
        insert_mod(c, &ModRow::signed(MOD, tier, &["internet", "notify"]));
        c.execute(
            "UPDATE mods SET manifest = ?2 WHERE id = ?1",
            params![MOD, manifest(&["internet", "notify"])],
        )
        .unwrap();
        for permission in ["internet", "notify"] {
            c.execute(
                "INSERT INTO mod_grants(mod_id, permission, decision) \
                 VALUES (?1, ?2, 'granted')",
                params![MOD, permission],
            )
            .unwrap();
        }
    });
    serve(&host, MOD);
    let session = live_session(&host, MOD, crate::mods::outbox::testing::discarding());
    (host, session)
}

fn setup() -> (TestHost, String) {
    setup_tier("second")
}

fn frame(session: &str) -> Caller {
    Caller::Frame {
        window: MAIN_LABEL.into(),
        session: session.into(),
    }
}

async fn key_slots_set(host: &TestHost, session: &str, args: Value) -> Reply {
    dispatch(host, frame(session), "secrets.set", &args.to_string()).await
}

async fn notify(host: &TestHost, session: &str, args: Value) -> Reply {
    dispatch(host, frame(session), "notify", &args.to_string()).await
}

fn page(reply: Reply) -> (PageAction, Value) {
    match reply {
        Reply::Page { action, args } => (action, args),
        other => panic!("not a page action: {other:?}"),
    }
}

fn error(reply: Reply) -> ModError {
    match reply {
        Reply::Error { error } => error,
        other => panic!("not a refusal: {other:?}"),
    }
}

fn code(reply: Reply) -> Code {
    error(reply).code
}

fn store_key(host: &TestHost, name: &str) {
    let mut conn = host.store().conn().unwrap();
    secrets::save(&mut conn, MOD, name, KEY).unwrap();
}

/// `(notify_day, notify_count)` of the mod's row.
fn counter(host: &TestHost) -> (Option<String>, i64) {
    host.sql(|c| {
        c.query_row(
            "SELECT notify_day, notify_count FROM mods WHERE id = ?1",
            [MOD],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    })
}

// --- secrets.set --------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn key_slots_set_asks_the_page_for_the_slots_field_and_says_where_the_key_will_go() {
    let (host, session) = setup();
    let (action, args) = page(key_slots_set(&host, &session, json!({"name": "token"})).await);
    assert_eq!(action, PageAction::SecretsSet);
    assert_eq!(
        args,
        json!({"name": "token", "host": HOST, "header": "X-Api-Key", "set": false})
    );
    // Once a key is held for the slot, the mod is told so (and only that).
    store_key(&host, "token");
    let (_, args) = page(key_slots_set(&host, &session, json!({"name": "token"})).await);
    assert_eq!(args["set"], true);
    assert!(!args.to_string().contains(KEY));
    // The other slot is its own.
    let (_, args) = page(key_slots_set(&host, &session, json!({"name": "other"})).await);
    assert_eq!(
        (args["header"].as_str(), args["set"].as_bool()),
        (Some("Authorization"), Some(false))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_key_bound_to_a_moved_slot_is_reported_as_not_set() {
    let (host, session) = setup();
    store_key(&host, "token");
    host.sql(|c| {
        c.execute("UPDATE mod_secrets SET host = 'cdn.example.com'", [])
            .unwrap();
    });
    let (_, args) = page(key_slots_set(&host, &session, json!({"name": "token"})).await);
    assert_eq!(args["set"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn key_slots_set_refuses_what_is_not_a_declared_slot_and_counts_nothing_for_it() {
    let (host, session) = setup();
    for args in [
        json!({"name": "nope"}),
        json!({"name": ""}),
        json!({"name": 5}),
        json!({}),
        json!({"name": "token", "host": "evil.example.com"}),
        json!(null),
    ] {
        assert_eq!(
            code(key_slots_set(&host, &session, args.clone()).await),
            Code::BadArgs,
            "{args}"
        );
    }
    // None of those used up an ask: the session still has all of its own.
    for _ in 0..limits::SECRET_ASKS_PER_SESSION {
        page(key_slots_set(&host, &session, json!({"name": "token"})).await);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_may_ask_only_so_many_times_and_a_new_frame_starts_fresh() {
    let (host, session) = setup();
    for _ in 0..limits::SECRET_ASKS_PER_SESSION {
        page(key_slots_set(&host, &session, json!({"name": "token"})).await);
    }
    let e = error(key_slots_set(&host, &session, json!({"name": "token"})).await);
    assert_eq!(e.code, Code::RateLimited);
    // Still refused, and still told the same.
    assert_eq!(
        code(key_slots_set(&host, &session, json!({"name": "other"})).await),
        Code::RateLimited
    );
    // Another frame of the same mod is another session with its own count.
    let again = live_session(&host, MOD, crate::mods::outbox::testing::discarding());
    page(key_slots_set(&host, &again, json!({"name": "token"})).await);
}

/// The last ask is taken once however many calls race for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_asks_cannot_take_more_than_the_sessions_limit() {
    let (host, session) = setup();
    let calls = (0..12).map(|_| {
        let auth = authorise(&host, table::ROWS, &session, "secrets.set").unwrap();
        let host = host.clone();
        async move { surface::key_slots_set(&host, &auth, &json!({"name": "token"})).await }
    });
    let results = futures_util::future::join_all(calls).await;
    let allowed = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(allowed, limits::SECRET_ASKS_PER_SESSION as usize);
    assert!(results
        .iter()
        .filter_map(|r| r.as_ref().err())
        .all(|e| e.code == Code::RateLimited));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn key_slots_set_needs_the_internet_permission_and_a_live_session() {
    let (host, session) = setup();
    host.sql(|c| {
        c.execute(
            "UPDATE mod_grants SET decision = 'denied' WHERE permission = 'internet'",
            [],
        )
        .unwrap();
    });
    assert_eq!(
        code(key_slots_set(&host, &session, json!({"name": "token"})).await),
        Code::NotGranted
    );
    host.sql(|c| {
        c.execute(
            "UPDATE mod_grants SET decision = 'granted' WHERE permission = 'internet'",
            [],
        )
        .unwrap();
    });
    host.runtime().close(&session);
    assert_eq!(
        code(key_slots_set(&host, &session, json!({"name": "token"})).await),
        Code::SessionEnded
    );
}

// --- notify -------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_notice_is_a_page_action_with_the_clean_text_and_a_tone() {
    let (host, session) = setup();
    let (action, args) = page(notify(&host, &session, json!({"text": "  Rates updated  "})).await);
    assert_eq!(action, PageAction::Notify);
    assert_eq!(args, json!({"text": "Rates updated", "tone": "info"}));
    for tone in ["info", "warn", "good"] {
        let (_, args) = page(notify(&host, &session, json!({"text": "x", "tone": tone})).await);
        assert_eq!(args["tone"], tone);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_notice_that_is_not_one_plain_short_line_is_refused_and_never_counted() {
    let (host, session) = setup();
    let stacked = format!("a{}", "\u{0301}".repeat(5));
    let cases: Vec<Value> = vec![
        json!({}),
        json!({"text": ""}),
        json!({"text": "   "}),
        json!({"text": 5}),
        json!({"text": "x", "tone": "loud"}),
        json!({"text": "x", "tone": "INFO"}),
        json!({"text": "x", "tone": 1}),
        json!({"text": "x", "extra": true}),
        json!({"text": "two\nlines"}),
        json!({"text": "tab\there"}),
        json!({"text": "bell\u{7}"}),
        json!({"text": "zero\u{200b}width"}),
        json!({"text": "rtl\u{202e}override"}),
        json!({"text": stacked}),
        json!({"text": "a".repeat(limits::NOTIFY_MAX_CHARS + 1)}),
        json!({"text": "é".repeat(limits::NOTIFY_MAX_CHARS + 1)}),
        json!("just text"),
    ];
    for args in cases {
        let e = error(notify(&host, &session, args.clone()).await);
        assert_eq!(e.code, Code::BadArgs, "{args}");
        // The refusal names the rule, never the text.
        assert!(!e.message.contains("override"), "{}", e.message);
    }
    assert_eq!(counter(&host), (None, 0), "no refusal used up a notice");
    // The longest allowed text, in characters (not bytes), goes through.
    for text in [
        "a".repeat(limits::NOTIFY_MAX_CHARS),
        "é".repeat(limits::NOTIFY_MAX_CHARS),
    ] {
        page(notify(&host, &session, json!({ "text": text })).await);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mod_may_show_five_notices_a_day_and_each_mod_has_its_own_count() {
    let (host, session) = setup();
    for n in 1..=limits::NOTIFY_PER_DAY {
        page(notify(&host, &session, json!({"text": format!("n{n}")})).await);
    }
    let e = error(notify(&host, &session, json!({"text": "one too many"})).await);
    assert_eq!(e.code, Code::RateLimited);
    let (day, count) = counter(&host);
    assert!(day.is_some());
    assert_eq!(
        count,
        i64::from(limits::NOTIFY_PER_DAY),
        "the refused one is not counted"
    );

    // Another mod has its own five.
    host.sql(|c| {
        insert_mod(c, &ModRow::signed("acme.other", "second", &["notify"]));
        c.execute(
            "INSERT INTO mod_grants(mod_id, permission, decision) VALUES ('acme.other', 'notify', 'granted')",
            [],
        )
        .unwrap();
    });
    serve(&host, "acme.other");
    let other = live_session(
        &host,
        "acme.other",
        crate::mods::outbox::testing::discarding(),
    );
    page(notify(&host, &other, json!({"text": "mine"})).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_day_starts_the_count_again() {
    let (host, session) = setup();
    for _ in 0..limits::NOTIFY_PER_DAY {
        page(notify(&host, &session, json!({"text": "x"})).await);
    }
    assert_eq!(
        code(notify(&host, &session, json!({"text": "x"})).await),
        Code::RateLimited
    );
    // Yesterday's date (and a date from the future: a clock that was ahead) is not today's.
    for other_day in ["2000-01-01", "2999-12-31"] {
        host.sql(|c| {
            c.execute("UPDATE mods SET notify_day = ?1", [other_day])
                .unwrap();
        });
        page(notify(&host, &session, json!({"text": "again"})).await);
        let (day, count) = counter(&host);
        assert_eq!(count, 1, "{other_day}");
        assert_ne!(day.as_deref(), Some(other_day));
    }
}

/// The day is the user's local one: in zones 25 hours apart the dates are never the same, so a
/// count kept by UTC date (or by a stale zone) would not start again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_day_is_the_users_local_date() {
    let (host, session) = setup();
    let set_zone = |zone: &str| {
        host.sql(|c| crate::db::set_setting(c, crate::settings::TIME_ZONE_KEY, zone).unwrap());
    };
    set_zone("Pacific/Kiritimati");
    page(notify(&host, &session, json!({"text": "x"})).await);
    page(notify(&host, &session, json!({"text": "x"})).await);
    assert_eq!(
        counter(&host),
        (
            Some(crate::clock::today_sql_in(chrono_tz::Pacific::Kiritimati)),
            2
        )
    );
    set_zone("Pacific/Pago_Pago");
    page(notify(&host, &session, json!({"text": "x"})).await);
    assert_eq!(
        counter(&host),
        (
            Some(crate::clock::today_sql_in(chrono_tz::Pacific::Pago_Pago)),
            1
        )
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_notice_whose_call_stopped_being_the_mods_counts_nothing() {
    let (host, session) = setup();
    let s = session.clone();
    host.hook(move |h, point| {
        // The frame goes away between the count and the commit (the guard is held: only the
        // runtime's own lock is taken).
        if point == Point::BeforeCommit {
            h.runtime().close(&s);
        }
    });
    assert_eq!(
        code(notify(&host, &session, json!({"text": "x"})).await),
        Code::SessionEnded
    );
    assert_eq!(
        counter(&host),
        (None, 0),
        "the count rolled back with the call"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parallel_notices_cannot_take_more_than_the_days_limit() {
    let (host, session) = setup();
    let calls = (0..12).map(|_| {
        let auth = authorise(&host, table::ROWS, &session, "notify").unwrap();
        let host = host.clone();
        async move { surface::notify(&host, &auth, &json!({"text": "x"})).await }
    });
    let results = futures_util::future::join_all(calls).await;
    assert_eq!(
        results.iter().filter(|r| r.is_ok()).count(),
        limits::NOTIFY_PER_DAY as usize
    );
    assert_eq!(counter(&host).1, i64::from(limits::NOTIFY_PER_DAY));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn who_may_notify_follows_the_policy_table() {
    // First party: automatic, no grant row needed.
    let (host, session) = setup_tier("first");
    host.sql(|c| {
        c.execute("DELETE FROM mod_grants WHERE permission = 'notify'", [])
            .unwrap();
    });
    page(notify(&host, &session, json!({"text": "x"})).await);
    // Second party: install-time, so a withdrawn or missing grant refuses.
    let (host, session) = setup_tier("second");
    host.sql(|c| {
        c.execute("DELETE FROM mod_grants WHERE permission = 'notify'", [])
            .unwrap();
    });
    let e = error(notify(&host, &session, json!({"text": "x"})).await);
    assert!(
        matches!(e.code, Code::NotGranted | Code::NeedsPrompt),
        "{e:?}"
    );
    assert_eq!(counter(&host), (None, 0));
    // Unsigned: asked (the first-use question), and counted only once allowed.
    let host = TestHost::new();
    host.sql(|c| insert_mod(c, &ModRow::unsigned(MOD, &["notify"])));
    serve(&host, MOD);
    let session = live_session(&host, MOD, crate::mods::outbox::testing::discarding());
    assert_eq!(
        code(notify(&host, &session, json!({"text": "x"})).await),
        Code::NeedsPrompt
    );
    host.sql(|c| {
        c.execute(
            "INSERT INTO mod_grants(mod_id, permission, decision) VALUES (?1, 'notify', 'granted')",
            [MOD],
        )
        .unwrap();
    });
    page(notify(&host, &session, json!({"text": "x"})).await);
}

#[test]
fn the_daily_slot_is_one_statement_that_resets_on_another_day_and_stops_at_the_limit() {
    let (_dir, conn) = crate::mods::api::test_support::store();
    insert_mod(&conn, &ModRow::signed(MOD, "second", &["notify"]));
    let take =
        |day: &str, per_day: u32| surface::take_notice_slot(&conn, MOD, day, per_day).unwrap();
    assert!(take("2026-11-17", 2), "no day stored yet");
    assert!(take("2026-11-17", 2));
    assert!(!take("2026-11-17", 2), "the third of a limit of two");
    assert!(take("2026-11-18", 2), "a new day starts again");
    assert_eq!(counter_of(&conn), (Some("2026-11-18".into()), 1));
    // Raising the limit lets the same day go on; a missing mod takes nothing.
    assert!(take("2026-11-18", 3));
    assert!(!surface::take_notice_slot(&conn, "acme.none", "2026-11-18", 3).unwrap());
}

fn counter_of(conn: &rusqlite::Connection) -> (Option<String>, i64) {
    conn.query_row(
        "SELECT notify_day, notify_count FROM mods WHERE id = ?1",
        [MOD],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap()
}

#[test]
fn both_rows_are_experimental_general_family_calls_with_the_permissions_the_spec_names() {
    for (name, permission, class) in [
        ("secrets.set", "internet", permissions::Class::Egress),
        ("notify", "notify", permissions::Class::Local),
    ] {
        let Lookup::Found(row) = table::lookup_call(table::ROWS, name, MOD_API_VERSION) else {
            panic!("no {name} row");
        };
        assert!(row.experimental, "{name}");
        assert_eq!(row.permission, Some(permission), "{name}");
        assert_eq!(row.class, class, "{name}");
        assert_eq!(row.family, limits::Family::General, "{name}");
    }
}

/// The page draws a notice from the tones and the length the backend holds it to: one list, kept in
/// both places by this test (`modPageHooks.ts` is the page's).
#[test]
fn the_page_holds_the_same_tones_and_length_as_rust() {
    let page = include_str!("../../../../src/components/mods/modPageHooks.ts");
    assert!(
        page.contains(&format!("NOTICE_MAX_CHARS = {};", limits::NOTIFY_MAX_CHARS)),
        "the page's notice length differs"
    );
    let tones = surface::TONES.map(|t| format!("\"{t}\"")).join(", ");
    assert!(
        page.contains(&format!("NOTICE_TONES = [{tones}] as const;")),
        "the page's tones differ"
    );
}
