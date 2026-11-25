// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! An app's requests through the real dispatcher over a real store: what each scope shows, that the
//! answers are the frame's own (one core, one projection), the order the checks run in with no hook
//! to help, the app's limiter, and the size an answer keeps whatever the rows hold.

use std::collections::BTreeSet;
use std::sync::atomic::Ordering;
use std::time::Instant;

use serde_json::{json, Value};

use super::jsonrpc::handle;
use super::limits::TOO_FAST;
use super::test_support::{open_vault, pair_app, result_json, App};
use super::tools::{PAGE_BUDGET, RESULT_BUDGET, TEXT_CAP};
use super::{device, Bound, SCOPES};
use crate::mods::api::call_fixtures::*;
use crate::mods::api::limits::Family;
use crate::mods::api::test_support::TestHost;
use crate::mods::api::{self, Code, ModError, Reply};
use crate::mods::devmode::Here;

const MOD: &str = "atlas.tasker";

/// What a reply line adds to its tool result's text: the wrapper and a short id.
const ENVELOPE: usize = 1 << 10;

/// A host with a mod holding `mod_perms` (for the frame side of a parity test) and an app holding
/// `scopes`, over one store.
fn world(mod_perms: &[&'static str], scopes: &[&str]) -> (TestHost, String, Here, App) {
    let (host, session) = first(MOD, mod_perms);
    let here = open_vault(&host);
    let app = pair_app(&host, &here, scopes);
    (host, session, here, app)
}

fn app_only(scopes: &[&str]) -> (TestHost, Here, App) {
    let (host, _, here, app) = world(&[], scopes);
    (host, here, app)
}

async fn ask(host: &TestHost, bound: Bound, tool: &str, args: Value) -> Result<Value, ModError> {
    match api::dispatch(host, bound.caller(), tool, &args.to_string()).await {
        Reply::Value { value } => Ok(value),
        Reply::Error { error } => Err(error),
        other => panic!("an app got {other:?}"),
    }
}

async fn ok(host: &TestHost, app: &App, tool: &str, args: Value) -> Value {
    ask(host, app.bound, tool, args)
        .await
        .unwrap_or_else(|e| panic!("{tool} refused: {e:?}"))
}

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().unwrap().keys().cloned().collect()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| (*n).to_owned()).collect()
}

/// A project with a milestone, an event, a task linked to both and a task of no project at all.
fn seed_linked(host: &TestHost) {
    host.sql(|c| {
        let milestone = seed_milestone(c, "Alpha", "Launch");
        // The registry the project list reads.
        c.execute(
            "INSERT INTO tags(kind, name, norm) VALUES ('project', 'Alpha', 'alpha')",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO calendar_events(id, calendar_id, summary, start, uid) \
             VALUES ('c:1', 'c', 'Sprint review', '2026-12-05T10:00:00Z', 'uid-1@example.com')",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO tasks(title, project_name, milestone_id, event_uid, deadline_from, \
                               deadline, ordinal) \
             VALUES ('Linked', 'Alpha', ?1, 'uid-1@example.com', 'event', '2026-12-05', 0)",
            [milestone],
        )
        .unwrap();
        c.execute(
            "UPDATE project_milestones SET event_uid = 'uid-1@example.com' WHERE id = ?1",
            [milestone],
        )
        .unwrap();
        seed_task(c, "Plain", None);
    });
}

/// A calendar and its source.
fn seed_calendar(host: &TestHost, id: &str, quiet: bool) {
    host.sql(|c| {
        crate::calendar::upsert_source(
            c,
            "gcal:me@example.com",
            "google",
            Some("me@example.com"),
            "me@example.com",
        )
        .unwrap();
        c.execute(
            "INSERT INTO calendars(id, source_id, provider, remote_id, name, quiet) \
             VALUES (?1, 'gcal:me@example.com', 'google', 'me@example.com', 'Work', ?2)",
            rusqlite::params![id, quiet as i64],
        )
        .unwrap();
    });
}

/// One event `days` from today at noon.
fn seed_event(
    host: &TestHost,
    id: &str,
    summary: &str,
    location: Option<&str>,
    days: i64,
    uid: Option<&str>,
) {
    host.sql(|c| {
        c.execute(
            "INSERT INTO calendar_events(id, calendar_id, summary, location, start, end, uid) \
             VALUES (?1, 'cal-1', ?2, ?3, datetime(date('now'), ?4, '+12 hours'), \
                     datetime(date('now'), ?4, '+12 hours', '+30 minutes'), ?5)",
            rusqlite::params![id, summary, location, format!("{days:+} days"), uid],
        )
        .unwrap();
    });
}

// --- scopes: what each one shows ----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tasks_scope_alone_shows_no_project_or_event_fields() {
    let always = [
        "id",
        "title",
        "why",
        "status",
        "completed_at",
        "lane",
        "stack_id",
        "parked",
        "ordinal",
        "source",
        "created_at",
        "updated_at",
        "do_start",
        "do_end",
        "deadline",
        "deadline_kind",
        "deadline_from",
        "effective_deadline",
        "deadline_missing",
        "milestone_linked",
        "event_linked",
    ];
    let projects = [
        "project_name",
        "project_archived",
        "milestone_id",
        "milestone_label",
    ];
    let calendar = ["event_uid", "event_title"];
    for (p, c) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut held = vec!["tasks.read"];
        if p {
            held.push("projects.read");
        }
        if c {
            held.push("calendar.read");
        }
        let (host, _, app) = app_only(&held);
        seed_linked(&host);
        let v = ok(&host, &app, "tasks", json!({})).await;
        let mut expected = set(&always);
        if p {
            expected.extend(set(&projects));
        }
        if c {
            expected.extend(set(&calendar));
        }
        let tasks = v["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 2, "{held:?}");
        for task in tasks {
            assert_eq!(keys(task), expected, "{held:?}");
        }
        // The flags are always said, so a link the app can't look through never reads as removed.
        assert_eq!(tasks[0]["milestone_linked"], true);
        assert_eq!(tasks[0]["event_linked"], true);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn milestones_need_projects_read_and_events_need_calendar_read() {
    let (host, _, app) = app_only(&["tasks.read", "calendar.read"]);
    seed_linked(&host);
    // A milestone always names its project: without `projects.read` the tool isn't there.
    let e = ask(&host, app.bound, "milestones", json!({}))
        .await
        .unwrap_err();
    assert_eq!(e.code, Code::UnknownMethod);
    let e2 = ask(&host, app.bound, "nonesuch", json!({}))
        .await
        .unwrap_err();
    assert_eq!(e, e2, "a hidden tool and an unknown one are told alike");
    // And the same for projects, and for a project filter on the task list.
    assert_eq!(
        ask(&host, app.bound, "projects", json!({}))
            .await
            .unwrap_err()
            .code,
        Code::UnknownMethod
    );
    let e = ask(&host, app.bound, "tasks", json!({"project": "Alpha"}))
        .await
        .unwrap_err();
    assert_eq!(e.code, Code::NotGranted);
    // With `projects.read`, milestones carry the calendar fields only with `calendar.read`.
    let (host, _, app) = app_only(&["projects.read"]);
    seed_linked(&host);
    let v = ok(&host, &app, "milestones", json!({})).await;
    let m = &v["milestones"][0];
    assert_eq!(m["project_name"], "Alpha");
    assert_eq!(m["calendar_linked"], true);
    assert!(m.get("event_uid").is_none() && m.get("event_missing").is_none());
    assert_eq!(
        ok(&host, &app, "projects", json!({})).await["projects"],
        json!(["Alpha"])
    );
    // A client without `calendar.read` has no events tool.
    assert_eq!(
        ask(&host, app.bound, "events", json!({}))
            .await
            .unwrap_err()
            .code,
        Code::UnknownMethod
    );
    let (host, _, app) = app_only(&["projects.read", "calendar.read"]);
    seed_linked(&host);
    let v = ok(&host, &app, "milestones", json!({})).await;
    assert_eq!(v["milestones"][0]["event_uid"], "uid-1@example.com");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_event_has_exactly_the_seven_fields_and_no_guest_or_account() {
    let (host, _, app) = app_only(&["calendar.read"]);
    seed_calendar(&host, "cal-1", false);
    seed_event(
        &host,
        "e1",
        "Standup",
        Some("Room 4"),
        1,
        Some("u1@example.com"),
    );
    seed_event(&host, "e2", "No uid", None, 2, None);
    host.sql(|c| {
        c.execute(
            "UPDATE calendar_events SET description = 'zz-marker@example.invalid', \
             attendees = '[\"zz-marker@example.invalid\"]' WHERE id = 'e1'",
            [],
        )
        .unwrap();
    });
    let v = ok(&host, &app, "events", json!({})).await;
    let events = v["events"].as_array().unwrap();
    assert_eq!(events.len(), 2);
    for e in events {
        assert_eq!(
            keys(e),
            set(&[
                "uid",
                "title",
                "start",
                "end",
                "allDay",
                "location",
                "recurring"
            ])
        );
    }
    assert_eq!(events[0]["uid"], "u1@example.com");
    assert_eq!(events[1]["uid"], Value::Null);
    assert!(!v.to_string().contains("zz-marker"));
    assert_eq!(v["truncated"], false);
}

// --- one core: the app's answer is the frame's ---------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tasks_and_milestones_equal_the_frames_under_the_equivalent_grants() {
    for (p, c) in [(false, false), (true, false), (false, true), (true, true)] {
        // The frame: a mod holding the same permissions. The app: the same scopes.
        let mut perms: Vec<&'static str> = vec!["tasks.read"];
        let mut held = vec!["tasks.read"];
        if p {
            perms.push("projects.read");
            held.push("projects.read");
        }
        if c {
            perms.push("calendar.read");
            held.push("calendar.read");
        }
        let (host, session, _, app) = world(&perms, &held);
        seed_linked(&host);
        let frame = value(call(&host, &session, "tasks.list", json!({})).await);
        let mcp = ok(&host, &app, "tasks", json!({})).await;
        assert_eq!(frame["tasks"], mcp["tasks"], "tasks {held:?}");
        assert_eq!(frame["next"], mcp["next"]);
    }
    for c in [false, true] {
        let mut perms: Vec<&'static str> = vec!["projects.read"];
        let mut held = vec!["projects.read"];
        if c {
            perms.push("calendar.read");
            held.push("calendar.read");
        }
        let (host, session, _, app) = world(&perms, &held);
        seed_linked(&host);
        let frame = value(call(&host, &session, "milestones.list", json!({})).await);
        let mcp = ok(&host, &app, "milestones", json!({})).await;
        assert_eq!(
            frame["milestones"], mcp["milestones"],
            "milestones {held:?}"
        );
        let frame = value(call(&host, &session, "projects.list", json!({})).await);
        let mcp = ok(&host, &app, "projects", json!({})).await;
        assert_eq!(frame, mcp, "projects");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_agenda_equals_the_frames() {
    let (host, session, _, app) = world(&["calendar.read"], &["calendar.read"]);
    seed_calendar(&host, "cal-1", false);
    seed_event(
        &host,
        "e1",
        "Standup",
        Some("Room 4"),
        1,
        Some("u1@example.com"),
    );
    seed_event(&host, "e2", "Review", None, 2, None);
    let frame = value(call(&host, &session, "calendar.agenda", json!({"limit": 5})).await);
    let mcp = ok(&host, &app, "events", json!({"limit": 5})).await;
    assert_eq!(frame["events"], mcp["events"]);
    assert_eq!(mcp["events"].as_array().unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_arguments_are_range_checked_the_same_way() {
    let (host, _, app) = app_only(&SCOPES);
    for (tool, args) in [
        ("tasks", json!({"limit": 0})),
        ("tasks", json!({"limit": 201})),
        ("tasks", json!({"limit": "5"})),
        ("tasks", json!({"after": 5})),
        ("tasks", json!({"extra": 1})),
        ("milestones", json!({"limit": 0})),
        ("milestones", json!({"limit": 201})),
        ("milestones", json!({"after": -1})),
        ("events", json!({"days": 0})),
        ("events", json!({"days": 61})),
        ("events", json!({"limit": 0})),
        ("events", json!({"limit": 201})),
        ("events", json!({"days": 1.5})),
        ("projects", json!({"x": 1})),
        ("search", json!({})),
        ("search", json!({"query": "   "})),
        ("search", json!({"query": "a", "limit": 21})),
        ("search", json!({"query": "a", "limit": 0})),
    ] {
        let e = ask(&host, app.bound, tool, args.clone()).await.unwrap_err();
        assert_eq!(e.code, Code::BadArgs, "{tool} {args}");
    }
    // The argument text itself: not JSON, not an object, past 1 MiB.
    for text in ["not json", "[1]", "7", "\"x\""] {
        let reply = api::dispatch(&host, app.bound.caller(), "tasks", text).await;
        let Reply::Error { error } = reply else {
            panic!("{text}")
        };
        assert_eq!(error.code, Code::BadArgs, "{text}");
    }
    let big = format!("{{\"project\":\"{}\"}}", "a".repeat(1 << 20));
    let Reply::Error { error } = api::dispatch(&host, app.bound.caller(), "tasks", &big).await
    else {
        panic!("a request past the cap was answered");
    };
    assert_eq!(error.code, Code::TooLarge);
    // Nothing at all is `{}`.
    let reply = api::dispatch(&host, app.bound.caller(), "projects", "").await;
    assert!(matches!(reply, Reply::Value { .. }));
}

// --- search -------------------------------------------------------------------------------------

fn vector0() -> Vec<f32> {
    let mut v = vec![0.0f32; 384];
    v[0] = 1.0;
    v
}

/// A vault document with one chunk, found by the words `indexed` and by dimension 0.
fn seed_doc(host: &TestHost, title: &str, project: &str, content: &str) -> i64 {
    host.sql(|c| {
        c.execute(
            "INSERT INTO documents(vault_path, source_path, title, content_hash, source_type, \
                                   project, source_id) \
             VALUES (?1, ?2, ?3, ?1, 'vault', ?4, 'external-ref-1')",
            rusqlite::params![
                "secret/vault-path.md",
                "/home/secret/source-path.md",
                title,
                project
            ],
        )
        .unwrap();
        let document = c.last_insert_rowid();
        c.execute(
            "INSERT INTO chunks(document_id, ordinal, heading, content, char_count) \
             VALUES (?1, 0, 'Terms', ?2, ?3)",
            rusqlite::params![document, content, content.len() as i64],
        )
        .unwrap();
        let chunk = c.last_insert_rowid();
        c.execute(
            "INSERT INTO chunk_vec(rowid, embedding) VALUES (?1, ?2)",
            rusqlite::params![chunk, serde_json::to_string(&vector0()).unwrap()],
        )
        .unwrap();
        c.execute(
            "INSERT INTO chunks_fts(rowid, content) VALUES (?1, ?2)",
            rusqlite::params![chunk, content],
        )
        .unwrap();
        document
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_answers_with_the_allowed_fields_and_the_projects_one_only_with_its_scope() {
    for (scopes, with_project) in [
        (vec!["documents.text"], false),
        (vec!["documents.text", "projects.read"], true),
    ] {
        let (host, _, app) = app_only(&scopes);
        host.set_engine_ready(true);
        host.set_query_vector(vector0());
        let doc = seed_doc(&host, "Lease", "Alpha", "rent is due on the first");
        let v = ok(&host, &app, "search", json!({"query": "rent due"})).await;
        let hits = v["hits"].as_array().unwrap();
        assert_eq!(hits.len(), 1, "{scopes:?}");
        let mut expected = set(&["documentId", "title", "heading", "snippet", "indexOnly"]);
        if with_project {
            expected.insert("project".into());
        }
        assert_eq!(keys(&hits[0]), expected, "{scopes:?}");
        assert_eq!(hits[0]["documentId"], doc);
        let said = v.to_string();
        for never in ["vault-path", "source-path", "external-ref"] {
            assert!(!said.contains(never), "{never}");
        }
        assert_eq!(host.searched(), ["rent due"]);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_needs_its_scope_and_a_ready_engine() {
    // Without `documents.text` the tool is not there, and nothing was embedded.
    let (host, _, app) = app_only(&["tasks.read"]);
    host.set_engine_ready(true);
    let e = ask(&host, app.bound, "search", json!({"query": "rent"}))
        .await
        .unwrap_err();
    assert_eq!(e.code, Code::UnknownMethod);
    assert!(host.searched().is_empty());
    // With it but the engine not set up: MOD-5007, before any retrieval.
    let (host, _, app) = app_only(&["documents.text"]);
    host.set_engine_ready(false);
    seed_doc(&host, "Lease", "", "rent is due");
    let e = ask(&host, app.bound, "search", json!({"query": "rent"}))
        .await
        .unwrap_err();
    assert_eq!(e.code, Code::EngineNotReady);
    assert!(host.searched().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_search_cut_off_by_a_revoked_app_returns_nothing() {
    let (host, here, app) = app_only(&["documents.text"]);
    host.set_engine_ready(true);
    host.set_query_vector(vector0());
    seed_doc(&host, "Lease", "", "rent is due");
    // The app is revoked while the embed runs (the host's hook fires at the heavy step).
    let (revoke_host, revoke_here, id) = (host.clone(), here.clone(), app.id);
    host.hook(move |_, point| {
        if point == api::Point::Embedding {
            revoke_host
                .sql(|c| super::clients::revoke(c, &revoke_here, id))
                .unwrap();
        }
    });
    let e = ask(&host, app.bound, "search", json!({"query": "rent"}))
        .await
        .unwrap_err();
    assert_eq!(e.code, Code::NotGranted);
}

// --- the order of the checks, with no hook to help ----------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_locked_app_refuses_everything() {
    let (host, _, app) = app_only(&SCOPES);
    assert!(ask(&host, app.bound, "projects", json!({})).await.is_ok());
    host.0.locked.store(true, Ordering::SeqCst);
    for tool in ["tasks", "events", "projects", "milestones", "search"] {
        let e = ask(&host, app.bound, tool, json!({"query": "x"}))
            .await
            .unwrap_err();
        assert_eq!(e.code, Code::Locked, "{tool}");
    }
    let e = super::list_tools(&host, app.bound).await.unwrap_err();
    assert_eq!(e.code, Code::Locked);
    host.0.locked.store(false, Ordering::SeqCst);
    assert!(ask(&host, app.bound, "projects", json!({})).await.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_closed_store_or_a_vault_that_is_not_open_refuses() {
    let (host, _, app) = app_only(&["tasks.read"]);
    // No vault open: nothing to name this device in.
    host.store().clear_vault_runtime().unwrap();
    let e = ask(&host, app.bound, "tasks", json!({})).await.unwrap_err();
    assert_eq!(e.code, Code::Locked);
    // A store taken away (a vault switch, a lock): closed.
    let (host, _, app) = app_only(&["tasks.read"]);
    host.store().take_conn().unwrap();
    let e = ask(&host, app.bound, "tasks", json!({})).await.unwrap_err();
    assert_eq!(e.code, Code::Locked);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connection_made_against_another_store_is_over() {
    let (host, _, app) = app_only(&["tasks.read"]);
    assert!(ask(&host, app.bound, "tasks", json!({})).await.is_ok());
    for other in [
        app.bound.store_epoch + 1,
        app.bound.store_epoch.wrapping_sub(1),
    ] {
        let stale = Bound {
            client: app.id,
            store_epoch: other,
        };
        let e = ask(&host, stale, "tasks", json!({})).await.unwrap_err();
        assert_eq!(e.code, Code::SessionEnded, "{other}");
        assert_eq!(
            super::list_tools(&host, stale).await.unwrap_err().code,
            Code::SessionEnded
        );
    }
    // A real move of the epoch: the same vault's store closed and opened again.
    let conn = host.store().take_conn().unwrap().unwrap();
    let (runtime, _, _) = super::test_support::vault_runtime(&host);
    host.store().open_session(conn, runtime).unwrap();
    let e = ask(&host, app.bound, "tasks", json!({})).await.unwrap_err();
    assert_eq!(e.code, Code::SessionEnded);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_revoked_switched_off_or_unpaired_app_is_refused_at_its_next_request() {
    let (host, here, app) = app_only(&["tasks.read"]);
    assert!(ask(&host, app.bound, "tasks", json!({})).await.is_ok());
    assert!(host
        .sql(|c| super::clients::verify(c, &here, &app.token))
        .is_some());
    // Switched off.
    device::set_on(&here, false).unwrap();
    assert_eq!(
        ask(&host, app.bound, "tasks", json!({}))
            .await
            .unwrap_err()
            .code,
        Code::NotGranted
    );
    assert_eq!(
        super::list_tools(&host, app.bound).await.unwrap_err().code,
        Code::NotGranted
    );
    device::set_on(&here, true).unwrap();
    assert!(ask(&host, app.bound, "tasks", json!({})).await.is_ok());
    // Its pairing lost.
    let pair = device::device_state(&here).clients[0].clone();
    device::unpair(&here, app.id).unwrap();
    assert_eq!(
        ask(&host, app.bound, "tasks", json!({}))
            .await
            .unwrap_err()
            .code,
        Code::NotGranted
    );
    device::pair(&here, pair).unwrap();
    assert!(ask(&host, app.bound, "tasks", json!({})).await.is_ok());
    // Widened in the store.
    host.sql(|c| {
        c.execute(
            "UPDATE mcp_clients SET scopes = '[\"tasks.read\",\"documents.text\"]'",
            [],
        )
        .unwrap()
    });
    assert_eq!(
        ask(&host, app.bound, "tasks", json!({}))
            .await
            .unwrap_err()
            .code,
        Code::NotGranted
    );
    host.sql(|c| {
        c.execute("UPDATE mcp_clients SET scopes = '[\"tasks.read\"]'", [])
            .unwrap()
    });
    assert!(ask(&host, app.bound, "tasks", json!({})).await.is_ok());
    // Its row deleted.
    host.sql(|c| c.execute("DELETE FROM mcp_clients", []).unwrap());
    assert_eq!(
        ask(&host, app.bound, "tasks", json!({}))
            .await
            .unwrap_err()
            .code,
        Code::NotGranted
    );
    assert!(host
        .sql(|c| super::clients::verify(c, &here, &app.token))
        .is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_is_stamped_used() {
    let (host, _, app) = app_only(&["tasks.read"]);
    let used = |host: &TestHost| -> Option<String> {
        host.sql(|c| {
            c.query_row("SELECT last_used_at FROM mcp_clients", [], |r| r.get(0))
                .unwrap()
        })
    };
    assert_eq!(used(&host), None);
    ok(&host, &app, "tasks", json!({})).await;
    assert!(used(&host).is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scope_change_is_seen_by_the_next_request_without_reconnecting() {
    let (host, here, app) = app_only(&["tasks.read"]);
    let refused = |e: ModError| (e.code, e.message);
    let before = ask(&host, app.bound, "projects", json!({}))
        .await
        .unwrap_err();
    assert_eq!(before.code, Code::UnknownMethod);
    // Paired again with a wider set (the only way the file lets a scope grow): the same connection
    // (the same `Bound`) can use the new tool at once, and the listing shows it.
    let widen = |held: &[&str]| {
        let held: Vec<String> = held.iter().map(|s| (*s).to_owned()).collect();
        let hash = device::device_state(&here).clients[0].hash.clone();
        host.sql(|c| {
            c.execute(
                "UPDATE mcp_clients SET scopes = ?1",
                [serde_json::to_string(&held).unwrap()],
            )
            .unwrap()
        });
        device::pair(
            &here,
            device::PairedClient {
                id: app.id,
                scopes_hash: super::clients::scopes_hash(app.id, &hash, &held),
                hash,
            },
        )
        .unwrap();
    };
    widen(&["projects.read", "tasks.read"]);
    ok(&host, &app, "projects", json!({})).await;
    let listed = super::list_tools(&host, app.bound).await.unwrap();
    assert_eq!(listed.len(), 3, "{listed:?}");
    // And narrowed again: the tool is gone as before, told in the same words.
    widen(&["tasks.read"]);
    let after = ask(&host, app.bound, "projects", json!({}))
        .await
        .unwrap_err();
    assert_eq!(refused(after), refused(before));
}

// --- the app's limiter --------------------------------------------------------------------------

/// Hold every slot in flight `app` may have (the cap is on what is running at once, so it is spent
/// without any clock to race).
fn fill_slots(host: &TestHost, app: i64) -> Vec<super::limits::Held> {
    let limits = api::Host::mcp(host).limits();
    let mut held = Vec::new();
    while let Ok(slot) = limits.admit_call(app) {
        held.push(slot);
        assert!(held.len() < 1000, "the app's slots never ran out");
    }
    assert!(!held.is_empty());
    held
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refusal_for_speed_is_told_in_the_apps_words_not_the_mods() {
    let (host, here, app) = app_only(&["tasks.read"]);
    let other = pair_app(&host, &here, &["tasks.read"]);
    let slots = fill_slots(&host, app.id);
    let e = ask(&host, app.bound, "tasks", json!({}))
        .await
        .expect_err("a spent app is refused");
    assert_eq!(e.code, Code::RateLimited);
    assert_eq!(e.message, TOO_FAST);
    let words = e.message.to_lowercase();
    assert!(!words.contains("mod"), "{words}");
    // Another app has its own allowance.
    assert!(ask(&host, other.bound, "tasks", json!({})).await.is_ok());
    // The slots come back when the requests end.
    drop(slots);
    assert!(ask(&host, app.bound, "tasks", json!({})).await.is_ok());
}

#[test]
fn the_bucket_refuses_past_its_burst_at_one_instant() {
    // At one fixed instant no token refills, so the burst is spent exactly (no race with the clock).
    let host = TestHost::new();
    let limits = api::Host::mcp(&host).limits();
    let now = Instant::now();
    for _ in 0..100 {
        drop(limits.admit_call_at(1, now).unwrap());
    }
    let e = limits.admit_call_at(1, now).unwrap_err();
    assert_eq!((e.code, e.message.as_str()), (Code::RateLimited, TOO_FAST));
    assert!(limits.admit_call_at(2, now).is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requests_that_are_refused_anyway_still_cost_the_app_its_allowance() {
    // An unknown tool, a tool the scopes hide and a listing all fail before any tool runs, but each
    // does the full read of the device's file and the store: they are limited like any request.
    let (host, _, app) = app_only(&["tasks.read"]);
    let slots = fill_slots(&host, app.id);
    for tool in ["nope", "projects", "search", "tasks"] {
        let e = ask(&host, app.bound, tool, json!({})).await.unwrap_err();
        assert_eq!(
            (e.code, e.message.as_str()),
            (Code::RateLimited, TOO_FAST),
            "{tool}"
        );
    }
    let e = super::list_tools(&host, app.bound).await.unwrap_err();
    assert_eq!((e.code, e.message.as_str()), (Code::RateLimited, TOO_FAST));
    // With room again, an unknown tool and a hidden one are told the same way, word for word.
    drop(slots);
    let unknown = ask(&host, app.bound, "nope", json!({})).await.unwrap_err();
    let hidden = ask(&host, app.bound, "projects", json!({}))
        .await
        .unwrap_err();
    assert_eq!(unknown.code, Code::UnknownMethod);
    assert_eq!(
        (unknown.code, unknown.message),
        (hidden.code, hidden.message)
    );
    assert!(super::list_tools(&host, app.bound).await.is_ok());
}

#[test]
fn search_is_one_at_a_time_per_app_and_the_words_are_the_same() {
    let host = TestHost::new();
    let limits = api::Host::mcp(&host).limits();
    let now = Instant::now();
    let held = limits.admit_at(1, Family::Embed, now).unwrap();
    let e = limits.admit_at(1, Family::Embed, now).unwrap_err();
    assert_eq!((e.code, e.message.as_str()), (Code::RateLimited, TOO_FAST));
    // Another app, and a general request of the same app, are not held up by it.
    assert!(limits.admit_at(2, Family::Embed, now).is_ok());
    assert!(limits.admit_at(1, Family::General, now).is_ok());
    drop(held);
    assert!(limits.admit_at(1, Family::Embed, now).is_ok());
}

// --- the size an answer keeps -------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_enormous_row_cannot_push_an_answer_past_its_budget() {
    let (host, _, app) = app_only(&["tasks.read", "projects.read", "calendar.read"]);
    host.sql(|c| {
        c.execute(
            "INSERT INTO tasks(title, why, event_uid, ordinal) VALUES (?1, ?1, ?1, 0)",
            [&"x".repeat(2 << 20)],
        )
        .unwrap();
    });
    let v = ok(&host, &app, "tasks", json!({"limit": 200})).await;
    let size = v.to_string().len();
    assert!(size <= RESULT_BUDGET, "{size}");
    let title = v["tasks"][0]["title"].as_str().unwrap();
    assert_eq!(title.chars().count(), TEXT_CAP);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hostile_list_is_paged_under_the_budget_even_with_every_text_at_its_worst() {
    let (host, _, app) = app_only(&["tasks.read", "projects.read", "calendar.read"]);
    // Control characters are the widest text JSON can hold (six bytes each once escaped).
    let nasty = "\u{1}".repeat(50_000);
    host.sql(|c| {
        for i in 0..200 {
            c.execute(
                "INSERT INTO tasks(title, why, event_uid, ordinal) VALUES (?1, ?1, ?1, ?2)",
                rusqlite::params![nasty, i],
            )
            .unwrap();
        }
    });
    let v = ok(&host, &app, "tasks", json!({"limit": 200})).await;
    let size = v.to_string().len();
    assert!(size <= RESULT_BUDGET, "{size}");
    let tasks = v["tasks"].as_array().unwrap();
    assert!(!tasks.is_empty() && tasks.len() < 200, "{}", tasks.len());
    // The page said where it stopped, and the next one picks up there.
    let next = v["next"].as_str().expect("a cursor").to_owned();
    let more = ok(&host, &app, "tasks", json!({"limit": 200, "after": next})).await;
    assert!(!more["tasks"].as_array().unwrap().is_empty());
    assert!(tasks
        .iter()
        .all(|t| t["title"].as_str().unwrap().chars().count() <= TEXT_CAP));
    assert!(v["tasks"].to_string().len() <= PAGE_BUDGET);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hostile_events_projects_and_milestones_keep_the_budget_too() {
    let (host, _, app) = app_only(&["calendar.read", "projects.read"]);
    seed_calendar(&host, "cal-1", false);
    let nasty = "\u{1}".repeat(50_000);
    host.sql(|c| {
        for i in 0..200 {
            c.execute(
                "INSERT INTO calendar_events(id, calendar_id, summary, location, start, end, uid) \
                 VALUES (?1, 'cal-1', ?2, ?2, datetime(date('now'), '+1 days', '+12 hours'), \
                         datetime(date('now'), '+1 days', '+13 hours'), ?3)",
                rusqlite::params![format!("e{i}"), nasty, format!("u{i}")],
            )
            .unwrap();
            c.execute(
                "INSERT INTO projects(name) VALUES (?1)",
                [format!("{i}-{nasty}")],
            )
            .unwrap();
            c.execute(
                "INSERT INTO project_milestones(project_name, label, due_date, sort_order) \
                 VALUES (?1, ?2, '2026-12-01', ?3)",
                rusqlite::params![format!("{i}-{nasty}"), nasty, i],
            )
            .unwrap();
        }
    });
    for (tool, args) in [
        ("events", json!({"limit": 200})),
        ("projects", json!({})),
        ("milestones", json!({"limit": 200})),
    ] {
        let v = ok(&host, &app, tool, args).await;
        let size = v.to_string().len();
        assert!(size <= RESULT_BUDGET, "{tool} {size}");
    }
    // The agenda says it was cut short.
    let v = ok(&host, &app, "events", json!({"limit": 200})).await;
    assert_eq!(v["truncated"], true);
    assert!(!v["events"].as_array().unwrap().is_empty());
}

/// The reply line of `tool` as the transport would send it, and its parsed form.
async fn wire(host: &TestHost, app: &App, tool: &str, args: Value) -> (usize, Value) {
    let request = json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
                         "params": {"name": tool, "arguments": args}});
    let line = handle(host, app.bound, &request.to_string())
        .await
        .expect("a reply");
    (line.len(), serde_json::from_str(&line).unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_reply_line_itself_keeps_the_budget_even_when_every_byte_is_escaped_twice() {
    // Quotes and backslashes are the worst text for a reply: they double once inside the result's own
    // JSON and again when that JSON rides as a content item's text.
    let (host, _, app) = app_only(&["tasks.read", "projects.read", "calendar.read"]);
    seed_calendar(&host, "cal-1", false);
    let nasty = "\"\\".repeat(2_000);
    host.sql(|c| {
        for i in 0..200 {
            c.execute(
                "INSERT INTO tasks(title, why, event_uid, ordinal) VALUES (?1, ?1, ?1, ?2)",
                rusqlite::params![nasty, i],
            )
            .unwrap();
            c.execute(
                "INSERT INTO calendar_events(id, calendar_id, summary, location, start, end, uid) \
                 VALUES (?1, 'cal-1', ?2, ?2, datetime(date('now'), '+1 days', '+12 hours'), \
                         datetime(date('now'), '+1 days', '+13 hours'), ?3)",
                rusqlite::params![format!("e{i}"), nasty, format!("u{i}")],
            )
            .unwrap();
            // The registry the project list reads, and the table a milestone belongs to.
            c.execute(
                "INSERT INTO tags(kind, name, norm) VALUES ('project', ?1, ?2)",
                rusqlite::params![format!("{i}-{nasty}"), format!("n{i}")],
            )
            .unwrap();
            c.execute(
                "INSERT INTO projects(name) VALUES (?1)",
                [format!("{i}-{nasty}")],
            )
            .unwrap();
            c.execute(
                "INSERT INTO project_milestones(project_name, label, due_date, sort_order) \
                 VALUES (?1, ?2, '2026-12-01', ?3)",
                rusqlite::params![format!("{i}-{nasty}"), nasty, i],
            )
            .unwrap();
        }
    });
    for (tool, key, args) in [
        ("tasks", "tasks", json!({"limit": 200})),
        ("events", "events", json!({"limit": 200})),
        ("projects", "projects", json!({})),
        ("milestones", "milestones", json!({"limit": 200})),
    ] {
        let (size, reply) = wire(&host, &app, tool, args).await;
        assert!(size <= RESULT_BUDGET + ENVELOPE, "{tool} {size}");
        assert_eq!(reply["result"]["isError"], false, "{tool}");
        let page = result_json(&reply);
        assert!(!page[key].as_array().unwrap().is_empty(), "{tool}");
    }
    // And the search's answer, at its widest.
    let (host, _, app) = app_only(&["documents.text", "projects.read"]);
    host.set_engine_ready(true);
    host.set_query_vector(vector0());
    for i in 0..25 {
        seed_doc_n(&host, i, &nasty);
    }
    let (size, reply) = wire(&host, &app, "search", json!({"query": "rent", "limit": 20})).await;
    assert!(size <= RESULT_BUDGET + ENVELOPE, "search {size}");
    assert_eq!(reply["result"]["isError"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_event_with_a_hostile_start_or_end_cannot_empty_the_page() {
    let (host, _, app) = app_only(&["calendar.read"]);
    seed_calendar(&host, "cal-1", false);
    seed_event(&host, "e1", "Hostile times", None, 1, Some("u1"));
    seed_event(&host, "e2", "Fine", None, 2, Some("u2"));
    // A start that sorts first, and an end, each hundreds of kilobytes (trailing spaces still parse as
    // a date, so the row stays in the agenda's window).
    host.sql(|c| {
        c.execute(
            "UPDATE calendar_events SET start = start || ?1, end = end || ?1 WHERE id = 'e1'",
            [&" ".repeat(300_000)],
        )
        .unwrap();
    });
    let v = ok(&host, &app, "events", json!({})).await;
    let events = v["events"].as_array().unwrap();
    assert_eq!(events.len(), 2, "{v}");
    for field in ["start", "end"] {
        assert!(
            events[0][field].as_str().unwrap().chars().count() <= 64,
            "{field}"
        );
    }
    assert_eq!(events[1]["title"], "Fine");
    assert_eq!(v["truncated"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_equals_the_frames() {
    let (host, session, _, app) = world(
        &["documents.text", "projects.read"],
        &["documents.text", "projects.read"],
    );
    host.set_engine_ready(true);
    host.set_query_vector(vector0());
    seed_doc(&host, "Lease", "Alpha", "rent is due on the first");
    let frame = value(
        call(
            &host,
            &session,
            "documents.search",
            json!({"query": "rent due"}),
        )
        .await,
    );
    let mcp = ok(&host, &app, "search", json!({"query": "rent due"})).await;
    assert_eq!(frame["hits"].as_array().unwrap().len(), 1);
    assert_eq!(frame, mcp);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_hits_keep_to_their_caps_whatever_the_rows_hold() {
    let (host, _, app) = app_only(&["documents.text", "projects.read"]);
    host.set_engine_ready(true);
    host.set_query_vector(vector0());
    let nasty = "\u{1}".repeat(50_000);
    for i in 0..25 {
        seed_doc_n(&host, i, &nasty);
    }
    let v = ok(&host, &app, "search", json!({"query": "rent", "limit": 20})).await;
    let hits = v["hits"].as_array().unwrap();
    assert!(hits.len() <= 20);
    assert!(v.to_string().len() <= RESULT_BUDGET);
    for h in hits {
        assert!(h["snippet"].as_str().unwrap().chars().count() <= 1000);
        assert!(h["title"].as_str().unwrap().chars().count() <= 200);
        assert!(h["project"].as_str().unwrap().chars().count() <= 200);
    }
}

fn seed_doc_n(host: &TestHost, i: usize, nasty: &str) {
    host.sql(|c| {
        c.execute(
            "INSERT INTO documents(vault_path, source_path, title, content_hash, source_type, \
                                   project, source_id) \
             VALUES (?1, ?1, ?2, ?1, 'vault', ?2, ?1)",
            rusqlite::params![format!("p{i}"), nasty],
        )
        .unwrap();
        let document = c.last_insert_rowid();
        c.execute(
            "INSERT INTO chunks(document_id, ordinal, heading, content, char_count) \
             VALUES (?1, 0, ?2, ?2, 1)",
            rusqlite::params![document, nasty],
        )
        .unwrap();
        let chunk = c.last_insert_rowid();
        c.execute(
            "INSERT INTO chunk_vec(rowid, embedding) VALUES (?1, ?2)",
            rusqlite::params![chunk, serde_json::to_string(&vector0()).unwrap()],
        )
        .unwrap();
        c.execute(
            "INSERT INTO chunks_fts(rowid, content) VALUES (?1, 'rent')",
            rusqlite::params![chunk],
        )
        .unwrap();
    });
}

#[test]
fn the_frames_own_calls_are_not_cut() {
    // The frame's page keeps its own list budget and cuts no text: only the app's pages are held to
    // the small one.
    use crate::mods::api::calls::{clip_texts, Fit};
    assert_eq!(Fit::FRAME.text_cap, None);
    const { assert!(Fit::FRAME.budget > RESULT_BUDGET) };
    let long = json!({"a": "x".repeat(5000), "b": ["y".repeat(3000)], "c": 5});
    let cut = clip_texts(long, 1000);
    assert_eq!(cut["a"].as_str().unwrap().len(), 1000);
    assert_eq!(cut["b"][0].as_str().unwrap().len(), 1000);
    assert_eq!(cut["c"], 5);
    // At a character boundary, never inside one.
    let wide = clip_texts(json!("é".repeat(10)), 3);
    assert_eq!(wide, json!("ééé"));
}
