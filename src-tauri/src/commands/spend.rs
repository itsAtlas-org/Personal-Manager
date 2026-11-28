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

/// Spend for one model, on one road, over a window. `cost_usd` is `None` when the model isn't in
/// the price cache yet — surfaced as "—", never an understated $0. One line per (model, which
/// company answered): the same model id reached through OpenRouter and through a person's own key
/// is two lines, because they are priced differently.
#[derive(Serialize)]
pub struct ModelSpend {
    pub model: String,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub request_count: i64,
    pub cost_usd: Option<f64>,
    /// Which road the calls took: `openrouter`, `openai`, `gemini`, `anthropic`, `local`, or `None`
    /// for a cloud row from before PM recorded it (never guessed from the model id).
    pub served: Option<String>,
    /// Prompt tokens read from the provider's cache, and tokens it spent thinking (already inside
    /// `completion_tokens`). Information; the cost has them priced already.
    pub cached_tokens: i64,
    pub thinking_tokens: i64,
    /// Replies the provider's safety system refused (billed, no text). Only Anthropic's stop reason
    /// is read for this; an OpenAI content-filter stop is an error and Gemini's is not read.
    pub refused_requests: i64,
    /// Calls PM could not price (or could only price in part): the figure beside is a floor.
    pub unpriced_requests: i64,
    /// `cost_usd` is a floor ("at least"), or unknown for a non-local model.
    pub partial: bool,
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
    /// Some of what this mod spent could not be priced, so `cost_usd` is a floor.
    pub partial: bool,
}

/// How many replies in the window carried a token type PM can't price, by reason slug.
#[derive(Serialize, Debug, PartialEq)]
pub struct PartialReason {
    /// A [`openrouter::CostNote`] slug naming a token type (never the vendor's own words).
    pub slug: String,
    pub replies: i64,
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
    /// The totals are floors: some calls in the window couldn't be priced.
    pub total_30d_partial: bool,
    pub total_all_time_partial: bool,
    /// How many calls in the last 30 days couldn't be priced (shown only when above zero).
    pub unpriced_30d: i64,
    /// The named token types PM can't price, in the last 30 days, with how many replies each.
    pub partial_reasons: Vec<PartialReason>,
    /// Replies in the last 30 days that a LATER row of the person's own list answered, so a bad
    /// first row cannot quietly bill the second one night after night (Background and Review have
    /// no strip). `None` when there were none.
    pub list_walks: Option<ListWalks>,
    pub pricing_updated_at: Option<String>,
}

/// The persistent record of a list's walks, read from `usage_log.fallback_reason`.
#[derive(Serialize, Debug, PartialEq)]
pub struct ListWalks {
    pub replies: i64,
    /// Why the earliest of them moved, in the class table's words ("busy", "no credit left or at a
    /// spend limit"): one copy of each phrase, shared with the strip and `Error::Exhausted`.
    pub first_reason: String,
    /// When the earliest of them was logged (RFC 3339); the page words the date.
    pub first_at: String,
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
/// so errors are swallowed. `model` is `Option` only because the callers' own values are; every
/// caller passes one (the served model, else the requested one), because the column is `NOT NULL`
/// and a `None` is rejected by the insert and reported on stderr (below), not stored. `meta` tags
/// the row with how it was served (provider / latency / fallback reason, migration v37) and WHICH
/// company answered (migration v64), so a reader can tell local from cloud spend, a direct row from
/// an OpenRouter one, and a power-policy route from a failure. The dev inspector shows them; the
/// Usage tab shows the company, the cache and thinking counts, refusals and what could not be
/// priced. `pub(crate)` so the chat housekeeping modules (summary / title / prefs) route their rows
/// through it too.
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

/// [`log_usage`] for a reply that may have been CUT OFF at the model's token ceiling (the chat reply
/// and the booking of an empty one are the callers that hold `Completion.truncated`): the row says so
/// in `outcome`. Every other caller has no such flag and writes none.
pub(crate) fn log_usage_truncated(
    conn: &Connection,
    kind: &str,
    model: Option<&str>,
    usage: &openrouter::Usage,
    meta: &llm_gateway::CallMeta,
    truncated: bool,
) {
    write_usage_row(conn, kind, model, usage, meta, None, truncated);
}

/// [`log_usage`] for a row that may be a mod's: `mod_id` is the mod the spend was for (`mods::ai`),
/// `None` for PM's own calls. A mod's row is written by the same code as any other and no new `kind`
/// is needed (a mod's calls are `background`, told apart by this column).
pub(crate) fn log_usage_as(
    conn: &Connection,
    kind: &str,
    model: Option<&str>,
    usage: &openrouter::Usage,
    meta: &llm_gateway::CallMeta,
    mod_id: Option<&str>,
) {
    write_usage_row(conn, kind, model, usage, meta, mod_id, false);
}

/// Why this row's cost is missing or partial, as a fixed slug (`usage_log.cost_note`). An adapter's
/// own note (a token type PM has no price for) comes first; then the gaps this seam can see itself: a
/// count that "can't be priced exactly" (`output_partial`), a direct reply that reported one side of
/// the count only, and a cloud reply that reported no usage at all. A local row is free and never
/// carries a note, and a row with a cost and no reason carries none.
fn cost_note_for(
    usage: &openrouter::Usage,
    meta: &llm_gateway::CallMeta,
) -> Option<openrouter::CostNote> {
    use openrouter::CostNote;
    if let Some(note) = usage.cost_note {
        return Some(note);
    }
    let provider = meta.cloud_provider?;
    if usage.output_partial {
        return Some(CostNote::PartialUsage);
    }
    if usage.cost.is_some() {
        return None;
    }
    match (usage.prompt_tokens, usage.completion_tokens) {
        (None, None) => Some(CostNote::NoUsage),
        // One side only: OpenRouter's price cache can still estimate a row like that, but a direct
        // provider's cost is worked out by PM from both sides and left empty when one is missing.
        (Some(_), None) | (None, Some(_)) if provider.is_direct() => Some(CostNote::PartialUsage),
        _ => None,
    }
}

/// The one INSERT into `usage_log` (I-11): every `log_usage*` function ends here.
fn write_usage_row(
    conn: &Connection,
    kind: &str,
    model: Option<&str>,
    usage: &openrouter::Usage,
    meta: &llm_gateway::CallMeta,
    mod_id: Option<&str>,
    truncated: bool,
) {
    // One row, tagged with how it was served (provider / latency / fallback, the v37 columns) and
    // what it carried (the v64 columns).
    let fallback = meta.fallback.as_ref().map(|f| f.as_log_str());
    let served_provider = meta.cloud_provider.map(|p| p.as_str());
    // A refusal wins over a cut-off: it has no text to be cut.
    let outcome = if usage.refused {
        Some("refused")
    } else if truncated {
        Some("truncated")
    } else {
        None
    };
    let cost_note = cost_note_for(usage, meta).map(openrouter::CostNote::as_str);
    // Best-effort: accounting must NEVER fail a model call, so we don't propagate the error. But we do
    // NOT swallow it silently — a rejected insert here almost always means a schema mismatch (a store
    // missing the v37 columns), the exact class of bug v36 hid for months by pairing a rejecting CHECK
    // with a silent `let _ =`. Surface it at once so a mismatch shows up in seconds, not as months of
    // missing cost data.
    if let Err(e) = conn.execute(
        "INSERT INTO usage_log(model, kind, prompt_tokens, completion_tokens, cost_usd, \
         provider, latency_ms, fallback_reason, mod_id, served_provider, cached_tokens, \
         cache_write_tokens, thinking_tokens, outcome, cost_note) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            model,
            kind,
            usage.prompt_tokens,
            usage.completion_tokens,
            usage.cost,
            meta.provider.as_str(),
            meta.latency_ms as i64,
            fallback,
            mod_id,
            served_provider,
            usage.cached_tokens,
            usage.cache_write_tokens,
            usage.thinking_tokens,
            outcome,
            cost_note
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
///
/// A mod's `ai.complete` asks it too, before it prices a cloud route (a fresh machine has no prices
/// until the first refresh, and a call with no price is refused): the same key gate holds, so a
/// local-only user's mod call contacts nothing.
pub(crate) async fn ensure_pricing_fresh(app: &AppHandle) -> Result<()> {
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
    let total_30d_partial = total_is_partial(&last_30d);
    let total_all_time_partial = total_is_partial(&all_time);
    let unpriced_30d = last_30d.iter().map(|r| r.unpriced_requests).sum();
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
        total_30d_partial,
        total_all_time_partial,
        unpriced_30d,
        partial_reasons: partial_reasons(conn)?,
        list_walks: list_walks(conn)?,
        pricing_updated_at,
    })
}

/// The replies of the last 30 days that a later row of a person's list answered. The `_` in a LIKE
/// pattern matches any character, so it is escaped: only `list_...` counts, never a look-alike.
fn list_walks(conn: &Connection) -> Result<Option<ListWalks>> {
    const WHERE: &str = "fallback_reason LIKE 'list\\_%' ESCAPE '\\' \
         AND created_at >= strftime('%Y-%m-%dT%H:%M:%fZ','now','-30 days')";
    let replies: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM usage_log WHERE {WHERE}"),
        [],
        |r| r.get(0),
    )?;
    if replies == 0 {
        return Ok(None);
    }
    let (reason, at): (String, String) = conn.query_row(
        &format!(
            "SELECT fallback_reason, created_at FROM usage_log WHERE {WHERE} \
             ORDER BY created_at, id LIMIT 1"
        ),
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(Some(ListWalks {
        replies,
        first_reason: crate::error::list_reason_words(&reason)
            .unwrap_or("it couldn't answer")
            .to_string(),
        first_at: at,
    }))
}

/// The cost notes that name a token type PM can't price, as a SQL list of their fixed slugs. Built
/// from [`openrouter::CostNote`] so the list the writer, the reader and the Usage tab share can't
/// drift (a slug is a constant from that enum, never stored text, so it is safe to splice in).
fn unpriced_type_slugs() -> String {
    openrouter::CostNote::ALL
        .into_iter()
        .filter(|n| n.is_unpriced_type())
        .map(|n| format!("'{}'", n.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// How many replies in the last 30 days carried each token type PM can't price, in the Usage tab's
/// order. Local rows carry no note, so they never appear here.
fn partial_reasons(conn: &Connection) -> Result<Vec<PartialReason>> {
    let mut found = Vec::new();
    let mut stmt = conn.prepare(&format!(
        "SELECT cost_note, COUNT(*) FROM usage_log \
         WHERE cost_note IN ({}) \
           AND created_at >= strftime('%Y-%m-%dT%H:%M:%fZ','now','-30 days') \
         GROUP BY cost_note",
        unpriced_type_slugs()
    ))?;
    let counts = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for note in openrouter::CostNote::ALL {
        if let Some((slug, replies)) = counts.iter().find(|(s, _)| s == note.as_str()) {
            found.push(PartialReason {
                slug: slug.clone(),
                replies: *replies,
            });
        }
    }
    Ok(found)
}

/// Whether the total over these rows is a floor: some line is partial (a reported cost with an
/// unpriced remainder, a token type PM can't price) or wholly unknown, which the total skips.
fn total_is_partial(rows: &[ModelSpend]) -> bool {
    rows.iter().any(|r| r.partial)
}

/// Per-model token sums + request counts (optionally only the last 30 days), priced
/// from the cache; ordered by request count desc. Rows with a NULL model are excluded.
fn spend_rows(conn: &Connection, last_30d: bool) -> Result<Vec<ModelSpend>> {
    let window = if last_30d {
        Window::Last30Days
    } else {
        Window::All
    };
    spend_rows_in(conn, window, None)
}

/// Which `usage_log` rows a spend read covers.
#[derive(Clone, Copy)]
pub(crate) enum Window<'a> {
    All,
    Last30Days,
    /// From this instant on, given in SQLite's `YYYY-MM-DD HH:MM:SS` UTC form (what
    /// `clock::day_start_utc_in` returns). The stored `created_at` is `YYYY-MM-DDTHH:MM:SS.fffZ`, and
    /// `'T'` sorts after `' '`, so the two are NEVER compared as they stand: the bound goes through
    /// the same `strftime` that wrote the column. Compared raw, every row from UTC midnight up to the
    /// person's own midnight would count as "today" in a zone east of UTC.
    Since(&'a str),
}

/// [`spend_rows`] over a window, optionally for one company's rows only (`served_provider`, so an
/// OpenRouter row and a direct one with a look-alike id never count as each other's). The daily limit
/// reads its day's spend through this, so the Usage tab and a limit price a row the same way.
fn spend_rows_in(
    conn: &Connection,
    window: Window<'_>,
    provider: Option<&str>,
) -> Result<Vec<ModelSpend>> {
    let mut binds: Vec<String> = Vec::new();
    let mut window_sql = match window {
        Window::All => String::new(),
        Window::Last30Days => {
            "AND u.created_at >= strftime('%Y-%m-%dT%H:%M:%fZ','now','-30 days')".to_string()
        }
        Window::Since(from) => {
            binds.push(from.to_string());
            "AND u.created_at >= strftime('%Y-%m-%dT%H:%M:%fZ', ?)".to_string()
        }
    };
    if let Some(p) = provider {
        binds.push(p.to_string());
        window_sql.push_str(" AND u.served_provider = ?");
    }
    let window = window_sql;
    // Split the token sums by whether the row carried OpenRouter's reported cost, so cost is
    // computed ROW-ADDITIVELY: real reported spend (`SUM(cost_usd)` over the rows that have it) plus
    // a tokens × cached-price estimate for ONLY the rows that don't. The earlier all-or-nothing rule
    // abandoned the whole group's real cost the moment a single pre-feature row (NULL `cost_usd`) was
    // present — so a model with both old and new calls fell back to the estimate and went blank when
    // it wasn't in the price cache. Additive costing keeps the known real spend visible regardless.
    //
    // One line per (model, which company answered). `served` is the recorded company, or "local" for
    // a row the v37 `provider` column already says was local; a cloud row from before v64 has neither
    // and stays NULL ("Before PM recorded this"), never guessed from its model id.
    //
    // ANY note on a non-local row makes the line a floor, not only the notes that name a token type:
    // an OpenAI 12-hour cache write carries `partial_usage` WITH a cost (priced at the one write rate),
    // and a cost with a note is a number PM knows is short. Judging by the note, not by whether a cost
    // is missing, keeps every provider on one rule (Anthropic blanks its cost for the same case and is
    // caught either way). The named warnings below still list only the token-type slugs.
    // The OpenRouter price joins ONLY an OpenRouter row (or an old row that recorded nothing): a
    // direct row whose cost was left empty must never borrow the price of an OpenRouter model that
    // happens to share its id, which would read as an exact cost and be wrong. A local row is free and
    // joins nothing, so a local model named like an OpenRouter one is not priced either.
    let types = unpriced_type_slugs();
    let sql = format!(
        "SELECT u.model, \
                COALESCE(SUM(u.prompt_tokens), 0), \
                COALESCE(SUM(u.completion_tokens), 0), \
                COUNT(*), \
                p.prompt_price, p.completion_price, \
                SUM(u.cost_usd), COUNT(u.cost_usd), \
                COALESCE(SUM(CASE WHEN u.cost_usd IS NULL THEN u.prompt_tokens     ELSE 0 END), 0), \
                COALESCE(SUM(CASE WHEN u.cost_usd IS NULL THEN u.completion_tokens ELSE 0 END), 0), \
                CASE WHEN u.served_provider IN ('openrouter', 'openai', 'gemini', 'anthropic') \
                          THEN u.served_provider \
                     WHEN u.provider = 'local' THEN 'local' END AS served, \
                COALESCE(SUM(u.cached_tokens), 0), \
                COALESCE(SUM(u.thinking_tokens), 0), \
                COALESCE(SUM(CASE WHEN u.outcome = 'refused' THEN 1 ELSE 0 END), 0), \
                COALESCE(SUM(CASE WHEN COALESCE(u.provider, '') <> 'local' AND ( \
                        u.cost_note IS NOT NULL \
                        OR (u.cost_usd IS NULL AND (p.prompt_price IS NULL OR p.completion_price IS NULL \
                            OR (u.prompt_tokens IS NULL AND u.completion_tokens IS NULL) \
                            OR ((u.prompt_tokens IS NULL) <> (u.completion_tokens IS NULL)))) \
                    ) THEN 1 ELSE 0 END), 0), \
                COALESCE(SUM(CASE WHEN u.cost_note IN ({types}) THEN 1 ELSE 0 END), 0) \
         FROM usage_log u LEFT JOIN model_pricing p ON p.model = u.model \
              AND (u.served_provider IS NULL OR u.served_provider = 'openrouter') \
              AND COALESCE(u.provider, '') <> 'local' \
         WHERE u.model IS NOT NULL {window} \
         GROUP BY u.model, served \
         ORDER BY COUNT(*) DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt
        .query_map(rusqlite::params_from_iter(binds.iter()), |r| {
            let prompt_tokens: i64 = r.get(1)?;
            let completion_tokens: i64 = r.get(2)?;
            let request_count: i64 = r.get(3)?;
            let prompt_price: Option<f64> = r.get(4)?;
            let completion_price: Option<f64> = r.get(5)?;
            let reported_cost: Option<f64> = r.get(6)?; // SUM(cost_usd); NULL when no call reported one
            let reported_count: i64 = r.get(7)?; // calls in this group that reported an actual cost
            let est_prompt_tokens: i64 = r.get(8)?; // tokens from ONLY the rows lacking a reported cost
            let est_completion_tokens: i64 = r.get(9)?;
            let served: Option<String> = r.get(10)?;
            let typed_requests: i64 = r.get(15)?; // replies that carried a token type PM can't price
            let spend = row_additive_cost(
                request_count,
                reported_cost,
                reported_count,
                est_prompt_tokens,
                est_completion_tokens,
                prompt_price,
                completion_price,
            );
            // A local model is free, so no cost is not a gap there. Anywhere else a floor, a token
            // type PM can't price, or no figure at all makes the line a floor.
            // The count includes a reply that reported no usage on a model the cache can price: the
            // estimate for it is $0, which would read as exact, so it is counted and flagged here.
            let unpriced_requests: i64 = r.get(14)?;
            let local = served.as_deref() == Some("local");
            let partial = !local
                && (spend.floor
                    || typed_requests > 0
                    || spend.usd.is_none()
                    || unpriced_requests > 0);
            Ok(ModelSpend {
                model: r.get(0)?,
                prompt_tokens,
                completion_tokens,
                request_count,
                cost_usd: spend.usd,
                served,
                cached_tokens: r.get(11)?,
                thinking_tokens: r.get(12)?,
                refused_requests: r.get(13)?,
                unpriced_requests,
                partial,
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
) -> GroupSpend {
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
        (Some(actual), Some(est)) => GroupSpend::exact(Some(actual + est)),
        // Real cost known; the unpriced remainder is omitted, so the figure is a floor.
        (Some(actual), None) => GroupSpend {
            usd: Some(actual),
            floor: true,
        },
        (None, Some(est)) => GroupSpend::exact(Some(est)),
        (None, None) => GroupSpend::exact(None),
    }
}

/// One group's priced spend and whether it is only a floor: some rows in it were left out because
/// nothing could price them (a reported cost beside rows with neither a cost nor a price). The
/// number is never changed by the flag; the page says "at least" for it.
struct GroupSpend {
    usd: Option<f64>,
    floor: bool,
}

impl GroupSpend {
    fn exact(usd: Option<f64>) -> Self {
        Self { usd, floor: false }
    }
}

/// Spend per mod, all time (a mod's calls are `usage_log` rows carrying its `mod_id`). Rows whose
/// mod is no longer installed keep their history with `name: None` (the page says "removed mod").
/// Priced row-additively like [`spend_rows`], per (mod, model) and then summed, so one unpriced
/// model doesn't blank a mod's real spend. Best-effort: this reads `usage_log`, which a failed
/// insert can skip. The cap reads `mods.ai_spent_usd` instead, so the two numbers can differ and
/// the page labels them. Most spent first; a mod with nothing priced sorts last.
fn spend_by_mod(conn: &Connection) -> Result<Vec<ModAiSpend>> {
    // Any note makes a line a floor here too (see [`spend_rows`]): a cost with a `partial_usage` note
    // is a known under-count.
    // Joined like [`spend_rows`]: an OpenRouter price only for an OpenRouter row (a mod never reaches a
    // direct provider, but the rule is the same one), and none for a local row, which costs nothing.
    let types = unpriced_type_slugs();
    let mut stmt = conn.prepare(&format!(
        "SELECT u.mod_id, m.name, \
                COALESCE(SUM(u.prompt_tokens), 0), \
                COALESCE(SUM(u.completion_tokens), 0), \
                COUNT(*), \
                p.prompt_price, p.completion_price, \
                SUM(u.cost_usd), COUNT(u.cost_usd), \
                COALESCE(SUM(CASE WHEN u.cost_usd IS NULL THEN u.prompt_tokens     ELSE 0 END), 0), \
                COALESCE(SUM(CASE WHEN u.cost_usd IS NULL THEN u.completion_tokens ELSE 0 END), 0), \
                COALESCE(u.provider, '') = 'local' AS is_local, \
                COALESCE(SUM(CASE WHEN u.cost_note IN ({types}) THEN 1 ELSE 0 END), 0), \
                COALESCE(SUM(CASE WHEN COALESCE(u.provider, '') <> 'local' AND ( \
                        u.cost_note IS NOT NULL \
                        OR (u.cost_usd IS NULL \
                            AND (p.prompt_price IS NULL OR p.completion_price IS NULL \
                                 OR (u.prompt_tokens IS NULL AND u.completion_tokens IS NULL) \
                                 OR ((u.prompt_tokens IS NULL) <> (u.completion_tokens IS NULL))))) \
                    THEN 1 ELSE 0 END), 0) \
         FROM usage_log u \
         LEFT JOIN mods m ON m.id = u.mod_id \
         LEFT JOIN model_pricing p ON p.model = u.model \
              AND (u.served_provider IS NULL OR u.served_provider = 'openrouter') \
              AND COALESCE(u.provider, '') <> 'local' \
         WHERE u.mod_id IS NOT NULL \
         GROUP BY u.mod_id, u.model, is_local"
    ))?;
    let mut by_mod: Vec<ModAiSpend> = Vec::new();
    let groups = stmt.query_map([], |r| {
        let request_count: i64 = r.get(4)?;
        let spend = row_additive_cost(
            request_count,
            r.get(7)?,
            r.get(8)?,
            r.get(9)?,
            r.get(10)?,
            r.get(5)?,
            r.get(6)?,
        );
        let is_local: bool = r.get(11)?;
        let typed: i64 = r.get(12)?;
        // A reply with no usage (or one side only) on a model the cache can price estimates to a
        // confident $0 or an under-count; the by-model table counts it, and so does this line.
        let unpriced: i64 = r.get(13)?;
        let partial =
            !is_local && (spend.floor || typed > 0 || spend.usd.is_none() || unpriced > 0);
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
            request_count,
            spend.usd,
            partial,
        ))
    })?;
    for group in groups {
        let (mod_id, name, prompt_tokens, completion_tokens, request_count, cost_usd, partial) =
            group?;
        match by_mod.iter_mut().find(|m| m.mod_id == mod_id) {
            Some(m) => {
                m.prompt_tokens += prompt_tokens;
                m.completion_tokens += completion_tokens;
                m.request_count += request_count;
                // One priced and one unpriced model: the sum is what could be priced, a floor.
                // The `partial` flag says so; the number itself is unchanged.
                m.cost_usd = match (m.cost_usd, cost_usd) {
                    (Some(a), Some(b)) => Some(a + b),
                    (a, b) => a.or(b),
                };
                m.partial |= partial;
            }
            None => by_mod.push(ModAiSpend {
                mod_id,
                name,
                prompt_tokens,
                completion_tokens,
                request_count,
                cost_usd,
                partial,
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

/// What one company's calls cost since an instant, and how many of them PM could not price.
pub(crate) struct Spent {
    pub usd: f64,
    /// Calls with no figure to add (nothing reported, nothing to price them by): they count as zero
    /// in `usd`, so a non-zero count means the sum is a floor.
    pub unpriced: i64,
}

/// What `provider`'s calls cost since `since` (`YYYY-MM-DD HH:MM:SS` UTC), through the same code as
/// the Usage tab so a limit and the page can never disagree about a day.
pub(crate) fn spent_since(conn: &Connection, since: &str, provider: &str) -> Result<Spent> {
    let rows = spend_rows_in(conn, Window::Since(since), Some(provider))?;
    Ok(Spent {
        usd: rows.iter().filter_map(|r| r.cost_usd).sum(),
        unpriced: rows.iter().map(|r| r.unpriced_requests).sum(),
    })
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

        // The number above is unchanged, but only the exact line is exact: a reported cost beside
        // an unpriceable row is a FLOOR, and a model with nothing known is partial too.
        let partial_of = |m: &str| rows.iter().find(|r| r.model == m).unwrap().partial;
        assert!(
            !partial_of("vendor/priced"),
            "estimated from a price: exact"
        );
        assert!(partial_of("vendor/unpriced"), "the floor case says so");
        assert!(partial_of("vendor/unknown"), "nothing known is not exact");
        assert!(
            total_is_partial(&rows),
            "a total with a floor in it is a floor"
        );
        let unpriced_of = |m: &str| {
            rows.iter()
                .find(|r| r.model == m)
                .unwrap()
                .unpriced_requests
        };
        assert_eq!(unpriced_of("vendor/priced"), 0);
        assert_eq!(unpriced_of("vendor/unpriced"), 1);
        assert_eq!(unpriced_of("vendor/unknown"), 1);
    }

    fn meta() -> llm_gateway::CallMeta {
        llm_gateway::CallMeta {
            provider: llm_gateway::Provider::Cloud,
            latency_ms: 5,
            fallback: None,
            displaced_local_model: None,
            cloud_provider: Some(llm_gateway::CloudProvider::OpenRouter),
        }
    }

    /// The meta of a reply that came back by `via` (`None` is a local one).
    fn meta_via(via: Option<llm_gateway::CloudProvider>) -> llm_gateway::CallMeta {
        llm_gateway::CallMeta {
            provider: if via.is_some() {
                llm_gateway::Provider::Cloud
            } else {
                llm_gateway::Provider::Local
            },
            latency_ms: 5,
            fallback: None,
            displaced_local_model: None,
            cloud_provider: via,
        }
    }

    /// Every column v64 added, read back from the one row with this model.
    #[allow(clippy::type_complexity)]
    fn detail_row(
        conn: &Connection,
        model: &str,
    ) -> (
        Option<String>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<String>,
        Option<String>,
        String,
    ) {
        conn.query_row(
            "SELECT served_provider, cached_tokens, cache_write_tokens, thinking_tokens, outcome, \
                    cost_note, provider FROM usage_log WHERE model = ?1",
            [model],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            },
        )
        .unwrap()
    }

    /// The writer records WHICH company answered and what the reply carried, for every road: one row
    /// per provider (OpenRouter, each direct provider, local) goes through the one writer and every
    /// new column is read back. A refusal is an `outcome`, a cut-off is one only when the caller
    /// knows it, a cost PM could not work out says why, and a local row carries none of it.
    #[test]
    fn every_road_writes_its_provider_and_detail_columns() {
        use llm_gateway::CloudProvider as P;
        let (_dir, conn) = temp_db();
        let counted = |cost: Option<f64>| openrouter::Usage {
            prompt_tokens: Some(100),
            completion_tokens: Some(50),
            cost,
            ..Default::default()
        };
        // OpenRouter: a priced reply that read from and wrote to the cache and spent thinking.
        let mut u = counted(Some(0.01));
        u.cached_tokens = Some(40);
        u.cache_write_tokens = Some(8);
        u.thinking_tokens = Some(12);
        log_usage(
            &conn,
            "chat",
            Some("or/m"),
            &u,
            &meta_via(Some(P::OpenRouter)),
        );
        // OpenAI: a refusal (billed, no text).
        let mut u = counted(Some(0.02));
        u.refused = true;
        log_usage(
            &conn,
            "chat",
            Some("openai/m"),
            &u,
            &meta_via(Some(P::OpenAi)),
        );
        // Gemini: a count that can't be priced exactly, so no cost.
        let mut u = counted(None);
        u.output_partial = true;
        log_usage(
            &conn,
            "chat",
            Some("gemini/m"),
            &u,
            &meta_via(Some(P::Gemini)),
        );
        // Anthropic: cut off at the ceiling, and carrying a token type PM has no price for.
        let mut u = counted(Some(0.03));
        u.cost_note = Some(openrouter::CostNote::AudioTokens);
        log_usage_truncated(
            &conn,
            "chat",
            Some("anthropic/m"),
            &u,
            &meta_via(Some(P::Anthropic)),
            true,
        );
        // Local: no cost, no company, and no note: free is not a gap.
        log_usage(
            &conn,
            "chat",
            Some("local/m"),
            &counted(None),
            &meta_via(None),
        );
        // A cloud reply with no usage block at all.
        log_usage(
            &conn,
            "chat",
            Some("openai/silent"),
            &openrouter::Usage::default(),
            &meta_via(Some(P::OpenAi)),
        );

        let row = |m: &str| detail_row(&conn, m);
        let s = |v: &str| Some(v.to_string());
        assert_eq!(
            row("or/m"),
            (
                s("openrouter"),
                Some(40),
                Some(8),
                Some(12),
                None,
                None,
                "cloud".into()
            )
        );
        assert_eq!(
            row("openai/m"),
            (
                s("openai"),
                None,
                None,
                None,
                s("refused"),
                None,
                "cloud".into()
            )
        );
        assert_eq!(
            row("gemini/m"),
            (
                s("gemini"),
                None,
                None,
                None,
                None,
                s("partial_usage"),
                "cloud".into()
            )
        );
        assert_eq!(
            row("anthropic/m"),
            (
                s("anthropic"),
                None,
                None,
                None,
                s("truncated"),
                s("audio_tokens"),
                "cloud".into()
            )
        );
        assert_eq!(
            row("local/m"),
            (None, None, None, None, None, None, "local".into())
        );
        assert_eq!(
            row("openai/silent"),
            (
                s("openai"),
                None,
                None,
                None,
                None,
                s("no_usage"),
                "cloud".into()
            )
        );
    }

    /// A refusal wins over a cut-off in `outcome` (it has no text to cut), and nothing else is ever
    /// written there.
    #[test]
    fn a_refusal_outranks_a_cut_off_and_outcome_has_two_values() {
        let (_dir, conn) = temp_db();
        let refused = openrouter::Usage {
            refused: true,
            ..Default::default()
        };
        let m = meta_via(Some(llm_gateway::CloudProvider::Anthropic));
        log_usage_truncated(&conn, "chat", Some("a/refused"), &refused, &m, true);
        log_usage_truncated(&conn, "chat", Some("a/cut"), &Default::default(), &m, true);
        log_usage_truncated(
            &conn,
            "chat",
            Some("a/plain"),
            &Default::default(),
            &m,
            false,
        );
        assert_eq!(detail_row(&conn, "a/refused").4.as_deref(), Some("refused"));
        assert_eq!(detail_row(&conn, "a/cut").4.as_deref(), Some("truncated"));
        assert_eq!(detail_row(&conn, "a/plain").4, None);
    }

    /// The cost-note slugs are a closed list: every variant round-trips through its slug, the list
    /// is complete (an exhaustive match, so a new variant that is not in `ALL` stops compiling) and
    /// no other string reads back as a note, so an odd stored value is never shown.
    #[test]
    fn the_cost_note_slugs_are_a_closed_round_tripping_list() {
        use openrouter::CostNote;
        for note in CostNote::ALL {
            assert_eq!(CostNote::from_slug(note.as_str()), Some(note));
            // Exhaustive on purpose: adding a variant breaks this match until it is listed.
            match note {
                CostNote::PartialUsage
                | CostNote::NoUsage
                | CostNote::AudioTokens
                | CostNote::PredictionTokens
                | CostNote::NonTextPrompt
                | CostNote::ServerToolUse
                | CostNote::NewTokenType => {}
            }
        }
        let slugs: std::collections::HashSet<_> =
            CostNote::ALL.iter().map(|n| n.as_str()).collect();
        assert_eq!(
            slugs.len(),
            CostNote::ALL.len(),
            "no two notes share a slug"
        );
        for odd in ["", "audio", "AUDIO_TOKENS", "partial_usage ", "<script>"] {
            assert_eq!(CostNote::from_slug(odd), None, "{odd:?}");
        }
        // Only the token types are named in the Usage tab's warning.
        let named: Vec<_> = CostNote::ALL
            .into_iter()
            .filter(|n| n.is_unpriced_type())
            .map(CostNote::as_str)
            .collect();
        assert_eq!(
            named,
            [
                "audio_tokens",
                "prediction_tokens",
                "non_text_prompt",
                "server_tool_use",
                "new_token_type"
            ]
        );
    }

    /// I-11: there is ONE `INSERT INTO usage_log` in this file's production code, in the one writer
    /// every `log_usage*` function ends in. The needle is built from parts so this test can't match
    /// itself, and only the part before the test module counts.
    #[test]
    fn there_is_exactly_one_insert_into_usage_log_here() {
        let src = include_str!("spend.rs");
        let production = src
            .split(concat!("#[cfg(", "test)]\nmod tests"))
            .next()
            .unwrap();
        assert_eq!(
            production
                .matches(concat!("INSERT INTO usage_", "log("))
                .count(),
            1
        );
        // And it is the one inside the writer every public logger delegates to.
        let writer = production.split("fn write_usage_row(").nth(1).unwrap();
        assert!(writer.contains(concat!("INSERT INTO usage_", "log(")));
        // The definition, and the two loggers that call it (`log_usage` goes through `log_usage_as`).
        assert_eq!(production.matches("write_usage_row(").count(), 3);
    }

    /// A direct row whose cost was left empty must not borrow an OpenRouter price just because a
    /// model with the same id is in the price cache: it would read as an exact cost and be wrong.
    /// The same id reached through OpenRouter IS priced from the cache, a row from before v64 that
    /// recorded no company still is (old rows behave as before), and a local model with an
    /// OpenRouter-like name is not priced at all.
    #[test]
    fn a_direct_row_never_borrows_an_openrouter_price_for_the_same_id() {
        use llm_gateway::CloudProvider as P;
        let (_dir, conn) = temp_db();
        conn.execute(
            "INSERT INTO model_pricing(model, prompt_price, completion_price, fetched_at) \
             VALUES ('openai/gpt-x', 1e-6, 2e-6, strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
            [],
        )
        .unwrap();
        let empty = |meta: &llm_gateway::CallMeta| {
            let usage = openrouter::Usage {
                prompt_tokens: Some(1_000_000),
                completion_tokens: Some(1_000_000),
                cost: None,
                ..Default::default()
            };
            log_usage(&conn, "chat", Some("openai/gpt-x"), &usage, meta);
        };
        empty(&meta_via(Some(P::OpenAi)));
        empty(&meta_via(Some(P::OpenRouter)));
        empty(&meta_via(None)); // local
                                // An old cloud row: no company recorded.
        conn.execute(
            "INSERT INTO usage_log(model, kind, prompt_tokens, completion_tokens, provider) \
             VALUES ('openai/gpt-x', 'chat', 1000000, 1000000, 'cloud')",
            [],
        )
        .unwrap();

        let rows = spend_rows(&conn, false).unwrap();
        let line = |served: Option<&str>| {
            rows.iter()
                .find(|r| r.served.as_deref() == served)
                .unwrap_or_else(|| panic!("no line for {served:?}"))
        };
        assert_eq!(
            rows.len(),
            4,
            "one line per road: {:?}",
            rows.iter().map(|r| &r.served).collect::<Vec<_>>()
        );
        assert_eq!(line(Some("openai")).cost_usd, None, "no borrowed price");
        assert!(line(Some("openai")).partial);
        // 1M x 1e-6 + 1M x 2e-6 = $3 from the cache, for the OpenRouter row and the old row alike.
        assert!((line(Some("openrouter")).cost_usd.unwrap() - 3.0).abs() < 1e-9);
        assert!((line(None).cost_usd.unwrap() - 3.0).abs() < 1e-9);
        assert_eq!(
            line(Some("local")).cost_usd,
            None,
            "a local model costs nothing"
        );
        assert!(!line(Some("local")).partial, "and that is not a gap");
        assert_eq!(line(Some("local")).unpriced_requests, 0);
    }

    /// Local models are listed with their request count and no cost, under their own label, and they
    /// do not make a total "at least": they are free, not unknown.
    #[test]
    fn local_rows_are_listed_without_a_cost_and_do_not_make_a_total_partial() {
        let (_dir, conn) = temp_db();
        let usage = openrouter::Usage {
            prompt_tokens: Some(10),
            completion_tokens: Some(5),
            ..Default::default()
        };
        log_usage(&conn, "chat", Some("gemma"), &usage, &meta_via(None));
        log_usage(&conn, "chat", Some("gemma"), &usage, &meta_via(None));
        let summary = build_cost_summary(&conn).unwrap();
        assert_eq!(summary.all_time.len(), 1);
        let l = &summary.all_time[0];
        assert_eq!(
            (l.served.as_deref(), l.request_count, l.cost_usd, l.partial),
            (Some("local"), 2, None, false)
        );
        assert!(!summary.total_all_time_partial);
        assert_eq!(summary.unpriced_30d, 0);
        assert!(summary.partial_reasons.is_empty());
    }

    /// The same model reached by two roads is two lines, each with its own cache and thinking counts
    /// and refusals, and the named token types are counted for the window.
    #[test]
    fn the_summary_splits_roads_and_counts_what_could_not_be_priced() {
        use llm_gateway::CloudProvider as P;
        let (_dir, conn) = temp_db();
        let mut cached = openrouter::Usage {
            prompt_tokens: Some(100),
            completion_tokens: Some(50),
            cost: Some(0.5),
            cached_tokens: Some(60),
            thinking_tokens: Some(20),
            ..Default::default()
        };
        log_usage(
            &conn,
            "chat",
            Some("x/m"),
            &cached,
            &meta_via(Some(P::Gemini)),
        );
        log_usage(
            &conn,
            "chat",
            Some("x/m"),
            &cached,
            &meta_via(Some(P::Gemini)),
        );
        cached.refused = true;
        log_usage(
            &conn,
            "chat",
            Some("x/m"),
            &cached,
            &meta_via(Some(P::OpenRouter)),
        );
        // Two replies with a token type PM can't price, one a different type; and an old one.
        for note in [
            openrouter::CostNote::AudioTokens,
            openrouter::CostNote::AudioTokens,
            openrouter::CostNote::ServerToolUse,
        ] {
            let u = openrouter::Usage {
                prompt_tokens: Some(1),
                completion_tokens: Some(1),
                cost: Some(0.01),
                cost_note: Some(note),
                ..Default::default()
            };
            log_usage(
                &conn,
                "chat",
                Some("x/typed"),
                &u,
                &meta_via(Some(P::Anthropic)),
            );
        }
        conn.execute(
            "INSERT INTO usage_log(model, kind, cost_usd, cost_note, created_at) \
             VALUES ('x/old', 'chat', 0.01, 'audio_tokens', strftime('%Y-%m-%dT%H:%M:%fZ','now','-60 days'))",
            [],
        )
        .unwrap();

        let summary = build_cost_summary(&conn).unwrap();
        let gem = summary
            .all_time
            .iter()
            .find(|r| r.served.as_deref() == Some("gemini"))
            .unwrap();
        assert_eq!(
            (
                gem.request_count,
                gem.cached_tokens,
                gem.thinking_tokens,
                gem.refused_requests
            ),
            (2, 120, 40, 0)
        );
        let or = summary
            .all_time
            .iter()
            .find(|r| r.served.as_deref() == Some("openrouter"))
            .unwrap();
        assert_eq!((or.request_count, or.refused_requests), (1, 1));
        // A typed reply is priced at what it cost and flagged: the total is a floor.
        let typed = summary
            .all_time
            .iter()
            .find(|r| r.model == "x/typed")
            .unwrap();
        assert!((typed.cost_usd.unwrap() - 0.03).abs() < 1e-9);
        assert_eq!((typed.partial, typed.unpriced_requests), (true, 3));
        assert!(summary.total_30d_partial && summary.total_all_time_partial);
        // The 30-day count and reasons leave the 60-day-old row out.
        assert_eq!(summary.unpriced_30d, 3);
        assert_eq!(
            summary.partial_reasons,
            vec![
                PartialReason {
                    slug: "audio_tokens".into(),
                    replies: 2
                },
                PartialReason {
                    slug: "server_tool_use".into(),
                    replies: 1
                },
            ]
        );
    }

    /// A mod's total is a floor when any of its models could not be priced, and a local call of a mod
    /// is free, not a gap.
    #[test]
    fn a_mods_line_is_partial_only_for_what_could_not_be_priced() {
        let (_dir, conn) = temp_db();
        let log = |mod_id: &str, model: &str, cost: Option<f64>, provider: &str| {
            conn.execute(
                "INSERT INTO usage_log(model, kind, prompt_tokens, completion_tokens, cost_usd, mod_id, provider) \
                 VALUES (?1, 'background', 10, 10, ?2, ?3, ?4)",
                params![model, cost, mod_id, provider],
            )
            .unwrap();
        };
        log("a.exact", "a/m", Some(0.1), "cloud");
        log("a.floor", "a/m", Some(0.1), "cloud");
        log("a.floor", "b/unpriced", None, "cloud");
        log("a.local", "gemma", None, "local");
        log("a.local", "a/m", Some(0.1), "cloud");
        let rows = spend_by_mod(&conn).unwrap();
        let of = |id: &str| rows.iter().find(|m| m.mod_id == id).unwrap();
        assert!(!of("a.exact").partial);
        assert!(of("a.floor").partial);
        assert!((of("a.floor").cost_usd.unwrap() - 0.1).abs() < 1e-9);
        assert!(!of("a.local").partial, "a local call costs nothing");
    }

    /// The pure arithmetic and its flag: a reported cost beside rows nothing can price is a FLOOR
    /// (the number is unchanged, the flag says so); an estimate that fills every unreported row, and
    /// a group that reported everything, are exact; nothing known at all is unknown, not a floor.
    #[test]
    fn row_additive_cost_flags_a_floor_without_changing_the_number() {
        // 2 rows, one reported $0.02, the other has no cost and no price: the floor case.
        let floor = row_additive_cost(2, Some(0.02), 1, 100, 100, None, None);
        assert_eq!(floor.usd, Some(0.02));
        assert!(floor.floor, "the unpriceable remainder makes it a floor");
        // The same two rows with a price for the second: an estimate, exact.
        let est = row_additive_cost(2, Some(0.02), 1, 100, 100, Some(1e-6), Some(1e-6));
        assert!((est.usd.unwrap() - 0.0202).abs() < 1e-9);
        assert!(!est.floor);
        // Everything reported.
        let all = row_additive_cost(2, Some(0.05), 2, 0, 0, None, None);
        assert_eq!((all.usd, all.floor), (Some(0.05), false));
        // Nothing known: unknown, and not a floor (there is nothing to be at least).
        let none = row_additive_cost(1, None, 0, 10, 10, None, None);
        assert_eq!((none.usd, none.floor), (None, false));
    }

    /// A reply that reported NO usage on a model the price cache can price would estimate to a
    /// confident $0. It is counted as a call PM couldn't price, and the line is a floor.
    #[test]
    fn a_reply_with_no_usage_on_a_priced_model_is_not_an_exact_zero() {
        let (_dir, conn) = temp_db();
        conn.execute(
            "INSERT INTO model_pricing(model, prompt_price, completion_price, fetched_at) \
             VALUES ('or/priced', 1e-6, 2e-6, strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
            [],
        )
        .unwrap();
        let silent = openrouter::Usage::default();
        log_usage(
            &conn,
            "chat",
            Some("or/priced"),
            &silent,
            &meta_via(Some(llm_gateway::CloudProvider::OpenRouter)),
        );
        let rows = spend_rows(&conn, false).unwrap();
        assert_eq!(rows[0].cost_usd, Some(0.0));
        assert_eq!((rows[0].unpriced_requests, rows[0].partial), (1, true));
        assert_eq!(
            detail_row(&conn, "or/priced").5.as_deref(),
            Some("no_usage")
        );
    }

    /// The same two gaps on a MOD's line (a reply with no usage, and a reply that counted one side
    /// only) on a model the cache can price: each would estimate to an exact-looking figure, so the
    /// mod's line says "at least" like the by-model table does.
    #[test]
    fn a_mods_line_is_partial_for_a_reply_with_no_usage_or_one_side_only() {
        let (_dir, conn) = temp_db();
        conn.execute(
            "INSERT INTO model_pricing(model, prompt_price, completion_price, fetched_at) \
             VALUES ('or/priced', 1e-6, 2e-6, strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
            [],
        )
        .unwrap();
        let log = |mod_id: &str, p: Option<i64>, c: Option<i64>| {
            conn.execute(
                "INSERT INTO usage_log(model, kind, prompt_tokens, completion_tokens, mod_id, provider) \
                 VALUES ('or/priced', 'background', ?1, ?2, ?3, 'cloud')",
                params![p, c, mod_id],
            )
            .unwrap();
        };
        log("a.counted", Some(10), Some(10));
        log("a.silent", None, None);
        log("a.one_side", Some(10), None);
        let rows = spend_by_mod(&conn).unwrap();
        let of = |id: &str| rows.iter().find(|m| m.mod_id == id).unwrap();
        assert!(!of("a.counted").partial);
        assert!(of("a.silent").partial, "no usage is not an exact $0");
        assert!(of("a.one_side").partial, "one side is an under-count");
        // The by-model table agrees on the one-sided OpenRouter row.
        conn.execute(
            "INSERT INTO usage_log(model, kind, prompt_tokens, completion_tokens, provider, served_provider) \
             VALUES ('or/priced', 'chat', 10, NULL, 'cloud', 'openrouter')",
            [],
        )
        .unwrap();
        // Pick the OpenRouter-served line by name: the mod rows above have no served company and form
        // their own, larger line, whose silent row already trips the count, so taking the first line
        // would pass without the one-sided clause.
        let line = spend_rows(&conn, false)
            .unwrap()
            .into_iter()
            .find(|r| r.served.as_deref() == Some("openrouter"))
            .unwrap();
        assert!(line.partial && line.unpriced_requests == 1);
    }

    /// A row that carries a cost AND a `partial_usage` note (an OpenAI 12-hour cache write is priced
    /// at the one write rate) is a known under-count, so the line and the mod's line both say "at
    /// least" even though no cost is missing. The note alone makes it so, for every provider.
    #[test]
    fn a_priced_row_with_a_partial_usage_note_is_still_a_floor() {
        let (_dir, conn) = temp_db();
        let log = |model: &str, mod_id: Option<&str>, note: Option<&str>| {
            conn.execute(
                "INSERT INTO usage_log(model, kind, prompt_tokens, completion_tokens, cost_usd, \
                    cost_note, mod_id, provider, served_provider) \
                 VALUES (?1, 'chat', 10, 10, 0.02, ?2, ?3, 'cloud', 'openai')",
                params![model, note, mod_id],
            )
            .unwrap();
        };
        log("x/noted", None, Some("partial_usage"));
        log("x/plain", None, None);
        log("x/mod", Some("a.noted"), Some("partial_usage"));
        log("x/mod2", Some("a.plain"), None);
        let rows = spend_rows(&conn, false).unwrap();
        let of = |m: &str| rows.iter().find(|r| r.model == m).unwrap();
        assert_eq!(of("x/noted").cost_usd, Some(0.02));
        assert!(of("x/noted").partial);
        assert_eq!(of("x/noted").unpriced_requests, 1);
        assert!(!of("x/plain").partial && of("x/plain").unpriced_requests == 0);
        let mods = spend_by_mod(&conn).unwrap();
        let m = |id: &str| mods.iter().find(|m| m.mod_id == id).unwrap();
        assert!(m("a.noted").partial, "the mod's line reads the note too");
        assert!(!m("a.plain").partial);
        // The named warnings list token types only, so a partial_usage row adds no warning.
        assert!(partial_reasons(&conn).unwrap().is_empty());
    }

    /// A restored backup can hold any string in `served_provider`; the line carries only a company
    /// PM knows, and an odd value reads as "before PM recorded this" rather than reaching the page.
    #[test]
    fn an_odd_served_provider_never_reaches_the_page() {
        let (_dir, conn) = temp_db();
        conn.execute(
            "INSERT INTO usage_log(model, kind, prompt_tokens, completion_tokens, cost_usd, provider, served_provider) \
             VALUES ('a/m', 'chat', 1, 1, 0.01, 'cloud', '<b>evil</b>')",
            [],
        )
        .unwrap();
        let rows = spend_rows(&conn, false).unwrap();
        assert_eq!(rows[0].served, None);
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
            ..Default::default()
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

    /// A call on the person's own Anthropic key logs `anthropic/<catalogue id>` with PM's own worked
    /// out cost. The summary sums that cost as it does an OpenRouter-reported one, with NO price-cache
    /// row (`model_pricing` is OpenRouter's catalogue and holds nothing for it) and no network:
    /// `build_cost_summary` is a read of `usage_log` alone. A call with no cost (a stream that never
    /// reported its output) stays a dash, never a $0.
    #[test]
    fn an_anthropic_direct_row_sums_its_own_cost_with_no_openrouter_price() {
        let (_dir, conn) = temp_db();
        let usage = |cost: Option<f64>| openrouter::Usage {
            prompt_tokens: Some(1000),
            completion_tokens: Some(200),
            cost,
            ..Default::default()
        };
        let sonnet = "anthropic/claude-sonnet-5-5";
        let via = meta_via(Some(llm_gateway::CloudProvider::Anthropic));
        log_usage(&conn, "chat", Some(sonnet), &usage(Some(0.0125)), &via);
        log_usage(&conn, "chat", Some(sonnet), &usage(Some(0.0075)), &via);
        log_usage(
            &conn,
            "background",
            Some("anthropic/claude-haiku-5-5"),
            &usage(None),
            &via,
        );
        let priced: i64 = conn
            .query_row("SELECT COUNT(*) FROM model_pricing", [], |r| r.get(0))
            .unwrap();
        assert_eq!(priced, 0, "nothing from OpenRouter is in the price cache");

        let summary = build_cost_summary(&conn).unwrap();
        let row = |m: &str| summary.all_time.iter().find(|r| r.model == m).unwrap();
        let s = row(sonnet);
        assert_eq!((s.request_count, s.prompt_tokens), (2, 2000));
        assert!((s.cost_usd.unwrap() - 0.02).abs() < 1e-9);
        assert_eq!(row("anthropic/claude-haiku-5-5").cost_usd, None);
        assert!((summary.total_all_time_usd.unwrap() - 0.02).abs() < 1e-9);
        assert!((summary.total_30d_usd.unwrap() - 0.02).abs() < 1e-9);

        // The 0.02 total skips the Haiku line, which has no figure: the totals are floors, and
        // the page must say "at least" and count the call it couldn't price.
        assert!(!s.partial, "the Sonnet line is exact");
        assert!(row("anthropic/claude-haiku-5-5").partial);
        assert_eq!(s.served.as_deref(), Some("anthropic"));
        assert!(summary.total_all_time_partial);
        assert!(summary.total_30d_partial);
        assert_eq!(summary.unpriced_30d, 1);
    }

    /// The Usage line for a list's walks counts only `list_...` replies from the last 30 days, names
    /// the EARLIEST one's reason and date in the class table's words, and never counts a local
    /// failure, a power route, or an `_`-wildcard look-alike.
    #[test]
    fn the_usage_line_counts_a_lists_walks_and_nothing_that_only_looks_like_one() {
        let (_dir, conn) = temp_db();
        assert_eq!(list_walks(&conn).unwrap(), None);
        let log = |reason: &str, at: &str| {
            conn.execute(
                "INSERT INTO usage_log(model, kind, fallback_reason, created_at) \
                 VALUES ('m/x', 'chat', ?1, ?2)",
                params![reason, at],
            )
            .unwrap();
        };
        let ago = |days: i64| -> String {
            conn.query_row(
                "SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now', ?1)",
                [format!("-{days} days")],
                |r| r.get(0),
            )
            .unwrap()
        };
        log("hard_failure:timeout", &ago(2));
        log("power_policy", &ago(2));
        log("cooldown", &ago(2));
        log("listX_busy", &ago(2));
        assert_eq!(
            list_walks(&conn).unwrap(),
            None,
            "none of those is a list's walk"
        );
        log("list_busy", &ago(40));
        assert_eq!(list_walks(&conn).unwrap(), None, "older than 30 days");
        let three = ago(3);
        log("list_no_credit", &three);
        log("list_skip_down", &ago(1));
        let walks = list_walks(&conn).unwrap().unwrap();
        assert_eq!(walks.replies, 2);
        assert_eq!(
            walks.first_reason,
            crate::error::FailureClass::NoCredit.why()
        );
        assert_eq!(walks.first_at, three);
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

    /// The Usage and cost totals are everything PM spent, the mods' calls included: a mod's rows
    /// are in the per-model lists and in both totals, with `by_mod` as the breakdown of the part
    /// that was theirs.
    #[test]
    fn the_totals_include_what_mods_spent() {
        let (_dir, conn) = temp_db();
        let log = |mod_id: Option<&str>, cost: f64| {
            conn.execute(
                "INSERT INTO usage_log(model, kind, prompt_tokens, completion_tokens, cost_usd, mod_id) \
                 VALUES ('a/m', 'background', 10, 10, ?1, ?2)",
                params![cost, mod_id],
            )
            .unwrap();
        };
        log(None, 1.0);
        log(Some("acme.tool"), 0.25);
        log(Some("acme.other"), 0.5);
        let summary = build_cost_summary(&conn).unwrap();
        assert!((summary.total_all_time_usd.unwrap() - 1.75).abs() < 1e-9);
        assert!((summary.total_30d_usd.unwrap() - 1.75).abs() < 1e-9);
        assert_eq!(summary.all_time[0].request_count, 3);
        let theirs: f64 = summary.by_mod.iter().filter_map(|m| m.cost_usd).sum();
        assert!((theirs - 0.75).abs() < 1e-9);
    }
}
