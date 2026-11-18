// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! An update that adds cache keys to a mod's manifest and nothing else: the Document Map's enhanced
//! layout (1.0.0 declared `settings` as content and `semantic-meta` as cache; a later version adds
//! `semantic-meta-tsne` as cache). Three facts it rests on, each of which already exists in PM and is
//! pinned here so a change to any of them is seen:
//!
//! 1. A data key is not a widening (`policy::widening` compares permissions, internet hosts, secrets,
//!    `nav.tab` / `calendar.overlay` and help ids), so a background update installs unattended.
//! 2. A key's class is read from the installed manifest at every write, so the new key is stored as
//!    `cache` from its first write.
//! 3. A backup's strip deletes every cache-class row and keeps the content-class ones, so the new key
//!    never travels in a backup.
//!
//! This one runs over the API's own test host (a real store, a real runtime, the real dispatcher) with
//! the real ledger on top of its store, the way the API's fixture tests do.

use super::*;
use crate::mods::api::test_support::{live_session, OverHost, TestHost, DB_KEY as HOST_KEY};
use crate::mods::api::{dispatch, Caller, Host, Reply};
use crate::mods::MAIN_LABEL;
use serde_json::json;

const SETTINGS: &str = "settings";
const OLD_CACHE: &str = "semantic-meta";
const NEW_KEY: &str = "semantic-meta-tsne";

/// 1.0.0's keys.
fn keys_100(m: &mut Value) {
    m["data"]["keys"] = json!([
        {"key": SETTINGS, "class": "content"},
        {"key": OLD_CACHE, "class": "cache"},
    ]);
}

/// A later version's keys: 1.0.0's and one more, of `class`.
fn keys_110(m: &mut Value, class: &str) {
    keys_100(m);
    m["data"]["keys"]
        .as_array_mut()
        .unwrap()
        .push(json!({"key": NEW_KEY, "class": class}));
}

async fn set(host: &TestHost, session: &str, key: &str, value: Value) -> Reply {
    dispatch(
        host,
        Caller::Frame {
            window: MAIN_LABEL.into(),
            session: session.into(),
        },
        "storage.set",
        &json!({"key": key, "value": value, "expectedRevision": 0}).to_string(),
    )
    .await
}

fn classes(host: &TestHost) -> Vec<(String, String)> {
    host.sql(|c| {
        let mut stmt = c
            .prepare("SELECT key, class FROM mod_kv WHERE mod_id = 'atlas.map' ORDER BY key")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    })
}

/// 1.0.0 installed with `settings` and `semantic-meta` written, then 1.1.0 (the new key declared as
/// `class`) installed as a background update; the new key then written by the mod. Returns the host
/// and what the update said.
async fn update_adding_a_key(class: &str) -> (TestHost, Result<Installed, LedgerError>) {
    let host = TestHost::new();
    let layout = Layout::new(
        host.dir(),
        VAULT,
        &crate::mods::disk::new_store_id().unwrap(),
    )
    .unwrap();
    let store = OverHost(host.store());
    let ledger = Ledger::new(&layout, &store, host.runtime());

    let old = pkg("1.0.0", keys_100);
    let new = pkg("1.1.0", |m| keys_110(m, class));
    let trusted = |p: &(Vec<u8>, PublicKey)| TrustedKeys::of_test_keys(std::slice::from_ref(&p.1));
    ledger
        .install_package(&old.0, &trusted(&old), &opts(Approval::Approved), now())
        .unwrap();

    let session = live_session(
        &host,
        "atlas.map",
        crate::mods::outbox::testing::discarding(),
    );
    assert!(matches!(
        set(&host, &session, SETTINGS, json!({"layoutMode": "semantic"})).await,
        Reply::Value { .. }
    ));
    assert!(matches!(
        set(&host, &session, OLD_CACHE, json!({"v": 1})).await,
        Reply::Value { .. }
    ));

    let updated =
        ledger.install_package(&new.0, &trusted(&new), &opts(Approval::Unattended), now());
    if updated.is_ok() {
        // The old frame was replaced by the update: a new one writes the new key.
        let again = live_session(
            &host,
            "atlas.map",
            crate::mods::outbox::testing::discarding(),
        );
        assert!(matches!(
            set(&host, &again, NEW_KEY, json!({"v": 1})).await,
            Reply::Value { .. }
        ));
    }
    (host, updated)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_update_that_only_adds_a_cache_key_installs_unattended() {
    let (host, updated) = update_adding_a_key("cache").await;
    // No `NeedsApproval`: a data key is not something an update can widen.
    let installed = updated.expect("the update needed no approval");
    assert_eq!(
        (installed.kind, installed.version.as_str()),
        (InstallKind::Update, "1.1.0")
    );
    let version: String = host.sql(|c| {
        c.query_row("SELECT version FROM mods WHERE id = 'atlas.map'", [], |r| {
            r.get(0)
        })
        .unwrap()
    });
    assert_eq!(version, "1.1.0");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_new_key_is_stored_as_cache_from_its_first_write_and_the_old_ones_keep_their_class() {
    let (host, updated) = update_adding_a_key("cache").await;
    updated.unwrap();
    assert_eq!(
        classes(&host),
        [
            (OLD_CACHE.to_owned(), "cache".to_owned()),
            (NEW_KEY.to_owned(), "cache".to_owned()),
            (SETTINGS.to_owned(), "content".to_owned()),
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_backup_strip_removes_the_new_key_and_keeps_the_settings() {
    let (host, updated) = update_adding_a_key("cache").await;
    updated.unwrap();
    // The copy a backup makes: a VACUUM INTO of the live store, then the strip.
    let work = host.dir().join("work.sqlite");
    let out = host.dir().join("leaves.sqlite");
    crate::vault::migrate::vacuum_into(&host.store().conn().unwrap(), &work).unwrap();
    crate::backup::strip::strip_snapshot(&work, &out, HOST_KEY, true).unwrap();

    let c = crate::db::open_snapshot(&out, HOST_KEY).unwrap();
    let keys: Vec<String> = c
        .prepare("SELECT key FROM mod_kv WHERE mod_id = 'atlas.map' ORDER BY key")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    // Only what the person wrote travels: the settings. Both cache keys are gone.
    assert_eq!(keys, [SETTINGS]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn declared_as_content_instead_the_new_key_would_travel_in_a_backup() {
    // The contrast that gives the strip test its meaning: the class the manifest declares is the class
    // stored, and a content key survives the strip.
    let (host, updated) = update_adding_a_key("content").await;
    updated.unwrap();
    let work = host.dir().join("work.sqlite");
    let out = host.dir().join("leaves.sqlite");
    crate::vault::migrate::vacuum_into(&host.store().conn().unwrap(), &work).unwrap();
    crate::backup::strip::strip_snapshot(&work, &out, HOST_KEY, true).unwrap();
    let c = crate::db::open_snapshot(&out, HOST_KEY).unwrap();
    let keys: Vec<String> = c
        .prepare("SELECT key FROM mod_kv WHERE mod_id = 'atlas.map' ORDER BY key")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(keys, [NEW_KEY, SETTINGS]);
}
