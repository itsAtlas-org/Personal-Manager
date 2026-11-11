// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The events end to end over a real store, real sessions and real outboxes (a recording sink in place
//! of the Tauri channel): who is told, who is not, what the payload is, and what happens to an event
//! when the session it was for ends first.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rusqlite::{params, Connection};
use serde_json::{json, Value};

use super::*;
use crate::mods::api::test_support::{insert_mod, live_session, serve, ModRow, TestHost};
use crate::mods::api::Point;
use crate::mods::outbox::testing::Recorder;
use crate::mods::outbox::{EventMessage, EventSink, Outbox, SESSION_CLOSED};

fn id(s: &str) -> ModId {
    ModId::parse(s).unwrap()
}

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

/// A mod's frame: its live session and what its channel received.
struct Frame {
    session: String,
    rec: Recorder,
}

/// Install `row`, grant it `granted`, and open a live frame for it.
fn open(host: &TestHost, row: ModRow, granted: &[&str]) -> Frame {
    let m = row.id;
    host.sql(|c| insert_mod(c, &row));
    serve(host, m);
    for permission in granted {
        grant(host, m, permission, "granted");
    }
    let rec = Recorder::default();
    let session = live_session(host, m, rec.outbox());
    Frame { session, rec }
}

/// A second-party mod that declares `permissions`.
fn second(m: &'static str, permissions: &[&'static str]) -> ModRow {
    ModRow::signed(m, "second", permissions)
}

/// A document row with a secret in every column a mod must never see.
fn doc(conn: &Connection, id: i64, importance: Option<&str>) {
    conn.execute(
        "INSERT INTO documents(id, vault_path, title, content_hash, project, importance, tags, \
                               source_path, external_ref, source_id, source_parent_folder_name, \
                               source_account, source_folder_path) \
         VALUES (?1, ?2, ?3, ?2, 'Atlas', ?4, '[\"a\"]', '/home/me/secret.md', \
                 'https://drive.example/x', 'gdrive:me@example.com:f' || ?1, 'Private folder', \
                 'me@example.com', '[\"Private folder\"]')",
        params![id, format!("d{id}.md"), format!("Doc {id}"), importance],
    )
    .unwrap();
}

/// Poll until `done`, up to five seconds.
fn wait_for(done: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < until, "timed out waiting");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn say(host: &TestHost, name: &'static str, payload: Value) -> Emitted {
    emit_with(host, name, &Audience::Everyone, |_| Ok(vec![payload]))
}

/// The nine fields of the list DTO.
const DTO_FIELDS: [&str; 9] = [
    "id",
    "title",
    "homeProject",
    "importance",
    "chunkCount",
    "reviewed",
    "sourceType",
    "tags",
    "ingestedAt",
];

/// Every name a mod must never learn from an event.
const FORBIDDEN: [&str; 10] = [
    "source_path",
    "sourcePath",
    "secret.md",
    "external_ref",
    "drive.example",
    "source_id",
    "me@example.com",
    "Private folder",
    "vault_path",
    "d1.md",
];

// ---- who is told ----------------------------------------------------------------------------------

#[test]
fn an_ungranted_mod_receives_nothing() {
    let host = TestHost::new();
    let granted = open(
        &host,
        second("acme.granted", &["documents.meta"]),
        &["documents.meta"],
    );
    // Declared, never granted (second party: asked at install).
    let ungranted = open(&host, second("acme.ungranted", &["documents.meta"]), &[]);
    let denied = open(&host, second("acme.denied", &["documents.meta"]), &[]);
    grant(&host, "acme.denied", "documents.meta", "denied");
    // Never asked for it, so can't be told.
    let undeclared = open(&host, second("acme.undeclared", &["storage"]), &[]);
    // A neighbouring read permission isn't this one.
    let vectors = open(
        &host,
        second("acme.vectors", &["documents.vectors"]),
        &["documents.vectors"],
    );
    // Unsigned: asked at first use, and not asked yet.
    let unsigned = open(
        &host,
        ModRow::unsigned("acme.unsigned", &["documents.meta"]),
        &[],
    );
    // A row that isn't running (paused) with a session still live in the registry.
    let mut paused_row = second("acme.paused", &["documents.meta"]);
    paused_row.state = "paused";
    let paused = open(&host, paused_row, &["documents.meta"]);

    for (name, payload) in [
        (DOCUMENTS_ADDED, json!({ "documents": [{ "id": 1 }] })),
        (DOCUMENTS_DELETED, json!({ "ids": [1] })),
    ] {
        let said = say(&host, name, payload);
        assert_eq!((said.sent, said.refused), (1, 6), "{name}");
    }
    assert_eq!(
        granted.rec.names(),
        [DOCUMENTS_ADDED, DOCUMENTS_DELETED],
        "the mod that holds documents.meta hears both"
    );
    for (who, frame) in [
        ("ungranted", &ungranted),
        ("denied", &denied),
        ("undeclared", &undeclared),
        ("vectors only", &vectors),
        ("unsigned, unasked", &unsigned),
        ("paused", &paused),
    ] {
        assert!(frame.rec.all().is_empty(), "{who} was told something");
        assert_eq!(
            frame.rec.all().len(),
            0,
            "{who}: nothing, not even a notice that something happened"
        );
    }

    // A grant that arrives later is heard from then on, and only from then on.
    grant(&host, "acme.ungranted", "documents.meta", "granted");
    say(&host, DOCUMENTS_DELETED, json!({ "ids": [2] }));
    assert_eq!(
        ungranted.rec.payloads(DOCUMENTS_DELETED),
        [json!({ "ids": [2] })]
    );
}

#[test]
fn the_payload_is_not_built_unless_someone_may_hear_it() {
    let host = TestHost::new();
    open(&host, second("acme.ungranted", &["documents.meta"]), &[]);
    let built = AtomicBool::new(false);
    let said = emit_with(&host, DOCUMENTS_ADDED, &Audience::Everyone, |_| {
        built.store(true, Ordering::SeqCst);
        Ok(vec![json!({})])
    });
    assert_eq!((said.sent, said.refused), (0, 1));
    assert!(!built.load(Ordering::SeqCst), "built for nobody");
    // And with nobody live at all, not even the table is consulted.
    let empty = TestHost::new();
    let said = emit_with(&empty, DOCUMENTS_ADDED, &Audience::Everyone, |_| {
        built.store(true, Ordering::SeqCst);
        Ok(vec![json!({})])
    });
    assert_eq!(said, Emitted::default());
    assert!(!built.load(Ordering::SeqCst));
}

#[test]
fn a_name_with_no_row_is_never_sent() {
    let host = TestHost::new();
    let frame = open(&host, second("acme.tool", &["storage"]), &[]);
    let said = emit_with(&host, "tasks.changed", &Audience::Everyone, |_| {
        Ok(vec![json!({})])
    });
    assert_eq!(said, Emitted::default());
    assert!(frame.rec.all().is_empty());
}

#[test]
fn unsigned_mods_are_judged_by_d3_and_the_sealed_engine_before_they_are_told() {
    let host = TestHost::new();
    let reader = open(
        &host,
        ModRow::unsigned("acme.reader", &["documents.meta", "internet"]),
        &["documents.meta"],
    );
    assert_eq!(say(&host, DOCUMENTS_DELETED, json!({ "ids": [1] })).sent, 1);
    // An unsealed engine: an unsigned mod gets no read-class grant, so no read-class event.
    host.0.sealed.store(false, Ordering::SeqCst);
    assert_eq!(say(&host, DOCUMENTS_DELETED, json!({ "ids": [2] })).sent, 0);
    host.0.sealed.store(true, Ordering::SeqCst);
    // The same mod also holding egress is refused by D3 (a pair stored anyway, as a restore could).
    grant(&host, "acme.reader", "internet", "granted");
    assert_eq!(say(&host, DOCUMENTS_DELETED, json!({ "ids": [3] })).sent, 0);
    assert_eq!(
        reader.rec.payloads(DOCUMENTS_DELETED),
        [json!({ "ids": [1] })]
    );
}

#[test]
fn a_locked_app_or_another_store_is_told_nothing() {
    let host = TestHost::new();
    let frame = open(
        &host,
        second("acme.tool", &["documents.meta"]),
        &["documents.meta"],
    );
    host.0.locked.store(true, Ordering::SeqCst);
    let said = say(&host, DOCUMENTS_DELETED, json!({ "ids": [1] }));
    assert_eq!((said.sent, said.refused), (0, 1));
    host.0.locked.store(false, Ordering::SeqCst);

    // A session minted over a store that has since been swapped for another.
    let stale = Recorder::default();
    host.sql(|c| insert_mod(c, &second("acme.old", &["documents.meta"])));
    serve(&host, "acme.old");
    grant(&host, "acme.old", "documents.meta", "granted");
    let opened = host
        .runtime()
        .open_frame_in(
            &id("acme.old"),
            host.store().store_epoch() + 1,
            stale.outbox(),
        )
        .unwrap();
    let req = tauri::http::Request::builder()
        .uri(opened.entry_url.as_str())
        .body(Vec::new())
        .unwrap();
    assert_eq!(host.runtime().serve("main", &req).status(), 200);
    let said = say(&host, DOCUMENTS_DELETED, json!({ "ids": [2] }));
    assert_eq!(said.sent, 1, "only the session over this store");
    assert!(stale.all().is_empty());
    assert_eq!(
        frame.rec.payloads(DOCUMENTS_DELETED),
        [json!({ "ids": [2] })]
    );
}

#[test]
fn a_session_whose_frame_has_not_loaded_is_not_told() {
    let host = TestHost::new();
    host.sql(|c| insert_mod(c, &second("acme.tool", &["documents.meta"])));
    serve(&host, "acme.tool");
    grant(&host, "acme.tool", "documents.meta", "granted");
    let rec = Recorder::default();
    host.runtime()
        .open_frame_in(&id("acme.tool"), host.store().store_epoch(), rec.outbox())
        .unwrap();
    assert!(!host.runtime().any_live());
    assert_eq!(
        say(&host, DOCUMENTS_DELETED, json!({ "ids": [1] })),
        Emitted::default()
    );
}

// ---- sessions that end -----------------------------------------------------------------------------

#[test]
fn a_closed_sessions_channel_is_dropped_and_hears_nothing_more() {
    struct Probe(Arc<AtomicBool>);
    impl EventSink for Probe {
        fn deliver(&self, _: &EventMessage) -> bool {
            true
        }
    }
    impl Drop for Probe {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let host = TestHost::new();
    let dropped = Arc::new(AtomicBool::new(false));
    host.sql(|c| insert_mod(c, &second("acme.tool", &["documents.meta"])));
    serve(&host, "acme.tool");
    grant(&host, "acme.tool", "documents.meta", "granted");
    let outbox = Outbox::new(Probe(dropped.clone()));
    let session = live_session(&host, "acme.tool", outbox.clone());
    assert_eq!(say(&host, DOCUMENTS_DELETED, json!({ "ids": [1] })).sent, 1);

    host.runtime().close(&session);
    // The channel itself is gone (not just unused), though something still holds the outbox.
    assert!(dropped.load(Ordering::SeqCst));
    assert!(outbox.stats().closed);
    assert_eq!(
        outbox.deliver(DOCUMENTS_DELETED, json!({})),
        Delivery::Closed
    );
    // And the registry no longer names the session at all.
    assert_eq!(
        say(&host, DOCUMENTS_DELETED, json!({ "ids": [2] })),
        Emitted::default()
    );
    assert_eq!(outbox.stats().sent, 1);
}

#[test]
fn every_way_a_session_ends_closes_its_channel_with_the_right_goodbye() {
    let host = TestHost::new();
    let a = open(&host, second("acme.a", &["storage"]), &[]);
    let b = open(&host, second("acme.b", &["storage"]), &[]);
    let c = open(&host, second("acme.c", &["storage"]), &[]);
    let d = open(&host, second("acme.d", &["storage"]), &[]);

    host.runtime().pause(&id("acme.a"));
    assert_eq!(a.rec.names(), ["mod.paused", SESSION_CLOSED]);
    assert_eq!(
        a.rec.payloads(SESSION_CLOSED),
        [json!({ "reason": "paused" })]
    );

    host.runtime().uninstall(&id("acme.b"));
    assert_eq!(b.rec.names(), [SESSION_CLOSED]);
    assert_eq!(
        b.rec.payloads(SESSION_CLOSED),
        [json!({ "reason": "uninstalled" })]
    );

    // Installed again over itself (an update, a repair): every frame of the old one ends.
    serve(&host, "acme.c");
    assert_eq!(
        c.rec.payloads(SESSION_CLOSED),
        [json!({ "reason": "replaced" })]
    );

    // A registry made exactly something else: a mod that left is uninstalled.
    host.runtime().replace_all(Vec::new());
    assert_eq!(
        d.rec.payloads(SESSION_CLOSED),
        [json!({ "reason": "uninstalled" })]
    );

    // Never a second goodbye, and nothing after one.
    for f in [&a, &b, &c, &d] {
        assert_eq!(say(&host, DOCUMENTS_DELETED, json!({})), Emitted::default());
        let closed = f
            .rec
            .names()
            .iter()
            .filter(|n| **n == SESSION_CLOSED)
            .count();
        assert_eq!(closed, 1);
    }
}

#[test]
fn a_frame_that_closes_itself_or_a_reload_is_not_sent_a_goodbye() {
    let host = TestHost::new();
    let a = open(&host, second("acme.a", &["storage"]), &[]);
    let b = open(&host, second("acme.b", &["storage"]), &[]);
    host.runtime().close(&a.session);
    host.runtime().close_all();
    assert!(a.rec.all().is_empty() && b.rec.all().is_empty());

    // A frame that never loaded has nobody to tell either.
    host.sql(|c| insert_mod(c, &second("acme.p", &["storage"])));
    serve(&host, "acme.p");
    let rec = Recorder::default();
    let outbox = rec.outbox();
    host.runtime()
        .open_frame_in(&id("acme.p"), host.store().store_epoch(), outbox.clone())
        .unwrap();
    host.runtime().uninstall(&id("acme.p"));
    assert!(rec.all().is_empty());
    assert!(outbox.stats().closed);
}

#[test]
fn an_event_emitted_during_an_uninstall_never_reaches_the_next_session() {
    let host = TestHost::new();
    let old = open(
        &host,
        second("acme.tool", &["documents.meta"]),
        &["documents.meta"],
    );
    // The new session's channel, filled in by the hook, in the middle of the emission: after the
    // recipients are chosen and the payload built, before anything is delivered.
    let next = Recorder::default();
    let next_session = Arc::new(std::sync::Mutex::new(None::<String>));
    {
        let (next, next_session) = (next.clone(), next_session.clone());
        host.hook(move |h, point| {
            if point == crate::mods::api::Point::BeforeDelivery {
                h.runtime().uninstall(&id("acme.tool"));
                serve(h, "acme.tool");
                *next_session.lock().unwrap() = Some(live_session(h, "acme.tool", next.outbox()));
            }
        });
    }
    let said = say(&host, DOCUMENTS_DELETED, json!({ "ids": [9] }));
    assert_eq!((said.sent, said.refused), (0, 1));
    // The old session, uninstalled mid-emission, got its goodbye and nothing of the event.
    assert_eq!(old.rec.names(), [SESSION_CLOSED]);
    // The next session of the same mod was chosen after the event was, so it never hears it.
    assert!(next_session.lock().unwrap().is_some());
    assert!(next.all().is_empty());

    // It does hear the next one: nothing was lost but the one the old session missed.
    host.hook(|_, _| {});
    assert_eq!(
        say(&host, DOCUMENTS_DELETED, json!({ "ids": [10] })).sent,
        1
    );
    assert_eq!(next.payloads(DOCUMENTS_DELETED), [json!({ "ids": [10] })]);
}

#[test]
fn a_pause_or_a_lock_between_the_check_and_the_delivery_delivers_nothing() {
    let host = TestHost::new();
    let a = open(
        &host,
        second("acme.a", &["documents.meta"]),
        &["documents.meta"],
    );
    host.hook(|h, point| {
        if point == crate::mods::api::Point::BeforeDelivery {
            h.runtime().bump_all();
        }
    });
    let said = say(&host, DOCUMENTS_DELETED, json!({ "ids": [1] }));
    assert_eq!((said.sent, said.refused), (0, 1));
    assert!(a.rec.all().is_empty());

    host.hook(|h, point| {
        if point == crate::mods::api::Point::BeforeDelivery {
            h.runtime().pause(&id("acme.a"));
        }
    });
    // The generation bump above ended its currency for old tickets only; a fresh emission reads new
    // ones, and the pause in the middle of this one ends the session under it.
    let said = say(&host, DOCUMENTS_DELETED, json!({ "ids": [2] }));
    assert_eq!(said.sent, 0);
    assert_eq!(a.rec.names(), ["mod.paused", SESSION_CLOSED]);
}

// ---- a slow frame ----------------------------------------------------------------------------------

#[test]
fn a_frame_that_never_reads_is_bounded_and_told_what_it_missed() {
    let host = TestHost::new();
    let f = open(
        &host,
        second("acme.tool", &["documents.meta"]),
        &["documents.meta"],
    );
    for n in 0..1000 {
        say(&host, DOCUMENTS_DELETED, json!({ "ids": [n] }));
    }
    let sent = f.rec.payloads(DOCUMENTS_DELETED).len() as u64;
    assert_eq!(sent, limits::EVENT_WINDOW);

    // An acknowledgement from a session that doesn't exist changes nothing.
    host.runtime().ack("not-a-session", u64::MAX);
    say(&host, DOCUMENTS_DELETED, json!({ "ids": [5000] }));
    assert_eq!(f.rec.all().len() as u64, limits::EVENT_WINDOW);

    // The frame takes them all, and no later event is coming: it is told it missed some anyway, at
    // once, by the resync notice (the last event of a burst being the dropped one is the case this
    // is for).
    host.runtime().ack(&f.session, limits::EVENT_WINDOW);
    let notice = f.rec.all().last().cloned().unwrap();
    assert_eq!(
        (notice.name, notice.payload, notice.missed),
        (
            outbox::RESYNC,
            json!({}),
            Some(1000 - limits::EVENT_WINDOW + 1)
        )
    );
    // Events flow again, and carry no count: it was told once.
    say(&host, DOCUMENTS_DELETED, json!({ "ids": [6000] }));
    let last = f.rec.all().last().cloned().unwrap();
    assert_eq!(last.payload, json!({ "ids": [6000] }));
    assert_eq!(last.missed, None);
}

/// Ids the hub's cap dropped are lost to every frame, and a frame that may hear the event is told it
/// missed them; one that may not is told nothing, as for any event.
#[test]
fn ids_dropped_for_the_hubs_cap_are_reported_to_the_frames_that_may_hear_them() {
    let host = TestHost::new();
    let heard = open(
        &host,
        second("acme.tool", &["documents.meta"]),
        &["documents.meta"],
    );
    let deaf = open(&host, second("acme.deaf", &["storage"]), &[]);
    let epoch = host.store().store_epoch();
    let over = limits::MAX_PENDING_IDS as i64 + 25;
    Hub::note(&host.hub().landed, &(1..=over).collect::<Vec<_>>(), epoch);
    Hub::note(&host.hub().deleted, &(1..=over).collect::<Vec<_>>(), epoch);
    // No documents exist: nothing is added, and every noted id is gone. The 25 past the cap are not
    // known to anyone, so each hearing frame is owed a notice for each set.
    flush_landed(&host);
    settle_deleted(&host);
    let notices: Vec<_> = heard
        .rec
        .all()
        .into_iter()
        .filter(|m| m.name == outbox::RESYNC)
        .collect();
    assert_eq!(
        notices.iter().map(|m| m.missed).collect::<Vec<_>>(),
        [Some(25), Some(25)],
        "{:?}",
        heard.rec.names()
    );
    assert!(deaf.rec.all().is_empty());
    // Taken: the count starts again.
    assert_eq!(host.hub().counts(), ((0, 0), (0, 0)));
    // A cap hit over a store that is gone isn't reported over the next one.
    Hub::note(
        &host.hub().deleted,
        &(1..=over).collect::<Vec<_>>(),
        epoch + 1,
    );
    let before = heard.rec.all().len();
    assert_eq!(settle_deleted(&host), Emitted::default());
    assert_eq!(heard.rec.all().len(), before);
}

/// The store swapped after a flush has checked its epoch and let go of the DB guard, before the
/// emission takes it again: sessions opened over the new store are told nothing of the old one's
/// deleted ids.
#[test]
fn a_store_swap_between_the_settle_and_its_emission_announces_nothing() {
    let host = TestHost::new();
    let old = open(
        &host,
        second("acme.tool", &["documents.meta"]),
        &["documents.meta"],
    );
    let new_rec = Recorder::default();
    let fired = Arc::new(AtomicBool::new(false));
    let fresh = new_rec.clone();
    host.hook(move |host, at| {
        if at == Point::BeforeEmit && !fired.swap(true, Ordering::SeqCst) {
            // The swap, as every swap moves the epoch, and a mod opened over the new store.
            let conn = host.store().take_conn().unwrap().unwrap();
            *host.store().db.lock().unwrap() = Some(conn);
            live_session(host, "acme.tool", fresh.outbox());
        }
    });
    Hub::note(&host.hub().deleted, &[5, 9], host.store().store_epoch());
    let said = settle_deleted(&host);
    assert_eq!(said.sent, 0);
    assert!(new_rec.all().is_empty(), "{:?}", new_rec.names());
    assert!(old.rec.all().is_empty());
}

// ---- documents.added -------------------------------------------------------------------------------

#[test]
fn a_landing_is_announced_as_the_lists_own_dto_and_nothing_more() {
    let host = TestHost::new();
    let f = open(
        &host,
        second("acme.tool", &["documents.meta"]),
        &["documents.meta"],
    );
    host.sql(|c| doc(c, 1, None));
    host.sql(|c| doc(c, 2, Some("archive")));
    host.sql(|c| doc(c, 3, Some("high")));
    // 2 is shelved; 4 was never there (a deleted document that landed a moment before): both are
    // simply absent, as they are from `documents.list`.
    for n in [1, 2, 3, 4] {
        Hub::note(&host.hub().landed, &[n], host.store().store_epoch());
    }
    let said = flush_landed(&host);
    assert_eq!(said.sent, 1);
    let events = f.rec.payloads(DOCUMENTS_ADDED);
    assert_eq!(events.len(), 1);
    let documents = events[0]["documents"].as_array().unwrap();
    let ids: Vec<i64> = documents
        .iter()
        .map(|d| d["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, [1, 3]);
    for d in documents {
        let mut keys: Vec<&str> = d.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        let mut want = DTO_FIELDS.to_vec();
        want.sort_unstable();
        assert_eq!(keys, want, "every field of the DTO, and no other");
    }
    assert_eq!(documents[0]["homeProject"]["name"], "Atlas");
    assert_eq!(documents[0]["tags"], json!(["a"]));
    // Nothing a mod must never see is anywhere in what was sent.
    let text = serde_json::to_string(&f.rec.all()).unwrap();
    for secret in FORBIDDEN {
        assert!(!text.contains(secret), "{secret} reached the frame: {text}");
    }
}

#[test]
fn landings_are_coalesced_into_one_event_after_the_interval() {
    let host = TestHost::with_interval(Duration::from_millis(100));
    let f = open(
        &host,
        second("acme.tool", &["documents.meta"]),
        &["documents.meta"],
    );
    host.sql(|c| {
        for n in 1..=4 {
            doc(c, n, None);
        }
    });
    // Back to back, well inside one interval.
    for n in 1..=3 {
        document_landed(&host, n);
    }
    wait_for(|| !f.rec.payloads(DOCUMENTS_ADDED).is_empty());
    std::thread::sleep(Duration::from_millis(250));
    let events = f.rec.payloads(DOCUMENTS_ADDED);
    assert_eq!(events.len(), 1, "one event for the burst");
    assert_eq!(events[0]["documents"].as_array().unwrap().len(), 3);
    // The next landing starts a new interval.
    document_landed(&host, 4);
    wait_for(|| f.rec.payloads(DOCUMENTS_ADDED).len() == 2);
}

#[test]
fn a_landing_is_not_even_noted_while_no_frame_is_live() {
    let host = TestHost::new();
    host.sql(|c| doc(c, 1, None));
    document_landed(&host, 1);
    documents_deleted(&host, &[1]);
    assert_eq!(host.hub().counts(), ((0, 0), (0, 0)));
}

#[test]
fn a_big_flood_is_announced_in_events_that_fit_and_a_hub_holds_a_bounded_set() {
    let host = TestHost::new();
    let f = open(
        &host,
        second("acme.tool", &["documents.meta"]),
        &["documents.meta"],
    );
    host.sql(|c| {
        for n in 1..=700 {
            doc(c, n, None);
        }
    });
    for n in 1..=700 {
        Hub::note(&host.hub().landed, &[n], host.store().store_epoch());
    }
    flush_landed(&host);
    let events = f.rec.payloads(DOCUMENTS_ADDED);
    assert!(events.len() > 1);
    let mut seen = 0;
    for e in &events {
        let bytes = serde_json::to_vec(e).unwrap().len();
        assert!(bytes <= limits::MAX_EVENT_BYTES, "{bytes}");
        seen += e["documents"].as_array().unwrap().len();
    }
    assert_eq!(seen, 700);
    assert_eq!(f.rec.all().iter().filter(|m| m.missed.is_some()).count(), 0);

    // The set a hub holds is capped: the rest are dropped and counted, never queued.
    let hub = Hub::default();
    let all: Vec<i64> = (1..=limits::MAX_PENDING_IDS as i64 + 50).collect();
    assert!(Hub::note(&hub.landed, &all, 1));
    assert_eq!(hub.counts().0, (limits::MAX_PENDING_IDS, 50));
    // A note that is already in the set is still accepted (nothing new to hold).
    Hub::note(&hub.landed, &[1], 1);
    assert_eq!(hub.counts().0 .1, 50);
}

#[test]
fn ids_noted_over_one_store_are_never_announced_over_another() {
    let hub = Hub::default();
    assert!(Hub::note(&hub.deleted, &[1, 2], 5));
    // A second note over the same store starts no second timer.
    assert!(!Hub::note(&hub.deleted, &[3], 5));
    // The store changed: what was noted before is dropped, and a new timer isn't needed (one is
    // already waiting; it will take the new store's ids).
    assert!(!Hub::note(&hub.deleted, &[7], 6));
    let (epoch, ids, lost) = Hub::take(&hub.deleted);
    assert_eq!((epoch, ids, lost), (6, vec![7], 0));
    // Taken: the next note starts a timer again.
    assert!(Hub::note(&hub.deleted, &[8], 6));

    // And the flush itself refuses ids noted over a store that has gone.
    let host = TestHost::new();
    let f = open(
        &host,
        second("acme.tool", &["documents.meta"]),
        &["documents.meta"],
    );
    Hub::note(&host.hub().deleted, &[41], host.store().store_epoch() + 1);
    assert_eq!(settle_deleted(&host), Emitted::default());
    assert!(f.rec.all().is_empty());
}

// ---- documents.deleted -----------------------------------------------------------------------------

/// Reports to this test's host from `note_deleted`, on this thread, until dropped.
struct Reporting;

impl Reporting {
    fn to(host: &TestHost) -> Self {
        let host = host.clone();
        TEST_NOTES.with(|n| {
            *n.borrow_mut() = Some(Arc::new(move |ids| documents_deleted(&host, ids)));
        });
        Self
    }
}

impl Drop for Reporting {
    fn drop(&mut self) {
        TEST_NOTES.with(|n| *n.borrow_mut() = None);
    }
}

#[test]
fn a_delete_is_announced_only_after_it_committed_and_only_when_it_is_gone() {
    let host = TestHost::new();
    let f = open(
        &host,
        second("acme.tool", &["documents.meta"]),
        &["documents.meta"],
    );
    host.sql(|c| {
        doc(c, 1, None);
        doc(c, 2, Some("archive"));
        doc(c, 3, None);
    });
    let _report = Reporting::to(&host);

    // Deleted inside a transaction that then rolls back: noted, and nothing is gone, so nothing
    // is said.
    host.sql(|c| {
        let tx = c.unchecked_transaction().unwrap();
        crate::ingest::delete_document(&tx, 1).unwrap();
        crate::ingest::delete_document(&tx, 3).unwrap();
        tx.rollback().unwrap();
    });
    assert_eq!(settle_deleted(&host), Emitted::default());
    assert!(f.rec.all().is_empty());

    // Deleted for real: the listed ones are announced, once, and the archived one isn't (it was
    // never visible to a mod, and an id it couldn't read mustn't be learnable from an event).
    host.sql(|c| {
        let tx = c.unchecked_transaction().unwrap();
        for n in [1, 2, 3] {
            crate::ingest::delete_document(&tx, n).unwrap();
        }
        tx.commit().unwrap();
    });
    assert_eq!(settle_deleted(&host).sent, 1);
    assert_eq!(
        f.rec.payloads(DOCUMENTS_DELETED),
        [json!({ "ids": [1, 3] })]
    );
    // Settled once: nothing left to say.
    assert_eq!(settle_deleted(&host), Emitted::default());
}

#[test]
fn a_delete_made_while_the_settle_waits_is_announced_when_the_writer_lets_go() {
    let host = TestHost::new();
    let f = open(
        &host,
        second("acme.tool", &["documents.meta"]),
        &["documents.meta"],
    );
    host.sql(|c| doc(c, 1, None));

    // A writer holds the store through its delete and a pause before it commits; the interval ends
    // meanwhile. The settle waits for the guard rather than reading a half-made change.
    let writer = {
        let host = host.clone();
        std::thread::spawn(move || {
            let _report = Reporting::to(&host);
            let conn = host.store().conn().unwrap();
            let tx = conn.unchecked_transaction().unwrap();
            crate::ingest::delete_document(&tx, 1).unwrap();
            // The timer's flush starts now and blocks on the guard.
            std::thread::sleep(Duration::from_millis(300));
            tx.commit().unwrap();
        })
    };
    wait_for(|| host.hub().counts().1 .0 == 1);
    // The flush the timer would run, on its own thread.
    let settler = {
        let host = host.clone();
        std::thread::spawn(move || settle_deleted(&host))
    };
    writer.join().unwrap();
    assert_eq!(settler.join().unwrap().sent, 1);
    assert_eq!(f.rec.payloads(DOCUMENTS_DELETED), [json!({ "ids": [1] })]);
}

#[test]
fn the_deleted_ids_arrive_in_events_that_fit() {
    let host = TestHost::new();
    let f = open(
        &host,
        second("acme.tool", &["documents.meta"]),
        &["documents.meta"],
    );
    let ids: Vec<i64> = (1..=5000).collect();
    documents_deleted(&host, &ids);
    // The timer would run this; the ids are all absent from an empty store.
    let said = settle_deleted(&host);
    assert_eq!(said.sent, 3);
    let events = f.rec.payloads(DOCUMENTS_DELETED);
    let mut got: Vec<i64> = Vec::new();
    for e in &events {
        assert!(serde_json::to_vec(e).unwrap().len() <= limits::MAX_EVENT_BYTES);
        got.extend(
            e["ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_i64().unwrap()),
        );
    }
    assert_eq!(got, ids);
    assert_eq!(
        events[0]["ids"].as_array().unwrap().len(),
        limits::MAX_IDS_PER_EVENT
    );
}

#[test]
fn the_ids_a_wipe_announces_are_the_ones_a_mod_could_see() {
    let (_dir, conn) = crate::mods::api::test_support::store();
    doc(&conn, 1, None);
    doc(&conn, 2, Some("archive"));
    doc(&conn, 3, Some("low"));
    assert_eq!(listed_ids(&conn), [1, 3]);
    assert!(is_listed(&conn, 1));
    assert!(!is_listed(&conn, 2));
    assert!(!is_listed(&conn, 99), "no row: nothing to announce");
}

// ---- theme.changed ---------------------------------------------------------------------------------

fn theme() -> Value {
    json!({
        "vars": { "--bg": "oklch(1 0 0)", "--ink": "#000" },
        "mode": "dark", "system": "slate", "accent": "mono", "colorblind": false, "depth": "flat",
    })
}

#[test]
fn a_theme_change_goes_to_the_one_session_the_page_named_and_is_rebuilt() {
    let host = TestHost::new();
    let a = open(&host, second("acme.a", &["storage"]), &[]);
    let b = open(&host, second("acme.b", &["storage"]), &[]);
    let none = open(&host, second("acme.none", &["documents.meta"]), &[]);

    let mut sent = theme();
    sent["evil"] = json!("<script>");
    sent["vars"]["--ok"] = json!("1px");
    assert_eq!(theme_changed(&host, &a.session, &sent).sent, 1);
    let got = a.rec.payloads(THEME_CHANGED);
    let mut want = theme();
    want["vars"]["--ok"] = json!("1px");
    assert_eq!(
        got,
        [want],
        "rebuilt from the five fields and the vars, nothing else"
    );
    assert!(b.rec.all().is_empty(), "another frame is told its own");
    // A mod that never asked for storage isn't told, and the page isn't told it wasn't.
    assert_eq!(theme_changed(&host, &none.session, &theme()).sent, 0);
    assert!(none.rec.all().is_empty());
    // A session that doesn't exist.
    assert_eq!(theme_changed(&host, "nope", &theme()), Emitted::default());
}

#[test]
fn a_theme_that_isnt_the_snapshot_shape_is_refused_whole() {
    let host = TestHost::new();
    let a = open(&host, second("acme.a", &["storage"]), &[]);
    let bad: Vec<Value> = vec![
        json!(null),
        json!([]),
        json!({}),
        json!({ "vars": [] }),
        json!({ "vars": { "bg": "x" }, "mode": "dark", "system": "s", "accent": "a", "colorblind": false, "depth": "d" }),
        json!({ "vars": { "--bg": 1 }, "mode": "dark", "system": "s", "accent": "a", "colorblind": false, "depth": "d" }),
        json!({ "vars": {}, "mode": 1, "system": "s", "accent": "a", "colorblind": false, "depth": "d" }),
        json!({ "vars": {}, "mode": "dark", "system": "s", "accent": "a", "colorblind": "no", "depth": "d" }),
        json!({ "vars": {}, "mode": "dark", "system": "s", "accent": "a", "colorblind": false }),
        json!({ "vars": {}, "mode": "", "system": "s", "accent": "a", "colorblind": false, "depth": "d" }),
        json!({ "vars": {}, "mode": "x".repeat(65), "system": "s", "accent": "a", "colorblind": false, "depth": "d" }),
    ];
    for theme in bad {
        assert_eq!(
            theme_changed(&host, &a.session, &theme),
            Emitted::default(),
            "{theme}"
        );
    }
    assert!(a.rec.all().is_empty());
    // One over the event size cap is dropped and counted, not sent.
    let mut huge = theme();
    huge["vars"]["--big"] = json!("x".repeat(limits::MAX_EVENT_BYTES));
    let said = theme_changed(&host, &a.session, &huge);
    assert_eq!((said.sent, said.dropped), (0, 1));
}

// ---- grants.changed --------------------------------------------------------------------------------

#[test]
fn grants_changed_goes_to_each_running_mod_named_once_and_carries_nothing() {
    let host = TestHost::new();
    // No permission is needed to hear it, so a mod that has none at all does.
    let a = open(&host, second("acme.a", &[]), &[]);
    let other = open(&host, second("acme.b", &[]), &[]);
    let mut paused = second("acme.paused", &[]);
    paused.state = "paused";
    let p = open(&host, paused, &[]);

    let said = grants_changed(&host, &[id("acme.a"), id("acme.a"), id("acme.paused")]);
    assert_eq!((said.sent, said.refused), (1, 1));
    assert_eq!(a.rec.payloads(GRANTS_CHANGED), [json!({})]);
    assert!(other.rec.all().is_empty(), "a mod not named isn't told");
    assert!(
        p.rec.all().is_empty(),
        "a mod that isn't running isn't told"
    );
    // Both of one mod's frames hear it.
    let twice = {
        let rec = Recorder::default();
        live_session(&host, "acme.a", rec.outbox());
        rec
    };
    assert_eq!(grants_changed(&host, &[id("acme.a")]).sent, 2);
    assert_eq!(twice.payloads(GRANTS_CHANGED), [json!({})]);
}

// ---- the table -------------------------------------------------------------------------------------

#[test]
fn the_events_the_page_lets_through_are_exactly_the_events_there_are() {
    let page = include_str!("../../../../../src/components/mods/modBridge.ts");
    // The list runs over several lines once prettier has had it: from its opening to `as const`.
    let from = page
        .find("export const MOD_EVENTS = [")
        .expect("modBridge.ts has MOD_EVENTS");
    let line = &page[from..];
    let line = &line[..line.find("] as const").expect("MOD_EVENTS ends")];
    for name in EVENTS {
        assert!(
            line.contains(&format!("\"{name}\"")),
            "the page drops {name}"
        );
    }
    assert_eq!(line.matches('"').count() / 2, EVENTS.len(), "{line}");
    // Every event's row is where its permission says it is.
    for name in EVENTS {
        let row = table::event_row(table::ROWS, name, MOD_API_VERSION).unwrap();
        assert_eq!(row.name, *name);
    }
    // The lifecycle two need no permission; the data ones need the one the spec names.
    let permission = |n| {
        table::event_row(table::ROWS, n, MOD_API_VERSION)
            .unwrap()
            .permission
    };
    assert_eq!(permission(DOCUMENTS_ADDED), Some("documents.meta"));
    assert_eq!(permission(DOCUMENTS_DELETED), Some("documents.meta"));
    assert_eq!(permission(THEME_CHANGED), Some("storage"));
    assert_eq!(permission(GRANTS_CHANGED), None);
    assert_eq!(permission(outbox::MOD_PAUSED), None);
    assert_eq!(permission(outbox::SESSION_CLOSED), None);
}
