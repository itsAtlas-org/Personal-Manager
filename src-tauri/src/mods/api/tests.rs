// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The dispatcher end to end: real frame sessions, a real store, every refusal in the check order,
//! calls in flight across an uninstall, a reinstall, a pause, a lock and a store swap, D3 across
//! two unsigned mods, and the sealed-engine gate.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex};

use rusqlite::params;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::http::Request;

use super::table::{Kind, Row};
use super::test_support::{insert_mod, manifest, ModRow, TestHost};
use super::*;
use crate::mods::ids::ModId;
use crate::mods::runtime::{InstalledMod, ServedFile};

fn id(s: &str) -> ModId {
    ModId::parse(s).unwrap()
}

/// Register `m`'s files with the runtime, as the ledger's startup load does for a row it serves.
fn serve(host: &TestHost, m: &str) {
    let root = host.dir().join("mods").join(m);
    std::fs::create_dir_all(root.join("app")).unwrap();
    let page = b"<!doctype html><p>mod";
    std::fs::write(root.join("app/index.html"), page).unwrap();
    let file = ServedFile {
        sha256: Sha256::digest(page).into(),
        size: page.len() as u64,
    };
    let installed = InstalledMod::from_ledger(
        id(m),
        &root,
        "app/index.html",
        [("app/index.html".into(), file)],
    )
    .unwrap();
    host.runtime().install(installed);
}

/// A live frame session for `m` over the open store: minted, and its entry page served.
fn session(host: &TestHost, m: &str) -> String {
    let opened = host
        .runtime()
        .open_frame_in(
            &id(m),
            host.store().store_epoch(),
            crate::mods::outbox::testing::discarding(),
        )
        .unwrap();
    let req = Request::builder()
        .uri(opened.entry_url.as_str())
        .body(Vec::new())
        .unwrap();
    assert_eq!(host.runtime().serve("main", &req).status(), 200);
    opened.session
}

/// A mod installed, served, and in a live session.
fn installed(host: &TestHost, row: ModRow) -> String {
    let m = row.id;
    host.sql(|c| insert_mod(c, &row));
    serve(host, m);
    session(host, m)
}

fn frame(session: &str) -> Caller {
    Caller::Frame {
        window: MAIN_LABEL.into(),
        session: session.into(),
    }
}

async fn call(host: &TestHost, session: &str, method: &str, args: Value) -> Reply {
    dispatch(host, frame(session), method, &args.to_string()).await
}

async fn call_in(
    host: &TestHost,
    rows: &'static [Row],
    session: &str,
    method: &str,
    args: Value,
) -> Reply {
    dispatch_in(host, rows, frame(session), method, &args.to_string()).await
}

fn value(reply: Reply) -> Value {
    match reply {
        Reply::Value { value } => value,
        other => panic!("not a value: {other:?}"),
    }
}

fn error(reply: Reply) -> ModError {
    match reply {
        Reply::Error { error } => error,
        other => panic!("not an error: {other:?}"),
    }
}

fn code(reply: Reply) -> Code {
    error(reply).code
}

fn kv_rows(host: &TestHost, m: &str) -> BTreeMap<String, (String, i64, String)> {
    host.sql(|c| {
        let mut stmt = c
            .prepare("SELECT key, value, revision, class FROM mod_kv WHERE mod_id = ?1")
            .unwrap();
        stmt.query_map([m], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?, r.get(3)?))))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    })
}

fn map_row() -> ModRow {
    ModRow {
        keys: vec![("layout", "cache"), ("settings", "content")],
        ..ModRow::signed("atlas.map", "first", &["storage", "documents.meta"])
    }
}

/// `storage.set`'s arguments.
fn set_args(key: &str, value: Value, revision: i64) -> Value {
    json!({"key": key, "value": value, "expectedRevision": revision})
}

/// A call from `window`, with arguments every handler refuses.
async fn call_from(host: &TestHost, window: &str, session: &str, method: &str) -> Reply {
    let caller = Caller::Frame {
        window: window.into(),
        session: session.into(),
    };
    dispatch(host, caller, method, "{\"x\":1}").await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_granted_call_runs_end_to_end() {
    let host = TestHost::new();
    let s = installed(&host, map_row());
    assert_eq!(
        call(&host, &s, "theme.get", Value::Null).await,
        Reply::Page {
            action: PageAction::ThemeGet,
            args: Value::Null
        }
    );
    assert_eq!(
        value(call(&host, &s, "storage.get", json!({"key": "settings"})).await),
        json!({"value": null, "revision": 0})
    );
    let set = |k: &'static str, v: Value, r| call(&host, &s, "storage.set", set_args(k, v, r));
    assert_eq!(
        value(set("settings", json!({"zoom": 2}), 0).await),
        json!({"revision": 1})
    );
    assert_eq!(
        value(set("layout", json!([1, 2]), 0).await),
        json!({"revision": 1})
    );
    assert_eq!(
        value(set("notes.v1", json!("hi"), 0).await),
        json!({"revision": 1})
    );
    assert_eq!(
        value(set("settings", json!({"zoom": 3}), 1).await),
        json!({"revision": 2})
    );
    assert_eq!(
        value(call(&host, &s, "storage.get", json!({"key": "settings"})).await),
        json!({"value": {"zoom": 3}, "revision": 2})
    );
    // Each key's class is the manifest's, and an undeclared key's is content (kept, backed up).
    let rows = kv_rows(&host, "atlas.map");
    assert_eq!(rows["settings"].2, "content");
    assert_eq!(rows["layout"].2, "cache");
    assert_eq!(rows["notes.v1"].2, "content");
    // A stale write is refused with the current revision, and changes nothing.
    let stale = error(set("settings", json!(0), 1).await);
    assert_eq!((stale.code, stale.revision), (Code::StaleRevision, Some(2)));
    assert_eq!(kv_rows(&host, "atlas.map")["settings"].0, r#"{"zoom":3}"#);
}

/// One refusal per step, in the dispatcher's order: with every later check also failing, the
/// earliest one answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_checks_refuse_in_order() {
    let host = TestHost::new();
    let s = installed(
        &host,
        ModRow::signed("acme.tool", "second", &["documents.meta"]),
    );
    // The mod lacks `storage` (step 5 fails), the method is unknown, the session is a stranger's,
    // and the arguments are wrong for every handler (step 7).
    host.0.locked.store(true, Ordering::SeqCst);
    host.sql(|c| c.execute("UPDATE mods SET state = 'paused'", []).unwrap());
    // 1. Window first.
    for window in ["calendar", "briefing", "", "Main", "main "] {
        let e = error(call_from(&host, window, "nobody", "nope.never").await);
        assert_eq!(e.code, Code::NotGranted, "{window:?}");
        assert_eq!(e.message, "Mods run only in PM's main window.");
    }
    // 2. The lock (read after the session is begun, so a stranger's session is "ended" first).
    let main = MAIN_LABEL;
    assert_eq!(
        code(call_from(&host, main, "nobody", "nope.never").await),
        Code::SessionEnded
    );
    assert_eq!(
        code(call_from(&host, main, &s, "nope.never").await),
        Code::Locked
    );
    host.0.locked.store(false, Ordering::SeqCst);
    // 3. The session.
    assert_eq!(
        code(call_from(&host, main, "nobody", "nope.never").await),
        Code::SessionEnded
    );
    // 4. The mod's state, before the method is even looked up.
    assert_eq!(
        code(call_from(&host, main, &s, "nope.never").await),
        Code::Paused
    );
    host.sql(|c| c.execute("UPDATE mods SET state = 'active'", []).unwrap());
    // 4. The method.
    assert_eq!(
        code(call_from(&host, main, &s, "nope.never").await),
        Code::UnknownMethod
    );
    // 5. The grant, before the rate.
    drain_bucket(&host, "acme.tool");
    let e = error(call_from(&host, main, &s, "storage.get").await);
    assert_eq!((e.code, e.permission), (Code::NotGranted, Some("storage")));
    host.sql(|c| {
        c.execute(
            "UPDATE mods SET manifest = ?1",
            [manifest(&["storage"], &[])],
        )
        .unwrap()
    });
    // 6. The rate, before the arguments.
    assert_eq!(
        code(call_from(&host, main, &s, "storage.get").await),
        Code::RateLimited
    );
    // 7. With a token again, the arguments.
    let later = std::time::Instant::now() + std::time::Duration::from_secs(10);
    drop(host.calls().admit(&id("acme.tool"), later).unwrap());
    // (The bucket refilled at `later`; real time catches up with it only in ten seconds, so the
    // next call reads a bucket stamped in its future, which earns nothing but holds 99 tokens.)
    assert_eq!(
        code(call_from(&host, main, &s, "storage.get").await),
        Code::BadArgs
    );
}

/// Spend `m`'s whole bucket now.
fn drain_bucket(host: &TestHost, m: &str) {
    let now = std::time::Instant::now();
    while host.calls().admit(&id(m), now).is_ok() {}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_other_refusal_has_its_code() {
    let host = TestHost::new();
    let s = installed(&host, map_row());
    // A session that never loaded its page.
    let pending = host
        .runtime()
        .open_frame_in(
            &id("atlas.map"),
            host.store().store_epoch(),
            crate::mods::outbox::testing::discarding(),
        )
        .unwrap();
    assert_eq!(
        code(call(&host, &pending.session, "theme.get", Value::Null).await),
        Code::SessionEnded
    );
    // A session from another store.
    let other_store = host
        .runtime()
        .open_frame_in(
            &id("atlas.map"),
            host.store().store_epoch() + 1,
            crate::mods::outbox::testing::discarding(),
        )
        .unwrap();
    let req = Request::builder()
        .uri(other_store.entry_url.as_str())
        .body(Vec::new())
        .unwrap();
    host.runtime().serve("main", &req);
    assert_eq!(
        code(call(&host, &other_store.session, "theme.get", Value::Null).await),
        Code::SessionEnded
    );
    // Every state that isn't running, and a mod not turned on yet (D17).
    for (state, consented) in [
        ("paused", true),
        ("parked", true),
        ("installing", true),
        ("active", false),
    ] {
        host.sql(|c| {
            c.execute(
                "UPDATE mods SET state = ?1, consented_at = CASE WHEN ?2 THEN 'x' END",
                params![state, consented],
            )
            .unwrap()
        });
        assert_eq!(
            code(call(&host, &s, "theme.get", Value::Null).await),
            Code::Paused,
            "{state} {consented}"
        );
    }
    host.sql(|c| {
        c.execute("UPDATE mods SET state = 'active', consented_at = 'x'", [])
            .unwrap()
    });
    // A denied grant.
    host.sql(|c| {
        c.execute(
            "INSERT INTO mod_grants(mod_id, permission, decision) VALUES ('atlas.map', 'storage', 'denied')",
            [],
        )
        .unwrap()
    });
    let e = error(call(&host, &s, "storage.get", json!({"key": "a"})).await);
    assert_eq!((e.code, e.permission), (Code::NotGranted, Some("storage")));
    host.sql(|c| c.execute("DELETE FROM mod_grants", []).unwrap());
    // Arguments: too large, not JSON, the wrong shape.
    let big = "x".repeat(limits::MAX_ARGS_BYTES);
    let reply = dispatch(&host, frame(&s), "storage.get", &format!("\"{big}\"")).await;
    assert_eq!(code(reply), Code::TooLarge);
    assert_eq!(
        code(dispatch(&host, frame(&s), "storage.get", "{not json").await),
        Code::BadArgs
    );
    for args in [
        json!(null),
        json!({}),
        json!({"key": 1}),
        json!({"key": "a", "extra": true}),
        json!({"key": "Bad Key"}),
    ] {
        assert_eq!(
            code(call(&host, &s, "storage.get", args.clone()).await),
            Code::BadArgs,
            "{args}"
        );
    }
    for args in [
        json!({"key": "a", "value": 1}),
        json!({"key": "a", "expectedRevision": 0}),
        json!({"key": "a", "value": 1, "expectedRevision": -1}),
        json!({"key": "a", "value": 1, "expectedRevision": 1.5}),
        json!({"key": "a", "value": 1, "expectedRevision": "0"}),
    ] {
        assert_eq!(
            code(call(&host, &s, "storage.set", args.clone()).await),
            Code::BadArgs,
            "{args}"
        );
    }
    assert_eq!(
        code(call(&host, &s, "theme.get", json!({"mode": "dark"})).await),
        Code::BadArgs
    );
    assert!(kv_rows(&host, "atlas.map").is_empty());
    // A value PM can't read back is PM's failure, said without what failed.
    host.sql(|c| {
        c.execute(
            "INSERT INTO mod_kv(mod_id, key, class, value) VALUES ('atlas.map', 'bad', 'cache', 'not json')",
            [],
        )
        .unwrap()
    });
    let e = error(call(&host, &s, "storage.get", json!({"key": "bad"})).await);
    assert_eq!(
        (e.code, e.message.as_str()),
        (Code::Internal, error::INTERNAL_MESSAGE)
    );
    // The store closed: PM is "locked" to a mod.
    let conn = host.store().take_conn().unwrap();
    assert_eq!(
        code(call(&host, &s, "theme.get", Value::Null).await),
        Code::Locked
    );
    drop(conn);
}

#[test]
fn a_removed_or_unknown_name_has_its_own_code() {
    let row = &table::ROWS[0];
    assert_eq!(
        method_row(table::Lookup::Found(row)).unwrap().name,
        row.name
    );
    assert_eq!(
        method_row(table::Lookup::Removed(row)).unwrap_err().code,
        Code::RemovedMethod
    );
    assert_eq!(
        method_row(table::Lookup::Unknown).unwrap_err().code,
        Code::UnknownMethod
    );
}

#[test]
fn an_answer_past_the_cap_is_refused_whole() {
    let small = json!({"a": "b"});
    assert_eq!(Reply::value(small.clone()).capped(), Reply::value(small));
    let big = Value::String("x".repeat(limits::MAX_RESULT_BYTES));
    assert_eq!(
        Reply::value(big.clone()).capped(),
        Reply::Error {
            error: ModError::new(Code::TooLarge, "The answer is too large to send.")
        }
    );
    let page = Reply::Page {
        action: PageAction::ThemeGet,
        args: big,
    };
    assert!(matches!(page.capped(), Reply::Error { .. }));
    assert!(fits_json(&json!("abc"), 5));
    assert!(!fits_json(&json!("abcd"), 5));
}

#[test]
fn a_reply_reaches_the_page_in_the_shape_it_reads() {
    let to = |r: Reply| serde_json::to_value(r).unwrap();
    assert_eq!(
        to(Reply::value(json!(1))),
        json!({"kind": "value", "value": 1})
    );
    assert_eq!(
        to(Reply::Page {
            action: PageAction::ThemeGet,
            args: Value::Null
        }),
        json!({"kind": "page", "action": "theme.get", "args": null})
    );
    assert_eq!(
        to(Reply::Error {
            error: ModError::new(Code::Paused, "p")
        }),
        json!({"kind": "error", "error": {"code": "MOD-5002", "message": "p"}})
    );
}

/// The rate bucket, through the dispatcher: a burst gets through, then the mod is refused, and
/// another mod isn't.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mod_calling_too_fast_is_refused_and_another_is_not() {
    let host = TestHost::new();
    let a = installed(&host, map_row());
    let b = installed(&host, ModRow::signed("acme.tool", "second", &["storage"]));
    let mut admitted = 0;
    let refused = loop {
        match call(&host, &a, "theme.get", Value::Null).await {
            Reply::Page { .. } => admitted += 1,
            other => break other,
        }
        assert!(admitted < 1000, "never refused");
    };
    assert_eq!(code(refused), Code::RateLimited);
    assert!(admitted >= limits::CALL_BURST, "{admitted}");
    assert!(matches!(
        call(&host, &b, "theme.get", Value::Null).await,
        Reply::Page { .. }
    ));
}

/// A gate a hook waits at until the test opens it.
#[derive(Default)]
struct Gate {
    open: Mutex<bool>,
    waiting: Mutex<usize>,
    changed: Condvar,
}

impl Gate {
    fn wait(&self) {
        *self.waiting.lock().unwrap() += 1;
        self.changed.notify_all();
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.changed.wait(open).unwrap();
        }
    }

    fn waiting(&self) -> usize {
        *self.waiting.lock().unwrap()
    }

    fn open(&self) {
        *self.open.lock().unwrap() = true;
        self.changed.notify_all();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn at_most_sixteen_calls_of_a_mod_run_at_once() {
    let host = TestHost::new();
    let s = installed(&host, map_row());
    let gate = Arc::new(Gate::default());
    let held = Arc::clone(&gate);
    host.hook(move |_, point| {
        if point == Point::BeforeData {
            held.wait();
        }
    });
    let mut running = Vec::new();
    for _ in 0..limits::MAX_IN_FLIGHT {
        let (host, s) = (host.clone(), s.clone());
        running.push(tokio::spawn(async move {
            call(&host, &s, "storage.get", json!({"key": "a"})).await
        }));
    }
    while gate.waiting() < limits::MAX_IN_FLIGHT {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(
        host.calls().in_flight(&id("atlas.map")),
        limits::MAX_IN_FLIGHT
    );
    let e = error(call(&host, &s, "storage.get", json!({"key": "a"})).await);
    assert_eq!(e.code, Code::RateLimited);
    gate.open();
    for task in running {
        assert!(matches!(task.await.unwrap(), Reply::Value { .. }));
    }
    assert_eq!(host.calls().in_flight(&id("atlas.map")), 0);
}

/// What the ledger's uninstall does, in its order: the row's transaction, then the registry.
fn uninstall(host: &TestHost, m: &str) {
    host.sql(|c| c.execute("DELETE FROM mods WHERE id = ?1", [m]).unwrap());
    host.runtime().uninstall(&id(m));
}

/// A call in flight across an uninstall commits nothing, wherever the uninstall lands: between the
/// checks and the handler, between the handler's start and its transaction (even with only the
/// row's half done), and across an uninstall followed at once by a fresh install of the same id,
/// whose storage the old call must not reach.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_in_flight_across_an_uninstall_commits_nothing() {
    for point in [Point::Authorised, Point::BeforeData] {
        for reinstall in [false, true] {
            let host = TestHost::new();
            let s = installed(&host, map_row());
            let fired = Arc::new(Mutex::new(false));
            let once = Arc::clone(&fired);
            host.hook(move |host, at| {
                let mut fired = once.lock().unwrap();
                if at != point || *fired {
                    return;
                }
                *fired = true;
                uninstall(host, "atlas.map");
                if reinstall {
                    host.sql(|c| insert_mod(c, &map_row()));
                    serve(host, "atlas.map");
                }
            });
            let e = error(call(&host, &s, "storage.set", set_args("settings", json!(1), 0)).await);
            assert_eq!(
                e.code,
                Code::SessionEnded,
                "{point:?} reinstall {reinstall}"
            );
            assert!(*fired.lock().unwrap());
            assert!(
                kv_rows(&host, "atlas.map").is_empty(),
                "{point:?} {reinstall}"
            );
        }
    }
    // Only the row's half of the uninstall done when the handler looks: no row, no write.
    let host = TestHost::new();
    let s = installed(&host, map_row());
    host.hook(|host, at| {
        if at == Point::BeforeData {
            host.sql(|c| c.execute("DELETE FROM mods", []).unwrap());
        }
    });
    let e = error(call(&host, &s, "storage.set", set_args("settings", json!(1), 0)).await);
    assert_eq!(e.code, Code::SessionEnded);
    assert!(kv_rows(&host, "atlas.map").is_empty());
}

/// The ledger's store over a test host's open store, with no vault to compare (the host has none).
struct OverHost<'a>(&'a AppState);

impl crate::mods::ledger::LedgerStore for OverHost<'_> {
    fn with_conn<T>(
        &self,
        _layout: &crate::mods::disk::Layout,
        f: impl FnOnce(&mut rusqlite::Connection) -> Result<T, crate::mods::ledger::LedgerError>,
    ) -> Result<T, crate::mods::ledger::LedgerError> {
        let mut conn = self
            .0
            .conn()
            .map_err(|e| crate::mods::ledger::LedgerError::Store(e.to_string()))?;
        f(&mut conn)
    }
}

/// The signed list's blacklist applied to the host's mods (the real `Ledger::apply_blacklist`: cut
/// the registry, write the row, remove the files), naming `atlas.map` 1.0.0 with `action`.
fn blacklist(host: &TestHost, action: &str) {
    let layout = crate::mods::disk::Layout::new(
        host.dir(),
        "11111111-2222-3333-4444-555555555555",
        &crate::mods::disk::new_store_id().unwrap(),
    )
    .unwrap();
    let index = crate::mods::safety::tests::index_with(|v| {
        v["blacklist"] = json!([{"id": "atlas.map", "versions": "=1.0.0", "action": action,
            "reason": "A reason."}]);
    });
    let store = OverHost(host.store());
    let report = crate::mods::ledger::Ledger::new(&layout, &store, host.runtime())
        .apply_blacklist(&index, crate::mods::safety::tests::now());
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(report.notices.len(), 1, "the list applied to the mod");
}

/// A call in flight across the safety list's pause or removal commits nothing, wherever the list
/// lands: after the checks passed, or just before the handler reaches the store. The cut ends the
/// session, and the handler's own reads see the row's new state; either alone stops the write.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_in_flight_across_the_blacklist_commits_nothing() {
    for action in ["paused", "removed"] {
        for point in [Point::Authorised, Point::BeforeData] {
            let host = TestHost::new();
            let s = installed(&host, map_row());
            let fired = Arc::new(Mutex::new(false));
            let once = Arc::clone(&fired);
            let act = action.to_owned();
            host.hook(move |host, at| {
                let mut fired = once.lock().unwrap();
                if at != point || *fired {
                    return;
                }
                *fired = true;
                // Not under the DB guard at these two points, so the ledger may take it.
                blacklist(host, &act);
            });
            let e = error(call(&host, &s, "storage.set", set_args("settings", json!(1), 0)).await);
            assert!(
                matches!(e.code, Code::SessionEnded | Code::Paused),
                "{action} at {point:?}: {:?}",
                e.code
            );
            assert!(*fired.lock().unwrap(), "{action} at {point:?}");
            assert!(
                kv_rows(&host, "atlas.map").is_empty(),
                "{action} at {point:?}: nothing was written"
            );
            // The mod is stopped for good: a new call on the old session, and a new frame, both fail.
            host.hook(|_, _| {});
            assert_eq!(
                code(call(&host, &s, "storage.get", json!({"key": "settings"})).await),
                Code::SessionEnded
            );
            assert!(host
                .runtime()
                .open_frame_in(
                    &id("atlas.map"),
                    host.store().store_epoch(),
                    crate::mods::outbox::testing::discarding()
                )
                .is_err());
        }
    }
}

/// A pause written while a call is in flight stops it before its write, even before the pause's
/// writer has told the registry; and a bump between the write and the commit (the app lock, a
/// pause's registry half) rolls the write back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pause_or_a_lock_in_flight_commits_nothing() {
    let host = TestHost::new();
    let s = installed(&host, map_row());
    assert_eq!(
        value(call(&host, &s, "storage.set", set_args("settings", json!(1), 0)).await),
        json!({"revision": 1})
    );
    host.hook(|host, at| {
        if at == Point::BeforeData {
            host.sql(|c| c.execute("UPDATE mods SET state = 'paused'", []).unwrap());
        }
    });
    let e = error(call(&host, &s, "storage.set", set_args("settings", json!(2), 1)).await);
    assert_eq!(e.code, Code::Paused);
    host.sql(|c| c.execute("UPDATE mods SET state = 'active'", []).unwrap());

    type Event = (&'static str, fn(&ModRuntime));
    let events: [Event; 2] = [
        ("lock", ModRuntime::bump_all),
        ("pause", |rt| rt.pause(&id("atlas.map"))),
    ];
    for (what, event) in events {
        let host = TestHost::new();
        let s = installed(&host, map_row());
        host.hook(move |host, at| {
            // The DB guard is held here: only the registry may change.
            if at == Point::BeforeCommit {
                event(host.runtime());
            }
        });
        let e = error(call(&host, &s, "storage.set", set_args("settings", json!(1), 0)).await);
        assert_eq!(e.code, Code::SessionEnded, "{what}");
        assert!(
            kv_rows(&host, "atlas.map").is_empty(),
            "{what}: rolled back"
        );
    }
    // A read in flight across a lock answers nothing either.
    let host = TestHost::new();
    let s = installed(&host, map_row());
    host.hook(|host, at| {
        if at == Point::Authorised {
            host.runtime().bump_all();
        }
    });
    assert_eq!(
        code(call(&host, &s, "storage.get", json!({"key": "settings"})).await),
        Code::SessionEnded
    );
}

/// The lock enabled between the dispatcher's lock check and the rest of its checks: enabling writes
/// its flag and then moves every session to a new generation, so the call (whose ticket was taken
/// before the read) is stopped by the generation. With the lock read first, the call took its
/// ticket AFTER the bump and nothing ever stopped it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lock_enabled_just_after_the_lock_check_stops_the_call() {
    let host = TestHost::new();
    let s = installed(&host, map_row());
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    host.hook(move |host, at| {
        if at == Point::LockRead && !fired.swap(true, Ordering::SeqCst) {
            // set_app_lock: the flag first, then every session to a new generation.
            host.0.locked.store(true, Ordering::SeqCst);
            host.runtime().bump_all();
        }
    });
    let reply = call(&host, &s, "storage.set", set_args("settings", json!(1), 0)).await;
    assert!(
        matches!(error(reply).code, Code::SessionEnded | Code::Locked),
        "the call must not run"
    );
    assert!(kv_rows(&host, "atlas.map").is_empty());
}

/// A store swapped under a call in flight (the epoch moves while the guard is free): the call
/// reaches neither store.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_in_flight_across_a_store_swap_touches_nothing() {
    let host = TestHost::new();
    let s = installed(&host, map_row());
    host.hook(|host, at| {
        if at == Point::BeforeData {
            // Take the store and put one back: `take_conn` moves the epoch as every swap does.
            let conn = host.store().take_conn().unwrap().unwrap();
            *host.store().db.lock().unwrap() = Some(conn);
        }
    });
    let e = error(call(&host, &s, "storage.set", set_args("settings", json!(1), 0)).await);
    assert_eq!(e.code, Code::SessionEnded);
    assert!(kv_rows(&host, "atlas.map").is_empty());
    // The session belongs to the old store for good.
    host.hook(|_, _| {});
    assert_eq!(
        code(call(&host, &s, "theme.get", Value::Null).await),
        Code::SessionEnded
    );
    // A frame opened over the store now works.
    let fresh = session(&host, "atlas.map");
    assert!(matches!(
        call(&host, &fresh, "theme.get", Value::Null).await,
        Reply::Page { .. }
    ));
}

/// A table for the D3 and engine tests: the real storage handlers, needing a read-class and an
/// egress-class permission instead of `storage`.
const RELABELLED: &[Row] = &[
    Row {
        name: "storage.get",
        kind: Kind::Call,
        permission: Some("documents.meta"),
        class: permissions::Class::Read,
        added: (1, 0),
        deprecated: None,
        removed: None,
        experimental: false,
    },
    Row {
        name: "storage.set",
        kind: Kind::Call,
        permission: Some("internet"),
        class: permissions::Class::Egress,
        added: (1, 0),
        deprecated: None,
        removed: None,
        experimental: false,
    },
];

const READ: &str = "storage.get";
const EGRESS: &str = "storage.set";

/// A read-class call (`storage.get` relabelled), and an egress-class one (`storage.set`).
async fn read(host: &TestHost, session: &str) -> Reply {
    call_in(host, RELABELLED, session, READ, json!({"key": "k"})).await
}

async fn egress(host: &TestHost, session: &str) -> Reply {
    // Each write reads the key's revision first, so it never meets a stale one.
    let current = value(call_in(host, &RELABELLED_GET, session, READ, json!({"key": "k"})).await)
        ["revision"]
        .as_i64()
        .unwrap();
    call_in(
        host,
        RELABELLED,
        session,
        EGRESS,
        set_args("k", json!(1), current),
    )
    .await
}

/// `storage.get` under `storage`, so `egress` can learn a revision without a read grant.
static RELABELLED_GET: [Row; 1] = [Row {
    name: "storage.get",
    kind: Kind::Call,
    permission: Some("storage"),
    class: permissions::Class::Local,
    added: (1, 0),
    deprecated: None,
    removed: None,
    experimental: false,
}];

fn grant(host: &TestHost, m: &str, permission: &str, decision: &str) {
    host.sql(|c| {
        c.execute(
            "INSERT INTO mod_grants(mod_id, permission, decision) VALUES (?1, ?2, ?3) \
             ON CONFLICT(mod_id, permission) DO UPDATE SET decision = ?3",
            params![m, permission, decision],
        )
        .unwrap()
    });
}

/// D3 through the dispatcher, across two unsigned mods: one's read refuses the other's internet
/// and the reverse, a pair stored anyway (a restore, a hand edit) refuses both, an unasked read
/// next to a held egress is refused rather than prompted, and signed mods are never judged by it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsigned_mods_are_judged_together_on_every_call() {
    let host = TestHost::new();
    let both = ["storage", "documents.meta", "internet"];
    let reader = installed(&host, ModRow::unsigned("acme.reader", &both));
    let poster = installed(&host, ModRow::unsigned("acme.poster", &both));
    let signed = installed(&host, ModRow::signed("acme.signed", "second", &both));

    // Nothing granted: an unsigned read is asked for at first use.
    let e = error(read(&host, &reader).await);
    assert_eq!(
        (e.code, e.permission),
        (Code::NeedsPrompt, Some("documents.meta"))
    );
    grant(&host, "acme.reader", "documents.meta", "granted");
    assert!(matches!(read(&host, &reader).await, Reply::Value { .. }));
    // The other unsigned mod's internet, even granted by hand, is refused while a read is held.
    grant(&host, "acme.poster", "internet", "granted");
    let e = error(egress(&host, &poster).await);
    assert_eq!((e.code, e.permission), (Code::NotGranted, Some("internet")));
    // The frame is told nothing about the other side: the exclusion's refusal is the very text of
    // a permission the user turned off, so a mod can't tell the two apart (D3 exists so one unsigned
    // mod's grants aren't another's business).
    let exclusion = e.message.clone();
    assert!(!exclusion.contains("another"), "{exclusion}");
    // ...and with both held, the reader's read is refused as well.
    assert_eq!(code(read(&host, &reader).await), Code::NotGranted);
    // Once the read is denied, internet works, and a fresh read is refused, not prompted.
    grant(&host, "acme.reader", "documents.meta", "denied");
    let denied = error(read(&host, &reader).await);
    assert_eq!(
        (denied.code, denied.message),
        (Code::NotGranted, exclusion),
        "an exclusion reads exactly as a denial"
    );
    assert!(matches!(egress(&host, &poster).await, Reply::Value { .. }));
    let e = error(read(&host, &poster).await);
    assert_eq!(
        (e.code, e.permission),
        (Code::NotGranted, Some("documents.meta"))
    );
    // A signed mod reads and reaches out whatever unsigned mods hold.
    grant(&host, "acme.signed", "documents.meta", "granted");
    grant(&host, "acme.signed", "internet", "granted");
    assert!(matches!(read(&host, &signed).await, Reply::Value { .. }));
    assert!(matches!(egress(&host, &signed).await, Reply::Value { .. }));
}

/// The sealed-engine gate through the dispatcher: no unsigned read while the engine is unsealed,
/// granted or not; signed reads and unsigned egress go on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unsealed_engine_refuses_unsigned_reads_only() {
    let host = TestHost::new();
    let unsigned = installed(&host, ModRow::unsigned("acme.free", &["documents.meta"]));
    let signed = installed(
        &host,
        ModRow::signed("acme.signed", "second", &["documents.meta"]),
    );
    let poster = installed(
        &host,
        ModRow::unsigned("acme.poster", &["storage", "internet"]),
    );
    grant(&host, "acme.free", "documents.meta", "granted");
    grant(&host, "acme.signed", "documents.meta", "granted");
    host.0.sealed.store(false, Ordering::SeqCst);
    let e = error(read(&host, &unsigned).await);
    assert_eq!(
        (e.code, e.permission),
        (Code::NotGranted, Some("documents.meta"))
    );
    assert!(e.message.contains("sealed"), "{}", e.message);
    assert!(matches!(read(&host, &signed).await, Reply::Value { .. }));
    // An unsigned egress isn't a read; it is still D3-judged (the read grant above is held), so
    // that goes first.
    grant(&host, "acme.free", "documents.meta", "denied");
    grant(&host, "acme.poster", "internet", "granted");
    assert!(matches!(egress(&host, &poster).await, Reply::Value { .. }));
    // Sealed again: the unsigned read works once granted.
    host.0.sealed.store(true, Ordering::SeqCst);
    grant(&host, "acme.poster", "internet", "denied");
    grant(&host, "acme.free", "documents.meta", "granted");
    assert!(matches!(read(&host, &unsigned).await, Reply::Value { .. }));
}

/// The grant can change under a call in flight: a permission denied between the checks and the
/// write stops it there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_grant_withdrawn_in_flight_stops_the_write() {
    let host = TestHost::new();
    let s = installed(&host, map_row());
    host.hook(|host, at| {
        if at == Point::BeforeData {
            grant(host, "atlas.map", "storage", "denied");
        }
    });
    let e = error(call(&host, &s, "storage.set", set_args("settings", json!(1), 0)).await);
    assert_eq!((e.code, e.permission), (Code::NotGranted, Some("storage")));
    assert!(kv_rows(&host, "atlas.map").is_empty());
}

// ---- the Map's calls (part 3): documents, vectors, the reader, per-document storage ----

/// A first-party Map with both read permissions on record (as install consent writes them).
fn mapper(host: &TestHost) -> String {
    let s = installed(
        host,
        ModRow {
            keys: vec![("layout", "cache"), ("settings", "content")],
            ..ModRow::signed(
                "atlas.map",
                "first",
                &["storage", "documents.meta", "documents.vectors"],
            )
        },
    );
    grant(host, "atlas.map", "documents.meta", "granted");
    grant(host, "atlas.map", "documents.vectors", "granted");
    s
}

/// A document whose leaf chunks are constant vectors of each value in `leaves`, carrying secrets in
/// every column a mod must never see.
fn document(host: &TestHost, id: i64, importance: Option<&str>, leaves: &[f64]) {
    host.sql(|c| {
        let dim = crate::db::vec0_dim(c).unwrap();
        c.execute(
            "INSERT INTO documents(id, vault_path, title, content_hash, project, importance, \
                                   source_path, external_ref, source_id, source_parent_folder_name) \
             VALUES (?1, ?2, ?3, ?2, 'Atlas', ?4, '/home/me/secret.md', 'https://drive.example/x', \
                     'gdrive:me@example.com:f' || ?1, 'Private folder')",
            params![id, format!("d{id}.md"), format!("Doc {id}"), importance],
        )
        .unwrap();
        for (ordinal, v) in leaves.iter().enumerate() {
            c.execute(
                "INSERT INTO chunks(document_id, ordinal, content, char_count, kind) \
                 VALUES (?1, ?2, 'c', 1, 'leaf')",
                params![id, ordinal as i64],
            )
            .unwrap();
            let chunk = c.last_insert_rowid();
            let json = format!("[{}]", vec![v.to_string(); dim].join(","));
            c.execute(
                "INSERT INTO chunk_vec(rowid, embedding) VALUES (?1, ?2)",
                params![chunk, json],
            )
            .unwrap();
        }
    });
}

fn doc_rows(host: &TestHost, m: &str) -> Vec<(i64, String, String)> {
    host.sql(|c| {
        let mut stmt = c
            .prepare(
                "SELECT document_id, key, value FROM mod_doc_data WHERE mod_id = ?1 \
                 ORDER BY document_id, key",
            )
            .unwrap();
        stmt.query_map([m], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    })
}

fn bytes(reply: Reply) -> Vec<u8> {
    match reply {
        Reply::Binary { bytes } => bytes,
        other => panic!("not bytes: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_map_reads_documents_vectors_and_opens_the_reader() {
    let host = TestHost::new();
    let s = mapper(&host);
    document(&host, 1, Some("high"), &[0.0, 0.2]);
    document(&host, 2, None, &[0.3]);
    document(&host, 3, Some("archive"), &[0.9]);
    document(&host, 4, Some("low"), &[]);

    let listed = value(
        call(
            &host,
            &s,
            "documents.list",
            json!({"fields": ["id", "title", "importance", "chunkCount"]}),
        )
        .await,
    );
    assert_eq!(
        listed,
        json!({
            "documents": [
                {"id": 1, "title": "Doc 1", "importance": "high", "chunkCount": 2},
                {"id": 2, "title": "Doc 2", "importance": null, "chunkCount": 1},
                {"id": 4, "title": "Doc 4", "importance": "low", "chunkCount": 0},
            ],
            "next": null
        }),
        "the archived document is off the list, like the core Map's"
    );

    let signature = value(call(&host, &s, "embeddings.signature", Value::Null).await);
    let dimension = host.sql(|c| crate::db::vec0_dim(c).unwrap());
    assert_eq!(
        signature["count"], 2,
        "documents with vectors, not archived"
    );
    assert_eq!(signature["dimension"], dimension);
    assert_eq!(signature["hash"].as_str().unwrap().len(), 64);
    // A re-index to the same number of chunks changes it; the same state doesn't.
    let again = value(call(&host, &s, "embeddings.signature", json!({})).await);
    assert_eq!(again, signature);
    host.sql(|c| {
        c.execute(
            "INSERT INTO chunks(document_id, ordinal, content, char_count, kind) \
             VALUES (2, 1, 'c', 1, 'leaf')",
            [],
        )
        .unwrap();
        let chunk = c.last_insert_rowid();
        c.execute(
            "INSERT INTO chunk_vec(rowid, embedding) VALUES (?1, ?2)",
            params![chunk, format!("[{}]", vec!["0.5"; dimension].join(","))],
        )
        .unwrap();
    });
    assert_ne!(
        value(call(&host, &s, "embeddings.signature", Value::Null).await)["hash"],
        signature["hash"]
    );

    let (count, dim, ids, floats) = embeddings::decode(&bytes(
        call(&host, &s, "embeddings.documentVectors", json!({})).await,
    ));
    assert_eq!((count, dim as usize), (2, dimension));
    assert_eq!(ids, vec![1.0, 2.0]);
    assert_eq!(floats.len(), 2 * dimension);
    let unit = 1.0 / (dimension as f32).sqrt();
    assert!(floats.iter().all(|x| (x - unit).abs() < 1e-5));
    // Named ids, and a cap: only what is asked and kept.
    let named = embeddings::decode(&bytes(
        call(
            &host,
            &s,
            "embeddings.documentVectors",
            json!({"ids": [2, 3, 4, 99]}),
        )
        .await,
    ));
    assert_eq!((named.0, named.2), (1, vec![2.0]));
    let capped = embeddings::decode(&bytes(
        call(&host, &s, "embeddings.documentVectors", json!({"cap": 1})).await,
    ));
    assert_eq!((capped.0, capped.2), (1, vec![1.0]), "the important one");

    // The reader: Rust authorises, the page opens.
    assert_eq!(
        call(&host, &s, "ui.openDocument", json!({"id": 2})).await,
        Reply::Page {
            action: PageAction::UiOpenDocument,
            args: json!({"id": 2})
        }
    );
}

/// Without `documents.vectors` a mod gets nothing of the vectors, and without `documents.meta`
/// nothing of the documents, whether or not the document it names is there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_caller_without_the_permission_gets_nothing() {
    let host = TestHost::new();
    document(&host, 1, None, &[0.1]);
    // Meta granted, vectors not even declared.
    let meta = installed(
        &host,
        ModRow::signed("acme.meta", "second", &["storage", "documents.meta"]),
    );
    grant(&host, "acme.meta", "documents.meta", "granted");
    for (method, args) in [
        ("embeddings.documentVectors", json!({})),
        ("embeddings.documentVectors", json!({"ids": [1]})),
        ("embeddings.signature", json!({})),
    ] {
        let e = error(call(&host, &meta, method, args).await);
        assert_eq!(
            (e.code, e.permission),
            (Code::NotGranted, Some("documents.vectors")),
            "{method}"
        );
    }
    // Vectors declared but never consented to (install policy, no row): refused, not prompted.
    let unconsented = installed(
        &host,
        ModRow::signed("acme.vec", "second", &["documents.vectors"]),
    );
    assert_eq!(
        code(call(&host, &unconsented, "embeddings.signature", json!({})).await),
        Code::NotGranted
    );
    // Vectors granted and meta not: the vectors, not the documents.
    grant(&host, "acme.vec", "documents.vectors", "granted");
    assert!(matches!(
        call(&host, &unconsented, "embeddings.signature", json!({})).await,
        Reply::Value { .. }
    ));
    for (method, args) in [
        ("documents.list", json!({"fields": ["id"]})),
        ("ui.openDocument", json!({"id": 1})),
        ("ui.openDocument", json!({"id": 999})),
        ("storage.doc.get", json!({"documentId": 1, "key": "k"})),
        (
            "storage.doc.set",
            json!({"key": "k", "entries": [{"documentId": 1, "value": 1}]}),
        ),
        ("storage.doc.list", json!({"key": "k"})),
        ("storage.doc.delete", json!({"key": "k"})),
    ] {
        let e = error(call(&host, &unconsented, method, args).await);
        assert_eq!(
            (e.code, e.permission),
            (Code::NotGranted, Some("documents.meta")),
            "{method}"
        );
    }
    assert!(doc_rows(&host, "acme.vec").is_empty());
    // A denial on record wins over a policy that would allow it.
    grant(&host, "acme.meta", "documents.meta", "denied");
    assert_eq!(
        code(call(&host, &meta, "documents.list", json!({"fields": ["id"]})).await),
        Code::NotGranted
    );
}

/// A document that isn't there and one the mod can't read (shelved) get the same error, byte for
/// byte, from every call that takes one, and `ui.openDocument` opens neither.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_and_an_unreadable_document_are_the_same_error() {
    let host = TestHost::new();
    let s = mapper(&host);
    document(&host, 1, None, &[0.1]);
    document(&host, 2, Some("archive"), &[0.1]);
    let calls = |id: i64| {
        [
            ("ui.openDocument", json!({"id": id})),
            (
                "storage.doc.get",
                json!({"documentId": id, "key": "layout"}),
            ),
            (
                "storage.doc.set",
                json!({"key": "layout", "entries": [{"documentId": id, "value": [0, 0]}]}),
            ),
        ]
    };
    for ((method, missing), (_, shelved)) in calls(404).into_iter().zip(calls(2)) {
        let a = error(call(&host, &s, method, missing).await);
        let b = error(call(&host, &s, method, shelved).await);
        assert_eq!(a, b, "{method}");
        assert_eq!(a.code, Code::NotFound, "{method}");
        // And in the shape the frame reads.
        assert_eq!(
            serde_json::to_value(&a).unwrap(),
            serde_json::to_value(&b).unwrap()
        );
    }
    assert!(doc_rows(&host, "atlas.map").is_empty(), "nothing written");
    // The readable one answers.
    assert!(matches!(
        call(&host, &s, "ui.openDocument", json!({"id": 1})).await,
        Reply::Page { .. }
    ));
    // Malformed ids are the shape's refusal, not a lookup.
    for args in [
        json!({"id": "1"}),
        json!({"id": 1.5}),
        json!({}),
        json!({"id": 1, "x": 1}),
    ] {
        assert_eq!(
            code(call(&host, &s, "ui.openDocument", args).await),
            Code::BadArgs
        );
    }
}

/// The DTO through the dispatcher: a forbidden field named in the request is refused whole, and
/// nothing of a document's secret columns is in any answer a mod can get.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_document_list_never_carries_a_forbidden_field() {
    let host = TestHost::new();
    let s = mapper(&host);
    document(&host, 1, Some("high"), &[0.1]);
    for field in [
        "source_path",
        "sourcePath",
        "external_ref",
        "sourceId",
        "source_parent_folder_name",
        "folderPath",
        "vaultPath",
    ] {
        for fields in [json!([field]), json!(["id", "title", field])] {
            let e = error(call(&host, &s, "documents.list", json!({"fields": fields})).await);
            assert_eq!(e.code, Code::BadArgs, "{field}");
            assert!(!e.message.contains("secret"), "{}", e.message);
        }
    }
    let every = json!({"fields": [
        "id", "title", "homeProject", "importance", "chunkCount", "reviewed", "sourceType", "tags",
        "ingestedAt"
    ]});
    let all = value(call(&host, &s, "documents.list", every).await);
    let text = all.to_string();
    for leaked in [
        "secret.md",
        "drive.example",
        "me@example.com",
        "Private folder",
        "gdrive:",
        "d1.md",
    ] {
        assert!(!text.contains(leaked), "{leaked} reached a mod");
    }
    let keys: Vec<&String> = all["documents"][0].as_object().unwrap().keys().collect();
    assert_eq!(keys.len(), 9);
    // Every other answer the Map's calls give is as clean.
    for (method, args) in [
        ("storage.doc.list", json!({"key": "layout"})),
        ("embeddings.signature", json!({})),
    ] {
        let text = value(call(&host, &s, method, args).await).to_string();
        assert!(
            !text.contains("secret") && !text.contains("example"),
            "{method}"
        );
    }
    // Paging is the caller's choice and bounded.
    for limit in [0, 5001] {
        assert_eq!(
            code(
                call(
                    &host,
                    &s,
                    "documents.list",
                    json!({"fields": ["id"], "limit": limit})
                )
                .await
            ),
            Code::BadArgs
        );
    }
    assert_eq!(
        value(
            call(
                &host,
                &s,
                "documents.list",
                json!({"fields": ["id"], "after": 1})
            )
            .await
        ),
        json!({"documents": [], "next": null})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn per_document_storage_runs_end_to_end_and_goes_with_its_document() {
    let host = TestHost::new();
    let s = mapper(&host);
    for n in 1..=3 {
        document(&host, n, None, &[0.1]);
    }
    assert_eq!(
        value(
            call(
                &host,
                &s,
                "storage.doc.set",
                json!({"key": "layout", "entries": [
                    {"documentId": 1, "value": [1, 2]}, {"documentId": 3, "value": {"x": 3}}]})
            )
            .await
        ),
        json!({"written": 2})
    );
    assert_eq!(
        value(
            call(
                &host,
                &s,
                "storage.doc.get",
                json!({"documentId": 1, "key": "layout"})
            )
            .await
        ),
        json!({"found": true, "value": [1, 2]})
    );
    assert_eq!(
        value(
            call(
                &host,
                &s,
                "storage.doc.get",
                json!({"documentId": 2, "key": "layout"})
            )
            .await
        ),
        json!({"found": false, "value": null})
    );
    assert_eq!(
        value(call(&host, &s, "storage.doc.list", json!({"key": "layout"})).await),
        json!({"entries": [
            {"documentId": 1, "value": [1, 2]}, {"documentId": 3, "value": {"x": 3}}], "next": null})
    );
    // The mod's private `mod_kv` keys are another namespace.
    assert_eq!(
        value(call(&host, &s, "storage.get", json!({"key": "layout"})).await),
        json!({"value": null, "revision": 0})
    );
    // A content-class key is refused (cache only), and nothing is written.
    let e = error(
        call(
            &host,
            &s,
            "storage.doc.set",
            json!({"key": "settings", "entries": [{"documentId": 1, "value": 1}]}),
        )
        .await,
    );
    assert_eq!(e.code, Code::BadArgs);
    assert_eq!(doc_rows(&host, "atlas.map").len(), 2);
    // Argument shapes.
    for args in [
        json!({"key": "layout", "entries": []}),
        json!({"key": "layout", "entries": [{"documentId": 1, "value": 1}, {"documentId": 1, "value": 2}]}),
        json!({"key": "Layout", "entries": [{"documentId": 1, "value": 1}]}),
        json!({"key": "layout", "entries": [{"documentId": 1}]}),
        json!({"key": "layout", "entries": [{"documentId": 1, "value": 1, "x": 2}]}),
    ] {
        assert_eq!(
            code(call(&host, &s, "storage.doc.set", args.clone()).await),
            Code::BadArgs,
            "{args}"
        );
    }
    // The document goes (a delete, or a Rebuild's re-creation, with no frame needing to be told):
    // its rows go with it.
    host.sql(|c| c.execute("DELETE FROM documents WHERE id = 1", []).unwrap());
    assert_eq!(doc_rows(&host, "atlas.map").len(), 1);
    assert_eq!(
        code(
            call(
                &host,
                &s,
                "storage.doc.get",
                json!({"documentId": 1, "key": "layout"})
            )
            .await
        ),
        Code::NotFound
    );
    assert_eq!(
        value(call(&host, &s, "storage.doc.list", json!({"key": "layout"})).await),
        json!({"entries": [{"documentId": 3, "value": {"x": 3}}], "next": null})
    );
    // Delete by id, then the whole key.
    assert_eq!(
        value(
            call(
                &host,
                &s,
                "storage.doc.delete",
                json!({"key": "layout", "documentIds": [2, 3]})
            )
            .await
        ),
        json!({"deleted": 1})
    );
    assert!(doc_rows(&host, "atlas.map").is_empty());
}

/// The doc-storage write stops the way `storage.set` does: a pause, a lock or an uninstall in flight
/// writes nothing, and the quota is the one shared with `mod_kv`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_doc_write_in_flight_across_a_pause_or_a_lock_commits_nothing() {
    let write = json!({"key": "layout", "entries": [{"documentId": 1, "value": 1}]});
    type Event = (&'static str, fn(&ModRuntime));
    let events: [Event; 2] = [
        ("lock", ModRuntime::bump_all),
        ("pause", |rt| rt.pause(&id("atlas.map"))),
    ];
    for (what, event) in events {
        let host = TestHost::new();
        let s = mapper(&host);
        document(&host, 1, None, &[0.1]);
        host.hook(move |host, at| {
            if at == Point::BeforeCommit {
                event(host.runtime());
            }
        });
        let e = error(call(&host, &s, "storage.doc.set", write.clone()).await);
        assert_eq!(e.code, Code::SessionEnded, "{what}");
        assert!(doc_rows(&host, "atlas.map").is_empty(), "{what}");
    }
    // A grant withdrawn between the checks and the write.
    let host = TestHost::new();
    let s = mapper(&host);
    document(&host, 1, None, &[0.1]);
    host.hook(|host, at| {
        if at == Point::BeforeData {
            grant(host, "atlas.map", "documents.meta", "denied");
        }
    });
    for method in ["storage.doc.set", "storage.doc.delete", "documents.list"] {
        let args = match method {
            "storage.doc.set" => write.clone(),
            "storage.doc.delete" => json!({"key": "layout"}),
            _ => json!({"fields": ["id"]}),
        };
        assert_eq!(
            code(call(&host, &s, method, args).await),
            Code::NotGranted,
            "{method}"
        );
    }
    assert!(doc_rows(&host, "atlas.map").is_empty());
    // The shared quota: full through mod_kv, a doc write is refused.
    let host = TestHost::new();
    let s = mapper(&host);
    document(&host, 1, None, &[0.1]);
    host.sql(|c| {
        let filler = "x".repeat(limits::STORAGE_QUOTA_BYTES as usize - 1000);
        c.execute(
            "INSERT INTO mod_kv(mod_id, key, class, value) VALUES ('atlas.map', 'big', 'content', ?1)",
            [filler],
        )
        .unwrap();
    });
    let e = error(
        call(
            &host,
            &s,
            "storage.doc.set",
            json!({"key": "layout", "entries": [{"documentId": 1, "value": "y".repeat(2000)}]}),
        )
        .await,
    );
    assert_eq!(e.code, Code::QuotaExceeded);
    assert!(doc_rows(&host, "atlas.map").is_empty());
}

/// A binary reply is held to its own cap, is never JSON, and every other reply still is.
#[test]
fn a_binary_reply_has_its_cap_and_goes_out_raw() {
    let ok = Reply::Binary {
        bytes: vec![1; 100],
    };
    assert_eq!(ok.clone().capped(), ok);
    let big = Reply::Binary {
        bytes: vec![0; limits::MAX_BINARY_RESULT_BYTES + 1],
    };
    assert_eq!(
        big.capped(),
        Reply::Error {
            error: ModError::new(Code::TooLarge, "The answer is too large to send.")
        }
    );
    // The JSON cap does not bind bytes, and the byte cap is the spec's 20 MiB.
    const {
        assert!(limits::MAX_BINARY_RESULT_BYTES > limits::MAX_RESULT_BYTES);
        assert!(limits::MAX_BINARY_RESULT_BYTES == 20 << 20);
    }
    match (Reply::Binary { bytes: vec![7, 8] }).into_body() {
        InvokeResponseBody::Raw(b) => assert_eq!(b, vec![7, 8]),
        other => panic!("{other:?}"),
    }
    match Reply::value(json!({"a": 1})).into_body() {
        InvokeResponseBody::Json(text) => assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            json!({"kind": "value", "value": {"a": 1}})
        ),
        other => panic!("{other:?}"),
    }
}
