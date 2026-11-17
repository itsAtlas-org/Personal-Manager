// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The cost logger and the spend summary it feeds.

use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::error::Result;
use crate::llm_gateway;
use crate::{cost, openrouter, AppState};

// --- cost logger (spec §11.2 / §17.1) ---

/// Spend for one model over a window. `cost_usd` is `None` when the model isn't in
/// the price cache yet — surfaced as "—", never an understated $0.
#[derive(Serialize)]
pub struct ModelSpend {
    pub model: String,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub request_count: i64,
    pub cost_usd: Option<f64>,
}

/// Spend for one mod, all time. `name` is `None` once the mod is uninstalled (its rows stay).
/// `cost_usd` is `None` when nothing it spent could be priced, never an understated $0.
#[derive(Serialize)]
pub struct ModAiSpend {
    pub mod_id: String,
    pub name: Option<String>,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub request_count: i64,
    pub cost_usd: Option<f64>,
}

/// The Settings "Usage & cost" payload: per-model spend over two windows + totals,
/// plus when the cached pricing was last refreshed.
#[derive(Serialize)]
pub struct CostSummary {
    pub last_30d: Vec<ModelSpend>,
    pub all_time: Vec<ModelSpend>,
    /// Per-mod spend (a mod's calls are also inside the two lists above, by model).
    pub by_mod: Vec<ModAiSpend>,
    pub total_30d_usd: Option<f64>,
    pub total_all_time_usd: Option<f64>,
    pub pricing_updated_at: Option<String>,
}

/// Per-model spend (trailing 30 days + all time) joined against the cached OpenRouter
/// prices. CHECK-ON-READ: if an OpenRouter key is stored and the price cache is empty or older
/// than a day, refresh it from the public catalogue first (the request itself needs no key; no
/// model call, no scheduler — mirrors the briefing's staleness rule). Without a key nothing is
/// fetched: see [`ensure_pricing_fresh`]. Read-mostly; safe on every Settings open.
#[tauri::command]
pub async fn cost_summary(app: AppHandle) -> Result<CostSummary> {
    // Best-effort refresh: if it fails (offline, etc.) still return the summary —
    // token counts come from the local log and need no network; only the priced
    // costs fall back to "unknown". The explicit "Refresh prices" button surfaces
    // the error instead.
    let _ = ensure_pricing_fresh(&app).await;
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    build_cost_summary(&conn)
}

/// Force a re-pull of OpenRouter's public pricing into the cache, then return the
/// refreshed summary (the Settings "Refresh prices" action).
#[tauri::command]
pub async fn refresh_pricing(app: AppHandle) -> Result<CostSummary> {
    refresh_pricing_now(&app).await?;
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    build_cost_summary(&conn)
}

/// Reconstruct a view of the catalogue from the daily price/signal cache (`model_pricing`,
/// extended in migration v8). Reading from the cache — not a live fetch — is what lets the
/// chat context meter work offline. Only the **latest refresh batch** is in scope
/// (`fetched_at = MAX(fetched_at)`): a model that has left OpenRouter keeps an older
/// timestamp and is excluded. (The cost-summary join reads `model_pricing` unfiltered, so
/// historical spend on a now-removed model is still priced.)
///
/// Note this cache is **not** ZDR-filtered — that filter lives in `openrouter::list_models`,
/// on the picker. This feeds the context meter, which only needs a window size for a model
/// the user already has selected.
pub(super) fn cached_catalogue(conn: &Connection) -> Result<Vec<openrouter::ModelDetail>> {
    let mut stmt = conn.prepare(
        "SELECT model, COALESCE(name, ''), context_length, prompt_price, completion_price, \
                cache_read_price, supported_parameters, input_modalities, intelligence_index \
         FROM model_pricing \
         WHERE fetched_at = (SELECT MAX(fetched_at) FROM model_pricing)",
    )?;
    let rows = stmt
        .query_map([], |r| {
            let context_length: Option<i64> = r.get(2)?;
            let supported: Option<String> = r.get(6)?;
            let modalities: Option<String> = r.get(7)?;
            Ok(openrouter::ModelDetail {
                id: r.get(0)?,
                name: r.get(1)?,
                description: String::new(),
                context_length: context_length.map(|v| v as u64),
                prompt_price: r.get(3)?,
                completion_price: r.get(4)?,
                cache_read_price: r.get(5)?,
                input_modalities: modalities
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default(),
                supported_parameters: supported
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default(),
                intelligence_index: r.get(8)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Append a `usage_log` row — best-effort: cost logging must never fail a model call,
/// so errors are swallowed. `model = None` is allowed (an unreported served model). `meta` tags the
/// row with how it was served (provider / latency / fallback reason, migration v37) so a reader can
/// tell local from cloud spend and a power-policy route from a failure. The dev inspector shows
/// them; the Usage & cost table does not yet. `pub(crate)` so the chat housekeeping modules
/// (summary / title / prefs) route their rows through it too.
pub(crate) fn log_usage(
    conn: &Connection,
    kind: &str,
    model: Option<&str>,
    usage: &openrouter::Usage,
    meta: &llm_gateway::CallMeta,
) {
    // PM's own calls: no mod. Every existing caller goes through here, so I-11's one writer holds.
    log_usage_as(conn, kind, model, usage, meta, None);
}

/// [`log_usage`] for a row that may be a mod's: `mod_id` is the mod the spend was for (`mods::ai`),
/// `None` for PM's own calls. The one INSERT, so a mod's row is written by the same code as any
/// other and no new `kind` is needed (a mod's calls are `background`, told apart by this column).
pub(crate) fn log_usage_as(
    conn: &Connection,
    kind: &str,
    model: Option<&str>,
    usage: &openrouter::Usage,
    meta: &llm_gateway::CallMeta,
    mod_id: Option<&str>,
) {
    // One row, tagged with how it was served (provider / latency / fallback, the v37 columns).
    let fallback = meta.fallback.as_ref().map(|f| f.as_log_str());
    // Best-effort: accounting must NEVER fail a model call, so we don't propagate the error. But we do
    // NOT swallow it silently — a rejected insert here almost always means a schema mismatch (a store
    // missing the v37 columns), the exact class of bug v36 hid for months by pairing a rejecting CHECK
    // with a silent `let _ =`. Surface it at once so a mismatch shows up in seconds, not as months of
    // missing cost data.
    if let Err(e) = conn.execute(
        "INSERT INTO usage_log(model, kind, prompt_tokens, completion_tokens, cost_usd, \
         provider, latency_ms, fallback_reason, mod_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            model,
            kind,
            usage.prompt_tokens,
            usage.completion_tokens,
            usage.cost,
            meta.provider.as_str(),
            meta.latency_ms as i64,
            fallback,
            mod_id
        ],
    ) {
        eprintln!(
            "usage_log: could not record a '{kind}' usage row — cost/usage accounting will be \
             incomplete ({e})"
        );
    }
}

/// Write collected background usage rows under one short lock (best-effort), each attributed to its
/// served model (or the requested primary when none was reported) and tagged with how it was served.
pub(super) fn log_background_usage(
    app: &AppHandle,
    models: &[String],
    rows: &[(Option<String>, openrouter::Usage, llm_gateway::CallMeta)],
) {
    if rows.is_empty() {
        return;
    }
    let state = app.state::<AppState>();
    let Ok(conn) = state.conn() else { return };
    for (served, usage, meta) in rows {
        let model = served
            .as_deref()
            .or_else(|| models.first().map(String::as_str));
        log_usage(&conn, "background", model, usage, meta);
    }
}

/// Refresh the cached pricing when it's stale (check-on-read), for a user who has chosen OpenRouter.
/// Resolves staleness under a short lock, then does the network fetch + upsert without holding it
/// (rule #4).
///
/// Only with an OpenRouter key stored (the main one or a background-only one): opening Settings →
/// AI & Models must not contact openrouter.ai for someone who never set it up, a local-AI-only user
/// above all. Such a user reaches it only by pressing "Refresh prices" or opening the model
/// picker. A keychain that can't be read counts as no key, so the check fails closed.
async fn ensure_pricing_fresh(app: &AppHandle) -> Result<()> {
    if !uses_openrouter(&crate::secrets::get_background_or_primary_key()) {
        return Ok(());
    }
    let stale = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let hours: Option<f64> = conn
            .query_row(
                "SELECT (julianday('now') - julianday(replace(MAX(fetched_at),'Z',''))) * 24.0 FROM model_pricing",
                [],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        cost::pricing_is_stale(hours)
    };
    if stale {
        refresh_pricing_now(app).await?;
    }
    Ok(())
}

/// Whether the stored-key read says this user has chosen OpenRouter: a key is there. No key, and a
/// read that failed, both say no.
fn uses_openrouter<T>(key: &Result<Option<T>>) -> bool {
    matches!(key, Ok(Some(_)))
}

/// Pull the public OpenRouter catalogue (no key) and upsert every model's prices into the cache,
/// which the cost logger reads. Also caches the cache-read rate, context length, supported params
/// and capability indices: those fed the model recommender, DELETED in v3.18.0-alpha (#369), and
/// are write-only today — migration v8's columns are append-only and the dev inspector reads them.
/// Never holds the DB lock across the network call (rule #4).
async fn refresh_pricing_now(app: &AppHandle) -> Result<()> {
    let models = openrouter::fetch_catalogue().await?;
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    let tx = conn.unchecked_transaction()?;
    // One timestamp for the whole batch, so every model in this pull shares an identical
    // `fetched_at`. That lets the recommender read only the latest batch (a model that left
    // OpenRouter keeps an older timestamp and drops out of candidacy — see `cached_catalogue`),
    // and keeps the staleness check exact.
    let fetched_at: String =
        tx.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |r| {
            r.get(0)
        })?;
    for m in &models {
        let supported =
            serde_json::to_string(&m.supported_parameters).unwrap_or_else(|_| "[]".into());
        let modalities = serde_json::to_string(&m.input_modalities).unwrap_or_else(|_| "[]".into());
        tx.execute(
            "INSERT INTO model_pricing(model, prompt_price, completion_price, name, context_length, \
                cache_read_price, supported_parameters, input_modalities, intelligence_index, fetched_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
             ON CONFLICT(model) DO UPDATE SET \
                prompt_price = ?2, completion_price = ?3, name = ?4, context_length = ?5, \
                cache_read_price = ?6, supported_parameters = ?7, input_modalities = ?8, \
                intelligence_index = ?9, fetched_at = ?10",
            params![
                m.id,
                m.prompt_price,
                m.completion_price,
                m.name,
                m.context_length.map(|v| v as i64),
                m.cache_read_price,
                supported,
                modalities,
                m.intelligence_index,
                fetched_at,
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Assemble the cost summary from `usage_log` × the cached `model_pricing`.
fn build_cost_summary(conn: &Connection) -> Result<CostSummary> {
    let last_30d = spend_rows(conn, true)?;
    let all_time = spend_rows(conn, false)?;
    let total_30d_usd = total_cost(&last_30d);
    let total_all_time_usd = total_cost(&all_time);
    let pricing_updated_at: Option<String> = conn
        .query_row("SELECT MAX(fetched_at) FROM model_pricing", [], |r| {
            r.get(0)
        })
        .ok()
        .flatten();
    Ok(CostSummary {
        last_30d,
        all_time,
        by_mod: spend_by_mod(conn)?,
        total_30d_usd,
        total_all_time_usd,
        pricing_updated_at,
    })
}

/// Per-model token sums + request counts (optionally only the last 30 days), priced
/// from the cache; ordered by request count desc. Rows with a NULL model are excluded.
fn spend_rows(conn: &Connection, last_30d: bool) -> Result<Vec<ModelSpend>> {
    let window = if last_30d {
        "AND u.created_at >= strftime('%Y-%m-%dT%H:%M:%fZ','now','-30 days')"
    } else {
        ""
    };
    // Split the token sums by whether the row carried OpenRouter's reported cost, so cost is
    // computed ROW-ADDITIVELY: real reported spend (`SUM(cost_usd)` over the rows that have it) plus
    // a tokens × cached-price estimate for ONLY the rows that don't. The earlier all-or-nothing rule
    // abandoned the whole group's real cost the moment a single pre-feature row (NULL `cost_usd`) was
    // present — so a model with both old and new calls fell back to the estimate and went blank when
    // it wasn't in the price cache. Additive costing keeps the known real spend visible regardless.
    let sql = format!(
        "SELECT u.model, \
                COALESCE(SUM(u.prompt_tokens), 0), \
                COALESCE(SUM(u.completion_tokens), 0), \
                COUNT(*), \
                p.prompt_price, p.completion_price, \
                SUM(u.cost_usd), COUNT(u.cost_usd), \
                COALESCE(SUM(CASE WHEN u.cost_usd IS NULL THEN u.prompt_tokens     ELSE 0 END), 0), \
                COALESCE(SUM(CASE WHEN u.cost_usd IS NULL THEN u.completion_tokens ELSE 0 END), 0) \
         FROM usage_log u LEFT JOIN model_pricing p ON p.model = u.model \
         WHERE u.model IS NOT NULL {window} \
         GROUP BY u.model \
         ORDER BY COUNT(*) DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt
        .query_map([], |r| {
            let prompt_tokens: i64 = r.get(1)?;
            let completion_tokens: i64 = r.get(2)?;
            let request_count: i64 = r.get(3)?;
            let prompt_price: Option<f64> = r.get(4)?;
            let completion_price: Option<f64> = r.get(5)?;
            let reported_cost: Option<f64> = r.get(6)?; // SUM(cost_usd); NULL when no call reported one
            let reported_count: i64 = r.get(7)?; // calls in this group that reported an actual cost
            let est_prompt_tokens: i64 = r.get(8)?; // tokens from ONLY the rows lacking a reported cost
            let est_completion_tokens: i64 = r.get(9)?;
            let cost_usd = row_additive_cost(
                request_count,
                reported_cost,
                reported_count,
                est_prompt_tokens,
                est_completion_tokens,
                prompt_price,
                completion_price,
            );
            Ok(ModelSpend {
                model: r.get(0)?,
                prompt_tokens,
                completion_tokens,
                request_count,
                cost_usd,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    // Rank by cost (most expensive first); unpriced models (unknown cost) sort last,
    // then by request count — so the breakdown reads as a spend ranking.
    rows.sort_by(|a, b| {
        let ak = a.cost_usd.unwrap_or(f64::NEG_INFINITY);
        let bk = b.cost_usd.unwrap_or(f64::NEG_INFINITY);
        bk.partial_cmp(&ak)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.request_count.cmp(&a.request_count))
    });
    Ok(rows)
}

/// One group's cost, row-additively: the real reported spend, plus an estimate (tokens × cached
/// price) for ONLY the rows that reported none. `reported_cost` is `SUM(cost_usd)` (NULL when no
/// row reported one), `reported_count` how many rows did, and the `est_*` tokens come from the rows
/// that did not. Estimate the unreported rows; `None` when the model isn't priced, and `Some(0.0)`
/// when every row reported an actual cost (nothing left to estimate).
///
/// Real reported spend is always honoured; the estimate only fills in the rows that lacked a
/// reported cost. "Unknown" (`None`) survives only when NOTHING is known — no reported cost and the
/// leftover rows are unpriced — never just because of an old row.
fn row_additive_cost(
    request_count: i64,
    reported_cost: Option<f64>,
    reported_count: i64,
    est_prompt_tokens: i64,
    est_completion_tokens: i64,
    prompt_price: Option<f64>,
    completion_price: Option<f64>,
) -> Option<f64> {
    let estimate = if request_count - reported_count > 0 {
        cost::call_cost(
            Some(est_prompt_tokens),
            Some(est_completion_tokens),
            prompt_price,
            completion_price,
        )
    } else {
        Some(0.0)
    };
    match (reported_cost, estimate) {
        (Some(actual), Some(est)) => Some(actual + est),
        (Some(actual), None) => Some(actual), // real cost known; unpriced remainder omitted
        (None, Some(est)) => Some(est),
        (None, None) => None,
    }
}

/// Spend per mod, all time (a mod's calls are `usage_log` rows carrying its `mod_id`). Rows whose
/// mod is no longer installed keep their history with `name: None` (the page says "removed mod").
/// Priced row-additively like [`spend_rows`], per (mod, model) and then summed, so one unpriced
/// model doesn't blank a mod's real spend. Best-effort: this reads `usage_log`, which a failed
/// insert can skip. The cap reads `mods.ai_spent_usd` instead, so the two numbers can differ and
/// the page labels them. Most spent first; a mod with nothing priced sorts last.
fn spend_by_mod(conn: &Connection) -> Result<Vec<ModAiSpend>> {
    let mut stmt = conn.prepare(
        "SELECT u.mod_id, m.name, \
                COALESCE(SUM(u.prompt_tokens), 0), \
                COALESCE(SUM(u.completion_tokens), 0), \
                COUNT(*), \
                p.prompt_price, p.completion_price, \
                SUM(u.cost_usd), COUNT(u.cost_usd), \
                COALESCE(SUM(CASE WHEN u.cost_usd IS NULL THEN u.prompt_tokens     ELSE 0 END), 0), \
                COALESCE(SUM(CASE WHEN u.cost_usd IS NULL THEN u.completion_tokens ELSE 0 END), 0) \
         FROM usage_log u \
         LEFT JOIN mods m ON m.id = u.mod_id \
         LEFT JOIN model_pricing p ON p.model = u.model \
         WHERE u.mod_id IS NOT NULL \
         GROUP BY u.mod_id, u.model",
    )?;
    let mut by_mod: Vec<ModAiSpend> = Vec::new();
    let groups = stmt.query_map([], |r| {
        let request_count: i64 = r.get(4)?;
        let cost_usd = row_additive_cost(
            request_count,
            r.get(7)?,
            r.get(8)?,
            r.get(9)?,
            r.get(10)?,
            r.get(5)?,
            r.get(6)?,
        );
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
            request_count,
            cost_usd,
        ))
    })?;
    for group in groups {
        let (mod_id, name, prompt_tokens, completion_tokens, request_count, cost_usd) = group?;
        match by_mod.iter_mut().find(|m| m.mod_id == mod_id) {
            Some(m) => {
                m.prompt_tokens += prompt_tokens;
                m.completion_tokens += completion_tokens;
                m.request_count += request_count;
                m.cost_usd = match (m.cost_usd, cost_usd) {
                    (Some(a), Some(b)) => Some(a + b),
                    (a, b) => a.or(b),
                };
            }
            None => by_mod.push(ModAiSpend {
                mod_id,
                name,
                prompt_tokens,
                completion_tokens,
                request_count,
                cost_usd,
            }),
        }
    }
    by_mod.sort_by(|a, b| {
        let ak = a.cost_usd.unwrap_or(f64::NEG_INFINITY);
        let bk = b.cost_usd.unwrap_or(f64::NEG_INFINITY);
        bk.partial_cmp(&ak)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.request_count.cmp(&a.request_count))
            .then_with(|| a.mod_id.cmp(&b.mod_id))
    });
    Ok(by_mod)
}

/// Total spend across rows: `Some(0)` with no usage, `None` when there's usage but no
/// model is priced yet, else the sum of the priced rows (unpriced models shown "—").
fn total_cost(rows: &[ModelSpend]) -> Option<f64> {
    if rows.is_empty() {
        return Some(0.0);
    }
    let known: Vec<f64> = rows.iter().filter_map(|r| r.cost_usd).collect();
    if known.is_empty() {
        return None;
    }
    Some(known.iter().sum())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::shared::temp_db;

    /// The automatic price refresh runs only for a user with an OpenRouter key; with none, or with a
    /// keychain that can't be read, Settings → AI & Models contacts nothing.
    #[test]
    fn prices_refresh_by_themselves_only_with_a_key_stored() {
        assert!(uses_openrouter::<&str>(&Ok(Some("sk-or-v1-a"))));
        assert!(!uses_openrouter::<&str>(&Ok(None)));
        assert!(!uses_openrouter::<&str>(&Err(crate::error::Error::Other(
            "keychain locked".into()
        ))));
    }

    /// The gate is wired in, not only defined: `ensure_pricing_fresh` asks it first, before the
    /// staleness read and the fetch, and `cost_summary` is the one caller that refreshes by itself.
    /// The names are built from parts so this test can't match itself.
    #[test]
    fn the_automatic_refresh_asks_for_a_key_before_anything_else() {
        let src = include_str!("spend.rs");
        let sig = concat!("async fn ensure_pricing_", "fresh(");
        assert_eq!(src.matches(sig).count(), 1);
        let body = src.split(sig).nth(1).unwrap();
        let body = &body[..body.find("\n}\n").unwrap()];
        // Code only: a commented-out gate must not count as one, in either comment form.
        let mut body: String = body
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        while let Some(open) = body.find("/*") {
            let close = body[open..]
                .find("*/")
                .map_or(body.len(), |end| open + end + 2);
            body.replace_range(open..close, "");
        }
        let gate = body
            .find(concat!(
                "if !uses_open",
                "router(&crate::secrets::get_background_or_primary_key()) {"
            ))
            .expect("the key check");
        let stale = body
            .find(concat!("let ", "stale"))
            .expect("the staleness read");
        let fetch = body
            .find(concat!("refresh_pricing_", "now(app)"))
            .expect("the fetch");
        assert!(gate < stale && gate < fetch);
        assert!(body[gate..stale].contains(concat!("return ", "Ok(());")));
        // The definition and `cost_summary`'s call: no other caller refreshes by itself.
        assert_eq!(src.matches(concat!("ensure_pricing_", "fresh(")).count(), 2);
        // The ungated fetch: its definition, the button (`refresh_pricing`) and the gated check
        // above. Another caller would be a way round the gate.
        assert_eq!(src.matches(concat!("refresh_pricing_", "now(")).count(), 3);
    }

    /// Cost is ROW-ADDITIVE: a model's real reported spend always shows, with an estimate filling in
    /// only the rows that lacked one. The earlier all-or-nothing rule went blank for any model that
    /// mixed a reported call with an older unreported one and wasn't in the price cache — this pins
    /// the fix.
    #[test]
    fn spend_rows_adds_real_cost_and_estimates_only_unreported_rows() {
        let (_dir, conn) = temp_db();
        let priced_now = |model: &str| {
            conn.execute(
                "INSERT INTO model_pricing(model, prompt_price, completion_price, fetched_at) \
                 VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
                params![model, 3e-6_f64, 15e-6_f64],
            )
            .unwrap();
        };
        let log = |model: &str, pt: i64, ct: i64, cost: Option<f64>| {
            conn.execute(
                "INSERT INTO usage_log(model, kind, prompt_tokens, completion_tokens, cost_usd) \
                 VALUES (?1, 'chat', ?2, ?3, ?4)",
                params![model, pt, ct, cost],
            )
            .unwrap();
        };

        // Priced model, mixed rows: a reported $0.05 call + an older unreported one (1000/500 tokens).
        priced_now("vendor/priced");
        log("vendor/priced", 2000, 1000, Some(0.05));
        log("vendor/priced", 1000, 500, None);

        // Unpriced model, mixed rows — the regression case: a reported $0.02 call + an unreported one.
        log("vendor/unpriced", 100, 100, Some(0.02));
        log("vendor/unpriced", 100, 100, None);

        // Unpriced model, only an old unreported row — genuinely unknown.
        log("vendor/unknown", 100, 100, None);

        let rows = spend_rows(&conn, false).unwrap();
        let cost_of = |m: &str| rows.iter().find(|r| r.model == m).unwrap().cost_usd;

        // Reported 0.05 + estimate(1000·3e-6 + 500·15e-6 = 0.0105) = 0.0605.
        assert!((cost_of("vendor/priced").unwrap() - 0.0605).abs() < 1e-9);
        // The fix: the real reported cost shows as a floor even though the model isn't priced —
        // never blank. The unpriced unreported row is omitted (unknown), not understated to $0.
        assert!((cost_of("vendor/unpriced").unwrap() - 0.02).abs() < 1e-9);
        // Nothing known at all → still "unknown".
        assert!(cost_of("vendor/unknown").is_none());

        // And the grand total is the sum of the known rows (0.0605 + 0.02), not blank.
        assert!((total_cost(&rows).unwrap() - 0.0805).abs() < 1e-9);
    }

    fn meta() -> llm_gateway::CallMeta {
        llm_gateway::CallMeta {
            provider: llm_gateway::Provider::Cloud,
            latency_ms: 5,
            fallback: None,
            displaced_local_model: None,
        }
    }

    /// PM's own calls go through `log_usage` and carry no mod; a mod's row is the same INSERT with
    /// its id, so all 26 existing callers are untouched and there is one writer (I-11).
    #[test]
    fn log_usage_writes_no_mod_and_log_usage_as_writes_the_one_it_is_given() {
        let (_dir, conn) = temp_db();
        let usage = openrouter::Usage {
            prompt_tokens: Some(10),
            completion_tokens: Some(5),
            cost: Some(0.01),
        };
        log_usage(&conn, "chat", Some("a/m"), &usage, &meta());
        log_usage_as(&conn, "background", Some("a/m"), &usage, &meta(), None);
        log_usage_as(
            &conn,
            "background",
            Some("a/m"),
            &usage,
            &meta(),
            Some("acme.tool"),
        );
        let ids: Vec<Option<String>> = conn
            .prepare("SELECT mod_id FROM usage_log ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(ids, vec![None, None, Some("acme.tool".to_string())]);
        // The row is otherwise the one every caller writes.
        let (kind, cost, provider): (String, f64, String) = conn
            .query_row(
                "SELECT kind, cost_usd, provider FROM usage_log WHERE mod_id IS NOT NULL",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (kind.as_str(), cost, provider.as_str()),
            ("background", 0.01, "cloud")
        );
    }

    /// `by_mod`: one line per mod across its models, priced row-additively like the per-model
    /// lists; PM's own rows are not in it; an uninstalled mod keeps its history with no name.
    #[test]
    fn by_mod_sums_each_mods_models_and_marks_a_removed_mod() {
        let (_dir, conn) = temp_db();
        conn.execute(
            "INSERT INTO model_pricing(model, prompt_price, completion_price, fetched_at) \
             VALUES ('a/priced', 3e-6, 15e-6, strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO mods(id, name, version, tier, author, source_repo, source_commit, signed, \
                              install_id, installed_from, package_sha256, manifest, files) \
             VALUES ('acme.tool', 'Tool', '1.0.0', 'second', 'A', 'a/a', ?1, 1, '0123456789abcdef', \
                     'file', ?2, '{}', '[]')",
            params!["0".repeat(40), "a".repeat(64)],
        )
        .unwrap();
        let log = |mod_id: Option<&str>,
                   model: Option<&str>,
                   pt: i64,
                   ct: i64,
                   cost: Option<f64>| {
            conn.execute(
                "INSERT INTO usage_log(model, kind, prompt_tokens, completion_tokens, cost_usd, mod_id) \
                 VALUES (?1, 'background', ?2, ?3, ?4, ?5)",
                params![model, pt, ct, cost, mod_id],
            )
            .unwrap();
        };
        // acme.tool: a reported $0.05 call and an unreported one on a priced model (estimate
        // 1000*3e-6 + 500*15e-6 = 0.0105), plus a reported $0.02 on an unpriced model.
        log(Some("acme.tool"), Some("a/priced"), 2000, 1000, Some(0.05));
        log(Some("acme.tool"), Some("a/priced"), 1000, 500, None);
        log(Some("acme.tool"), Some("a/unpriced"), 100, 100, Some(0.02));
        // gone.mod has no `mods` row any more: only an unpriceable call.
        log(Some("gone.mod"), Some("a/unpriced"), 7, 3, None);
        // PM's own call is not a mod's.
        log(None, Some("a/priced"), 9, 9, Some(1.0));

        let rows = spend_by_mod(&conn).unwrap();
        assert_eq!(rows.len(), 2);
        let tool = &rows[0];
        assert_eq!(tool.mod_id, "acme.tool");
        assert_eq!(tool.name.as_deref(), Some("Tool"));
        assert_eq!(tool.request_count, 3);
        assert_eq!(tool.prompt_tokens, 3100);
        assert_eq!(tool.completion_tokens, 1600);
        assert!((tool.cost_usd.unwrap() - (0.0605 + 0.02)).abs() < 1e-9);
        // Nothing priced for the removed mod: "unknown", not $0, and sorted last.
        let gone = &rows[1];
        assert_eq!(gone.mod_id, "gone.mod");
        assert_eq!(gone.name, None);
        assert_eq!(gone.request_count, 1);
        assert!(gone.cost_usd.is_none());
        // And the summary carries it.
        assert_eq!(build_cost_summary(&conn).unwrap().by_mod.len(), 2);
    }
}
