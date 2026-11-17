// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The ledger's writers against the registry's frames: whatever changes a row's state outside the
//! dispatcher ends the mod's live frames the right way. A frame told `mod.paused` knows the mod is
//! still installed and may come back; one told "uninstalled" knows it won't; one told "replaced"
//! should reload. A Mods tab built on the goodbye shows exactly that, so a pause reported as an
//! uninstall (or the reverse) is a lie the user reads.
//!
//! Each test opens a real session over the ledger tests' own world (real store, real files, a
//! recording channel) and reads what the channel heard.

use super::*;
use crate::mods::index::Index;
use crate::mods::outbox::testing::Recorder;
use crate::mods::outbox::{MOD_PAUSED, SESSION_CLOSED};
use crate::mods::safety::tests::{index_with, now as list_now};
use crate::mods::safety::SafetyCheck;
use serde_json::json;
use tauri::http::Request;

/// A live frame of `atlas.map`: its session, and what its channel hears.
fn live_frame(w: &World) -> (String, Recorder) {
    let rec = Recorder::default();
    let opened = w
        .runtime
        .open_frame_in(&id("atlas.map"), 0, rec.outbox())
        .unwrap();
    let req = Request::builder()
        .uri(opened.entry_url.as_str())
        .body(Vec::new())
        .unwrap();
    assert_eq!(w.runtime.serve("main", &req).status(), 200);
    (opened.session, rec)
}

fn reason(rec: &Recorder) -> Value {
    rec.payloads(SESSION_CLOSED)[0]["reason"].clone()
}

/// The row's `state:reason`.
fn row_state(w: &World) -> String {
    w.sql("SELECT state || ':' || COALESCE(state_reason, '') FROM mods")
}

/// The two ways a frame can be left: `mod.paused` then `session.closed`, or `session.closed` alone.
const PAUSED: [&str; 2] = [MOD_PAUSED, SESSION_CLOSED];
const CLOSED_ONLY: [&str; 1] = [SESSION_CLOSED];

fn pause_index() -> Index {
    index_with(|v| {
        v["blacklist"] = json!([{"id": "atlas.map", "versions": "=1.0.0", "action": "paused",
            "reason": "It loses notes.", "fixedIn": "1.0.1"}]);
    })
}

fn removal_index() -> Index {
    index_with(|v| {
        v["blacklist"] = json!([{"id": "atlas.map", "versions": "=1.0.0", "action": "removed",
            "reason": "It sends your documents elsewhere."}]);
    })
}

// ---- the safety list's cut ---------------------------------------------------------------------

#[test]
fn a_frame_of_a_mod_the_blacklist_pauses_hears_paused_then_closed_and_nothing_after() {
    let w = world();
    w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
        .unwrap();
    let (session, rec) = live_frame(&w);
    // A second frame of the same mod hears it too, each on its own channel.
    let other = live_frame(&w);

    let index = pause_index();
    let report = w.ledger().apply_blacklist(&index, list_now());
    assert_eq!(report.notices.len(), 1);
    assert_eq!(rec.names(), PAUSED);
    assert_eq!(other.1.names(), PAUSED);
    assert_eq!(reason(&rec), json!("paused"));
    assert!(
        w.runtime.begin_call(&session).is_err(),
        "the session is gone"
    );

    // Nothing after: the next pass changes nothing, a startup load leaves it stopped and tells no
    // one, and an install of the fixed version (the row's pause survives it) sends nothing to a
    // channel that has said goodbye.
    assert!(w.ledger().apply_blacklist(&index, list_now()).is_empty());
    w.ledger().startup(now(), &SafetyCheck::serve(Some(&index)));
    w.install(&pkg("1.0.1", |_| {}), Approval::Approved)
        .unwrap();
    for rec in [&rec, &other.1] {
        assert_eq!(rec.names(), PAUSED, "no event after the goodbye");
    }
}

#[test]
fn a_frame_of_a_mod_the_blacklist_removes_hears_uninstalled_and_nothing_after() {
    let w = world();
    w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
        .unwrap();
    let (session, rec) = live_frame(&w);

    let index = removal_index();
    let report = w.ledger().apply_blacklist(&index, list_now());
    assert_eq!(report.notices.len(), 1);
    assert_eq!(rec.names(), CLOSED_ONLY, "a removal is not a pause");
    assert_eq!(reason(&rec), json!("uninstalled"));
    assert!(w.runtime.begin_call(&session).is_err());

    // The row waits for the user's answer, paused; the repeat pass (finishing the removal) and a
    // startup load have no frame to tell.
    assert_eq!(row_state(&w), "paused:blacklist-removed");
    assert!(w.ledger().apply_blacklist(&index, list_now()).is_empty());
    w.ledger().startup(now(), &SafetyCheck::serve(Some(&index)));
    assert_eq!(rec.names(), CLOSED_ONLY);
}

#[test]
fn a_frame_opened_before_a_pause_is_not_revived_by_the_resume() {
    let w = world();
    w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
        .unwrap();
    let (old_session, rec) = live_frame(&w);
    w.ledger().apply_blacklist(&pause_index(), list_now());
    assert_eq!(rec.names(), PAUSED);

    // The fixed version lands and the list lifts: the mod runs again, for new frames only.
    w.install(&pkg("1.0.1", |_| {}), Approval::Approved)
        .unwrap();
    w.ledger().apply_blacklist(&index_with(|_| {}), list_now());
    assert_eq!(row_state(&w), "active:");
    assert!(w.runtime.begin_call(&old_session).is_err());
    assert_eq!(rec.names(), PAUSED, "the dead channel hears nothing more");
    let (fresh, fresh_rec) = live_frame(&w);
    assert!(w.runtime.begin_call(&fresh).is_ok());
    assert!(fresh_rec.all().is_empty());
}

// ---- the ledger's own writers ------------------------------------------------------------------

#[test]
fn an_uninstall_tells_its_frames_uninstalled() {
    let w = world();
    w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
        .unwrap();
    let (_, rec) = live_frame(&w);
    w.ledger().uninstall(&id("atlas.map")).unwrap();
    assert_eq!(rec.names(), CLOSED_ONLY);
    assert_eq!(reason(&rec), json!("uninstalled"));
}

#[test]
fn an_update_and_a_repair_tell_their_frames_replaced() {
    let w = world();
    let first = pkg("1.0.0", |_| {});
    w.install(&first, Approval::Approved).unwrap();

    let (_, on_update) = live_frame(&w);
    w.install(&pkg("1.1.0", |_| {}), Approval::Approved)
        .unwrap();
    assert_eq!(on_update.names(), CLOSED_ONLY);
    assert_eq!(reason(&on_update), json!("replaced"));

    // A repair is the very same package again (the fixture is signed afresh on every call, so the
    // bytes are kept and installed twice).
    let same = pkg("1.2.0", |_| {});
    w.install(&same, Approval::Approved).unwrap();
    let (_, on_repair) = live_frame(&w);
    let got = w.install(&same, Approval::Approved).unwrap();
    assert_eq!(got.kind, InstallKind::Repair);
    assert_eq!(on_repair.names(), CLOSED_ONLY);
    assert_eq!(reason(&on_repair), json!("replaced"));
}

#[test]
fn an_install_over_a_row_that_does_not_serve_pauses_its_frames_not_uninstalls_them() {
    let w = world();
    w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
        .unwrap();
    let (_, rec) = live_frame(&w);
    // Another owner paused the row (the user's pause, dev mode's) and hasn't told the registry yet.
    w.exec("UPDATE mods SET state = 'paused', state_reason = 'user'");
    let got = w
        .install(&pkg("1.1.0", |_| {}), Approval::Approved)
        .unwrap();
    assert_eq!(got.state, "paused", "the pause outlives the update");
    assert_eq!(rec.names(), PAUSED);
    assert_eq!(reason(&rec), json!("paused"));
    assert_eq!(w.served_from(), None);
}

#[test]
fn a_load_that_parks_or_holds_a_mod_pauses_its_frames_and_one_that_lost_its_row_uninstalls_them() {
    // Parked: its files went missing.
    let w = world();
    w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
        .unwrap();
    let (_, rec) = live_frame(&w);
    fs::remove_dir_all(w.version_dir("1.0.0")).unwrap();
    let report = w.ledger().startup(now(), &SafetyCheck::serve(None));
    assert_eq!(report.parked.len(), 1);
    assert_eq!(
        rec.names(),
        PAUSED,
        "parked is a pause, the row is still there"
    );
    assert_eq!(reason(&rec), json!("paused"));

    // Held: the safety list's serve check keeps a row out of the registry. A fresh world, a
    // listing that blacklists the version the apply has not reached yet.
    let w = world();
    w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
        .unwrap();
    let (_, rec) = live_frame(&w);
    let index = pause_index();
    let report = w.ledger().startup(now(), &SafetyCheck::serve(Some(&index)));
    assert_eq!(report.held.len(), 1);
    assert_eq!(rec.names(), PAUSED);

    // No row any more (it was removed under the registry): that one is gone.
    let w = world();
    w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
        .unwrap();
    let (_, rec) = live_frame(&w);
    w.exec("DELETE FROM mods");
    w.ledger().startup(now(), &SafetyCheck::serve(None));
    assert_eq!(rec.names(), CLOSED_ONLY);
    assert_eq!(reason(&rec), json!("uninstalled"));
}

#[test]
fn a_load_that_changes_nothing_leaves_every_frame_alone() {
    let w = world();
    w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
        .unwrap();
    let (session, rec) = live_frame(&w);
    w.ledger().startup(now(), &SafetyCheck::serve(None));
    assert!(rec.all().is_empty());
    assert!(w.runtime.begin_call(&session).is_ok());
}

#[test]
fn a_closed_store_ends_every_frame_without_saying_anything_false() {
    let w = world();
    w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
        .unwrap();
    let (session, rec) = live_frame(&w);
    // `on_store_closed`, `settle_registry` and `quiesce` all do this.
    w.runtime.clear();
    assert!(w.runtime.begin_call(&session).is_err());
    assert!(
        rec.all().is_empty(),
        "no mod was paused or uninstalled: {:?}",
        rec.names()
    );
    assert_eq!(w.served_from(), None);
}
