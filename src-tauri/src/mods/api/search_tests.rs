// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `documents.search` through the real dispatcher over a real store (PR C3). The host's embedder is
//! scripted (one query vector); everything after it is the real retrieval, so what a mod may and may
//! not find is tested where it is decided: no chat, no archived document, no placeholder, and the
//! page backfilled past what is hidden.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::call_fixtures::*;
use super::search_calls::Query;
use super::test_support::TestHost;
use super::{limits, Code, Point};

const SEARCH_MOD: &str = "atlas.finder";
const TEXT: &[&str] = &["documents.text"];

/// A vector with its weight on dimension `i` (and a little on `j`, to rank one chunk behind another
/// at the same `i`).
fn vector(i: usize, j: Option<usize>) -> Vec<f32> {
    let mut v = vec![0.0f32; 384];
    v[i] = 1.0;
    if let Some(j) = j {
        v[j] = 0.5;
    }
    v
}

/// What a seeded document is.
struct Doc<'a> {
    title: &'a str,
    source_type: &'a str,
    importance: Option<&'a str>,
    summary: Option<&'a str>,
    project: &'a str,
}

impl<'a> Doc<'a> {
    fn vault(title: &'a str) -> Self {
        Self {
            title,
            source_type: "vault",
            importance: None,
            summary: None,
            project: "",
        }
    }
}

/// A document with one leaf chunk `(heading, content)` indexed under `embedding` and, in the keyword
/// index, under `indexed` (what a query's words must match). Returns `(document, chunk)` ids.
fn seed(
    host: &TestHost,
    doc: &Doc,
    heading: Option<&str>,
    content: &str,
    indexed: &str,
    embedding: &[f32],
) -> (i64, i64) {
    // Paths, hashes and source ids are unique per document, however often a title repeats.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    host.sql(|c| {
        c.execute(
            "INSERT INTO documents(vault_path, source_path, title, content_hash, source_type, \
                                   importance, stored_summary, project, source_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                format!("secret/vault-path-{n}.md"),
                format!("/home/secret/source-path-{n}.md"),
                doc.title,
                format!("hash-{n}"),
                doc.source_type,
                doc.importance,
                doc.summary,
                doc.project,
                format!("external-ref-{n}"),
            ],
        )
        .unwrap();
        let document = c.last_insert_rowid();
        c.execute(
            "INSERT INTO chunks(document_id, ordinal, heading, content, char_count) \
             VALUES (?1, 0, ?2, ?3, ?4)",
            rusqlite::params![document, heading, content, content.len() as i64],
        )
        .unwrap();
        let chunk = c.last_insert_rowid();
        c.execute(
            "INSERT INTO chunk_vec(rowid, embedding) VALUES (?1, ?2)",
            rusqlite::params![chunk, serde_json::to_string(embedding).unwrap()],
        )
        .unwrap();
        c.execute(
            "INSERT INTO chunks_fts(rowid, content) VALUES (?1, ?2)",
            rusqlite::params![chunk, indexed],
        )
        .unwrap();
        (document, chunk)
    })
}

/// A host with a mod that may search, the engine on and queries landing on dimension 0.
fn searcher(perms: &[&'static str]) -> World {
    let (host, s) = first(SEARCH_MOD, perms);
    host.set_engine_ready(true);
    host.set_query_vector(vector(0, None));
    (host, s)
}

async fn search(host: &TestHost, s: &str, args: Value) -> Value {
    value(call(host, s, "documents.search", args).await)
}

fn ids(v: &Value) -> Vec<i64> {
    v["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["documentId"].as_i64().unwrap())
        .collect()
}

// --- who may ask, and when ----------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_without_documents_text_is_refused() {
    let (host, s) = searcher(&["documents.meta", "tasks.read"]);
    seed(
        &host,
        &Doc::vault("Lease"),
        None,
        "rent is due",
        "rent due",
        &vector(0, None),
    );
    let e = error(call(&host, &s, "documents.search", json!({"query": "rent"})).await);
    assert_eq!(e.code, Code::NotGranted);
    assert!(!serde_json::to_string(&e).unwrap().contains("Lease"));
    // Nothing was embedded or searched.
    assert!(host.searched().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_engine_off_is_mod_5007() {
    let (host, s) = searcher(TEXT);
    host.set_engine_ready(false);
    seed(
        &host,
        &Doc::vault("Lease"),
        None,
        "rent is due",
        "rent due",
        &vector(0, None),
    );
    let reply = call(&host, &s, "documents.search", json!({"query": "rent"})).await;
    assert_eq!(code(reply), Code::EngineNotReady);
    // The check came before any retrieval.
    assert!(host.searched().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_uninstalled_mid_call_returns_nothing() {
    let (host, s) = searcher(TEXT);
    seed(
        &host,
        &Doc::vault("Lease"),
        None,
        "rent is due",
        "rent due",
        &vector(0, None),
    );
    // The mod is gone while its query is being embedded: the answer it asked for is never built.
    host.hook(|h, point| {
        if point == Point::Embedding {
            h.sql(|c| c.execute("DELETE FROM mods", []).unwrap());
        }
    });
    let reply = call(&host, &s, "documents.search", json!({"query": "rent"})).await;
    assert_eq!(code(reply), Code::SessionEnded);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_is_admitted_under_the_embed_family() {
    let (host, s) = searcher(TEXT);
    seed(
        &host,
        &Doc::vault("Lease"),
        None,
        "rent is due",
        "rent due",
        &vector(0, None),
    );
    // The embed family's burst, then its bucket is empty (a general call would not be limited so).
    for _ in 0..limits::EMBED_BURST {
        search(&host, &s, json!({"query": "rent"})).await;
    }
    let reply = call(&host, &s, "documents.search", json!({"query": "rent"})).await;
    assert_eq!(code(reply), Code::RateLimited);
}

// --- what may be found --------------------------------------------------------------------------

/// A chat chunk found by both branches, by the keyword branch alone and by the vector branch alone is
/// never returned; an ordinary document found the same ways is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_never_returns_chat_chunks() {
    let (host, s) = searcher(TEXT);
    let chat = Doc {
        source_type: "chat",
        ..Doc::vault("Chat")
    };
    let both = seed(
        &host,
        &chat,
        None,
        "rent talk",
        "rent talk",
        &vector(0, None),
    )
    .0;
    // Words match, vector far away.
    let keyword_only = seed(
        &host,
        &chat,
        None,
        "rent talk 2",
        "rent talk",
        &vector(5, None),
    )
    .0;
    // Vector right on the query, words don't match.
    let vector_only = seed(
        &host,
        &chat,
        None,
        "elsewhere",
        "unrelated",
        &vector(0, None),
    )
    .0;
    let doc = seed(
        &host,
        &Doc::vault("Lease"),
        None,
        "rent is due",
        "rent due",
        &vector(0, Some(1)),
    )
    .0;
    let found = ids(&search(&host, &s, json!({"query": "rent"})).await);
    assert_eq!(found, [doc]);
    for hidden in [both, keyword_only, vector_only] {
        assert!(!found.contains(&hidden));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_never_returns_archived_documents() {
    let (host, s) = searcher(TEXT);
    let shelved = Doc {
        importance: Some("archive"),
        ..Doc::vault("Old lease")
    };
    // Archived documents stay keyword-findable and vector-findable, so both branches must drop them.
    seed(
        &host,
        &shelved,
        None,
        "rent was due",
        "rent due",
        &vector(0, None),
    );
    seed(
        &host,
        &shelved,
        None,
        "rent too",
        "rent due",
        &vector(5, None),
    );
    seed(
        &host,
        &shelved,
        None,
        "elsewhere",
        "unrelated",
        &vector(0, None),
    );
    let live = seed(
        &host,
        &Doc::vault("Lease"),
        None,
        "rent is due",
        "rent due",
        &vector(0, Some(1)),
    )
    .0;
    assert_eq!(
        ids(&search(&host, &s, json!({"query": "rent"})).await),
        [live]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_never_returns_the_index_only_placeholder() {
    let (host, s) = searcher(TEXT);
    let placeholder = crate::ingest::INDEX_ONLY_BODY_PLACEHOLDER;
    // An index-only document with no summary yet: its chunk is the placeholder, found both ways.
    let bare = Doc {
        source_type: "index_only",
        ..Doc::vault("Remote file")
    };
    seed(
        &host,
        &bare,
        None,
        placeholder,
        "rent due",
        &vector(0, None),
    );
    // ...and one whose summary is blank (stored as an empty string, which the loader treats as none).
    let blank = Doc {
        source_type: "index_only",
        summary: Some(""),
        ..Doc::vault("Blank summary")
    };
    seed(
        &host,
        &blank,
        None,
        placeholder,
        "rent due",
        &vector(0, None),
    );
    let live = seed(
        &host,
        &Doc::vault("Lease"),
        None,
        "rent is due",
        "rent due",
        &vector(0, Some(1)),
    )
    .0;
    let v = search(&host, &s, json!({"query": "rent"})).await;
    assert_eq!(ids(&v), [live]);
    assert!(!v.to_string().contains(placeholder));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_marks_summary_hits_index_only() {
    let (host, s) = searcher(TEXT);
    let remote = Doc {
        source_type: "index_only",
        summary: Some("A model's summary of the lease"),
        ..Doc::vault("Remote lease")
    };
    let summarised = seed(
        &host,
        &remote,
        None,
        crate::ingest::INDEX_ONLY_BODY_PLACEHOLDER,
        "rent due",
        &vector(0, None),
    )
    .0;
    let local = seed(
        &host,
        &Doc::vault("Lease"),
        None,
        "rent is due",
        "rent due",
        &vector(0, Some(1)),
    )
    .0;
    let v = search(&host, &s, json!({"query": "rent"})).await;
    let hits = v["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 2);
    for hit in hits {
        let id = hit["documentId"].as_i64().unwrap();
        if id == summarised {
            // The snippet is the summary, and the hit says so.
            assert_eq!(hit["snippet"], "A model's summary of the lease");
            assert_eq!(hit["indexOnly"], true);
        } else {
            assert_eq!(id, local);
            assert_eq!(hit["snippet"], "rent is due");
            assert_eq!(hit["indexOnly"], false);
        }
    }
}

// --- what a hit tells ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_hit_has_only_the_allowed_fields() {
    let doc = Doc {
        project: "Alpha",
        ..Doc::vault("Lease")
    };
    for (perms, expect_project) in [
        (&["documents.text"][..], false),
        (&["documents.text", "projects.read"][..], true),
    ] {
        let (host, s) = searcher(perms);
        let id = seed(
            &host,
            &doc,
            Some("Rent"),
            "rent is due",
            "rent due",
            &vector(0, None),
        )
        .0;
        let v = search(&host, &s, json!({"query": "rent"})).await;
        let hit = &v["hits"][0];
        let mut want = BTreeSet::from(["documentId", "title", "heading", "snippet", "indexOnly"]);
        if expect_project {
            want.insert("project");
            assert_eq!(hit["project"], "Alpha");
        }
        let got: BTreeSet<&str> = hit
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(got, want, "{perms:?}");
        assert_eq!(hit["documentId"], id);
        assert_eq!(hit["heading"], "Rent");
        assert_eq!(hit["title"], "Lease");
        // Nothing that locates a file, a chunk, a conversation or a vector, and no score.
        let text = v.to_string();
        for leak in [
            "secret/",
            "source-path",
            "external-ref",
            "chunk",
            "conversation",
            "score",
        ] {
            assert!(!text.contains(leak), "{leak} leaked: {text}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_caps_results_at_20_and_snippets_at_1000() {
    let (host, s) = searcher(TEXT);
    // Thirty documents, each with a long multi-byte body (a cut in the middle of a character would
    // panic or mangle it).
    let body = "é".repeat(3000);
    for n in 0..30 {
        seed(
            &host,
            &Doc::vault(&format!("Doc {n}")),
            None,
            &body,
            "rent due",
            &vector(0, Some(1 + n % 7)),
        );
    }
    // The default page is ten; the most a page may be is twenty.
    let v = search(&host, &s, json!({"query": "rent"})).await;
    assert!(v["hits"].as_array().unwrap().len() <= 10);
    let v = search(&host, &s, json!({"query": "rent", "limit": 20})).await;
    let hits = v["hits"].as_array().unwrap();
    assert!(hits.len() > 10 && hits.len() <= 20, "{}", hits.len());
    for hit in hits {
        assert_eq!(hit["snippet"].as_str().unwrap().chars().count(), 1000);
    }
    // One bad call through the dispatcher (the embed family's few calls a minute are not for a loop of
    // them); the rest are the argument check itself.
    assert_eq!(
        code(
            call(
                &host,
                &s,
                "documents.search",
                json!({"query": "rent", "limit": 21})
            )
            .await
        ),
        Code::BadArgs
    );
    for args in [
        json!({"query": "rent", "limit": 21}),
        json!({"query": "rent", "limit": 0}),
        json!({"query": "rent", "limit": 1.5}),
        json!({"query": "rent", "limit": -1}),
        json!({"query": ""}),
        json!({"query": "   "}),
        json!({"query": "x".repeat(2001)}),
        json!({"query": 7}),
        json!({}),
        json!(null),
        json!(["rent"]),
        json!("rent"),
        json!({"query": "rent", "project": "Alpha"}),
    ] {
        assert_eq!(
            Query::parse(&args).unwrap_err().code,
            Code::BadArgs,
            "{args}"
        );
    }
    // The edges: 2000 characters, and a page of one or twenty.
    for args in [
        json!({"query": "x".repeat(2000)}),
        json!({"query": "rent", "limit": 1}),
        json!({"query": "rent", "limit": 20}),
    ] {
        Query::parse(&args).unwrap();
    }
    search(&host, &s, json!({"query": "x".repeat(2000)})).await;
}

/// A document's title and a chunk's heading are whatever a file or feed says: a hit cuts both, so a
/// page of them can't outgrow the result cap.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_cuts_a_long_title_and_heading() {
    let (host, s) = searcher(TEXT);
    let long = "é".repeat(40_000);
    seed(
        &host,
        &Doc::vault(&long),
        Some(&long),
        "rent is due",
        "rent due",
        &vector(0, None),
    );
    let v = search(&host, &s, json!({"query": "rent"})).await;
    let hit = &v["hits"][0];
    for field in ["title", "heading"] {
        assert_eq!(
            hit[field].as_str().unwrap().chars().count(),
            super::search_calls::LABEL_MAX_CHARS,
            "{field}"
        );
    }
}

// --- backfill, and the existing callers ----------------------------------------------------------

/// Thirty chunks a mod may not see (`hidden`, with `content`) sit nearer to the query than the five
/// documents it may: the page is still the five, because the filter runs over the wide pool and not
/// over a pool already cut to twenty. The hits' own placeholder check can't stand in for the
/// retrieval's filter here: if the filter let these through they would fill the pool, and the page
/// would come up short.
async fn assert_backfills_past(hidden: &Doc<'_>, content: &str) {
    let (host, s) = searcher(TEXT);
    for _ in 0..30 {
        seed(&host, hidden, None, content, "rent talk", &vector(0, None));
    }
    let mut want = Vec::new();
    for n in 0..5 {
        // Only the vector branch can find these: their words don't match the query.
        want.push(
            seed(
                &host,
                &Doc::vault(&format!("Doc {n}")),
                None,
                "a visible excerpt",
                "unrelated words",
                &vector(0, Some(10 + n)),
            )
            .0,
        );
    }
    let mut found = ids(&search(&host, &s, json!({"query": "rent", "limit": 5})).await);
    found.sort_unstable();
    assert_eq!(found, want);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_backfills_past_filtered_chunks() {
    let chat = Doc {
        source_type: "chat",
        ..Doc::vault("Chat")
    };
    assert_backfills_past(&chat, "rent talk").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_backfills_past_archived_chunks() {
    let archived = Doc {
        importance: Some("archive"),
        ..Doc::vault("Old")
    };
    assert_backfills_past(&archived, "rent old").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_backfills_past_summaryless_index_only_chunks() {
    let bare = Doc {
        source_type: "index_only",
        ..Doc::vault("Remote file")
    };
    assert_backfills_past(&bare, crate::ingest::INDEX_ONLY_BODY_PLACEHOLDER).await;
}

/// Without `mod_visible` retrieval is what it always was: a chat chunk, an archived one and a
/// placeholder are all still found by the callers that are not a mod.
#[test]
fn chat_retrieval_is_unchanged_without_mod_visible() {
    let host = TestHost::new();
    let chat = Doc {
        source_type: "chat",
        ..Doc::vault("Chat")
    };
    let archived = Doc {
        importance: Some("archive"),
        ..Doc::vault("Old")
    };
    let remote = Doc {
        source_type: "index_only",
        ..Doc::vault("Remote")
    };
    let all = [
        seed(
            &host,
            &chat,
            None,
            "rent talk",
            "rent talk",
            &vector(0, None),
        )
        .0,
        seed(
            &host,
            &archived,
            None,
            "rent old",
            "rent talk",
            &vector(0, None),
        )
        .0,
        seed(
            &host,
            &remote,
            None,
            crate::ingest::INDEX_ONLY_BODY_PLACEHOLDER,
            "rent talk",
            &vector(0, None),
        )
        .0,
    ];
    let query = |mod_visible| {
        let q = crate::retrieval::RetrieveQuery {
            text: "rent",
            embedding: &vector(0, None),
            k: 6,
            filters: crate::retrieval::Filters {
                mod_visible,
                ..Default::default()
            },
            strategy: crate::retrieval::Strategy::HybridRrf,
            multilingual: false,
        };
        let mut found: Vec<i64> = host
            .sql(|c| crate::retrieval::retrieve_fused(c, &q))
            .unwrap()
            .iter()
            .map(|c| c.document_id)
            .collect();
        found.sort_unstable();
        found
    };
    assert_eq!(query(false), all);
    assert!(query(true).is_empty());
}

/// The retrieval's archive test and the mods API's own (`documents.list`) are one rule.
#[test]
fn the_archive_shelf_is_the_one_the_list_leaves_out() {
    assert_eq!(super::documents::ARCHIVE, "archive");
    assert!(super::documents::LISTED.contains("'archive'"));
}
