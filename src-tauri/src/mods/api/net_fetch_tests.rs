// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `net.fetch` end to end through the dispatcher, over a real store and a scripted network
//! (`netfetch::fake::ScriptedHops`, so nothing here touches a socket): what reaches the wire, where
//! the key goes and where it never goes, every refusal and failure and the one fixed sentence each
//! becomes, a session that ends mid-fetch, D3, and the family's own limits. The URL, header and
//! redirect rules themselves are `net.rs`'s and `netfetch.rs`'s own tests.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rusqlite::params;
use serde_json::{json, Value};

use super::test_support::{insert_mod, live_session, serve, ModRow, TestHost};
use super::*;
use crate::mods::ids::ModId;
use crate::mods::netfetch::fake::{reply as hop_reply, Seen};
use crate::mods::netfetch::{FetchError, Method, Policy};
use crate::mods::policy::MOD_API_VERSION;
use crate::mods::secrets;

const MOD: &str = "acme.rates";
const HOST: &str = "api.example.com";
const OTHER: &str = "cdn.example.com";
const WEB: &str = "93.184.216.34";
// A test value, never a credential (gitleaks reads this file).
const KEY: &str = "typed-value-7";

fn manifest() -> String {
    json!({
        "permissions": [{"id": "internet", "why": "test"}],
        "data": {"keys": []},
        "entry": {"page": "app/index.html"},
        "internetHosts": [HOST, OTHER],
        "secrets": [{"name": "token", "host": HOST, "header": "X-Api-Key"}],
    })
    .to_string()
}

/// A second-party mod with `internet` granted, a key slot on `HOST`, a live session, and a network
/// where every listed name resolves to a public address and answers 200 `hello`.
fn setup() -> (TestHost, String) {
    let host = TestHost::new();
    host.sql(|c| {
        insert_mod(c, &ModRow::signed(MOD, "second", &["internet"]));
        c.execute(
            "UPDATE mods SET manifest = ?2 WHERE id = ?1",
            params![MOD, manifest()],
        )
        .unwrap();
        c.execute(
            "INSERT INTO mod_grants(mod_id, permission, decision) \
             VALUES (?1, 'internet', 'granted')",
            [MOD],
        )
        .unwrap();
    });
    serve(&host, MOD);
    let session = live_session(&host, MOD, crate::mods::outbox::testing::discarding());
    host.0
        .hops
        .dns(HOST, &[WEB])
        .dns(OTHER, &[WEB])
        .dns("evil.example.com", &[WEB])
        .answer(|_| {
            Ok(hop_reply(
                200,
                &[("Content-Type", "text/plain"), ("Set-Cookie", "a=b")],
                b"hello",
            ))
        });
    (host, session)
}

fn store_key(host: &TestHost) {
    let mut conn = host.store().conn().unwrap();
    secrets::save(&mut conn, MOD, "token", KEY).unwrap();
}

fn frame(session: &str) -> Caller {
    Caller::Frame {
        window: MAIN_LABEL.into(),
        session: session.into(),
    }
}

fn policy() -> Policy {
    Policy {
        total: Duration::from_secs(5),
        ..Policy::for_mods()
    }
}

/// A call through the grant check and then the handler under `policy`, past the family's limits.
async fn call_with(host: &TestHost, session: &str, args: Value, policy: Policy) -> Reply {
    let auth = match authorise(host, table::ROWS, session, "net.fetch") {
        Ok(auth) => auth,
        Err(error) => return Reply::Error { error },
    };
    match net_fetch::run(host, &auth, &args, policy).await {
        Ok(reply) => reply,
        Err(error) => Reply::Error { error },
    }
}

async fn call(host: &TestHost, session: &str, args: Value) -> Reply {
    call_with(host, session, args, policy()).await
}

fn get(url: &str) -> Value {
    json!({ "url": url })
}

fn value(reply: Reply) -> Value {
    match reply {
        Reply::Value { value } => value,
        other => panic!("not an answer: {other:?}"),
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

fn seen(host: &TestHost) -> Vec<Seen> {
    host.0.hops.seen()
}

fn header<'a>(s: &'a Seen, name: &str) -> Option<&'a str> {
    s.headers
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

// --- the answer ---------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fetch_reaches_the_listed_host_and_the_mod_sees_status_allowed_headers_and_body() {
    let (host, session) = setup();
    let v = value(
        call(
            &host,
            &session,
            json!({"url": "https://api.example.com/v1/rates?x=1#frag", "headers": {"Accept": "application/json"}}),
        )
        .await,
    );
    assert_eq!(v["status"], 200);
    assert_eq!(v["bodyText"], "hello");
    assert_eq!(v["headers"]["content-type"], "text/plain");
    // Never the cookie the server set, nor the address asked for.
    let text = v.to_string();
    assert!(!text.to_lowercase().contains("set-cookie"), "{text}");
    assert!(!text.contains("example.com"), "{text}");
    let hops = seen(&host);
    assert_eq!(hops.len(), 1);
    assert_eq!(hops[0].method, Method::Get);
    // The fragment is dropped, the query kept, the mod's header sent (lowercased), no key.
    assert_eq!(hops[0].url, "https://api.example.com/v1/rates?x=1");
    assert_eq!(header(&hops[0], "accept"), Some("application/json"));
    assert!(header(&hops[0], "x-api-key").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_text_or_binary_body_goes_with_a_method_that_takes_one() {
    let (host, session) = setup();
    value(
        call(
            &host,
            &session,
            json!({"url": "https://api.example.com/p", "method": "post", "body": "payload"}),
        )
        .await,
    );
    value(
        call(
            &host,
            &session,
            json!({"url": "https://api.example.com/p", "method": "PUT", "bodyBase64": "AAEC/w=="}),
        )
        .await,
    );
    let hops = seen(&host);
    assert_eq!(hops[0].method, Method::Post);
    assert_eq!(hops[0].body.as_deref(), Some(&b"payload"[..]));
    assert_eq!(hops[1].method, Method::Put);
    assert_eq!(hops[1].body.as_deref(), Some(&[0u8, 1, 2, 255][..]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_binary_answer_comes_back_as_base64() {
    let (host, session) = setup();
    host.0
        .hops
        .answer(|_| Ok(hop_reply(200, &[], &[0xff, 0xfe, 0x00])));
    let v = value(call(&host, &session, get("https://api.example.com/b")).await);
    assert_eq!(v["bodyBase64"], "//4A");
    assert!(v.get("bodyText").is_none());
}

// --- the key ------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_named_key_goes_on_the_first_request_as_the_slots_header_and_nowhere_else() {
    let (host, session) = setup();
    store_key(&host);
    let v = value(
        call(
            &host,
            &session,
            json!({"url": "https://api.example.com/start", "secret": "token", "headers": {"x-other": "1"}}),
        )
        .await,
    );
    let hops = seen(&host);
    assert_eq!(header(&hops[0], "x-api-key"), Some(KEY));
    assert_eq!(header(&hops[0], "x-other"), Some("1"));
    // Not in the URL, the body or what the mod is told.
    assert!(!hops[0].url.contains(KEY));
    assert!(hops[0].body.is_none());
    assert!(!v.to_string().contains(KEY));
    // A call that does not name the slot sends no key, though one is stored.
    call(&host, &session, get("https://api.example.com/other")).await;
    assert!(header(&seen(&host)[1], "x-api-key").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_key_never_follows_a_redirect_even_to_the_same_host() {
    let (host, session) = setup();
    store_key(&host);
    host.0.hops.answer(|s| {
        Ok(if s.url.ends_with("/start") {
            hop_reply(302, &[("Location", "https://api.example.com/next")], b"")
        } else {
            hop_reply(200, &[], b"done")
        })
    });
    let v = value(
        call(
            &host,
            &session,
            json!({"url": "https://api.example.com/start", "secret": "token"}),
        )
        .await,
    );
    assert_eq!(v["bodyText"], "done");
    let hops = seen(&host);
    assert_eq!(hops.len(), 2);
    assert_eq!(header(&hops[0], "x-api-key"), Some(KEY));
    assert!(header(&hops[1], "x-api-key").is_none(), "{:?}", hops[1]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_key_is_never_sent_to_another_listed_host_and_nothing_is_dialled() {
    let (host, session) = setup();
    store_key(&host);
    // `cdn.example.com` is on the mod's list, but the key is for `api.example.com` alone.
    let e = error(
        call(
            &host,
            &session,
            json!({"url": "https://cdn.example.com/x", "secret": "token"}),
        )
        .await,
    );
    assert_eq!(e.code, Code::HostNotAllowed);
    assert!(seen(&host).is_empty());
    assert!(host.0.hops.looked_up().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_key_an_undeclared_slot_and_a_key_bound_elsewhere_are_all_just_not_set() {
    let (host, session) = setup();
    let ask = |name: &str| json!({"url": "https://api.example.com/x", "secret": name});
    // Nothing typed yet.
    assert_eq!(
        code(call(&host, &session, ask("token")).await),
        Code::SecretNotSet
    );
    store_key(&host);
    // A slot the manifest never declared.
    assert_eq!(
        code(call(&host, &session, ask("nope")).await),
        Code::SecretNotSet
    );
    // A row whose host was changed behind PM's back (a crafted backup, a hand edit) is no key.
    host.sql(|c| {
        c.execute("UPDATE mod_secrets SET host = ?1", [OTHER])
            .unwrap();
    });
    assert_eq!(
        code(call(&host, &session, ask("token")).await),
        Code::SecretNotSet
    );
    host.sql(|c| {
        c.execute(
            "UPDATE mod_secrets SET host = ?1, header = 'X-Other'",
            [HOST],
        )
        .unwrap();
    });
    assert_eq!(
        code(call(&host, &session, ask("token")).await),
        Code::SecretNotSet
    );
    assert!(seen(&host).is_empty(), "nothing was sent without its key");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mod_cannot_set_the_header_its_key_travels_in() {
    let (host, session) = setup();
    store_key(&host);
    let e = error(
        call(
            &host,
            &session,
            json!({"url": "https://api.example.com/x", "secret": "token", "headers": {"X-API-KEY": "mine"}}),
        )
        .await,
    );
    assert_eq!(e.code, Code::BadArgs);
    assert!(seen(&host).is_empty());
}

/// Whatever goes wrong, a call that carried a key tells the mod nothing of it, of the address, or of
/// the network's own words.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_failure_carries_the_key_the_address_or_the_networks_words() {
    let (host, session) = setup();
    store_key(&host);
    let ask = json!({"url": "https://api.example.com/secret-path?q=1", "secret": "token"});
    let net = &host.0.hops;
    let scripts: [(&str, &dyn Fn()); 4] = [
        ("unreachable", &|| {
            net.answer(|_| Err(FetchError::Unavailable));
        }),
        ("bad request", &|| {
            net.answer(|_| Err(FetchError::BadRequest));
        }),
        ("private", &|| {
            net.dns(HOST, &["10.0.0.1"]);
        }),
        ("unresolved", &|| {
            net.dns(HOST, &[]);
        }),
    ];
    for (name, script) in scripts {
        script();
        let reply = call(&host, &session, ask.clone()).await;
        let text = serde_json::to_string(&reply).unwrap();
        assert!(matches!(reply, Reply::Error { .. }), "{name}");
        for secret in [KEY, "secret-path", "example.com", WEB, "10.0.0.1"] {
            assert!(!text.contains(secret), "{name}: {text}");
        }
    }
}

// --- the arguments ------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_arguments_are_refused_before_anything_is_sent() {
    let (host, session) = setup();
    let cases: Vec<Value> = vec![
        json!({}),
        json!({"url": 5}),
        json!({"url": "https://api.example.com/", "nope": 1}),
        json!({"url": "https://api.example.com/", "headers": {"a": 1}}),
        json!({"url": "https://api.example.com/", "headers": ["a"]}),
        json!({"url": "https://api.example.com/", "method": "TRACE"}),
        json!({"url": "https://api.example.com/", "method": "CONNECT"}),
        json!({"url": "https://api.example.com/", "body": "x"}),
        json!({"url": "https://api.example.com/", "method": "POST", "body": "x", "bodyBase64": "AA=="}),
        json!({"url": "https://api.example.com/", "method": "POST", "bodyBase64": "!!"}),
        json!({"url": "https://api.example.com/", "headers": {"Host": "evil.example.com"}}),
        json!({"url": "https://api.example.com/", "headers": {"Cookie": "a=b"}}),
        json!({"url": "https://api.example.com/", "headers": {"a": "x\r\nb: c"}}),
        json!({"url": "https://api.example.com/", "secret": 7}),
        json!({"url": "not a url"}),
        json!({"url": " https://api.example.com/"}),
    ];
    for args in cases {
        assert_eq!(
            code(call(&host, &session, args.clone()).await),
            Code::BadArgs,
            "{args}"
        );
    }
    // A body over the request cap.
    let over = "a".repeat(limits::MAX_FETCH_REQUEST_BYTES + 1);
    let args = json!({"url": "https://api.example.com/", "method": "POST", "body": over});
    assert_eq!(code(call(&host, &session, args).await), Code::BadArgs);
    assert!(seen(&host).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_url_off_the_list_or_not_plain_https_is_refused_and_never_looked_up() {
    let (host, session) = setup();
    for url in [
        "https://evil.example.com/",
        "http://api.example.com/",
        "https://api.example.com:8443/",
        "https://user@api.example.com/",
        "https://api.example.com.evil.example.com/",
        "https://93.184.216.34/",
        "https://api.example.com./",
    ] {
        let c = code(call(&host, &session, get(url)).await);
        assert_eq!(c, Code::HostNotAllowed, "{url}");
    }
    assert!(host.0.hops.looked_up().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mod_whose_manifest_lists_no_hosts_can_fetch_nothing() {
    let (host, session) = setup();
    host.sql(|c| {
        c.execute(
            "UPDATE mods SET manifest = ?2 WHERE id = ?1",
            params![
                MOD,
                json!({"permissions": [{"id": "internet", "why": "t"}], "internetHosts": []})
                    .to_string()
            ],
        )
        .unwrap();
    });
    let c = code(call(&host, &session, get("https://api.example.com/")).await);
    assert_eq!(c, Code::HostNotAllowed);
}

// --- the failures -------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redirect_off_the_list_is_refused_like_a_first_request_and_never_looked_up() {
    let (host, session) = setup();
    host.0.hops.answer(|_| {
        Ok(hop_reply(
            302,
            &[("Location", "https://evil.example.com/steal")],
            b"",
        ))
    });
    let c = code(call(&host, &session, get("https://api.example.com/")).await);
    assert_eq!(c, Code::HostNotAllowed);
    assert_eq!(seen(&host).len(), 1);
    assert_eq!(
        host.0.hops.looked_up().len(),
        1,
        "the target was never looked up"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_way_a_fetch_can_fail_is_its_own_fixed_sentence() {
    let (host, session) = setup();
    let ask = get("https://api.example.com/");

    host.0.hops.dns(HOST, &["10.0.0.1"]);
    let e = error(call(&host, &session, ask.clone()).await);
    assert_eq!(e, ModError::host_not_public());

    host.0.hops.dns(HOST, &[]);
    let e = error(call(&host, &session, ask.clone()).await);
    assert_eq!(e, ModError::host_unresolved());

    host.0.hops.dns(HOST, &[WEB]);
    host.0.hops.answer(|_| Err(FetchError::Unavailable));
    let e = error(call(&host, &session, ask.clone()).await);
    assert_eq!(e, ModError::network_unavailable());

    host.0.hops.answer(|_| {
        Ok(hop_reply(
            302,
            &[("Location", "https://api.example.com/again")],
            b"",
        ))
    });
    let e = error(call(&host, &session, ask.clone()).await);
    assert_eq!(e, ModError::too_many_redirects());

    host.0
        .hops
        .answer(|_| Ok(hop_reply(200, &[], &[b'x'; 100])));
    let small = Policy {
        max_body: 10,
        ..policy()
    };
    let e = error(call_with(&host, &session, ask.clone(), small).await);
    assert_eq!(e, ModError::response_too_large());

    // An error status is an answer, not a failure: the mod reads it.
    host.0.hops.answer(|_| Ok(hop_reply(503, &[], b"down")));
    assert_eq!(value(call(&host, &session, ask).await)["status"], 503);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fetch_that_runs_out_of_time_is_stopped_and_told_the_site_was_unreachable() {
    let (host, session) = setup();
    host.0.hops.send_takes(Duration::from_secs(20));
    let short = Policy {
        total: Duration::from_millis(200),
        ..policy()
    };
    let began = Instant::now();
    let e = error(call_with(&host, &session, get("https://api.example.com/"), short).await);
    assert_eq!(e, ModError::network_unavailable());
    assert!(began.elapsed() < Duration::from_secs(5));
}

// --- a call that stops being the mod's ----------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_that_ends_during_one_hop_sends_no_second() {
    let (host, session) = setup();
    let (h, s) = (host.clone(), session.clone());
    host.0.hops.answer(move |_| {
        // The frame goes away while the first hop is on the wire, and the answer is a redirect.
        h.runtime().close(&s);
        Ok(hop_reply(
            302,
            &[("Location", "https://api.example.com/next")],
            b"",
        ))
    });
    let c = code(call(&host, &session, get("https://api.example.com/start")).await);
    assert_eq!(c, Code::SessionEnded);
    assert_eq!(seen(&host).len(), 1, "no second hop");
    assert_eq!(host.0.hops.looked_up().len(), 1, "no second lookup");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_uninstall_stops_a_fetch_that_is_waiting_without_waiting_it_out() {
    let (host, session) = setup();
    host.0.hops.send_takes(Duration::from_secs(20));
    let long = Policy {
        total: Duration::from_secs(60),
        ..policy()
    };
    let h = host.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        h.runtime().uninstall(&ModId::parse(MOD).unwrap());
    });
    let began = Instant::now();
    let reply = tokio::time::timeout(
        Duration::from_secs(5),
        call_with(&host, &session, get("https://api.example.com/"), long),
    )
    .await
    .expect("the call was stopped when its session ended");
    assert_eq!(code(reply), Code::SessionEnded);
    assert!(began.elapsed() < Duration::from_secs(5));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lookup_that_is_still_waiting_is_stopped_the_same_way() {
    let (host, session) = setup();
    host.0.hops.resolve_takes(Duration::from_secs(20));
    let long = Policy {
        total: Duration::from_secs(60),
        ..policy()
    };
    let (h, s) = (host.clone(), session.clone());
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        h.runtime().close(&s);
    });
    let reply = tokio::time::timeout(
        Duration::from_secs(5),
        call_with(&host, &session, get("https://api.example.com/"), long),
    )
    .await
    .expect("the lookup was dropped");
    assert_eq!(code(reply), Code::SessionEnded);
    assert!(seen(&host).is_empty());
}

/// The dispatcher's own check passed; the grant is withdrawn before the handler reads the store (the
/// recheck under the guard is what catches it), and the key is never read nor anything dialled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_grant_withdrawn_after_the_dispatchers_check_refuses_the_call() {
    let (host, session) = setup();
    store_key(&host);
    host.hook(|h, point| {
        if point == Point::BeforeData {
            h.sql(|c| {
                c.execute("UPDATE mod_grants SET decision = 'denied'", [])
                    .unwrap();
            });
        }
    });
    let args = json!({"url": "https://api.example.com/", "secret": "token"});
    assert_eq!(code(call(&host, &session, args).await), Code::NotGranted);
    assert!(host.0.hops.looked_up().is_empty());
    assert!(seen(&host).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_grant_withdrawn_before_the_call_is_refused_at_the_door() {
    let (host, session) = setup();
    host.sql(|c| {
        c.execute("UPDATE mod_grants SET decision = 'denied'", [])
            .unwrap();
    });
    let c = code(call(&host, &session, get("https://api.example.com/")).await);
    assert_eq!(c, Code::NotGranted);
    assert!(host.0.hops.looked_up().is_empty());
}

// --- the table row, D3 and the family's limits --------------------------------------------------

#[test]
fn the_row_is_an_experimental_egress_call_in_the_net_family() {
    let Lookup::Found(row) = table::lookup_call(table::ROWS, "net.fetch", MOD_API_VERSION) else {
        panic!("no net.fetch row");
    };
    assert!(row.experimental);
    assert_eq!(row.permission, Some("internet"));
    assert_eq!(row.class, permissions::Class::Egress);
    assert_eq!(row.family, limits::Family::Net);
}

fn grant(host: &TestHost, m: &str, permission: &str) {
    host.sql(|c| {
        c.execute(
            "INSERT INTO mod_grants(mod_id, permission, decision) VALUES (?1, ?2, 'granted') \
             ON CONFLICT(mod_id, permission) DO UPDATE SET decision = 'granted'",
            params![m, permission],
        )
        .unwrap();
    });
}

/// `net.fetch` of the listed host through the whole dispatcher, family limits included.
async fn fetched(host: &TestHost, session: &str) -> Reply {
    let args = get("https://api.example.com/").to_string();
    dispatch(host, frame(session), "net.fetch", &args).await
}

async fn asked(host: &TestHost, session: &str) -> Reply {
    let args = json!({"name": "token"}).to_string();
    dispatch(host, frame(session), "secrets.set", &args).await
}

/// D3 comes free from the row's permission: an unsigned mod's `net.fetch` (and `secrets.set`) is
/// refused while another unsigned mod holds a read grant, and goes through when none does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn d3_refuses_an_unsigned_mods_egress_calls_while_another_unsigned_mod_reads() {
    let host = TestHost::new();
    host.sql(|c| {
        insert_mod(c, &ModRow::unsigned("acme.reader", &["documents.meta"]));
        insert_mod(c, &ModRow::unsigned("acme.poster", &["internet"]));
        c.execute(
            "UPDATE mods SET manifest = ?1 WHERE id = 'acme.poster'",
            [json!({
                "permissions": [{"id": "internet", "why": "t"}],
                "internetHosts": [HOST],
                "secrets": [{"name": "token", "host": HOST, "header": "X-Api-Key"}],
            })
            .to_string()],
        )
        .unwrap();
    });
    serve(&host, "acme.poster");
    let session = live_session(
        &host,
        "acme.poster",
        crate::mods::outbox::testing::discarding(),
    );
    host.0
        .hops
        .dns(HOST, &[WEB])
        .answer(|_| Ok(hop_reply(200, &[], b"ok")));
    grant(&host, "acme.poster", "internet");
    // Nobody reads: the egress works.
    assert!(matches!(
        fetched(&host, &session).await,
        Reply::Value { .. }
    ));
    assert!(matches!(asked(&host, &session).await, Reply::Page { .. }));

    // Another unsigned mod is granted a read: both of this mod's calls are refused as "off".
    grant(&host, "acme.reader", "documents.meta");
    for reply in [fetched(&host, &session).await, asked(&host, &session).await] {
        let e = error(reply);
        assert_eq!((e.code, e.permission), (Code::NotGranted, Some("internet")));
        assert!(!e.message.contains("another"), "{}", e.message);
    }
}

/// A mod's fetches run four at once and no more (the net family); the fifth is refused as flooding
/// before its handler runs, and the four finish.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mod_has_four_fetches_in_flight_and_the_fifth_is_refused() {
    let (host, session) = setup();
    host.0.hops.send_takes(Duration::from_millis(400));
    let calls = (0..5).map(|_| fetched(&host, &session));
    let replies = futures_util::future::join_all(calls).await;
    let refused = replies
        .iter()
        .filter(|r| matches!(r, Reply::Error { error } if error.code == Code::RateLimited))
        .count();
    let answered = replies
        .iter()
        .filter(|r| matches!(r, Reply::Value { .. }))
        .count();
    assert_eq!((refused, answered), (1, 4));
    // Each answered call really went to the network; the refused one did not.
    assert_eq!(seen(&host).len(), 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_dispatcher_runs_a_fetch_with_a_key_end_to_end() {
    let (host, session) = setup();
    store_key(&host);
    let counted = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&counted);
    host.0.hops.answer(move |_| {
        c.fetch_add(1, Ordering::SeqCst);
        Ok(hop_reply(200, &[], b"through"))
    });
    let args = json!({"url": "https://api.example.com/", "secret": "token"});
    let v = value(dispatch(&host, frame(&session), "net.fetch", &args.to_string()).await);
    assert_eq!(v["bodyText"], "through");
    assert_eq!(counted.load(Ordering::SeqCst), 1);
}

/// The answer arrives in the same breath as the session ends: the mod is not handed it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_answer_for_a_session_that_ended_as_it_arrived_is_not_delivered() {
    let (host, session) = setup();
    let (h, s) = (host.clone(), session.clone());
    host.0.hops.answer(move |_| {
        h.runtime().close(&s);
        Ok(hop_reply(200, &[], b"too late"))
    });
    let reply = call(&host, &session, get("https://api.example.com/")).await;
    assert!(!serde_json::to_string(&reply).unwrap().contains("too late"));
    assert_eq!(code(reply), Code::SessionEnded);
}

/// A store swapped under the call (a vault switch): its session is the old store's, so the answer is
/// not delivered, though the session itself was not ended by the test's swap.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_store_swap_mid_fetch_delivers_nothing() {
    let (host, session) = setup();
    let swap = host.clone();
    host.0.hops.answer(move |_| {
        let conn = swap.store().take_conn().unwrap().unwrap();
        *swap.store().db.lock().unwrap() = Some(conn);
        Ok(hop_reply(200, &[], b"from the old store"))
    });
    let reply = call(&host, &session, get("https://api.example.com/")).await;
    assert!(!serde_json::to_string(&reply).unwrap().contains("old store"));
    assert_eq!(code(reply), Code::SessionEnded);
}
