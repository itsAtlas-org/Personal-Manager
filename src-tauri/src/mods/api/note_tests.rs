// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `documents.saveNote` and `documents.statusOf` through the real dispatcher over a real store (PR C,
//! K2): where a note lives for each kind of caller (the namespace table), the key, title and body
//! rules, the engine that must already be set up, and what a mod may learn about a document.

use serde_json::{json, Value};

use super::call_fixtures::*;
use super::note_calls::{check_key, checked_body, checked_title, namespace_for, PINBOARD_ID};
use super::test_support::{SavedNote, TestHost};
use super::{limits, Code, Point};
use crate::mods::api::grants;
use crate::mods::ids::ModId;
use crate::notes::Namespace;

const WRITE: &[&str] = &["documents.write"];

fn saved(host: &TestHost) -> Vec<SavedNote> {
    host.saved_notes()
}

async fn save(host: &TestHost, s: &str, key: &str) -> super::Reply {
    call(
        host,
        s,
        "documents.saveNote",
        json!({"key": key, "title": "A note", "body": "Some text"}),
    )
    .await
}

/// The namespace the real dispatcher chose for a mod of this identity, read off the note it saved.
async fn where_it_lives(host: &TestHost, s: &str, key: &str) -> (String, String) {
    host.set_engine_ready(true);
    value(save(host, s, key).await);
    let note = saved(host).pop().expect("a note was saved");
    (note.source_id, note.file_name)
}

// --- the namespace table ------------------------------------------------------------------------

/// Who a caller is decides where its notes live: only Atlas's own signed Pinboard keeps the legacy
/// names. The id alone is not enough, since a dev build may load an unsigned `atlas.*` folder.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_note_lives_where_the_callers_identity_says_and_never_where_its_text_says() {
    let legacy = ("note:k1".to_owned(), "note-k1.md".to_owned());
    let cases: [(&str, World, (String, String)); 6] = [
        // First-party atlas.pinboard, signed, tier First: the legacy mapping.
        ("first-party pinboard", first(PINBOARD_ID, WRITE), legacy),
        // An unsigned dev-folder atlas.pinboard, claiming tier first in its own row: its own namespace.
        (
            "unsigned dev-folder pinboard",
            unsigned(PINBOARD_ID, "first", WRITE),
            (
                "mod:atlas.pinboard:k1".into(),
                "mod-atlas.pinboard.k1.md".into(),
            ),
        ),
        // The same unsigned folder claiming the third-party tier.
        (
            "unsigned pinboard, third tier",
            unsigned(PINBOARD_ID, "third", WRITE),
            (
                "mod:atlas.pinboard:k1".into(),
                "mod-atlas.pinboard.k1.md".into(),
            ),
        ),
        // A signed second-party row that is called atlas.pinboard (the ledger refuses it at install;
        // the namespace rule must not rely on that).
        (
            "signed second-tier pinboard",
            second(PINBOARD_ID, WRITE),
            (
                "mod:atlas.pinboard:k1".into(),
                "mod-atlas.pinboard.k1.md".into(),
            ),
        ),
        // Another first-party mod: its own.
        (
            "another first-party mod",
            first("atlas.map", WRITE),
            ("mod:atlas.map:k1".into(), "mod-atlas.map.k1.md".into()),
        ),
        // Any other mod.
        (
            "an ordinary mod",
            second("acme.notes", WRITE),
            ("mod:acme.notes:k1".into(), "mod-acme.notes.k1.md".into()),
        ),
    ];
    for (what, (host, s), (source_id, file_name)) in cases {
        let got = where_it_lives(&host, &s, "k1").await;
        assert_eq!(got, (source_id, file_name), "{what}");
    }
}

/// The same mapping read from the facts a store row gives, with nothing a call could say.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_namespace_is_chosen_from_the_row_and_never_from_a_call() {
    let facts = |host: &TestHost, id: &str| {
        host.sql(|c| grants::read_mod(c, &ModId::parse(id).unwrap()))
            .unwrap()
            .unwrap()
    };
    let (host, _s) = first(PINBOARD_ID, WRITE);
    assert_eq!(namespace_for(&facts(&host, PINBOARD_ID)), Namespace::Legacy);
    let (host, _s) = unsigned(PINBOARD_ID, "first", WRITE);
    assert_eq!(
        namespace_for(&facts(&host, PINBOARD_ID)),
        Namespace::Mod(ModId::parse(PINBOARD_ID).unwrap())
    );
    // A call that tries to name a source or a namespace is refused whole.
    let (host, s) = first(PINBOARD_ID, WRITE);
    host.set_engine_ready(true);
    for extra in [
        json!({"key": "k", "title": "t", "body": "b", "sourceId": "note:other"}),
        json!({"key": "k", "title": "t", "body": "b", "namespace": "legacy"}),
        json!({"key": "k", "title": "t", "body": "b", "path": "../x"}),
    ] {
        assert_eq!(
            code(call(&host, &s, "documents.saveNote", extra).await),
            Code::BadArgs
        );
    }
    assert!(saved(&host).is_empty());
}

// --- keys ---------------------------------------------------------------------------------------

/// The keys a namespace takes, as the rule itself (the dispatcher's family limit, which every call
/// pays before its handler, is not what these are about).
#[test]
fn a_key_is_held_to_its_namespaces_length_and_alphabet() {
    let own = Namespace::Mod(ModId::parse("acme.notes").unwrap());
    // The legacy mapping takes up to 200 (a board whose widget ids are older or imported), so a
    // 100-character id is fine; another mod's own namespace stops at 64.
    for len in [1, 36, 64, 100, limits::NOTE_KEY_MAX_LEGACY] {
        assert!(
            check_key(&Namespace::Legacy, &"a".repeat(len)).is_ok(),
            "{len}"
        );
    }
    assert!(check_key(
        &Namespace::Legacy,
        &"a".repeat(limits::NOTE_KEY_MAX_LEGACY + 1)
    )
    .is_err());
    for len in [1, 36, limits::NOTE_KEY_MAX_OWN] {
        assert!(check_key(&own, &"a".repeat(len)).is_ok(), "{len}");
    }
    assert!(check_key(&own, &"a".repeat(limits::NOTE_KEY_MAX_OWN + 1)).is_err());
    // The alphabet: letters, digits, - and _ ; nothing that could name a path or a second namespace.
    for ns in [&Namespace::Legacy, &own] {
        for ok in [
            "w-1730800000000-ab12",
            "3f2b9c1e-8a4d-4f6b-9c2e-1a2b3c4d5e6f",
            "A_b-9",
        ] {
            assert!(check_key(ns, ok).is_ok(), "{ok}");
        }
        for bad in [
            "", "a b", "a.b", "a/b", "..", "a:b", "a\\b", "\u{e9}", "a\nb", "k\u{0}",
        ] {
            let e = check_key(ns, bad).unwrap_err();
            assert_eq!(e.code, Code::BadArgs, "{bad:?}");
            assert!(!e.message.contains(bad) || bad.is_empty());
        }
    }
}

/// A 100-character widget id ingests through the legacy mapping; a 65-character key is refused for
/// another mod. Through the dispatcher, as the call is made.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_long_widget_id_saves_through_the_legacy_mapping_and_not_in_another_namespace() {
    let (host, s) = first(PINBOARD_ID, WRITE);
    host.set_engine_ready(true);
    let long = "w".repeat(100);
    assert_eq!(value(save(&host, &s, &long).await)["saved"], true);
    assert_eq!(saved(&host)[0].source_id, format!("note:{long}"));
    assert_eq!(saved(&host)[0].file_name, format!("note-{long}.md"));
    let (host, s) = second("acme.notes", WRITE);
    host.set_engine_ready(true);
    assert_eq!(code(save(&host, &s, &"a".repeat(65)).await), Code::BadArgs);
    assert_eq!(value(save(&host, &s, &"a".repeat(64)).await)["saved"], true);
    assert_eq!(saved(&host).len(), 1);
}

/// Chat saves its answers as `note:chat-<message id>`: a board planted in a shared vault must not
/// overwrite another person's saved answer through the legacy mapping.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_legacy_mapping_refuses_a_chat_key_and_another_mods_namespace_does_not_need_to() {
    let (host, s) = first(PINBOARD_ID, &["documents.write", "documents.meta"]);
    host.set_engine_ready(true);
    assert_eq!(code(save(&host, &s, "chat-42").await), Code::BadArgs);
    assert_eq!(
        code(
            call(
                &host,
                &s,
                "documents.statusOf",
                json!({"keys": ["chat-42"]})
            )
            .await
        ),
        Code::BadArgs
    );
    assert!(saved(&host).is_empty());
    // In a mod's own namespace the same key is just a key: it can never be `note:chat-42`.
    let (host, s) = second("acme.notes", WRITE);
    let got = where_it_lives(&host, &s, "chat-42").await;
    assert_eq!(got.0, "mod:acme.notes:chat-42");
    // The rule itself.
    assert!(check_key(&Namespace::Legacy, "chat-").is_err());
    assert!(check_key(&Namespace::Legacy, "chat").is_ok());
    assert!(check_key(&Namespace::Legacy, "Chat-1").is_ok());
}

// --- title and body -----------------------------------------------------------------------------

#[test]
fn a_title_is_one_plain_line_and_a_body_has_text_and_a_size() {
    for bad_title in [
        "".to_owned(),
        "   ".to_owned(),
        "two\nlines".to_owned(),
        "tab\there".to_owned(),
        "dashes --- inside".to_owned(),
        "---".to_owned(),
        "t".repeat(limits::NOTE_TITLE_MAX_CHARS + 1),
        "hidden\u{200b}char".to_owned(),
    ] {
        let e = checked_title(&bad_title).unwrap_err();
        assert_eq!(e.code, Code::BadArgs, "{bad_title:?}");
        assert!(!e.message.contains("dashes"), "{}", e.message);
    }
    // At the limit, with spaces around it, is fine and comes back without them; a dash or two is fine.
    let at_limit = "t".repeat(limits::NOTE_TITLE_MAX_CHARS);
    assert_eq!(checked_title(&format!("  {at_limit} ")).unwrap(), at_limit);
    assert_eq!(checked_title("a - b -- c").unwrap(), "a - b -- c");
    // A body: not blank, at most 512 KiB.
    assert_eq!(checked_body("  \n ").unwrap_err().code, Code::BadArgs);
    assert!(checked_body(&"b".repeat(limits::NOTE_BODY_MAX_BYTES)).is_ok());
    assert_eq!(
        checked_body(&"b".repeat(limits::NOTE_BODY_MAX_BYTES + 1))
            .unwrap_err()
            .code,
        Code::TooLarge
    );
}

/// A title is the person's own line: an emoji (variation selector, joiner) and a no-break space
/// inside are fine; a hidden or control character, and a space of any kind at an end, are not.
#[test]
fn a_title_may_hold_an_emoji_and_an_inner_no_break_space_but_nothing_hidden() {
    for good in [
        "\u{26a0}\u{fe0f} Visa",
        "\u{1F468}\u{200d}\u{1F469} family",
        "two\u{a0}words",
    ] {
        assert_eq!(checked_title(good).unwrap(), good);
    }
    for bad in [
        "flip\u{202e}ed",
        "zero\u{200b}width",
        "bell\u{7}",
        "trailing\u{a0}",
        "\u{a0}leading",
    ] {
        assert_eq!(
            checked_title(bad).unwrap_err().code,
            Code::BadArgs,
            "{bad:?}"
        );
    }
}

/// The same rules, through the call: nothing is saved for a call that breaks one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_save_that_breaks_a_rule_saves_nothing() {
    let (host, s) = second("acme.notes", WRITE);
    host.set_engine_ready(true);
    for args in [
        json!({"key": "k", "title": "two\nlines", "body": "text"}),
        json!({"key": "k", "title": "a --- b", "body": "text"}),
        json!({"key": "k", "title": "t", "body": "  "}),
        json!({"key": "k", "title": "t"}),
        json!({"key": "k", "title": "t", "body": "b", "project": "Alpha"}),
    ] {
        assert_eq!(
            code(call(&host, &s, "documents.saveNote", args.clone()).await),
            Code::BadArgs,
            "{args}"
        );
    }
    let over = "b".repeat(limits::NOTE_BODY_MAX_BYTES + 1);
    assert_eq!(
        code(
            call(
                &host,
                &s,
                "documents.saveNote",
                json!({"key": "k", "title": "t", "body": over})
            )
            .await
        ),
        Code::TooLarge
    );
    assert!(saved(&host).is_empty());
    // A title with spaces around it is stored without them.
    value(
        call(
            &host,
            &s,
            "documents.saveNote",
            json!({"key": "k", "title": "  Spaced  ", "body": "text"}),
        )
        .await,
    );
    assert_eq!(saved(&host)[0].title, "Spaced");
}

// --- the engine ---------------------------------------------------------------------------------

/// Spec 1.5: a note's embed needs the engine set up, and PM sets nothing up for a mod. With it off the
/// call answers MOD-5007 and the host's note step (which is `notes::save`, which provisions the venv
/// through `ensure_installed`) is never reached.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_the_engine_a_save_answers_5007_and_touches_nothing() {
    let (host, s) = first(PINBOARD_ID, WRITE);
    assert!(!host.store().sidecar.is_ready());
    let e = error(save(&host, &s, "k1").await);
    assert_eq!(
        (e.code, e.message.as_str()),
        (
            Code::EngineNotReady,
            "PM's document engine isn't set up yet."
        )
    );
    assert_eq!(e.code.as_str(), "MOD-5007");
    assert!(saved(&host).is_empty(), "the save step was reached");
    assert_eq!(count(&host, "SELECT count(*) FROM documents"), 0);
    // The real engine was not asked to provision anything either: its status is as it was.
    assert!(
        matches!(
            host.store().sidecar.status(),
            crate::sidecar::SidecarStatus::NotInstalled
        ),
        "the engine was provisioned"
    );
    // A bad key is a bad key whether or not the engine is up.
    assert_eq!(code(save(&host, &s, "chat-1").await), Code::BadArgs);
    // Set up, it saves.
    host.set_engine_ready(true);
    assert_eq!(value(save(&host, &s, "k1").await)["saved"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_save_is_checked_again_right_before_the_heavy_step() {
    let (host, s) = first(PINBOARD_ID, WRITE);
    host.set_engine_ready(true);
    let session = s.clone();
    host.hook(move |h, point| {
        // The frame goes away after the call was authorised and before its heavy step.
        if point == Point::BeforeData {
            h.runtime().close(&session);
        }
    });
    assert_eq!(code(save(&host, &s, "k1").await), Code::SessionEnded);
    assert!(saved(&host).is_empty());
}

/// Between the engine being read and the heavy step the mod can be gone: the second check, right before
/// the embed, is what catches it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_save_whose_session_ends_after_the_engine_check_never_starts_the_heavy_step() {
    let (host, s) = first(PINBOARD_ID, WRITE);
    host.set_engine_ready(true);
    let session = s.clone();
    host.hook(move |h, point| {
        if point == Point::EngineRead {
            h.runtime().close(&session);
        }
    });
    assert_eq!(code(save(&host, &s, "k1").await), Code::SessionEnded);
    assert!(saved(&host).is_empty());
}

/// The one deliberate exception: the embed and the vault write are not transactional with the mod's
/// state, so a call already into its heavy step when the mod is uninstalled still finishes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_save_already_started_when_the_mod_is_uninstalled_still_finishes() {
    let (host, s) = first(PINBOARD_ID, &["documents.write", "documents.meta"]);
    host.set_engine_ready(true);
    host.hook(|h, point| {
        if point == Point::Embedding {
            h.sql(|c| c.execute("DELETE FROM mods", []).unwrap());
        }
    });
    let v = value(save(&host, &s, "k1").await);
    // It saved, and it says no more than a mod that is gone may be told.
    assert_eq!(v, json!({"saved": true}));
    assert_eq!(saved(&host).len(), 1);
}

// --- what a save tells --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_save_names_the_document_only_to_a_mod_that_may_know_documents() {
    let (host, s) = first(PINBOARD_ID, WRITE);
    host.set_engine_ready(true);
    assert_eq!(value(save(&host, &s, "k1").await), json!({"saved": true}));
    let (host, s) = first(PINBOARD_ID, &["documents.write", "documents.meta"]);
    host.set_engine_ready(true);
    let v = value(save(&host, &s, "k1").await);
    let id = host.sql(|c| {
        c.query_row("SELECT id FROM documents", [], |r| r.get::<_, i64>(0))
            .unwrap()
    });
    assert_eq!(v, json!({"saved": true, "documentId": id}));
    // Never a path, a project or a source id.
    let text = v.to_string();
    assert!(!text.contains("note:") && !text.contains(".md"));
}

// --- documents.statusOf -------------------------------------------------------------------------

/// Three namespaces holding a note under the same key `k`, and one with a project.
fn seed_notes(host: &TestHost) {
    host.sql(|c| {
        for (source, project, importance) in [
            ("note:k", "Alpha", None),
            ("mod:acme.m:k", "", None),
            ("mod:acme.n:k", "", None),
            ("note:chat-1", "", None),
            ("note:shelved", "", Some("archive")),
        ] {
            c.execute(
                "INSERT INTO documents(vault_path, title, content_hash, source_id, project, \
                                       importance, reviewed) \
                 VALUES (?1, 't', ?1, ?1, ?2, ?3, 1)",
                rusqlite::params![source, project, importance],
            )
            .unwrap();
        }
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_status_of_a_note_is_asked_of_the_callers_own_namespace_and_no_other() {
    let (host, a) = first(PINBOARD_ID, &["documents.meta"]);
    let m = add_mod(&host, "acme.m", true, "second", &["documents.meta"], &[]);
    let n = add_mod(&host, "acme.n", true, "second", &["documents.meta"], &[]);
    let unsigned_pin = add_mod(
        &host,
        "atlas.pinboard2",
        false,
        "first",
        &["documents.meta"],
        &[],
    );
    seed_notes(&host);
    let status = |s: &str| {
        let (host, s) = (host.clone(), s.to_owned());
        async move {
            value(
                call(
                    &host,
                    &s,
                    "documents.statusOf",
                    json!({"keys": ["k", "nothing"]}),
                )
                .await,
            )
        }
    };
    // The Pinboard: the legacy `note:k`; the other two: their own; nobody sees anyone else's.
    let v = status(&a).await;
    assert_eq!(v["nothing"], Value::Null);
    assert_eq!(v["k"]["reviewed"], true);
    let id_of = |source: &str| -> i64 {
        host.sql(|c| {
            c.query_row(
                "SELECT id FROM documents WHERE source_id = ?1",
                [source],
                |r| r.get(0),
            )
            .unwrap()
        })
    };
    assert_eq!(v["k"]["documentId"], id_of("note:k"));
    assert_eq!(status(&m).await["k"]["documentId"], id_of("mod:acme.m:k"));
    assert_eq!(status(&n).await["k"]["documentId"], id_of("mod:acme.n:k"));
    // A mod that has none of its own sees none, though three exist under the same key.
    assert_eq!(status(&unsigned_pin).await["k"], Value::Null);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_status_names_a_project_only_to_a_mod_that_may_read_projects_and_a_deleted_note_is_not_ingested(
) {
    let (host, s) = first(PINBOARD_ID, &["documents.meta"]);
    seed_notes(&host);
    let ask = |host: TestHost, s: String| async move {
        value(call(&host, &s, "documents.statusOf", json!({"keys": ["k"]})).await)
    };
    let v = ask(host.clone(), s.clone()).await;
    assert_eq!(
        v["k"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        ["archived", "documentId", "reviewed"]
    );
    let (host2, s2) = first(PINBOARD_ID, &["documents.meta", "projects.read"]);
    seed_notes(&host2);
    let v = ask(host2.clone(), s2.clone()).await;
    assert_eq!(v["k"]["project"], "Alpha");
    assert!(!v.to_string().contains("note:"));
    // A note with no project says null, not an empty name, and an archived one says so.
    let shelved = value(
        call(
            &host2,
            &s2,
            "documents.statusOf",
            json!({"keys": ["shelved"]}),
        )
        .await,
    );
    assert_eq!(shelved["shelved"]["project"], Value::Null);
    assert_eq!(shelved["shelved"]["archived"], true);
    assert_eq!(v["k"]["archived"], false);
    // A deleted document is "not ingested": null, never a stale answer.
    host2.sql(|c| {
        c.execute("DELETE FROM documents WHERE source_id = 'note:k'", [])
            .unwrap()
    });
    assert_eq!(ask(host2, s2).await["k"], Value::Null);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_status_call_refuses_too_many_keys_and_keys_a_note_could_not_have() {
    let (host, s) = first(PINBOARD_ID, &["documents.meta"]);
    seed_notes(&host);
    let many: Vec<String> = (0..=limits::MAX_NOTE_KEYS_PER_CALL)
        .map(|i| format!("k{i}"))
        .collect();
    assert_eq!(
        code(call(&host, &s, "documents.statusOf", json!({"keys": many})).await),
        Code::BadArgs
    );
    let fits: Vec<String> = (0..limits::MAX_NOTE_KEYS_PER_CALL)
        .map(|i| format!("k{i}"))
        .collect();
    let v = value(call(&host, &s, "documents.statusOf", json!({"keys": fits})).await);
    assert_eq!(v.as_object().unwrap().len(), limits::MAX_NOTE_KEYS_PER_CALL);
    // The legacy mapping must never reach Chat's saved answers, even to ask about them.
    for bad in [
        json!({"keys": ["chat-1"]}),
        json!({"keys": ["a.b"]}),
        json!({"keys": [""]}),
        json!({"ids": [1]}),
    ] {
        assert_eq!(
            code(call(&host, &s, "documents.statusOf", bad.clone()).await),
            Code::BadArgs,
            "{bad}"
        );
    }
}
