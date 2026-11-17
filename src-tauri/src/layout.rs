// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The semantic memory map's backend: project every document's averaged embedding to 2-D so the Map
//! can lay documents out by meaning ("nearby = similar"), alongside the existing by-project layout.
//!
//! The work is a non-retrieval consumer of the embedding layer. It reads the vector dimension from
//! the registry (no magic 384), so it keeps working across embedder swaps and multilingual vaults.
//!
//! Shape, mirroring the Drive-sync seams:
//!   - The 2-D coordinates are cached in `doc_layout` and invalidated on a fingerprint
//!     (embedder / dim / node-cap / document-set / t-SNE-availability). The cache spares re-reading
//!     and averaging every leaf-chunk vector on each Map open.
//!   - [`precompute_semantic_layout`] recomputes when stale. It is single-flight, kicked off at idle
//!     priority after unlock ([`start_semantic_layout`]), and **defers to any active connector sync**
//!     so it never contends with ingest. Opening the Map calls [`prioritise_semantic_layout`], which
//!     jumps that queue. Progress rides a global `layout://progress` event, like the Drive sync.
//!   - The reducer is **PCA by default** (numpy-only, in the sidecar) and **t-SNE when the optional
//!     component is installed** ([`install_optional_tsne`]). The reducer never holds the DB lock.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::doc_vectors::{
    capped_document_vectors, doc_set_hash, doc_signatures, DEFAULT_NODE_CAP, NODE_CAP_MAX,
    NODE_CAP_MIN,
};
// The uncapped read, for the tests below: it is the reference the capped read is held to.
#[cfg(test)]
use crate::doc_vectors::document_vectors;
use crate::error::{Error, Result};
use crate::{db, AppState};

/// Bump to force a one-time recompute of every cached layout after a tuning change (perplexity,
/// init, scaling) without touching user data.
const LAYOUT_VERSION: u32 = 1;

// The default cap on individually-projected nodes, and the range the user may adjust it within
// (`DEFAULT_NODE_CAP`, `NODE_CAP_MIN`, `NODE_CAP_MAX`), are in `doc_vectors.rs` with the cap itself.

/// `settings` key holding the layout fingerprint + the method actually used (see [`LayoutMeta`]).
const META_KEY: &str = "doc_layout_meta";
/// `settings` key for the webview-writable Map preferences blob (JSON `{ nodeCap?: number }`).
const MAP_PREF_KEY: &str = "map";

// ---- shared state ---------------------------------------------------------

/// A snapshot of the layout precompute job: the reducer that last ran and the last error, so the Map
/// can say what produced the cached coordinates and surface a failure.
///
/// Deliberately does NOT hold the single-flight flag. That lives on `AppState::layout_busy` and is
/// claimed with [`crate::BusyGuard`] — see the claim in [`precompute_semantic_layout`].
#[derive(Default, Clone, Serialize)]
pub struct LayoutJobState {
    pub method: Option<String>,
    pub error: Option<String>,
}

/// Global progress event (broadcast on `layout://progress`), mirroring the Drive sync's pattern so
/// progress reaches the Map regardless of which view started the job.
#[derive(Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LayoutProgressEvent {
    /// Reading + averaging the document vectors.
    Preparing,
    /// Handing the vectors to the reducer.
    Reducing {
        count: usize,
        method: String,
    },
    /// Stepped aside for an active connector sync (idle priority); will run when the Map is opened.
    Deferred,
    /// A fresh layout has been cached.
    Done {
        method: String,
    },
    Error {
        message: String,
    },
}

fn emit(app: &AppHandle, ev: LayoutProgressEvent) {
    let _ = app.emit("layout://progress", ev);
}

/// Mutate the shared job snapshot, best-effort. Binds the lock guard to a named local first to
/// sidestep the `if let` temporary-lifetime pitfall (same pattern as `with_drive_snap`).
fn with_job(app: &AppHandle, f: impl FnOnce(&mut LayoutJobState)) {
    let state = app.state::<AppState>();
    let guard = state.layout_job.lock();
    if let Ok(mut job) = guard {
        f(&mut job);
    }
}

// ---- fingerprint + cache types -------------------------------------------

/// Everything that, if changed, makes a cached layout stale.
#[derive(Serialize, Deserialize, PartialEq)]
struct Fingerprint {
    /// Whether t-SNE is the *effective* reducer (installed AND enabled by the user) — flips the
    /// reducer, so it invalidates. (Renamed from `tsne_available`; an older stored fingerprint simply
    /// fails to deserialize and triggers one recompute.)
    tsne: bool,
    embedder: String,
    dim: usize,
    node_cap: usize,
    layout_version: u32,
    /// Hash over the set of documents-with-vectors, their leaf-chunk counts, and a leaf-identity
    /// marker (max leaf rowid) so a same-count content edit still invalidates the cache (B4-1).
    doc_set_hash: String,
}

/// What's stored under [`META_KEY`]: the fingerprint plus the reducer that actually ran (which may be
/// `pca` even when t-SNE was requested but unavailable). Freshness compares the fingerprint; the Map
/// labels itself by `used_method`.
#[derive(Serialize, Deserialize)]
struct LayoutMeta {
    fingerprint: Fingerprint,
    used_method: String,
}

// ---- command return types -------------------------------------------------

#[derive(Serialize)]
pub struct Coord {
    id: i64,
    x: f32,
    y: f32,
}

#[derive(Serialize)]
pub struct SemanticLayout {
    /// The reducer that produced the cached coords (`pca` | `tsne` | `none` when nothing is cached).
    method: String,
    coords: Vec<Coord>,
    /// A recompute is in flight — the Map shows a progress strip and re-fetches when it lands.
    computing: bool,
}

#[derive(Serialize)]
pub struct TsneStatus {
    installed: bool,
}

// The vector assembly (decode, average, normalise, the node cap) and the freshness signature live in
// `doc_vectors.rs`, shared with the mod API's `embeddings.*` calls.

/// The `map` webview-writable pref blob (`{ nodeCap?, tsneEnabled? }`), or `None` if unset/malformed.
fn read_map_pref(conn: &Connection) -> Option<serde_json::Value> {
    db::get_setting(conn, MAP_PREF_KEY)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
}

/// The user-chosen node cap, clamped to the supported range; the default when unset.
fn read_node_cap(conn: &Connection) -> usize {
    read_map_pref(conn)
        .and_then(|v| v.get("nodeCap").and_then(|n| n.as_u64()))
        .map(|n| (n as usize).clamp(NODE_CAP_MIN, NODE_CAP_MAX))
        .unwrap_or(DEFAULT_NODE_CAP)
}

/// Whether the user has the enhanced (t-SNE) layout *enabled* — separate from whether it's installed.
/// Default true, so an existing install keeps using t-SNE; toggling it off uses PCA without
/// uninstalling. Part of the layout fingerprint via [`Fingerprint::tsne`], so a change recomputes.
fn read_tsne_enabled(conn: &Connection) -> bool {
    read_map_pref(conn)
        .and_then(|v| v.get("tsneEnabled").and_then(|b| b.as_bool()))
        .unwrap_or(true)
}

// ---- precompute -----------------------------------------------------------

/// Recompute the semantic layout if stale. Single-flight (folds a concurrent request into the running
/// one). `force_recompute` ignores the freshness check; `ignore_sync` runs even while a connector sync
/// is in flight (the Map-open path) rather than deferring to it (the idle launch path).
pub async fn precompute_semantic_layout(
    app: &AppHandle,
    force_recompute: bool,
    ignore_sync: bool,
) -> Result<()> {
    // Claim the single-flight slot, or fold this request into the running pass. RAII, never a
    // hand-cleared bool — F-43 is the repo's own rule about exactly this shape (docs/DECISIONS.md,
    // "2026-07-05 — Audit-03 remediation, Wave 2 PR 2 (X-D1a): the sync single-flight becomes RAII"):
    //
    //   "hand-rolled single-flight with a `running` bool: set `true` at claim, back to `false` at the
    //    tail of the rerun loop. A panic (or any early return) mid-pass skips that tail, so `running`
    //    stays `true` for the rest of the session — after which every later sync sees 'already
    //    running' and folds into a follow-up sweep that never comes."
    //
    // The layout's wedge is the same sentence with the Map in place of the connector: it would serve
    // the stale cached coordinates forever, with the spinner stuck on, until an app restart.
    // `BusyGuard` clears the flag on drop however the pass ends — an unwinding panic, or this future
    // being dropped at the await below — and an `AtomicBool` cannot poison, so the poisoned-mutex `?`
    // that used to turn one bad lock into a permanent `Err` for every later precompute is gone with
    // it. `&AppState` (not the `State` temporary) so the guard's borrow lives across the await.
    let state: &AppState = app.state::<AppState>().inner();
    let Some(_busy) = crate::BusyGuard::acquire(&state.layout_busy) else {
        return Ok(());
    };
    with_job(app, |job| job.error = None);

    let outcome = run_precompute(app, force_recompute, ignore_sync).await;

    if let Err(e) = &outcome {
        with_job(app, |job| job.error = Some(e.to_string()));
        emit(
            app,
            LayoutProgressEvent::Error {
                message: e.to_string(),
            },
        );
    }
    outcome
}

async fn run_precompute(app: &AppHandle, force_recompute: bool, ignore_sync: bool) -> Result<()> {
    let tsne_available = app.state::<AppState>().sidecar.optional_tsne_ready();

    // 1) Cheap freshness check under a short lock — no vector blobs read.
    let (fingerprint, doc_count, fresh) = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let dim = db::vec0_dim(&conn)?;
        let embedder = db::selected_embedder(&conn)?.id.to_string();
        let node_cap = read_node_cap(&conn);
        // t-SNE is the effective reducer only when it's installed AND the user has it enabled.
        let use_tsne = tsne_available && read_tsne_enabled(&conn);
        let sigs = doc_signatures(&conn)?;
        let fp = Fingerprint {
            tsne: use_tsne,
            embedder,
            dim,
            node_cap,
            layout_version: LAYOUT_VERSION,
            doc_set_hash: doc_set_hash(&sigs),
        };
        let stored: Option<LayoutMeta> =
            db::get_setting(&conn, META_KEY)?.and_then(|s| serde_json::from_str(&s).ok());
        let fresh = stored
            .as_ref()
            .map(|m| m.fingerprint == fp)
            .unwrap_or(false);
        (fp, sigs.len(), fresh)
    };

    if doc_count == 0 || (fresh && !force_recompute) {
        return Ok(());
    }

    // 2) Idle priority: step aside for an active connector sync unless the user opened the Map. The
    //    read goes through `sync_active` so the Map defers to the same slot register every other
    //    background job does — the gate hand-rolled here read only the Drive slot, so a OneDrive or
    //    local-folder sync never held the precompute back (audit B1-7).
    if !ignore_sync && app.state::<AppState>().sync_active() {
        emit(app, LayoutProgressEvent::Deferred);
        return Ok(());
    }

    emit(app, LayoutProgressEvent::Preparing);

    // 3) Read + average the vectors (the expensive part) and apply the node cap, under a short lock.
    let (ids, vectors) = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        // The node cap keeps the most important + most substantial documents (the rest fall to their
        // project centroid on the frontend, still shown, grouped, without paying the reducer for the
        // tail); only the kept documents' vectors are read.
        let docs = capped_document_vectors(&conn, None, fingerprint.node_cap)?;
        let ids: Vec<i64> = docs.iter().map(|d| d.doc_id).collect();
        let vectors: Vec<Vec<f32>> = docs.into_iter().map(|d| d.vec).collect();
        (ids, vectors)
    };
    if ids.is_empty() {
        return Ok(());
    }

    let requested = if fingerprint.tsne { "tsne" } else { "pca" };
    emit(
        app,
        LayoutProgressEvent::Reducing {
            count: ids.len(),
            method: requested.to_string(),
        },
    );

    // 4) Reduce off the async runtime (the sidecar call blocks).
    let app2 = app.clone();
    let method_owned = requested.to_string();
    let vectors_owned = vectors;
    let (coords, used_method) = tauri::async_runtime::spawn_blocking(move || {
        app2.state::<AppState>()
            .sidecar
            .reduce(&vectors_owned, &method_owned)
    })
    .await
    .map_err(|e| Error::Other(format!("layout reduce task panicked: {e}")))??;

    if coords.len() != ids.len() {
        return Err(Error::Other(format!(
            "reducer returned {} points for {} documents",
            coords.len(),
            ids.len()
        )));
    }

    // 5) Replace the cache + fingerprint in one transaction.
    {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM doc_layout", [])?;
        {
            let mut ins = tx.prepare(
                "INSERT OR REPLACE INTO doc_layout(document_id, method, x, y) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (id, c) in ids.iter().zip(coords.iter()) {
                ins.execute(params![id, used_method, c[0], c[1]])?;
            }
        }
        let meta = LayoutMeta {
            fingerprint,
            used_method: used_method.clone(),
        };
        let meta_json = serde_json::to_string(&meta).map_err(|e| Error::Other(e.to_string()))?;
        tx.execute(
            "INSERT INTO settings(key, value) VALUES(?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![META_KEY, meta_json],
        )?;
        tx.commit()?;
    }

    with_job(app, |job| job.method = Some(used_method.clone()));
    emit(
        app,
        LayoutProgressEvent::Done {
            method: used_method,
        },
    );
    Ok(())
}

// ---- commands -------------------------------------------------------------

/// The cached semantic layout: the coordinates and the reducer that produced them, plus whether a
/// recompute is in flight. Always returns immediately (stale-but-cached); recompute is the job's role.
#[tauri::command]
pub fn semantic_layout(state: State<'_, AppState>) -> Result<SemanticLayout> {
    let conn = state.conn()?;
    let method = db::get_setting(&conn, META_KEY)?
        .and_then(|s| serde_json::from_str::<LayoutMeta>(&s).ok())
        .map(|m| m.used_method)
        .unwrap_or_else(|| "none".to_string());
    let mut stmt = conn.prepare("SELECT document_id, x, y FROM doc_layout")?;
    let coords = stmt
        .query_map([], |row| {
            Ok(Coord {
                id: row.get(0)?,
                x: row.get(1)?,
                y: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<Coord>>>()?;
    let computing = state.layout_busy.load(std::sync::atomic::Ordering::SeqCst);
    Ok(SemanticLayout {
        method,
        coords,
        computing,
    })
}

/// Kick off the background layout precompute after unlock — detached, idle priority, defers to a
/// running connector sync. Mirrors `resume_drive_sync`'s fire-and-forget shape.
#[tauri::command]
pub fn start_semantic_layout(app: AppHandle) -> Result<bool> {
    tauri::async_runtime::spawn(async move {
        // Idle priority: defer to any active connector sync (ignore_sync = false).
        let _ = precompute_semantic_layout(&app, false, false).await;
    });
    Ok(true)
}

/// The Map calls this when opened in semantic mode: recompute if stale, jumping ahead of a connector
/// sync (the user is looking at the Map now). A no-op when the cache is already fresh.
#[tauri::command]
pub async fn prioritise_semantic_layout(app: AppHandle) -> Result<()> {
    precompute_semantic_layout(&app, false, true).await
}

// ---- "by project" layout cache --------------------------------------------
//
// The by-project arrangement is computed in the WEBVIEW (d3-force on a worker), unlike the semantic
// one which is computed here. So the backend's only job is storage: it keeps the frontend's result
// verbatim, as an opaque JSON string, and never interprets it. It lives in the encrypted store
// rather than localStorage because coordinates keyed by document id are user data.
//
// Deliberately a single `settings` blob rather than rows in `doc_layout`: the force layout also
// contains project HUB nodes, which are not documents and have no `documents(id)` to key on. One
// blob keeps hubs and documents together at exact fidelity, needs no migration, and the frontend's
// fingerprint invalidates the whole thing at once — so a deleted document cannot leave a stale
// coordinate behind.

/// `settings` key holding the frontend's cached by-project layout (fingerprint + positions).
const PROJECT_LAYOUT_KEY: &str = "project_layout_cache";

/// Refuse to store an absurd payload. A 1,000-node layout is a few tens of KB; this is far above any
/// real map and exists so a frontend bug can't grow the settings row without bound.
const PROJECT_LAYOUT_MAX_BYTES: usize = 4 * 1024 * 1024;

/// The cached by-project layout as the frontend last wrote it, or `None` when nothing is stored.
#[tauri::command]
pub fn project_layout(state: State<'_, AppState>) -> Result<Option<String>> {
    let conn = state.conn()?;
    db::get_setting(&conn, PROJECT_LAYOUT_KEY)
}

/// Store the by-project layout the frontend just computed, so the next launch repaints instead of
/// re-running the simulation. Passing `None` clears it.
#[tauri::command]
pub fn set_project_layout(state: State<'_, AppState>, cache: Option<String>) -> Result<()> {
    let conn = state.conn()?;
    match cache {
        Some(json) => {
            if json.len() > PROJECT_LAYOUT_MAX_BYTES {
                return Err(Error::Other("map layout cache too large".into()));
            }
            db::set_setting(&conn, PROJECT_LAYOUT_KEY, &json)
        }
        None => db::delete_setting(&conn, PROJECT_LAYOUT_KEY),
    }
}

/// Whether the optional t-SNE reducer is installed.
#[tauri::command]
pub fn optional_tsne_status(state: State<'_, AppState>) -> Result<TsneStatus> {
    Ok(TsneStatus {
        installed: state.sidecar.optional_tsne_ready(),
    })
}

/// Install the optional t-SNE reducer on demand (a pip download into the managed venv), then recompute
/// the layout with it in the background. Blocking install runs off the async runtime; errors surface
/// to the caller so Settings can show them.
#[tauri::command]
pub async fn install_optional_tsne(app: AppHandle) -> Result<()> {
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let progress_app = app2.clone();
        app2.state::<AppState>()
            .sidecar
            .install_optional_tsne(move |fraction| {
                crate::commands::emit_install_progress(&progress_app, "tsne", fraction);
            })
    })
    .await
    .map_err(|e| Error::Other(format!("t-SNE install task panicked: {e}")))??;

    // Now that t-SNE is available, refresh the layout with it (force, and don't defer).
    let app3 = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = precompute_semantic_layout(&app3, true, true).await;
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use std::sync::atomic::{AtomicBool, Ordering};

    const KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    /// The F-43 property `precompute_semantic_layout` now leans on, pinned on the flag it claims.
    ///
    /// `precompute_semantic_layout` itself needs an `AppHandle`, so it cannot be called from a unit
    /// test; what IS testable is the contract the claim depends on, in the shape the precompute uses
    /// it — claim, bail out early through a `?`, and the slot must be open for the next pass. Before
    /// this, the clear was a hand-written `job.running = false` after the awaited body, and anything
    /// that skipped it (a panic, this future being dropped, a poisoned `layout_job`) left the Map
    /// serving stale coordinates with the spinner on until an app restart.
    #[test]
    fn the_layout_slot_reopens_after_a_pass_that_returns_early() {
        // Stands in for `precompute_semantic_layout`: claim, then leave by an error path that never
        // reaches the tail of the function.
        fn guarded_pass(flag: &AtomicBool, fail: bool) -> Result<&'static str> {
            let Some(_busy) = crate::BusyGuard::acquire(flag) else {
                return Ok("folded into the running pass");
            };
            if fail {
                // Every `?` inside `run_precompute` (a locked store, a dead reducer, a failed
                // transaction) lands here, as does the `Err` for a coord/id count mismatch.
                return Err(Error::Other("the reducer failed".into()));
            }
            Ok("recomputed")
        }

        let layout_busy = AtomicBool::new(false);

        assert!(guarded_pass(&layout_busy, true).is_err(), "the pass failed");
        assert!(
            !layout_busy.load(Ordering::SeqCst),
            "an early return must still release the slot — a stuck flag wedges the Map for the session"
        );
        assert_eq!(
            guarded_pass(&layout_busy, false).unwrap(),
            "recomputed",
            "and the NEXT open must recompute rather than serve the stale cache"
        );
        assert!(!layout_busy.load(Ordering::SeqCst), "released again");

        // The other half of the contract: while a pass really is running, a second request folds
        // into it instead of stacking a second reducer run.
        let held = crate::BusyGuard::acquire(&layout_busy).expect("an idle slot admits the claim");
        assert_eq!(
            guarded_pass(&layout_busy, false).unwrap(),
            "folded into the running pass"
        );
        drop(held);
        assert!(!layout_busy.load(Ordering::SeqCst));
    }

    /// Proves the whole vector-assembly path against a real sqlite-vec store: a document's leaf-chunk
    /// embeddings decode (whatever wire form the vector column returns), average per document, and
    /// L2-normalise — plus the leaf count and importance rank the node cap ranks on.
    #[test]
    fn document_vectors_decodes_averages_and_normalises_leaf_embeddings() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open(&dir.path().join("pm.sqlite"), KEY).unwrap();

        conn.execute(
            "INSERT INTO documents(vault_path, title, content_hash, project, importance) \
             VALUES ('a.md','A','ha','PM','high')",
            [],
        )
        .unwrap();
        let doc_id = conn.last_insert_rowid();

        // Two leaf chunks whose 384-d vectors are all-0.0 and all-0.2: the per-document mean is 0.1
        // in every dimension, which L2-normalises to 1/sqrt(384) per component.
        let dim = db::vec0_dim(&conn).unwrap();
        let make_vec = |v: f64| format!("[{}]", vec![v.to_string(); dim].join(","));
        let mut max_chunk_id = 0i64;
        for (ordinal, val) in [(0i64, 0.0f64), (1, 0.2)] {
            conn.execute(
                "INSERT INTO chunks(document_id, ordinal, content, char_count, kind) \
                 VALUES (?1, ?2, 'c', 1, 'leaf')",
                params![doc_id, ordinal],
            )
            .unwrap();
            let chunk_id = conn.last_insert_rowid();
            max_chunk_id = chunk_id;
            conn.execute(
                "INSERT INTO chunk_vec(rowid, embedding) VALUES (?1, ?2)",
                params![chunk_id, make_vec(val)],
            )
            .unwrap();
        }

        let docs = document_vectors(&conn).unwrap();
        assert_eq!(docs.len(), 1, "one document with vectors");
        let d = &docs[0];
        assert_eq!(d.doc_id, doc_id);
        assert_eq!(d.leaf_count, 2);
        assert_eq!(
            d.importance_rank, 3,
            "'high' ranks highest for the node cap"
        );
        assert_eq!(d.vec.len(), dim);
        let expected = 1.0f32 / (dim as f32).sqrt();
        for x in &d.vec {
            assert!(
                (x - expected).abs() < 1e-5,
                "normalised component {x} ~= {expected}"
            );
        }

        // The cheap signature path sees the same single document with two leaves; the third element is
        // the max leaf rowid (B4-1's re-ingest marker).
        let sigs = doc_signatures(&conn).unwrap();
        assert_eq!(sigs, vec![(doc_id, 2, max_chunk_id)]);
    }

    /// Archive-as-outcome (card F): an archived document is hidden from the Map — absent from both the
    /// vector assembly and the freshness signature — while an active document stays. Applies to any source.
    #[test]
    fn archived_documents_are_excluded_from_the_map() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open(&dir.path().join("pm.sqlite"), KEY).unwrap();
        let dim = db::vec0_dim(&conn).unwrap();
        let make_vec = |v: f64| format!("[{}]", vec![v.to_string(); dim].join(","));

        // One active document, one archived — each with a single leaf chunk + vector. Returns
        // (doc_id, leaf_chunk_id) so the signature assert can name the max leaf rowid.
        let add = |vault: &str, importance: &str, val: f64| -> (i64, i64) {
            conn.execute(
                "INSERT INTO documents(vault_path, title, content_hash, project, importance) \
                 VALUES (?1, 'T', ?1, 'PM', ?2)",
                params![vault, importance],
            )
            .unwrap();
            let doc_id = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO chunks(document_id, ordinal, content, char_count, kind) \
                 VALUES (?1, 0, 'c', 1, 'leaf')",
                params![doc_id],
            )
            .unwrap();
            let chunk_id = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO chunk_vec(rowid, embedding) VALUES (?1, ?2)",
                params![chunk_id, make_vec(val)],
            )
            .unwrap();
            (doc_id, chunk_id)
        };
        let (active, active_leaf) = add("active.md", "high", 0.1);
        add("archived.md", "archive", 0.2);

        let docs = document_vectors(&conn).unwrap();
        assert_eq!(docs.len(), 1, "only the active document is a node");
        assert_eq!(docs[0].doc_id, active);

        let sigs = doc_signatures(&conn).unwrap();
        assert_eq!(
            sigs,
            vec![(active, 1, active_leaf)],
            "archived doc absent from the signature too"
        );
    }

    /// B4-1: a content edit that re-chunks a document to the SAME number of leaves must still
    /// invalidate the map cache. `(document_id, leaf_count)` alone missed it — the document id and the
    /// leaf count are both unchanged — so the map kept placing the document by its old meaning. The
    /// max-leaf-rowid marker catches it because re-ingest (`replace_chunks`) DELETEs then re-INSERTs the
    /// chunks with fresh `AUTOINCREMENT` rowids, so the max leaf id strictly rises.
    #[test]
    fn same_leaf_count_content_edit_changes_the_doc_set_hash() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open(&dir.path().join("pm.sqlite"), KEY).unwrap();
        let dim = db::vec0_dim(&conn).unwrap();
        let make_vec = |v: f64| format!("[{}]", vec![v.to_string(); dim].join(","));

        conn.execute(
            "INSERT INTO documents(vault_path, title, content_hash, project, importance) \
             VALUES ('a.md','A','h1','PM','high')",
            [],
        )
        .unwrap();
        let doc_id = conn.last_insert_rowid();

        // Re-index the document with `n` fresh leaf chunks (+ vectors), exactly as `replace_chunks`
        // does — delete the old chunk_vec/chunks rows, then insert new ones with new AUTOINCREMENT ids.
        // Returns the resulting doc-set hash.
        let reindex = |n: i64, val: f64| -> String {
            conn.execute(
                "DELETE FROM chunk_vec WHERE rowid IN (SELECT id FROM chunks WHERE document_id = ?1)",
                params![doc_id],
            )
            .unwrap();
            conn.execute("DELETE FROM chunks WHERE document_id = ?1", params![doc_id])
                .unwrap();
            for ordinal in 0..n {
                conn.execute(
                    "INSERT INTO chunks(document_id, ordinal, content, char_count, kind) \
                     VALUES (?1, ?2, 'c', 1, 'leaf')",
                    params![doc_id, ordinal],
                )
                .unwrap();
                let chunk_id = conn.last_insert_rowid();
                conn.execute(
                    "INSERT INTO chunk_vec(rowid, embedding) VALUES (?1, ?2)",
                    params![chunk_id, make_vec(val)],
                )
                .unwrap();
            }
            doc_set_hash(&doc_signatures(&conn).unwrap())
        };

        let before = reindex(2, 0.1);
        // Re-ingest at the SAME leaf count (2) — only the chunk ids and vectors change.
        let after = reindex(2, 0.9);
        assert_ne!(
            before, after,
            "a same-leaf-count content edit must change the signature (B4-1)"
        );

        // Confirm it really was a constant-leaf-count edit: the count stayed 2, so it was the
        // max-rowid marker — not the leaf count — that flipped the hash.
        let sigs = doc_signatures(&conn).unwrap();
        assert_eq!(sigs.len(), 1);
        assert_eq!(sigs[0].1, 2, "leaf count unchanged across the edit");
    }
}
