// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Mods tab's writers against the ledger (slice 6, part 2): the user's pause, resume and
//! first-open "Turn on"; the two-step signed install (the consent screen, then the staged bytes and
//! the user's answers as grants); the uninstall with its gifts; the icon. Over the ledger tests' own
//! world: a real store, real files, a recording channel.

use super::*;
use crate::mods::api::grants::GrantError;
use crate::mods::index::Index;
use crate::mods::keys::{AtlasKey, KeyId, KeyRing, KeyRole};
use crate::mods::outbox::testing::Recorder;
use crate::mods::outbox::{MOD_PAUSED, SESSION_CLOSED};
use crate::mods::safety::tests::{entry_for, index_listing, index_with, now as list_now};
use crate::mods::safety::{Mode, SafetyCheck};
use crate::mods::tab::catalogue;
use crate::mods::tab::consent::{self, Answers};
use crate::mods::tab::error::{Code, ModsError};
use crate::mods::tab::install::{self, Env};
use crate::mods::tab::stage::{Stage, Staged};
use crate::mods::tab::{disclaimer, icon, listing, uninstall};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Instant;
use tauri::http::Request;

const DEVICE: &str = "dddddddddddddddddddddddddddddddd";

/// A live frame of `atlas.map` and what its channel hears.
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

fn state(w: &World) -> String {
    w.sql("SELECT state || ':' || COALESCE(state_reason, '') FROM mods")
}

fn consented(w: &World) -> bool {
    w.sql::<i64>("SELECT consented_at IS NOT NULL FROM mods") == 1
}

fn serves(w: &World) -> bool {
    w.runtime.is_installed(&id("atlas.map"))
}

fn grants_of(w: &World) -> BTreeMap<String, String> {
    let c = w.store.conn.lock().unwrap();
    let mut stmt = c
        .prepare("SELECT permission, decision FROM mod_grants WHERE mod_id = 'atlas.map'")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn serve_all() -> impl ServeCheck {
    |_: &str, _: &str, _: Option<&str>| None
}

fn listing_index() -> Index {
    index_with(|v| {
        v["blacklist"] = json!([{"id": "atlas.map", "versions": "=1.0.0", "action": "paused",
            "reason": "It loses notes.", "fixedIn": "1.0.1"}]);
    })
}

fn answers(items: &[(&str, bool)]) -> Answers {
    items.iter().map(|(p, a)| ((*p).to_owned(), *a)).collect()
}

fn install_unconsented(w: &World, version: &str) {
    let (bytes, key) = pkg(version, |_| {});
    let opts = InstallOptions {
        consented: false,
        ..opts(Approval::Approved)
    };
    w.ledger()
        .install_package(
            &bytes,
            &TrustedKeys::of_test_keys(std::slice::from_ref(&key)),
            &opts,
            now(),
        )
        .unwrap();
}

// ---- the user's pause --------------------------------------------------------------------------

#[test]
fn a_pause_ends_the_frames_after_the_row_commits_and_outlives_an_update_and_a_lifted_entry() {
    let w = world();
    w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
        .unwrap();
    let rec = live_frame(&w);

    w.ledger().pause_by_user(&id("atlas.map"), now()).unwrap();
    assert_eq!(state(&w), "paused:user");
    assert_eq!(rec.names(), [MOD_PAUSED, SESSION_CLOSED]);
    assert!(!serves(&w));
    // Pausing again says nothing more and changes nothing.
    w.ledger().pause_by_user(&id("atlas.map"), now()).unwrap();
    assert_eq!(rec.names().len(), 2);

    // An update keeps the user's stop (it never resumes by itself).
    w.install(&pkg("1.1.0", |_| {}), Approval::Approved)
        .unwrap();
    assert_eq!(state(&w), "paused:user");
    assert!(!serves(&w));
    // The safety list leaves it alone, and when its entry is lifted does not switch it back on.
    let paused_entry = index_with(|v| {
        v["blacklist"] = json!([{"id": "atlas.map", "versions": "=1.1.0", "action": "paused",
            "reason": "It loses notes."}]);
    });
    assert!(w
        .ledger()
        .apply_blacklist(&paused_entry, list_now())
        .notices
        .is_empty());
    let lifted = index_with(|v| v["blacklist"] = json!([]));
    assert!(w
        .ledger()
        .apply_blacklist(&lifted, list_now())
        .notices
        .is_empty());
    assert_eq!(state(&w), "paused:user");
}

#[test]
fn a_pause_is_refused_for_a_mod_that_is_not_running_and_changes_nothing() {
    let w = world();
    install_unconsented(&w, "1.0.0");
    let refuse = |w: &World| match w.ledger().pause_by_user(&id("atlas.map"), now()) {
        Err(LedgerError::Change(why)) => why,
        other => panic!("{other:?}"),
    };
    // Not turned on yet.
    assert!(matches!(refuse(&w), ChangeRefusal::NotRunning { .. }));
    assert_eq!(state(&w), "active:");
    // Parked, paused by the safety list, removed by it.
    for (set, want_removed) in [
        ("state = 'parked', state_reason = 'files-missing'", false),
        ("state = 'paused', state_reason = 'blacklist'", false),
        ("state = 'paused', state_reason = 'blacklist-removed'", true),
    ] {
        w.exec(&format!(
            "UPDATE mods SET consented_at = '2026-11-05T00:00:00.000Z', {set}"
        ));
        let before = state(&w);
        let why = refuse(&w);
        assert_eq!(matches!(why, ChangeRefusal::Removed), want_removed, "{set}");
        assert_eq!(state(&w), before, "{set}");
    }
    // A mod that isn't installed.
    assert!(matches!(
        w.ledger().pause_by_user(&id("acme.nothing"), now()),
        Err(LedgerError::NotInstalled)
    ));
}

#[test]
fn a_row_that_changed_under_the_check_loses_the_compare_and_set() {
    let w = world();
    w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
        .unwrap();
    // Active with a reason no writer leaves: the decision says pause, the write finds no match.
    w.exec("UPDATE mods SET state_reason = 'odd'");
    assert!(matches!(
        w.ledger().pause_by_user(&id("atlas.map"), now()),
        Err(LedgerError::Change(ChangeRefusal::Changed))
    ));
    assert_eq!(state(&w), "active:odd");
}

// ---- resume --------------------------------------------------------------------------------------

#[test]
fn a_resume_lifts_only_the_users_pause_and_serves_the_mod_again() {
    let w = world();
    w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
        .unwrap();
    w.ledger().pause_by_user(&id("atlas.map"), now()).unwrap();
    let got = w
        .ledger()
        .resume_by_user(&id("atlas.map"), &serve_all(), now())
        .unwrap();
    assert_eq!(
        got,
        Standing {
            state: "active".into(),
            reason: None,
            serving: true
        }
    );
    assert_eq!(state(&w), "active:");
    assert!(serves(&w));
    // Not paused by the user any more: nothing to resume.
    assert!(matches!(
        w.ledger()
            .resume_by_user(&id("atlas.map"), &serve_all(), now()),
        Err(LedgerError::Change(ChangeRefusal::NotYourPause { .. }))
    ));
    // Another owner's stops are never lifted from here.
    for (set, removed) in [
        ("state = 'paused', state_reason = 'blacklist'", false),
        ("state = 'paused', state_reason = 'dev-mode'", false),
        ("state = 'parked', state_reason = 'files-missing'", false),
        ("state = 'paused', state_reason = 'blacklist-removed'", true),
    ] {
        w.exec(&format!("UPDATE mods SET {set}"));
        match w
            .ledger()
            .resume_by_user(&id("atlas.map"), &serve_all(), now())
        {
            Err(LedgerError::Change(ChangeRefusal::Removed)) => assert!(removed, "{set}"),
            Err(LedgerError::Change(ChangeRefusal::NotYourPause { .. })) => {
                assert!(!removed, "{set}")
            }
            other => panic!("{set}: {other:?}"),
        }
        assert_eq!(
            state(&w),
            set.replace("state = ", "")
                .replace(", state_reason = ", ":")
                .replace('\'', "")
        );
    }
}

#[test]
fn a_resume_asks_the_safety_list_and_a_listed_mod_stays_off_with_its_words() {
    let w = world();
    w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
        .unwrap();
    w.ledger().pause_by_user(&id("atlas.map"), now()).unwrap();
    let index = listing_index();
    let check = SafetyCheck::serve(Some(&index));
    match w.ledger().resume_by_user(&id("atlas.map"), &check, now()) {
        Err(LedgerError::Change(ChangeRefusal::Listed(words))) => {
            assert_eq!(words, "PM's safety list: It loses notes.")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(state(&w), "paused:user");
    assert!(!serves(&w));
    // The entry matched by the file's hash holds it too.
    let sha: String = w.sql("SELECT package_sha256 FROM mods");
    let by_hash = index_with(|v| {
        v["blacklist"] = json!([{"sha256": sha, "action": "removed", "reason": "Known bad."}]);
    });
    assert!(matches!(
        w.ledger()
            .resume_by_user(&id("atlas.map"), &SafetyCheck::serve(Some(&by_hash)), now()),
        Err(LedgerError::Change(ChangeRefusal::Listed(_)))
    ));
    // With the entry gone it resumes.
    assert!(
        w.ledger()
            .resume_by_user(&id("atlas.map"), &SafetyCheck::serve(None), now())
            .unwrap()
            .serving
    );
}

#[test]
fn a_resume_parks_a_mod_whose_files_went_missing_while_it_was_paused() {
    let w = world();
    w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
        .unwrap();
    w.ledger().pause_by_user(&id("atlas.map"), now()).unwrap();
    fs::remove_dir_all(w.version_dir("1.0.0")).unwrap();
    let got = w
        .ledger()
        .resume_by_user(&id("atlas.map"), &serve_all(), now())
        .unwrap();
    assert_eq!(
        got,
        Standing {
            state: "parked".into(),
            reason: Some("files-missing".into()),
            serving: false
        }
    );
    assert!(!serves(&w));
}

// ---- the first-open "Turn on" ------------------------------------------------------------------------

#[test]
fn turning_on_a_pilot_sets_consent_writes_the_grants_and_the_disclaimer_and_loads_it() {
    let w = world();
    install_unconsented(&w, "1.0.0");
    assert!(!consented(&w) && !serves(&w), "waits for its first open");
    let got = w
        .ledger()
        .consent(&id("atlas.map"), &serve_all(), now(), |conn| {
            let plan = consent::GrantPlan {
                grant: vec!["documents.meta".into()],
                ..consent::GrantPlan::default()
            };
            consent::apply(conn, &id("atlas.map"), &plan, true)?;
            disclaimer::record(conn, DEVICE).map_err(|e| GrantError::Db(e.to_string()))
        })
        .unwrap();
    assert!(got.serving && got.state == "active");
    assert!(consented(&w) && serves(&w));
    assert_eq!(
        grants_of(&w),
        BTreeMap::from([("documents.meta".into(), "granted".into())])
    );
    let accepted = {
        let c = w.store.conn.lock().unwrap();
        disclaimer::is_accepted(&c, DEVICE).unwrap()
    };
    assert!(accepted, "Turn on counts as accepting the disclaimer");
    // The consent time is the moment of the click.
    assert_eq!(
        w.sql::<String>("SELECT consented_at FROM mods"),
        "2026-11-05T12:00:00.000Z"
    );
}

#[test]
fn a_refused_grant_at_turn_on_leaves_the_mod_off_and_rolls_back_whatever_was_written() {
    let w = world();
    install_unconsented(&w, "1.0.0");
    let got = w
        .ledger()
        .consent(&id("atlas.map"), &serve_all(), now(), |conn| {
            // One row lands, then the writer refuses: both must go.
            consent::apply(
                conn,
                &id("atlas.map"),
                &consent::GrantPlan {
                    grant: vec!["documents.meta".into()],
                    ..consent::GrantPlan::default()
                },
                true,
            )?;
            disclaimer::record(conn, DEVICE).map_err(|e| GrantError::Db(e.to_string()))?;
            Err(GrantError::Undeclared("internet".into()))
        });
    assert!(matches!(
        got,
        Err(LedgerError::Grants(GrantError::Undeclared(_)))
    ));
    assert!(!consented(&w) && !serves(&w));
    assert!(grants_of(&w).is_empty());
    let accepted = {
        let c = w.store.conn.lock().unwrap();
        disclaimer::is_accepted(&c, DEVICE).unwrap()
    };
    assert!(!accepted, "the disclaimer rolls back with the rest");
}

#[test]
fn turning_on_twice_and_turning_on_a_removed_mod_are_refused() {
    let w = world();
    install_unconsented(&w, "1.0.0");
    let on = |w: &World| {
        w.ledger()
            .consent(&id("atlas.map"), &serve_all(), now(), |_| Ok(()))
    };
    on(&w).unwrap();
    assert!(matches!(
        on(&w),
        Err(LedgerError::Change(ChangeRefusal::AlreadyOn))
    ));
    w.exec(
        "UPDATE mods SET consented_at = NULL, state = 'paused', state_reason = 'blacklist-removed'",
    );
    assert!(matches!(
        on(&w),
        Err(LedgerError::Change(ChangeRefusal::Removed))
    ));
    assert!(!consented(&w));
    assert!(matches!(
        w.ledger()
            .consent(&id("acme.nothing"), &serve_all(), now(), |_| Ok(())),
        Err(LedgerError::NotInstalled)
    ));
}

#[test]
fn a_turn_on_of_a_mod_the_safety_list_names_is_consented_but_not_served() {
    let w = world();
    install_unconsented(&w, "1.0.0");
    let index = listing_index();
    let got = w
        .ledger()
        .consent(
            &id("atlas.map"),
            &SafetyCheck::serve(Some(&index)),
            now(),
            |_| Ok(()),
        )
        .unwrap();
    assert!(consented(&w));
    assert!(!got.serving, "the list holds it back at the load");
}

// ---- installing with consent ----------------------------------------------------------------------------

struct Fixture {
    bytes: Vec<u8>,
    keys: TrustedKeys,
}

fn fixture(version: &str, change: impl FnOnce(&mut Value)) -> Fixture {
    let (bytes, key) = pkg(version, change);
    Fixture {
        bytes,
        keys: TrustedKeys::of_test_keys(std::slice::from_ref(&key)),
    }
}

fn env<'a>(keys: &'a TrustedKeys, index: Option<&'a Index>) -> Env<'a> {
    Env {
        now: list_now(),
        keys,
        index,
        engine_sealed: true,
    }
}

fn prepare(
    w: &World,
    f: &Fixture,
    source: Source,
    index: Option<&Index>,
) -> Result<(install::ConsentScreen, Staged), ModsError> {
    install::prepare(&w.ledger(), f.bytes.clone(), source, &env(&f.keys, index))
}

fn nothing_installed(w: &World) {
    assert_eq!(w.rows(), 0);
    assert!(w.installed_tree().is_empty() && w.staging_is_empty());
    assert!(!serves(w));
}

#[test]
fn the_consent_screen_installs_nothing_and_says_what_the_file_is() {
    let w = world();
    let f = fixture("1.0.0", |_| {});
    let (screen, staged) = prepare(&w, &f, Source::File, None).unwrap();
    nothing_installed(&w);
    assert_eq!(
        (
            screen.id.as_str(),
            screen.name.as_str(),
            screen.version.as_str(),
            screen.kind
        ),
        ("atlas.map", "Map", "1.0.0", "fresh")
    );
    assert_eq!((screen.tier.as_str(), screen.from), ("first", "file"));
    assert_eq!(screen.sha256, hex::encode(Sha256::digest(&f.bytes)));
    assert_eq!(staged.sha256, screen.sha256);
    assert_eq!(staged.bytes, f.bytes);
    let by: BTreeMap<&str, bool> = screen
        .permissions
        .iter()
        .map(|p| (p.permission.id.as_str(), p.answer_required))
        .collect();
    assert_eq!(
        by,
        BTreeMap::from([
            ("storage", false),
            ("ui.tab", false),
            ("documents.meta", true)
        ])
    );
}

#[test]
fn a_file_every_check_refuses_never_reaches_a_screen_and_installs_nothing() {
    let w = world();
    let f = fixture("1.0.0", |_| {});
    let sha = hex::encode(Sha256::digest(&f.bytes));
    let code = |r: Result<(install::ConsentScreen, Staged), ModsError>| r.unwrap_err().code;

    // A key this machine doesn't trust, and one it has seen revoked.
    let stranger = fixture("1.0.0", |_| {});
    let wrong = install::prepare(
        &w.ledger(),
        f.bytes.clone(),
        Source::File,
        &env(&stranger.keys, None),
    );
    assert_eq!(code(wrong), Code::NotAMod);
    let signer = KeyId::parse("00000000000000AA").unwrap();
    let (bytes, key) = pkg("1.0.0", |_| {});
    let ring = KeyRing::of([AtlasKey {
        role: KeyRole::Main,
        id: signer,
        key,
    }]);
    let revoked = ring.trusted(&BTreeSet::from([signer]));
    let r = install::prepare(&w.ledger(), bytes, Source::File, &env(&revoked, None));
    assert_eq!(code(r), Code::NotAMod);
    // Not a package at all.
    let r = install::prepare(
        &w.ledger(),
        b"nope".to_vec(),
        Source::File,
        &env(&f.keys, None),
    );
    assert_eq!(code(r), Code::NotAMod);

    // The safety list: by hash, and decision 11's sha256 where it lists this id and version.
    let (verified, vsha) = crate::mods::safety::tests::verified("1.0.0");
    assert_ne!(vsha, sha, "another signing run, another file");
    let by_hash = index_with(|v| {
        v["blacklist"] = json!([{"sha256": sha, "action": "removed", "reason": "Known bad."}]);
    });
    assert_eq!(
        code(prepare(&w, &f, Source::File, Some(&by_hash))),
        Code::SafetyList
    );
    let mismatch = index_listing(
        {
            let mut e = entry_for(&verified, &vsha);
            e["version"] = json!("1.0.0");
            e
        },
        json!([]),
    );
    assert_eq!(
        code(prepare(&w, &f, Source::File, Some(&mismatch))),
        Code::SafetyList
    );
    // The same file offered as the catalogue's, with no index at all: refused too.
    assert_eq!(
        code(prepare(&w, &f, Source::Catalogue, None)),
        Code::SafetyList
    );

    // The ledger's own rules, through the same preview as the install.
    let newer_api = fixture("1.0.0", |m| {
        m["apiVersion"] = json!({"min": "1.1", "target": "1.1"})
    });
    assert_eq!(
        code(prepare(&w, &newer_api, Source::File, None)),
        Code::NeedsNewerPm
    );
    nothing_installed(&w);

    // Against an installed 1.1.0: older, and another publisher.
    w.install(&pkg("1.1.0", |_| {}), Approval::Approved)
        .unwrap();
    let before = w.installed_tree();
    assert_eq!(code(prepare(&w, &f, Source::File, None)), Code::Refused);
    let other = fixture("1.2.0", |m| {
        m["publisher"]["author"] = json!("Someone Else")
    });
    assert_eq!(code(prepare(&w, &other, Source::File, None)), Code::Refused);
    assert_eq!(w.installed_tree(), before);
}

#[test]
fn the_catalogues_own_file_passes_decision_11_and_another_versions_listing_says_nothing() {
    let w = world();
    let f = fixture("1.2.0", |_| {});
    let (verified, _) = crate::mods::safety::tests::verified("1.2.0");
    let sha = hex::encode(Sha256::digest(&f.bytes));
    // The index lists exactly this file: both sources pass.
    let listed = index_listing(entry_for(&verified, &sha), json!([]));
    assert!(prepare(&w, &f, Source::File, Some(&listed)).is_ok());
    assert!(prepare(&w, &f, Source::Catalogue, Some(&listed)).is_ok());
    // The index lists another version of the id: an offline file install still works.
    let g = fixture("1.0.0", |_| {});
    assert!(prepare(&w, &g, Source::File, Some(&listed)).is_ok());
    nothing_installed(&w);
}

/// The staged bytes as the install command gets them back: through the stage, by token.
fn stage_and_take(staged: Staged) -> Staged {
    let stage = Stage::default();
    let t = Instant::now();
    let token = stage.put(staged, t).unwrap();
    assert_eq!(token.len(), 32);
    stage.take(&token, t).unwrap()
}

#[test]
fn finish_installs_what_was_staged_and_writes_the_answers_as_grants() {
    let w = world();
    let f = fixture("1.0.0", |_| {});
    let (_, staged) = prepare(&w, &f, Source::File, None).unwrap();
    let staged = stage_and_take(staged);
    let done = install::finish(
        &w.ledger(),
        staged,
        &answers(&[("documents.meta", true)]),
        &env(&f.keys, None),
    )
    .unwrap();
    assert_eq!(
        (
            done.result.id.as_str(),
            done.result.version.as_str(),
            done.result.state.as_str(),
            done.result.kind
        ),
        ("atlas.map", "1.0.0", "active", "fresh")
    );
    assert!(done.result.grants_saved && done.result.grants_problem.is_none());
    assert!(
        consented(&w) && serves(&w),
        "the user is looking at it: consented, running"
    );
    assert_eq!(
        grants_of(&w),
        BTreeMap::from([("documents.meta".into(), "granted".into())])
    );
    assert_eq!(
        done.told.iter().map(|m| m.as_str()).collect::<Vec<_>>(),
        ["atlas.map"]
    );
    assert_eq!(w.sql::<String>("SELECT installed_from FROM mods"), "file");
}

#[test]
fn an_answer_that_does_not_fit_the_manifest_installs_nothing() {
    let w = world();
    let f = fixture("1.0.0", |_| {});
    let finish = |a: Answers| {
        let (_, staged) = prepare(&w, &f, Source::File, None).unwrap();
        install::finish(&w.ledger(), staged, &a, &env(&f.keys, None))
            .unwrap_err()
            .code
    };
    // The install-time permission with no answer; one the mod never declared; one PM lacks.
    assert_eq!(finish(answers(&[])), Code::BadRequest);
    assert_eq!(
        finish(answers(&[("documents.meta", true), ("internet", true)])),
        Code::BadRequest
    );
    assert_eq!(
        finish(answers(&[("documents.meta", true), ("made.up", true)])),
        Code::BadRequest
    );
    nothing_installed(&w);
}

#[test]
fn a_no_is_recorded_as_a_denial_and_the_mod_installs_without_that_grant() {
    let w = world();
    let f = fixture("1.0.0", |_| {});
    let (_, staged) = prepare(&w, &f, Source::File, None).unwrap();
    install::finish(
        &w.ledger(),
        staged,
        &answers(&[("documents.meta", false)]),
        &env(&f.keys, None),
    )
    .unwrap();
    assert_eq!(
        grants_of(&w),
        BTreeMap::from([("documents.meta".into(), "denied".into())])
    );
    assert!(serves(&w));
}

#[test]
fn a_key_revoked_while_the_screen_waited_is_refused_when_it_installs() {
    let w = world();
    let (bytes, key) = pkg("1.0.0", |_| {});
    let trusted = TrustedKeys::of_test_keys(std::slice::from_ref(&key));
    let (_, staged) =
        install::prepare(&w.ledger(), bytes, Source::File, &env(&trusted, None)).unwrap();
    let signer = KeyId::parse("00000000000000AA").unwrap();
    let ring = KeyRing::of([AtlasKey {
        role: KeyRole::Main,
        id: signer,
        key,
    }]);
    let revoked = ring.trusted(&BTreeSet::from([signer]));
    let r = install::finish(
        &w.ledger(),
        staged,
        &answers(&[("documents.meta", true)]),
        &env(&revoked, None),
    );
    assert_eq!(r.unwrap_err().code, Code::NotAMod);
    nothing_installed(&w);
}

#[test]
fn bytes_that_are_not_what_the_screen_named_are_refused() {
    let w = world();
    let f = fixture("1.0.0", |_| {});
    let (_, mut staged) = prepare(&w, &f, Source::File, None).unwrap();
    staged.version = "9.9.9".into();
    let r = install::finish(
        &w.ledger(),
        staged,
        &answers(&[("documents.meta", true)]),
        &env(&f.keys, None),
    );
    assert_eq!(r.unwrap_err().code, Code::ConsentExpired);
    nothing_installed(&w);
}

#[test]
fn a_grants_failure_leaves_the_mod_installed_with_nothing_granted_and_says_so() {
    let w = world();
    w.exec(
        "CREATE TRIGGER no_grants BEFORE INSERT ON mod_grants \
         BEGIN SELECT RAISE(ABORT, 'disk full'); END",
    );
    let f = fixture("1.0.0", |_| {});
    let (_, staged) = prepare(&w, &f, Source::File, None).unwrap();
    let done = install::finish(
        &w.ledger(),
        staged,
        &answers(&[("documents.meta", true)]),
        &env(&f.keys, None),
    )
    .unwrap();
    assert!(!done.result.grants_saved);
    let problem = done.result.grants_problem.unwrap();
    assert_eq!(problem.code, Code::Internal);
    assert!(
        !problem.message.contains("disk full"),
        "{}",
        problem.message
    );
    assert!(done.told.is_empty());
    assert!(grants_of(&w).is_empty());
    assert!(
        serves(&w),
        "installed, with every install-time permission refusing until granted"
    );
}

#[test]
fn an_update_through_the_screen_marks_what_is_new_and_writes_the_new_grants() {
    let w = world();
    // 1.0.0 installed with documents.meta granted and storage denied.
    let first = fixture("1.0.0", |_| {});
    let (_, staged) = prepare(&w, &first, Source::File, None).unwrap();
    install::finish(
        &w.ledger(),
        staged,
        &answers(&[("documents.meta", true)]),
        &env(&first.keys, None),
    )
    .unwrap();
    w.exec("INSERT INTO mod_grants(mod_id, permission, decision) VALUES ('atlas.map', 'storage', 'denied')");
    // 1.1.0 asks for tasks.read as well.
    let next = fixture("1.1.0", |m| {
        m["permissions"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": "tasks.read", "why": "Shows your tasks."}));
    });
    let (screen, staged) = prepare(&w, &next, Source::Catalogue, None)
        .or_else(|_| prepare(&w, &next, Source::File, None))
        .unwrap();
    assert_eq!(screen.kind, "update");
    assert_eq!(screen.installed_version.as_deref(), Some("1.0.0"));
    assert_eq!(screen.widening, ["permission:tasks.read"]);
    let by: BTreeMap<&str, (bool, bool)> = screen
        .permissions
        .iter()
        .map(|p| (p.permission.id.as_str(), (p.is_new, p.default_answer)))
        .collect();
    assert_eq!(by["tasks.read"], (true, true));
    assert_eq!(by["documents.meta"], (false, true), "kept as granted");
    assert_eq!(
        by["storage"],
        (false, false),
        "an earlier refusal starts as no"
    );
    let done = install::finish(
        &w.ledger(),
        staged,
        &answers(&[("documents.meta", true), ("tasks.read", true)]),
        &env(&next.keys, None),
    )
    .unwrap();
    assert_eq!(done.result.kind, "update");
    assert_eq!(
        grants_of(&w),
        BTreeMap::from([
            ("documents.meta".into(), "granted".into()),
            ("tasks.read".into(), "granted".into()),
            ("storage".into(), "denied".into()),
        ])
    );
    assert_eq!(w.sql::<String>("SELECT version FROM mods"), "1.1.0");
}

#[test]
fn the_installed_file_again_is_a_repair_with_nothing_new() {
    let w = world();
    let f = fixture("1.0.0", |_| {});
    let (_, staged) = prepare(&w, &f, Source::File, None).unwrap();
    install::finish(
        &w.ledger(),
        staged,
        &answers(&[("documents.meta", true)]),
        &env(&f.keys, None),
    )
    .unwrap();
    fs::remove_dir_all(w.version_dir("1.0.0")).unwrap();
    let (screen, _) = prepare(&w, &f, Source::File, None).unwrap();
    assert_eq!(screen.kind, "repair");
    assert!(screen.widening.is_empty());
}

// ---- the catalogue, end to end over the same index ----------------------------------------------------------

#[test]
fn what_the_catalogue_lists_installs_with_the_check_the_listing_used() {
    let w = world();
    let f = fixture("1.2.0", |_| {});
    let (verified, _) = crate::mods::safety::tests::verified("1.2.0");
    let sha = hex::encode(Sha256::digest(&f.bytes));
    let index = index_listing(entry_for(&verified, &sha), json!([]));
    let entry = &index.mods[0];
    assert_eq!(
        catalogue::availability(entry, &index, list_now()),
        catalogue::Availability::Ok
    );
    let (screen, staged) = prepare(&w, &f, Source::Catalogue, Some(&index)).unwrap();
    assert_eq!(screen.from, "catalogue");
    assert_eq!(
        (staged.id.as_str(), staged.version.as_str()),
        (entry.id.as_str(), entry.version.as_str())
    );
    // And a repair of it, as the repair command runs it: the pinned file, whatever the index says.
    install::finish(
        &w.ledger(),
        staged,
        &answers(&[("documents.meta", true)]),
        &env(&f.keys, Some(&index)),
    )
    .unwrap();
    fs::remove_dir_all(w.version_dir("1.2.0")).unwrap();
    w.exec("UPDATE mods SET state = 'parked', state_reason = 'files-missing'");
    let target = w.ledger().repair_target(&id("atlas.map")).unwrap();
    assert_eq!(target.sha256, sha);
    let repaired = w
        .ledger()
        .install_repair(&target, &f.bytes, &f.keys, None, now())
        .unwrap();
    assert_eq!(
        (repaired.kind, repaired.state.as_str()),
        (InstallKind::Repair, "active")
    );
    assert!(serves(&w));
    let _ = SafetyCheck::catalogue(None, now(), Mode::Latest);
}

// ---- uninstalling ---------------------------------------------------------------------------------------------------

fn install_with_data(w: &World) {
    w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
        .unwrap();
    w.exec(
        "INSERT INTO mod_kv(mod_id, key, class, value) VALUES \
            ('atlas.map', 'settings', 'content', 'xxxxxxxxxx'), \
            ('atlas.map', 'layout', 'cache', 'yyyy');
         INSERT INTO documents(vault_path, title, content_hash, source_id) VALUES \
            ('a.md', 'A', 'h1', 'mod:atlas.map:n1'), \
            ('b.md', 'B', 'h2', 'mod:atlas.map:n2'), \
            ('c.md', 'C', 'h3', 'mod:atlas.mapx:n1'), \
            ('d.md', 'D', 'h4', 'note:other');
         INSERT INTO mod_doc_data(mod_id, document_id, key, value) \
            SELECT 'atlas.map', id, 'k', 'zz' FROM documents WHERE vault_path = 'c.md';
         INSERT INTO projects(name) VALUES ('Atlas');
         INSERT INTO project_milestones(project_name, label, due_date) VALUES ('Atlas', 'pitch', '2026-12-01');
         INSERT INTO tasks(title, project_name) VALUES ('gift one', 'Atlas'), ('gift two', 'Atlas'), ('mine', 'Atlas');
         INSERT INTO mod_ledger(mod_id, mod_name, tier, author, source_repo, signed, install_id, task_id)
            SELECT 'atlas.map', 'Map', 'first', 'Atlas', 'r', 1, '0123456789abcdef', id FROM tasks WHERE title LIKE 'gift%';
         INSERT INTO mod_ledger(mod_id, mod_name, tier, author, source_repo, signed, install_id, milestone_id)
            SELECT 'atlas.map', 'Map', 'first', 'Atlas', 'r', 1, '0123456789abcdef', id FROM project_milestones;
         INSERT INTO mod_ledger(mod_id, mod_name, tier, author, source_repo, signed, install_id, task_id)
            SELECT 'acme.other', 'Other', 'second', 'Acme', 'r', 1, '0123456789abcdef', id FROM tasks WHERE title = 'mine';",
    );
}

fn count(w: &World, table: &str) -> i64 {
    w.sql(&format!("SELECT count(*) FROM {table}"))
}

#[test]
fn the_uninstall_preview_counts_what_leaves_and_what_stays() {
    let w = world();
    install_with_data(&w);
    let p = uninstall::preview(&w.ledger(), &id("atlas.map")).unwrap();
    assert_eq!(p.name, "Map");
    assert_eq!((p.leaves.content_bytes, p.leaves.cache_bytes), (10, 4));
    assert_eq!(p.leaves.document_data_bytes, 2);
    assert!(p.leaves.files_bytes > 0, "the version folder's files");
    assert_eq!(p.leaves.nav_tab.as_deref(), Some("Map"));
    // Its own gifts (two tasks, a milestone) and the documents under its prefix, not another mod's
    // and not a longer id that shares the prefix.
    assert_eq!(
        (p.stays.tasks, p.stays.milestones, p.stays.documents),
        (2, 1, 2)
    );
    assert!(!p.removed_by_safety_list);
    // The preview changed nothing.
    assert_eq!(w.rows(), 1);
    assert!(matches!(
        uninstall::preview(&w.ledger(), &id("acme.nothing"))
            .unwrap_err()
            .code,
        Code::NotInstalled
    ));
    w.exec("UPDATE mods SET state = 'paused', state_reason = 'blacklist-removed'");
    assert!(
        uninstall::preview(&w.ledger(), &id("atlas.map"))
            .unwrap()
            .removed_by_safety_list
    );
}

#[test]
fn an_uninstall_keeps_what_the_mod_made_unless_the_user_asks() {
    let w = world();
    install_with_data(&w);
    let done = uninstall::run(&w.ledger(), &id("atlas.map"), false).unwrap();
    assert!(done.files_removed && done.gifts_deleted.is_none());
    assert_eq!(done.prune.tab, "mod:atlas.map");
    assert_eq!(w.rows(), 0);
    assert!(w.installed_tree().is_empty());
    // The row's cascade took its storage; nothing the user made went.
    assert_eq!(count(&w, "mod_kv"), 0);
    assert_eq!(count(&w, "tasks"), 3);
    assert_eq!(count(&w, "project_milestones"), 1);
    assert_eq!(count(&w, "documents"), 4);
    assert_eq!(count(&w, "mod_ledger"), 4);
}

#[test]
fn deleting_what_the_mod_made_goes_through_the_delete_paths_and_only_its_own() {
    let w = world();
    install_with_data(&w);
    let before: String =
        w.sql("SELECT COALESCE(last_touched, '') FROM projects WHERE name = 'Atlas'");
    std::thread::sleep(std::time::Duration::from_millis(5));
    let done = uninstall::run(&w.ledger(), &id("atlas.map"), true).unwrap();
    let gone = done.gifts_deleted.unwrap();
    assert_eq!((gone.tasks, gone.milestones, gone.documents), (2, 1, 2));
    assert!(gone.problem.is_none());
    // Its tasks, its milestone and the two documents under its prefix are gone; the user's own task,
    // the other mod's gift, the lookalike id's document and the unrelated one stay.
    let titles: Vec<String> = {
        let c = w.store.conn.lock().unwrap();
        let mut stmt = c.prepare("SELECT title FROM tasks ORDER BY title").unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    assert_eq!(titles, ["mine"]);
    assert_eq!(count(&w, "project_milestones"), 0);
    let docs: Vec<String> = {
        let c = w.store.conn.lock().unwrap();
        let mut stmt = c
            .prepare("SELECT source_id FROM documents ORDER BY source_id")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    assert_eq!(docs, ["mod:atlas.mapx:n1", "note:other"]);
    assert_eq!(
        count(&w, "mod_ledger"),
        1,
        "only the other mod's gift row is left"
    );
    // The delete paths touched the project, as the delete commands do.
    let after: String =
        w.sql("SELECT COALESCE(last_touched, '') FROM projects WHERE name = 'Atlas'");
    assert!(after > before, "{after} should be after {before}");
}

#[test]
fn a_gift_that_cannot_be_deleted_is_reported_and_the_rest_still_go() {
    let w = world();
    install_with_data(&w);
    w.exec(
        "CREATE TRIGGER keep_one BEFORE DELETE ON tasks WHEN old.title = 'gift one' \
         BEGIN SELECT RAISE(ABORT, 'locked'); END",
    );
    let done = uninstall::run(&w.ledger(), &id("atlas.map"), true).unwrap();
    // The mod is gone whatever happened to its gifts.
    assert_eq!(w.rows(), 0);
    let gone = done.gifts_deleted.unwrap();
    assert_eq!((gone.tasks, gone.milestones, gone.documents), (1, 1, 2));
    assert!(gone.problem.is_some());
    assert_eq!(count(&w, "tasks"), 2, "the locked gift and the user's own");
}

#[test]
fn an_uninstall_of_a_mod_that_is_not_installed_deletes_nothing_even_when_asked() {
    let w = world();
    install_with_data(&w);
    let r = uninstall::run(&w.ledger(), &id("acme.nothing"), true);
    assert_eq!(r.unwrap_err().code, Code::NotInstalled);
    assert_eq!(count(&w, "tasks"), 3);
    assert_eq!(w.rows(), 1);
}

// ---- the icon -----------------------------------------------------------------------------------------------------------

fn real_png() -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
    b.extend_from_slice(b"pretend image data");
    b
}

#[test]
fn the_icon_is_read_through_the_row_only_and_only_when_it_is_a_real_png() {
    let w = world();
    // The fixture's own icon is "\x89PNG fake": not an image, so there is none to show.
    w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
        .unwrap();
    assert_eq!(
        icon::read(&w.ledger(), &id("atlas.map")).unwrap_err().code,
        Code::FileUnreadable
    );
    // A real PNG in the package is read, with its media type.
    let w2 = world();
    let (bytes, key) = crate::mods::package::tests::signed_package(|_, files| {
        files.insert("app/icon.png".into(), real_png());
    });
    w2.ledger()
        .install_package(
            &bytes,
            &TrustedKeys::of_test_keys(std::slice::from_ref(&key)),
            &opts(Approval::Approved),
            now(),
        )
        .unwrap();
    let got = icon::read(&w2.ledger(), &id("atlas.map")).unwrap();
    assert_eq!(got.media_type, "image/png");
    // The listing says there is an icon, and the flag and the command agree.
    let listed = {
        let c = w2.store.conn.lock().unwrap();
        listing::list(
            &c,
            &listing::Ctx {
                index: None,
                engine_sealed: true,
            },
        )
        .unwrap()
    };
    assert!(listed[0].has_icon);
    // Not installed: not found, not a file error.
    assert_eq!(
        icon::read(&w2.ledger(), &id("acme.nothing"))
            .unwrap_err()
            .code,
        Code::NotInstalled
    );
    // Changed on disk since: no icon, and nothing else touched.
    let path = w2.version_dir("1.0.0").join("app/icon.png");
    let mut changed = real_png();
    changed[10] ^= 1;
    fs::write(&path, changed).unwrap();
    assert_eq!(
        icon::read(&w2.ledger(), &id("atlas.map")).unwrap_err().code,
        Code::FileUnreadable
    );
    assert_eq!(state(&w2), "active:");
}

#[test]
fn a_manifest_that_names_an_svg_icon_gets_none() {
    let w = world();
    let (bytes, key) = crate::mods::package::tests::signed_package(|m, files| {
        m["icon"] = json!("app/icon.svg");
        files.insert(
            "app/icon.svg".into(),
            b"<svg xmlns='http://www.w3.org/2000/svg'/>".to_vec(),
        );
    });
    w.ledger()
        .install_package(
            &bytes,
            &TrustedKeys::of_test_keys(std::slice::from_ref(&key)),
            &opts(Approval::Approved),
            now(),
        )
        .unwrap();
    assert!(icon::read(&w.ledger(), &id("atlas.map")).is_err());
    let listed = {
        let c = w.store.conn.lock().unwrap();
        listing::list(
            &c,
            &listing::Ctx {
                index: None,
                engine_sealed: true,
            },
        )
        .unwrap()
    };
    assert!(!listed[0].has_icon);
}

#[test]
fn the_listing_over_a_real_install_tells_what_the_row_says() {
    let w = world();
    let f = fixture("1.0.0", |_| {});
    let (_, staged) = prepare(&w, &f, Source::File, None).unwrap();
    install::finish(
        &w.ledger(),
        staged,
        &answers(&[("documents.meta", true)]),
        &env(&f.keys, None),
    )
    .unwrap();
    w.ledger().pause_by_user(&id("atlas.map"), now()).unwrap();
    let c = w.store.conn.lock().unwrap();
    let listed = listing::list(
        &c,
        &listing::Ctx {
            index: None,
            engine_sealed: true,
        },
    )
    .unwrap();
    let map = &listed[0];
    assert_eq!(map.state.kind, crate::mods::tab::words::Kind::PausedByYou);
    assert_eq!(map.state.fix, crate::mods::tab::words::Fix::Resume);
    let access: BTreeMap<&str, &str> = map
        .permissions
        .iter()
        .map(|p| (p.id.as_str(), p.access))
        .collect();
    assert_eq!(access["documents.meta"], "allowed");
    assert_eq!(
        (map.installed_from.as_str(), map.signed, map.consented),
        ("file", true, true)
    );
}
