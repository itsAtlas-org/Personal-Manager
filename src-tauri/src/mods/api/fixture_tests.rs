// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The experimental calls end to end, with one fixture mod that uses all four: PR B's X9, the only
//! check of `net.fetch`, `secrets.set`, `notify` and `ai.complete` together until a real mod uses
//! them (the sealed-frame CI never calls them).
//!
//! What is real: the mod is a folder on disk (an `internet`, `notify` and `ai` third-party mod with
//! one host and one key slot) loaded by the developer-mode folder loader (`Ledger::load_folder`), its
//! entry page served through the `pmmod` scheme, every call made through `dispatch` (session, store,
//! grant, limits, handler), the person's yes and their limit written by `consent::decide`, the key
//! typed through `secrets::save`, and the web hop sent over a real socket: `net.fetch`'s own checks
//! (URL rules, the lookup screen, redirects, the key on the first hop only) run unchanged and the last
//! step, the hop itself, goes through reqwest to a plain-http server on this machine's loopback
//! (`ScriptedHops::forward_to`, a test module: it is in no build). What is scripted: the name
//! lookups (public addresses, so the real screen passes them) and the model.
//!
//! The mod's own page does not run here (no webview): the tests make the calls its page would make,
//! in the order a real mod would make them, and check what each answer is and what reached the wire.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{json, Value};

use super::test_support::{live_session, Answer, OverHost, PlanSpec, TestHost};
use super::*;
use crate::mods::disk::Layout;
use crate::mods::folder;
use crate::mods::folder::tests::{edit_manifest, tool_folder};
use crate::mods::ids::ModId;
use crate::mods::ledger::{Ledger, UnsignedOptions};
use crate::mods::secrets;
use crate::mods::tab::consent;

const MOD: &str = "acme.rates";
const DEVICE: &str = "dddddddddddddddddddddddddddddddd";
const VAULT: &str = "11111111-2222-3333-4444-555555555555";
const HOST: &str = "api.example.com";
const OTHER: &str = "cdn.example.com";
/// A public address (documentation range of a real host), so the real screen lets the lookup through.
const WEB: &str = "93.184.216.34";
// A test value, never a credential (gitleaks reads this file).
const KEY: &str = "typed-value-9";

// --- the server on the loopback -----------------------------------------------------------------

/// One request as the server read it off the socket.
#[derive(Debug, Clone)]
struct Wire {
    method: String,
    target: String,
    /// Names lowercased, in the order sent.
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Wire {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    fn names(&self) -> Vec<&str> {
        self.headers.iter().map(|(n, _)| n.as_str()).collect()
    }
}

/// A response as raw bytes.
fn http(status: &str, headers: &[&str], body: &[u8]) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 {status}\r\nConnection: close\r\n").into_bytes();
    for h in headers {
        out.extend_from_slice(format!("{h}\r\n").as_bytes());
    }
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(body);
    out
}

/// A plain-http server on `127.0.0.1`, one thread per connection, that keeps every request it got
/// and answers each with what `route` says. Stopped (and its threads ended) on drop.
struct Server {
    port: u16,
    seen: Arc<Mutex<Vec<Wire>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    fn start(route: impl Fn(&Wire) -> Vec<u8> + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let route = Arc::new(route);
        let thread = {
            let (seen, stop) = (Arc::clone(&seen), Arc::clone(&stop));
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((sock, _)) => {
                            let (seen, route) = (Arc::clone(&seen), Arc::clone(&route));
                            std::thread::spawn(move || answer(sock, &seen, &*route));
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(5)),
                    }
                }
            })
        };
        Self {
            port,
            seen,
            stop,
            thread: Some(thread),
        }
    }

    fn seen(&self) -> Vec<Wire> {
        self.seen.lock().unwrap().clone()
    }

    fn targets(&self) -> Vec<String> {
        self.seen().into_iter().map(|w| w.target).collect()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn answer(mut sock: TcpStream, seen: &Mutex<Vec<Wire>>, route: &dyn Fn(&Wire) -> Vec<u8>) {
    let _ = sock.set_nonblocking(false);
    let _ = sock.set_read_timeout(Some(Duration::from_secs(5)));
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") && head.len() < 64 * 1024 {
        if sock.read(&mut byte).unwrap_or(0) == 0 {
            return;
        }
        head.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&head).into_owned();
    let mut lines = text.split("\r\n");
    let mut start = lines.next().unwrap_or_default().split(' ');
    let method = start.next().unwrap_or_default().to_owned();
    let target = start.next().unwrap_or_default().to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(n, _)| n == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    if length > 0 && sock.read_exact(&mut body).is_err() {
        return;
    }
    let wire = Wire {
        method,
        target,
        headers,
        body,
    };
    let reply = route(&wire);
    seen.lock().unwrap().push(wire);
    let _ = sock.write_all(&reply);
    let _ = sock.flush();
    // `Connection: close`: the client reads to the end, so a short pause lets it drain first.
    std::thread::sleep(Duration::from_millis(20));
}

/// The website the fixture mod talks to.
fn website(wire: &Wire) -> Vec<u8> {
    match wire.target.split('?').next().unwrap_or_default() {
        "/v1/rates" => http(
            "200 OK",
            &[
                "Content-Type: application/json",
                "Set-Cookie: sid=1; HttpOnly",
                "X-Served-By: fixture",
                "Content-Length: 13",
            ],
            br#"{"rate":1.07}"#,
        ),
        "/old" => http(
            "302 Found",
            &[
                "Location: https://cdn.example.com/rates.json",
                "Content-Length: 0",
            ],
            b"",
        ),
        "/rates.json" => http("200 OK", &["Content-Length: 8"], b"cdn-body"),
        "/away" => http(
            "302 Found",
            &[
                "Location: https://evil.example.com/steal",
                "Content-Length: 0",
            ],
            b"",
        ),
        "/loop" => http(
            "302 Found",
            &[
                "Location: https://api.example.com/loop",
                "Content-Length: 0",
            ],
            b"",
        ),
        // Longer than a mod may be told, declared up front and never sent.
        "/big" => http(
            "200 OK",
            &[&format!(
                "Content-Length: {}",
                crate::mods::api::limits::MAX_FETCH_RESPONSE_BYTES + 1
            )],
            b"",
        ),
        "/boom" => http("500 Internal Server Error", &["Content-Length: 4"], b"oops"),
        "/echo" => http(
            "200 OK",
            &[&format!("Content-Length: {}", wire.body.len())],
            &wire.body,
        ),
        _ => http("404 Not Found", &["Content-Length: 0"], b""),
    }
}

// --- the world ----------------------------------------------------------------------------------

/// A host with the fixture folder loaded, a live session on it, the website on the loopback and the
/// model scripted. Fields are dropped in an order that stops the server last.
struct World {
    host: TestHost,
    session: String,
    dir: tempfile::TempDir,
    server: Server,
}

fn id() -> ModId {
    ModId::parse(MOD).unwrap()
}

/// The fixture as a developer's folder: `internet` with one host list (`HOST`, `OTHER`) and one key
/// slot on `HOST`, `notify` and `ai`, and nothing that reads the person's data (D3).
fn write_fixture(dir: &Path) {
    tool_folder(dir, |m, _| {
        m["id"] = json!(MOD);
        m["name"] = json!("Rates");
        m["permissions"] = json!([
            {"id": "storage", "why": "Remembers the last rate."},
            {"id": "ui.tab", "why": "Its own tab."},
            {"id": "internet", "why": "Fetches the rates."},
            {"id": "notify", "why": "Tells you when a rate moves."},
            {"id": "ai", "why": "Writes the one-line summary."},
        ]);
        m["internetHosts"] = json!([HOST, OTHER]);
        m["secrets"] = json!([{"name": "token", "host": HOST, "header": "X-Api-Key"}]);
    });
}

fn load(host: &TestHost, dir: &Path) {
    {
        let mut conn = host.store().conn().unwrap();
        crate::mods::devmode::write(&mut conn, crate::mods::devmode::test_here(DEVICE), true)
            .unwrap();
    }
    host.runtime().set_unsigned_allowed(true);
    let scan = folder::scan(dir, true).unwrap();
    with_ledger(host, |ledger| {
        ledger
            .load_folder(
                &scan,
                &UnsignedOptions {
                    device: crate::mods::devmode::test_here(DEVICE),
                    index: None,
                },
                chrono::Utc::now(),
            )
            .unwrap()
    });
}

/// The ledger over the host's open store, for the length of `f`. The layout is only named, never
/// written to: a folder load and an unload touch no file.
fn with_ledger<T>(host: &TestHost, f: impl FnOnce(&Ledger<'_, OverHost<'_>>) -> T) -> T {
    let layout = Layout::new(
        host.dir(),
        VAULT,
        &crate::mods::disk::new_store_id().unwrap(),
    )
    .unwrap();
    let store = OverHost(host.store());
    f(&Ledger::new(&layout, &store, host.runtime()))
}

impl World {
    fn new() -> Self {
        let host = TestHost::new();
        let dir = tempfile::tempdir().unwrap();
        write_fixture(dir.path());
        load(&host, dir.path());
        let server = Server::start(website);
        host.0
            .hops
            .dns(HOST, &[WEB])
            .dns(OTHER, &[WEB])
            .dns("evil.example.com", &[WEB])
            .forward_to(server.port);
        host.0.model.set_plan(PlanSpec::Local);
        let session = live_session(&host, MOD, crate::mods::outbox::testing::discarding());
        Self {
            host,
            session,
            dir,
            server,
        }
    }

    /// A call through the whole dispatcher, as the frame makes it.
    async fn call(&self, method: &str, args: Value) -> Reply {
        dispatch(
            &self.host,
            Caller::Frame {
                window: MAIN_LABEL.into(),
                session: self.session.clone(),
            },
            method,
            &args.to_string(),
        )
        .await
    }

    /// The person's yes to `permission` (and a limit for `ai`), as the permission prompt's command
    /// writes it.
    fn allow(&self, permission: &str, cap: Option<f64>) {
        let mut conn = self.host.store().conn().unwrap();
        consent::decide(&mut conn, &id(), permission, true, cap, true).unwrap();
    }

    fn allow_all(&self) {
        for p in ["internet", "notify", "ai"] {
            self.allow(p, None);
        }
    }

    /// The person types the key into PM's field.
    fn type_key(&self) {
        let mut conn = self.host.store().conn().unwrap();
        secrets::save(&mut conn, MOD, "token", KEY).unwrap();
    }

    fn count(&self, sql: &str) -> i64 {
        self.host
            .sql(|c| c.query_row(sql, [], |r| r.get(0)).unwrap())
    }

    /// A new frame on the (reloaded or reloaded-again) mod: the old session ended.
    fn reopen(&mut self) {
        self.session = live_session(&self.host, MOD, crate::mods::outbox::testing::discarding());
    }
}

fn value(reply: Reply) -> Value {
    match reply {
        Reply::Value { value } => value,
        other => panic!("not an answer: {other:?}"),
    }
}

fn code(reply: Reply) -> Code {
    match reply {
        Reply::Error { error } => error.code,
        other => panic!("not a refusal: {other:?}"),
    }
}

fn page(reply: Reply) -> (PageAction, Value) {
    match reply {
        Reply::Page { action, args } => (action, args),
        other => panic!("not a page action: {other:?}"),
    }
}

fn fetch(url: &str) -> Value {
    json!({ "url": url })
}

fn fetch_with_key(url: &str) -> Value {
    json!({ "url": url, "secret": "token" })
}

// --- the tests ----------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_fixture_loads_from_its_folder_and_every_call_asks_before_the_person_has_said_yes() {
    let w = World::new();
    // What the loader made of the folder: an unsigned third-party row from a folder, serving.
    let (tier, signed, from, state): (String, bool, String, String) = w.host.sql(|c| {
        c.query_row(
            "SELECT tier, signed, installed_from, state FROM mods WHERE id = ?1",
            [MOD],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap()
    });
    assert_eq!(
        (tier.as_str(), signed, from.as_str(), state.as_str()),
        ("third", false, "folder", "active")
    );
    // Its four experimental calls are all in PM's table, marked experimental.
    for name in ["ai.complete", "net.fetch", "secrets.set", "notify"] {
        let row = table::ROWS.iter().find(|r| r.name == name).unwrap();
        assert!(row.experimental, "{name}");
    }
    // Nothing is granted, so each asks (the unsigned mod's first use), and nothing was spent,
    // sent or booked while it asked.
    for (method, args) in [
        ("net.fetch", fetch("https://api.example.com/v1/rates")),
        ("secrets.set", json!({"name": "token"})),
        ("notify", json!({"text": "hello"})),
        (
            "ai.complete",
            json!({"messages": [{"role": "user", "content": "hi"}], "maxTokens": 10}),
        ),
    ] {
        assert_eq!(
            code(w.call(method, args).await),
            Code::NeedsPrompt,
            "{method}"
        );
    }
    assert!(w.server.seen().is_empty());
    assert!(w.host.0.hops.seen().is_empty() && w.host.0.model.sent().is_empty());
    assert_eq!(w.count("SELECT count(*) FROM ai_reservations"), 0);
    assert_eq!(w.count("SELECT count(*) FROM usage_log"), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mod_fetches_with_its_key_asks_the_model_and_tells_the_person() {
    let w = World::new();
    w.allow_all();

    // 1. The mod asks for its key: PM is told where it goes, and that none is held yet.
    let (action, args) = page(w.call("secrets.set", json!({"name": "token"})).await);
    assert_eq!(action, PageAction::SecretsSet);
    assert_eq!(
        args,
        json!({"name": "token", "host": HOST, "header": "X-Api-Key", "set": false})
    );
    // 2. It tries anyway: refused before anything is looked up, let alone sent.
    assert_eq!(
        code(
            w.call(
                "net.fetch",
                fetch_with_key("https://api.example.com/v1/rates")
            )
            .await
        ),
        Code::SecretNotSet
    );
    assert!(w.server.seen().is_empty() && w.host.0.hops.looked_up().is_empty());

    // 3. The person types the key; the mod asks again and is told only that one is held.
    w.type_key();
    let (_, args) = page(w.call("secrets.set", json!({"name": "token"})).await);
    assert_eq!(args["set"], true);
    assert!(!args.to_string().contains(KEY));

    // 4. The fetch: over a real socket, with the key as the slot's header and nothing else of PM's.
    let rates = value(
        w.call(
            "net.fetch",
            fetch_with_key("https://api.example.com/v1/rates?pair=eur"),
        )
        .await,
    );
    assert_eq!(rates["status"], 200);
    assert_eq!(rates["bodyText"], r#"{"rate":1.07}"#);
    assert_eq!(rates["headers"]["content-type"], "application/json");
    let reply = rates.to_string();
    assert!(!reply.to_lowercase().contains("set-cookie") && !reply.contains("sid=1"));
    assert!(!reply.contains(KEY) && !reply.contains("example.com") && !reply.contains("127.0.0.1"));
    let on_wire = w.server.seen();
    assert_eq!(on_wire.len(), 1);
    assert_eq!(on_wire[0].method, "GET");
    assert_eq!(on_wire[0].target, "/v1/rates?pair=eur");
    assert_eq!(on_wire[0].header("x-api-key"), Some(KEY));
    assert_eq!(on_wire[0].header("user-agent"), Some("PM-mod/1"));
    assert!(on_wire[0]
        .header("host")
        .is_some_and(|h| h.starts_with("api.example.com")));
    // The lookup the fetch made was for the listed name, at the one port a mod may use.
    assert_eq!(w.host.0.hops.looked_up(), vec![(HOST.to_owned(), 443)]);

    // 5. The mod writes its own prompt from the answer; PM adds nothing and passes it on as given.
    w.host.0.model.set_answer(Answer::local("The euro is up."));
    let prompt = format!(
        "Summarise in one line: {}",
        rates["bodyText"].as_str().unwrap()
    );
    let said = value(
        w.call(
            "ai.complete",
            json!({"messages": [{"role": "user", "content": prompt}], "maxTokens": 40}),
        )
        .await,
    );
    assert_eq!(said["text"], "The euro is up.");
    assert_eq!(said["truncated"], false);
    let asked = w.host.0.model.sent();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].messages, vec![("user".to_owned(), prompt)]);
    assert_eq!(asked[0].max_tokens, 40);
    // The key never travelled toward the model or the mod: not in what it was asked, not in a reply.
    assert!(!format!("{asked:?}").contains(KEY) && !said.to_string().contains(KEY));
    // A local answer cost nothing, and the log names the mod that asked.
    let used: Vec<(String, Option<String>)> = w.host.sql(|c| {
        let mut s = c
            .prepare("SELECT kind, mod_id FROM usage_log ORDER BY id")
            .unwrap();
        s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    });
    assert_eq!(used, vec![("background".to_owned(), Some(MOD.to_owned()))]);

    // 6. It tells the person: PM's page is asked to draw a strip, with the mod's words.
    let (action, args) = page(
        w.call("notify", json!({"text": said["text"], "tone": "good"}))
            .await,
    );
    assert_eq!(action, PageAction::Notify);
    assert_eq!(args, json!({"text": "The euro is up.", "tone": "good"}));
    assert_eq!(w.count("SELECT notify_count FROM mods"), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redirect_is_followed_on_the_wire_and_the_key_stays_on_the_first_request() {
    let w = World::new();
    w.allow_all();
    w.type_key();

    // To another listed host: followed (the second request is the cdn's), keyless.
    let got = value(
        w.call("net.fetch", fetch_with_key("https://api.example.com/old"))
            .await,
    );
    assert_eq!(got["bodyText"], "cdn-body");
    let on_wire = w.server.seen();
    assert_eq!(
        on_wire
            .iter()
            .map(|x| x.target.as_str())
            .collect::<Vec<_>>(),
        ["/old", "/rates.json"]
    );
    assert_eq!(on_wire[0].header("x-api-key"), Some(KEY));
    assert_eq!(on_wire[1].header("x-api-key"), None);
    assert!(on_wire[1]
        .header("host")
        .is_some_and(|h| h.starts_with("cdn.example.com")));

    // To a host the mod did not list: refused after the first hop, the second never looked up.
    let before = w.host.0.hops.looked_up().len();
    assert_eq!(
        code(
            w.call("net.fetch", fetch("https://api.example.com/away"))
                .await
        ),
        Code::HostNotAllowed
    );
    assert_eq!(w.server.targets().last().map(String::as_str), Some("/away"));
    assert_eq!(
        w.host.0.hops.looked_up().len(),
        before + 1,
        "the first hop's lookup only"
    );
    assert!(!w.server.targets().contains(&"/steal".to_owned()));

    // A loop ends at PM's hop cap, not the mod's patience.
    assert_eq!(
        code(
            w.call("net.fetch", fetch("https://api.example.com/loop"))
                .await
        ),
        Code::TooManyRedirects
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_the_mod_sends_is_filtered_and_nothing_of_pms_own_goes_with_it() {
    let w = World::new();
    w.allow_all();
    w.type_key();

    // A POST carries its body and the mod's own headers, and the key only when the mod names the
    // slot.
    let echoed = value(
        w.call(
            "net.fetch",
            json!({"url": "https://api.example.com/echo", "method": "POST",
                   "headers": {"Content-Type": "text/plain", "X-Trace": "7"}, "body": "payload"}),
        )
        .await,
    );
    assert_eq!(echoed["bodyText"], "payload");
    let sent = &w.server.seen()[0];
    assert_eq!(sent.method, "POST");
    assert_eq!(sent.body, b"payload");
    assert_eq!(sent.header("x-trace"), Some("7"));
    assert_eq!(sent.header("x-api-key"), None, "no slot named, no key");
    // Nothing a browser would add for a page, and nothing that identifies the person or the mod.
    for forbidden in [
        "cookie",
        "authorization",
        "referer",
        "origin",
        "proxy-authorization",
        "x-forwarded-for",
    ] {
        assert!(!sent.names().contains(&forbidden), "{forbidden}");
    }
    assert!(!format!("{:?}", sent.headers).contains(MOD));

    // The headers a browser owns, and the slot's own header, cannot be set by the mod; the refusal
    // comes before anything is sent.
    let before = w.server.seen().len();
    for header in [
        "Cookie",
        "Host",
        "Proxy-Authorization",
        "Sec-Fetch-Mode",
        "X-Api-Key",
    ] {
        let got = w
            .call(
                "net.fetch",
                json!({"url": "https://api.example.com/v1/rates", "secret": "token",
                       "headers": {header: "x"}}),
            )
            .await;
        assert_eq!(code(got), Code::BadArgs, "{header}");
    }
    assert_eq!(w.server.seen().len(), before);

    // A server error is an answer (the mod decides what a 500 means); a body past the cap is not.
    let boom = value(
        w.call("net.fetch", fetch("https://api.example.com/boom"))
            .await,
    );
    assert_eq!(
        (boom["status"].as_i64(), boom["bodyText"].as_str()),
        (Some(500), Some("oops"))
    );
    assert_eq!(
        code(
            w.call("net.fetch", fetch("https://api.example.com/big"))
                .await
        ),
        Code::ResponseTooLarge
    );
    // A page of a listed name that PM has no route to is a network answer, never a refusal of the mod.
    assert_eq!(
        value(
            w.call("net.fetch", fetch("https://api.example.com/missing"))
                .await
        )["status"],
        404
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_key_goes_only_where_the_slot_says_and_a_reload_cannot_move_it() {
    let w = World::new();
    w.allow_all();
    w.type_key();

    // A key named for a request to the other listed host is refused before anything is looked up.
    assert_eq!(
        code(
            w.call(
                "net.fetch",
                fetch_with_key("https://cdn.example.com/rates.json")
            )
            .await
        ),
        Code::HostNotAllowed
    );
    assert!(w.server.seen().is_empty() && w.host.0.hops.looked_up().is_empty());

    // A reload may not move the slot (it is the same refusal as any widening: where the key goes is
    // what the person agreed to). The old frame keeps running on the old manifest, key and all.
    let reload = |w: &World| {
        let scanned = folder::scan(w.dir.path(), true);
        with_ledger(&w.host, |ledger| {
            ledger.apply_reload(&id(), scanned, &|_: &str, _: &str, _: Option<&str>| None)
        })
    };
    edit_manifest(w.dir.path(), |m| {
        m["secrets"] = json!([{"name": "token", "host": HOST, "header": "X-Rates-Key"}]);
    });
    assert!(matches!(
        reload(&w),
        Err(crate::mods::ledger::LedgerError::Refused(
            crate::mods::policy::Refusal::ReloadWidens(_)
        ))
    ));
    assert_eq!(w.count("SELECT count(*) FROM mod_secrets"), 1);
    value(
        w.call(
            "net.fetch",
            fetch_with_key("https://api.example.com/v1/rates"),
        )
        .await,
    );
    assert_eq!(w.server.seen()[0].header("x-api-key"), Some(KEY));

    // A reload that leaves the slot alone keeps the key. (Only the manifest's text changed, not a
    // served file, so the running frame is not replaced.)
    edit_manifest(w.dir.path(), |m| {
        m["version"] = json!("1.0.1");
        m["secrets"] = json!([{"name": "token", "host": HOST, "header": "X-Api-Key"}]);
    });
    assert!(reload(&w).unwrap().changed);
    let (_, args) = page(w.call("secrets.set", json!({"name": "token"})).await);
    assert_eq!(args["set"], true);

    // A reload that drops the slot takes the key with it, in the same step: a later reload that
    // brings the slot back finds it empty.
    edit_manifest(w.dir.path(), |m| {
        m["version"] = json!("1.0.2");
        m["secrets"] = json!([]);
    });
    assert!(reload(&w).unwrap().changed);
    assert_eq!(w.count("SELECT count(*) FROM mod_secrets"), 0);
    assert_eq!(
        code(w.call("secrets.set", json!({"name": "token"})).await),
        Code::BadArgs
    );
    assert_eq!(
        code(
            w.call(
                "net.fetch",
                fetch_with_key("https://api.example.com/v1/rates")
            )
            .await
        ),
        Code::SecretNotSet
    );
    let requests = w.server.seen().len();
    edit_manifest(w.dir.path(), |m| {
        m["version"] = json!("1.0.3");
        m["secrets"] = json!([{"name": "token", "host": HOST, "header": "X-Api-Key"}]);
    });
    // Restoring a dropped slot is a widening again (a key slot the person never agreed to): refused.
    assert!(matches!(
        reload(&w),
        Err(crate::mods::ledger::LedgerError::Refused(
            crate::mods::policy::Refusal::ReloadWidens(_)
        ))
    ));
    assert_eq!(w.server.seen().len(), requests);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notices_and_key_asks_are_capped_per_mod() {
    let w = World::new();
    w.allow_all();
    for n in 1..=limits::NOTIFY_PER_DAY {
        let (action, _) = page(w.call("notify", json!({"text": format!("note {n}")})).await);
        assert_eq!(action, PageAction::Notify);
    }
    assert_eq!(
        code(w.call("notify", json!({"text": "one too many"})).await),
        Code::RateLimited
    );
    for _ in 0..limits::SECRET_ASKS_PER_SESSION {
        page(w.call("secrets.set", json!({"name": "token"})).await);
    }
    assert_eq!(
        code(w.call("secrets.set", json!({"name": "token"})).await),
        Code::RateLimited
    );
    // A slot the mod did not declare is not a slot, and is not counted against it.
    assert_eq!(
        code(w.call("secrets.set", json!({"name": "other"})).await),
        Code::BadArgs
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_paid_route_waits_for_the_persons_limit_and_stops_at_it() {
    let w = World::new();
    w.allow("internet", None);
    w.allow("notify", None);
    w.host.0.model.set_plan(PlanSpec::Cloud(&["a/cheap"]));
    w.host.sql(|c| {
        c.execute(
            "INSERT INTO model_pricing(model, prompt_price, completion_price, fetched_at) \
             VALUES ('a/cheap', 1e-6, 2e-6, '2026-11-17T00:00:00.000Z')",
            [],
        )
        .unwrap();
    });
    w.host
        .0
        .model
        .set_answer(Answer::cloud("a line", "a/cheap", 1e-4));
    let ask = json!({"messages": [{"role": "user", "content": "hi"}], "maxTokens": 100});

    // Not yet allowed: asks. Allowed with no limit on a paid route: asks again (a ceiling first).
    assert_eq!(
        code(w.call("ai.complete", ask.clone()).await),
        Code::NeedsPrompt
    );
    w.allow("ai", None);
    assert_eq!(
        code(w.call("ai.complete", ask.clone()).await),
        Code::NeedsPrompt
    );
    assert_eq!(w.host.0.model.entered(), 0);

    // With a limit of 0.0004 and a worst case of 2.54e-4 a call (54 prompt bytes at 1e-6 plus 100
    // tokens at 2e-6), two calls that really cost 1e-4 fit (0 + 2.54e-4, then 1e-4 + 2.54e-4) and the third's
    // worst case does not (2e-4 + 2.54e-4 is past the limit).
    w.allow("ai", Some(4e-4));
    for _ in 0..2 {
        assert_eq!(
            value(w.call("ai.complete", ask.clone()).await)["text"],
            "a line"
        );
    }
    assert_eq!(
        code(w.call("ai.complete", ask.clone()).await),
        Code::AiCapReached
    );
    assert_eq!(
        w.host.0.model.entered(),
        2,
        "the refused call never reached the model"
    );
    let (cap, spent): (f64, f64) = w.host.sql(|c| {
        c.query_row("SELECT ai_cap_usd, ai_spent_usd FROM mods", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap()
    });
    assert_eq!(cap, 4e-4);
    assert!((spent - 2e-4).abs() < 1e-12, "{spent}");
    assert_eq!(w.count("SELECT count(*) FROM ai_reservations"), 0);

    // The spend is in the person's usage log under the mod, and stays when the mod goes.
    let by_mod =
        "SELECT count(*) FROM usage_log WHERE mod_id = 'acme.rates' AND kind = 'background'";
    assert_eq!(w.count(by_mod), 2);
    with_ledger(&w.host, |ledger| ledger.unload_folder(&id())).unwrap();
    assert_eq!(w.count(by_mod), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unloading_the_fixture_ends_its_calls_and_takes_its_key_and_limit_with_it() {
    let mut w = World::new();
    w.allow_all();
    w.type_key();
    value(
        w.call(
            "net.fetch",
            fetch_with_key("https://api.example.com/v1/rates"),
        )
        .await,
    );
    assert_eq!(w.server.seen().len(), 1);
    assert_eq!(w.count("SELECT count(*) FROM mod_secrets"), 1);

    with_ledger(&w.host, |ledger| ledger.unload_folder(&id())).unwrap();
    // The registry let go of it too (the row's absence alone already ends a call; the registry's
    // copy is what would serve its page to a new frame).
    assert!(!w.host.runtime().is_installed(&id()));
    assert!(!w.host.runtime().folders().contains(&id()));

    // Every call of the old frame ends, and nothing more reaches the wire or the model.
    for (method, args) in [
        (
            "net.fetch",
            fetch_with_key("https://api.example.com/v1/rates"),
        ),
        ("secrets.set", json!({"name": "token"})),
        ("notify", json!({"text": "late"})),
        (
            "ai.complete",
            json!({"messages": [{"role": "user", "content": "late"}], "maxTokens": 10}),
        ),
    ] {
        assert_eq!(
            code(w.call(method, args).await),
            Code::SessionEnded,
            "{method}"
        );
    }
    assert_eq!(w.server.seen().len(), 1);
    assert_eq!(w.host.0.model.entered(), 0);
    assert_eq!(w.count("SELECT count(*) FROM mod_secrets"), 0);
    assert_eq!(w.count("SELECT count(*) FROM mod_grants"), 0);

    // The same folder loaded again is a stranger: no grants, no key.
    load(&w.host, w.dir.path());
    w.reopen();
    assert_eq!(
        code(
            w.call(
                "net.fetch",
                fetch_with_key("https://api.example.com/v1/rates")
            )
            .await
        ),
        Code::NeedsPrompt
    );
    w.allow("internet", None);
    assert_eq!(
        code(
            w.call(
                "net.fetch",
                fetch_with_key("https://api.example.com/v1/rates")
            )
            .await
        ),
        Code::SecretNotSet
    );
    assert_eq!(w.server.seen().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_way_the_website_can_be_out_of_reach_is_its_own_fixed_answer_and_sends_nothing() {
    let w = World::new();
    w.allow_all();
    let url = fetch("https://api.example.com/v1/rates");

    // A listed name that points inside the person's network: the real screen refuses it, and the
    // loopback server (which the forward would have reached) hears nothing.
    w.host.0.hops.dns(HOST, &["10.0.0.5"]);
    assert_eq!(
        code(w.call("net.fetch", url.clone()).await),
        Code::HostNotPublic
    );
    // A name nobody can resolve.
    w.host.0.hops.dns(HOST, &[]);
    assert_eq!(
        code(w.call("net.fetch", url.clone()).await),
        Code::HostUnresolved
    );
    assert!(w.server.seen().is_empty());

    // A public address with nothing listening (a closed port on the loopback stands in for it).
    w.host.0.hops.dns(HOST, &[WEB]);
    let closed = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    w.host.0.hops.forward_to(closed);
    assert_eq!(
        code(w.call("net.fetch", url).await),
        Code::NetworkUnavailable
    );
    assert!(w.server.seen().is_empty());
}
