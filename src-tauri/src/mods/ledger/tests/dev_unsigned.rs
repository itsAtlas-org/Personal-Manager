// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The unsigned file install and developer mode as the ledger carries it out (slice 6, part 3),
//! over the ledger tests' own world: a real store, real files, a recording channel. What it pins:
//! an unsigned file installs only while developer mode is on for THIS device (read by Rust, again in
//! the commit); every refusal comes before anything is written; a reinstall is a fresh install
//! (new install id, grants and secrets reset, stored content kept); D3 is judged across the unsigned
//! set before consent; and the switch pauses and resumes exactly the unsigned mods, through every
//! load, whoever runs it.

use super::*;
use crate::mods::devmode;
use crate::mods::index::Index;
use crate::mods::outbox::testing::Recorder;
use crate::mods::outbox::{MOD_PAUSED, SESSION_CLOSED};
use crate::mods::package::tests::unsigned_package;
use crate::mods::safety::tests::{entry_for, index_listing, index_with, now as list_now};
use crate::mods::safety::SafetyCheck;
use crate::mods::tab::consent::Answers;
use crate::mods::tab::error::{Code, ModsError};
use crate::mods::tab::install::{self, ConsentScreen};
use crate::mods::tab::stage::{Stage, Staged, Trust};
use crate::mods::tab::testing::{insert, Row as TestRow};
use crate::mods::tab::unsigned::{self as stark, UnsignedEnv};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Instant;
use tauri::http::Request;

const DEVICE: &str = "dddddddddddddddddddddddddddddddd";
const OTHER_DEVICE: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

// ---- the world ----------------------------------------------------------------------------------

fn dev(w: &World, device: &str, on: bool) {
    let mut c = w.store.conn.lock().unwrap();
    devmode::write(&mut c, device, on).unwrap();
}

/// Developer mode on for this device, in the store and in the registry's copy.
fn dev_on(w: &World) {
    dev(w, DEVICE, true);
    w.runtime.set_unsigned_allowed(true);
}

fn file(change: impl FnOnce(&mut Value, &mut BTreeMap<String, Vec<u8>>)) -> Vec<u8> {
    unsigned_package(change)
}

fn plain() -> Vec<u8> {
    file(|_, _| {})
}

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn uopts(index: Option<&Index>) -> UnsignedOptions<'_> {
    UnsignedOptions {
        device: DEVICE,
        index,
    }
}

fn install_file(
    w: &World,
    bytes: &[u8],
    index: Option<&Index>,
) -> Result<UnsignedInstall, LedgerError> {
    let pkg = package::verify_unsigned(bytes).unwrap();
    w.ledger()
        .install_unsigned(&pkg, &sha(bytes), &uopts(index), now())
}

fn refusal(r: Result<UnsignedInstall, LedgerError>) -> Refusal {
    match r {
        Err(LedgerError::Refused(r)) => r,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn acme() -> ModId {
    id("acme.tool")
}

fn row_state(w: &World) -> String {
    w.sql("SELECT state || ':' || COALESCE(state_reason, '') FROM mods WHERE id = 'acme.tool'")
}

fn serving(w: &World) -> bool {
    w.runtime.is_installed(&acme())
}

fn install_id(w: &World) -> String {
    w.sql("SELECT install_id FROM mods WHERE id = 'acme.tool'")
}

fn tool_dir(w: &World, version: &str) -> PathBuf {
    w.layout
        .version_dir(&acme(), &VersionName::parse(version).unwrap())
}

fn nothing_written(w: &World) {
    assert_eq!(w.rows(), 0);
    assert!(w.installed_tree().is_empty() && w.staging_is_empty());
    assert!(!serving(w));
}

fn live_frame(w: &World, who: &str) -> Recorder {
    let rec = Recorder::default();
    let opened = w.runtime.open_frame_in(&id(who), 0, rec.outbox()).unwrap();
    let req = Request::builder()
        .uri(opened.entry_url.as_str())
        .body(Vec::new())
        .unwrap();
    assert_eq!(w.runtime.serve("main", &req).status(), 200);
    rec
}

fn serve_all() -> impl ServeCheck {
    |_: &str, _: &str, _: Option<&str>| None
}

fn grants_of(w: &World, who: &str) -> BTreeMap<String, String> {
    let c = w.store.conn.lock().unwrap();
    let mut stmt = c
        .prepare("SELECT permission, decision FROM mod_grants WHERE mod_id = ?1")
        .unwrap();
    stmt.query_map([who], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn answers(items: &[(&str, bool)]) -> Answers {
    items.iter().map(|(p, a)| ((*p).to_owned(), *a)).collect()
}

// ---- who may install, and what is refused before anything is written -----------------------------------

#[test]
fn an_unsigned_file_installs_only_while_developer_mode_is_on_for_this_device() {
    let w = world();
    // No record at all, then on for another device only, then off again: refused each time, with
    // nothing written, whatever the registry's own copy says.
    assert_eq!(
        refusal(install_file(&w, &plain(), None)),
        Refusal::DevModeOff
    );
    nothing_written(&w);
    dev(&w, OTHER_DEVICE, true);
    w.runtime.set_unsigned_allowed(true);
    assert_eq!(
        refusal(install_file(&w, &plain(), None)),
        Refusal::DevModeOff
    );
    nothing_written(&w);
    dev(&w, DEVICE, true);
    dev(&w, DEVICE, false);
    assert_eq!(
        refusal(install_file(&w, &plain(), None)),
        Refusal::DevModeOff
    );
    nothing_written(&w);

    // On for this device: a fresh install, signed = 0, third party, consented, running.
    dev_on(&w);
    let bytes = plain();
    let done = install_file(&w, &bytes, None).unwrap();
    assert_eq!(
        (
            done.installed.kind,
            done.installed.state.as_str(),
            done.reinstall
        ),
        (InstallKind::Fresh, "active", false)
    );
    assert_eq!(
        w.sql::<String>(
            "SELECT signed || tier || installed_from || author || source_repo FROM mods"
        ),
        "0thirdfileAcmeacme/tool"
    );
    assert_eq!(install_id(&w).len(), 16);
    assert_eq!(
        w.sql::<i64>("SELECT consented_at IS NOT NULL FROM mods"),
        1,
        "the user is looking at the stark screen"
    );
    assert_eq!(
        w.sql::<String>("SELECT package_sha256 FROM mods"),
        sha(&bytes)
    );
    assert!(serving(&w));
    assert!(tool_dir(&w, "1.0.0").join("app/index.html").is_file());
}

#[test]
fn developer_mode_is_read_again_in_the_commit_so_a_switch_off_while_staging_wins() {
    let w = world();
    dev_on(&w);
    // The installs's steps: developer mode (0), the row (1), then the commit (2). Another owner
    // switches developer mode off just before the commit.
    *w.store.before_step.lock().unwrap() = Some((
        2,
        "UPDATE settings SET value = '{\"v\":1,\"devices\":[]}' WHERE key = 'mods_dev_devices'"
            .into(),
    ));
    assert_eq!(
        refusal(install_file(&w, &plain(), None)),
        Refusal::DevModeOff
    );
    // Refused at the commit: its files were rolled back with it.
    nothing_written(&w);
}

#[test]
fn a_signed_package_never_installs_through_the_unsigned_path() {
    let w = world();
    dev_on(&w);
    let digest = sha(&plain());
    let (signed, key) = pkg("1.0.0", |_| {});
    let signed = package::verify(&signed, std::slice::from_ref(&key)).unwrap();
    let got = w
        .ledger()
        .install_unsigned(&signed, &digest, &uopts(None), now());
    assert!(matches!(
        got,
        Err(LedgerError::Package(PackageError {
            refusal: package::Refusal::Signed,
            ..
        }))
    ));
    nothing_written(&w);
}

#[test]
fn a_manifest_that_claims_a_signed_tier_is_refused_even_unsigned() {
    let w = world();
    dev_on(&w);
    let bytes = plain();
    // Second party is a tier an Atlas signature vouches for, so an unsigned file can't say it either
    // (the id grammar alone would let it: `acme.tool` may be second party).
    for tier in [
        crate::mods::ids::Tier::First,
        crate::mods::ids::Tier::Second,
    ] {
        let mut claims = package::verify_unsigned(&bytes).unwrap();
        claims.manifest.publisher.tier = tier;
        assert!(
            matches!(
                w.ledger()
                    .install_unsigned(&claims, &sha(&bytes), &uopts(None), now()),
                Err(LedgerError::Refused(Refusal::Tier(_)))
            ),
            "{tier:?}"
        );
        nothing_written(&w);
    }
}

#[test]
fn an_api_range_this_pm_does_not_serve_is_refused_before_anything_is_written() {
    let w = world();
    dev_on(&w);
    let too_new = file(|m, _| m["apiVersion"] = json!({"min": "9.0", "target": "9.0"}));
    assert_eq!(
        refusal(install_file(&w, &too_new, None)),
        Refusal::NeedsNewerPm { min: (9, 0) }
    );
    nothing_written(&w);
}

#[test]
fn a_catalogue_id_and_a_blacklisted_file_are_refused_before_anything_is_written() {
    let w = world();
    dev_on(&w);
    let bytes = plain();
    let digest = sha(&bytes);
    // An id the catalogue lists, and a file the blacklist names by its hash.
    let pkg = package::verify_unsigned(&bytes).unwrap();
    let mut entry = entry_for(&pkg, &digest);
    entry["id"] = json!("acme.tool");
    entry["tier"] = json!("second");
    let listed = index_listing(entry, json!([]));
    match refusal(install_file(&w, &bytes, Some(&listed))) {
        Refusal::Catalogue(why) => assert!(why.contains("catalogue"), "{why}"),
        other => panic!("{other:?}"),
    }
    let hashed = index_with(|v| {
        v["blacklist"] =
            json!([{"sha256": digest, "action": "removed", "reason": "A known bad file."}]);
    });
    match refusal(install_file(&w, &bytes, Some(&hashed))) {
        Refusal::Catalogue(why) => assert!(why.contains("A known bad file."), "{why}"),
        other => panic!("{other:?}"),
    }
    // The blacklist by id and version too.
    let by_id = index_with(|v| {
        v["blacklist"] = json!([{"id": "acme.tool", "versions": "=1.0.0", "action": "paused",
            "reason": "Loses notes.", "fixedIn": "1.0.1"}]);
    });
    assert!(matches!(
        refusal(install_file(&w, &bytes, Some(&by_id))),
        Refusal::Catalogue(_)
    ));
    nothing_written(&w);
}

#[test]
fn an_id_held_by_anything_but_an_unsigned_file_is_never_taken_over() {
    let held =
        |signed: bool, from: &'static str, reason: Option<&'static str>, state: &'static str| {
            let w = world();
            dev_on(&w);
            {
                let c = w.store.conn.lock().unwrap();
                insert(
                    &c,
                    &TestRow {
                        id: "acme.tool",
                        tier: if signed { "second" } else { "third" },
                        signed,
                        installed_from: from,
                        state,
                        reason,
                        ..TestRow::default()
                    },
                );
            }
            let before: String = w.sql("SELECT install_id || version FROM mods");
            let r = refusal(install_file(&w, &plain(), None));
            // Nothing moved: not the row, not a file.
            assert_eq!(
                w.sql::<String>("SELECT install_id || version FROM mods"),
                before
            );
            assert!(w.installed_tree().is_empty() && w.staging_is_empty());
            r
        };
    assert_eq!(
        held(true, "file", None, "active"),
        Refusal::OtherInstallHoldsId
    );
    assert_eq!(
        held(true, "catalogue", None, "active"),
        Refusal::OtherInstallHoldsId
    );
    assert_eq!(
        held(false, "folder", None, "active"),
        Refusal::OtherInstallHoldsId
    );
}

#[test]
fn a_row_the_safety_list_removed_waits_for_its_uninstall_not_for_a_file() {
    let w = world();
    dev_on(&w);
    {
        let c = w.store.conn.lock().unwrap();
        insert(
            &c,
            &TestRow {
                id: "acme.tool",
                tier: "third",
                signed: false,
                installed_from: "file",
                state: "paused",
                reason: Some("blacklist-removed"),
                ..TestRow::default()
            },
        );
    }
    assert_eq!(
        refusal(install_file(&w, &plain(), None)),
        Refusal::RemovedPending
    );
    assert!(w.installed_tree().is_empty() && w.staging_is_empty());
}

// ---- a reinstall is a fresh install ---------------------------------------------------------------------

fn seed_what_an_install_made(w: &World) {
    w.exec(
        "INSERT INTO mod_grants(mod_id, permission, decision) VALUES ('acme.tool','documents.meta','granted');
         INSERT INTO mod_secrets(mod_id, name, host, header, value) VALUES ('acme.tool','key','example.com','X-Key','s3cret');
         INSERT INTO mod_kv(mod_id, key, class, value) VALUES ('acme.tool','settings','content','{\"keep\":1}');
         INSERT INTO mod_kv(mod_id, key, class, value) VALUES ('acme.tool','layout','cache','{}');",
    );
}

/// An unsigned mod with a grant, a secret and stored data, and a live frame, then the same version
/// installed again as a different file. Returns the world, the first install id, the frame's
/// channel and the second file's bytes.
fn reinstalled() -> (World, String, Recorder, Vec<u8>) {
    let w = world();
    dev_on(&w);
    install_file(&w, &plain(), None).unwrap();
    seed_what_an_install_made(&w);
    let first_id = install_id(&w);
    let rec = live_frame(&w, "acme.tool");
    // The same version, a different file: the folder is replaced, the old frame told to reload.
    let changed = file(|_, f| {
        f.insert("app/main.js".into(), b"console.log('v2');".to_vec());
    });
    let done = install_file(&w, &changed, None).unwrap();
    assert!(done.reinstall);
    (w, first_id, rec, changed)
}

#[test]
fn a_reinstall_mints_a_new_install_id() {
    let (w, first_id, _, _) = reinstalled();
    assert_ne!(
        install_id(&w),
        first_id,
        "a fresh install id, so the old gifts aren't its own"
    );
    assert_eq!(install_id(&w).len(), 16);
}

#[test]
fn a_reinstall_resets_the_grants() {
    let (w, ..) = reinstalled();
    assert!(grants_of(&w, "acme.tool").is_empty(), "grants reset");
}

#[test]
fn a_reinstall_resets_the_secrets() {
    let (w, ..) = reinstalled();
    assert_eq!(
        w.sql::<i64>("SELECT count(*) FROM mod_secrets"),
        0,
        "secrets reset"
    );
}

#[test]
fn a_reinstall_keeps_what_the_mod_stored() {
    let (w, ..) = reinstalled();
    assert_eq!(
        w.sql::<String>("SELECT value FROM mod_kv WHERE key = 'settings'"),
        "{\"keep\":1}",
        "stored content is the user's, and stays"
    );
    assert_eq!(w.sql::<i64>("SELECT count(*) FROM mod_kv"), 2);
}

#[test]
fn a_reinstall_serves_the_new_files_and_tells_the_old_frame_to_reload() {
    let (w, _, rec, changed) = reinstalled();
    assert_eq!(
        w.sql::<String>("SELECT signed || package_sha256 FROM mods"),
        format!("0{}", sha(&changed))
    );
    assert_eq!(
        fs::read(tool_dir(&w, "1.0.0").join("app/main.js")).unwrap(),
        b"console.log('v2');"
    );
    assert!(serving(&w));
    assert_eq!(rec.names().last().copied(), Some(SESSION_CLOSED));
    assert_eq!(rec.payloads(SESSION_CLOSED)[0]["reason"], json!("replaced"));
    assert_eq!(row_state(&w), "active:");
}

#[test]
fn a_reinstall_at_another_version_prunes_the_old_folder_and_keeps_anothers_pause() {
    let w = world();
    dev_on(&w);
    install_file(&w, &plain(), None).unwrap();
    w.exec("UPDATE mods SET state = 'paused', state_reason = 'user'");
    let v2 = file(|m, _| m["version"] = json!("1.1.0"));
    let done = install_file(&w, &v2, None).unwrap();
    assert_eq!(done.installed.pruned, ["1.0.0"]);
    assert!(!tool_dir(&w, "1.0.0").exists() && tool_dir(&w, "1.1.0").is_dir());
    // The user's own pause survives: nothing resumes by itself.
    assert_eq!(row_state(&w), "paused:user");
    assert!(!serving(&w));
    // A downgrade is allowed: a developer goes back to the version they were on.
    let v1 = file(|_, _| {});
    assert!(install_file(&w, &v1, None).unwrap().reinstall);
    assert_eq!(w.sql::<String>("SELECT version FROM mods"), "1.0.0");
}

#[test]
fn a_reinstall_whose_row_changes_before_the_commit_is_refused_and_the_old_files_come_back() {
    let w = world();
    dev_on(&w);
    let first = plain();
    install_file(&w, &first, None).unwrap();
    let old = fs::read(tool_dir(&w, "1.0.0").join("app/main.js")).unwrap();
    *w.store.before_step.lock().unwrap() = Some((
        w.store.steps.load(Ordering::SeqCst) + 2,
        "UPDATE mods SET author = 'Someone Else'".into(),
    ));
    let changed = file(|_, f| {
        f.insert("app/main.js".into(), b"console.log('v2');".to_vec());
    });
    assert_eq!(refusal(install_file(&w, &changed, None)), Refusal::Changed);
    // The folder the row names is the old one again.
    assert_eq!(
        fs::read(tool_dir(&w, "1.0.0").join("app/main.js")).unwrap(),
        old
    );
    assert_eq!(
        w.sql::<String>("SELECT package_sha256 FROM mods"),
        sha(&first)
    );
    assert!(w.staging_is_empty());
}

#[test]
fn a_reinstall_over_a_row_dev_mode_paused_is_active_again() {
    let w = world();
    dev_on(&w);
    install_file(&w, &plain(), None).unwrap();
    w.exec("UPDATE mods SET state = 'paused', state_reason = 'dev-mode'");
    install_file(&w, &plain(), None).unwrap();
    assert_eq!(row_state(&w), "active:");
}

// ---- the stark screen: D3 before consent, the typed id, the answers --------------------------------------

fn senv(sealed: bool) -> UnsignedEnv<'static> {
    UnsignedEnv {
        now: list_now(),
        index: None,
        engine_sealed: sealed,
        device: DEVICE,
    }
}

fn prepare(w: &World, bytes: Vec<u8>, sealed: bool) -> Result<(ConsentScreen, Staged), ModsError> {
    stark::prepare(&w.ledger(), bytes, &senv(sealed))
}

fn finish(
    w: &World,
    bytes: Vec<u8>,
    answers: &Answers,
    typed: Option<&str>,
    sealed: bool,
) -> Result<install::Finished, ModsError> {
    let (_, staged) = prepare(w, bytes, sealed).unwrap();
    stark::finish(&w.ledger(), staged, answers, typed, &senv(sealed))
}

/// A file that asks to read documents and to reach the internet, as acme.both.
fn both() -> Vec<u8> {
    file(|m, _| {
        m["id"] = json!("acme.both");
        m["contributions"] = json!({});
        m["permissions"] = json!([
            {"id": "storage", "why": "Remembers things."},
            {"id": "documents.meta", "why": "Shows your documents' titles."},
            {"id": "internet", "why": "Fetches rates."}
        ]);
        m["internetHosts"] = json!(["rates.example.com"]);
    })
}

fn access(screen: &ConsentScreen, perm: &str) -> (&'static str, bool, bool) {
    let p = screen
        .permissions
        .iter()
        .find(|p| p.permission.id == perm)
        .unwrap_or_else(|| panic!("{perm} isn't on the screen"));
    (p.permission.access, p.answer_required, p.default_answer)
}

#[test]
fn the_stark_screen_installs_nothing_and_asks_one_by_one() {
    let w = world();
    // Developer mode off: no screen at all.
    assert_eq!(
        prepare(&w, plain(), true).unwrap_err().code,
        Code::DevModeOff
    );
    dev_on(&w);
    let (screen, staged) = prepare(&w, plain(), true).unwrap();
    nothing_written(&w);
    assert_eq!(
        (
            screen.id.as_str(),
            screen.tier.as_str(),
            screen.kind,
            screen.from
        ),
        ("acme.tool", "third", "fresh", "file")
    );
    let u = screen.unsigned.as_ref().unwrap();
    assert_eq!(u.confirm_id, "acme.tool");
    assert!(!u.reinstall && u.engine_sealed && u.read_or_internet.is_none());
    assert_eq!(staged.trust, Trust::Unsigned);
    assert_eq!(staged.sha256, sha(&plain()));
    // storage is automatic; documents.meta is asked one by one and starts at no.
    assert_eq!(access(&screen, "storage"), ("allowed", false, false));
    assert_eq!(access(&screen, "documents.meta"), ("asks", true, false));
}

#[test]
fn a_file_that_asks_for_both_sides_gets_the_read_or_internet_choice() {
    let w = world();
    dev_on(&w);
    let (screen, _) = prepare(&w, both(), true).unwrap();
    let choice = screen.unsigned.unwrap().read_or_internet.unwrap();
    assert_eq!(choice.read, ["documents.meta"]);
    assert_eq!(choice.internet, ["internet"]);
    assert_eq!(choice.held_by_others, None);
}

#[test]
fn only_the_typed_id_installs_and_nothing_is_written_for_any_other() {
    let w = world();
    dev_on(&w);
    let a = answers(&[("documents.meta", false)]);
    for typed in [
        None,
        Some(""),
        Some("acme.too"),
        Some(" acme.tool"),
        Some("acme.tool "),
        Some("ACME.TOOL"),
    ] {
        let e = finish(&w, plain(), &a, typed, true).unwrap_err();
        assert_eq!(e.code, Code::ConfirmMismatch, "{typed:?}");
        nothing_written(&w);
    }
    let done = finish(&w, plain(), &a, Some("acme.tool"), true).unwrap();
    assert!(done.result.grants_saved);
    assert!(serving(&w));
}

#[test]
fn the_answers_must_fit_the_manifest_or_nothing_installs() {
    let w = world();
    dev_on(&w);
    // No answer for the one asked one by one.
    let e = finish(&w, plain(), &answers(&[]), Some("acme.tool"), true).unwrap_err();
    assert_eq!(e.code, Code::BadRequest);
    // An answer for what the file didn't declare, and for what PM doesn't have.
    for (p, code) in [
        ("internet", Code::BadRequest),
        ("made.up", Code::BadRequest),
    ] {
        let e = finish(
            &w,
            plain(),
            &answers(&[("documents.meta", true), (p, true)]),
            Some("acme.tool"),
            true,
        )
        .unwrap_err();
        assert_eq!(e.code, code, "{p}");
    }
    nothing_written(&w);
    // A yes is a granted row, a no a denied one.
    finish(
        &w,
        plain(),
        &answers(&[("documents.meta", true)]),
        Some("acme.tool"),
        true,
    )
    .unwrap();
    assert_eq!(
        grants_of(&w, "acme.tool"),
        BTreeMap::from([("documents.meta".into(), "granted".into())])
    );
}

#[test]
fn a_no_is_recorded_and_a_first_use_yes_is_left_for_its_prompt() {
    let w = world();
    dev_on(&w);
    let bytes = file(|m, _| {
        m["contributions"] = json!({});
        m["permissions"] = json!([
            {"id": "storage", "why": "x"},
            {"id": "documents.meta", "why": "y"},
            {"id": "ai", "why": "z"}
        ]);
    });
    finish(
        &w,
        bytes,
        &answers(&[("documents.meta", false), ("ai", true)]),
        Some("acme.tool"),
        true,
    )
    .unwrap();
    assert_eq!(
        grants_of(&w, "acme.tool"),
        BTreeMap::from([("documents.meta".into(), "denied".into())]),
        "ai is first use: a yes waits for its prompt"
    );
}

#[test]
fn read_and_internet_together_are_refused_before_anything_is_installed() {
    let w = world();
    dev_on(&w);
    let both_yes = answers(&[("documents.meta", true), ("internet", true)]);
    let e = finish(&w, both(), &both_yes, Some("acme.both"), true).unwrap_err();
    assert_eq!(e.code, Code::GrantRefused);
    nothing_written(&w);
    // One side is the user's choice.
    for (read, internet) in [(true, false), (false, true), (false, false)] {
        let w = world();
        dev_on(&w);
        let a = answers(&[("documents.meta", read), ("internet", internet)]);
        finish(&w, both(), &a, Some("acme.both"), true).unwrap();
        let granted: Vec<String> = grants_of(&w, "acme.both")
            .into_iter()
            .filter(|(_, d)| d == "granted")
            .map(|(p, _)| p)
            .collect();
        assert_eq!(granted.len(), usize::from(read) + usize::from(internet));
    }
}

fn install_other_holding(w: &World, who: &str, permission: &str) {
    let bytes = file(|m, _| {
        m["id"] = json!(who);
        m["contributions"] = json!({});
        m["permissions"] = json!([{"id": "storage", "why": "x"}, {"id": permission, "why": "y"}]);
        if permission == "internet" {
            m["internetHosts"] = json!(["rates.example.com"]);
        }
    });
    let a = answers(&[(permission, true)]);
    let (_, staged) = prepare(w, bytes, true).unwrap();
    stark::finish(&w.ledger(), staged, &a, Some(who), &senv(true)).unwrap();
    assert_eq!(
        grants_of(w, who),
        BTreeMap::from([(permission.to_owned(), "granted".to_owned())])
    );
}

#[test]
fn d3_is_judged_across_the_unsigned_mods_before_the_screen_and_before_the_install() {
    let w = world();
    dev_on(&w);
    install_other_holding(&w, "acme.net", "internet");
    // The screen already says reading is off, and which side the others hold.
    let (screen, _) = prepare(&w, plain(), true).unwrap();
    assert_eq!(access(&screen, "documents.meta").0, "off");
    let why = &screen
        .permissions
        .iter()
        .find(|p| p.permission.id == "documents.meta")
        .unwrap()
        .permission
        .off_because;
    assert!(why.as_deref().unwrap().contains("never both"), "{why:?}");
    // A file asking for both: the choice shows who holds what.
    let (both_screen, _) = prepare(&w, both(), true).unwrap();
    assert_eq!(
        both_screen
            .unsigned
            .unwrap()
            .read_or_internet
            .unwrap()
            .held_by_others,
        Some("internet")
    );
    // A yes to reading is refused, whatever the page sends, and nothing for acme.tool is written.
    let e = finish(
        &w,
        plain(),
        &answers(&[("documents.meta", true)]),
        Some("acme.tool"),
        true,
    )
    .unwrap_err();
    assert_eq!(e.code, Code::GrantRefused);
    assert_eq!(
        w.sql::<i64>("SELECT count(*) FROM mods WHERE id = 'acme.tool'"),
        0
    );
    // A no is always fine.
    finish(
        &w,
        plain(),
        &answers(&[("documents.meta", false)]),
        Some("acme.tool"),
        true,
    )
    .unwrap();
}

#[test]
fn a_reinstall_is_judged_without_its_own_old_grants_and_tells_the_other_unsigned_mods() {
    let w = world();
    dev_on(&w);
    install_other_holding(&w, "acme.net", "internet");
    // acme.tool reads? Refused while acme.net holds the internet. So make acme.tool the holder of
    // the internet instead, then reinstall it asking to read once acme.net is gone: its OWN old
    // grant must not stand in its way.
    w.ledger().uninstall(&id("acme.net")).unwrap();
    install_other_holding(&w, "acme.tool", "documents.meta");
    // acme.tool now holds reading; the same id again, asking for the internet instead, is a fresh
    // install judged against the OTHERS (none), so it may.
    let bytes = file(|m, _| {
        m["contributions"] = json!({});
        m["permissions"] = json!([{"id": "storage", "why": "x"}, {"id": "internet", "why": "y"}]);
        m["internetHosts"] = json!(["rates.example.com"]);
    });
    let done = finish(
        &w,
        bytes,
        &answers(&[("internet", true)]),
        Some("acme.tool"),
        true,
    )
    .unwrap();
    assert!(done.result.grants_saved, "{:?}", done.result.grants_problem);
    assert_eq!(
        grants_of(&w, "acme.tool"),
        BTreeMap::from([("internet".into(), "granted".into())])
    );
    // A reinstall resets grants, which can free the other side for every other unsigned mod: all told.
    install_file(&w, &file(|m, _| m["id"] = json!("acme.other")), None).unwrap();
    let again = finish(
        &w,
        plain(),
        &answers(&[("documents.meta", false)]),
        Some("acme.tool"),
        true,
    )
    .unwrap();
    let mut told: Vec<&str> = again.told.iter().map(ModId::as_str).collect();
    told.sort_unstable();
    assert_eq!(told, ["acme.other", "acme.tool"]);
}

#[test]
fn an_unsealed_engine_gives_an_unsigned_mod_no_read() {
    let w = world();
    dev_on(&w);
    let (screen, _) = prepare(&w, plain(), false).unwrap();
    assert_eq!(access(&screen, "documents.meta").0, "off");
    assert!(!screen.unsigned.unwrap().engine_sealed);
    let e = finish(
        &w,
        plain(),
        &answers(&[("documents.meta", true)]),
        Some("acme.tool"),
        false,
    )
    .unwrap_err();
    assert_eq!(e.code, Code::GrantRefused);
    nothing_written(&w);
}

#[test]
fn a_token_made_for_one_kind_of_file_never_finishes_as_the_other() {
    let w = world();
    dev_on(&w);
    // Through the stage, as the command does.
    let stage = Stage::default();
    let t = Instant::now();
    let (_, unsigned_staged) = prepare(&w, plain(), true).unwrap();
    let token = stage.put(unsigned_staged, t).unwrap();
    let back = stage.take(&token, t).unwrap();
    assert_eq!(back.trust, Trust::Unsigned);
    // The signed finish refuses it.
    let (_, key) = pkg("1.0.0", |_| {});
    let keys = TrustedKeys::of_test_keys(std::slice::from_ref(&key));
    let env = install::Env {
        now: list_now(),
        keys: &keys,
        index: None,
        engine_sealed: true,
    };
    let e = install::finish(&w.ledger(), back, &answers(&[]), &env).unwrap_err();
    assert_eq!(e.code, Code::ConsentExpired);
    nothing_written(&w);
    // And a signed screen's staged bytes never finish as unsigned.
    let (signed_bytes, key) = pkg("1.0.0", |_| {});
    let keys = TrustedKeys::of_test_keys(std::slice::from_ref(&key));
    let (_, mut staged) = install::prepare(
        &w.ledger(),
        signed_bytes,
        Source::File,
        &install::Env {
            now: list_now(),
            keys: &keys,
            index: None,
            engine_sealed: true,
        },
    )
    .unwrap();
    assert_eq!(staged.trust, Trust::Signed);
    let e = stark::finish(
        &w.ledger(),
        staged.clone(),
        &answers(&[]),
        Some("atlas.map"),
        &senv(true),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::ConsentExpired);
    // Even relabelled, the unsigned verifier refuses a signed archive's bytes.
    staged.trust = Trust::Unsigned;
    let e = stark::finish(
        &w.ledger(),
        staged,
        &answers(&[]),
        Some("atlas.map"),
        &senv(true),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::NotAMod);
    assert_eq!(w.rows(), 0);
}

#[test]
fn the_signed_install_refuses_an_unsigned_package_whatever_calls_it() {
    let w = world();
    dev_on(&w);
    let pkg = package::verify_unsigned(&plain()).unwrap();
    let err = w
        .ledger()
        .install_verified(&pkg, &sha(&plain()), &opts(Approval::Approved), now())
        .unwrap_err();
    assert!(matches!(
        err,
        LedgerError::Package(PackageError {
            refusal: package::Refusal::Unsigned,
            ..
        })
    ));
    nothing_written(&w);
}

// ---- developer mode's switch: pause exactly the unsigned, resume exactly those ---------------------------

fn install_both_kinds(w: &World) -> (Recorder, Recorder) {
    w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
        .unwrap();
    install_file(w, &plain(), None).unwrap();
    (live_frame(w, "atlas.map"), live_frame(w, "acme.tool"))
}

fn state_of(w: &World, who: &str) -> String {
    w.sql(&format!(
        "SELECT state || ':' || COALESCE(state_reason, '') FROM mods WHERE id = '{who}'"
    ))
}

#[test]
fn switching_off_pauses_every_unsigned_mod_and_only_those_and_ends_their_sessions() {
    let w = world();
    dev_on(&w);
    let (atlas, tool) = install_both_kinds(&w);
    let done = w
        .ledger()
        .set_dev_mode_for_device(DEVICE, false, &serve_all(), now())
        .unwrap();
    assert_eq!(
        done,
        DevSwitch {
            on: false,
            paused: vec!["acme.tool".into()],
            resumed: vec![]
        }
    );
    assert_eq!(state_of(&w, "acme.tool"), "paused:dev-mode");
    assert_eq!(state_of(&w, "atlas.map"), "active:");
    assert!(!serving(&w) && w.runtime.is_installed(&id("atlas.map")));
    assert_eq!(tool.names(), [MOD_PAUSED, SESSION_CLOSED]);
    assert!(
        atlas.names().is_empty(),
        "a signed mod is not told anything"
    );
    assert!(!w.runtime.unsigned_allowed());
    let c = w.store.conn.lock().unwrap();
    assert!(!devmode::is_on(&c, DEVICE).unwrap());
}

#[test]
fn switching_back_on_resumes_exactly_what_it_paused() {
    let w = world();
    dev_on(&w);
    install_both_kinds(&w);
    // A second unsigned mod the user paused themselves, before developer mode went off.
    install_file(&w, &file(|m, _| m["id"] = json!("acme.mine")), None).unwrap();
    w.ledger().pause_by_user(&id("acme.mine"), now()).unwrap();
    w.ledger()
        .set_dev_mode_for_device(DEVICE, false, &serve_all(), now())
        .unwrap();
    assert_eq!(
        state_of(&w, "acme.mine"),
        "paused:user",
        "someone else's stop is theirs"
    );
    let done = w
        .ledger()
        .set_dev_mode_for_device(DEVICE, true, &serve_all(), now())
        .unwrap();
    assert_eq!(done.resumed, ["acme.tool"]);
    assert!(done.paused.is_empty());
    assert_eq!(state_of(&w, "acme.tool"), "active:");
    assert!(serving(&w) && w.runtime.unsigned_allowed());
    assert_eq!(state_of(&w, "acme.mine"), "paused:user");
    assert!(!w.runtime.is_installed(&id("acme.mine")));
}

#[test]
fn a_resume_asks_the_safety_list_and_hands_a_listed_mod_to_it() {
    let w = world();
    dev_on(&w);
    install_file(&w, &plain(), None).unwrap();
    w.ledger()
        .set_dev_mode_for_device(DEVICE, false, &serve_all(), now())
        .unwrap();
    let listed = index_with(|v| {
        v["blacklist"] = json!([{"id": "acme.tool", "versions": "=1.0.0", "action": "paused",
            "reason": "Loses notes.", "fixedIn": "1.0.1"}]);
    });
    let done = w
        .ledger()
        .set_dev_mode_for_device(DEVICE, true, &SafetyCheck::serve(Some(&listed)), now())
        .unwrap();
    assert!(done.resumed.is_empty(), "not resumed past a standing entry");
    assert_eq!(state_of(&w, "acme.tool"), "paused:blacklist");
    assert!(!serving(&w));
}

#[test]
fn a_switch_that_cannot_write_changes_nothing() {
    let w = world();
    dev_on(&w);
    install_file(&w, &plain(), None).unwrap();
    w.store.closed.store(true, Ordering::SeqCst);
    let e = w
        .ledger()
        .set_dev_mode_for_device(DEVICE, false, &serve_all(), now())
        .unwrap_err();
    assert!(matches!(e, LedgerError::Store(_)));
    w.store.closed.store(false, Ordering::SeqCst);
    assert!(
        w.runtime.unsigned_allowed(),
        "the registry's copy didn't move"
    );
    assert!(serving(&w));
    assert_eq!(state_of(&w, "acme.tool"), "active:");
}

#[test]
fn switching_for_this_device_leaves_another_devices_switch_alone() {
    let w = world();
    dev(&w, OTHER_DEVICE, true);
    dev_on(&w);
    w.ledger()
        .set_dev_mode_for_device(DEVICE, false, &serve_all(), now())
        .unwrap();
    let c = w.store.conn.lock().unwrap();
    assert!(devmode::is_on(&c, OTHER_DEVICE).unwrap());
    assert!(!devmode::is_on(&c, DEVICE).unwrap());
}

// ---- every load follows developer mode ------------------------------------------------------------------

#[test]
fn a_load_with_developer_mode_off_pauses_an_unsigned_row_it_would_have_served() {
    let w = world();
    dev_on(&w);
    install_file(&w, &plain(), None).unwrap();
    // A restart: the registry's copy is false until the writer's open has looked developer mode up,
    // and a row it finds active is paused, not served.
    w.runtime.set_unsigned_allowed(false);
    let report = w.ledger().startup(now(), &serve_all());
    assert_eq!(report.dev_paused, ["acme.tool"]);
    assert!(report.loaded.is_empty());
    assert_eq!(state_of(&w, "acme.tool"), "paused:dev-mode");
    assert!(!serving(&w));
    // The next start finds developer mode on for this device: resumed and served.
    w.runtime.set_unsigned_allowed(true);
    let report = w.ledger().startup(now(), &serve_all());
    assert_eq!(report.dev_resumed, ["acme.tool"]);
    assert_eq!(report.loaded, ["acme.tool"]);
    assert_eq!(state_of(&w, "acme.tool"), "active:");
    assert!(serving(&w));
}

#[test]
fn a_park_that_lifts_while_developer_mode_is_off_ends_paused_not_active() {
    let w = world();
    dev_on(&w);
    install_file(&w, &plain(), None).unwrap();
    w.ledger()
        .set_dev_mode_for_device(DEVICE, false, &serve_all(), now())
        .unwrap();
    // Parked for missing files while it was off (the row is in the ledger's own park).
    w.exec("UPDATE mods SET state = 'parked', state_reason = 'files-missing'");
    let aside = w.dir.path().join("aside");
    fs::rename(tool_dir(&w, "1.0.0"), &aside).unwrap();
    w.ledger().startup(now(), &serve_all());
    assert_eq!(state_of(&w, "acme.tool"), "parked:files-missing");
    // The files come back: the ledger lifts its park, and developer mode still holds the mod.
    fs::rename(&aside, tool_dir(&w, "1.0.0")).unwrap();
    w.ledger().startup(now(), &serve_all());
    assert_eq!(state_of(&w, "acme.tool"), "paused:dev-mode");
    assert!(!serving(&w));
}

#[test]
fn a_user_resume_of_an_unsigned_mod_does_not_serve_it_while_developer_mode_is_off() {
    let w = world();
    dev_on(&w);
    install_file(&w, &plain(), None).unwrap();
    w.ledger().pause_by_user(&acme(), now()).unwrap();
    w.ledger()
        .set_dev_mode_for_device(DEVICE, false, &serve_all(), now())
        .unwrap();
    let stood = w
        .ledger()
        .resume_by_user(&acme(), &serve_all(), now())
        .unwrap();
    assert!(!stood.serving);
    assert_eq!(stood.state, "paused");
    assert_eq!(stood.reason.as_deref(), Some("dev-mode"));
}

#[test]
fn a_restore_that_turns_every_device_off_pauses_the_unsigned_mods_at_the_next_open() {
    let w = world();
    dev_on(&w);
    dev(&w, OTHER_DEVICE, true);
    install_file(&w, &plain(), None).unwrap();
    // The restore reset writes the record on its connection...
    {
        let c = w.store.conn.lock().unwrap();
        devmode::all_devices_off(&c).unwrap();
        assert!(!devmode::is_on(&c, DEVICE).unwrap());
        assert!(!devmode::is_on(&c, OTHER_DEVICE).unwrap());
    }
    // ...and the open that follows reads it before the load.
    let on = {
        let c = w.store.conn.lock().unwrap();
        devmode::is_on(&c, DEVICE).unwrap()
    };
    w.runtime.set_unsigned_allowed(on);
    w.ledger().startup(now(), &serve_all());
    assert_eq!(state_of(&w, "acme.tool"), "paused:dev-mode");
    assert!(!serving(&w));
}

#[test]
fn a_closed_store_serves_no_unsigned_mod_until_developer_mode_is_looked_up_again() {
    let w = world();
    dev_on(&w);
    install_file(&w, &plain(), None).unwrap();
    assert!(w.runtime.unsigned_allowed());
    w.runtime.clear();
    assert!(!w.runtime.unsigned_allowed());
    // A load with the flag at its default pauses, never serves.
    w.ledger().startup(now(), &serve_all());
    assert!(!serving(&w));
}

#[test]
fn an_install_the_registry_does_not_yet_know_developer_mode_for_is_recorded_but_not_served() {
    // The store says on (the commit read it) but the registry's own copy is still off: the row is
    // written, and the mod waits to be served by the load that follows the lookup.
    let w = world();
    dev(&w, DEVICE, true);
    assert!(!w.runtime.unsigned_allowed());
    let done = install_file(&w, &plain(), None).unwrap();
    assert_eq!(done.installed.state, "active");
    assert!(!serving(&w));
}

#[test]
fn a_mod_developer_mode_paused_stays_paused_while_its_files_are_missing() {
    let w = world();
    dev_on(&w);
    install_file(&w, &plain(), None).unwrap();
    w.ledger()
        .set_dev_mode_for_device(DEVICE, false, &serve_all(), now())
        .unwrap();
    let aside = w.dir.path().join("aside");
    fs::rename(tool_dir(&w, "1.0.0"), &aside).unwrap();
    // Developer mode comes back on, but the files don't check out: the row is not set active, and
    // not served; it says what stopped it until it can run.
    let done = w
        .ledger()
        .set_dev_mode_for_device(DEVICE, true, &serve_all(), now())
        .unwrap();
    assert!(done.resumed.is_empty());
    assert_eq!(state_of(&w, "acme.tool"), "paused:dev-mode");
    assert!(!serving(&w));
    // With the files back, the next load resumes it.
    fs::rename(&aside, tool_dir(&w, "1.0.0")).unwrap();
    w.ledger().startup(now(), &serve_all());
    assert_eq!(state_of(&w, "acme.tool"), "active:");
    assert!(serving(&w));
}
