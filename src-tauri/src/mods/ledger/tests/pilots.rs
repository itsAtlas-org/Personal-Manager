// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The pilots' auto-install against the ledger (PR B, B8): a marker seeded by hand (the migration
//! that writes one doesn't exist yet), a signed fixture package standing in for the Map, and a
//! fake index. Real store, real files; the download itself is `fetch`'s and is tested there.

use super::*;
use crate::mods::autoinstall::{self, Leave, Step, ADOPT_FLAG, PILOTS};
use crate::mods::index::Index;
use crate::mods::package;
use crate::mods::safety::tests::{entry_for, index_listing, index_with, now as list_now};
use crate::mods::updater::Download;
use serde_json::json;
use sha2::{Digest, Sha256};

const MARKER: &str = "mods_autoinstall_map";

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
    fn entry(&self) -> Value {
        entry_for(&self.verified, &self.sha)
    }

    fn index(&self) -> Index {
        index_listing(self.entry(), json!([]))
    }
}

fn pilot() -> &'static autoinstall::Pilot {
    &PILOTS[0]
}

impl World {
    /// One read of the store, under its lock.
    fn read<T>(&self, f: impl FnOnce(&Connection) -> T) -> T {
        f(&self.store.conn.lock().unwrap())
    }

    /// The mod installed from the catalogue at `version`, as a user would have it.
    fn have(&self, version: &str) {
        self.install(&pkg(version, |_| {}), Approval::Approved)
            .unwrap();
    }

    /// A store that used the core Map: the marker and the three rows the handover moves or deletes.
    fn seed_core_map(&self, pref: &str) {
        self.exec(&format!(
            "INSERT INTO settings(key, value) VALUES ('{MARKER}', '1');
             INSERT INTO settings(key, value) VALUES ('map', '{pref}');
             INSERT INTO settings(key, value) VALUES ('doc_layout_meta', '{{\"x\":1}}');
             INSERT INTO settings(key, value) VALUES ('project_layout_cache', '[1,2]');"
        ));
    }

    fn setting(&self, key: &str) -> Option<String> {
        self.read(|c| {
            c.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()
            .unwrap()
        })
    }

    fn plan(&self, index: Option<&Index>) -> Vec<Step> {
        self.plan_at(index, list_now())
    }

    fn plan_at(&self, index: Option<&Index>, at: DateTime<Utc>) -> Vec<Step> {
        self.ledger()
            .pilot_plan(index, at)
            .unwrap()
            .into_iter()
            .map(|(_, step)| step)
            .collect()
    }

    fn land(&self, b: &Built, index: &Index) -> PilotLanded {
        let Step::Download(d) = self.plan(Some(index)).remove(0) else {
            panic!("not a download")
        };
        self.ledger()
            .land_pilot(pilot(), &d, &b.bytes, &b.keys, index, list_now())
    }

    fn untouched(&self) {
        assert_eq!(self.rows(), 0, "no mods row");
        assert!(self.setting(MARKER).is_some(), "the marker stays");
    }
}

// ---- Install ---------------------------------------------------------------------------------

#[test]
fn a_marker_with_a_current_index_installs_unconsented_and_hands_over_in_the_rows_own_transaction() {
    let w = world();
    let b = built("1.0.0", |_| {});
    let index = b.index();
    w.seed_core_map(r#"{"nodeCap":2500,"tsneEnabled":false}"#);
    // The plan names the listed file, and nothing else is written by planning.
    let plan = w.plan(Some(&index));
    assert_eq!(plan.len(), 1);
    match &plan[0] {
        Step::Download(d) => assert_eq!(
            (d.id.as_str(), d.version.as_str(), d.sha256.as_str()),
            ("atlas.map", "1.0.0", b.sha.as_str())
        ),
        other => panic!("{other:?}"),
    }
    assert_eq!(w.rows(), 0);
    assert!(w.setting(MARKER).is_some());

    match w.land(&b, &index) {
        PilotLanded::Installed(i) => assert_eq!(
            (i.kind, i.version.as_str(), i.state.as_str()),
            (InstallKind::Fresh, "1.0.0", "active")
        ),
        other => panic!("{other:?}"),
    }
    // The row is there, from the catalogue, signed, and not consented: D17's first-open screen.
    assert_eq!(w.rows(), 1);
    assert_eq!(
        w.sql::<String>("SELECT installed_from FROM mods"),
        "catalogue"
    );
    assert!(!w.sql::<bool>("SELECT consented_at IS NOT NULL FROM mods"));
    assert_eq!(w.served_from(), None, "it doesn't run before 'Turn on'");
    assert_eq!(w.sql::<i64>("SELECT count(*) FROM mod_grants"), 0);
    // The marker is gone, the Map's one key is the mod's (content, revision 1, the cap and the
    // switched-off Enhanced layout), and the core Map's rows are deleted.
    assert_eq!(w.setting(MARKER), None);
    assert_eq!(
        serde_json::from_str::<Value>(
            &w.sql::<String>("SELECT value FROM mod_kv WHERE key = 'settings'")
        )
        .unwrap(),
        json!({"nodeCap": 2500, "enhanced": false})
    );
    assert_eq!(
        w.sql::<String>("SELECT class FROM mod_kv WHERE key = 'settings'"),
        "content"
    );
    assert_eq!(
        w.sql::<i64>("SELECT revision FROM mod_kv WHERE key = 'settings'"),
        1
    );
    for key in ["map", "doc_layout_meta", "project_layout_cache"] {
        assert_eq!(w.setting(key), None, "{key}");
    }
    assert!(w.setting(ADOPT_FLAG).is_some());
    assert!(w.setting("mods_pilot_landed_map").is_some());
    // And a later pass has nothing to do.
    assert!(w.plan(Some(&index)).is_empty());
}

#[test]
fn a_store_without_a_marker_is_never_touched() {
    let w = world();
    let b = b_default();
    w.exec("INSERT INTO settings(key, value) VALUES ('map', '{\"nodeCap\":900}')");
    assert!(w.plan(Some(&b.index())).is_empty());
    assert_eq!(w.rows(), 0);
    assert!(w.setting("map").is_some());
    assert!(autoinstall::pending(&w.store.conn.lock().unwrap())
        .unwrap()
        .is_empty());
}

fn b_default() -> Built {
    built("1.0.0", |_| {})
}

#[test]
fn nothing_is_downloaded_without_a_usable_current_index() {
    let w = world();
    let b = b_default();
    w.seed_core_map(r#"{"nodeCap":2500}"#);
    // None at all.
    assert_eq!(
        w.plan(None),
        vec![Step::Leave(Leave::NoCurrentIndex)],
        "no index was ever accepted"
    );
    // One past its life (it may restrict, it may not install).
    let index = b.index();
    let later = list_now() + chrono::Duration::days(400);
    assert_eq!(
        w.plan_at(Some(&index), later),
        vec![Step::Leave(Leave::NoCurrentIndex)]
    );
    // Current, so it is a download: the contrast.
    assert!(matches!(w.plan(Some(&index))[0], Step::Download(_)));
    w.untouched();
}

#[test]
fn an_entry_that_is_blacklisted_too_new_or_missing_is_left_for_later() {
    let w = world();
    let b = b_default();
    w.seed_core_map(r#"{"nodeCap":2500}"#);
    let with = |change: &dyn Fn(&mut Value), blacklist: Value| {
        let mut e = b.entry();
        change(&mut e);
        index_listing(e, blacklist)
    };
    let cases: Vec<(Index, Leave)> = vec![
        (
            with(
                &|_| {},
                json!([{"id": "atlas.map", "versions": ">=1.0.0", "action": "paused",
                        "reason": "It loses notes."}]),
            ),
            Leave::Blacklisted,
        ),
        (
            with(&|e| e["api"] = json!("9.0-9.0"), json!([])),
            Leave::NeedsNewerPm,
        ),
        (
            // The catalogue lists nothing: this pilot isn't there.
            index_with(|v| v["mods"] = json!([])),
            Leave::NotListed,
        ),
    ];
    for (index, why) in cases {
        assert_eq!(w.plan(Some(&index)), vec![Step::Leave(why)], "{why:?}");
    }
    w.untouched();
}

#[test]
fn a_download_that_is_not_the_listed_file_installs_nothing() {
    let w = world();
    let listed = b_default();
    let other = built("1.0.0", |m| {
        m["description"] = json!("Not the listed file.")
    });
    assert_ne!(listed.sha, other.sha);
    let index = listed.index();
    w.seed_core_map(r#"{"nodeCap":2500}"#);
    let Step::Download(d) = w.plan(Some(&index)).remove(0) else {
        panic!()
    };
    // Another file under the listed version: the catalogue check (sha256) refuses.
    let landed = w
        .ledger()
        .land_pilot(pilot(), &d, &other.bytes, &other.keys, &index, list_now());
    assert!(matches!(landed, PilotLanded::Failed(_)), "{landed:?}");
    w.untouched();
    assert_eq!(
        w.sql::<i64>("SELECT count(*) FROM settings WHERE key = 'map'"),
        1
    );
    assert!(w.installed_tree().is_empty(), "no files kept");
    // A download for another pilot's id is refused before the verifier.
    let wrong = Download {
        id: "atlas.other".into(),
        ..d
    };
    assert!(matches!(
        w.ledger().land_pilot(
            pilot(),
            &wrong,
            &listed.bytes,
            &listed.keys,
            &index,
            list_now()
        ),
        PilotLanded::Failed(_)
    ));
    w.untouched();
}

#[test]
fn a_package_signed_by_a_key_this_pm_does_not_trust_installs_nothing() {
    let w = world();
    let b = b_default();
    let stranger = built("1.0.0", |_| {});
    let index = b.index();
    w.seed_core_map(r#"{"nodeCap":2500}"#);
    let Step::Download(d) = w.plan(Some(&index)).remove(0) else {
        panic!()
    };
    // The right bytes, but the keys this PM trusts aren't the ones that signed them.
    let landed = w
        .ledger()
        .land_pilot(pilot(), &d, &b.bytes, &stranger.keys, &index, list_now());
    assert!(matches!(landed, PilotLanded::Failed(_)), "{landed:?}");
    w.untouched();
}

// ---- Failure leaves the old rows and the marker ---------------------------------------------------

#[test]
fn a_failure_while_writing_the_mods_settings_leaves_the_old_rows_the_marker_and_no_files() {
    let w = world();
    let b = b_default();
    let index = b.index();
    w.seed_core_map(r#"{"nodeCap":2500}"#);
    w.exec(
        "CREATE TRIGGER no_kv BEFORE INSERT ON mod_kv \
         BEGIN SELECT RAISE(ABORT, 'disk full'); END;",
    );
    assert!(matches!(w.land(&b, &index), PilotLanded::Failed(_)));
    w.untouched();
    for key in ["map", "doc_layout_meta", "project_layout_cache"] {
        assert!(w.setting(key).is_some(), "{key} stays");
    }
    assert_eq!(w.sql::<i64>("SELECT count(*) FROM mod_kv"), 0);
    assert_eq!(w.setting(ADOPT_FLAG), None);
    assert!(w.installed_tree().is_empty(), "the files went back out");
    assert!(w.staging_is_empty());
    assert_eq!(w.served_from(), None);
    // The next pass finds everything as it was and finishes the job.
    w.exec("DROP TRIGGER no_kv");
    assert!(matches!(w.land(&b, &index), PilotLanded::Installed(_)));
    assert_eq!(w.setting(MARKER), None);
}

#[test]
fn a_failure_while_deleting_the_old_rows_rolls_back_the_row_the_settings_and_the_rows_already_gone()
{
    let w = world();
    let b = b_default();
    let index = b.index();
    w.seed_core_map(r#"{"nodeCap":2500}"#);
    // The last row to go refuses: by then the mod's key is written and two old rows deleted, all of
    // which must come back with the install row.
    w.exec(
        "CREATE TRIGGER no_delete BEFORE DELETE ON settings WHEN OLD.key = 'project_layout_cache' \
         BEGIN SELECT RAISE(ABORT, 'locked'); END;",
    );
    assert!(matches!(w.land(&b, &index), PilotLanded::Failed(_)));
    w.untouched();
    assert_eq!(w.sql::<i64>("SELECT count(*) FROM mod_kv"), 0);
    assert_eq!(w.setting("map").as_deref(), Some(r#"{"nodeCap":2500}"#));
    assert!(w.setting("doc_layout_meta").is_some());
    assert!(w.setting("project_layout_cache").is_some());
    assert!(w.installed_tree().is_empty());
    assert_eq!(w.setting("mods_pilot_landed_map"), None);
}

#[test]
fn a_failure_after_the_handover_but_before_the_commit_leaves_the_old_rows_too() {
    let w = world();
    let b = b_default();
    let index = b.index();
    w.seed_core_map(r#"{"nodeCap":2500}"#);
    // The landed note is written last, in the handover's own step; a refusal there is a failure of
    // the whole install.
    w.exec(
        "CREATE TRIGGER no_note BEFORE INSERT ON settings WHEN NEW.key = 'mods_pilot_landed_map' \
         BEGIN SELECT RAISE(ABORT, 'nope'); END;",
    );
    assert!(matches!(w.land(&b, &index), PilotLanded::Failed(_)));
    w.untouched();
    assert_eq!(w.setting("map").as_deref(), Some(r#"{"nodeCap":2500}"#));
    assert_eq!(w.sql::<i64>("SELECT count(*) FROM mod_kv"), 0);
    assert_eq!(w.setting(ADOPT_FLAG), None);
}

// ---- A row that appeared meanwhile, and a guest ---------------------------------------------------

#[test]
fn a_mod_that_appeared_since_the_plan_is_never_updated_by_the_pass() {
    let w = world();
    let b = b_default();
    let index = b.index();
    w.seed_core_map(r#"{"nodeCap":2500}"#);
    let Step::Download(d) = w.plan(Some(&index)).remove(0) else {
        panic!()
    };
    // The user installed it from the Mods tab while the file came down.
    w.have("1.0.0");
    let before: String = w.sql("SELECT updated_at FROM mods");
    let landed = w
        .ledger()
        .land_pilot(pilot(), &d, &b.bytes, &b.keys, &index, list_now());
    assert!(matches!(landed, PilotLanded::Failed(_)), "{landed:?}");
    // Nothing from the pass: marker and old rows stay, the row is as it was, its files are in place.
    assert!(w.setting(MARKER).is_some());
    assert!(w.setting("map").is_some());
    assert_eq!(w.sql::<String>("SELECT updated_at FROM mods"), before);
    assert!(w.version_dir("1.0.0").exists());
    // A later pass settles it, with no download.
    assert_eq!(w.plan(Some(&index)), vec![Step::Settle]);
}

/// What a store that never asked for the Map keeps: its own core rows, no landed or adopt note.
fn assert_other_vault_untouched(w: &World) {
    assert_eq!(w.setting("map").as_deref(), Some(r#"{"nodeCap":2500}"#));
    assert!(w.setting("doc_layout_meta").is_some());
    assert!(w.setting("project_layout_cache").is_some());
    assert_eq!(w.setting(ADOPT_FLAG), None);
    assert_eq!(w.setting(pilot().landed), None);
    assert_eq!(w.sql::<i64>("SELECT count(*) FROM mod_kv"), 0);
}

#[test]
fn a_download_planned_in_one_vault_installs_nothing_in_a_vault_without_the_marker() {
    let w = world();
    let b = b_default();
    let index = b.index();
    w.seed_core_map(r#"{"nodeCap":2500}"#);
    let Step::Download(d) = w.plan(Some(&index)).remove(0) else {
        panic!()
    };
    // The user switched to a vault that never asked for the Map (or uninstalled it on purpose)
    // while the file came down: the active store is now one with no marker.
    w.exec(&format!("DELETE FROM settings WHERE key = '{MARKER}'"));
    let landed = w
        .ledger()
        .land_pilot(pilot(), &d, &b.bytes, &b.keys, &index, list_now());
    assert!(matches!(landed, PilotLanded::Failed(_)), "{landed:?}");
    assert_eq!(w.rows(), 0, "no mods row");
    assert!(w.installed_tree().is_empty(), "no files");
    assert_other_vault_untouched(&w);
}

#[test]
fn a_settle_planned_in_one_vault_settles_nothing_in_a_vault_without_the_marker() {
    let w = world();
    w.have("1.0.0");
    w.seed_core_map(r#"{"nodeCap":2500}"#);
    assert_eq!(w.plan(None), vec![Step::Settle]);
    // The other vault holds a catalogue row for the mod but never had a marker.
    w.exec(&format!("DELETE FROM settings WHERE key = '{MARKER}'"));
    assert!(w.ledger().settle_pilot(pilot()).is_err());
    assert_eq!(w.rows(), 1, "its own row is as it was");
    assert_other_vault_untouched(&w);
}

#[test]
fn an_older_copy_that_appeared_since_the_plan_is_not_updated_by_the_pass() {
    let w = world();
    let b = b_default();
    let index = b.index();
    w.seed_core_map(r#"{"nodeCap":2500}"#);
    let Step::Download(d) = w.plan(Some(&index)).remove(0) else {
        panic!()
    };
    // The user installed an older version meanwhile. The listed file would be an update, which
    // asks for approval when it widens what was granted: the pass is never the one to give it.
    w.have("0.9.0");
    let landed = w
        .ledger()
        .land_pilot(pilot(), &d, &b.bytes, &b.keys, &index, list_now());
    assert!(matches!(landed, PilotLanded::Failed(_)), "{landed:?}");
    assert_eq!(w.sql::<String>("SELECT version FROM mods"), "0.9.0");
    assert!(w.version_dir("0.9.0").exists());
    assert!(!w.version_dir("1.0.0").exists(), "no files of the update");
    assert!(w.setting(MARKER).is_some());
    assert!(w.setting("map").is_some());
    assert_eq!(w.sql::<i64>("SELECT count(*) FROM mod_kv"), 0);
}

#[test]
fn a_store_that_is_not_the_writers_is_closed_and_nothing_is_planned_or_installed() {
    let w = world();
    let b = b_default();
    let index = b.index();
    w.seed_core_map(r#"{"nodeCap":2500}"#);
    let Step::Download(d) = w.plan(Some(&index)).remove(0) else {
        panic!()
    };
    // A guest of a shared vault has no open store (`lock_session` closes it).
    w.store
        .closed
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(w.ledger().pilot_plan(Some(&index), list_now()).is_err());
    assert!(matches!(
        w.ledger()
            .land_pilot(pilot(), &d, &b.bytes, &b.keys, &index, list_now()),
        PilotLanded::Failed(_)
    ));
    assert!(w.ledger().settle_pilot(pilot()).is_err());
    w.store
        .closed
        .store(false, std::sync::atomic::Ordering::SeqCst);
    w.untouched();
    assert!(w.installed_tree().is_empty());
}

// ---- Settling a pilot that already has a row -------------------------------------------------------

#[test]
fn a_pilot_the_user_already_installed_is_settled_without_a_download() {
    let w = world();
    let b = b_default();
    w.have("1.0.0");
    w.seed_core_map(r#"{"nodeCap":300}"#);
    // Settled whatever the index says, even none: nothing is fetched.
    assert_eq!(w.plan(None), vec![Step::Settle]);
    assert_eq!(w.plan(Some(&b.index())), vec![Step::Settle]);
    w.ledger().settle_pilot(pilot()).unwrap();
    assert_eq!(w.setting(MARKER), None);
    assert_eq!(
        w.sql::<String>("SELECT value FROM mod_kv WHERE key = 'settings'"),
        r#"{"nodeCap":300}"#
    );
    assert_eq!(w.setting("map"), None);
    // The row itself is untouched: still consented, still running.
    assert!(w.sql::<bool>("SELECT consented_at IS NOT NULL FROM mods"));
    assert!(w.served_from().is_some());
}

#[test]
fn the_users_own_settings_in_the_mod_win_and_a_bad_pref_blob_carries_nothing() {
    let w = world();
    w.have("1.0.0");
    w.exec(
        "INSERT INTO mod_kv(mod_id, key, class, value, revision) \
         VALUES ('atlas.map', 'settings', 'content', '{\"nodeCap\":777,\"labels\":true}', 4)",
    );
    w.seed_core_map(r#"{"nodeCap":300}"#);
    w.ledger().settle_pilot(pilot()).unwrap();
    assert_eq!(
        w.sql::<String>("SELECT value FROM mod_kv WHERE key = 'settings'"),
        r#"{"nodeCap":777,"labels":true}"#
    );
    assert_eq!(
        w.sql::<i64>("SELECT revision FROM mod_kv WHERE key = 'settings'"),
        4
    );
    assert_eq!(w.setting("map"), None, "the old row still goes");

    // A blob that isn't JSON, or has no usable cap, carries nothing and is still deleted.
    for (n, blob) in [
        "not json",
        r#"{"tsneEnabled":true}"#,
        r#"{"nodeCap":"big"}"#,
        // Not a JSON boolean: nothing carries (the core read these as the default too).
        r#"{"tsneEnabled":"false"}"#,
        r#"{"tsneEnabled":0}"#,
        r#"{"tsneEnabled":null}"#,
        r#"{"tsneEnabled":[]}"#,
    ]
    .iter()
    .enumerate()
    {
        let w = world();
        w.have("1.0.0");
        w.seed_core_map(blob);
        w.ledger().settle_pilot(pilot()).unwrap();
        assert_eq!(w.sql::<i64>("SELECT count(*) FROM mod_kv"), 0, "case {n}");
        assert_eq!(w.setting("map"), None, "case {n}");
        assert_eq!(w.setting(MARKER), None, "case {n}");
    }
}

#[test]
fn a_cap_outside_the_range_is_clamped_as_the_core_clamped_it() {
    for (given, kept) in [(5, 200), (999_999, 5000), (200, 200), (5000, 5000)] {
        let w = world();
        w.have("1.0.0");
        w.seed_core_map(&format!(r#"{{"nodeCap":{given}}}"#));
        w.ledger().settle_pilot(pilot()).unwrap();
        assert_eq!(
            w.sql::<String>("SELECT value FROM mod_kv WHERE key = 'settings'"),
            format!(r#"{{"nodeCap":{kept}}}"#)
        );
    }
}

#[test]
fn a_row_that_is_not_a_catalogue_install_is_left_alone_with_its_marker() {
    for from in ["folder", "file"] {
        let w = world();
        let b = b_default();
        w.have("1.0.0");
        w.exec(&format!("UPDATE mods SET installed_from = '{from}'"));
        w.seed_core_map(r#"{"nodeCap":300}"#);
        assert_eq!(
            w.plan(Some(&b.index())),
            vec![Step::Leave(Leave::OtherRow)],
            "{from}"
        );
        // Even asked to settle (a plan that went stale), the transaction re-reads the row.
        assert!(w.ledger().settle_pilot(pilot()).is_err(), "{from}");
        assert!(w.setting(MARKER).is_some(), "{from}");
        assert!(w.setting("map").is_some(), "{from}");
        assert_eq!(w.sql::<i64>("SELECT count(*) FROM mod_kv"), 0, "{from}");
    }
}

#[test]
fn an_unsigned_row_with_the_pilots_id_is_not_a_settled_pilot() {
    let w = world();
    w.have("1.0.0");
    w.exec("UPDATE mods SET signed = 0");
    w.seed_core_map(r#"{"nodeCap":300}"#);
    assert_eq!(w.plan(None), vec![Step::Leave(Leave::OtherRow)]);
    assert!(w.ledger().settle_pilot(pilot()).is_err());
    assert!(w.setting(MARKER).is_some());
}

/// What the handover wrote for one stored pref blob (None: it wrote no `settings` key at all).
fn handed_over(blob: &str) -> Option<Value> {
    let w = world();
    w.have("1.0.0");
    w.seed_core_map(blob);
    w.ledger().settle_pilot(pilot()).unwrap();
    assert_eq!(w.setting("map"), None, "{blob}: the old row always goes");
    assert_eq!(w.setting(MARKER), None, "{blob}");
    assert!(w.setting(ADOPT_FLAG).is_some(), "{blob}");
    w.read(|c| {
        c.query_row(
            "SELECT value FROM mod_kv WHERE mod_id = 'atlas.map' AND key = 'settings'",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .unwrap()
    })
    .map(|t| serde_json::from_str(&t).unwrap())
}

#[test]
fn a_switched_off_enhanced_layout_is_handed_over_and_nothing_else_about_t_sne_is() {
    // The cap alone: as before.
    assert_eq!(
        handed_over(r#"{"nodeCap":900}"#),
        Some(json!({"nodeCap": 900}))
    );
    // t-SNE off alone: the old handover wrote nothing; now the Enhanced layout starts off.
    assert_eq!(
        handed_over(r#"{"tsneEnabled":false}"#),
        Some(json!({"enhanced": false}))
    );
    // Both.
    assert_eq!(
        handed_over(r#"{"nodeCap":900,"tsneEnabled":false}"#),
        Some(json!({"nodeCap": 900, "enhanced": false}))
    );
    // Neither, or a `true` (the core's Settings wrote both fields on every save, so a stored
    // `true` says nothing about who had openTSNE): nothing carries.
    assert_eq!(handed_over(r#"{}"#), None);
    assert_eq!(handed_over(r#"{"tsneEnabled":true}"#), None);
    assert_eq!(
        handed_over(r#"{"nodeCap":900,"tsneEnabled":true}"#),
        Some(json!({"nodeCap": 900}))
    );
    // A value that is not a JSON boolean carries nothing, and a valid cap beside it still does.
    for junk in [r#""false""#, "0", "null", "[]", r#""""#, "{}"] {
        assert_eq!(
            handed_over(&format!(r#"{{"tsneEnabled":{junk}}}"#)),
            None,
            "{junk}"
        );
        assert_eq!(
            handed_over(&format!(r#"{{"nodeCap":900,"tsneEnabled":{junk}}}"#)),
            Some(json!({"nodeCap": 900})),
            "{junk}"
        );
    }
}

#[test]
fn the_users_own_enhanced_choice_in_the_mod_wins_over_the_old_t_sne_switch() {
    let w = world();
    w.have("1.0.0");
    w.exec(
        "INSERT INTO mod_kv(mod_id, key, class, value, revision) \
         VALUES ('atlas.map', 'settings', 'content', '{\"enhanced\":true}', 3)",
    );
    w.seed_core_map(r#"{"tsneEnabled":false}"#);
    w.ledger().settle_pilot(pilot()).unwrap();
    assert_eq!(
        w.sql::<String>("SELECT value FROM mod_kv WHERE key = 'settings'"),
        r#"{"enhanced":true}"#
    );
    assert_eq!(
        w.sql::<i64>("SELECT revision FROM mod_kv WHERE key = 'settings'"),
        3
    );
    assert_eq!(w.setting("map"), None, "the old row still goes");
}

// ---- A restore onto another computer: the row came, the files didn't ----------------------------------

/// A signed catalogue row installed at 1.0.0, then its files gone and the row parked for want of
/// them, as the open's load leaves a restored row on a computer that never had the files; with the
/// marker a restore writes. Returns the package the row pins and its keys.
fn restored_without_files(w: &World) -> (Vec<u8>, PublicKey, RepairTarget) {
    let (bytes, key) = pkg("1.0.0", |_| {});
    w.install(&(bytes.clone(), key.clone()), Approval::Approved)
        .unwrap();
    fs::remove_dir_all(w.layout.store_dir().join("atlas.map")).unwrap();
    w.exec(
        "UPDATE mods SET state = 'parked', state_reason = 'files-missing';
         INSERT INTO settings(key, value) VALUES ('mods_autoinstall_map', '1');",
    );
    let target = RepairTarget {
        url: "https://example.invalid/atlas.map-1.0.0.pmmod".to_owned(),
        version: "1.0.0".to_owned(),
        sha256: hex::encode(Sha256::digest(&bytes)),
    };
    (bytes, key, target)
}

#[test]
fn a_restored_row_without_its_files_is_put_back_by_the_pass_and_settled() {
    let w = world();
    let (bytes, key, target) = restored_without_files(&w);
    let keys = TrustedKeys::of_test_keys(&[key]);
    // The row's own pinned file, the Mods tab's repair, but never without a current index: the row
    // came from a backup.
    assert_eq!(w.plan(None), vec![Step::Leave(Leave::NoCurrentIndex)]);
    let index = built("1.0.0", |_| {}).index();
    assert_eq!(w.plan(Some(&index)), vec![Step::Repair]);
    assert_eq!(
        w.ledger().repair_target(&id("atlas.map")).unwrap().version,
        "1.0.0"
    );
    w.ledger()
        .repair_pilot(pilot(), &target, &bytes, &keys, None, list_now())
        .unwrap();
    assert!(w.version_dir("1.0.0").exists(), "the files are back");
    assert_eq!(w.sql::<String>("SELECT state FROM mods"), "active");
    assert!(w.served_from().is_some());
    assert!(
        w.sql::<bool>("SELECT consented_at IS NOT NULL FROM mods"),
        "its consent is kept"
    );
    assert_eq!(w.setting(MARKER), None, "the marker is spent");
    assert!(w.setting("mods_pilot_landed_map").is_some());
    // Whole again, a later pass has nothing to do.
    assert_eq!(w.plan(None), Vec::<Step>::new());
}

#[test]
fn a_restored_row_without_its_files_is_put_back_whatever_state_the_backup_had_it_in() {
    let index = built("1.0.0", |_| {}).index();
    for state in ["paused", "installing", "active", "parked"] {
        let w = world();
        restored_without_files(&w);
        w.exec(&format!(
            "UPDATE mods SET state = '{state}', state_reason = NULL"
        ));
        assert_eq!(w.plan(Some(&index)), vec![Step::Repair], "{state}");
    }
    // A row with its files is settled, not repaired, in any state.
    let w = world();
    w.have("1.0.0");
    w.exec(
        "UPDATE mods SET state = 'paused', state_reason = NULL;
         INSERT INTO settings(key, value) VALUES ('mods_autoinstall_map', '1');",
    );
    assert_eq!(w.plan(Some(&index)), vec![Step::Settle]);
}

#[test]
fn a_restored_row_the_safety_list_names_is_not_put_back_by_the_pass() {
    let w = world();
    let (_, _, target) = restored_without_files(&w);
    let b = built("1.0.0", |_| {});
    let blacklisted = |action: &str| {
        index_listing(
            b.entry(),
            json!([{"id": "atlas.map", "versions": "<=1.0.0", "action": action,
                    "reason": "It loses notes."}]),
        )
    };
    assert_eq!(
        w.plan(Some(&blacklisted("paused"))),
        vec![Step::Leave(Leave::Blacklisted)]
    );
    // Named by its file rather than its version.
    let by_file = index_listing(
        b.entry(),
        json!([{"sha256": target.sha256, "action": "paused",
                "reason": "It loses notes."}]),
    );
    assert_eq!(
        w.plan(Some(&by_file)),
        vec![Step::Leave(Leave::Blacklisted)]
    );
    assert_eq!(w.plan(Some(&b.index())), vec![Step::Repair]);
}

#[test]
fn a_repair_with_a_file_that_is_not_the_pinned_one_changes_nothing_and_keeps_the_marker() {
    let w = world();
    let (_, key, target) = restored_without_files(&w);
    let (other, other_key) = pkg("1.0.0", |m| m["description"] = json!("Something else"));
    let keys = TrustedKeys::of_test_keys(&[key, other_key]);
    assert!(w
        .ledger()
        .repair_pilot(pilot(), &target, &other, &keys, None, list_now())
        .is_err());
    assert!(!w.version_dir("1.0.0").exists());
    assert_eq!(w.sql::<String>("SELECT state FROM mods"), "parked");
    assert!(w.setting(MARKER).is_some(), "the next pass tries again");
    assert_eq!(w.setting("mods_pilot_landed_map"), None);
}

#[test]
fn a_vault_that_landed_before_is_not_handed_over_again_when_a_restore_re_marks_it() {
    let w = world();
    w.have("1.0.0");
    // The restore's marker, on a vault whose handover already ran (the landed note is kept), and
    // whose core rows were deleted then: a leftover `map` row now is not the core Map's.
    w.exec(
        "INSERT INTO settings(key, value) VALUES ('mods_autoinstall_map', '1');
         INSERT INTO settings(key, value) VALUES ('mods_pilot_landed_map', '1');
         INSERT INTO settings(key, value) VALUES ('map', '{\"nodeCap\":300}');",
    );
    w.ledger().settle_pilot(pilot()).unwrap();
    assert_eq!(w.setting(MARKER), None);
    assert_eq!(
        w.setting(ADOPT_FLAG),
        None,
        "no second round of device prefs"
    );
    assert_eq!(w.sql::<i64>("SELECT count(*) FROM mod_kv"), 0);
}

// ---- The table ---------------------------------------------------------------------------------

#[test]
fn the_pilot_table_names_a_first_party_id_and_a_manifest_that_agrees() {
    for p in &PILOTS {
        assert!(p.id.starts_with("atlas."), "{}", p.id);
        assert!(ids::ModId::parse(p.id).is_ok());
        assert!(p.marker.starts_with("mods_autoinstall_"));
    }
    // The committed Map manifest's tab label is the table's, so the "Installing" tab and the
    // installed one read alike.
    let manifest = include_str!("../../../../../mods/atlas.map/manifest.json");
    let v: Value = serde_json::from_str(manifest).unwrap();
    assert_eq!(v["id"], pilot().id);
    assert_eq!(v["contributions"]["nav.tab"]["label"], pilot().tab_label);
    assert_eq!(v["name"], pilot().name);
    // The key the handover writes is one the manifest declares, as content.
    assert!(v["data"]["keys"]
        .as_array()
        .unwrap()
        .iter()
        .any(|k| k["key"] == "settings" && k["class"] == "content"));
}

// ---- A rename or delete before 'Turn on' (PR C, K3) -----------------------------------------------------

/// A pilot installed through `land_pilot` that declares the right to have its stored project names
/// rewritten (`projects.read` and a `projectRefs` path), with its `settings` value holding two.
fn landed_with_project_refs(w: &World, project: &str) {
    let b = built("1.0.0", |m| {
        m["permissions"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": "projects.read", "why": "Keeps its cards on the right project."}));
        m["data"]["projectRefs"] = json!([{"key": "settings", "path": "$.items[*].project"}]);
    });
    let index = b.index();
    w.seed_core_map(r#"{"nodeCap":2500}"#);
    assert!(matches!(w.land(&b, &index), PilotLanded::Installed(_)));
    w.exec(&format!(
        "UPDATE mod_kv SET value = '{}' WHERE key = 'settings'",
        json!({"items": [{"project": project}, {"project": "Other"}]})
    ));
}

fn stored(w: &World) -> (Value, i64) {
    let (value, revision): (String, i64) = w.read(|c| {
        c.query_row(
            "SELECT value, revision FROM mod_kv WHERE key = 'settings'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    });
    (serde_json::from_str(&value).unwrap(), revision)
}

#[test]
fn a_pilot_that_is_not_turned_on_yet_has_its_project_names_rewritten_by_a_rename_and_a_delete() {
    let w = world();
    landed_with_project_refs(&w, "Atlas");
    // As an auto-install leaves it: no grants, no consent, not running.
    assert_eq!(w.sql::<i64>("SELECT count(*) FROM mod_grants"), 0);
    assert!(!w.sql::<bool>("SELECT consented_at IS NOT NULL FROM mods"));
    let (_, before) = stored(&w);

    w.read(|c| crate::projects::rename_project_satellites(c, "Atlas", "Atlas 2"))
        .unwrap();
    let (value, revision) = stored(&w);
    assert_eq!(
        value,
        json!({"items": [{"project": "Atlas 2"}, {"project": "Other"}]})
    );
    assert_eq!(
        revision,
        before + 1,
        "the revision moves, so a frame's stale copy is refused"
    );

    w.read(|c| crate::projects::delete_project_satellites(c, "Atlas 2"))
        .unwrap();
    let (value, revision) = stored(&w);
    assert_eq!(
        value,
        json!({"items": [{"project": null}, {"project": "Other"}]})
    );
    assert_eq!(revision, before + 2);

    // Bind the board to a name again, then drive the real 'Turn on' (the ledger's `consent`, which
    // sets `consented_at` and writes the grants the screen named in one transaction). It reads and
    // writes no stored value: bytes and revision stay exactly as they were.
    w.exec(&format!(
        "UPDATE mod_kv SET value = '{}' WHERE key = 'settings'",
        json!({"items": [{"project": "Atlas"}, {"project": "Other"}]})
    ));
    let bound = stored(&w);
    let raw_before: String = w.sql("SELECT value FROM mod_kv WHERE key = 'settings'");
    let standing = w
        .ledger()
        .consent(
            &crate::mods::ids::ModId::parse("atlas.map").unwrap(),
            &|_: &str, _: &str, _: Option<&str>| None,
            now(),
            |conn| {
                let plan = crate::mods::tab::consent::GrantPlan {
                    grant: vec!["projects.read".into()],
                    ..Default::default()
                };
                crate::mods::tab::consent::apply(
                    conn,
                    &crate::mods::ids::ModId::parse("atlas.map").unwrap(),
                    &plan,
                    true,
                )
            },
        )
        .unwrap();
    assert_eq!(standing.state, "active");
    assert_eq!(
        w.sql::<String>("SELECT value FROM mod_kv WHERE key = 'settings'"),
        raw_before,
        "'Turn on' leaves the stored bytes alone"
    );
    assert_eq!(stored(&w), bound, "and its revision");

    // From now on the grant is what lets a rename through: one rewrite, one bump, and a second run
    // of the same names finds nothing.
    w.read(|c| crate::projects::rename_project_satellites(c, "Atlas", "Atlas 2"))
        .unwrap();
    let after = stored(&w);
    assert_eq!(
        after,
        (
            json!({"items": [{"project": "Atlas 2"}, {"project": "Other"}]}),
            bound.1 + 1
        )
    );
    w.read(|c| crate::projects::rename_project_satellites(c, "Atlas", "Atlas 2"))
        .unwrap();
    assert_eq!(stored(&w), after);
}

#[test]
fn a_pilot_that_was_asked_and_denied_projects_read_keeps_its_string() {
    let w = world();
    landed_with_project_refs(&w, "Atlas");
    // The first-open screen: consented, and said no to this one.
    w.exec(
        "UPDATE mods SET consented_at = '2026-11-20T00:00:00.000Z';
         INSERT INTO mod_grants(mod_id, permission, decision)
             VALUES ('atlas.map', 'projects.read', 'denied');",
    );
    let (_, before) = stored(&w);
    w.read(|c| crate::projects::rename_project_satellites(c, "Atlas", "Atlas 2"))
        .unwrap();
    w.read(|c| crate::projects::delete_project_satellites(c, "Atlas"))
        .unwrap();
    assert_eq!(
        stored(&w),
        (
            json!({"items": [{"project": "Atlas"}, {"project": "Other"}]}),
            before
        )
    );
}
