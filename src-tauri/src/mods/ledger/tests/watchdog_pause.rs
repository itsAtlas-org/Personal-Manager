// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The frozen-page watchdog's pause as the ledger carries it out (slice 6, part 4): a mod whose
//! frame froze PM's page is paused with its own reason, its sessions end the way a pause's do, and
//! only a person's resume lifts it, never a restart, the safety list, developer mode or an update.

use super::*;
use crate::mods::index::Index;
use crate::mods::outbox::testing::Recorder;
use crate::mods::outbox::{MOD_PAUSED, SESSION_CLOSED};
use crate::mods::safety::tests::{index_with, now as list_now};
use crate::mods::safety::SafetyCheck;
use serde_json::json;
use tauri::http::Request;

fn live_frame(w: &World) -> Recorder {
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
    rec
}

fn row_state(w: &World) -> String {
    w.sql("SELECT state || ':' || COALESCE(state_reason, '') FROM mods WHERE id = 'atlas.map'")
}

fn map() -> ModId {
    id("atlas.map")
}

fn pause_index() -> Index {
    index_with(|v| {
        v["blacklist"] = json!([{"id": "atlas.map", "versions": "=1.0.0", "action": "paused",
            "reason": "It loses notes.", "fixedIn": "1.0.1"}]);
    })
}

fn installed() -> World {
    let w = world();
    w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
        .unwrap();
    w
}

#[test]
fn the_watchdog_pauses_the_mod_with_its_own_reason_and_ends_its_sessions_like_a_pause() {
    let w = installed();
    let frame = live_frame(&w);
    assert!(w.ledger().pause_frozen(&map(), now()).unwrap());
    assert_eq!(row_state(&w), "paused:frozen");
    assert!(!w.runtime.is_installed(&map()));
    assert_eq!(frame.names(), [MOD_PAUSED, SESSION_CLOSED]);
    // A second time is nothing to write.
    assert!(!w.ledger().pause_frozen(&map(), now()).unwrap());
    assert_eq!(row_state(&w), "paused:frozen");
}

#[test]
fn it_never_takes_over_a_stop_that_isnt_a_running_mods() {
    // Already paused by the user: left as theirs, nothing written, not an error.
    let w = installed();
    w.ledger().pause_by_user(&map(), now()).unwrap();
    assert!(!w.ledger().pause_frozen(&map(), now()).unwrap());
    assert_eq!(row_state(&w), "paused:user");
    // Paused by the safety list, or parked, or waiting for its first-open turn on: refused.
    let w = installed();
    w.ledger().apply_blacklist(&pause_index(), list_now());
    assert_eq!(row_state(&w), "paused:blacklist");
    assert!(matches!(
        w.ledger().pause_frozen(&map(), now()),
        Err(LedgerError::Change(ChangeRefusal::NotRunning { .. }))
    ));
    assert_eq!(row_state(&w), "paused:blacklist");
    let w = installed();
    w.exec("UPDATE mods SET state = 'parked', state_reason = 'files-missing'");
    assert!(matches!(
        w.ledger().pause_frozen(&map(), now()),
        Err(LedgerError::Change(ChangeRefusal::NotRunning { .. }))
    ));
    // A mod that isn't installed at all.
    let w = world();
    assert!(matches!(
        w.ledger().pause_frozen(&map(), now()),
        Err(LedgerError::NotInstalled)
    ));
    // And the user's own pause on a frozen mod is already paused: it doesn't take the reason over.
    let w = installed();
    w.ledger().pause_frozen(&map(), now()).unwrap();
    w.ledger().pause_by_user(&map(), now()).unwrap();
    assert_eq!(row_state(&w), "paused:frozen");
}

#[test]
fn a_frozen_pause_survives_a_restart_a_load_the_safety_list_and_an_update() {
    let w = installed();
    w.ledger().pause_frozen(&map(), now()).unwrap();
    // A restart's startup, and a plain reload of the registry.
    w.runtime.clear();
    w.ledger().startup(now(), &serve_all);
    assert_eq!(row_state(&w), "paused:frozen");
    assert!(!w.runtime.is_installed(&map()));
    let rows = w.ledger().with_conn(|c| Ok(read_rows(c)?)).unwrap();
    w.ledger().load(&rows, now(), &serve_all);
    assert_eq!(row_state(&w), "paused:frozen");
    // The safety list pausing and then lifting its entry leaves another owner's pause alone: it
    // doesn't take it over, and doesn't turn the mod back on when the entry goes.
    let index = pause_index();
    w.ledger().apply_blacklist(&index, list_now());
    assert_eq!(row_state(&w), "paused:frozen");
    let lifted = index_with(|_| {});
    w.ledger().apply_blacklist(&lifted, list_now());
    assert_eq!(row_state(&w), "paused:frozen");
    // An update installs but doesn't switch it on.
    w.install(&pkg("1.1.0", |_| {}), Approval::Approved)
        .unwrap();
    assert_eq!(row_state(&w), "paused:frozen");
    assert!(!w.runtime.is_installed(&map()));
}

#[test]
fn only_the_users_resume_lifts_it_and_the_safety_list_still_has_its_say() {
    let w = installed();
    w.ledger().pause_frozen(&map(), now()).unwrap();
    // The safety list names the mod now: the resume is refused, with the list's words, and the mod
    // stays frozen.
    let listed = pause_index();
    let check = SafetyCheck::serve(Some(&listed));
    let refused = w.ledger().resume_by_user(&map(), &check, now());
    assert!(matches!(
        refused,
        Err(LedgerError::Change(ChangeRefusal::Listed(_)))
    ));
    assert_eq!(row_state(&w), "paused:frozen");
    // Not listed: the resume turns it back on and the registry serves it.
    let standing = w
        .ledger()
        .resume_by_user(&map(), &serve_all, now())
        .unwrap();
    assert!(standing.serving);
    assert_eq!(row_state(&w), "active:");
    assert!(w.runtime.is_installed(&map()));
    // Resuming a mod that isn't paused for either of those reasons is still refused.
    assert!(matches!(
        w.ledger().resume_by_user(&map(), &serve_all, now()),
        Err(LedgerError::Change(ChangeRefusal::NotYourPause { .. }))
    ));
}

#[test]
fn developer_mode_off_pauses_an_unsigned_mod_and_on_does_not_resume_a_frozen_one() {
    use crate::mods::devmode::{self, test_here};
    use crate::mods::package::tests::unsigned_package;
    const DEVICE: &str = "dddddddddddddddddddddddddddddddd";
    let w = world();
    {
        let mut c = w.store.conn.lock().unwrap();
        devmode::write(&mut c, test_here(DEVICE), true).unwrap();
    }
    w.runtime.set_unsigned_allowed(true);
    let file = unsigned_package(|_, _| {});
    let pkg = package::verify_unsigned(&file).unwrap();
    w.ledger()
        .install_unsigned(
            &pkg,
            &hex::encode(Sha256::digest(&file)),
            &UnsignedOptions {
                device: test_here(DEVICE),
                index: None,
            },
            now(),
        )
        .unwrap();
    let tool = id("acme.tool");
    w.ledger().pause_frozen(&tool, now()).unwrap();
    for on in [false, true] {
        w.ledger()
            .set_dev_mode_for_device(test_here(DEVICE), on, &serve_all, now())
            .unwrap();
        assert_eq!(
            w.sql::<String>("SELECT state || ':' || state_reason FROM mods"),
            "paused:frozen",
            "developer mode {on}"
        );
    }
    assert!(!w.runtime.is_installed(&tool));
}
