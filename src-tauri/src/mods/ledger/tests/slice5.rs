// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The safety list against the ledger (slice 5): applying a blacklist to installed rows, the
//! background updater's ledger half, and a pinned-version repair. The harness is the ledger tests'
//! own (`World`, `pkg`), over a real store and real files.

use super::*;
use crate::mods::index::Index;
use crate::mods::package;
use crate::mods::safety::tests::{entry_for, index_listing, index_with, now as list_now};
use crate::mods::safety::{ModNotice, SafetyCheck};
use serde_json::json;
use sha2::{Digest, Sha256};

/// A signed package at `version` and what the index needs to know about it.
struct Built {
    bytes: Vec<u8>,
    keys: TrustedKeys,
    sha: String,
    verified: VerifiedPackage,
}

fn built(version: &str, change: impl FnOnce(&mut Value)) -> Built {
    let (bytes, key) = pkg(version, change);
    let verified = package::verify(&bytes, std::slice::from_ref(&key)).unwrap();
    Built {
        sha: hex::encode(Sha256::digest(&bytes)),
        keys: TrustedKeys::of_test_keys(std::slice::from_ref(&key)),
        verified,
        bytes,
    }
}

impl Built {
    /// An index whose one entry is exactly this package, with `blacklist`.
    fn index(&self, blacklist: Value) -> Index {
        index_listing(entry_for(&self.verified, &self.sha), blacklist)
    }
}

fn pause_of(range: &str, fixed_in: Option<&str>) -> Value {
    let mut e = json!({"id": "atlas.map", "versions": range, "action": "paused",
        "reason": "It loses notes."});
    if let Some(f) = fixed_in {
        e["fixedIn"] = json!(f);
    }
    e
}

fn removal_of(range: &str) -> Value {
    json!({"id": "atlas.map", "versions": range, "action": "removed",
        "reason": "It sends your documents elsewhere."})
}

fn blacklist(entries: Vec<Value>) -> Index {
    index_with(|v| v["blacklist"] = json!(entries))
}

impl World {
    fn apply(&self, index: &Index) -> ApplyReport {
        self.ledger().apply_blacklist(index, list_now())
    }

    fn installed(&self, version: &str) {
        self.install(&pkg(version, |_| {}), Approval::Approved)
            .unwrap();
    }

    fn name(&self) -> String {
        self.sql("SELECT name FROM mods")
    }

    fn state(&self) -> (String, Option<String>) {
        (
            self.sql("SELECT state FROM mods"),
            self.sql("SELECT state_reason FROM mods"),
        )
    }

    fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> T) -> T {
        f(&self.store.conn.lock().unwrap())
    }
}

fn state(s: &str, reason: Option<&str>) -> (String, Option<String>) {
    (s.to_owned(), reason.map(str::to_owned))
}

// ---- Applying the blacklist -----------------------------------------------------------------

#[test]
fn a_pause_stops_the_mod_keeps_its_data_and_files_and_is_applied_once() {
    let w = world();
    w.installed("1.0.0");
    w.exec(
        "INSERT INTO mod_kv(mod_id, key, class, value) VALUES ('atlas.map', 'board', 'content', '{}');
         INSERT INTO mod_grants(mod_id, permission, decision) VALUES ('atlas.map', 'storage', 'granted');",
    );
    assert!(w.served_from().is_some());
    let index = blacklist(vec![pause_of("=1.0.0", Some("1.0.1"))]);

    let report = w.apply(&index);
    assert_eq!(
        report.notices,
        vec![ModNotice::Paused {
            id: "atlas.map".into(),
            name: w.name(),
            version: "1.0.0".into(),
            reason: "It loses notes.".into(),
            fixed_in: Some("1.0.1".into()),
        }]
    );
    assert_eq!(w.state(), state("paused", Some("blacklist")));
    assert_eq!(w.served_from(), None, "the registry is cut");
    assert_eq!(
        w.runtime.open_frame(&id("atlas.map")).unwrap_err(),
        FrameError::NotInstalled
    );
    assert_eq!(w.sql::<i64>("SELECT count(*) FROM mod_kv"), 1, "data kept");
    assert_eq!(w.sql::<i64>("SELECT count(*) FROM mod_grants"), 1);
    assert!(w.version_dir("1.0.0").exists(), "files kept");

    assert!(w.apply(&index).is_empty(), "a second pass changes nothing");
    // The startup load leaves it stopped.
    let load = w.ledger().startup(now(), &SafetyCheck::serve(Some(&index)));
    assert!(load.loaded.is_empty() && load.parked.is_empty());
    assert_eq!(w.state(), state("paused", Some("blacklist")));
}

#[test]
fn a_pause_resumes_when_the_fixed_version_lands_and_says_so() {
    let w = world();
    w.installed("1.0.0");
    let index = blacklist(vec![pause_of(">=1.0.0, <1.0.1", Some("1.0.1"))]);
    w.apply(&index);

    // A pre-release of the fix is past the range but below `fixedIn`: still paused.
    let beta = w
        .install(&pkg("1.0.1-beta.1", |_| {}), Approval::Unattended)
        .unwrap();
    assert_eq!(
        (beta.kind, beta.state.as_str()),
        (InstallKind::Update, "paused")
    );
    assert!(w.apply(&index).is_empty());
    assert_eq!(w.state(), state("paused", Some("blacklist")));
    assert_eq!(w.served_from(), None);

    // The fix lands: the update itself keeps the pause (the ledger doesn't resume), then the
    // blacklist does, and the mod is served again from the new files.
    w.install(&pkg("1.0.1", |_| {}), Approval::Unattended)
        .unwrap();
    assert_eq!(w.state(), state("paused", Some("blacklist")));
    assert_eq!(w.served_from(), None);

    // An index past its life never resumes anything.
    let late = crate::mods::index::parse_time("2027-03-01T00:00:00Z").unwrap();
    assert!(w.ledger().apply_blacklist(&index, late).is_empty());
    assert_eq!(w.state(), state("paused", Some("blacklist")));

    let report = w.apply(&index);
    assert_eq!(
        report.notices,
        vec![ModNotice::Resumed {
            id: "atlas.map".into(),
            name: w.name(),
            version: "1.0.1".into(),
        }]
    );
    assert_eq!(w.state(), state("active", None));
    assert_eq!(w.served_from(), Some(w.canonical_version_dir("1.0.1")));
    let resumed = w.with_conn(|c| read_resumed(c).unwrap());
    assert_eq!(resumed.len(), 1);
    assert_eq!(
        (resumed[0].id.as_str(), resumed[0].version.as_str()),
        ("atlas.map", "1.0.1")
    );
    assert!(w.apply(&index).is_empty());
    w.with_conn(|c| dismiss_resumed(c, "atlas.map").unwrap());
    assert!(w.with_conn(|c| read_resumed(c).unwrap()).is_empty());
}

#[test]
fn a_pause_the_index_lifts_resumes_and_a_users_own_pause_is_not_resumed() {
    let w = world();
    w.installed("1.0.0");
    w.apply(&blacklist(vec![pause_of("*", None)]));
    assert_eq!(w.state(), state("paused", Some("blacklist")));
    // The entry is withdrawn (a false positive): the mod comes back on its own.
    let report = w.apply(&blacklist(vec![]));
    assert!(matches!(report.notices[..], [ModNotice::Resumed { .. }]));
    assert_eq!(w.state(), state("active", None));
    assert!(w.served_from().is_some());
    // What the user paused, with no blacklist entry, is left as it is.
    w.exec("UPDATE mods SET state = 'paused', state_reason = 'user'");
    assert!(w.apply(&blacklist(vec![])).is_empty());
    assert_eq!(w.state(), state("paused", Some("user")));
}

#[test]
fn a_removal_deletes_files_and_rebuildable_rows_and_keeps_what_the_user_made() {
    let w = world();
    w.installed("1.0.0");
    w.exec(
        "INSERT INTO documents(vault_path, title, content_hash) VALUES ('n.md', 'N', 'h');
         INSERT INTO tasks(title) VALUES ('call Sam');
         INSERT INTO mod_grants(mod_id, permission, decision) VALUES ('atlas.map', 'storage', 'granted');
         INSERT INTO mod_kv(mod_id, key, class, value) VALUES ('atlas.map', 'board', 'content', '{}');
         INSERT INTO mod_kv(mod_id, key, class, value) VALUES ('atlas.map', 'layout', 'cache', '[]');
         INSERT INTO mod_doc_data(mod_id, document_id, key, value)
             VALUES ('atlas.map', (SELECT id FROM documents), 'status', 'x');
         INSERT INTO mod_secrets(mod_id, name, host, header, value)
             VALUES ('atlas.map', 'token', 'api.example.com', 'Authorization', 's');
         INSERT INTO mod_overlay(mod_id, id, date, label) VALUES ('atlas.map', 'w1', '2026-12-01', 'x');
         INSERT INTO mod_ledger(mod_id, mod_name, tier, author, source_repo, signed, install_id, task_id)
             SELECT 'atlas.map', name, tier, author, source_repo, signed, install_id,
                    (SELECT id FROM tasks) FROM mods;
         INSERT INTO settings(key, value) VALUES ('mods_updates_waiting',
             '{\"atlas.map\":{\"version\":\"2.0.0\",\"sha256\":\"x\",\"kind\":\"approval\"}}');",
    );
    let index = blacklist(vec![removal_of("<2.0.0")]);
    let report = w.apply(&index);
    assert_eq!(
        report.notices,
        vec![ModNotice::Removed {
            id: "atlas.map".into(),
            name: w.name(),
            version: "1.0.0".into(),
            reason: "It sends your documents elsewhere.".into(),
        }]
    );
    // The typed pending-removal state slice 6's dialog reads.
    assert_eq!(w.state(), state("paused", Some("blacklist-removed")));
    assert_eq!(w.served_from(), None);
    assert!(!w.layout.mod_dir(&id("atlas.map")).exists(), "files gone");
    let count = |t: &str| w.sql::<i64>(&format!("SELECT count(*) FROM {t}"));
    assert_eq!(count("mod_grants"), 0, "grants revoked");
    assert_eq!(count("mod_overlay"), 0);
    assert_eq!(count("mod_doc_data"), 0);
    assert_eq!(
        w.sql::<i64>("SELECT count(*) FROM mod_kv WHERE class = 'cache'"),
        0
    );
    // What the user made or typed waits for their answer (I-16).
    assert_eq!(
        w.sql::<i64>("SELECT count(*) FROM mod_kv WHERE class = 'content'"),
        1,
        "content-class storage stays"
    );
    assert_eq!(count("mod_ledger"), 1, "the gift's attribution stays");
    assert_eq!(count("tasks"), 1, "and the gift");
    assert_eq!(count("mod_secrets"), 1);
    assert_eq!(count("mods"), 1);
    assert!(
        w.with_conn(|c| read_waiting(c).unwrap()).is_empty(),
        "a waiting update for a removed mod is forgotten"
    );

    // Nothing installs onto it, loads it, or puts its files back.
    let wasn_t = w.install(&pkg("1.1.0", |_| {}), Approval::Approved);
    assert!(
        matches!(wasn_t, Err(LedgerError::Refused(Refusal::RemovedPending))),
        "{wasn_t:?}"
    );
    assert!(matches!(
        w.ledger().repair_target(&id("atlas.map")),
        Err(LedgerError::Refused(Refusal::RemovedPending))
    ));
    let check = SafetyCheck::serve(Some(&index));
    let load = w.ledger().startup(now(), &check);
    assert!(load.loaded.is_empty() && load.parked.is_empty() && load.recovered.is_empty());
    let recovered = w.ledger().recover_missing(now(), &check).unwrap();
    assert!(recovered.recovered.is_empty() && recovered.unrecovered.is_empty());
    assert_eq!(w.state(), state("paused", Some("blacklist-removed")));

    // A repeat pass sends no second notice, and finishes a removal a crash interrupted.
    fs::create_dir_all(w.version_dir("1.0.0")).unwrap();
    fs::write(w.version_dir("1.0.0").join("left-over"), b"x").unwrap();
    let again = w.apply(&index);
    assert!(again.notices.is_empty() && again.failed.is_empty());
    assert!(!w.layout.mod_dir(&id("atlas.map")).exists());
    // Even an index that no longer lists it doesn't bring it back.
    assert!(w.apply(&blacklist(vec![])).is_empty());
    assert_eq!(w.state(), state("paused", Some("blacklist-removed")));

    // The user's answer, the ordinary uninstall: the row and its content go, the gift stays.
    w.ledger().uninstall(&id("atlas.map")).unwrap();
    assert_eq!(count("mods"), 0);
    assert_eq!(count("mod_kv"), 0);
    assert_eq!(count("mod_ledger"), 1);
    assert_eq!(count("tasks"), 1);
}

#[test]
fn a_removed_mods_files_are_never_copied_back_from_another_store_or_quarantine() {
    // Another store of the vault on this machine still holds the removed mod's files (a restored
    // copy of this one, say). A removed row is skipped by the recovery that would copy them in.
    let a = world();
    let data = a.layout.root().parent().unwrap().to_path_buf();
    let b = world_over(Some(&data));
    a.installed("1.0.0");
    b.restore_rows_from(&a);
    let index = blacklist(vec![removal_of("*")]);
    assert!(matches!(
        b.apply(&index).notices[..],
        [ModNotice::Removed { .. }]
    ));
    let check = SafetyCheck::serve(Some(&index));
    let report = b.ledger().recover_missing(now(), &check).unwrap();
    assert_eq!(report, StartupReport::default());
    assert!(b.installed_tree().is_empty(), "nothing was copied in");
    b.ledger().startup(now(), &check);
    assert!(b.installed_tree().is_empty());
    // The same row, not removed, would have been recovered: the guard is what stopped it.
    b.exec("UPDATE mods SET state = 'parked', state_reason = 'files-missing'");
    let report = b.ledger().recover_missing(now(), &serve_all).unwrap();
    assert_eq!(report.recovered.len(), 1, "{report:?}");
    // And A, which still has the mod, is untouched by all of it.
    assert_eq!(a.served_from(), Some(a.canonical_version_dir("1.0.0")));
}

#[test]
fn a_removal_by_file_hash_removes_whatever_the_mod_calls_itself() {
    let w = world();
    w.installed("1.0.0");
    let sha: String = w.sql("SELECT package_sha256 FROM mods");
    let index = blacklist(vec![
        json!({"sha256": sha, "action": "removed", "reason": "Known bad file."}),
    ]);
    let report = w.apply(&index);
    assert!(matches!(report.notices[..], [ModNotice::Removed { .. }]));
    assert_eq!(w.state(), state("paused", Some("blacklist-removed")));
}

#[test]
fn a_row_that_changed_since_the_plan_is_left_to_whoever_changed_it() {
    let w = world();
    w.installed("1.0.0");
    // The pass reads the rows (its first step) and then writes the pause (its second): the user's
    // own pause lands between them.
    let first = w.store.steps.load(Ordering::SeqCst);
    *w.store.before_step.lock().unwrap() = Some((
        first + 1,
        "UPDATE mods SET state = 'paused', state_reason = 'user'".to_owned(),
    ));
    let report = w.apply(&blacklist(vec![pause_of("*", None)]));
    assert!(report.notices.is_empty() && report.failed.is_empty());
    assert_eq!(w.state(), state("paused", Some("user")));
    assert_eq!(w.served_from(), None, "and the mod is off all the same");
    // The next pass takes it over.
    *w.store.before_step.lock().unwrap() = None;
    assert_eq!(
        w.apply(&blacklist(vec![pause_of("*", None)])).notices.len(),
        1
    );
    assert_eq!(w.state(), state("paused", Some("blacklist")));
}

#[test]
fn a_failed_write_still_leaves_the_mod_off_and_the_next_pass_writes_it() {
    let w = world();
    w.installed("1.0.0");
    w.exec(
        "CREATE TRIGGER refuse_update BEFORE UPDATE ON mods \
         BEGIN SELECT RAISE(ABORT, 'locked'); END;",
    );
    let index = blacklist(vec![pause_of("*", None)]);
    let report = w.apply(&index);
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert!(report.notices.is_empty());
    assert_eq!(w.served_from(), None, "fail closed: it is off for this run");
    assert_eq!(w.state(), state("active", None));
    w.exec("DROP TRIGGER refuse_update");
    assert_eq!(w.apply(&index).notices.len(), 1);
    assert_eq!(w.state(), state("paused", Some("blacklist")));
}

#[test]
fn an_unreadable_ledger_applies_nothing_and_says_why() {
    let w = world();
    w.installed("1.0.0");
    w.store.closed.store(true, Ordering::SeqCst);
    let report = w.apply(&blacklist(vec![removal_of("*")]));
    assert!(report.skipped.is_some() && report.notices.is_empty());
    w.store.closed.store(false, Ordering::SeqCst);
    assert_eq!(w.state(), state("active", None));
    assert!(w.version_dir("1.0.0").exists());
}

#[test]
fn the_serve_check_holds_a_row_the_apply_has_not_reached_yet() {
    let w = world();
    w.installed("1.0.0");
    let index = blacklist(vec![pause_of("*", None)]);
    let load = w.ledger().startup(now(), &SafetyCheck::serve(Some(&index)));
    assert_eq!(
        load.held,
        vec![(
            "atlas.map".to_owned(),
            "PM's safety list: It loses notes.".to_owned()
        )]
    );
    assert_eq!(w.served_from(), None);
    assert_eq!(w.state(), state("active", None), "held, not yet written");
}

// ---- The background updater ------------------------------------------------------------------

#[test]
fn a_listed_newer_version_downloads_and_installs_as_an_update_that_keeps_consent() {
    let w = world();
    w.installed("1.0.0");
    let new = built("1.1.0", |_| {});
    let index = new.index(json!([]));
    let pass = w.ledger().plan_pass(&index, list_now()).unwrap();
    assert!(pass.notices.is_empty());
    assert_eq!(pass.downloads.len(), 1);
    let download = &pass.downloads[0];
    assert_eq!(
        (
            download.id.as_str(),
            download.version.as_str(),
            download.sha256.as_str()
        ),
        ("atlas.map", "1.1.0", new.sha.as_str())
    );
    let (landed, notices) = w
        .ledger()
        .land(download, &new.bytes, &new.keys, &index, list_now());
    assert!(notices.is_empty());
    match landed {
        Landed::Installed(i) => {
            assert_eq!(
                (i.kind, i.version.as_str(), i.state.as_str()),
                (InstallKind::Update, "1.1.0", "active")
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(w.sql::<String>("SELECT version FROM mods"), "1.1.0");
    assert_eq!(w.served_from(), Some(w.canonical_version_dir("1.1.0")));
    assert!(
        !w.version_dir("1.0.0").exists(),
        "the old version is pruned"
    );
    assert!(w.sql::<bool>("SELECT consented_at IS NOT NULL FROM mods"));
    // Nothing is newer now.
    assert!(w
        .ledger()
        .plan_pass(&index, list_now())
        .unwrap()
        .downloads
        .is_empty());
}

#[test]
fn an_update_never_gives_consent_and_never_unpauses_a_users_pause() {
    let w = world();
    w.installed("1.0.0");
    w.exec("UPDATE mods SET consented_at = NULL, state = 'paused', state_reason = 'user'");
    let new = built("1.1.0", |_| {});
    let index = new.index(json!([]));
    let pass = w.ledger().plan_pass(&index, list_now()).unwrap();
    let (landed, _) = w.ledger().land(
        &pass.downloads[0],
        &new.bytes,
        &new.keys,
        &index,
        list_now(),
    );
    assert!(matches!(landed, Landed::Installed(_)), "{landed:?}");
    assert!(!w.sql::<bool>("SELECT consented_at IS NOT NULL FROM mods"));
    assert_eq!(w.state(), state("paused", Some("user")));
    assert_eq!(w.served_from(), None);
}

#[test]
fn an_update_that_asks_for_more_is_not_applied_and_waits_for_approval() {
    let w = world();
    w.installed("1.0.0");
    let wider = built("1.1.0", |m| {
        m["permissions"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": "tasks.read", "why": "x"}));
    });
    let index = wider.index(json!([]));
    let pass = w.ledger().plan_pass(&index, list_now()).unwrap();
    let (landed, notices) = w.ledger().land(
        &pass.downloads[0],
        &wider.bytes,
        &wider.keys,
        &index,
        list_now(),
    );
    let waiting = match landed {
        Landed::Waiting {
            waiting,
            changed: true,
        } => waiting,
        other => panic!("{other:?}"),
    };
    assert_eq!(waiting.kind, WaitingKind::Approval);
    assert_eq!(waiting.widenings, ["permission:tasks.read"]);
    assert_eq!(
        notices,
        vec![ModNotice::UpdateWaiting {
            id: "atlas.map".into(),
            name: w.name(),
            version: "1.1.0".into(),
        }]
    );
    // Not applied: the old version still runs, with its folder and row.
    assert_eq!(w.sql::<String>("SELECT version FROM mods"), "1.0.0");
    assert_eq!(w.served_from(), Some(w.canonical_version_dir("1.0.0")));
    assert!(!w.version_dir("1.1.0").exists());
    let record = w.with_conn(|c| read_waiting(c).unwrap());
    assert_eq!(record.get("atlas.map"), Some(&waiting));

    // It is asked once: no second download, and a repeat land is no news.
    assert!(w
        .ledger()
        .plan_pass(&index, list_now())
        .unwrap()
        .downloads
        .is_empty());
    let (again, notices) = w.ledger().land(
        &pass.downloads[0],
        &wider.bytes,
        &wider.keys,
        &index,
        list_now(),
    );
    assert!(matches!(again, Landed::Waiting { changed: false, .. }));
    assert!(notices.is_empty());
    assert_eq!(w.ledger().notes().unwrap().0.len(), 1);

    // The user approves: an approved install lands it and the note is gone.
    w.ledger()
        .install_package(
            &wider.bytes,
            &wider.keys,
            &InstallOptions {
                source: Source::Catalogue,
                consented: true,
                approval: Approval::Approved,
                catalogue: &SafetyCheck::catalogue(
                    Some(&index),
                    list_now(),
                    crate::mods::safety::Mode::Latest,
                ),
            },
            list_now(),
        )
        .unwrap();
    assert_eq!(w.sql::<String>("SELECT version FROM mods"), "1.1.0");
    assert!(w.with_conn(|c| read_waiting(c).unwrap()).is_empty());
}

#[test]
fn an_update_that_needs_a_newer_pm_is_recorded_and_announced_once() {
    let w = world();
    w.installed("1.0.0");
    let new = built("1.1.0", |_| {});
    let mut entry = entry_for(&new.verified, &new.sha);
    entry["api"] = json!("1.1-1.1");
    let index = index_listing(entry, json!([]));
    let pass = w.ledger().plan_pass(&index, list_now()).unwrap();
    assert!(pass.downloads.is_empty(), "nothing is downloaded for it");
    assert_eq!(
        pass.notices,
        vec![ModNotice::UpdateNeedsNewerPm {
            id: "atlas.map".into(),
            name: w.name(),
            version: "1.1.0".into(),
            needs_api: "1.1".into(),
        }]
    );
    let record = w.with_conn(|c| read_waiting(c).unwrap());
    let record = record.get("atlas.map").unwrap();
    assert_eq!(record.kind, WaitingKind::NewerPm);
    assert_eq!(record.needs_api.as_deref(), Some("1.1"));
    let second = w.ledger().plan_pass(&index, list_now()).unwrap();
    assert!(second.notices.is_empty() && second.downloads.is_empty());
    assert_eq!(w.sql::<String>("SELECT version FROM mods"), "1.0.0");
}

#[test]
fn a_download_that_isnt_the_listed_file_or_is_blacklisted_or_unwanted_is_refused() {
    let w = world();
    w.installed("1.0.0");
    let new = built("1.1.0", |_| {});
    let index = new.index(json!([]));
    let dl = w.ledger().plan_pass(&index, list_now()).unwrap().downloads;

    // The same version number with other contents: the index vouches for another sha256.
    let other = built("1.1.0", |m| {
        m["description"] = json!("a different file");
    });
    let (landed, _) = w
        .ledger()
        .land(&dl[0], &other.bytes, &other.keys, &index, list_now());
    assert!(
        matches!(&landed, Landed::Failed(why) if why.contains("sha256")),
        "{landed:?}"
    );

    // The right file, signed by a key this machine doesn't trust.
    let stranger = built("1.1.0", |_| {});
    let (landed, _) = w
        .ledger()
        .land(&dl[0], &new.bytes, &stranger.keys, &index, list_now());
    assert!(matches!(landed, Landed::Failed(_)), "{landed:?}");

    // The listed file, but the blacklist now names it by hash.
    let bad = new.index(json!([{"sha256": new.sha, "action": "paused", "reason": "Bad file."}]));
    let (landed, _) = w
        .ledger()
        .land(&dl[0], &new.bytes, &new.keys, &bad, list_now());
    assert!(
        matches!(&landed, Landed::Failed(why) if why.contains("Bad file.")),
        "{landed:?}"
    );

    // An index past its life lists nothing to install.
    let late = crate::mods::index::parse_time("2027-03-01T00:00:00Z").unwrap();
    let (landed, _) = w.ledger().land(&dl[0], &new.bytes, &new.keys, &index, late);
    assert!(
        matches!(&landed, Landed::Failed(why) if why.contains("out of date")),
        "{landed:?}"
    );
    assert!(w
        .ledger()
        .plan_pass(&index, late)
        .unwrap()
        .downloads
        .is_empty());
    assert_eq!(w.sql::<String>("SELECT version FROM mods"), "1.0.0");
    assert!(w.staging_is_empty());
}

#[test]
fn background_work_never_installs_a_mod_that_isnt_there() {
    let w = world();
    let new = built("1.1.0", |_| {});
    let index = new.index(json!([]));
    let download = crate::mods::updater::Download {
        id: "atlas.map".into(),
        version: "1.1.0".into(),
        url: String::new(),
        sha256: new.sha.clone(),
    };
    let (landed, _) = w
        .ledger()
        .land(&download, &new.bytes, &new.keys, &index, list_now());
    assert!(
        matches!(&landed, Landed::Failed(why) if why.contains("no longer installed")),
        "{landed:?}"
    );
    assert_eq!(w.rows(), 0);
    assert!(w.installed_tree().is_empty());
    // The ledger itself refuses it, not only the updater's caller: unattended work never installs
    // fresh.
    let direct = w.ledger().install_package(
        &new.bytes,
        &new.keys,
        &InstallOptions {
            source: Source::Catalogue,
            consented: false,
            approval: Approval::Unattended,
            catalogue: &allow,
        },
        now(),
    );
    assert!(
        matches!(direct, Err(LedgerError::Refused(Refusal::NothingToUpdate))),
        "{direct:?}"
    );
    assert_eq!(w.rows(), 0);
    assert!(w.installed_tree().is_empty() && w.staging_is_empty());
}

#[test]
fn nothing_updates_a_mod_the_safety_list_removed() {
    let w = world();
    w.installed("1.0.0");
    w.apply(&blacklist(vec![removal_of("*")]));
    let new = built("1.1.0", |_| {});
    let index = new.index(json!([]));
    assert!(w
        .ledger()
        .plan_pass(&index, list_now())
        .unwrap()
        .downloads
        .is_empty());
}

// ---- Repair by pinned version ----------------------------------------------------------------

#[test]
fn a_repair_fetches_the_pinned_version_and_file_and_needs_no_index() {
    let w = world();
    let pinned = built("1.0.0", |_| {});
    let landed = w.ledger().install_package(
        &pinned.bytes,
        &pinned.keys,
        &InstallOptions {
            source: Source::Catalogue,
            consented: true,
            approval: Approval::Approved,
            catalogue: &allow,
        },
        now(),
    );
    assert_eq!(landed.unwrap().kind, InstallKind::Fresh);
    // A restore left the row and not the files.
    fs::remove_dir_all(w.layout.mod_dir(&id("atlas.map"))).unwrap();
    let load = w.ledger().startup(now(), &serve_all);
    assert_eq!(load.parked, [("atlas.map".to_owned(), "files-missing")]);

    let target = w.ledger().repair_target(&id("atlas.map")).unwrap();
    assert_eq!(
        target,
        RepairTarget {
            url: "https://github.com/itsAtlas-org/PM-Mods/releases/download/atlas.map-v1.0.0/atlas.map-1.0.0.pmmod".into(),
            version: "1.0.0".into(),
            sha256: pinned.sha.clone(),
        }
    );
    // Not the pinned file: refused, nothing written.
    let other = built("1.0.0", |m| m["description"] = json!("another file"));
    let err = w
        .ledger()
        .install_repair(&target, &other.bytes, &other.keys, None, list_now())
        .unwrap_err();
    assert!(err.to_string().contains("exactly the installed"), "{err}");
    let newer = built("1.1.0", |_| {});
    assert!(w
        .ledger()
        .install_repair(&target, &newer.bytes, &newer.keys, None, list_now())
        .is_err());
    assert!(!w.version_dir("1.0.0").exists());
    // A blacklist that now names it still stops a repair.
    let bad =
        pinned.index(json!([{"sha256": pinned.sha, "action": "paused", "reason": "Bad file."}]));
    let err = w
        .ledger()
        .install_repair(&target, &pinned.bytes, &pinned.keys, Some(&bad), list_now())
        .unwrap_err();
    assert!(err.to_string().contains("Bad file."), "{err}");
    // The pinned file, offline (no index): back, and served again.
    let done = w
        .ledger()
        .install_repair(&target, &pinned.bytes, &pinned.keys, None, list_now())
        .unwrap();
    assert_eq!(done.kind, InstallKind::Repair);
    let load = w.ledger().startup(now(), &serve_all);
    assert_eq!(load.loaded, ["atlas.map"]);
    assert_eq!(w.state(), state("active", None));
}

#[test]
fn an_unsigned_or_unknown_mod_has_nothing_to_repair_from() {
    let w = world();
    assert!(matches!(
        w.ledger().repair_target(&id("atlas.map")),
        Err(LedgerError::NotInstalled)
    ));
    w.installed("1.0.0");
    w.exec("UPDATE mods SET signed = 0");
    assert!(w.ledger().repair_target(&id("atlas.map")).is_err());
}
