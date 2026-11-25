// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! One connection's rules, over `tokio::io::duplex` and the API's test host with a real store: the
//! first frame, the caps, the idle clock, what a refusal says, and that a connection that outlives
//! its hooks is still refused request by request.
//!
//! The tests that wait on the paused clock run on the current-thread runtime and pause time only AFTER
//! the blocking steps (authentication) are done: tokio's paused clock advances whenever the runtime is
//! idle, and a blocking task in flight looks idle to it. A test that has to wait while one may be in
//! flight uses a short real [`Timing`] instead.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{
    duplex, AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf,
};

use super::jsonrpc::MAX_LINE_BYTES;
use super::limits::TOO_FAST;
use super::serve::{
    read_frame, serve, Conns, Frame, Timing, MAX_AUTH_FAILURES, MAX_CONNECTIONS, MAX_PER_CLIENT,
};
use super::test_support::{open_vault, pair_app, App};
use super::SCOPES;
use crate::mods::api::test_support::TestHost;
use crate::mods::api::{Code, Host};
use crate::mods::devmode::Here;

/// A client's end of a connection.
struct Conn {
    read: BufReader<ReadHalf<DuplexStream>>,
    write: WriteHalf<DuplexStream>,
}

impl Conn {
    async fn send(&mut self, line: &str) {
        self.write.write_all(line.as_bytes()).await.unwrap();
        self.write.write_all(b"\n").await.unwrap();
    }

    async fn auth(&mut self, token: &str) {
        self.send(&json!({ "pm_auth": token }).to_string()).await;
    }

    /// The next line, or `None` when the server closed the connection.
    async fn line(&mut self) -> Option<String> {
        tokio::time::timeout(Duration::from_secs(5), self.raw_line())
            .await
            .expect("the server answered or closed in time")
    }

    /// [`line`](Self::line) with no limit of its own, for a test that brings its own (and a paused
    /// clock, which a limit of five seconds would race).
    async fn raw_line(&mut self) -> Option<String> {
        let mut text = String::new();
        let n = self.read.read_line(&mut text).await.unwrap_or(0);
        (n > 0).then(|| text.trim_end().to_owned())
    }

    /// Send a request and read its answer.
    async fn ask(&mut self, id: u64, method: &str, params: Value) -> Value {
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.send(&request.to_string()).await;
        let line = self.line().await.expect("an answer");
        serde_json::from_str(&line).unwrap()
    }

    /// Whether the server has closed the connection (nothing more will come).
    async fn is_closed(&mut self) -> bool {
        self.line().await.is_none()
    }
}

struct World {
    host: TestHost,
    here: Here,
    conns: Arc<Conns>,
}

impl World {
    fn new() -> Self {
        let host = TestHost::new();
        let here = open_vault(&host);
        Self {
            host,
            here,
            conns: Arc::new(Conns::default()),
        }
    }

    fn app(&self, scopes: &[&str]) -> App {
        pair_app(&self.host, &self.here, scopes)
    }

    /// A new connection, or `None` when the caps are full (the listener drops it).
    fn open(&self) -> Option<Conn> {
        self.open_with(Timing::default())
    }

    fn open_with(&self, timing: Timing) -> Option<Conn> {
        let slot = self.conns.admit()?;
        let (client, server) = duplex(1 << 20);
        let host = self.host.clone();
        tokio::spawn(async move { serve(&host, server, slot, timing).await });
        let (read, write) = tokio::io::split(client);
        Some(Conn {
            read: BufReader::new(read),
            write,
        })
    }

    /// A connection that has authenticated as `app`.
    async fn session(&self, app: &App) -> Conn {
        let mut c = self.open().expect("room for a connection");
        c.auth(&app.token).await;
        // An answered ping proves the server took the auth frame.
        let reply = c.ask(0, "ping", json!({})).await;
        assert!(reply.get("result").is_some(), "{reply}");
        c
    }
}

fn code_of(reply: &Value) -> &str {
    reply["error"]["data"]["code"].as_str().unwrap_or("")
}

// --- the first frame ------------------------------------------------------------------------------

#[tokio::test]
async fn a_token_serves_requests() {
    let w = World::new();
    let app = w.app(&SCOPES);
    let mut c = w.session(&app).await;
    let init = c
        .ask(1, "initialize", json!({"protocolVersion": "2025-06-18"}))
        .await;
    assert_eq!(init["result"]["serverInfo"]["name"], "Personal Manager");
    let listed = c.ask(2, "tools/list", json!({})).await;
    assert_eq!(listed["result"]["tools"].as_array().unwrap().len(), 5);
    let projects = c
        .ask(
            3,
            "tools/call",
            json!({"name": "projects", "arguments": {}}),
        )
        .await;
    assert_eq!(projects["result"]["isError"], false);
}

#[tokio::test]
async fn the_first_frame_must_be_the_auth_frame() {
    let w = World::new();
    let app = w.app(&SCOPES);
    // A request sent before any auth is not answered, and the connection closes.
    let mut c = w.open().unwrap();
    c.send(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).await;
    assert!(c.is_closed().await);
    // Not even with a token tucked into it.
    let mut c = w.open().unwrap();
    let sneaky = json!({"jsonrpc": "2.0", "id": 1, "method": "ping", "pm_auth": app.token});
    c.send(&sneaky.to_string()).await;
    assert!(c.is_closed().await);
    // An auth frame carrying anything else closes too.
    let mut c = w.open().unwrap();
    c.send(&json!({"pm_auth": app.token, "extra": 1}).to_string())
        .await;
    assert!(c.is_closed().await);
    // And the connection that did it right still works.
    let mut ok = w.session(&app).await;
    assert!(ok.ask(9, "ping", json!({})).await.get("result").is_some());
}

#[tokio::test]
async fn a_wrong_token_closes_with_no_word() {
    let w = World::new();
    let app = w.app(&SCOPES);
    let mut wrong = app.token.clone();
    let last = wrong.pop().unwrap();
    wrong.push(if last == '0' { '1' } else { '0' });
    let other_id = app.token.replacen(
        &format!("pmcp_{}_", app.id),
        &format!("pmcp_{}_", app.id + 1),
        1,
    );
    for token in [
        wrong.as_str(),
        other_id.as_str(),
        "pmcp_1_nothex",
        "",
        "not a token",
    ] {
        let mut c = w.open().unwrap();
        c.auth(token).await;
        // Not a byte comes back: the connection simply ends.
        assert!(c.is_closed().await, "{token:?}");
    }
}

/// A token shaped like PM's, for an app this device never paired.
fn unpaired_token(app: &App) -> String {
    app.token.replacen(
        &format!("pmcp_{}_", app.id),
        &format!("pmcp_{}_", app.id + 1),
        1,
    )
}

/// `app`'s token with its last character changed: shaped right, wrong secret.
fn wrong_secret(app: &App) -> String {
    let mut wrong = app.token.clone();
    let last = wrong.pop().unwrap();
    wrong.push(if last == '0' { '1' } else { '0' });
    wrong
}

#[tokio::test]
async fn a_guess_never_waits_on_the_stores_one_guard() {
    let w = World::new();
    let app = w.app(&SCOPES);
    // Somebody holds the guard (the app's own command, say): a connection that is still guessing
    // must not queue behind it, or a flood of guesses would stall the person's own commands in turn.
    let guard = w.host.store().conn().unwrap();
    for token in [unpaired_token(&app), wrong_secret(&app)] {
        let mut c = w.open().unwrap();
        c.auth(&token).await;
        assert!(c.is_closed().await, "{token}");
    }
    drop(guard);
    // The paired token is looked at the usual way once the guard is free.
    let mut ok = w.session(&app).await;
    assert!(ok.ask(1, "ping", json!({})).await.get("result").is_some());
}

#[tokio::test]
async fn a_failed_token_is_held_before_it_is_closed() {
    let w = World::new();
    let app = w.app(&SCOPES);
    let mut c = w.open().unwrap();
    let began = std::time::Instant::now();
    c.auth(&wrong_secret(&app)).await;
    assert!(c.is_closed().await);
    assert!(
        began.elapsed() >= Duration::from_millis(200),
        "closed after {:?}",
        began.elapsed()
    );
    // A token that was right is not held at all.
    let began = std::time::Instant::now();
    let mut ok = w.session(&app).await;
    assert!(ok.ask(2, "ping", json!({})).await.get("result").is_some());
    assert!(began.elapsed() < Duration::from_millis(200));
}

#[tokio::test]
async fn a_run_of_wrong_guesses_shuts_the_door_even_on_the_right_token() {
    let w = World::new();
    let app = w.app(&SCOPES);
    // No hold, so the run is quick: the count is what is under test.
    let quick = Timing {
        fail: Duration::ZERO,
        ..Timing::default()
    };
    let wrong = wrong_secret(&app);
    for _ in 0..MAX_AUTH_FAILURES {
        let mut c = w.open_with(quick).unwrap();
        c.auth(&wrong).await;
        assert!(c.is_closed().await);
    }
    // The door is shut for the rest of the window: the right token is closed without a word, though it
    // would have been served a moment ago.
    let mut c = w.open_with(quick).unwrap();
    c.auth(&app.token).await;
    assert!(c.is_closed().await);
}

#[test]
fn the_gate_counts_failures_in_a_window_and_starts_a_new_one() {
    use super::serve::AuthGate;
    let start = std::time::Instant::now();
    let mut gate = AuthGate::default();
    assert!(gate.open(start));
    for _ in 0..MAX_AUTH_FAILURES - 1 {
        gate.failed(start);
    }
    assert!(gate.open(start), "one under the limit is still open");
    gate.failed(start);
    assert!(!gate.open(start));
    assert!(
        !gate.open(start + Duration::from_secs(9)),
        "still the same window"
    );
    // The window runs out and the count with it.
    assert!(gate.open(start + Duration::from_secs(10)));
    gate.failed(start + Duration::from_secs(10));
    assert!(gate.open(start + Duration::from_secs(11)));
}

#[tokio::test]
async fn a_revoked_or_switched_off_app_cannot_authenticate() {
    let w = World::new();
    let app = w.app(&SCOPES);
    super::device::set_on(&w.here, false).unwrap();
    let mut c = w.open().unwrap();
    c.auth(&app.token).await;
    assert!(c.is_closed().await);
    super::device::set_on(&w.here, true).unwrap();
    w.host
        .sql(|conn| super::clients::revoke(conn, &w.here, app.id))
        .unwrap();
    let mut c = w.open().unwrap();
    c.auth(&app.token).await;
    assert!(c.is_closed().await);
}

#[tokio::test]
async fn a_locked_pm_authenticates_nobody() {
    let w = World::new();
    let app = w.app(&SCOPES);
    w.host.0.locked.store(true, Ordering::SeqCst);
    let mut c = w.open().unwrap();
    c.auth(&app.token).await;
    assert!(c.is_closed().await);
}

// --- sizes and clocks -----------------------------------------------------------------------------

#[tokio::test]
async fn a_frame_past_the_cap_closes_the_connection() {
    let w = World::new();
    let app = w.app(&SCOPES);
    let mut c = w.session(&app).await;
    // One byte past the cap and no newline: the server gives up without waiting for one. (The last
    // bytes may meet a closed pipe, which is the point.)
    let big = vec![b'a'; MAX_LINE_BYTES + 1];
    let _ = c.write.write_all(&big).await;
    assert!(c.is_closed().await);
    // The auth frame has a far smaller cap of its own.
    let mut c = w.open().unwrap();
    let long_token = format!("pmcp_1_{}", "a".repeat(4096));
    c.auth(&long_token).await;
    assert!(c.is_closed().await);
}

#[tokio::test]
async fn a_line_of_exactly_the_cap_is_still_read() {
    let line = "x".repeat(100);
    let data = format!("{line}\n").into_bytes();
    let mut reader = BufReader::new(&data[..]);
    assert_eq!(
        read_frame(&mut reader, 100).await.unwrap(),
        Frame::Line(line.clone())
    );
    let mut reader = BufReader::new(&data[..]);
    assert_eq!(read_frame(&mut reader, 99).await.unwrap(), Frame::TooLong);
}

#[tokio::test]
async fn the_reader_never_pulls_far_past_the_cap() {
    // A writer that never stops, into a tiny pipe: the reader stops pulling when the line is over
    // the cap, so the writer can't get further than the cap plus what is in flight.
    let (mut writer, reader) = duplex(1024);
    let written = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&written);
    let pump = tokio::spawn(async move {
        let chunk = [b'a'; 4096];
        while writer.write_all(&chunk).await.is_ok() {
            counter.fetch_add(chunk.len(), Ordering::SeqCst);
            tokio::task::yield_now().await;
        }
    });
    let mut reader = BufReader::with_capacity(8 * 1024, reader);
    let cap = 100_000;
    assert_eq!(read_frame(&mut reader, cap).await.unwrap(), Frame::TooLong);
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    // The cap, the reader's buffer, the pipe and one chunk of slack.
    assert!(written.load(Ordering::SeqCst) <= cap + 8 * 1024 + 1024 + 2 * 4096);
    pump.abort();
}

#[tokio::test]
async fn crlf_is_stripped_and_text_must_be_utf8_and_a_half_line_is_nothing() {
    let mut r = BufReader::new(&b"abc\r\n"[..]);
    assert_eq!(
        read_frame(&mut r, 10).await.unwrap(),
        Frame::Line("abc".into())
    );
    let mut r = BufReader::new(&b"\xff\xfe\n"[..]);
    assert_eq!(read_frame(&mut r, 10).await.unwrap(), Frame::NotText);
    let mut r = BufReader::new(&b"no newline"[..]);
    assert_eq!(read_frame(&mut r, 100).await.unwrap(), Frame::Eof);
    let mut r = BufReader::new(&b""[..]);
    assert_eq!(read_frame(&mut r, 100).await.unwrap(), Frame::Eof);
}

#[tokio::test]
async fn a_connection_that_never_authenticates_is_closed_in_five_seconds() {
    let w = World::new();
    let mut c = w.open().unwrap();
    tokio::time::pause();
    // The test's own limit is far past PM's: only the server's clock can end this in time.
    let closed = tokio::time::timeout(Duration::from_secs(120), c.raw_line()).await;
    assert!(matches!(closed, Ok(None)), "{closed:?}");
    // And one that dribbles a first frame without finishing it is no better off.
    let mut c = w.open().unwrap();
    c.write.write_all(br#"{"pm_auth":"#).await.unwrap();
    let closed = tokio::time::timeout(Duration::from_secs(120), c.raw_line()).await;
    assert!(matches!(closed, Ok(None)), "{closed:?}");
}

#[tokio::test]
async fn a_silent_connection_is_closed_after_the_idle_time() {
    let w = World::new();
    let app = w.app(&SCOPES);
    let mut c = w.session(&app).await;
    // Authenticated and served; now nothing for as long as PM waits.
    tokio::time::pause();
    let closed = tokio::time::timeout(Duration::from_secs(120), c.raw_line()).await;
    assert!(matches!(closed, Ok(None)), "{closed:?}");
    assert_eq!(w.conns.count(), 0, "its place is free again");
}

#[tokio::test]
async fn a_client_that_never_reads_cannot_hold_its_place() {
    // A pipe too small for one answer, and a client that sends requests and never reads.
    let w = World::new();
    let app = w.app(&SCOPES);
    let slot = w.conns.admit().unwrap();
    let (client, server) = duplex(256);
    let host = w.host.clone();
    let timing = Timing {
        auth: Duration::from_secs(5),
        idle: Duration::from_millis(300),
        fail: Duration::from_millis(250),
    };
    tokio::spawn(async move { serve(&host, server, slot, timing).await });
    let (read, mut write) = tokio::io::split(client);
    let _read = read; // held open, never read
    write
        .write_all(json!({"pm_auth": app.token}).to_string().as_bytes())
        .await
        .unwrap();
    write.write_all(b"\n").await.unwrap();
    write
        .write_all(br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
        .await
        .unwrap();
    write.write_all(b"\n").await.unwrap();
    let mut waited = 0;
    while w.conns.count() > 0 && waited < 500 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        waited += 1;
    }
    assert_eq!(w.conns.count(), 0, "the connection was given up on");
}

// --- the caps -------------------------------------------------------------------------------------

#[tokio::test]
async fn at_most_four_connections_and_an_unauthenticated_one_counts() {
    let w = World::new();
    let mut held: Vec<Conn> = (0..MAX_CONNECTIONS).map(|_| w.open().unwrap()).collect();
    assert_eq!(w.conns.count(), MAX_CONNECTIONS);
    // Nobody has said a word and the fifth is already turned away.
    assert!(w.open().is_none());
    // A place frees when its connection ends.
    let mut gone = held.pop().unwrap();
    gone.write.shutdown().await.unwrap();
    drop(gone);
    for _ in 0..200 {
        if w.conns.count() < MAX_CONNECTIONS {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(w.open().is_some());
}

#[tokio::test]
async fn an_app_holds_at_most_two_connections() {
    let w = World::new();
    let app = w.app(&SCOPES);
    let mut first = w.session(&app).await;
    let mut second = w.session(&app).await;
    assert_eq!(MAX_PER_CLIENT, 2);
    // The third authenticates fine and is closed.
    let mut third = w.open().unwrap();
    third.auth(&app.token).await;
    assert!(third.is_closed().await);
    // The first two are untouched, and another app is not held to this app's count.
    assert!(first
        .ask(1, "ping", json!({}))
        .await
        .get("result")
        .is_some());
    assert!(second
        .ask(1, "ping", json!({}))
        .await
        .get("result")
        .is_some());
    let other = w.app(&["tasks.read"]);
    let mut c = w.session(&other).await;
    assert!(c.ask(1, "ping", json!({})).await.get("result").is_some());
}

#[tokio::test]
async fn an_apps_flood_is_told_the_fixed_sentence() {
    let w = World::new();
    let app = w.app(&SCOPES);
    let mut c = w.session(&app).await;
    // Far past the burst, written before any answer is read.
    let n = 400;
    for id in 1..=n {
        let request = json!({"jsonrpc": "2.0", "id": id, "method": "tools/list"});
        c.send(&request.to_string()).await;
    }
    let mut refused = 0;
    for _ in 0..n {
        let reply: Value = serde_json::from_str(&c.line().await.unwrap()).unwrap();
        if let Some(message) = reply["error"]["message"].as_str() {
            assert_eq!(message, TOO_FAST);
            assert_eq!(code_of(&reply), Code::RateLimited.as_str());
            assert!(!message.to_lowercase().contains("mod"));
            refused += 1;
        }
    }
    assert!(refused > 0, "some of {n} requests in a burst are refused");
}

#[tokio::test]
async fn an_unknown_tool_and_a_scope_that_is_not_held_answer_alike() {
    let w = World::new();
    let app = w.app(&["tasks.read"]);
    let mut c = w.session(&app).await;
    let unknown = c
        .ask(1, "tools/call", json!({"name": "nope", "arguments": {}}))
        .await;
    let hidden = c
        .ask(2, "tools/call", json!({"name": "events", "arguments": {}}))
        .await;
    assert_eq!(unknown["error"], hidden["error"]);
    let listed = c.ask(3, "tools/list", json!({})).await;
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["tasks"]);
}

// --- the guarantee: a connection that outlives every hook is refused request by request -----------

#[tokio::test]
async fn a_locked_pm_refuses_an_open_connection_with_no_hook_to_help() {
    let w = World::new();
    let app = w.app(&SCOPES);
    let mut c = w.session(&app).await;
    let call = json!({"name": "projects", "arguments": {}});
    assert!(c
        .ask(1, "tools/call", call.clone())
        .await
        .get("result")
        .is_some());
    // No hook runs: the flag alone.
    w.host.0.locked.store(true, Ordering::SeqCst);
    let reply = c.ask(2, "tools/call", call.clone()).await;
    assert_eq!(code_of(&reply), Code::Locked.as_str(), "{reply}");
    let listed = c.ask(3, "tools/list", json!({})).await;
    assert_eq!(code_of(&listed), Code::Locked.as_str(), "{listed}");
    w.host.0.locked.store(false, Ordering::SeqCst);
    assert!(c.ask(4, "tools/call", call).await.get("result").is_some());
}

#[tokio::test]
async fn a_moved_store_refuses_an_open_connection_with_no_hook_to_help() {
    let w = World::new();
    let app = w.app(&SCOPES);
    let mut c = w.session(&app).await;
    let call = json!({"name": "projects", "arguments": {}});
    assert!(c
        .ask(1, "tools/call", call.clone())
        .await
        .get("result")
        .is_some());
    // The same vault's store closed and opened again (a lock cycle, a restore): a new epoch.
    let conn = w.host.store().take_conn().unwrap().unwrap();
    let (runtime, _, _) = super::test_support::vault_runtime(&w.host);
    w.host.store().open_session(conn, runtime).unwrap();
    let reply = c.ask(2, "tools/call", call).await;
    assert_eq!(code_of(&reply), Code::SessionEnded.as_str(), "{reply}");
}

#[tokio::test]
async fn a_revoked_app_is_refused_on_a_connection_nothing_closed() {
    let w = World::new();
    let app = w.app(&SCOPES);
    let mut c = w.session(&app).await;
    w.host
        .sql(|conn| super::clients::revoke(conn, &w.here, app.id))
        .unwrap();
    let reply = c
        .ask(
            1,
            "tools/call",
            json!({"name": "projects", "arguments": {}}),
        )
        .await;
    assert_eq!(code_of(&reply), Code::NotGranted.as_str(), "{reply}");
}

// --- the hooks: a connection ends early -----------------------------------------------------------

#[tokio::test]
async fn closing_all_ends_every_connection_even_one_that_never_spoke() {
    let w = World::new();
    let app = w.app(&SCOPES);
    let mut a = w.session(&app).await;
    let mut unauthenticated = w.open().unwrap();
    w.conns.close_all();
    assert!(a.is_closed().await);
    assert!(unauthenticated.is_closed().await);
}

#[tokio::test]
async fn closing_a_client_ends_only_its_connections() {
    let w = World::new();
    let one = w.app(&SCOPES);
    let two = w.app(&SCOPES);
    let mut a = w.session(&one).await;
    let mut b = w.session(&two).await;
    w.conns.close_client(one.id);
    assert!(a.is_closed().await);
    assert!(b.ask(5, "ping", json!({})).await.get("result").is_some());
}
