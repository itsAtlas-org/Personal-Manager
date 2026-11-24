// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Which namespace the Pinboard's notes get when `atlas.pinboard` is INSTALLED through the ledger and
//! signed by a developer's own test key (PR C, K2). `documents.saveNote` maps a note to the legacy
//! `note:<key>` only for Atlas's own signed Pinboard (id AND tier First, as the row says it). The
//! live tests install the Pinboard through a local index signed by `PM_MODS_TEST_KEY`
//! (`keys.rs`: a key of role `DevTest`, which signs and cannot revoke), so what that row is, and so
//! where its notes go, is settled here by the real install path and not assumed.
//!
//! The answer: the install writes the row from the package's own manifest and a verified signature,
//! whichever trusted key made it (`signed = 1`, `tier = first`), so it is `PolicyTier::First` and its
//! notes keep the legacy names. A dev build has no other way to be the Pinboard: an UNSIGNED folder
//! with that id is `PolicyTier::Unsigned` and gets its own namespace (`note_tests.rs`).

use std::collections::BTreeSet;

use super::*;
use crate::mods::api::test_support::{live_session, OverHost, TestHost};
use crate::mods::api::{dispatch, Caller, Host, Reply};
use crate::mods::keys::{AtlasKey, KeyId, KeyRing, KeyRole};
use crate::mods::MAIN_LABEL;
use serde_json::json;

/// The Pinboard as the live tests would build it: its id, and the permission a note needs.
fn pinboard(m: &mut Value) {
    m["id"] = "atlas.pinboard".into();
    m["name"] = "Pinboard".into();
    m["permissions"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id": "documents.write", "why": "Saves your notes."}));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pinboard_signed_by_the_dev_test_key_is_first_party_and_keeps_the_legacy_note_names() {
    let host = TestHost::new();
    let layout = Layout::new(
        host.dir(),
        VAULT,
        &crate::mods::disk::new_store_id().unwrap(),
    )
    .unwrap();
    let store = OverHost(host.store());
    let ledger = Ledger::new(&layout, &store, host.runtime());

    let package = pkg("1.0.0", pinboard);
    // A ring that holds only the dev test key, with the role a dev build gives it.
    let ring = KeyRing::of([AtlasKey {
        role: KeyRole::DevTest,
        id: KeyId::from_bytes([7; 8]),
        key: package.1.clone(),
    }]);
    let trusted = ring.trusted(&BTreeSet::new());
    assert_eq!(
        trusted.signer(0).map(|k| k.role),
        Some(KeyRole::DevTest),
        "the only key is the test key"
    );
    ledger
        .install_package(&package.0, &trusted, &opts(Approval::Approved), now())
        .unwrap();

    // What the install wrote: signed, tier first, from the manifest.
    let row: (bool, String) = host.sql(|c| {
        c.query_row(
            "SELECT signed, tier FROM mods WHERE id = 'atlas.pinboard'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    });
    assert_eq!(row, (true, "first".to_owned()));
    let facts = host
        .sql(|c| crate::mods::api::grants::read_mod(c, &id("atlas.pinboard")))
        .unwrap()
        .unwrap();
    assert_eq!(facts.tier, crate::mods::api::permissions::PolicyTier::First);

    // The person's consent at install records the grant, as the Mods tab does; then the note it
    // saves lands where the Pinboard's notes already are.
    host.sql(|c| {
        crate::mods::api::grants::record_install_consent(
            c,
            &id("atlas.pinboard"),
            &["documents.write"],
            true,
        )
        .unwrap()
    });
    host.set_engine_ready(true);
    let session = live_session(
        &host,
        "atlas.pinboard",
        crate::mods::outbox::testing::discarding(),
    );
    let reply = dispatch(
        &host,
        Caller::Frame {
            window: MAIN_LABEL.into(),
            session,
        },
        "documents.saveNote",
        &json!({"key": "w-1", "title": "T", "body": "text"}).to_string(),
    )
    .await;
    assert!(matches!(reply, Reply::Value { .. }), "{reply:?}");
    let note = host.saved_notes().pop().expect("saved");
    assert_eq!(
        (note.source_id.as_str(), note.file_name.as_str()),
        ("note:w-1", "note-w-1.md")
    );
}
