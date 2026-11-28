// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Streaming chat against OpenRouter (spec §6 — one key, any model, swappable).
//! The API key is read from the keychain on the Rust side and never reaches the
//! webview.
//!
//! The request, the stream loop and the one-shot completion are the OpenAI dialect and live in
//! `cloud_openai.rs`; what is OpenRouter's own — its endpoint, the ZDR pin, the `models` fallback
//! list, its error copy and its public catalogue — lives here, as [`FLAVOUR`].

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::cloud_openai::{self, Flavour};
use crate::error::{Error, FailureClass, Result};

const ENDPOINT: &str = "https://openrouter.ai/api/v1/chat/completions";
const MODELS_ENDPOINT: &str = "https://openrouter.ai/api/v1/models";
/// The authoritative list of Zero-Data-Retention endpoints, keyed by `model_id`. Public, like
/// the catalogue. This is what lets PM filter the picker to models it can actually serve
/// rather than only failing closed at request time.
const ZDR_ENDPOINT: &str = "https://openrouter.ai/api/v1/endpoints/zdr";

// The HTTP client every cloud call shares, and the rule that no request carries an app-attribution
// header or a `User-Agent`, are in `cloud_openai.rs`; the catalogue and ZDR fetches below go through
// the same client, and `no_request_carries_app_attribution` holds the rule here too.

#[derive(Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// Bound *and* single-line one untrusted field on its way into a prompt.
///
/// Two jobs, and they are the same job. The **clip** is sizing: a title is a filename, filenames run
/// to 255 characters, and a prompt built from four hundred of them is a prompt no local server will
/// hold. The **collapse** is what makes clipping correct: every prompt that carries these fields
/// builds them into a line-oriented block — `=== Document N ===`, `Title: …`, one title per line —
/// so an embedded CR/LF in a value forges a structural line the model reads as PM's own framing. A
/// document title is untrusted (an HTML `<title>`, PDF metadata, a filename in a shared Drive
/// folder) and nothing sanitises it between ingest and the prompt: `ingest::yaml_quote` collapses on
/// the way into the vault manifest, not on the way into `documents.title`.
///
/// Only for fields that are supposed to be ONE line. Document bodies are not, and are handled by the
/// untrusted-data framing instead.
pub fn clip_prompt_line(s: &str, max: usize) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Token usage OpenRouter reports for one completion. Any field may be absent
/// (a provider that doesn't report usage, or a degraded/early-terminated response),
/// so the cost logger stores NULLs and shows the spend as unknown rather than zero.
/// `cost` is OpenRouter's own usage-accounting figure (actual USD charged, net of any
/// prompt-cache discount) — preferred over the local tokens × price estimate when present.
#[derive(Clone, Copy, Default, Debug)]
pub struct Usage {
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub cost: Option<f64>,
    /// The part of the prompt the provider read from its cache, and the part it wrote to it. Only a
    /// provider that prices its own cache reports them (OpenAI direct; OpenRouter folds them into
    /// `cost`), and only `openai_direct::cost_usd` reads them.
    pub cached_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    /// The provider reported a prompt count but no total, so the output count may leave its thinking
    /// out (Gemini counts thoughts in a total, not in `completion_tokens`). A price worked out from
    /// such a count would look exact and under-count, so the cost reads "-" instead. False for every
    /// provider that reports output whole. Anthropic also sets it for a reply that wrote one-hour
    /// cache entries, which PM has no price for: the same "can't be priced exactly" dash.
    pub output_partial: bool,
    /// The provider's safety system stopped this reply (Anthropic's `stop_reason: "refusal"`). It is
    /// HTTP 200 and BILLED, so it travels as a completion with its usage and an empty text, not as an
    /// `Err` (which carries no usage and would book nothing). It sits on `Usage` rather than on
    /// `Completion` so the many `Completion { .. }` literals (and the mod tests, which this change
    /// may not touch) keep compiling: every `Usage` outside the dialect builders is built from
    /// `..Default::default()`. False for every provider that never says it.
    pub refused: bool,
    /// The part of `completion_tokens` the model spent thinking, when the provider says so (OpenAI's
    /// `reasoning_tokens`, Anthropic's `thinking_tokens`, Gemini's `total - prompt - completion`).
    /// Information only: thinking is already inside `completion_tokens` and priced as output, so
    /// nothing may add it to a cost again.
    pub thinking_tokens: Option<i64>,
    /// Why the cost on this reply is missing or partial, when an adapter saw a reason (a token type
    /// PM has no price for). The logger adds the reasons it can derive itself ([`CostNote`]).
    pub cost_note: Option<CostNote>,
}

/// Why a reply's cost is missing or partial. A FIXED list of slugs: it is what the log stores
/// (`usage_log.cost_note`) and what the Usage tab names, so a vendor's own words, which are
/// untrusted text, can never reach the log or a page. A usage key PM has never heard of is
/// `NewTokenType`; its name is not kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CostNote {
    /// One side of the count is missing or can't be priced exactly (one-hour cache writes, a total
    /// with no prompt count): the cost reads "-" rather than a confident under-count.
    PartialUsage,
    /// An answer that carried no usage at all.
    NoUsage,
    AudioTokens,
    PredictionTokens,
    /// Images or audio in the prompt, which some providers bill at their own rate.
    NonTextPrompt,
    /// Anthropic's server-side tools (web search and the like), billed per use.
    ServerToolUse,
    /// A numeric usage key in none of the lists in `provider_usage_keys.json`.
    NewTokenType,
}

impl CostNote {
    /// Every note, in the order the Usage tab lists them.
    pub const ALL: [CostNote; 7] = [
        CostNote::PartialUsage,
        CostNote::NoUsage,
        CostNote::AudioTokens,
        CostNote::PredictionTokens,
        CostNote::NonTextPrompt,
        CostNote::ServerToolUse,
        CostNote::NewTokenType,
    ];

    /// The slug stored in `usage_log.cost_note` and sent to the page.
    pub fn as_str(self) -> &'static str {
        match self {
            CostNote::PartialUsage => "partial_usage",
            CostNote::NoUsage => "no_usage",
            CostNote::AudioTokens => "audio_tokens",
            CostNote::PredictionTokens => "prediction_tokens",
            CostNote::NonTextPrompt => "non_text_prompt",
            CostNote::ServerToolUse => "server_tool_use",
            CostNote::NewTokenType => "new_token_type",
        }
    }

    /// The note a stored slug names; `None` for anything else, so an odd value in an old or edited
    /// store is read as no note, never shown.
    pub fn from_slug(slug: &str) -> Option<CostNote> {
        CostNote::ALL.into_iter().find(|n| n.as_str() == slug)
    }

    /// Whether this note names a token type PM can't price (the Usage tab warns about those by
    /// name). `PartialUsage` and `NoUsage` are gaps in what the provider reported instead.
    pub fn is_unpriced_type(self) -> bool {
        !matches!(self, CostNote::PartialUsage | CostNote::NoUsage)
    }
}

/// The result of a chat call: the assembled text, the model that actually served it
/// (may differ from the requested primary when an auto-switch fallback fires), and
/// token usage when reported. Shared by the streaming and non-streaming paths so the
/// cost logger can attribute spend to the model that actually ran.
pub struct Completion {
    pub text: String,
    pub model: Option<String>,
    pub usage: Usage,
    /// The model stopped because it hit its token ceiling (`finish_reason == "length"`), not because
    /// it had finished. The text is real and worth keeping — but it is not a finished answer, and
    /// this was previously dropped on the floor, so a reply that trailed off mid-sentence was stored
    /// as a complete turn and later quoted as one. Not an error: the caller marks the turn honest.
    pub truncated: bool,
}

impl Completion {
    /// The reply, but only when it can be treated as a FINISHED answer — `None` otherwise.
    ///
    /// There are two ways a reply is not one, and to every parser in this tree they look identical
    /// to a reply that simply had nothing to say:
    ///
    ///   * the model hit its token ceiling mid-sentence (`truncated`), so the JSON has no closing
    ///     bracket and the prose has no last clause; or
    ///   * it returned nothing at all.
    ///
    /// Both arrive as HTTP 200 with text attached. Every structured parser here is deliberately
    /// defensive — `parse_chat_preferences`, `parse_assignments`, `parse_vocabulary`, `parse_batch`
    /// all degrade to an empty result rather than raise — which is right against a reliable model
    /// and wrong against an unreliable one, because the callers then write that empty result down as
    /// a real finding and advance a cursor past the turns it came from. Two outcomes PM could not
    /// tell apart: "I read those forty turn-pairs and there was nothing worth learning", and "I lost
    /// the thread". Only the first should ever be recorded.
    ///
    /// This is the one check that separates them, and it belongs before the parse rather than
    /// inside it: the parsers are pure functions over a string and cannot see `finish_reason`.
    ///
    /// It does NOT catch a prompt cut at the FRONT — a `--context-shift` server answers `stop` and
    /// reports its `prompt_tokens` after the cut, so nothing in the response says it happened. That
    /// is what the pre-flight sizing in #797 is for; the two are complementary halves of "the reply
    /// PM got back is not the reply it asked for".
    pub fn usable_text(&self) -> Option<&str> {
        if self.truncated || self.usage.refused {
            return None;
        }
        let text = self.text.trim();
        (!text.is_empty()).then_some(text)
    }

    /// Why [`Self::usable_text`] said no, phrased for a log line or an error the user may see.
    /// `None` when the reply was usable.
    pub fn unusable_reason(&self) -> Option<&'static str> {
        // Checked first: a refused reply is empty AND may be cut, and "the model returned an empty
        // reply" would hide that the provider's safety system stopped it.
        if self.usage.refused {
            Some("Anthropic's safety system stopped this reply")
        } else if self.truncated {
            Some("the model's reply was cut off before it finished")
        } else if self.text.trim().is_empty() {
            Some("the model returned an empty reply")
        } else {
            None
        }
    }
}

/// One model from OpenRouter's public catalogue, trimmed to what the Settings
/// picker needs: identity, pricing, context window, and input modalities (so the
/// UI can tag vision-capable models). Prices are USD **per token**; the frontend
/// renders them per-million.
#[derive(Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub context_length: Option<u64>,
    pub prompt_price: Option<f64>,
    pub completion_price: Option<f64>,
    pub input_modalities: Vec<String>,
}

/// The fuller per-model record the daily price refresh caches. On top of pricing it carries
/// the prompt-cache read rate (PM reuses stable prompt prefixes, so cache reads dominate
/// effective cost), the model's `supported_parameters`, and the Artificial-Analysis
/// `intelligence_index` from the catalogue's `benchmarks` block — a *general-capability*
/// signal, NOT a faithfulness metric. Every field is optional because the catalogue is
/// sparse: most models carry no benchmarks, and some report no price.
///
/// The last three fields are write-only today (populated by the price refresh, surfaced in
/// the dev inspector) — they fed the model recommender, removed in v3.18.0-alpha. They are
/// kept because migration v8's columns are append-only and the dev inspector reads them.
#[derive(Clone, Debug, Default)]
pub struct ModelDetail {
    pub id: String,
    pub name: String,
    pub description: String,
    pub context_length: Option<u64>,
    pub prompt_price: Option<f64>,
    pub completion_price: Option<f64>,
    pub cache_read_price: Option<f64>,
    pub input_modalities: Vec<String>,
    pub supported_parameters: Vec<String>,
    pub intelligence_index: Option<f64>,
}

/// Fetch the full OpenRouter model catalogue as the richer [`ModelDetail`] the price cache
/// needs. This endpoint is public (no API key required), but we still go through Rust so the
/// webview never talks to OpenRouter directly. Sorted newest-first by OpenRouter; we preserve
/// order. **Unfiltered** — [`list_models`] applies the ZDR filter for the picker.
pub async fn fetch_catalogue() -> Result<Vec<ModelDetail>> {
    #[derive(Deserialize)]
    struct Resp {
        data: Vec<RawModel>,
    }
    #[derive(Deserialize)]
    struct RawModel {
        id: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        description: String,
        #[serde(default)]
        context_length: Option<u64>,
        #[serde(default)]
        pricing: Option<RawPricing>,
        #[serde(default)]
        architecture: Option<RawArch>,
        #[serde(default)]
        supported_parameters: Vec<String>,
        // Sparse and schema-loose across models (present on ~1 in 7), so parsed as a free
        // Value and probed defensively rather than against a fixed struct.
        #[serde(default)]
        benchmarks: Option<serde_json::Value>,
    }
    #[derive(Deserialize)]
    struct RawPricing {
        // OpenRouter sends prices as decimal strings, e.g. "0.000003".
        prompt: Option<String>,
        completion: Option<String>,
        // The prompt-cache read rate, where the model supports prompt caching.
        input_cache_read: Option<String>,
    }
    #[derive(Deserialize)]
    struct RawArch {
        #[serde(default)]
        input_modalities: Vec<String>,
    }

    let response = cloud_openai::http()
        .get(MODELS_ENDPOINT)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let detail = crate::error::truncate_detail(&response.text().await.unwrap_or_default());
        return Err(Error::Other(format!(
            "OpenRouter models request failed ({status}): {detail}"
        )));
    }

    let parsed: Resp = response.json().await?;
    let models = parsed
        .data
        .into_iter()
        .map(|m| {
            let (prompt_price, completion_price, cache_read_price) = match m.pricing {
                Some(p) => (
                    parse_price(p.prompt),
                    parse_price(p.completion),
                    parse_price(p.input_cache_read),
                ),
                None => (None, None, None),
            };
            let intelligence_index = parse_intelligence_index(m.benchmarks.as_ref());
            ModelDetail {
                id: m.id,
                name: m.name,
                description: m.description,
                context_length: m.context_length,
                prompt_price,
                completion_price,
                cache_read_price,
                input_modalities: m
                    .architecture
                    .map(|a| a.input_modalities)
                    .unwrap_or_default(),
                supported_parameters: m.supported_parameters,
                intelligence_index,
            }
        })
        .collect();
    Ok(models)
}

/// Pull the Artificial-Analysis `intelligence_index` out of a model's sparse `benchmarks`
/// block, tolerating absence and non-numeric values (→ None). A general-capability proxy
/// only — the API has no faithfulness metric.
fn parse_intelligence_index(benchmarks: Option<&serde_json::Value>) -> Option<f64> {
    benchmarks
        .and_then(|b| b.get("artificial_analysis"))
        .and_then(|aa| aa.get("intelligence_index"))
        .and_then(|v| v.as_f64())
}

/// Fetch the set of model ids that have at least one Zero-Data-Retention endpoint.
///
/// `chat_body` pins `zdr: true` on every request, which makes a model with no ZDR endpoint
/// **uncallable** rather than merely less private — so this is the set PM can actually serve.
/// Public/unauthenticated, like the catalogue. Match key is `model_id`.
async fn fetch_zdr_model_ids() -> Result<HashSet<String>> {
    #[derive(Deserialize)]
    struct Resp {
        data: Vec<RawEndpoint>,
    }
    #[derive(Deserialize)]
    struct RawEndpoint {
        model_id: String,
    }

    let response = cloud_openai::http()
        .get(ZDR_ENDPOINT)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let detail = crate::error::truncate_detail(&response.text().await.unwrap_or_default());
        return Err(Error::Other(format!(
            "OpenRouter ZDR endpoint request failed ({status}): {detail}"
        )));
    }

    let parsed: Resp = response.json().await?;
    Ok(parsed.data.into_iter().map(|e| e.model_id).collect())
}

/// True when a model id is one of OpenRouter's **router pseudo-models** (`openrouter/free`,
/// `openrouter/auto`, …). These never appear in the ZDR endpoint feed because they aren't
/// concrete endpoints — they pick an underlying model per request, at which point our
/// `provider: {zdr, data_collection}` pin applies to whatever they resolve to. So they are
/// safe under PM's enforcement and must survive the filter; dropping them would remove
/// `openrouter/free` (a real, in-use model) from the picker.
fn is_router_model(id: &str) -> bool {
    id.starts_with("openrouter/")
}

/// Keep only models PM can actually serve: those with a ZDR endpoint, plus the routers.
/// Pure so the policy is unit-testable without the network.
fn retain_zdr_servable(models: Vec<ModelDetail>, zdr_ids: &HashSet<String>) -> Vec<ModelDetail> {
    models
        .into_iter()
        .filter(|m| zdr_ids.contains(&m.id) || is_router_model(&m.id))
        .collect()
}

/// Fetch the catalogue trimmed to the [`ModelInfo`] the Settings model picker shows,
/// **filtered to what PM's privacy enforcement can actually serve** (spec §6).
///
/// PM pins `zdr: true` + `data_collection: "deny"` on every request ([`chat_body`]), so a
/// model with no compliant endpoint doesn't degrade — it 404s. Offering such a model in the
/// picker is therefore offering a broken choice, which is exactly how the removed recommender
/// could serve `anthropic/claude-fable-5` (6 endpoints, 0 ZDR). Filtering here makes that
/// class of failure unreachable from the list.
///
/// **Fails closed**: if the ZDR list can't be fetched we return the error rather than fall
/// back to the unfiltered catalogue — showing every model on a network blip would silently
/// reintroduce the non-servable ones. The picker surfaces the error; typing a custom model id
/// still works (spec §6 — PM is never locked to the catalogue), and a non-compliant one fails
/// closed at request time with the hint from [`request_error`].
///
/// Only the ZDR axis is filtered on, not `data_collection`. Verified live (15-07-2026): all
/// 42 ZDR-serving providers also report `training: false`, so the deny axis excludes nothing
/// the ZDR axis hasn't already — and the per-request pin enforces both regardless. That keeps
/// this on documented, stable API surface (`/api/v1/endpoints/zdr`) rather than the
/// undocumented frontend provider feed.
pub async fn list_models() -> Result<Vec<ModelInfo>> {
    let (catalogue, zdr_ids) = tokio::try_join!(fetch_catalogue(), fetch_zdr_model_ids())?;
    Ok(retain_zdr_servable(catalogue, &zdr_ids)
        .into_iter()
        .map(|m| ModelInfo {
            id: m.id,
            name: m.name,
            description: m.description,
            context_length: m.context_length,
            prompt_price: m.prompt_price,
            completion_price: m.completion_price,
            input_modalities: m.input_modalities,
        })
        .collect())
}

/// Parse a price string ("0.000003") into a float, discarding anything that isn't
/// a real non-negative number (OpenRouter uses "-1" / absent for "not priced").
fn parse_price(s: Option<String>) -> Option<f64> {
    s.and_then(|s| s.parse::<f64>().ok()).filter(|v| *v >= 0.0)
}

/// Turn a failed chat-completions response into an error, adding an actionable hint when
/// the failure looks like "no provider meets PM's zero-data-retention requirement". PM
/// enforces ZDR on every request (see `chat_body`), so a model with no compliant endpoint
/// fails here rather than leaking data — but without the hint the user would see only an
/// opaque 404/no-endpoints string and not realise the *model* (not PM) is the problem.
fn request_error(status: reqwest::StatusCode, detail: &str) -> Error {
    // The core hands over the raw bounded body now; shortening it here keeps the text identical.
    let detail = &crate::error::truncate_detail(detail);
    let lower = detail.to_lowercase();
    let zdr_related = lower.contains("data policy")
        || lower.contains("data_collection")
        || lower.contains("no endpoints")
        || lower.contains("no allowed providers")
        || lower.contains("zero data")
        || lower.contains("zdr");
    let message = if zdr_related {
        format!(
            "OpenRouter request failed ({status}): {detail}. PM enforces zero-data-retention on \
             every request, so this model may have no compliant provider — pick another model, or \
             turn on auto-switch with a compliant fallback."
        )
    } else {
        format!("OpenRouter request failed ({status}): {detail}")
    };
    // A 404 "no endpoints match your data policy" is PM's zero-data-retention pin refusing the model,
    // not the model being gone: it keeps its own guidance sentence (above) as a plain error rather
    // than a card that says "OpenRouter no longer offers this model" and hides the hint.
    let class = if zdr_related {
        FailureClass::Other
    } else {
        failure_class(status.as_u16(), &lower)
    };
    Error::provider(
        crate::llm_gateway::CloudProvider::OpenRouter,
        class,
        message,
    )
}

/// The class of an OpenRouter failure, from its status (OpenRouter's errors are strings only) and,
/// for a 400, the words that say the conversation is too long. PURE. The status is advisory for a
/// mid-stream error (the transport said 200), which is fine: the class is wording, and billing is
/// never decided here.
fn failure_class(status: u16, lower_detail: &str) -> FailureClass {
    match status {
        401 => FailureClass::KeyBad,
        402 => FailureClass::NoCredit,
        404 => FailureClass::ModelGone,
        413 => FailureClass::ContextTooLong,
        429 => FailureClass::RateLimited,
        400 if lower_detail.contains("context length")
            || lower_detail.contains("maximum context")
            || lower_detail.contains("too long") =>
        {
            FailureClass::ContextTooLong
        }
        408 | 500..=599 => FailureClass::Busy,
        _ => FailureClass::Other,
    }
}

/// Build the request body, picking single-model vs. fallback-routing form. With
/// one model we send `"model"`; with several we send `"models"` (an ordered
/// fallback list — OpenRouter advances to the next on a rate-limit/quota/provider
/// error, which is how auto-switch works). Callers guarantee a non-empty list.
///
/// `max_tokens` is `None` for every caller PM has had: the body is then exactly what it always was
/// (a test holds it byte for byte). A mod's `ai.complete` is the one caller that sets it, because
/// the worst-case price PM books before the call is only a bound if the answer is bounded.
fn chat_body(
    models: &[String],
    messages: &[ChatMessage],
    stream: bool,
    cache_through: Option<usize>,
    max_tokens: Option<u32>,
) -> serde_json::Value {
    cloud_openai::build_body(
        &FLAVOUR,
        models,
        messages,
        stream,
        cache_through,
        max_tokens,
    )
}

/// OpenRouter's flavour of the OpenAI dialect. OpenRouter forwards `cache_control` to providers that
/// support prompt caching (e.g. Anthropic), which cache everything up to AND INCLUDING the marked
/// message; providers without caching, or a prefix below their minimum cacheable size, simply ignore
/// it.
pub(crate) const FLAVOUR: Flavour = Flavour {
    name: "OpenRouter",
    endpoint: ENDPOINT,
    auth: cloud_openai::bearer,
    cache_control_blocks: true,
    decorate,
    request_error,
    max_tokens_field: "max_tokens",
    patient: false,
    stream_error: None,
    stop_error: None,
    usage: None,
};

/// The fields only OpenRouter's body carries: usage accounting, how the model(s) are named, and the
/// privacy pin. With one model we send `"model"`; with several we send `"models"` (an ordered
/// fallback list — OpenRouter advances to the next on a rate-limit/quota/provider error, which is
/// how auto-switch works).
fn decorate(body: &mut serde_json::Value, models: &[String]) {
    // Usage accounting: have OpenRouter report the actual USD cost (net of any prompt-cache
    // discount) so the cost logger can store real spend, not just a tokens × price estimate.
    body["usage"] = serde_json::json!({ "include": true });
    if models.len() == 1 {
        body["model"] = serde_json::json!(models[0]);
    } else {
        body["models"] = serde_json::json!(models);
    }
    // PRIVACY (spec §6) — enforce Zero-Data-Retention on EVERY request, whichever model the
    // user picked. `zdr: true` keeps the request on endpoints that don't retain prompts;
    // `data_collection: "deny"` blocks providers that train on / store data. Two distinct
    // axes: some providers don't train but do retain (abuse-scanning/legal), so neither
    // implies the other and we pin both.
    //
    // OpenRouter combines these with the account-level policy using OR semantics — a
    // per-request flag can only *add* enforcement, never weaken it — so this is safe
    // regardless of the user's account config and is the real privacy boundary. It stays
    // the boundary even though `list_models` now filters the picker against
    // /api/v1/endpoints/zdr: that filter is reachability (don't offer a model we can't
    // serve), while this pin is enforcement, and it still covers ids that bypass the picker
    // — a custom-typed model, a stored id, DEFAULT_MODEL, or whatever a router resolves to.
    // (The catalogue's model objects still carry no data-policy field; /endpoints/zdr is a
    // separate feed, keyed by model_id.) If no compliant endpoint exists
    // for a model the request fails closed (privacy-preserving) and auto-switch falls
    // through to the next model in the list.
    body["provider"] = serde_json::json!({ "zdr": true, "data_collection": "deny" });
}

/// POST a streaming chat completion. `on_token` is called with each text delta as
/// it arrives. Returns the assembled reply plus the model that actually served it
/// (which can differ from the first requested model when a fallback fires).
pub async fn stream_chat<F>(
    api_key: &str,
    models: &[String],
    messages: &[ChatMessage],
    cache_through: Option<usize>,
    on_token: F,
) -> Result<Completion>
where
    F: FnMut(&str),
{
    // `cache_through` marks the end of the stable system prefix (the profile/agenda/rolling-summary
    // block, card 7C) so providers cache it and bill it cheaply turn after turn; the per-turn grounding
    // and growing history sit after it and stay uncached. `None` (no stable block) preserves the old
    // fully-uncached behaviour.
    let body = chat_body(models, messages, true, cache_through, None);
    cloud_openai::stream(&FLAVOUR, api_key, &body, on_token).await
}

/// Drain every complete `\n`-terminated line from a raw SSE byte buffer, decoding
/// each whole line as UTF-8 (lossy) and trimming it. Incomplete trailing bytes
/// stay in `buffer` for the next chunk, so a multi-byte char straddling a chunk
/// boundary is decoded once, intact, rather than as two replacement characters.
pub(crate) fn drain_lines(buffer: &mut Vec<u8>) -> Vec<String> {
    let mut lines = Vec::new();
    while let Some(newline) = buffer.iter().position(|&b| b == b'\n') {
        let line_bytes: Vec<u8> = buffer.drain(..=newline).collect();
        lines.push(String::from_utf8_lossy(&line_bytes).trim().to_string());
    }
    lines
}

/// A single non-streaming chat completion — used for background work (sorting
/// proposals, the Learning-You profile) where we want the whole answer at once,
/// not a token stream. Takes an ordered model list (one model, or several for
/// auto-switch fallback). Returns the assistant message content. Set `cache_prefix`
/// when the first message is a stable prefix reused across many back-to-back calls
/// (the review loop) so providers cache + cheaply reuse it.
///
/// `max_tokens` bounds the answer's length when `Some`; every caller but a mod's `ai.complete`
/// passes `None` (through `llm_gateway::complete`), which sends the body this call always sent.
pub async fn complete(
    api_key: &str,
    models: &[String],
    messages: &[ChatMessage],
    cache_prefix: bool,
    max_tokens: Option<u32>,
) -> Result<Completion> {
    // Background callers cache from message 0 (their stable system instruction); `false` ⇒ no breakpoint.
    let body = chat_body(
        models,
        messages,
        false,
        cache_prefix.then_some(0),
        max_tokens,
    );

    // The send goes through `wire_mark::send` (in the shared core), so a mod's `ai.complete` can tell
    // a request that never connected from one that may have left.
    cloud_openai::complete(&FLAVOUR, api_key, &body).await
}

#[cfg(test)]
mod tests {
    use super::*;
    // The stream helpers moved to the shared core with the loop that uses them; these tests still
    // hold them, through OpenRouter's flavour.
    use crate::cloud_openai::{is_length_stop, parse_usage};

    fn chunk_error(value: &serde_json::Value) -> Option<Error> {
        cloud_openai::chunk_error(&FLAVOUR, value)
    }

    /// OpenRouter's app-attribution headers put an app in its public rankings and per-app analytics,
    /// so none may come back (see `cloud_openai::HTTP`). Read case-insensitively from this file, the only one
    /// that builds OpenRouter requests, with the names built from parts so the test can't match
    /// itself.
    #[test]
    fn no_request_carries_app_attribution() {
        let src = include_str!("openrouter.rs").to_ascii_lowercase();
        for header in [
            concat!("http-", "referer"),
            concat!("x-", "title"),
            concat!("x-open", "router-"),
            concat!("user-", "agent"),
        ] {
            assert!(!src.contains(&format!("\"{header}")), "{header} is back");
        }
        assert!(!src.contains(concat!("header::", "referer")));
        assert!(!src.contains(concat!("header::", "user_agent")));
        assert!(!src.contains(concat!(".user_", "agent(")));
    }

    fn completion(text: &str, truncated: bool) -> Completion {
        Completion {
            text: text.into(),
            model: None,
            usage: Usage::default(),
            truncated,
        }
    }

    #[test]
    fn a_finished_answer_and_a_failed_one_are_now_different_things() {
        // The pair PM could not tell apart. Both arrive as HTTP 200 with text attached, and every
        // structured parser in the tree degrades to an empty result rather than raising — so the
        // caller wrote both down as "there was nothing to find" and moved a cursor past the turns
        // they came from.
        assert_eq!(
            completion("[]", false).usable_text(),
            Some("[]"),
            "an empty ARRAY is a real answer — most conversations state no preference"
        );
        assert_eq!(
            completion("  - a bullet  ", false).usable_text(),
            Some("- a bullet"),
            "usable text comes back trimmed"
        );

        // Cut off at the token ceiling: the JSON has no closing bracket and the prose has no last
        // clause, but nothing about the response says so except this flag.
        assert_eq!(completion(r#"[{"scope":"glob"#, true).usable_text(), None);
        // Nothing at all.
        assert_eq!(completion("", false).usable_text(), None);
        assert_eq!(completion("   \n  ", false).usable_text(), None);

        // The reason is the part a user can act on, so it has to distinguish the two as well.
        assert!(completion("x", true)
            .unusable_reason()
            .unwrap()
            .contains("cut off"));
        assert!(completion(" ", false)
            .unusable_reason()
            .unwrap()
            .contains("empty"));
        assert_eq!(completion("fine", false).unusable_reason(), None);
    }

    #[test]
    fn a_prompt_cut_at_the_front_is_deliberately_not_caught_here() {
        // The complementary half, stated so nobody widens this into a claim it cannot make. A
        // server running with `--context-shift` discards the front of an over-long prompt, answers
        // `finish_reason: stop`, and reports its `prompt_tokens` AFTER the cut. The reply is a
        // complete, well-formed answer to a question PM did not ask, and there is nothing in the
        // response to detect it by — which is why the sizing check that prevents it has to run
        // BEFORE the request goes out.
        let answered_a_decapitated_prompt = completion(r#"{"assignments":[]}"#, false);
        assert!(
            answered_a_decapitated_prompt.usable_text().is_some(),
            "this check is about the REPLY; input truncation is unobservable from here"
        );
    }

    /// OpenRouter's errors are strings only, so the class comes from the status (and, for a 400, the
    /// words for "too long"). The sentence is the one it has always been.
    #[test]
    fn a_failure_is_told_as_a_class_from_its_status_and_keeps_its_sentence() {
        use FailureClass as C;
        for (status, detail, class) in [
            (401u16, "bad key", C::KeyBad),
            (402, "no credits", C::NoCredit),
            (404, "no such model", C::ModelGone),
            // The zero-data-retention pin refusing a model is not "gone": it keeps its guidance.
            (
                404,
                "No endpoints found matching your data policy",
                C::Other,
            ),
            (413, "too big", C::ContextTooLong),
            (429, "slow down", C::RateLimited),
            (
                400,
                "This model's maximum context length is 8192 tokens",
                C::ContextTooLong,
            ),
            (400, "bad field", C::Other),
            (408, "timeout", C::Busy),
            (502, "upstream provider is down", C::Busy),
            (503, "overloaded", C::Busy),
            (403, "moderation", C::Other),
        ] {
            let e = request_error(reqwest::StatusCode::from_u16(status).unwrap(), detail);
            let f = e.provider_failure().expect("typed");
            assert_eq!(f.class, class, "{status} {detail}");
            assert_eq!(f.provider, crate::llm_gateway::CloudProvider::OpenRouter);
            assert!(
                e.to_string()
                    .starts_with(&format!("OpenRouter request failed ({status}")),
                "{e}"
            );
        }
    }

    /// An error chunk inside a 200 stream is told by its code but may have billed: `chunk_error` never
    /// calls it `Rejected`, which only a response status before any body can.
    #[test]
    fn a_mid_stream_error_chunk_may_have_billed() {
        let e = chunk_error(&serde_json::json!({
            "error": {"code": 429, "message": "slow down"}
        }))
        .unwrap();
        let f = e.provider_failure().expect("typed");
        assert_eq!(f.class, FailureClass::RateLimited);
        assert_eq!(f.billing, crate::error::Billing::MaybeBilled);
    }

    /// The status table through the REAL request path, for OpenRouter's flavour.
    #[tokio::test]
    async fn a_failed_status_is_rejected_only_for_the_documented_unbilled_ones() {
        use crate::error::Billing::{MaybeBilled, Rejected};
        use std::io::{Read, Write};
        for (status, want) in [
            (400u16, Rejected),
            (401, Rejected),
            (402, Rejected),
            (403, Rejected),
            (404, Rejected),
            (413, Rejected),
            (422, Rejected),
            (429, Rejected),
            (503, Rejected),
            (529, Rejected),
            (408, MaybeBilled),
            (500, MaybeBilled),
            (502, MaybeBilled),
            (504, MaybeBilled),
            (524, MaybeBilled),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            std::thread::spawn(move || {
                let (mut sock, _) = listener.accept().unwrap();
                let mut seen = Vec::new();
                let mut buf = [0u8; 4096];
                while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = sock.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    seen.extend_from_slice(&buf[..n]);
                }
                sock.set_read_timeout(Some(std::time::Duration::from_millis(50)))
                    .ok();
                let _ = sock.read(&mut buf);
                let body = "upstream said no";
                let head = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: text/plain\r\nconnection: close\r\ncontent-length: {}\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes());
                let _ = sock.write_all(body.as_bytes());
            });
            let endpoint: &'static str =
                Box::leak(format!("http://127.0.0.1:{port}/").into_boxed_str());
            let flavour = Flavour {
                endpoint,
                name: FLAVOUR.name,
                auth: FLAVOUR.auth,
                cache_control_blocks: FLAVOUR.cache_control_blocks,
                decorate: FLAVOUR.decorate,
                request_error: FLAVOUR.request_error,
                max_tokens_field: FLAVOUR.max_tokens_field,
                patient: FLAVOUR.patient,
                stream_error: FLAVOUR.stream_error,
                stop_error: FLAVOUR.stop_error,
                usage: FLAVOUR.usage,
            };
            let body = chat_body(&["m/x".to_string()], &[], true, None, None);
            let e = cloud_openai::stream(&flavour, "test-key-not-real", &body, |_| {})
                .await
                .err()
                .expect("an error");
            assert_eq!(
                e.provider_failure().expect("typed").billing,
                want,
                "{status}"
            );
        }
    }

    #[test]
    fn a_mid_stream_error_chunk_is_an_error() {
        // The whole bug: the HTTP status is 200 and the failure arrives INSIDE the stream. Ignoring
        // it let the loop run out of chunks and return the truncated text as a success, which the
        // caller then persisted to messages + the vault + the index as a complete assistant turn.
        let chunk = serde_json::json!({
            "error": {"code": 502, "message": "upstream provider is down"}
        });
        let err = chunk_error(&chunk).expect("an error chunk must not be ignored");
        assert!(err.to_string().contains("upstream provider is down"));

        // A ZDR refusal arriving mid-stream gets the same actionable copy as one at request time —
        // that is the point of routing through `request_error` rather than a bare message.
        let zdr = serde_json::json!({
            "error": {"code": 404, "message": "No endpoints found matching your data policy"}
        });
        assert!(chunk_error(&zdr)
            .unwrap()
            .to_string()
            .contains("auto-switch"));

        // A malformed error object is still an error — never a silent success.
        assert!(chunk_error(&serde_json::json!({"error": {}})).is_some());
    }

    #[test]
    fn an_ordinary_chunk_is_not_an_error() {
        // The common case runs through this on every token; it must never false-positive.
        let chunk = serde_json::json!({
            "model": "x/y",
            "choices": [{"delta": {"content": "hello"}}]
        });
        assert!(chunk_error(&chunk).is_none());
        assert!(!is_length_stop(&chunk));
    }

    #[test]
    fn a_length_stop_is_flagged_but_other_reasons_are_not() {
        let length = serde_json::json!({"choices": [{"finish_reason": "length"}]});
        assert!(is_length_stop(&length), "hit the token ceiling mid-thought");

        // A normal end must not be marked — otherwise every complete reply gets the caveat.
        for reason in ["stop", "tool_calls", "content_filter"] {
            let v = serde_json::json!({"choices": [{"finish_reason": reason}]});
            assert!(!is_length_stop(&v), "{reason} is a normal end");
        }
        // Streaming chunks carry no finish_reason until the last one.
        assert!(!is_length_stop(
            &serde_json::json!({"choices": [{"delta": {}}]})
        ));
    }

    #[test]
    fn drain_lines_decodes_multibyte_split_across_chunks() {
        // "é" is 0xC3 0xA9; arrive with the two bytes in separate chunks.
        let mut buffer: Vec<u8> = Vec::new();
        buffer.extend_from_slice(b"data: caf\xc3");
        // No newline and the char is incomplete: nothing should be emitted yet,
        // and the lone 0xC3 must not be decoded into a replacement character.
        assert!(drain_lines(&mut buffer).is_empty());
        buffer.extend_from_slice(b"\xa9\n");
        assert_eq!(drain_lines(&mut buffer), vec!["data: café".to_string()]);
        assert!(buffer.is_empty());
    }

    #[test]
    fn drain_lines_yields_multiple_lines_and_keeps_partial_tail() {
        let mut buffer = b"data: a\ndata: b\ndata: c".to_vec();
        assert_eq!(
            drain_lines(&mut buffer),
            vec!["data: a".to_string(), "data: b".to_string()],
        );
        // The unterminated "data: c" is held back for the next chunk.
        assert_eq!(buffer, b"data: c");
    }

    #[test]
    fn parse_usage_reads_tokens_and_defaults_to_none() {
        // The final streamed usage chunk has empty choices + a usage object.
        let chunk: serde_json::Value = serde_json::from_str(
            r#"{"choices":[],"usage":{"prompt_tokens":123,"completion_tokens":45}}"#,
        )
        .unwrap();
        let u = parse_usage(&chunk);
        assert_eq!(u.prompt_tokens, Some(123));
        assert_eq!(u.completion_tokens, Some(45));
        // A chunk with no usage object → both None (cost shown as unknown, not zero).
        let plain: serde_json::Value =
            serde_json::from_str(r#"{"choices":[{"delta":{"content":"hi"}}]}"#).unwrap();
        let n = parse_usage(&plain);
        assert!(n.prompt_tokens.is_none() && n.completion_tokens.is_none());
    }

    /// Every caller PM had before a mod could ask for a bounded answer sends this body, byte for
    /// byte (keys are sorted, so the text is stable): a bound on the answer is a new field only a
    /// mod's call may add. The strings were taken from the code before `max_tokens` existed.
    #[test]
    fn every_request_body_that_asks_for_no_bound_is_what_it_always_was() {
        let msgs = [
            ChatMessage {
                role: "system".into(),
                content: "s".into(),
            },
            ChatMessage {
                role: "user".into(),
                content: "u".into(),
            },
        ];
        let one = ["a/b".to_string()];
        let two = ["a/b".to_string(), "c/d".to_string()];
        let text = |models: &[String], stream: bool, cache: Option<usize>| {
            serde_json::to_string(&chat_body(models, &msgs, stream, cache, None)).unwrap()
        };
        assert_eq!(
            text(&one, false, None),
            r#"{"messages":[{"content":"s","role":"system"},{"content":"u","role":"user"}],"model":"a/b","provider":{"data_collection":"deny","zdr":true},"stream":false,"usage":{"include":true}}"#
        );
        assert_eq!(
            text(&one, true, None),
            r#"{"messages":[{"content":"s","role":"system"},{"content":"u","role":"user"}],"model":"a/b","provider":{"data_collection":"deny","zdr":true},"stream":true,"stream_options":{"include_usage":true},"usage":{"include":true}}"#
        );
        assert_eq!(
            text(&one, false, Some(0)),
            r#"{"messages":[{"content":[{"cache_control":{"type":"ephemeral"},"text":"s","type":"text"}],"role":"system"},{"content":"u","role":"user"}],"model":"a/b","provider":{"data_collection":"deny","zdr":true},"stream":false,"usage":{"include":true}}"#
        );
        assert_eq!(
            text(&two, false, None),
            r#"{"messages":[{"content":"s","role":"system"},{"content":"u","role":"user"}],"models":["a/b","c/d"],"provider":{"data_collection":"deny","zdr":true},"stream":false,"usage":{"include":true}}"#
        );
        assert_eq!(
            text(&two, true, Some(1)),
            r#"{"messages":[{"content":"s","role":"system"},{"content":[{"cache_control":{"type":"ephemeral"},"text":"u","type":"text"}],"role":"user"}],"models":["a/b","c/d"],"provider":{"data_collection":"deny","zdr":true},"stream":true,"stream_options":{"include_usage":true},"usage":{"include":true}}"#
        );
    }

    /// A bound adds `max_tokens` and nothing else: the ZDR pin, the usage request and the model
    /// routing are untouched.
    #[test]
    fn a_bound_adds_one_field_and_the_zdr_pin_stays() {
        let msgs = [ChatMessage {
            role: "user".into(),
            content: "u".into(),
        }];
        let models = ["a/b".to_string(), "c/d".to_string()];
        let plain = chat_body(&models, &msgs, false, None, None);
        let bounded = chat_body(&models, &msgs, false, None, Some(300));
        assert_eq!(bounded["max_tokens"], serde_json::json!(300));
        assert_eq!(bounded["provider"]["zdr"], serde_json::json!(true));
        let mut without = bounded.as_object().unwrap().clone();
        without.remove("max_tokens");
        assert_eq!(serde_json::Value::Object(without), plain);
        assert!(plain.get("max_tokens").is_none());
    }

    #[test]
    fn chat_body_enforces_zdr_and_keeps_model_routing() {
        // Single model: the "model" field, the ZDR provider object, usage accounting, and the
        // streaming usage opt-in. chat_body is PM's only ZDR enforcement point, so this guards it.
        let one = chat_body(&["a/b".to_string()], &[], true, None, None);
        assert_eq!(one["provider"]["zdr"], serde_json::json!(true));
        assert_eq!(
            one["provider"]["data_collection"],
            serde_json::json!("deny")
        );
        assert_eq!(one["model"], serde_json::json!("a/b"));
        assert!(one.get("models").is_none());
        assert_eq!(one["usage"]["include"], serde_json::json!(true));
        assert_eq!(
            one["stream_options"]["include_usage"],
            serde_json::json!(true)
        );
        // Several models: the ordered "models" fallback list, still ZDR-enforced + usage-accounted,
        // no "model" and (non-streaming) no stream_options.
        let many = chat_body(
            &["a/b".to_string(), "c/d".to_string()],
            &[],
            false,
            None,
            None,
        );
        assert_eq!(many["models"], serde_json::json!(["a/b", "c/d"]));
        assert!(many.get("model").is_none());
        assert_eq!(many["provider"]["zdr"], serde_json::json!(true));
        // Both privacy axes on the fallback form too: they're independent (a provider can
        // decline to train yet still retain), so asserting only zdr would let a regression
        // that dropped data_collection through on the multi-model path.
        assert_eq!(
            many["provider"]["data_collection"],
            serde_json::json!("deny")
        );
        assert_eq!(many["usage"]["include"], serde_json::json!(true));
        assert!(many.get("stream_options").is_none());
    }

    fn detail(id: &str) -> ModelDetail {
        ModelDetail {
            id: id.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn zdr_filter_keeps_compliant_models_and_drops_unservable_ones() {
        // The picker must not offer a model PM cannot serve: chat_body pins zdr:true, so a
        // model with no ZDR endpoint 404s rather than degrading. This is the fable-5 class of
        // bug that the removed recommender shipped (6 endpoints, none of them ZDR).
        let zdr: HashSet<String> = ["ok/servable".to_string()].into_iter().collect();
        let kept = retain_zdr_servable(
            vec![detail("ok/servable"), detail("anthropic/fable-5")],
            &zdr,
        );
        let ids: Vec<&str> = kept.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["ok/servable"]);
    }

    #[test]
    fn zdr_filter_keeps_router_pseudo_models() {
        // Routers are absent from the ZDR feed (they aren't concrete endpoints) yet are
        // servable: the per-request pin applies to whatever they resolve to. Filtering them
        // out would delete openrouter/free — a real, in-use model — from the picker.
        let zdr: HashSet<String> = HashSet::new();
        let kept = retain_zdr_servable(
            vec![
                detail("openrouter/free"),
                detail("openrouter/auto"),
                detail("x/no-zdr"),
            ],
            &zdr,
        );
        let ids: Vec<&str> = kept.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["openrouter/free", "openrouter/auto"]);
    }

    #[test]
    fn router_match_respects_the_namespace_boundary() {
        assert!(is_router_model("openrouter/free"));
        // A lookalike vendor namespace must not inherit the router carve-out.
        assert!(!is_router_model("openrouter-mirror/free"));
        assert!(!is_router_model("openai/gpt-5.5"));
    }

    #[test]
    fn chat_body_marks_the_cache_breakpoint_at_the_given_index() {
        let msgs = vec![
            ChatMessage {
                role: "system".into(),
                content: "profile".into(),
            },
            ChatMessage {
                role: "system".into(),
                content: "rolling summary".into(),
            },
            ChatMessage {
                role: "user".into(),
                content: "the latest turn".into(),
            },
        ];
        // None → every message is a plain string, no cache_control anywhere (the old chat behaviour).
        let plain = chat_body(&["a/b".to_string()], &msgs, false, None, None);
        for i in 0..msgs.len() {
            assert!(
                plain["messages"][i]["content"].is_string(),
                "message {i} stays a plain string when uncached"
            );
        }
        // Some(1) → the breakpoint sits on message 1 (the last stable system message), so the provider
        // caches the prefix through it; the earlier stable message and the dynamic turn after it stay
        // plain. (Anthropic caches everything up to AND INCLUDING the marked block.)
        let cached = chat_body(&["a/b".to_string()], &msgs, false, Some(1), None);
        assert!(
            cached["messages"][0]["content"].is_string(),
            "messages before the breakpoint are not themselves marked"
        );
        assert_eq!(
            cached["messages"][1]["content"][0]["cache_control"]["type"],
            serde_json::json!("ephemeral"),
            "the breakpoint is on the last stable message"
        );
        assert_eq!(
            cached["messages"][1]["content"][0]["text"],
            serde_json::json!("rolling summary")
        );
        assert_eq!(
            cached["messages"][2]["content"],
            serde_json::json!("the latest turn"),
            "the dynamic turn after the breakpoint stays uncached"
        );
    }
}
