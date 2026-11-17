// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The folder test-mod loader as the ledger carries it out (slice 6, part 4), over the ledger tests'
//! own world: a real store, real files, a recording channel, and a developer's folder in a temp
//! directory. What it pins: a folder loads only while developer mode is on for THIS device (read by
//! Rust, again in the commit); every refusal comes before anything is written; nothing is ever
//! copied into PM's mods folder or written to the developer's folder; the row is a session's (a
//! startup deletes any, no load serves one without its session, a store's close forgets them,
//! developer mode off and unload delete them, never a file); a reload replaces only what it may
//! and says why when it can't; and no package ever installs over a folder.

use super::*;
use crate::mods::devmode;
use crate::mods::folder::tests::{edit_manifest, tool_folder, write_files};
use crate::mods::folder::{self, Scan};
use crate::mods::index::Index;
use crate::mods::outbox::testing::Recorder;
use crate::mods::outbox::SESSION_CLOSED;
use crate::mods::package::tests::unsigned_package;
use crate::mods::safety::tests::{entry_for, index_listing, index_with};
use crate::mods::safety::SafetyCheck;
use crate::mods::tab::consent::Answers;
use crate::mods::tab::error::Code;
use crate::mods::tab::folder as folder_tab;
use crate::mods::tab::testing::{insert, Row as TestRow};
use crate::mods::tab::unsigned::UnsignedEnv;
use serde_json::json;
use std::collections::BTreeMap;
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

fn uopts(index: Option<&Index>) -> UnsignedOptions<'_> {
    UnsignedOptions {
        device: DEVICE,
        index,
    }
}

fn acme() -> ModId {
    id("acme.tool")
}

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The fixture third-party mod as the developer's folder.
fn tool_dir() -> tempfile::TempDir {
    tool_dir_with(|_, _| {})
}

fn tool_dir_with(
    change: impl FnOnce(&mut Value, &mut BTreeMap<String, Vec<u8>>),
) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    tool_folder(dir.path(), change);
    dir
}

fn scan(dir: &Path) -> Scan {
    folder::scan(dir, true).unwrap()
}

fn load(w: &World, dir: &Path, index: Option<&Index>) -> Result<FolderLoad, LedgerError> {
    w.ledger().load_folder(&scan(dir), &uopts(index), now())
}

fn refusal<T: std::fmt::Debug>(r: Result<T, LedgerError>) -> Refusal {
    match r {
        Err(LedgerError::Refused(r)) => r,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn serving(w: &World) -> bool {
    w.runtime.is_installed(&acme())
}

fn loaded(w: &World) -> bool {
    w.runtime.folders().contains(&acme())
}

fn row_state(w: &World) -> String {
    w.sql("SELECT state || ':' || COALESCE(state_reason, '') FROM mods WHERE id = 'acme.tool'")
}

fn install_id(w: &World) -> String {
    w.sql("SELECT install_id FROM mods WHERE id = 'acme.tool'")
}

fn nothing_written(w: &World) {
    assert_eq!(w.rows(), 0);
    assert!(w.installed_tree().is_empty() && w.staging_is_empty());
    assert!(!serving(w) && !loaded(w));
}

/// Every file under `dir` and its bytes: what the developer's folder holds.
fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            if e.file_type().unwrap().is_dir() {
                walk(root, &p, out);
            } else {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
                out.insert(rel, fs::read(&p).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// What the scheme answers for `path` of `who`, as a subresource request from the main window.
fn serve_path(w: &World, who: &str, path: &str) -> u16 {
    let uri = format!("{}/{who}/{path}", Platform::Linux.mod_origin());
    let req = Request::builder().uri(uri).body(Vec::new()).unwrap();
    w.runtime.serve("main", &req).status().as_u16()
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

/// A grant, a stored value, a secret and a hidden-overlay entry for `acme.tool`, so a cascade or a
/// prune has something to take.
fn give_it_things(w: &World) {
    w.exec(
        "INSERT INTO mod_grants(mod_id, permission, decision) VALUES ('acme.tool','storage','granted');
         INSERT INTO mod_kv(mod_id, key, class, value) VALUES ('acme.tool','notes','content','\"hi\"');
         INSERT INTO mod_secrets(mod_id, name, host, header, value)
             VALUES ('acme.tool','token','api.example.com','Authorization','s3cret');
         INSERT INTO settings(key, value) VALUES
             ('calendar_ui', '{\"hidden\":[\"mod:acme.tool\",\"google:me\"]}')
             ON CONFLICT(key) DO UPDATE SET value = excluded.value;",
    );
}

fn things_left(w: &World) -> i64 {
    w.sql(
        "SELECT (SELECT count(*) FROM mod_grants) + (SELECT count(*) FROM mod_kv) \
              + (SELECT count(*) FROM mod_secrets)",
    )
}

fn hidden(w: &World) -> String {
    w.sql("SELECT value FROM settings WHERE key = 'calendar_ui'")
}

// ---- who may load, and what is refused before anything is written ----------------------------------

#[test]
fn a_folder_loads_only_while_developer_mode_is_on_for_this_device() {
    let w = world();
    let dir = tool_dir();
    // No record, on for another device only, then off again: refused each time, with nothing
    // written, whatever the registry's own copy says.
    assert_eq!(refusal(load(&w, dir.path(), None)), Refusal::DevModeOff);
    nothing_written(&w);
    dev(&w, OTHER_DEVICE, true);
    w.runtime.set_unsigned_allowed(true);
    assert_eq!(refusal(load(&w, dir.path(), None)), Refusal::DevModeOff);
    nothing_written(&w);
    dev(&w, DEVICE, true);
    dev(&w, DEVICE, false);
    assert_eq!(refusal(load(&w, dir.path(), None)), Refusal::DevModeOff);
    nothing_written(&w);
    // The preview refuses the same way.
    assert_eq!(
        refusal(w.ledger().preview_folder(&scan(dir.path()), &uopts(None))),
        Refusal::DevModeOff
    );
}

#[test]
fn a_switch_off_between_the_checks_and_the_commit_wins() {
    let w = world();
    dev_on(&w);
    let dir = tool_dir();
    // The load's steps: developer mode (0), the row (1), then the commit (2). Another owner switches
    // developer mode off just before the commit.
    *w.store.before_step.lock().unwrap() = Some((
        2,
        "UPDATE settings SET value = '{\"v\":1,\"devices\":[]}' WHERE key = 'mods_dev_devices'"
            .into(),
    ));
    assert_eq!(refusal(load(&w, dir.path(), None)), Refusal::DevModeOff);
    nothing_written(&w);
}

#[test]
fn a_loaded_folder_is_a_third_party_unsigned_row_with_no_package_and_no_copy() {
    let w = world();
    dev_on(&w);
    let dir = tool_dir();
    let before = tree(dir.path());
    let done = load(&w, dir.path(), None).unwrap();
    assert_eq!(
        (
            done.installed.kind,
            done.installed.state.as_str(),
            done.reload
        ),
        (InstallKind::Fresh, "active", false)
    );
    assert_eq!(
        w.sql::<String>(
            "SELECT signed || tier || installed_from || author || source_repo FROM mods"
        ),
        "0thirdfolderAcmeacme/tool"
    );
    assert_eq!(w.sql::<i64>("SELECT package_sha256 IS NULL FROM mods"), 1);
    assert_eq!(w.sql::<i64>("SELECT consented_at IS NOT NULL FROM mods"), 1);
    assert_eq!(install_id(&w).len(), 16);
    // Served from the developer's own folder: canonical, the very place they gave.
    let root = fs::canonicalize(dir.path()).unwrap();
    assert!(serving(&w) && loaded(&w));
    assert_eq!(w.runtime.installed_root(&acme()), Some(root.clone()));
    assert_eq!(w.runtime.folders().get(&acme()).unwrap().root, root);
    assert_eq!(serve_path(&w, "acme.tool", "app/main.js"), 200);
    // Nothing was copied into PM's mods folder, and nothing was written to the developer's.
    assert!(w.installed_tree().is_empty() && w.staging_is_empty());
    assert_eq!(tree(dir.path()), before);
    // The files column lists what was read, so a viewer of the row sees the same set.
    let files: Vec<FileRecord> =
        serde_json::from_str(&w.sql::<String>("SELECT files FROM mods")).unwrap();
    assert!(files.iter().any(|f| f.path == "app/index.html"));
}

#[test]
fn an_atlas_folder_in_a_development_build_loads_as_third_party_past_the_catalogues_claim() {
    let w = world();
    dev_on(&w);
    let (bytes, key) = pkg("1.0.0", |_| {});
    let signed = package::verify(&bytes, &[key]).unwrap();
    let dir = tempfile::tempdir().unwrap();
    write_files(dir.path(), &signed.files);
    // The index lists atlas.map: a release would refuse the id to anything but the signed mod.
    let index = index_listing(entry_for(&signed, &sha(&bytes)), json!([]));
    let done = w
        .ledger()
        .load_folder(
            &folder::scan(dir.path(), true).unwrap(),
            &uopts(Some(&index)),
            now(),
        )
        .unwrap();
    assert_eq!(done.installed.state, "active");
    // First party in the manifest, third party and unsigned in the row.
    assert_eq!(
        w.sql::<String>("SELECT signed || tier || installed_from FROM mods"),
        "0thirdfolder"
    );
    assert!(w.runtime.is_installed(&id("atlas.map")));
    // A release build's scan (the flag false) refuses the same folder before the ledger sees it.
    assert!(folder::scan(dir.path(), false).is_err());
}

#[test]
fn every_refusal_comes_before_anything_is_written() {
    let w = world();
    dev_on(&w);
    // The API range: a mod that needs a newer PM.
    let newer = tool_dir_with(|m, _| m["apiVersion"] = json!({"min": "9.0", "target": "9.0"}));
    assert!(matches!(
        refusal(load(&w, newer.path(), None)),
        Refusal::NeedsNewerPm { .. }
    ));
    nothing_written(&w);
    // The blacklist, by id and version (a folder has no package hash to match).
    let listed = index_with(|v| {
        v["blacklist"] = json!([{"id": "acme.tool", "versions": "=1.0.0", "action": "paused",
            "reason": "Loses notes.", "fixedIn": "1.0.1"}]);
    });
    let dir = tool_dir();
    assert!(matches!(
        refusal(load(&w, dir.path(), Some(&listed))),
        Refusal::Catalogue(_)
    ));
    nothing_written(&w);
    // (A catalogue id for a folder that isn't `atlas.` is `safety::folder_check`'s, tested there:
    // the index only lists `atlas.` ids, so none can be built for this folder's id.)
    // A manifest that is no good is refused by the scan before the ledger is asked.
    let bad = tool_dir();
    edit_manifest(bad.path(), |m| m["version"] = json!("one"));
    assert!(folder::scan(bad.path(), true).is_err());
    nothing_written(&w);
}

#[test]
fn an_id_held_by_another_source_is_never_taken_over_by_a_folder() {
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
            let dir = tool_dir();
            let r = refusal(load(&w, dir.path(), None));
            assert_eq!(
                w.sql::<String>("SELECT install_id || version FROM mods"),
                before
            );
            assert!(!loaded(&w), "no session for a refused load");
            r
        };
    for (signed, from) in [
        (true, "file"),
        (true, "catalogue"),
        (false, "file"),
        (false, "catalogue"),
    ] {
        assert_eq!(
            held(signed, from, None, "active"),
            Refusal::OtherInstallHoldsId,
            "{signed} {from}"
        );
    }
    assert_eq!(
        held(false, "file", Some("blacklist-removed"), "paused"),
        Refusal::RemovedPending
    );
}

#[test]
fn no_package_installs_over_a_loaded_folder_not_even_the_signed_one_of_the_same_id() {
    let w = world();
    dev_on(&w);
    // An atlas.map folder in a development build, with a NULL pin.
    let (bytes, key) = pkg("1.0.0", |_| {});
    let signed = package::verify(&bytes, &[key]).unwrap();
    let dir = tempfile::tempdir().unwrap();
    write_files(dir.path(), &signed.files);
    w.ledger()
        .load_folder(
            &folder::scan(dir.path(), true).unwrap(),
            &uopts(None),
            now(),
        )
        .unwrap();
    let before: String = w.sql("SELECT install_id || version || state FROM mods");
    // A newer signed release of the very same mod, with the same publisher: an update to anything
    // else, and refused here.
    for version in ["1.0.0", "1.1.0", "2.0.0"] {
        let r = w.install(&pkg(version, |_| {}), Approval::Approved);
        assert_eq!(refusal(r), Refusal::TestFolderHoldsId, "{version}");
    }
    assert_eq!(
        w.sql::<String>("SELECT install_id || version || state FROM mods"),
        before
    );
    assert_eq!(
        w.sql::<String>("SELECT installed_from FROM mods"),
        "folder",
        "the row is still the folder's"
    );
    // An unsigned file of that id is refused too (an atlas. id never passes it, so use acme.tool's).
    let w = world();
    dev_on(&w);
    let tool = tool_dir();
    load(&w, tool.path(), None).unwrap();
    let file = unsigned_package(|_, _| {});
    let pkg = package::verify_unsigned(&file).unwrap();
    let r = w
        .ledger()
        .install_unsigned(&pkg, &sha(&file), &uopts(None), now());
    assert_eq!(refusal(r), Refusal::OtherInstallHoldsId);
}

// ---- loading again --------------------------------------------------------------------------------

#[test]
fn loading_again_is_a_fresh_install_that_resets_grants_and_secrets_and_keeps_storage() {
    let w = world();
    dev_on(&w);
    let dir = tool_dir();
    load(&w, dir.path(), None).unwrap();
    give_it_things(&w);
    let first_id = install_id(&w);
    let frame = live_frame(&w, "acme.tool");
    let other = tool_dir_with(|m, _| m["version"] = json!("1.1.0"));
    let again = load(&w, other.path(), None).unwrap();
    assert!(again.reload);
    assert_ne!(install_id(&w), first_id, "a fresh install id");
    assert!(grants_of(&w, "acme.tool").is_empty());
    assert_eq!(w.sql::<i64>("SELECT count(*) FROM mod_secrets"), 0);
    assert_eq!(
        w.sql::<i64>("SELECT count(*) FROM mod_kv"),
        1,
        "the stored data is kept for the session"
    );
    assert_eq!(w.sql::<String>("SELECT version FROM mods"), "1.1.0");
    assert_eq!(
        w.runtime.folders().get(&acme()).unwrap().root,
        fs::canonicalize(other.path()).unwrap(),
        "served from the folder it was last loaded from"
    );
    assert_eq!(
        w.runtime.installed_root(&acme()),
        Some(fs::canonicalize(other.path()).unwrap())
    );
    // The frame of the old one is told it was replaced.
    assert_eq!(frame.names(), [SESSION_CLOSED]);
    assert_eq!(frame.payloads(SESSION_CLOSED)[0]["reason"], "replaced");
}

#[test]
fn loading_again_under_another_author_or_repository_is_refused_and_inherits_nothing() {
    for (field, value) in [("author", "Mallory"), ("sourceRepo", "mallory/tool")] {
        let w = world();
        dev_on(&w);
        let dir = tool_dir();
        load(&w, dir.path(), None).unwrap();
        give_it_things(&w);
        let before: String = w.sql("SELECT install_id || version || author FROM mods");
        let hostile = tool_dir_with(|m, _| m["publisher"][field] = json!(value));
        assert_eq!(
            refusal(load(&w, hostile.path(), None)),
            Refusal::MakerChanged,
            "{field}"
        );
        assert_eq!(
            w.sql::<String>("SELECT install_id || version || author FROM mods"),
            before,
            "{field}"
        );
        assert_eq!(w.sql::<i64>("SELECT count(*) FROM mod_kv"), 1, "{field}");
    }
}

// ---- a session: nothing survives ---------------------------------------------------------------

#[test]
fn a_startup_deletes_every_folder_row_before_it_reads_any_and_serves_none() {
    let w = world();
    dev_on(&w);
    let dir = tool_dir();
    load(&w, dir.path(), None).unwrap();
    give_it_things(&w);
    // Beside it, a signed mod that must be untouched.
    w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
        .unwrap();
    let frame = live_frame(&w, "acme.tool");
    assert_eq!(w.rows(), 2);

    // The next launch of the app (the runtime is a new one in real life; here the same object, so
    // the startup's own forgetting is what is tested).
    let report = w.ledger().startup(now(), &serve_all);
    assert!(report.skipped.is_none());
    assert_eq!(w.sql::<i64>("SELECT count(*) FROM mods"), 1);
    assert_eq!(w.sql::<String>("SELECT id FROM mods"), "atlas.map");
    assert!(!serving(&w) && !loaded(&w));
    assert_eq!(
        things_left(&w),
        0,
        "grants, storage and secrets went with the row"
    );
    assert_eq!(hidden(&w), r#"{"hidden":["google:me"]}"#);
    assert!(
        w.runtime.is_installed(&id("atlas.map")),
        "a signed mod is untouched"
    );
    assert_eq!(frame.names(), [SESSION_CLOSED]);
    // And the developer's folder is exactly as it was.
    assert!(dir.path().join("app/index.html").is_file());
}

#[test]
fn a_folder_row_with_no_session_is_never_served_by_any_load_and_is_not_parked_or_recovered() {
    let w = world();
    dev_on(&w);
    {
        // A crash's leftover: a row, no session.
        let c = w.store.conn.lock().unwrap();
        insert(
            &c,
            &TestRow {
                id: "acme.tool",
                tier: "third",
                signed: false,
                installed_from: "folder",
                ..TestRow::default()
            },
        );
    }
    let rows = w.ledger().with_conn(|c| Ok(read_rows(c)?)).unwrap();
    let report = w.ledger().load(&rows, now(), &serve_all);
    assert!(report.loaded.is_empty() && report.parked.is_empty());
    assert!(!serving(&w));
    // Its state is not rewritten: the load has nothing to say about a row it doesn't serve.
    assert_eq!(row_state(&w), "active:");
    // The recovery pass looks for no files for it, in this store or any other.
    let r = w.ledger().recover_missing(now(), &serve_all).unwrap();
    assert!(r.recovered.is_empty() && r.unrecovered.is_empty());
    // The next startup deletes it.
    w.ledger().startup(now(), &serve_all);
    assert_eq!(w.rows(), 0);
}

#[test]
fn a_store_closing_forgets_the_session_and_a_load_after_it_serves_nothing() {
    let w = world();
    dev_on(&w);
    let dir = tool_dir();
    load(&w, dir.path(), None).unwrap();
    assert!(serving(&w) && loaded(&w));
    // `on_store_closed`, `settle_registry` and `quiesce` all do this.
    w.runtime.clear();
    assert!(!serving(&w) && !loaded(&w));
    // Reopened as the writer: developer mode is looked up again, and the load runs. The row (the
    // connection that closed couldn't delete it) is not served: there is no session.
    w.runtime.set_unsigned_allowed(true);
    let rows = w.ledger().with_conn(|c| Ok(read_rows(c)?)).unwrap();
    w.ledger().load(&rows, now(), &serve_all);
    assert!(!serving(&w) && w.rows() == 1);
    // ... until the writer's startup, which deletes it.
    w.ledger().startup(now(), &serve_all);
    assert_eq!(w.rows(), 0);
}

#[test]
fn switching_developer_mode_off_unloads_every_folder_and_on_does_not_bring_them_back() {
    let w = world();
    dev_on(&w);
    let dir = tool_dir();
    load(&w, dir.path(), None).unwrap();
    give_it_things(&w);
    // An unsigned FILE mod beside it, which is only paused (the existing behaviour).
    let file = unsigned_package(|m, _| m["id"] = json!("acme.other"));
    let pkg = package::verify_unsigned(&file).unwrap();
    w.ledger()
        .install_unsigned(&pkg, &sha(&file), &uopts(None), now())
        .unwrap();
    let frame = live_frame(&w, "acme.tool");
    let before = tree(dir.path());

    let done = w
        .ledger()
        .set_dev_mode_for_device(DEVICE, false, &serve_all, now())
        .unwrap();
    assert_eq!(done.unloaded, vec!["acme.tool".to_owned()]);
    assert_eq!(done.paused, vec!["acme.other".to_owned()]);
    assert_eq!(
        w.sql::<i64>("SELECT count(*) FROM mods WHERE installed_from = 'folder'"),
        0
    );
    assert!(!serving(&w) && !loaded(&w));
    assert_eq!(things_left(&w), 0);
    assert_eq!(hidden(&w), r#"{"hidden":["google:me"]}"#);
    assert_eq!(frame.names(), [SESSION_CLOSED]);
    assert_eq!(frame.payloads(SESSION_CLOSED)[0]["reason"], "uninstalled");
    assert_eq!(
        tree(dir.path()),
        before,
        "the developer's folder is never touched"
    );

    // On again: the file mod comes back, the folder does not.
    let back = w
        .ledger()
        .set_dev_mode_for_device(DEVICE, true, &serve_all, now())
        .unwrap();
    assert_eq!(back.resumed, vec!["acme.other".to_owned()]);
    assert!(back.unloaded.is_empty());
    assert!(!serving(&w) && !loaded(&w));
    assert_eq!(
        w.sql::<i64>("SELECT count(*) FROM mods WHERE id = 'acme.tool'"),
        0
    );
}

#[test]
fn unloading_deletes_the_row_and_what_cascades_and_never_a_file_or_another_mod() {
    let w = world();
    dev_on(&w);
    let dir = tool_dir();
    load(&w, dir.path(), None).unwrap();
    give_it_things(&w);
    let before = tree(dir.path());
    let frame = live_frame(&w, "acme.tool");
    w.ledger().unload_folder(&acme()).unwrap();
    assert_eq!(w.rows(), 0);
    assert!(!serving(&w) && !loaded(&w));
    assert_eq!(things_left(&w), 0);
    assert_eq!(hidden(&w), r#"{"hidden":["google:me"]}"#);
    assert_eq!(frame.payloads(SESSION_CLOSED)[0]["reason"], "uninstalled");
    assert_eq!(tree(dir.path()), before);
    // Nothing loaded: nothing to unload.
    assert!(matches!(
        w.ledger().unload_folder(&acme()),
        Err(LedgerError::NotInstalled)
    ));
    // An installed mod is never unloaded by this: the row and its files are untouched.
    w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
        .unwrap();
    let tree_before = w.installed_tree();
    assert!(matches!(
        w.ledger().unload_folder(&id("atlas.map")),
        Err(LedgerError::NotInstalled)
    ));
    assert_eq!(w.rows(), 1);
    assert_eq!(w.installed_tree(), tree_before);
    assert!(w.runtime.is_installed(&id("atlas.map")));
}

#[test]
fn an_uninstall_of_a_folder_mod_forgets_its_session_and_never_touches_the_folder() {
    let w = world();
    dev_on(&w);
    let dir = tool_dir();
    load(&w, dir.path(), None).unwrap();
    let before = tree(dir.path());
    let done = w.ledger().uninstall(&acme()).unwrap();
    assert!(done.files_removed);
    assert_eq!(w.rows(), 0);
    assert!(!serving(&w) && !loaded(&w));
    assert_eq!(tree(dir.path()), before);
}

#[test]
fn a_paused_folder_mod_comes_back_only_while_its_session_lasts() {
    let w = world();
    dev_on(&w);
    let dir = tool_dir();
    load(&w, dir.path(), None).unwrap();
    let id = acme();
    w.ledger().pause_by_user(&id, now()).unwrap();
    assert!(!serving(&w) && loaded(&w));
    let standing = w.ledger().resume_by_user(&id, &serve_all, now()).unwrap();
    assert!(standing.serving, "the session serves it again");
    assert!(serving(&w));
    // Paused, the store closes and opens again: the session is gone, so a resume has nothing to serve.
    w.ledger().pause_by_user(&id, now()).unwrap();
    w.runtime.clear();
    w.runtime.set_unsigned_allowed(true);
    let standing = w.ledger().resume_by_user(&id, &serve_all, now()).unwrap();
    assert!(!standing.serving && !serving(&w));
}

// ---- reloading -------------------------------------------------------------------------------------

fn reload(w: &World, dir: &Path) -> Result<Reloaded, LedgerError> {
    // The way `reload_test_mod` does it: read outside the lock, apply under it.
    w.ledger()
        .apply_reload(&acme(), folder::scan(dir, true), &serve_all)
}

#[test]
fn a_reload_picks_up_an_edit_keeps_grants_and_install_id_and_restarts_the_frame() {
    let w = world();
    dev_on(&w);
    let dir = tool_dir();
    load(&w, dir.path(), None).unwrap();
    give_it_things(&w);
    let id_before = install_id(&w);
    let frame = live_frame(&w, "acme.tool");
    // The edit, before the reload: the served file is no longer the one that was read.
    fs::write(dir.path().join("app/main.js"), "console.log('edited');").unwrap();
    assert_eq!(
        serve_path(&w, "acme.tool", "app/main.js"),
        404,
        "a file changed after the scan is refused until a reload reads it"
    );
    let done = reload(&w, dir.path()).unwrap();
    assert!(done.changed);
    assert_eq!(serve_path(&w, "acme.tool", "app/main.js"), 200);
    assert_eq!(install_id(&w), id_before, "the same install");
    assert_eq!(grants_of(&w, "acme.tool").len(), 1, "the grants are kept");
    assert_eq!(things_left(&w), 3);
    assert_eq!(frame.payloads(SESSION_CLOSED)[0]["reason"], "replaced");
    assert!(w.runtime.folders().get(&acme()).unwrap().problem.is_none());
    // The row's file list is the new scan's.
    let files: Vec<FileRecord> =
        serde_json::from_str(&w.sql::<String>("SELECT files FROM mods")).unwrap();
    let main = files.iter().find(|f| f.path == "app/main.js").unwrap();
    assert_eq!(main.size, "console.log('edited');".len() as u64);
}

#[test]
fn a_reload_that_changes_nothing_restarts_no_frame() {
    let w = world();
    dev_on(&w);
    let dir = tool_dir();
    load(&w, dir.path(), None).unwrap();
    let frame = live_frame(&w, "acme.tool");
    let done = reload(&w, dir.path()).unwrap();
    assert!(!done.changed);
    assert!(frame.names().is_empty(), "the frame was not told anything");
    // A change to a file the registry doesn't serve (the notice) updates the row, not the frame.
    fs::write(dir.path().join("NOTICE.txt"), "Changed notice.\n").unwrap();
    let done = reload(&w, dir.path()).unwrap();
    assert!(frame.names().is_empty());
    assert!(done.changed, "the row's file list is new");
}

#[test]
fn a_reload_that_asks_for_more_than_the_user_approved_is_refused_and_the_old_one_runs() {
    let w = world();
    dev_on(&w);
    let dir = tool_dir();
    load(&w, dir.path(), None).unwrap();
    let frame = live_frame(&w, "acme.tool");
    let version = w.sql::<String>("SELECT version || manifest FROM mods");
    for change in [
        // A permission, a host and a secret slot it hadn't asked for.
        Box::new(|m: &mut Value| {
            m["permissions"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id": "documents.text", "why": "x"}))
        }) as Box<dyn FnOnce(&mut Value)>,
        Box::new(|m| m["internetHosts"] = json!(["evil.example.com"])),
        Box::new(|m| m["contributions"]["calendar.overlay"] = json!({"label": "Cal"})),
    ] {
        edit_manifest(dir.path(), change);
        let r = reload(&w, dir.path());
        // Refused for asking for more, or for the manifest itself no longer being valid (a host
        // without the permission for it): either way the old one keeps running.
        assert!(r.is_err(), "{r:?}");
        assert!(w.runtime.folders().get(&acme()).unwrap().problem.is_some());
        assert_eq!(
            w.sql::<String>("SELECT version || manifest FROM mods"),
            version
        );
        assert!(serving(&w));
        assert!(frame.names().is_empty());
        // Put it back for the next round.
        tool_folder(dir.path(), |_, _| {});
        reload(&w, dir.path()).unwrap();
        assert!(w.runtime.folders().get(&acme()).unwrap().problem.is_none());
    }
    // Specifically a widening, by its typed refusal.
    edit_manifest(dir.path(), |m| {
        m["permissions"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": "documents.text", "why": "x"}))
    });
    assert!(matches!(
        refusal(reload(&w, dir.path())),
        Refusal::ReloadWidens(_)
    ));
    // The user loads the folder again and sees the new screen: that load takes it.
    load(&w, dir.path(), None).unwrap();
    assert!(w
        .sql::<String>("SELECT manifest FROM mods")
        .contains("documents.text"));
}

#[test]
fn a_reload_of_another_mod_or_maker_is_refused() {
    let w = world();
    dev_on(&w);
    let dir = tool_dir();
    load(&w, dir.path(), None).unwrap();
    for change in [
        Box::new(|m: &mut Value, _: &mut BTreeMap<String, Vec<u8>>| m["id"] = json!("acme.other"))
            as Box<dyn FnOnce(&mut Value, &mut BTreeMap<String, Vec<u8>>)>,
        Box::new(|m, _| m["publisher"]["author"] = json!("Someone Else")),
        Box::new(|m, _| m["publisher"]["sourceRepo"] = json!("someone/else")),
    ] {
        let other = tool_dir_with(change);
        // The same folder path, its manifest changed to say so.
        fs::copy(
            other.path().join("manifest.json"),
            dir.path().join("manifest.json"),
        )
        .unwrap();
        assert_eq!(refusal(reload(&w, dir.path())), Refusal::ReloadChangedId);
        assert!(serving(&w));
        tool_folder(dir.path(), |_, _| {});
    }
}

#[test]
fn a_half_written_folder_keeps_the_old_one_running_and_the_next_good_scan_clears_the_note() {
    let w = world();
    dev_on(&w);
    let dir = tool_dir();
    load(&w, dir.path(), None).unwrap();
    let good = fs::read(dir.path().join("manifest.json")).unwrap();
    fs::write(dir.path().join("manifest.json"), b"{\"id\": \"acme.to").unwrap();
    let r = reload(&w, dir.path());
    assert!(matches!(r, Err(LedgerError::Package(_))), "{r:?}");
    assert!(serving(&w));
    let problem = w.runtime.folders().get(&acme()).unwrap().problem.unwrap();
    assert!(problem.contains("refused"), "{problem}");
    // The folder disappears altogether: the same.
    let moved = dir.path().with_extension("moved");
    fs::rename(dir.path(), &moved).unwrap();
    assert!(reload(&w, dir.path()).is_err());
    assert!(serving(&w) && loaded(&w));
    fs::rename(&moved, dir.path()).unwrap();
    fs::write(dir.path().join("manifest.json"), good).unwrap();
    assert!(reload(&w, dir.path()).is_ok());
    assert!(w.runtime.folders().get(&acme()).unwrap().problem.is_none());
}

#[test]
fn a_reload_of_a_paused_folder_updates_the_row_and_leaves_it_off() {
    let w = world();
    dev_on(&w);
    let dir = tool_dir();
    load(&w, dir.path(), None).unwrap();
    w.ledger().pause_by_user(&acme(), now()).unwrap();
    fs::write(
        dir.path().join("app/main.js"),
        "console.log('while paused');",
    )
    .unwrap();
    let done = reload(&w, dir.path()).unwrap();
    assert!(done.changed);
    assert_eq!(row_state(&w), "paused:user");
    assert!(!serving(&w), "a reload never switches a paused mod on");
    // Resumed, it serves what was read.
    w.ledger()
        .resume_by_user(&acme(), &serve_all, now())
        .unwrap();
    assert_eq!(serve_path(&w, "acme.tool", "app/main.js"), 200);
}

#[test]
fn a_reload_into_a_listed_version_or_after_an_unload_is_refused() {
    let w = world();
    dev_on(&w);
    let dir = tool_dir();
    load(&w, dir.path(), None).unwrap();
    // The cached safety list names the new version.
    let listed = index_with(|v| {
        v["blacklist"] = json!([{"id": "acme.tool", "versions": "=1.1.0", "action": "paused",
            "reason": "Loses notes.", "fixedIn": "1.1.1"}]);
    });
    let check = SafetyCheck::serve(Some(&listed));
    edit_manifest(dir.path(), |m| m["version"] = json!("1.1.0"));
    let r = w
        .ledger()
        .apply_reload(&acme(), folder::scan(dir.path(), true), &check);
    assert!(matches!(refusal(r), Refusal::Catalogue(_)));
    assert_eq!(w.sql::<String>("SELECT version FROM mods"), "1.0.0");
    // After an unload there is nothing to reload.
    w.ledger().unload_folder(&acme()).unwrap();
    assert!(matches!(
        reload(&w, dir.path()),
        Err(LedgerError::NotInstalled)
    ));
    assert_eq!(w.rows(), 0, "a late reload doesn't bring the row back");
}

#[test]
fn a_reload_read_from_another_folder_than_the_session_is_not_applied() {
    // Loaded again from another folder while a scan of the first was in hand: that scan is stale.
    let w = world();
    dev_on(&w);
    let first = tool_dir();
    load(&w, first.path(), None).unwrap();
    let stale = folder::scan(first.path(), true);
    let second = tool_dir_with(|m, _| m["version"] = json!("2.0.0"));
    load(&w, second.path(), None).unwrap();
    let r = w.ledger().apply_reload(&acme(), stale, &serve_all);
    assert!(matches!(r, Err(LedgerError::NotInstalled)));
    assert_eq!(w.sql::<String>("SELECT version FROM mods"), "2.0.0");
}

// ---- the folder is the developer's, and only files of the layout are served ---------------------

#[cfg(unix)]
#[test]
fn a_file_swapped_for_a_link_out_of_the_folder_after_the_load_is_not_served() {
    use std::os::unix::fs::symlink;
    let w = world();
    dev_on(&w);
    let dir = tool_dir();
    load(&w, dir.path(), None).unwrap();
    assert_eq!(serve_path(&w, "acme.tool", "app/main.js"), 200);
    // The developer's (or anyone's) file replaced by a link to a file of the same content that
    // lives outside: the bytes match, the place doesn't.
    let outside = tempfile::tempdir().unwrap();
    let copy = outside.path().join("main.js");
    fs::copy(dir.path().join("app/main.js"), &copy).unwrap();
    fs::remove_file(dir.path().join("app/main.js")).unwrap();
    symlink(&copy, dir.path().join("app/main.js")).unwrap();
    assert_eq!(serve_path(&w, "acme.tool", "app/main.js"), 404);
    // A reload refuses the link outright (and keeps what it had).
    let r = reload(&w, dir.path());
    assert!(matches!(r, Err(LedgerError::Package(_))), "{r:?}");
    assert_eq!(serve_path(&w, "acme.tool", "app/main.js"), 404);
}

#[test]
fn only_app_files_of_the_scan_are_served_and_nothing_else_in_the_folder() {
    let w = world();
    dev_on(&w);
    let dir = tool_dir();
    fs::write(dir.path().join("README.md"), "private notes").unwrap();
    fs::create_dir_all(dir.path().join("secrets")).unwrap();
    fs::write(dir.path().join("secrets/key.txt"), "k").unwrap();
    load(&w, dir.path(), None).unwrap();
    assert_eq!(serve_path(&w, "acme.tool", "app/main.js"), 200);
    for path in [
        "README.md",
        "secrets/key.txt",
        "manifest.json",
        "LICENCE.txt",
        "app/missing.js",
        "app/../README.md",
    ] {
        assert_eq!(serve_path(&w, "acme.tool", path), 404, "{path}");
    }
}

// ---- the tab's two steps ---------------------------------------------------------------------------

fn env<'a>(index: Option<&'a Index>) -> UnsignedEnv<'a> {
    UnsignedEnv {
        now: now(),
        index,
        engine_sealed: true,
        device: DEVICE,
    }
}

fn answers(items: &[(&str, bool)]) -> Answers {
    items.iter().map(|(p, a)| ((*p).to_owned(), *a)).collect()
}

fn stage_and_take(
    w: &World,
    screen: &mut crate::mods::tab::install::ConsentScreen,
    staged: crate::mods::tab::stage::Staged,
) -> crate::mods::tab::stage::Staged {
    let _ = w;
    let stage = crate::mods::tab::stage::Stage::default();
    let now = std::time::Instant::now();
    screen.token = stage.put(staged, now, 0).unwrap();
    stage.take(&screen.token, now, 0).unwrap()
}

#[test]
fn the_screen_installs_nothing_and_the_typed_id_then_the_answers_load_the_folder() {
    let w = world();
    dev_on(&w);
    let dir = tool_dir();
    let (mut screen, staged) =
        folder_tab::prepare(&w.ledger(), scan(dir.path()), &env(None)).unwrap();
    // The screen: third party, a folder, the id to type; nothing written, nothing served.
    assert_eq!((screen.tier.as_str(), screen.from), ("third", "folder"));
    assert_eq!(screen.unsigned.as_ref().unwrap().confirm_id, "acme.tool");
    nothing_written(&w);
    let staged = stage_and_take(&w, &mut screen, staged);

    // A wrong id loads nothing.
    for typed in [None, Some(""), Some("acme"), Some("acme.tool ")] {
        let e = folder_tab::finish(
            &w.ledger(),
            staged.clone(),
            &answers(&[("storage", true)]),
            typed,
            &env(None),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ConfirmMismatch, "{typed:?}");
        nothing_written(&w);
    }
    // The right id, with the answers: loaded, and the grants written.
    let done = folder_tab::finish(
        &w.ledger(),
        staged.clone(),
        &answers(&[("documents.meta", true)]),
        Some("acme.tool"),
        &env(None),
    );
    let done = match done {
        Ok(d) => d,
        Err(e) => panic!("{e}"),
    };
    assert!(done.result.grants_saved && done.result.grants_problem.is_none());
    assert_eq!(done.result.kind, "fresh");
    assert!(serving(&w) && loaded(&w));
    assert!(grants_of(&w, "acme.tool").values().all(|d| d == "granted"));
}

#[test]
fn an_edited_manifest_since_the_screen_loads_nothing_on_the_old_answers() {
    let w = world();
    dev_on(&w);
    let dir = tool_dir();
    let (mut screen, staged) =
        folder_tab::prepare(&w.ledger(), scan(dir.path()), &env(None)).unwrap();
    let staged = stage_and_take(&w, &mut screen, staged);
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(dir.path().join("manifest.json")).unwrap()).unwrap();
    manifest["description"] = json!("Now it does something else.");
    fs::write(
        dir.path().join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let e = folder_tab::finish(
        &w.ledger(),
        staged,
        &answers(&[("storage", true), ("ui.tab", true)]),
        Some("acme.tool"),
        &env(None),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::ConsentExpired);
    nothing_written(&w);
}

#[test]
fn a_package_token_never_finishes_as_a_folder_and_a_folder_token_never_as_a_package() {
    let w = world();
    dev_on(&w);
    let dir = tool_dir();
    let (mut screen, staged) =
        folder_tab::prepare(&w.ledger(), scan(dir.path()), &env(None)).unwrap();
    let mut staged = stage_and_take(&w, &mut screen, staged);
    // As a package's token (signed, or unsigned): refused before anything is read.
    for trust in [
        crate::mods::tab::stage::Trust::Signed,
        crate::mods::tab::stage::Trust::Unsigned,
    ] {
        let mut other = staged.clone();
        other.trust = trust;
        let e = folder_tab::finish(
            &w.ledger(),
            other,
            &answers(&[("storage", true)]),
            Some("acme.tool"),
            &env(None),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ConsentExpired);
    }
    // A folder token with no folder in it: expired too.
    staged.folder = None;
    let e = folder_tab::finish(
        &w.ledger(),
        staged,
        &answers(&[("storage", true)]),
        Some("acme.tool"),
        &env(None),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::ConsentExpired);
    nothing_written(&w);
}

#[test]
fn d3_is_judged_across_the_other_unsigned_mods_before_and_at_the_load() {
    let w = world();
    dev_on(&w);
    // Another unsigned mod already holds a read grant.
    let file = unsigned_package(|m, _| {
        m["id"] = json!("acme.reader");
        m["contributions"] = json!({});
        m["permissions"] = json!([
            {"id": "storage", "why": "x"}, {"id": "documents.text", "why": "y"}
        ]);
    });
    let pkg = package::verify_unsigned(&file).unwrap();
    w.ledger()
        .install_unsigned(&pkg, &sha(&file), &uopts(None), now())
        .unwrap();
    w.exec(
        "INSERT INTO mod_grants(mod_id, permission, decision) \
         VALUES ('acme.reader','documents.text','granted')",
    );
    // This folder asks to reach the internet: the screen says it is off, and a yes is refused.
    let dir = tool_dir_with(|m, _| {
        m["contributions"] = json!({});
        m["permissions"] = json!([{"id": "storage", "why": "x"}, {"id": "links", "why": "y"}]);
    });
    let (mut screen, staged) =
        folder_tab::prepare(&w.ledger(), scan(dir.path()), &env(None)).unwrap();
    let links = screen
        .permissions
        .iter()
        .find(|p| p.permission.id == "links")
        .unwrap();
    assert!(
        format!("{:?}", links.permission.access).contains("Off")
            || format!("{:?}", links.permission.access)
                .to_lowercase()
                .contains("off"),
        "{:?}",
        links.permission.access
    );
    let staged = stage_and_take(&w, &mut screen, staged);
    let e = folder_tab::finish(
        &w.ledger(),
        staged,
        &answers(&[("storage", true), ("links", true)]),
        Some("acme.tool"),
        &env(None),
    )
    .unwrap_err();
    assert_eq!(e.code, Code::GrantRefused);
    nothing_written_for_tool(&w);
}

fn nothing_written_for_tool(w: &World) {
    assert_eq!(
        w.sql::<i64>("SELECT count(*) FROM mods WHERE id = 'acme.tool'"),
        0
    );
    assert!(!serving(w) && !loaded(w));
}

#[test]
fn a_folder_row_is_never_filled_from_another_stores_files() {
    // A second store of the same vault holds a mod whose files match a folder row's record exactly.
    // The recovery pass copies a missing row's files from a sibling store; a folder row's files are
    // the developer's own and in no store, so it must never be copied into.
    let a = world();
    let data = a.layout.root().parent().unwrap().to_path_buf();
    let b = world_over(Some(&data));
    a.install(&pkg("1.0.0", |_| {}), Approval::Approved)
        .unwrap();
    let (manifest, files): (String, String) = a
        .store
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT manifest, files FROM mods WHERE id = 'atlas.map'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    b.store
        .conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO mods(id, name, version, tier, author, source_repo, source_commit, signed, \
                              install_id, installed_from, package_sha256, manifest, files, state, \
                              consented_at) \
             VALUES ('atlas.map', 'Map', '1.0.0', 'third', 'Atlas', 'itsAtlas-org/Personal-Manager', \
                     'c', 0, '0123456789abcdef', 'folder', NULL, ?1, ?2, 'active', \
                     '2026-11-05T12:00:00.000Z')",
            params![manifest, files],
        )
        .unwrap();
    let report = b.ledger().recover_missing(now(), &serve_all).unwrap();
    assert!(report.recovered.is_empty() && report.unrecovered.is_empty());
    assert!(
        b.installed_tree().is_empty(),
        "nothing was copied into the folder row's place"
    );
}

// ---- a copy of the store that leaves (backup and export) ------------------------------------------

#[test]
fn a_real_folder_row_and_everything_hanging_off_it_stay_out_of_a_copy_that_leaves() {
    // The backup's strip (`backup::strip`, D4) keys on `installed_from = 'folder'`; this loads the
    // folder through the ledger, so the row is the one this code writes and not a hand-built look-alike.
    let w = world();
    dev_on(&w);
    let dir = tool_dir();
    load(&w, dir.path(), None).unwrap();
    give_it_things(&w);
    // A signed mod beside it, with its own content, which must travel.
    w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
        .unwrap();
    w.exec(
        "INSERT INTO mod_kv(mod_id, key, class, value) VALUES ('atlas.map','board','content','\"mine\"');
         INSERT INTO mod_overlay(mod_id, id, date, label) VALUES
             ('acme.tool','e1','2026-12-01','test overlay'), ('atlas.map','e1','2026-12-02','kept');
         INSERT INTO tasks(title) VALUES ('made by the test mod');
         INSERT INTO mod_ledger(mod_id, mod_name, tier, author, source_repo, signed, install_id, task_id)
             VALUES ('acme.tool','Acme Tool','third','Acme','acme/tool',0,'fedcba9876543210',last_insert_rowid());
         INSERT INTO usage_log(model, kind, mod_id) VALUES ('m','background','acme.tool');",
    );
    assert_eq!(
        w.sql::<String>("SELECT installed_from FROM mods WHERE id = 'acme.tool'"),
        crate::mods::store::FROM_FOLDER,
        "the ledger writes the value the strip looks for"
    );

    // The copy a backup makes: a VACUUM INTO of the live store, then the strip.
    let work = w.dir.path().join("work.sqlite");
    let out = w.dir.path().join("leaves.sqlite");
    crate::vault::migrate::vacuum_into(&w.store.conn.lock().unwrap(), &work).unwrap();
    let listed = crate::backup::strip::strip_snapshot(&work, &out, DB_KEY, true).unwrap();

    // The .pmbackup's mod list never names it; the signed mod is there.
    let ids: Vec<&str> = listed.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["atlas.map"]);
    let c = crate::db::open_snapshot(&out, DB_KEY).unwrap();
    let count = |sql: &str| c.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(count("SELECT count(*) FROM mods WHERE id = 'acme.tool'"), 0);
    for table in ["mod_grants", "mod_kv", "mod_secrets", "mod_overlay"] {
        assert_eq!(
            count(&format!(
                "SELECT count(*) FROM {table} WHERE mod_id = 'acme.tool'"
            )),
            0,
            "{table} rows of the folder mod went with it"
        );
    }
    // The signed mod's own content stays, and its overlay.
    assert_eq!(
        count("SELECT count(*) FROM mod_kv WHERE mod_id = 'atlas.map'"),
        1
    );
    assert_eq!(
        count("SELECT count(*) FROM mod_overlay WHERE mod_id = 'atlas.map'"),
        1
    );
    // Its gift stays the user's, made by "a test mod"; its spend stays as PM's own; its overlay is
    // out of the calendar's hidden set.
    assert_eq!(
        c.query_row(
            "SELECT mod_name || '|' || mod_id || '|' || author FROM mod_ledger",
            [],
            |r| { r.get::<_, String>(0) }
        )
        .unwrap(),
        format!("{}||", crate::backup::strip::STRIPPED_MOD_NAME)
    );
    assert_eq!(
        count("SELECT count(*) FROM usage_log WHERE mod_id = 'acme.tool'"),
        0
    );
    assert_eq!(count("SELECT count(*) FROM usage_log"), 1);
    assert_eq!(
        c.query_row(
            "SELECT value FROM settings WHERE key = 'calendar_ui'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        r#"{"hidden":["google:me"]}"#
    );
    // And the live store is exactly as it was: the session goes on.
    assert!(loaded(&w) && w.rows() == 2);
}

#[test]
fn a_restore_swapping_the_store_under_a_loaded_folder_ends_it_and_serves_none_of_the_restored_rows()
{
    // `switch_to_vault` swaps the store (`open_session`) and then engages: the writer's open
    // (`after_writer_open`) forgets every session, deletes every folder row of the store it opened
    // and loads, and the load ends the old frames. Here the "restored" store still holds a folder
    // row (a backup that left before the strip existed could carry one), and the reset runs on it
    // first, as the restore's commit does.
    let w = world();
    dev_on(&w);
    let dir = tool_dir();
    load(&w, dir.path(), None).unwrap();
    give_it_things(&w);
    let frame = live_frame(&w, "acme.tool");
    {
        let c = w.store.conn.lock().unwrap();
        crate::backup::adopt_mods::reset_for_adopt(&c).unwrap();
    }
    // The open that follows looks developer mode up (off for everyone after the reset) and loads.
    w.runtime.set_unsigned_allowed({
        let c = w.store.conn.lock().unwrap();
        devmode::is_on(&c, DEVICE).unwrap()
    });
    let report = w.ledger().startup(now(), &serve_all);
    assert!(report.skipped.is_none());
    assert_eq!(w.rows(), 0, "the folder row went at the open");
    assert!(!serving(&w) && !loaded(&w));
    assert_eq!(things_left(&w), 0, "and what cascades from it");
    assert_eq!(frame.names(), [SESSION_CLOSED]);
    // The same store closed first (the curtain, a lock hand-off): the frames end silently.
    let w = world();
    dev_on(&w);
    load(&w, dir.path(), None).unwrap();
    let frame = live_frame(&w, "acme.tool");
    w.runtime.clear();
    assert!(frame.names().is_empty(), "a closed store tells no frame");
    assert!(!serving(&w) && !loaded(&w));
}
