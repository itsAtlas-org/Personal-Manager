// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Your own Google Gemini key: the one module that knows Google's wire.
//!
//! Chat goes through Google's OpenAI-compatible endpoint, so this is a [`Flavour`] of the shared
//! dialect ([`crate::cloud_openai`]) plus what is Google's alone: the thinking levels, the errors
//! (the compatible endpoint wraps them in an ARRAY), the tokens (thinking is counted in a total, not in
//! `completion_tokens`), and the key check, which has to tell a FREE key from a PAID one, because
//! Google trains on what a free key sends and its terms allow only paid keys in the UK, the EEA and
//! Switzerland.
//!
//! The key check is two native calls (the list of models, then one constant `"ok"` prompt to a model
//! only paid keys can use). The key travels in the `x-goog-api-key` header and never in a URL. A free
//! key answers the probe with a limit-0 429, so nothing is processed under unpaid terms; a paid key's
//! probe costs about $0.0004. The probe is not a chat and is not written to the Usage log.
//!
//! Everything is named by its catalogue id (`gemini/<wire id>`) outside this file. This is not zero
//! data retention: Google logs a paid key's requests for a limited time and a free key's are used to
//! improve its products. The Settings card says so wherever the route is chosen.

use std::cell::Cell;
use std::future::Future;
use std::time::Duration;

use serde_json::{json, Value};

use crate::cloud_openai::{self, redact_detail, Flavour};
use crate::direct_models::{self, DirectModel};
use crate::direct_providers::{self, KeyCheck, KeyOutcome, KeyTier};
use crate::error::{Error, Result};
use crate::llm_gateway::{CloudArm, CloudProvider};
use crate::openrouter::{ChatMessage, Completion, Usage};
use crate::region::{self, Basis, Region};

/// The chat-completions URL. A const, so the key can only ever go to this host.
const ENDPOINT: &str = "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions";
/// The native list of models (the compatible list carries no limits and no methods).
const LIST_ENDPOINT: &str = "https://generativelanguage.googleapis.com/v1beta/models?pageSize=1000";
/// The native model base: `{base}/{wire}:generateContent` is the probe.
const MODELS_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";

pub(crate) const FLAVOUR: Flavour = Flavour {
    name: "Google Gemini",
    endpoint: ENDPOINT,
    // The compatible endpoint takes Bearer; the native key check uses `x-goog-api-key` (see
    // `goog_key`).
    auth: cloud_openai::bearer,
    // Gemini caches a repeated prefix by itself; the cache mark is dropped.
    cache_control_blocks: false,
    decorate,
    request_error,
    // Build day (2026-11-26, step 9): the compatible endpoint answered 200 to `max_tokens` at each of
    // the eight models' listed limits, and to `max_completion_tokens` too. PM keeps the one it
    // proved at the real limits; a body test pins it.
    max_tokens_field: "max_tokens",
    patient: true,
    stream_error: Some(stream_error),
    stop_error: Some(stop_error),
    usage: Some(usage_from),
};

/// The model, by its WIRE id, and nothing else: no `store`. Google's compatible endpoint answers it
/// with a 400 (`Unknown name "store": Cannot find field.`, build day 2026-11-26, step 9), so the one
/// field OpenAI's flavour adds here would fail every chat. The effort is the role's, which a const
/// flavour can't see, so [`chat_body`] adds it after.
fn decorate(body: &mut Value, models: &[String]) {
    body["model"] = json!(models[0]);
}

/// The request body for one call. `limit` is always sent: thinking tokens count toward it and are
/// billed even when the reply is cut, and a small cap came back empty after ~29 thinking tokens (the
/// spike), so a real call never sends a small one. `reasoning_effort` and `thinking_config` can't be
/// combined; PM sends only the first, and only a level the model lists.
///
/// The twin of `openai_direct::chat_body`, duplicated on purpose (about 40 lines, pure): sharing it
/// would touch OpenAI's path and its byte-for-byte body test.
fn chat_body(
    model: &DirectModel,
    effort: Option<&str>,
    messages: &[ChatMessage],
    stream: bool,
    cache_through: Option<usize>,
    limit: u32,
) -> Value {
    let mut body = cloud_openai::build_body(
        &FLAVOUR,
        std::slice::from_ref(&model.wire),
        messages,
        stream,
        cache_through,
        Some(limit),
    );
    if let Some(level) = effort.filter(|l| model.offers(l)) {
        body["reasoning_effort"] = json!(level);
    }
    body
}

/// The catalogue row for the arm's first model, or the refusal that stops a stale setting before
/// any request leaves. A direct arm calls only its first model in this version.
fn requested_model(arm: &CloudArm) -> Result<&'static DirectModel> {
    let id = arm.models.first().map(String::as_str).unwrap_or_default();
    direct_models::find(id)
        .filter(|m| m.id.starts_with("gemini/"))
        .ok_or_else(|| {
            Error::Other(format!(
                "PM no longer lists {id} for Google Gemini. Pick another model in Settings > AI & Models."
            ))
        })
}

/// The output limit: the model's maximum, lowered to a caller's bound.
fn output_limit(model: &DirectModel, bound: Option<u32>) -> u32 {
    bound.map_or(model.max_output, |b| b.min(model.max_output))
}

/// The effort that goes on the wire: the arm's, if the model still offers it. Checked again here so
/// a stale setting can never produce a 400. Each model's own set decides ("minimal" never goes to a
/// model that doesn't list it).
fn wire_effort<'a>(model: &DirectModel, arm: &'a CloudArm) -> Option<&'a str> {
    arm.effort.as_deref().filter(|e| model.offers(e))
}

// ---- the send seam --------------------------------------------------------------------------------

/// Whether this verdict may be used here. PURE: the one rule ([`direct_providers::refuses`]) turned
/// into the error a refused call carries. Every send passes through it BEFORE anything else, because a
/// running plan that already holds a Gemini arm is not stopped by a refusal that lived only at resolve.
fn seam(tier: Option<KeyTier>, region: &Region) -> Result<()> {
    match direct_providers::refuses(CloudProvider::Gemini, tier, region) {
        Some(r) => Err(Error::Other(r.sentence(CloudProvider::Gemini))),
        None => Ok(()),
    }
}

/// The tier a send is judged by: the latest word about THIS key (a call or a fresh check since the
/// plan was built, Paid and "couldn't tell" included), else the one the arm was stamped with.
fn effective_tier(arm: &CloudArm) -> Option<KeyTier> {
    let fp = direct_providers::key_fp(arm.key.expose());
    direct_providers::latest_tier(CloudProvider::Gemini, &fp).or(arm.tier)
}

/// The local "not before" for a model whose free allowance is spent for the day: the sentence, or
/// `None` when Google may be asked.
fn spent_allowance(model: &DirectModel) -> Option<Error> {
    let wait = direct_providers::not_before(CloudProvider::Gemini, &model.wire)?;
    Some(Error::Other(free_allowance_text(
        Some(wait.as_secs().max(1)),
        false,
    )))
}

/// A one-shot completion. Runs the stream and discards the deltas: a thinking model can sit silent for
/// minutes, a total deadline would kill a request that is being billed, and the dispatch scans keep
/// their shape.
pub(crate) async fn complete(
    arm: &CloudArm,
    messages: &[ChatMessage],
    cache_prefix: bool,
    max_tokens: Option<u32>,
) -> Result<Completion> {
    seam(effective_tier(arm), &region::assumed())?;
    let model = requested_model(arm)?;
    if let Some(e) = spent_allowance(model) {
        return Err(e);
    }
    let body = chat_body(
        model,
        wire_effort(model, arm),
        messages,
        true,
        cache_prefix.then_some(0),
        output_limit(model, max_tokens),
    );
    let (result, mark) = with_call_mark(cloud_openai::stream(
        &FLAVOUR,
        arm.key.expose(),
        &body,
        |_| {},
    ))
    .await;
    match result {
        Ok(completion) => Ok(settle(
            completion,
            model,
            effective_tier(arm),
            arm.key.expose(),
        )),
        Err(e) => Err(after_failure(e, arm, model, mark)),
    }
}

/// A streamed chat completion, each text delta to `on_token`.
pub(crate) async fn stream_chat<F: FnMut(&str)>(
    arm: &CloudArm,
    messages: &[ChatMessage],
    cache_through: Option<usize>,
    on_token: F,
) -> Result<Completion> {
    seam(effective_tier(arm), &region::assumed())?;
    let model = requested_model(arm)?;
    if let Some(e) = spent_allowance(model) {
        return Err(e);
    }
    let body = chat_body(
        model,
        wire_effort(model, arm),
        messages,
        true,
        cache_through,
        model.max_output,
    );
    let (result, mark) = with_call_mark(cloud_openai::stream(
        &FLAVOUR,
        arm.key.expose(),
        &body,
        on_token,
    ))
    .await;
    match result {
        Ok(completion) => Ok(settle(
            completion,
            model,
            effective_tier(arm),
            arm.key.expose(),
        )),
        Err(e) => Err(after_failure(e, arm, model, mark)),
    }
}

/// A failed call may have taught PM the project is on the free tier: the classifier left a mark for
/// THIS call (`mark`: the wait it gave, if any), and this function, which holds the arm, passes it on
/// under the ARM'S key (an old key's late failure is ignored by the inbox). Returns the error
/// unchanged.
fn after_failure(
    e: Error,
    arm: &CloudArm,
    model: &DirectModel,
    mark: Option<Option<u64>>,
) -> Error {
    if let Some(wait) = mark {
        direct_providers::note_tier_seen(
            CloudProvider::Gemini,
            KeyTier::Free,
            &direct_providers::key_fp(arm.key.expose()),
            &model.wire,
            wait,
        );
    }
    e
}

// ---- what a reply cost --------------------------------------------------------------------------------

/// Google counts thinking in `total_tokens` but NOT in `completion_tokens` on the compatible endpoint
/// (the spike: prompt 5, completion 0, total 34). Billable output is `total - prompt`, so that is what
/// is reported, never less than what `completion_tokens` says (a Google fix that starts counting
/// thoughts there can't make PM under-count). A prompt count with no total means the output may leave
/// its thinking out, and the price must not look exact.
fn usage_from(v: &Value) -> Usage {
    let mut u = cloud_openai::parse_usage(v);
    match (u.prompt_tokens, v["usage"]["total_tokens"].as_i64()) {
        (Some(p), Some(t)) => {
            u.completion_tokens = Some(u.completion_tokens.unwrap_or(0).max(t - p));
        }
        // A total with no prompt count can't be split into input and output, and the input would be
        // priced as nothing and look exact: show it as partial, like a prompt with no total.
        (Some(_), None) | (None, Some(_)) => u.output_partial = true,
        _ => {}
    }
    u
}

/// Turn what the server reported into what PM logs, and say what the cost is.
///
/// A catalogue model, always ([`direct_models::settle_on`]). A key PM knows is FREE, on a model with a
/// free allowance, costs nothing: showing PM's price for it would be wrong the other way. Unknown or
/// paid keep the estimate (over-estimating is the safe direction). A reply that reported a prompt but
/// no total reads "-" and never a priced under-count. A success on a model only paid keys can use is
/// PROOF the key is paid, so the free rule ends the first time one answers.
fn settle(c: Completion, requested: &DirectModel, tier: Option<KeyTier>, key: &str) -> Completion {
    settle_on(c, requested, tier, key, &direct_models::utc_today())
}

fn settle_on(
    c: Completion,
    requested: &DirectModel,
    tier: Option<KeyTier>,
    key: &str,
    today: &str,
) -> Completion {
    let partial = c.usage.output_partial;
    let mut c = direct_models::settle_on(c, requested, today);
    let priced = c
        .model
        .as_deref()
        .and_then(direct_models::find)
        .unwrap_or(requested);
    let mut tier = tier;
    if priced.free_tier == Some(false) {
        direct_providers::note_paid_proven(CloudProvider::Gemini, &direct_providers::key_fp(key));
        tier = Some(KeyTier::Paid);
    }
    if tier == Some(KeyTier::Free) && priced.free_tier == Some(true) {
        c.usage.cost = Some(0.0);
    } else if partial {
        c.usage.cost = None;
    }
    c
}

// ---- errors ----------------------------------------------------------------------------------------------

tokio::task_local! {
    /// Whether THIS call's failure named the free tier, and the wait it gave. The classifier
    /// ([`request_error`]) has no arm, no key and no database, so it leaves the mark here and the two
    /// entry points that hold the arm pass it on ([`after_failure`]). Per call, not per process: two
    /// calls on two keys run at once, and a free-tier answer to the old key's call must never be
    /// read as news about the new key's call that failed for another reason. `Some(None)` is a free
    /// answer with no wait worth keeping.
    static CALL_FREE: Cell<Option<Option<u64>>>;
}

/// Run one call with its own mark, and hand the mark back beside the result. The classifier runs
/// inside the awaited future ([`cloud_openai::stream`] spawns nothing), so the task-local reaches it.
async fn with_call_mark<F: Future<Output = Result<Completion>>>(
    call: F,
) -> (Result<Completion>, Option<Option<u64>>) {
    CALL_FREE
        .scope(Cell::new(None), async {
            let r = call.await;
            (r, CALL_FREE.with(Cell::get))
        })
        .await
}

/// Leave the mark for the call in progress; outside one (a key check, a test) nothing is left.
fn mark_free(wait: Option<u64>) {
    let _ = CALL_FREE.try_with(|c| c.set(Some(wait)));
}

/// The words of one Google error (`{"error":{code,message,status,details[]}}`), whichever endpoint
/// sent it. Quota ids are read from the details by NAME, never by position (their order differs from
/// call to call).
#[derive(Debug, Default, Clone)]
pub(crate) struct GErr {
    http: Option<u16>,
    status: String,
    message: String,
    reason: String,
    quota_ids: Vec<String>,
    quota_metrics: Vec<String>,
    retry_secs: Option<u64>,
    /// The lowercased body, set only when it could not be read as JSON (a proxy's page, a body cut at
    /// the 16 KiB bound): the words are then looked for in the text.
    scanned: Option<String>,
}

fn http_from_status(status: &str) -> Option<u16> {
    Some(match status {
        "RESOURCE_EXHAUSTED" => 429,
        "UNAVAILABLE" => 503,
        "PERMISSION_DENIED" => 403,
        "UNAUTHENTICATED" => 401,
        "NOT_FOUND" => 404,
        "INVALID_ARGUMENT" | "FAILED_PRECONDITION" => 400,
        "INTERNAL" => 500,
        "DEADLINE_EXCEEDED" => 504,
        _ => return None,
    })
}

/// `"70695s"` (RetryInfo) as whole seconds, rounded up.
fn seconds_in(delay: &str) -> Option<u64> {
    let n: f64 = delay.trim().strip_suffix('s')?.parse().ok()?;
    (n.is_finite() && n >= 0.0).then(|| n.ceil() as u64)
}

/// "Please retry in 19h38m14.82s" in a message, as whole seconds. For a body whose RetryInfo is missing.
fn retry_in_message(message: &str) -> Option<u64> {
    let tail = message.split("retry in ").nth(1)?;
    let word: String = tail
        .chars()
        .take_while(|c| c.is_ascii_digit() || matches!(c, 'h' | 'm' | 's' | '.'))
        .collect();
    let mut total = 0.0_f64;
    let mut number = String::new();
    let mut any = false;
    for c in word.chars() {
        match c {
            'h' | 'm' | 's' => {
                let n: f64 = number.parse().ok()?;
                total += n * match c {
                    'h' => 3600.0,
                    'm' => 60.0,
                    _ => 1.0,
                };
                number.clear();
                any = true;
            }
            _ => number.push(c),
        }
    }
    any.then(|| total.ceil() as u64)
}

/// Read the `error` object of one body or chunk into a [`GErr`].
fn gerr_from_value(err: &Value) -> GErr {
    let mut e = GErr {
        http: err["code"]
            .as_u64()
            .or_else(|| err["code"].as_str().and_then(|s| s.parse().ok()))
            .and_then(|c| u16::try_from(c).ok()),
        status: err["status"].as_str().unwrap_or_default().to_string(),
        message: err["message"].as_str().unwrap_or_default().to_string(),
        ..GErr::default()
    };
    if e.http.is_none() {
        e.http = http_from_status(&e.status);
    }
    for d in err["details"].as_array().into_iter().flatten() {
        if let Some(r) = d["reason"].as_str() {
            e.reason = r.to_string();
        }
        for v in d["violations"].as_array().into_iter().flatten() {
            if let Some(id) = v["quotaId"].as_str() {
                e.quota_ids.push(id.to_string());
            }
            if let Some(m) = v["quotaMetric"].as_str() {
                e.quota_metrics.push(m.to_string());
            }
        }
        if let Some(s) = d["retryDelay"].as_str().and_then(seconds_in) {
            e.retry_secs = Some(s);
        }
    }
    if e.retry_secs.is_none() {
        e.retry_secs = retry_in_message(&e.message);
    }
    e
}

/// Read a failed response's RAW body, whichever shape: the compatible endpoint's `[{"error":...}]`, the
/// native `{"error":...}`, or text that isn't JSON at all (then the words are looked for in it).
pub(crate) fn parse_error(raw: &str) -> GErr {
    if let Ok(v) = serde_json::from_str::<Value>(raw.trim()) {
        let v = v.as_array().and_then(|a| a.first()).cloned().unwrap_or(v);
        if v["error"].is_object() {
            return gerr_from_value(&v["error"]);
        }
    }
    GErr {
        message: raw.to_string(),
        scanned: Some(raw.to_lowercase()),
        ..GErr::default()
    }
}

impl GErr {
    /// Everything Google said, lowercased, for the word checks.
    fn words(&self) -> String {
        match &self.scanned {
            Some(s) => s.clone(),
            None => format!("{} {} {}", self.message, self.reason, self.status).to_lowercase(),
        }
    }

    /// A key Google doesn't know. The native endpoints say `API_KEY_INVALID` and "API key not valid.
    /// Please pass a valid API key."; the compatible chat says only "Please pass a valid API key", with
    /// no reason (build day 2026-11-26, step 10), so that sentence counts too.
    fn key_not_valid(&self) -> bool {
        let w = self.words();
        self.reason == "API_KEY_INVALID"
            || w.contains("api key not valid")
            || w.contains("api_key_invalid")
            || w.contains("pass a valid api key")
    }

    fn names_verification(&self) -> bool {
        self.words().contains("verif")
    }

    fn names_leak(&self) -> bool {
        self.words().contains("leaked")
    }

    /// The free-tier-unavailable-here precondition (HTTP 400).
    fn free_tier_unavailable(&self) -> bool {
        let w = self.words();
        // The status alone is not enough: a paid key can meet an unrelated FAILED_PRECONDITION, and
        // saying "no free keys where you are" about it would name the wrong cause.
        w.contains("free tier is not available")
            || (w.contains("failed_precondition")
                && ["free", "country", "location", "region"]
                    .iter()
                    .any(|t| w.contains(t)))
    }

    /// Whether this 429 is about the FREE tier's quota: any quota id ending `-FreeTier` or any metric
    /// naming `free_tier`. A body with no quota details (stripped, cut short, not JSON) is judged on its
    /// words. A 429 that names only other quotas (a paid project's own rate or daily cap) is not.
    fn names_free_tier(&self) -> bool {
        if !self.quota_ids.is_empty() || !self.quota_metrics.is_empty() {
            return self
                .quota_ids
                .iter()
                .any(|id| id.to_lowercase().ends_with("-freetier"))
                || self.quota_metrics.iter().any(|m| m.contains("free_tier"));
        }
        let w = self.words();
        w.contains("free_tier") || w.contains("free tier") || w.contains("-freetier")
    }

    /// A daily window (a paid project's own cap or a free allowance spent for the day), not a pause.
    fn daily(&self) -> bool {
        self.quota_ids.iter().any(|id| id.contains("PerDay")) || self.words().contains("per day")
    }
}

/// What went wrong, as PM tells it. Shared by the request-time and the mid-stream paths.
#[derive(Debug, PartialEq)]
pub(crate) enum Failure {
    OutOfCredit,
    KeyRejected,
    Leaked,
    NeedsVerification,
    NotAllowed,
    /// The key is on the free tier. `zero`: this model has no free allowance at all (limit 0).
    FreeTier {
        zero: bool,
        wait: Option<u64>,
    },
    RateLimited {
        wait: Option<u64>,
        daily: bool,
    },
    Busy,
    Trouble,
    NoFreeTierHere,
    ModelUnavailable,
    EffortRefused,
    ContextTooLong,
    LimitOutOfDate,
    Other {
        status: Option<u16>,
        detail: String,
    },
}

/// Decide what failed. PURE. Precedence matters: a 402 and a 429 are both `RESOURCE_EXHAUSTED`, and a
/// free-tier 429 must never be told as "try again shortly".
pub(crate) fn classify(e: &GErr) -> Failure {
    let http = e.http;
    let is = |s: u16| http == Some(s);
    let lower = e.words();
    if is(402) {
        return Failure::OutOfCredit;
    }
    if (is(400) && e.key_not_valid()) || is(401) {
        return Failure::KeyRejected;
    }
    if is(403) {
        return if e.names_leak() {
            Failure::Leaked
        } else if e.names_verification() {
            Failure::NeedsVerification
        } else {
            Failure::NotAllowed
        };
    }
    if is(429) {
        return if e.names_free_tier() {
            Failure::FreeTier {
                zero: lower.contains("limit: 0"),
                wait: e.retry_secs,
            }
        } else {
            Failure::RateLimited {
                wait: e.retry_secs,
                daily: e.daily(),
            }
        };
    }
    if is(503) || e.status == "UNAVAILABLE" || lower.contains("high demand") {
        return Failure::Busy;
    }
    if http.is_some_and(|s| s >= 500) || is(408) {
        return Failure::Trouble;
    }
    if is(400) && e.free_tier_unavailable() {
        return Failure::NoFreeTierHere;
    }
    if is(404) {
        return Failure::ModelUnavailable;
    }
    if is(400) {
        // Google's own words for a level a model refuses are "Thinking level MINIMAL is not supported
        // for this model" (build day, step 5), and the field names are matched too.
        if lower.contains("reasoning_effort")
            || lower.contains("thinking_level")
            || lower.contains("thinking level")
        {
            return Failure::EffortRefused;
        }
        if lower.contains("input token count") {
            return Failure::ContextTooLong;
        }
        if lower.contains("max_tokens") {
            return Failure::LimitOutOfDate;
        }
    }
    Failure::Other {
        status: http,
        // Redacted BEFORE the cut, so a key can't be cut in half and slip past the run length.
        detail: redact_detail(&e.message).chars().take(200).collect(),
    }
}

/// A wait in words. Short wording ("about 30 seconds", "a minute") only for a short wait or an
/// unknown one; 5 minutes or more is quoted as minutes or hours ("about 20 hours"), because the
/// spike's RetryInfo was 19.6 hours and "try again shortly" would be a lie.
pub(crate) fn wait_words(secs: Option<u64>) -> String {
    match secs {
        None => "a minute".into(),
        Some(s) if s < 60 => format!("about {} seconds", s.max(1)),
        Some(s) if s < 150 => "a minute".into(),
        Some(s) if s < 3600 => format!("about {} minutes", (s + 30) / 60),
        Some(s) => format!("about {} hours", (s + 1800) / 3600),
    }
}

/// A spent free allowance, outside the region. A wait of 5 minutes or more also stops PM asking again
/// until then.
fn free_allowance_text(wait: Option<u64>, zero: bool) -> String {
    if zero {
        return "Google's free tier has no allowance for this model, so this key can't use it. Pick a model that doesn't say \"Needs a paid key\" in Settings > AI & Models, or make the project paid in AI Studio.".into();
    }
    match wait {
        Some(w) if w >= 300 => format!(
            "This key's free allowance is used up. It comes back in {}. PM won't ask Google again about this model until then. Make the project paid in AI Studio for higher limits, then press Check again on your Google Gemini key in Settings > AI & Models so PM asks again.",
            wait_words(Some(w))
        ),
        Some(w) => format!(
            "This key's free allowance is used up for the moment. Try again in {}.",
            wait_words(Some(w))
        ),
        None => {
            "This key's free allowance is used up. Google resets it daily and didn't say when."
                .into()
        }
    }
}

/// The sentence for a failure, in this place. A free-tier failure INSIDE the region says Gemini is
/// paused (the same sentence the send seam gives); outside it, the allowance.
pub(crate) fn failure_text(f: &Failure, region: &Region) -> String {
    match f {
        Failure::OutOfCredit => "Google has stopped this key: its prepay balance is empty. Add credit at aistudio.google.com/billing, then try again in a few minutes.".into(),
        Failure::KeyRejected => "Google says this key isn't right any more. Check it in AI Studio and paste it again in Settings > AI & Models.".into(),
        Failure::Leaked => "Google has blocked this key because it was found in public. Make a new key in AI Studio and paste it in Settings > AI & Models.".into(),
        Failure::NeedsVerification => "Google wants this project verified in AI Studio before the key can be used. Verify it, then try again.".into(),
        Failure::NotAllowed => "Google refused this key. It may be limited to other Google services, or blocked. Make a fresh key in AI Studio and paste it in Settings > AI & Models.".into(),
        Failure::FreeTier { zero, wait } => match region.refusal(Basis::Free) {
            Some(r) => r.sentence(CloudProvider::Gemini),
            None => free_allowance_text(*wait, *zero),
        },
        Failure::RateLimited { wait, daily } => {
            if *daily {
                match wait {
                    Some(w) => format!(
                        "This key has reached Google's daily limit. It resets in {}.",
                        wait_words(Some(*w))
                    ),
                    None => "This key has reached Google's daily limit. It resets daily; Google didn't say when.".into(),
                }
            } else {
                format!(
                    "Google is limiting requests on this key. Try again in {}.",
                    wait_words(*wait)
                )
            }
        }
        Failure::Busy => "Google says this model is busy right now. Try again in a minute.".into(),
        Failure::Trouble => "Google had a problem on its side. Try again in a minute.".into(),
        Failure::NoFreeTierHere => "Google doesn't offer free Gemini keys where you are. Link billing to the key's project in AI Studio to make it paid.".into(),
        Failure::ModelUnavailable => "Your Google project can't use this model. Pick another in Settings > AI & Models.".into(),
        Failure::EffortRefused => "Google refused the thinking level set for this model. Pick another level in Settings > AI & Models.".into(),
        Failure::ContextTooLong => "That's more than this Gemini model can read in one go. Start a new chat or pick a model with a bigger window.".into(),
        Failure::LimitOutOfDate => "Google refused this model's output limit, so PM's list is out of date for it. Pick another model; PM needs an update to fix this one.".into(),
        Failure::Other { status, detail } => match status {
            Some(s) => format!("Google Gemini answered with an error ({s}): {detail}."),
            None => format!("Google Gemini answered with an error: {detail}."),
        },
    }
}

/// How a failed response reads, judged against where this computer is.
fn request_error(status: reqwest::StatusCode, raw: &str) -> Error {
    request_error_in(status, raw, &region::assumed())
}

/// [`request_error`] with the region given, so no test reads the host's time zone. A free-tier answer
/// also leaves the mark the entry point passes on.
pub(crate) fn request_error_in(status: reqwest::StatusCode, raw: &str, region: &Region) -> Error {
    let mut e = parse_error(raw);
    e.http = Some(status.as_u16());
    error_of(&e, region)
}

fn error_of(e: &GErr, region: &Region) -> Error {
    let f = classify(e);
    // A limit-0 answer ("no free allowance for this model") carries a retry delay of a whole day
    // (19.6 h on build day), but there is no allowance to come back: holding the model back for that
    // long would keep blocking it after the person pays. Only a real, spent allowance sets a wait.
    if let Failure::FreeTier { zero, wait } = &f {
        mark_free(if *zero { None } else { *wait });
    }
    Error::Other(failure_text(&f, region))
}

/// How a mid-stream `error` chunk reads: the same table.
fn stream_error(err: &Value) -> Error {
    error_of(&gerr_from_value(err), &region::assumed())
}

/// A stop reason that is a failure: Google's safety filter withheld the reply (the compatible endpoint
/// maps a safety stop to `content_filter`; UNVERIFIED until a live test provokes one).
fn stop_error(reason: &str) -> Option<Error> {
    (reason == "content_filter")
        .then(|| Error::Other("Google withheld this reply (its safety filter).".into()))
}

// ---- the key check ------------------------------------------------------------------------------------------

/// Whether a pasted key is the right shape to try: trimmed, 20 to 300 characters, visible ASCII only
/// (so `bearer_auth` can't fail on a header character), and not another company's (`sk-` is OpenAI's,
/// OpenRouter's and Anthropic's). No prefix is REQUIRED: none is documented, and a new kind of key may
/// look different.
pub(crate) fn well_formed(key: &str) -> bool {
    let key = key.trim();
    (20..=300).contains(&key.len())
        && key.chars().all(|c| c.is_ascii_graphic())
        && !direct_providers::looks_like_another_providers_key(CloudProvider::Gemini, key)
}

/// The key on a native request, in the header Google reads it from. NEVER in the URL's query string:
/// URLs end up in logs and error messages.
fn goog_key(request: reqwest::RequestBuilder, key: &str) -> reqwest::RequestBuilder {
    request.header("x-goog-api-key", key)
}

/// The client the key check uses. It follows NO redirect: reqwest strips only `Authorization` and
/// `Cookie` when a redirect leaves the host, never a custom header, so a redirect from Google would
/// carry `x-goog-api-key` to wherever it pointed. The chat path sends Bearer, which reqwest strips.
fn check_http() -> &'static reqwest::Client {
    static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest client with static settings should build")
    });
    &CLIENT
}

/// What the list of models says about the key.
#[derive(Debug, PartialEq)]
pub(crate) enum ListVerdict {
    /// 200 with a `models` array: go on to the probe.
    Proceed,
    /// The list already decided it: the outcome, and the tier it implies.
    Done(KeyOutcome, Option<KeyTier>),
}

/// What the list's response means. PURE: status and body in, verdict out. Google answers a bad key
/// with 400 (not 401), a 402 stops every key on the billing account (nothing to probe), and a 200 that
/// isn't Google's JSON (a captive portal's page) must never be an `Ok`, which would keep the key.
pub(crate) fn interpret_list(status: u16, body: &str) -> ListVerdict {
    let e = parse_error(body);
    match status {
        200 => {
            let listing = serde_json::from_str::<Value>(body).is_ok_and(|v| v["models"].is_array());
            if listing {
                ListVerdict::Proceed
            } else {
                ListVerdict::Done(KeyOutcome::Unexpected(200), None)
            }
        }
        401 => ListVerdict::Done(KeyOutcome::Rejected, None),
        400 if e.key_not_valid() => ListVerdict::Done(KeyOutcome::Rejected, None),
        400 if e.free_tier_unavailable() => {
            ListVerdict::Done(KeyOutcome::RegionBlocked, Some(KeyTier::Free))
        }
        402 => ListVerdict::Done(KeyOutcome::NoCredit, Some(KeyTier::Paid)),
        // A 403 is a refusal, never a quiet keep: an API key restriction is per Google API, so a key
        // that can't list can't chat either.
        403 if e.names_verification() && !e.names_leak() => {
            ListVerdict::Done(KeyOutcome::NeedsVerification, Some(KeyTier::Free))
        }
        403 => ListVerdict::Done(KeyOutcome::Rejected, None),
        408 | 429 | 500..=599 => ListVerdict::Done(KeyOutcome::Busy, None),
        other => ListVerdict::Done(KeyOutcome::Unexpected(other), None),
    }
}

/// The catalogue ids the key can chat with, from the native list, and how many models it listed that
/// can `generateContent`. The key lists about 49 (TTS, image, omni, lyria and robotics ids among them),
/// so the catalogue join is the filter.
pub(crate) fn listed_models(body: &str) -> (Vec<String>, usize) {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return (Vec::new(), 0);
    };
    let chat: Vec<&str> = v["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| {
            m["supportedGenerationMethods"]
                .as_array()
                .is_some_and(|a| a.iter().any(|x| x == "generateContent"))
        })
        .filter_map(|m| m["name"].as_str())
        .map(|n| n.strip_prefix("models/").unwrap_or(n))
        .collect();
    let listed = direct_models::models(CloudProvider::Gemini)
        .iter()
        .filter(|m| chat.contains(&m.wire.as_str()))
        .map(|m| m.id.clone())
        .collect();
    (listed, chat.len())
}

/// Why a probe model did not decide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Skip {
    /// 404: this key can't use that model.
    NotHere,
    /// Google was busy or limiting: a 408, a 5xx, a 429 that names no free-tier quota (a paid
    /// project's own cap).
    Busy,
    /// The request never got an answer (a transport error or a timeout).
    Transport,
    /// Anything PM doesn't recognise: another 4xx, a 200 of the wrong shape.
    Odd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Decision {
    Paid,
    Free,
    OutOfCredit,
    NeedsVerification,
}

/// One probe model's answer: decisive, or a reason to ask the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answer {
    Decisive(Decision),
    Skip(Skip),
}

/// What a probe answer means. PURE. "Paid" must be EARNED: a 200 counts only when it is Google's
/// generateContent reply (a `candidates` array or a `usageMetadata` object), because a bare, HTML or
/// otherwise odd 200 is not proof of a paid key.
pub(crate) fn interpret_probe(status: u16, body: &str) -> Answer {
    let e = parse_error(body);
    match status {
        200 => {
            let shaped = serde_json::from_str::<Value>(body)
                .is_ok_and(|v| v["candidates"].is_array() || v["usageMetadata"].is_object());
            if shaped {
                Answer::Decisive(Decision::Paid)
            } else {
                Answer::Skip(Skip::Odd)
            }
        }
        402 => Answer::Decisive(Decision::OutOfCredit),
        429 if e.names_free_tier() => Answer::Decisive(Decision::Free),
        429 => Answer::Skip(Skip::Busy),
        400 if e.free_tier_unavailable() => Answer::Decisive(Decision::Free),
        403 if e.names_verification() && !e.names_leak() => {
            Answer::Decisive(Decision::NeedsVerification)
        }
        404 => Answer::Skip(Skip::NotHere),
        408 | 500..=599 => Answer::Skip(Skip::Busy),
        _ => Answer::Skip(Skip::Odd),
    }
}

/// What the probe round decided.
#[derive(Debug, PartialEq)]
pub(crate) enum Probe {
    Tier(KeyTier),
    OutOfCredit,
    NeedsVerification,
    /// Nothing was decided and Google answered: nobody is told "unknown".
    Busy,
    /// Nothing was decided and nothing answered.
    Unreachable,
}

/// The rule over the probe models' answers, in order. PURE. The first DECISIVE answer wins. Otherwise
/// an answer PM didn't recognise makes the tier Unknown (which reads as free); else a busy server or a
/// capped paid project (which has not answered the tier question) makes the whole check `Busy`, or
/// `Unreachable` when every busy skip was a transport failure; else (only "not here", or no candidate
/// listed) Unknown. One model's 503 must not decide a person's tier, because Unknown refuses a paid
/// key in the UK.
pub(crate) fn settle_probes(answers: &[Answer]) -> Probe {
    for a in answers {
        if let Answer::Decisive(d) = a {
            return match d {
                Decision::Paid => Probe::Tier(KeyTier::Paid),
                Decision::Free => Probe::Tier(KeyTier::Free),
                Decision::OutOfCredit => Probe::OutOfCredit,
                Decision::NeedsVerification => Probe::NeedsVerification,
            };
        }
    }
    let skips: Vec<Skip> = answers
        .iter()
        .filter_map(|a| match a {
            Answer::Skip(s) => Some(*s),
            Answer::Decisive(_) => None,
        })
        .collect();
    if skips.contains(&Skip::Odd) {
        return Probe::Tier(KeyTier::Unknown);
    }
    let any_busy = skips.contains(&Skip::Busy);
    let any_transport = skips.contains(&Skip::Transport);
    if any_busy {
        Probe::Busy
    } else if any_transport {
        Probe::Unreachable
    } else {
        Probe::Tier(KeyTier::Unknown)
    }
}

/// The constant the probe sends. Never user text.
const PROBE_BODY: &str =
    r#"{"contents":[{"parts":[{"text":"ok"}]}],"generationConfig":{"maxOutputTokens":1}}"#;

/// The two URLs a key check talks to. The shipped pair is a const, so the key can only ever go to
/// Google; tests hand the private [`check_key_at`] a loopback pair.
struct CheckUrls {
    list: &'static str,
    models_base: &'static str,
}

const CHECK_URLS: CheckUrls = CheckUrls {
    list: LIST_ENDPOINT,
    models_base: MODELS_BASE,
};

/// Bounds: the list is ~60 short rows, an error body a few hundred bytes. Anything near these is not
/// Google.
const MAX_LIST_BODY: usize = 4 * 1024 * 1024;
const MAX_PROBE_BODY: usize = 16 * 1024;

/// The probe's models, in order, that the key lists.
fn probe_order(listed: &[String]) -> Vec<&'static DirectModel> {
    let Some(section) = direct_models::section(CloudProvider::Gemini) else {
        return Vec::new();
    };
    section
        .probe_models
        .iter()
        .filter(|id| listed.contains(id))
        .filter_map(|id| direct_models::find(id))
        .collect()
}

/// Send one probe. A plain send through the shared client (not `wire_mark::send`: it is not a paid
/// chat and no mod is involved), no attribution header and no User-Agent, with an explicit total
/// timeout so a stalled socket can't hold the Save button.
async fn send_probe(key: &str, models_base: &str, model: &DirectModel) -> Answer {
    let request = goog_key(
        check_http()
            .post(format!("{models_base}/{}:generateContent", model.wire))
            .timeout(Duration::from_secs(20)),
        key,
    )
    .header(reqwest::header::CONTENT_TYPE, "application/json")
    .body(PROBE_BODY);
    let Ok(response) = request.send().await else {
        return Answer::Skip(Skip::Transport);
    };
    let status = response.status().as_u16();
    match response.text().await {
        Ok(mut t) => {
            if t.len() > MAX_PROBE_BODY {
                let mut cut = MAX_PROBE_BODY;
                while !t.is_char_boundary(cut) {
                    cut -= 1;
                }
                t.truncate(cut);
            }
            interpret_probe(status, &t)
        }
        Err(_) => Answer::Skip(Skip::Transport),
    }
}

/// Ask Google which models the key can see, then which side of the free/paid line it is on. See the
/// module docs for what is sent. The key is handed to nothing but `x-goog-api-key` headers.
pub(crate) async fn check_key(key: &str) -> KeyCheck {
    check_key_at(key, &CHECK_URLS).await
}

async fn check_key_at(key: &str, urls: &CheckUrls) -> KeyCheck {
    let request = goog_key(
        check_http().get(urls.list).timeout(Duration::from_secs(30)),
        key,
    );
    let Ok(response) = request.send().await else {
        return KeyCheck::of(KeyOutcome::Unreachable);
    };
    let status = response.status().as_u16();
    let body = match response.text().await {
        Ok(t) if t.len() <= MAX_LIST_BODY => t,
        Ok(_) => return KeyCheck::of(KeyOutcome::Unexpected(status)),
        Err(_) => return KeyCheck::of(KeyOutcome::Unreachable),
    };
    match interpret_list(status, &body) {
        ListVerdict::Done(outcome, Some(tier)) => return KeyCheck::of_tier(outcome, tier),
        ListVerdict::Done(outcome, None) => return KeyCheck::of(outcome),
        ListVerdict::Proceed => {}
    }
    let (listed, listed_total) = listed_models(&body);
    // Every listed probe model, in order, until one is decisive.
    let mut answers = Vec::new();
    for model in probe_order(&listed) {
        let answer = send_probe(key, urls.models_base, model).await;
        let done = matches!(answer, Answer::Decisive(_));
        answers.push(answer);
        if done {
            break;
        }
    }
    match settle_probes(&answers) {
        Probe::Tier(tier) => KeyCheck {
            listed,
            listed_total,
            tier: Some(tier),
            ..KeyCheck::of(KeyOutcome::Ok)
        },
        Probe::OutOfCredit => KeyCheck {
            listed,
            listed_total,
            tier: Some(KeyTier::Paid),
            ..KeyCheck::of(KeyOutcome::NoCredit)
        },
        Probe::NeedsVerification => KeyCheck::of_tier(KeyOutcome::NeedsVerification, KeyTier::Free),
        Probe::Busy => KeyCheck::of(KeyOutcome::Busy),
        Probe::Unreachable => KeyCheck::of(KeyOutcome::Unreachable),
    }
}

#[cfg(test)]
#[path = "gemini_fixtures.rs"]
mod fixtures;

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use crate::direct_providers::INBOX_TEST_LOCK;
    use crate::region::{region_of_on, RegionKind};

    fn uk() -> Region {
        region_of_on(Some("Europe/London"), false)
    }
    fn nyc() -> Region {
        region_of_on(Some("America/New_York"), false)
    }
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        INBOX_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn reset() {
        direct_providers::forget_all_seen();
    }
    /// One failed call's error and the mark it left, the way `complete` and `stream_chat` see them.
    async fn failed_call_async(
        status: reqwest::StatusCode,
        raw: &str,
        region: &Region,
    ) -> (Error, Option<Option<u64>>) {
        let (r, mark) = with_call_mark(async {
            // Yield once so two calls started together really interleave.
            tokio::task::yield_now().await;
            Err(request_error_in(status, raw, region))
        })
        .await;
        match r {
            Err(e) => (e, mark),
            Ok(_) => unreachable!("the scripted call always fails"),
        }
    }
    fn native(compat: &str) -> String {
        compat
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string()
    }
    fn gem(id: &str) -> &'static DirectModel {
        direct_models::find(&format!("gemini/{id}")).unwrap()
    }
    fn msgs() -> Vec<ChatMessage> {
        vec![
            ChatMessage {
                role: "system".into(),
                content: "s".into(),
            },
            ChatMessage {
                role: "user".into(),
                content: "u".into(),
            },
        ]
    }
    fn arm_with(key: &str, model: &str, tier: Option<KeyTier>, effort: Option<&str>) -> CloudArm {
        let mut a = CloudArm::new(
            CloudProvider::Gemini,
            crate::secret::Secret::from(key.to_string()),
            vec![format!("gemini/{model}")],
        );
        a.tier = tier;
        a.effort = effort.map(str::to_string);
        a
    }
    const KEY_A: &str = "test-key-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const KEY_B: &str = "test-key-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    const BAD_KEY_400: &str = r#"{"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","status":"INVALID_ARGUMENT","details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"API_KEY_INVALID","domain":"googleapis.com"}]}}"#;
    const LIST_BODY: &str = r#"{"models":[{"name":"models/gemini-2.5-flash","inputTokenLimit":1048576,"outputTokenLimit":65536,"supportedGenerationMethods":["generateContent","countTokens"]},{"name":"models/gemini-3.1-pro-preview","supportedGenerationMethods":["generateContent"]},{"name":"models/gemini-3.8-flash","supportedGenerationMethods":["generateContent"]},{"name":"models/gemini-3.1-flash-lite","supportedGenerationMethods":["generateContent"]},{"name":"models/gemini-2.5-flash-preview-tts","supportedGenerationMethods":["countTokens","generateContent"]},{"name":"models/text-embedding-9","supportedGenerationMethods":["embedContent"]}]}"#;

    // ---- error parsing ------------------------------------------------------------------------

    #[test]
    fn the_real_free_429_reads_the_same_native_and_array_wrapped() {
        let compat = parse_error(FREE_429_COMPAT);
        let nat = parse_error(FREE_429_NATIVE);
        for e in [&compat, &nat] {
            assert_eq!(e.http, Some(429));
            assert_eq!(e.status, "RESOURCE_EXHAUSTED");
            assert_eq!(e.quota_ids.len(), 4);
            assert!(
                e.quota_ids.iter().all(|id| id.ends_with("-FreeTier")),
                "{:?}",
                e.quota_ids
            );
            assert!(e.names_free_tier());
        }
        assert_eq!(compat.retry_secs, Some(70694));
        assert_eq!(nat.retry_secs, Some(70695));
        // The same compatible body unwrapped by hand parses like its native twin.
        assert_eq!(parse_error(&native(FREE_429_COMPAT)).quota_ids.len(), 4);
    }

    /// The violations come back in a different order each call; nothing reads them by position.
    #[test]
    fn shuffling_the_violations_changes_nothing() {
        let mut v: Value = serde_json::from_str(FREE_429_NATIVE).unwrap();
        v["error"]["details"][1]["violations"]
            .as_array_mut()
            .unwrap()
            .reverse();
        let e = parse_error(&v.to_string());
        assert!(e.names_free_tier());
        assert!(matches!(classify(&e), Failure::FreeTier { zero: true, .. }));
    }

    #[test]
    fn classification_follows_the_precedence() {
        let free = parse_error(FREE_429_COMPAT);
        assert_eq!(
            classify(&free),
            Failure::FreeTier {
                zero: true,
                wait: Some(70694)
            }
        );
        // A 402 is also RESOURCE_EXHAUSTED and must beat the 429 rules.
        let mut e = free.clone();
        e.http = Some(402);
        assert_eq!(classify(&e), Failure::OutOfCredit);
        // A 429 naming NO free-tier quota is a rate limit; with a per-day id it is a daily one.
        let rate = GErr {
            http: Some(429),
            quota_ids: vec!["GenerateRequestsPerMinutePerProjectPerModel".into()],
            ..GErr::default()
        };
        assert_eq!(
            classify(&rate),
            Failure::RateLimited {
                wait: None,
                daily: false
            }
        );
        let day = GErr {
            http: Some(429),
            quota_ids: vec!["GenerateRequestsPerDayPerProjectPerModel".into()],
            ..GErr::default()
        };
        assert_eq!(
            classify(&day),
            Failure::RateLimited {
                wait: None,
                daily: true
            }
        );
        // A real exhausted free quota (limit above 0) is FreeTier but not zero.
        let some = GErr {
            http: Some(429),
            quota_ids: vec!["GenerateRequestsPerMinutePerProjectPerModel-FreeTier".into()],
            message: "limit: 15".into(),
            retry_secs: Some(30),
            ..GErr::default()
        };
        assert_eq!(
            classify(&some),
            Failure::FreeTier {
                zero: false,
                wait: Some(30)
            }
        );
        assert_eq!(classify(&parse_error(BAD_KEY_400)), Failure::KeyRejected);
        let other400 = GErr {
            http: Some(400),
            message: "Request contains an invalid argument.".into(),
            ..GErr::default()
        };
        assert!(matches!(
            classify(&other400),
            Failure::Other {
                status: Some(400),
                ..
            }
        ));
        for body in [BUSY_503_COMPAT, BUSY_503_NATIVE] {
            let mut e = parse_error(body);
            e.http = Some(503);
            assert_eq!(classify(&e), Failure::Busy);
        }
        let leaked = GErr {
            http: Some(403),
            message: "Your API key was reported as leaked.".into(),
            ..GErr::default()
        };
        assert_eq!(classify(&leaked), Failure::Leaked);
        let ver = GErr {
            http: Some(403),
            message: "Needs verification".into(),
            ..GErr::default()
        };
        assert_eq!(classify(&ver), Failure::NeedsVerification);
        assert_eq!(
            classify(&GErr {
                http: Some(403),
                ..GErr::default()
            }),
            Failure::NotAllowed
        );
        let pre = GErr {
            http: Some(400),
            status: "FAILED_PRECONDITION".into(),
            message: "Gemini API free tier is not available in your country.".into(),
            ..GErr::default()
        };
        assert_eq!(classify(&pre), Failure::NoFreeTierHere);
        assert_eq!(
            classify(&GErr {
                http: Some(500),
                ..GErr::default()
            }),
            Failure::Trouble
        );
        assert_eq!(
            classify(&GErr {
                http: Some(404),
                ..GErr::default()
            }),
            Failure::ModelUnavailable
        );
        let effort = GErr {
            http: Some(400),
            message: "Unsupported value for reasoning_effort".into(),
            ..GErr::default()
        };
        assert_eq!(classify(&effort), Failure::EffortRefused);
    }

    /// OpenAI's D12 regression, on Gemini's JSON: a long message ahead of the reason can't hide it.
    #[test]
    fn a_long_message_before_the_reason_still_classifies_on_the_reason() {
        let body = format!(
            r#"{{"error":{{"code":400,"message":"{}","status":"INVALID_ARGUMENT","details":[{{"reason":"API_KEY_INVALID"}}]}}}}"#,
            "x".repeat(900)
        );
        assert_eq!(classify(&parse_error(&body)), Failure::KeyRejected);
    }

    /// A body that is not JSON, or was cut short, still names the free tier in its words.
    #[test]
    fn a_body_that_is_not_json_is_judged_on_its_words() {
        for raw in [
            "<html>429 You exceeded your quota on Free Tier</html>",
            r#"{"error":{"code":429,"message":"quota ... generate_content_free_tier_requests, limit: 0, model: gemini-3.1-pro"#,
        ] {
            let mut e = parse_error(raw);
            e.http = Some(429);
            assert!(matches!(classify(&e), Failure::FreeTier { .. }), "{raw}");
        }
        // Details stripped from a parsed body: the message's words decide.
        let mut v: Value = serde_json::from_str(FREE_429_NATIVE).unwrap();
        v["error"].as_object_mut().unwrap().remove("details");
        let mut e = parse_error(&v.to_string());
        e.http = Some(429);
        assert!(matches!(classify(&e), Failure::FreeTier { zero: true, .. }));
        // A daily cap with no free-tier id, as text.
        let mut e = parse_error("quota exceeded, GenerateRequestsPerDayPerProjectPerModel per day");
        e.http = Some(429);
        assert!(matches!(
            classify(&e),
            Failure::RateLimited { daily: true, .. }
        ));
    }

    #[test]
    fn retry_delays_read_from_the_details_and_from_the_message() {
        assert_eq!(seconds_in("70695s"), Some(70695));
        assert_eq!(seconds_in("0.5s"), Some(1));
        assert_eq!(seconds_in("soon"), None);
        assert_eq!(
            retry_in_message("Please retry in 19h38m14.824741283s."),
            Some(70695)
        );
        assert_eq!(retry_in_message("retry in 40m"), Some(2400));
        assert_eq!(retry_in_message("nothing here"), None);
    }

    // ---- the words ------------------------------------------------------------------------------

    #[test]
    fn a_wait_is_never_called_a_minute_when_it_is_hours() {
        assert_eq!(wait_words(Some(70694)), "about 20 hours");
        assert_eq!(wait_words(Some(2400)), "about 40 minutes");
        assert_eq!(wait_words(Some(30)), "about 30 seconds");
        assert_eq!(wait_words(Some(90)), "a minute");
        assert_eq!(wait_words(None), "a minute");
    }

    #[test]
    fn a_free_tier_failure_never_promises_a_short_wait_it_was_not_given() {
        for region in [uk(), nyc(), region_of_on(Some("UTC"), false)] {
            let zero = failure_text(
                &Failure::FreeTier {
                    zero: true,
                    wait: Some(70694),
                },
                &region,
            );
            assert!(
                !zero.contains("shortly") && !zero.contains("a minute"),
                "{zero}"
            );
            let long = failure_text(
                &Failure::FreeTier {
                    zero: false,
                    wait: Some(70694),
                },
                &region,
            );
            assert!(
                !long.contains("a minute") && !long.contains("shortly"),
                "{long}"
            );
        }
        let short = failure_text(
            &Failure::FreeTier {
                zero: false,
                wait: Some(30),
            },
            &nyc(),
        );
        assert!(short.contains("about 30 seconds"), "{short}");
        let hours = failure_text(
            &Failure::FreeTier {
                zero: false,
                wait: Some(70694),
            },
            &nyc(),
        );
        assert!(hours.contains("about 20 hours"), "{hours}");
        let unknown = failure_text(
            &Failure::FreeTier {
                zero: false,
                wait: None,
            },
            &nyc(),
        );
        assert!(unknown.contains("resets it daily"), "{unknown}");
        let daily = failure_text(
            &Failure::RateLimited {
                wait: None,
                daily: true,
            },
            &nyc(),
        );
        assert!(
            daily.contains("daily") && !daily.contains("a minute"),
            "{daily}"
        );
        let plain = failure_text(
            &Failure::RateLimited {
                wait: None,
                daily: false,
            },
            &nyc(),
        );
        assert!(plain.contains("a minute"), "{plain}");
    }

    #[test]
    fn a_free_tier_failure_in_the_region_says_gemini_is_paused_and_names_the_zone() {
        let t = failure_text(
            &Failure::FreeTier {
                zero: true,
                wait: None,
            },
            &uk(),
        );
        assert!(t.starts_with("Gemini is paused:"), "{t}");
        assert!(
            t.contains("the United Kingdom") && t.contains("Europe/London"),
            "{t}"
        );
        let out = failure_text(
            &Failure::FreeTier {
                zero: true,
                wait: None,
            },
            &nyc(),
        );
        assert!(
            !out.contains("paused") && out.contains("no allowance"),
            "{out}"
        );
        let un = failure_text(
            &Failure::FreeTier {
                zero: true,
                wait: None,
            },
            &region_of_on(Some("UTC"), false),
        );
        assert!(
            un.contains("can't tell where this computer is") && un.contains("UTC"),
            "{un}"
        );
    }

    #[test]
    fn every_failure_has_a_sentence_and_none_carries_a_key() {
        let all = [
            Failure::OutOfCredit,
            Failure::KeyRejected,
            Failure::Leaked,
            Failure::NeedsVerification,
            Failure::NotAllowed,
            Failure::FreeTier {
                zero: true,
                wait: None,
            },
            Failure::RateLimited {
                wait: Some(5),
                daily: false,
            },
            Failure::Busy,
            Failure::Trouble,
            Failure::NoFreeTierHere,
            Failure::ModelUnavailable,
            Failure::EffortRefused,
            Failure::ContextTooLong,
            Failure::LimitOutOfDate,
        ];
        for f in &all {
            let t = failure_text(f, &nyc());
            assert!(!t.trim().is_empty() && !t.contains("AIza"), "{f:?}");
        }
        // A message echoing a Google key comes out redacted and cut.
        let key = format!("AIza{}", "x".repeat(35));
        let e = GErr {
            http: Some(418),
            message: format!("bad key {key} supplied"),
            ..GErr::default()
        };
        let t = failure_text(&classify(&e), &nyc());
        assert!(!t.contains(&key) && t.contains("[key]"), "{t}");
    }

    // ---- the passive flip and the latest verdict --------------------------------------------------

    /// Drive a future on a throwaway runtime, so a test that holds the inbox lock never awaits.
    fn run<F: Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }
    fn failed_call(
        status: reqwest::StatusCode,
        raw: &str,
        region: &Region,
    ) -> (Error, Option<Option<u64>>) {
        run(failed_call_async(status, raw, region))
    }

    #[test]
    fn a_free_tier_answer_leaves_a_mark_the_entry_point_passes_on() {
        let _g = lock();
        reset();
        let status = reqwest::StatusCode::TOO_MANY_REQUESTS;
        // A limit-0 answer marks the call, with NO wait: there is no allowance to come back.
        let (err, mark) = failed_call(status, FREE_429_COMPAT, &nyc());
        assert!(err.to_string().contains("no allowance"), "{err}");
        assert_eq!(mark, Some(None));
        // A rate limit that names no free-tier quota marks nothing.
        let rate = r#"{"error":{"code":429,"message":"slow","status":"RESOURCE_EXHAUSTED","details":[{"violations":[{"quotaId":"X-PerMinute"}]}]}}"#;
        assert_eq!(failed_call(status, rate, &nyc()).1, None);
        // Outside any call (a key check) nothing is left anywhere.
        let _ = request_error_in(status, FREE_429_COMPAT, &nyc());
        // after_failure notes it under the ARM'S key.
        let arm = arm_with(KEY_A, "gemini-3.8-flash", Some(KeyTier::Paid), None);
        let fp = direct_providers::key_fp(KEY_A);
        let e = after_failure(
            Error::Other("x".into()),
            &arm,
            gem("gemini-3.8-flash"),
            mark,
        );
        assert_eq!(e.to_string(), "x");
        assert!(direct_providers::effective_free(CloudProvider::Gemini, &fp));
        // No mark: nothing is noted.
        reset();
        let _ = after_failure(
            Error::Other("x".into()),
            &arm,
            gem("gemini-3.8-flash"),
            None,
        );
        assert!(!direct_providers::effective_free(
            CloudProvider::Gemini,
            &fp
        ));
    }

    /// Two calls at once, one per key: the old key's free-tier 429 must not flip the new key, whose
    /// own call failed for an unrelated reason.
    #[test]
    fn one_calls_free_answer_never_flips_another_calls_key() {
        let _g = lock();
        reset();
        let busy = r#"{"error":{"code":503,"message":"high demand","status":"UNAVAILABLE"}}"#;
        let region = nyc();
        let ((_, mark_a), (_, mark_b)) = run(async {
            tokio::join!(
                failed_call_async(
                    reqwest::StatusCode::TOO_MANY_REQUESTS,
                    FREE_429_COMPAT,
                    &region
                ),
                failed_call_async(reqwest::StatusCode::SERVICE_UNAVAILABLE, busy, &region),
            )
        });
        assert_eq!(mark_a, Some(None));
        assert_eq!(mark_b, None, "the other call's mark never reaches this one");
        let arm_b = arm_with(KEY_B, "gemini-3.8-flash", Some(KeyTier::Paid), None);
        let fp_b = direct_providers::key_fp(KEY_B);
        let _ = after_failure(
            Error::Other("x".into()),
            &arm_b,
            gem("gemini-3.8-flash"),
            mark_b,
        );
        assert!(!direct_providers::effective_free(
            CloudProvider::Gemini,
            &fp_b
        ));
        assert_eq!(effective_tier(&arm_b), Some(KeyTier::Paid));
        reset();
    }

    /// A limit-0 answer carries a day-long retry delay, but there is no allowance to wait for: it
    /// must set no local hold, or a person who then pays stays blocked on that model.
    #[test]
    fn a_limit_zero_answer_sets_no_local_hold_but_a_spent_allowance_does() {
        let _g = lock();
        reset();
        let status = reqwest::StatusCode::TOO_MANY_REQUESTS;
        let arm = arm_with(KEY_A, "gemini-3.8-flash", None, None);
        let (_, mark) = failed_call(status, FREE_429_COMPAT, &nyc());
        let _ = after_failure(
            Error::Other("x".into()),
            &arm,
            gem("gemini-3.8-flash"),
            mark,
        );
        assert!(spent_allowance(gem("gemini-3.8-flash")).is_none());
        // A daily allowance really spent (wait over 5 minutes, not limit 0) is held, and the
        // sentence says how to lift the hold after paying.
        let daily = r#"[{"error":{"code":429,"message":"Quota exceeded for metric: generate_content_free_tier_requests, limit: 20","status":"RESOURCE_EXHAUSTED","details":[{"@type":"type.googleapis.com/google.rpc.QuotaFailure","violations":[{"quotaMetric":"generativelanguage.googleapis.com/generate_content_free_tier_requests","quotaId":"GenerateRequestsPerDayPerProjectPerModel-FreeTier"}]},{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"7200s"}]}}]"#;
        let (_, mark) = failed_call(status, daily, &nyc());
        assert_eq!(mark, Some(Some(7200)));
        let _ = after_failure(
            Error::Other("x".into()),
            &arm,
            gem("gemini-3.8-flash"),
            mark,
        );
        let held = spent_allowance(gem("gemini-3.8-flash"))
            .unwrap()
            .to_string();
        assert!(
            held.contains("about 2 hours") && held.contains("Check again"),
            "{held}"
        );
        reset();
    }

    #[test]
    fn a_held_paid_plan_is_refused_at_its_next_send_once_a_call_says_free() {
        let _g = lock();
        reset();
        let arm = arm_with(KEY_A, "gemini-3.8-flash", Some(KeyTier::Paid), None);
        assert!(seam(effective_tier(&arm), &uk()).is_ok());
        direct_providers::note_tier_seen(
            CloudProvider::Gemini,
            KeyTier::Free,
            &direct_providers::key_fp(KEY_A),
            "gemini-3.8-flash",
            None,
        );
        let e = seam(effective_tier(&arm), &uk()).unwrap_err().to_string();
        assert!(
            e.starts_with("Gemini is paused:") && e.contains("Europe/London"),
            "{e}"
        );
        // Outside the region the same arm keeps working.
        assert!(seam(effective_tier(&arm), &nyc()).is_ok());
        // Another key's note does not touch this arm.
        reset();
        direct_providers::record_verdict(
            CloudProvider::Gemini,
            &direct_providers::key_fp(KEY_B),
            KeyTier::Paid,
        );
        direct_providers::note_tier_seen(
            CloudProvider::Gemini,
            KeyTier::Free,
            &direct_providers::key_fp(KEY_A),
            "w",
            None,
        );
        let b = arm_with(KEY_B, "gemini-3.8-flash", Some(KeyTier::Paid), None);
        assert!(
            seam(effective_tier(&b), &uk()).is_ok(),
            "an old key's late failure must not flip the new key"
        );
    }

    #[test]
    fn the_seam_refuses_free_unknown_and_unchecked_keys_in_the_region_only() {
        assert!(seam(Some(KeyTier::Free), &uk()).is_err());
        assert!(seam(Some(KeyTier::Unknown), &uk()).is_err());
        assert!(seam(None, &uk()).is_err(), "no verdict reads as free");
        assert!(seam(Some(KeyTier::Paid), &uk()).is_ok());
        assert!(seam(Some(KeyTier::Free), &nyc()).is_ok());
        assert!(seam(
            Some(KeyTier::Paid),
            &Region::Unplaceable {
                zone: String::new()
            }
        )
        .is_ok());
        assert!(seam(
            None,
            &Region::Unplaceable {
                zone: String::new()
            }
        )
        .is_err());
        let Region::Restricted { kind, .. } = uk() else {
            panic!()
        };
        assert_eq!(kind, RegionKind::Uk);
    }

    #[test]
    fn a_spent_daily_allowance_is_not_asked_again_and_only_for_that_model() {
        let _g = lock();
        reset();
        let fp = direct_providers::key_fp(KEY_A);
        direct_providers::note_tier_seen(
            CloudProvider::Gemini,
            KeyTier::Free,
            &fp,
            "gemini-3.8-flash",
            Some(30),
        );
        assert!(
            spent_allowance(gem("gemini-3.8-flash")).is_none(),
            "30 s is a rate limit, not a spent day"
        );
        direct_providers::note_tier_seen(
            CloudProvider::Gemini,
            KeyTier::Free,
            &fp,
            "gemini-3.8-flash",
            Some(70694),
        );
        let e = spent_allowance(gem("gemini-3.8-flash"))
            .expect("held")
            .to_string();
        assert!(
            e.contains("about 20 hours") && e.contains("won't ask Google again"),
            "{e}"
        );
        assert!(spent_allowance(gem("gemini-3.1-flash-lite")).is_none());
        direct_providers::record_verdict(CloudProvider::Gemini, &fp, KeyTier::Paid);
        assert!(
            spent_allowance(gem("gemini-3.8-flash")).is_none(),
            "a fresh verdict clears it"
        );
    }

    // ---- the key check ------------------------------------------------------------------------------

    #[test]
    fn the_list_decides_what_it_can() {
        use KeyOutcome::*;
        use ListVerdict::*;
        assert_eq!(interpret_list(200, LIST_BODY), Proceed);
        assert_eq!(
            interpret_list(200, "<html>welcome</html>"),
            Done(Unexpected(200), None)
        );
        assert_eq!(interpret_list(200, "{}"), Done(Unexpected(200), None));
        assert_eq!(interpret_list(400, BAD_KEY_400), Done(Rejected, None));
        assert_eq!(
            interpret_list(400, &format!("[{BAD_KEY_400}]")),
            Done(Rejected, None)
        );
        assert_eq!(interpret_list(401, ""), Done(Rejected, None));
        let region = r#"{"error":{"code":400,"message":"Gemini API free tier is not available in your country. Please enable billing on your project in Google AI Studio.","status":"FAILED_PRECONDITION"}}"#;
        assert_eq!(
            interpret_list(400, region),
            Done(RegionBlocked, Some(KeyTier::Free))
        );
        assert_eq!(
            interpret_list(
                402,
                r#"{"error":{"code":402,"status":"RESOURCE_EXHAUSTED"}}"#
            ),
            Done(NoCredit, Some(KeyTier::Paid))
        );
        let verif = r#"{"error":{"code":403,"message":"This project needs verification","status":"PERMISSION_DENIED"}}"#;
        assert_eq!(
            interpret_list(403, verif),
            Done(NeedsVerification, Some(KeyTier::Free))
        );
        let leaked = r#"{"error":{"code":403,"message":"Your API key was reported as leaked.","status":"PERMISSION_DENIED"}}"#;
        assert_eq!(
            interpret_list(403, leaked),
            Done(Rejected, None),
            "never a quiet keep"
        );
        assert_eq!(
            interpret_list(403, r#"{"error":{"code":403,"message":"nope"}}"#),
            Done(Rejected, None)
        );
        for s in [408, 429, 500, 503] {
            assert_eq!(interpret_list(s, ""), Done(Busy, None), "{s}");
        }
        assert_eq!(interpret_list(418, ""), Done(Unexpected(418), None));
    }

    #[test]
    fn a_probe_is_paid_only_when_google_really_answered() {
        use Decision::*;
        use Skip::*;
        assert_eq!(
            interpret_probe(200, PAID_NATIVE_PRO),
            Answer::Decisive(Paid)
        );
        assert_eq!(
            interpret_probe(200, PAID_NATIVE_FLASH),
            Answer::Decisive(Paid)
        );
        assert_eq!(
            interpret_probe(200, r#"{"usageMetadata":{}}"#),
            Answer::Decisive(Paid)
        );
        // A bare, HTML or otherwise odd 200 is not proof of a paid key.
        for odd in ["{}", "<html>hi</html>", r#"{"ok":true}"#, ""] {
            assert_eq!(interpret_probe(200, odd), Answer::Skip(Odd), "{odd}");
        }
        assert_eq!(interpret_probe(402, ""), Answer::Decisive(OutOfCredit));
        for free in [FREE_429_NATIVE.to_string(), FREE_429_COMPAT.to_string()] {
            assert_eq!(interpret_probe(429, &free), Answer::Decisive(Free));
        }
        let mut stripped: Value = serde_json::from_str(FREE_429_NATIVE).unwrap();
        stripped["error"].as_object_mut().unwrap().remove("details");
        assert_eq!(
            interpret_probe(429, &stripped.to_string()),
            Answer::Decisive(Free)
        );
        assert_eq!(
            interpret_probe(429, "Quota exceeded on Free Tier"),
            Answer::Decisive(Free)
        );
        let paid_cap = r#"{"error":{"code":429,"message":"slow down","details":[{"violations":[{"quotaId":"GenerateRequestsPerMinutePerProjectPerModel"}]}]}}"#;
        assert_eq!(interpret_probe(429, paid_cap), Answer::Skip(Busy));
        assert_eq!(interpret_probe(503, BUSY_503_COMPAT), Answer::Skip(Busy));
        assert_eq!(interpret_probe(408, ""), Answer::Skip(Busy));
        assert_eq!(interpret_probe(404, ""), Answer::Skip(NotHere));
        let maxout = r#"{"error":{"code":400,"message":"maxOutputTokens must be larger","status":"INVALID_ARGUMENT"}}"#;
        assert_eq!(interpret_probe(400, maxout), Answer::Skip(Odd));
        assert_eq!(
            interpret_probe(403, r#"{"error":{"code":403,"message":"no"}}"#),
            Answer::Skip(Odd)
        );
        assert_eq!(
            interpret_probe(
                403,
                r#"{"error":{"code":403,"message":"verification required"}}"#
            ),
            Answer::Decisive(NeedsVerification)
        );
    }

    #[test]
    fn every_listed_probe_model_is_asked_and_one_bad_second_does_not_decide_the_tier() {
        use Answer::{Decisive, Skip as S};
        use Decision::*;
        use Skip::*;
        let probe = |a: &[Answer]| settle_probes(a);
        assert_eq!(
            probe(&[S(NotHere), Decisive(Paid)]),
            Probe::Tier(KeyTier::Paid)
        );
        assert_eq!(
            probe(&[S(Busy), S(Busy), S(NotHere), Decisive(Paid)]),
            Probe::Tier(KeyTier::Paid)
        );
        assert_eq!(probe(&[S(Odd), Decisive(Paid)]), Probe::Tier(KeyTier::Paid));
        assert_eq!(
            probe(&[Decisive(Free), Decisive(Paid)]),
            Probe::Tier(KeyTier::Free),
            "the first decisive answer wins"
        );
        // Nothing decided: busy, never unknown; unreachable only when nothing answered.
        assert_eq!(probe(&[S(Busy); 4]), Probe::Busy);
        assert_eq!(probe(&[S(Transport); 4]), Probe::Unreachable);
        assert_eq!(probe(&[S(Transport), S(Busy)]), Probe::Busy);
        assert_eq!(probe(&[S(NotHere); 4]), Probe::Tier(KeyTier::Unknown));
        assert_eq!(probe(&[S(Odd), S(Busy)]), Probe::Tier(KeyTier::Unknown));
        assert_eq!(
            probe(&[]),
            Probe::Tier(KeyTier::Unknown),
            "no probe model listed"
        );
        assert_eq!(probe(&[Decisive(OutOfCredit)]), Probe::OutOfCredit);
        assert_eq!(
            probe(&[Decisive(NeedsVerification)]),
            Probe::NeedsVerification
        );
    }

    #[test]
    fn the_key_check_lists_only_what_the_catalogue_knows_and_the_key_can_chat() {
        let (listed, total) = listed_models(LIST_BODY);
        assert_eq!(
            total, 5,
            "the five that can generateContent, the TTS id included"
        );
        let mut ids = listed.clone();
        ids.sort();
        assert_eq!(
            ids,
            [
                "gemini/gemini-3.1-flash-lite",
                "gemini/gemini-3.1-pro-preview",
                "gemini/gemini-3.8-flash"
            ]
        );
        assert_eq!(listed_models("not json"), (Vec::new(), 0));
        let order = probe_order(&listed);
        assert_eq!(order.len(), 1);
        assert_eq!(order[0].id, "gemini/gemini-3.1-pro-preview");
    }

    #[test]
    fn a_key_fingerprint_is_stable_short_and_not_the_key() {
        let a = direct_providers::key_fp(KEY_A);
        assert_eq!(a, direct_providers::key_fp(KEY_A));
        assert_eq!(a.len(), 16);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(
            a,
            direct_providers::key_fp(&format!("{}x", &KEY_A[..KEY_A.len() - 1]))
        );
        assert!(!KEY_A.contains(&a));
    }

    #[test]
    fn a_google_key_is_the_shape_of_one_and_another_companys_is_not() {
        assert!(well_formed(&format!("AIza{}", "x".repeat(35))));
        assert!(well_formed("  AIzaSyAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA  "));
        assert!(!well_formed("short"));
        assert!(!well_formed(&"a".repeat(301)));
        assert!(!well_formed("AIzaSy with a space inside the key itself"));
        assert!(
            !well_formed(&format!("sk-proj-{}", "a".repeat(40))),
            "an OpenAI or OpenRouter key"
        );
        assert!(!well_formed("AIza\u{e9}xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"));
    }

    // ---- the wire ---------------------------------------------------------------------------------------

    /// The weekly provider canary sends these bodies (tests/golden/request_bodies.json), so they are
    /// built here by the real builders and compared byte for byte. 3.5 Flash-Lite is the canary's
    /// model; `none` is a 400 on it, so its lowest listed level (`minimal`) is the one that is sent.
    #[test]
    fn request_bodies_match_the_canary_golden() {
        use crate::golden_request_bodies::{assert_matches, SYSTEM, USER};
        let say = vec![
            ChatMessage {
                role: "system".into(),
                content: SYSTEM.into(),
            },
            ChatMessage {
                role: "user".into(),
                content: USER.into(),
            },
        ];
        let m = gem("gemini-3.5-flash-lite");
        assert_eq!(m.effort_default.as_deref(), Some("minimal"));
        let probe: Value = serde_json::from_str(PROBE_BODY).unwrap();
        assert_matches(
            "gemini",
            &[
                (
                    "stream",
                    chat_body(m, Some("minimal"), &say, true, Some(0), 256),
                ),
                (
                    "plain",
                    chat_body(m, Some("minimal"), &say, false, None, 256),
                ),
                ("tier_probe", probe),
            ],
        );
    }

    #[test]
    fn the_body_is_what_pm_means_to_send_and_nothing_else() {
        let m = gem("gemini-3.8-flash");
        let body = chat_body(m, None, &msgs(), true, Some(0), m.max_output);
        assert_eq!(
            serde_json::to_string(&body).unwrap(),
            format!(
                r#"{{"max_tokens":{},"messages":[{{"content":"s","role":"system"}},{{"content":"u","role":"user"}}],"model":"gemini-3.8-flash","stream":true,"stream_options":{{"include_usage":true}}}}"#,
                m.max_output
            )
        );
        let plain = chat_body(m, Some("low"), &msgs(), false, None, 10);
        assert_eq!(plain["reasoning_effort"], "low");
        assert!(plain.get("stream_options").is_none());
        for field in [
            "store",
            "max_completion_tokens",
            "thinking_config",
            "temperature",
            "tools",
            "cache_control",
            "provider",
            "models",
        ] {
            assert!(
                body.get(field).is_none() && plain.get(field).is_none(),
                "{field}"
            );
        }
        assert!(!body.to_string().contains("cache_control"));
    }

    /// A level the model doesn't list never goes out: Google would 400 it in someone's chat.
    #[test]
    fn a_level_a_model_does_not_list_never_goes_out() {
        let wire = |model: &str, level: &str| {
            chat_body(gem(model), Some(level), &msgs(), true, None, 10)
                .get("reasoning_effort")
                .cloned()
        };
        assert_eq!(wire("gemini-3.8-flash", "low"), Some(json!("low")));
        assert_eq!(wire("gemini-3.8-flash", "minimal"), None);
        assert_eq!(wire("gemini-4-carbon", "minimal"), None);
        assert_eq!(wire("gemini-4-carbon", "xhigh"), None);
        assert_eq!(wire("gemini-4-neon", "xhigh"), Some(json!("xhigh")));
        assert_eq!(
            wire("gemini-3.1-flash-lite", "minimal"),
            Some(json!("minimal"))
        );
        for m in direct_models::models(CloudProvider::Gemini) {
            assert_eq!(
                chat_body(m, Some("none"), &msgs(), true, None, 10).get("reasoning_effort"),
                None,
                "{}",
                m.id
            );
        }
        let arm = arm_with(
            KEY_A,
            "gemini-3.8-flash",
            Some(KeyTier::Paid),
            Some("minimal"),
        );
        assert_eq!(wire_effort(gem("gemini-3.8-flash"), &arm), None);
        let arm = arm_with(KEY_A, "gemini-3.8-flash", Some(KeyTier::Paid), Some("high"));
        assert_eq!(wire_effort(gem("gemini-3.8-flash"), &arm), Some("high"));
    }

    #[test]
    fn the_endpoint_is_googles_and_the_output_limit_is_the_models_lowered_to_a_bound() {
        assert!(FLAVOUR
            .endpoint
            .starts_with("https://generativelanguage.googleapis.com/"));
        let m = gem("gemini-3.8-flash");
        assert_eq!(output_limit(m, None), m.max_output);
        assert_eq!(output_limit(m, Some(500)), 500);
        assert_eq!(output_limit(m, Some(u32::MAX)), m.max_output);
    }

    #[test]
    fn an_arm_naming_a_model_not_in_the_catalogue_is_refused_before_the_network() {
        for id in ["gemini/gemini-9", "openai/gpt-6.2-sol", ""] {
            let mut arm = arm_with(KEY_A, "x", Some(KeyTier::Paid), None);
            arm.models = vec![id.into()];
            let e = requested_model(&arm).unwrap_err().to_string();
            assert!(
                e.contains("PM no longer lists") && e.contains("Google Gemini"),
                "{e}"
            );
        }
    }

    // ---- usage and cost -----------------------------------------------------------------------------------

    fn reply(body: &str) -> Value {
        serde_json::from_str(body).unwrap()
    }

    #[test]
    fn thinking_counts_in_the_total_so_output_is_total_minus_prompt() {
        let u = usage_from(&reply(PAID_USAGE_LENGTH));
        assert_eq!((u.prompt_tokens, u.completion_tokens), (Some(5), Some(29)));
        let u = usage_from(&reply(PAID_USAGE_OK));
        assert_eq!((u.prompt_tokens, u.completion_tokens), (Some(5), Some(27)));
        assert!(!u.output_partial);
        // A Google fix that counts thoughts in completion_tokens can't make PM under-count.
        let u = usage_from(
            &json!({"usage":{"prompt_tokens":5,"completion_tokens":40,"total_tokens":45}}),
        );
        assert_eq!(u.completion_tokens, Some(40));
        // Nothing reported: all None.
        let u = usage_from(&json!({}));
        assert_eq!(
            (u.prompt_tokens, u.completion_tokens, u.output_partial),
            (None, None, false)
        );
    }

    #[test]
    fn a_reply_with_a_prompt_but_no_total_reads_as_a_dash_never_a_priced_undercount() {
        let u = usage_from(&json!({"usage":{"prompt_tokens":5,"completion_tokens":1}}));
        assert!(u.output_partial);
        let c = Completion {
            text: "x".into(),
            model: Some("gemini-3.8-flash".into()),
            usage: u,
            truncated: false,
        };
        let _g = lock();
        reset();
        let done = settle_on(
            c,
            gem("gemini-3.8-flash"),
            Some(KeyTier::Paid),
            KEY_A,
            "2026-11-26",
        );
        assert_eq!(done.usage.cost, None);
        assert_eq!(done.model.as_deref(), Some("gemini/gemini-3.8-flash"));
    }

    /// The all-thinking reply (no `content` key at all, `finish_reason: length`) is an EMPTY TRUNCATED
    /// completion through the real stream parser, not a crash on the missing key, and it then reaches
    /// `empty_reply_error` ("stopped before it wrote an answer"). `complete` is a drained stream, so
    /// this is the path both entry points take. The streamed final chunk is built from the REAL
    /// reply's finish reason and usage numbers (the real STREAMED chunks are in
    /// `a_real_stream_carries_total_tokens_on_every_chunk` below).
    #[tokio::test]
    async fn the_all_thinking_reply_is_an_empty_truncated_completion() {
        let real = reply(PAID_USAGE_LENGTH);
        let chunk = json!({
            "choices": [{"delta": {"role": "assistant"}, "finish_reason": real["choices"][0]["finish_reason"], "index": 0}],
            "model": real["model"],
            "usage": real["usage"],
        });
        let sse = format!("data: {chunk}\n\ndata: [DONE]\n\n");
        let (url, _) = serve_scripted(vec![(200, sse)]);
        let flavour = Flavour {
            endpoint: url,
            ..flavour_copy()
        };
        let body = chat_body(gem("gemini-3.1-pro-preview"), None, &msgs(), true, None, 10);
        let c = cloud_openai::stream(&flavour, KEY_A, &body, |_| {})
            .await
            .expect("a truncated reply with no content key is not an error");
        assert!(c.truncated && c.text.is_empty());
        assert_eq!(
            (c.usage.prompt_tokens, c.usage.completion_tokens),
            (Some(5), Some(29))
        );
        assert_eq!(c.model.as_deref(), Some("gemini-3.1-pro-preview"));
    }

    /// A mid-stream error in Google's array wrapping ends the stream in an error (never a truncated
    /// "success"), told in Gemini's own words.
    #[tokio::test]
    async fn an_array_wrapped_mid_stream_error_fails_the_stream_in_geminis_words() {
        // A busy answer leaves no free-tier mark, so this needs no hold on the shared inbox (which
        // must not be held across the await).
        let sse = format!(
            "data: {}\n\ndata: [DONE]\n\n",
            BUSY_503_COMPAT.replace('\n', "")
        );
        let (url, _) = serve_scripted(vec![(200, sse)]);
        let flavour = Flavour {
            endpoint: url,
            ..flavour_copy()
        };
        let body = chat_body(gem("gemini-3.8-flash"), None, &msgs(), true, None, 10);
        let e = cloud_openai::stream(&flavour, KEY_A, &body, |_| {})
            .await
            .err()
            .expect("an error, not a reply");
        assert!(e.to_string().contains("busy right now"), "{e}");
    }

    #[test]
    fn what_a_reply_costs_follows_the_tier_the_day_and_the_model() {
        let _g = lock();
        reset();
        let one_m = |model: &str| Completion {
            text: String::new(),
            model: Some(model.into()),
            usage: Usage {
                prompt_tokens: Some(1_000_000),
                completion_tokens: Some(1_000_000),
                ..Usage::default()
            },
            truncated: false,
        };
        let near = |c: Completion, v: f64| (c.usage.cost.unwrap() - v).abs() < 1e-9;
        let m = gem("gemini-3.8-flash");
        // Paid and unknown keep the estimate; the price is the day's.
        assert!(near(
            settle_on(
                one_m("gemini-3.8-flash"),
                m,
                Some(KeyTier::Paid),
                KEY_A,
                "2026-12-31"
            ),
            0.75 + 3.75
        ));
        assert!(near(
            settle_on(
                one_m("gemini-3.8-flash"),
                m,
                Some(KeyTier::Unknown),
                KEY_A,
                "2027-01-01"
            ),
            1.5 + 7.5
        ));
        assert!(near(
            settle_on(one_m("gemini-3.8-flash"), m, None, KEY_A, "2026-12-31"),
            0.75 + 3.75
        ));
        // A key PM knows is free costs nothing on a free-tier model.
        assert!(near(
            settle_on(
                one_m("gemini-3.8-flash"),
                m,
                Some(KeyTier::Free),
                KEY_A,
                "2026-12-31"
            ),
            0.0
        ));
        // The model is always a catalogue id, even when Google names something PM doesn't list.
        let c = settle_on(
            one_m("mystery-1"),
            m,
            Some(KeyTier::Paid),
            KEY_A,
            "2026-12-31",
        );
        assert_eq!(c.model.as_deref(), Some("gemini/gemini-3.8-flash"));
        // A success on a paid-only model keeps the estimate AND proves the key is paid. A 1M-token
        // prompt is over 3.1 Pro's 200,000 line, so the whole request is 4 in and 18 out.
        let pro = gem("gemini-3.1-pro-preview");
        let fp = direct_providers::key_fp(KEY_A);
        direct_providers::note_tier_seen(CloudProvider::Gemini, KeyTier::Free, &fp, "w", None);
        assert!(direct_providers::effective_free(CloudProvider::Gemini, &fp));
        let c = settle_on(
            one_m("gemini-3.1-pro-preview"),
            pro,
            Some(KeyTier::Free),
            KEY_A,
            "2026-11-26",
        );
        assert!(near(c, 4.0 + 18.0));
        assert!(
            !direct_providers::effective_free(CloudProvider::Gemini, &fp),
            "a paid-only success cleared the flip"
        );
    }

    fn flavour_copy() -> Flavour {
        Flavour {
            name: FLAVOUR.name,
            endpoint: ENDPOINT,
            auth: FLAVOUR.auth,
            cache_control_blocks: FLAVOUR.cache_control_blocks,
            decorate: FLAVOUR.decorate,
            request_error: FLAVOUR.request_error,
            max_tokens_field: FLAVOUR.max_tokens_field,
            patient: FLAVOUR.patient,
            stream_error: FLAVOUR.stream_error,
            stop_error: FLAVOUR.stop_error,
            usage: FLAVOUR.usage,
        }
    }

    /// Serve the replies in order, one connection each, on loopback, and count the requests. Plain std
    /// sockets on a thread: no extra dependency. Each request is read through its body so the client
    /// is never answered before it has finished sending.
    fn serve_scripted(
        replies: Vec<(u16, String)>,
    ) -> (&'static str, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = seen.clone();
        std::thread::spawn(move || {
            for (status, body) in replies {
                let Ok((mut sock, _)) = listener.accept() else {
                    return;
                };
                let mut got: Vec<u8> = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    let n = sock.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    got.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&got).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let want = text[..end]
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if got.len() >= end + 4 + want {
                            break;
                        }
                    }
                }
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes());
                let _ = sock.write_all(body.as_bytes());
            }
        });
        let url: &'static str = Box::leak(format!("http://{addr}").into_boxed_str());
        (url, seen)
    }

    // ---- the key check end to end, against a scripted server -----------------------------------------------

    fn urls(base: &'static str) -> CheckUrls {
        CheckUrls {
            list: base,
            models_base: base,
        }
    }

    #[tokio::test]
    async fn a_free_key_is_told_apart_and_the_probe_stops_at_the_first_decisive_answer() {
        let (url, seen) = serve_scripted(vec![
            (200, LIST_BODY.into()),
            (429, FREE_429_NATIVE.into()),
            (200, PAID_NATIVE_PRO.into()),
        ]);
        let check = check_key_at(KEY_A, &urls(url)).await;
        assert_eq!(check.outcome, KeyOutcome::Ok);
        assert_eq!(check.tier, Some(KeyTier::Free));
        assert_eq!(check.listed_total, 5);
        assert_eq!(check.listed.len(), 3);
        assert_eq!(
            seen.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "the list, then ONE probe"
        );
    }

    #[tokio::test]
    async fn a_paid_key_reads_paid_and_a_busy_second_is_not_a_tier() {
        let (url, _) = serve_scripted(vec![(200, LIST_BODY.into()), (200, PAID_NATIVE_PRO.into())]);
        let c = check_key_at(KEY_A, &urls(url)).await;
        assert_eq!((c.outcome, c.tier), (KeyOutcome::Ok, Some(KeyTier::Paid)));
        // Only one probe model is listed and it is busy: nothing was decided.
        let (url, _) = serve_scripted(vec![(200, LIST_BODY.into()), (503, BUSY_503_NATIVE.into())]);
        let c = check_key_at(KEY_A, &urls(url)).await;
        assert_eq!((c.outcome, c.tier), (KeyOutcome::Busy, None));
    }

    /// With all four probe models listed, every one is tried until one decides, so one model's bad
    /// second can't decide a person's tier.
    #[tokio::test]
    async fn all_four_probe_models_are_asked_when_none_of_the_first_three_decides() {
        let all_four = r#"{"models":[
            {"name":"models/gemini-3.1-pro-preview","supportedGenerationMethods":["generateContent"]},
            {"name":"models/gemini-4-carbon","supportedGenerationMethods":["generateContent"]},
            {"name":"models/gemini-4-argon","supportedGenerationMethods":["generateContent"]},
            {"name":"models/gemini-4-neon","supportedGenerationMethods":["generateContent"]}]}"#;
        let (url, seen) = serve_scripted(vec![
            (200, all_four.into()),
            (503, BUSY_503_NATIVE.into()),
            (404, "{}".into()),
            (503, BUSY_503_NATIVE.into()),
            (200, PAID_NATIVE_PRO.into()),
        ]);
        let c = check_key_at(KEY_A, &urls(url)).await;
        assert_eq!((c.outcome, c.tier), (KeyOutcome::Ok, Some(KeyTier::Paid)));
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 5);
    }

    #[tokio::test]
    async fn a_bad_key_and_a_402_stop_at_the_list_without_a_probe() {
        let (url, seen) = serve_scripted(vec![(400, BAD_KEY_400.into())]);
        let c = check_key_at(KEY_A, &urls(url)).await;
        assert_eq!(c.outcome, KeyOutcome::Rejected);
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 1);
        let (url, seen) = serve_scripted(vec![(
            402,
            r#"{"error":{"code":402,"status":"RESOURCE_EXHAUSTED"}}"#.into(),
        )]);
        let c = check_key_at(KEY_A, &urls(url)).await;
        assert_eq!(
            (c.outcome, c.tier),
            (KeyOutcome::NoCredit, Some(KeyTier::Paid))
        );
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    // ---- build day: Google's real replies, 2026-11-26 -----------------------------------------------------

    /// Step 2, paid key: every probe model answered 200 with BOTH `candidates` and `usageMetadata`
    /// (an empty reply cut at MAX_TOKENS, `serviceTier: "standard"`; 3.1 Pro counts no candidate
    /// token, the Gemini 4 models count one). The shape rule for a Paid verdict is therefore met by
    /// all four, and nothing narrower is required: the free key's calls never got a 200 to compare
    /// (its native step 3 timed out), so no field is known to be one only a paid reply carries.
    #[test]
    fn the_real_paid_probe_replies_read_as_paid_on_every_probe_model() {
        for (wire, body) in [
            ("gemini-3.1-pro-preview", PAID_PROBE_200_PRO),
            ("gemini-4-carbon", PAID_PROBE_200_CARBON),
            ("gemini-4-argon", PAID_PROBE_200_ARGON),
            ("gemini-4-neon", PAID_PROBE_200_NEON),
        ] {
            assert_eq!(
                interpret_probe(200, body),
                Answer::Decisive(Decision::Paid),
                "{wire}"
            );
            let v = reply(body);
            assert!(v["candidates"].is_array(), "{wire}");
            assert_eq!(v["modelVersion"], wire);
            assert_eq!(v["usageMetadata"]["serviceTier"], "standard", "{wire}");
        }
        assert!(reply(PAID_PROBE_200_PRO)["usageMetadata"]["candidatesTokenCount"].is_null());
        // The same four, in the catalogue's order, are the ones the check asks.
        let section = direct_models::section(CloudProvider::Gemini).unwrap();
        assert_eq!(
            section.probe_models,
            [
                "gemini/gemini-3.1-pro-preview",
                "gemini/gemini-4-carbon",
                "gemini/gemini-4-argon",
                "gemini/gemini-4-neon"
            ]
        );
    }

    /// Step 2, free key: every probe model answered a limit-0 429 naming only `-FreeTier` quotas, so
    /// the catalogue's probe list is right and none of them has a free allowance. The script that
    /// captured these cut each reply at 4,000 bytes, so none is whole JSON: they are what a body cut
    /// short really looks like, and the free-tier words in the message decide.
    #[test]
    fn the_real_free_probe_replies_read_as_free_with_limit_zero_on_every_probe_model() {
        for (wire, body) in [
            ("gemini-3.1-pro", FREE_PROBE_429_PRO),
            ("gemini-4-carbon", FREE_PROBE_429_CARBON),
            ("gemini-4-argon", FREE_PROBE_429_ARGON),
            ("gemini-4-neon", FREE_PROBE_429_NEON),
        ] {
            assert_eq!(
                interpret_probe(429, body),
                Answer::Decisive(Decision::Free),
                "{wire}"
            );
            assert!(
                serde_json::from_str::<Value>(body).is_err(),
                "{wire}: cut short at 4,000 bytes"
            );
            let mut e = parse_error(body);
            e.http = Some(429);
            assert!(e.names_free_tier(), "{wire}");
            assert!(
                matches!(classify(&e), Failure::FreeTier { zero: true, .. }),
                "{wire}"
            );
            assert!(
                e.message.contains(&format!("limit: 0, model: {wire}")),
                "{wire}"
            );
            // Whatever quota ids survive the cut (the 3.1 Pro reply was cut before the first) are all
            // `-FreeTier`.
            let ids: Vec<&str> = body
                .lines()
                .filter_map(|l| l.trim().strip_prefix("\"quotaId\": \""))
                .map(|l| l.trim_end_matches(['"', ',']))
                .collect();
            assert!(
                ids.iter().all(|id| id.ends_with("-FreeTier")),
                "{wire}: {ids:?}"
            );
            assert!(
                matches!(classify(&e), Failure::FreeTier { zero: true, .. }),
                "{wire}"
            );
        }
    }

    /// Step 4, free key after 32 calls: a REAL exhausted allowance (limit 15) is a free-tier answer
    /// but not a zero one, with a wait of 28 seconds that PM may quote as seconds, and it is
    /// array-wrapped like every compatible error.
    #[test]
    fn a_real_exhausted_free_allowance_has_a_limit_and_a_short_wait() {
        let _g = lock();
        reset();
        let e = parse_error(FREE_LIMIT_15_429_COMPAT);
        assert_eq!(e.http, Some(429));
        assert_eq!(
            e.quota_ids,
            ["GenerateRequestsPerMinutePerProjectPerModel-FreeTier"]
        );
        assert_eq!(e.retry_secs, Some(28));
        assert_eq!(
            classify(&e),
            Failure::FreeTier {
                zero: false,
                wait: Some(28)
            }
        );
        // A probe that met this would still be a free key (it names the free tier).
        assert_eq!(
            interpret_probe(429, FREE_LIMIT_15_429_COMPAT),
            Answer::Decisive(Decision::Free)
        );
        let status = reqwest::StatusCode::TOO_MANY_REQUESTS;
        let outside = request_error_in(status, FREE_LIMIT_15_429_COMPAT, &nyc()).to_string();
        assert!(outside.contains("used up for the moment"), "{outside}");
        assert!(outside.contains("about 28 seconds"), "{outside}");
        assert!(
            !outside.contains("no allowance"),
            "not a limit-0 answer: {outside}"
        );
        let inside = request_error_in(status, FREE_LIMIT_15_429_COMPAT, &uk()).to_string();
        assert!(inside.starts_with("Gemini is paused"), "{inside}");
        let (_, mark) = failed_call(status, FREE_LIMIT_15_429_COMPAT, &nyc());
        assert_eq!(
            mark,
            Some(Some(28)),
            "a real wait is kept for the entry point"
        );
        reset();
    }

    /// Step 5: Google's words for a level a model refuses, array-wrapped. PM never sends one (the
    /// catalogue and `wire_effort` see to that), so this is the sentence a stale setting would meet.
    #[test]
    fn google_refuses_a_level_in_its_own_words_and_pm_tells_it() {
        let status = reqwest::StatusCode::BAD_REQUEST;
        for body in [EFFORT_400_MINIMAL_HYDROGEN, EFFORT_400_XHIGH_CARBON] {
            let e = parse_error(body);
            assert_eq!(e.http, Some(400));
            assert!(e.message.starts_with("Thinking level "), "{}", e.message);
            assert_eq!(classify(&e), Failure::EffortRefused);
            let text = request_error_in(status, body, &nyc()).to_string();
            assert!(text.contains("thinking level"), "{text}");
        }
        // The two `none` refusals say nothing about a level, so they are plain Google errors.
        for (body, words) in [
            (EFFORT_400_NONE_PRO, "Budget 0 is invalid"),
            (EFFORT_400_NONE_LITE_35, "invalid argument"),
        ] {
            let e = parse_error(body);
            assert!(e.message.contains(words), "{}", e.message);
            let text = request_error_in(status, body, &nyc()).to_string();
            assert!(
                text.starts_with("Google Gemini answered with an error (400)"),
                "{text}"
            );
        }
    }

    /// Step 9: `store: false` is a 400 on Google's compatible endpoint, so a body that carried it
    /// would fail every chat. No row's body has it, on any path.
    #[test]
    fn the_real_store_400_is_why_no_gemini_body_carries_store() {
        let e = parse_error(STORE_400);
        assert_eq!(e.http, Some(400));
        assert!(
            e.message.contains("Unknown name \"store\""),
            "{}",
            e.message
        );
        for m in direct_models::models(CloudProvider::Gemini) {
            for stream in [true, false] {
                let body = chat_body(m, Some("low"), &msgs(), stream, Some(0), m.max_output);
                assert!(body.get("store").is_none(), "{}", m.id);
                assert!(!body.to_string().contains("\"store\""), "{}", m.id);
            }
        }
    }

    /// Step 1 and 9: each row sends its OWN listed limit as `max_tokens` (Carbon and Argon stop at
    /// 32,768), and that limit is what Google accepted on all eight.
    #[test]
    fn each_row_sends_its_own_listed_output_limit() {
        for m in direct_models::models(CloudProvider::Gemini) {
            let want = if m.wire == "gemini-4-carbon" || m.wire == "gemini-4-argon" {
                32_768
            } else {
                65_536
            };
            let body = chat_body(m, None, &msgs(), true, None, output_limit(m, None));
            assert_eq!(body["max_tokens"], want, "{}", m.id);
            assert_eq!(output_limit(m, Some(100_000)), want, "{}", m.id);
        }
    }

    /// Step 10: a made-up key, on every path. The native list answers 400 `API_KEY_INVALID`, the
    /// compatible `/models` a plain object and the compatible chat an ARRAY, the last two with only
    /// "Please pass a valid API key" and no reason. All of them are a rejected key.
    #[test]
    fn a_made_up_key_is_a_rejected_key_on_every_path() {
        assert_eq!(
            interpret_list(400, BAD_KEY_NATIVE_LIST_400),
            ListVerdict::Done(KeyOutcome::Rejected, None)
        );
        for body in [
            BAD_KEY_NATIVE_LIST_400,
            BAD_KEY_COMPAT_MODELS_400,
            BAD_KEY_COMPAT_CHAT_400,
        ] {
            let e = parse_error(body);
            assert_eq!(e.http, Some(400));
            assert_eq!(classify(&e), Failure::KeyRejected, "{body}");
        }
        let text = request_error_in(
            reqwest::StatusCode::BAD_REQUEST,
            BAD_KEY_COMPAT_CHAT_400,
            &nyc(),
        )
        .to_string();
        assert!(text.contains("isn't right any more"), "{text}");
        // And the probe: a made-up key on a model is not "paid", not "free": it is odd.
        assert_eq!(
            interpret_probe(400, BAD_KEY_NATIVE_LIST_400),
            Answer::Skip(Skip::Odd)
        );
    }

    /// Step 6, the real streamed chunks: EVERY chunk carries `usage` with `total_tokens` (cumulative),
    /// the last one repeating it, and 115 tokens of thinking show only as total minus prompt minus
    /// completion. Through the real stream parser the output is `total - prompt` (118), never the 3
    /// the chunk calls the completion, and the price follows.
    #[tokio::test]
    async fn a_real_stream_carries_total_tokens_on_every_chunk() {
        let chunks: Vec<Value> = STREAM_STEP6
            .lines()
            .filter_map(|l| l.strip_prefix("data: "))
            .filter(|d| *d != "[DONE]")
            .map(|d| serde_json::from_str(d).unwrap())
            .collect();
        assert_eq!(chunks.len(), 2);
        for c in &chunks {
            assert_eq!(
                (
                    c["usage"]["prompt_tokens"].as_i64(),
                    c["usage"]["completion_tokens"].as_i64(),
                    c["usage"]["total_tokens"].as_i64()
                ),
                (Some(32), Some(3), Some(150))
            );
        }
        let sse = STREAM_STEP6.replace("\n\n", "\n").replace('\n', "\n\n");
        let (url, _) = serve_scripted(vec![(200, sse)]);
        let flavour = Flavour {
            endpoint: url,
            ..flavour_copy()
        };
        let body = chat_body(gem("gemini-3.8-flash"), None, &msgs(), true, None, 100);
        let mut text = String::new();
        let c = cloud_openai::stream(&flavour, KEY_A, &body, |t| text.push_str(t))
            .await
            .expect("the real stream reads");
        assert_eq!(text, "18 pounds");
        assert_eq!(
            (c.usage.prompt_tokens, c.usage.completion_tokens),
            (Some(32), Some(118))
        );
        assert!(!c.usage.output_partial, "the total WAS there");
        let _g = lock();
        reset();
        let done = settle_on(
            c,
            gem("gemini-3.8-flash"),
            Some(KeyTier::Paid),
            KEY_A,
            "2026-11-26",
        );
        let want = (32.0 * 0.75 + 118.0 * 3.75) / 1_000_000.0;
        assert!(
            (done.usage.cost.unwrap() - want).abs() < 1e-12,
            "{:?}",
            done.usage.cost
        );
    }

    /// Step 7: the second of two identical ~6.9K-token prompts reports `cached_tokens` 4861 INSIDE
    /// `prompt_tokens` 6906; the first has no `prompt_tokens_details` at all. The cached share is
    /// priced at the cached price when the row has one and at the full input price when it hasn't, so
    /// the figure is never below what Google bills.
    #[test]
    fn cached_tokens_are_inside_the_prompt_and_never_under_count() {
        let first = usage_from(&reply(PAID_CACHED_FIRST));
        let second = usage_from(&reply(PAID_CACHED_SECOND));
        assert_eq!(
            (first.prompt_tokens, first.cached_tokens),
            (Some(6906), None)
        );
        assert_eq!(
            (second.prompt_tokens, second.cached_tokens),
            (Some(6906), Some(4861))
        );
        assert_eq!(
            second.completion_tokens,
            Some(2),
            "total 6908 minus prompt 6906"
        );
        let flash = gem("gemini-3.8-flash");
        let today = "2026-11-26";
        let uncached = direct_models::cost_usd(flash, &first, today).unwrap();
        let cached = direct_models::cost_usd(flash, &second, today).unwrap();
        let want = (2045.0 * 0.75 + 4861.0 * 0.075 + 2.0 * 3.75) / 1_000_000.0;
        assert!((cached - want).abs() < 1e-12, "{cached} vs {want}");
        assert!(cached < uncached, "the cached share is cheaper");
        assert!(
            (uncached - (6906.0 * 0.75 + 2.0 * 3.75) / 1_000_000.0).abs() < 1e-12,
            "no details means every prompt token at the input price: over, never under"
        );
        // A row with no cached price charges the cached share at the input price.
        let mut no_cached_price = flash.clone();
        no_cached_price.price_cached_in = None;
        let flat = direct_models::cost_usd(&no_cached_price, &second, today).unwrap();
        assert!((flat - uncached).abs() < 1e-12, "{flat} vs {uncached}");
    }

    /// The real replies end to end through the key check: a free key's first probe is the verdict, and
    /// a paid key's is the shape Google really sends.
    #[tokio::test]
    async fn the_real_replies_decide_the_tier_through_the_whole_check() {
        let all_four = r#"{"models":[
            {"name":"models/gemini-3.1-pro-preview","supportedGenerationMethods":["generateContent"]},
            {"name":"models/gemini-4-carbon","supportedGenerationMethods":["generateContent"]},
            {"name":"models/gemini-4-argon","supportedGenerationMethods":["generateContent"]},
            {"name":"models/gemini-4-neon","supportedGenerationMethods":["generateContent"]}]}"#;
        let (url, seen) = serve_scripted(vec![
            (200, all_four.into()),
            (429, FREE_PROBE_429_PRO.into()),
        ]);
        let c = check_key_at(KEY_A, &urls(url)).await;
        assert_eq!((c.outcome, c.tier), (KeyOutcome::Ok, Some(KeyTier::Free)));
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 2);
        let (url, _) = serve_scripted(vec![
            (200, all_four.into()),
            (200, PAID_PROBE_200_PRO.into()),
        ]);
        let c = check_key_at(KEY_A, &urls(url)).await;
        assert_eq!((c.outcome, c.tier), (KeyOutcome::Ok, Some(KeyTier::Paid)));
        // A bad key stops at the list, with Google's real reply.
        let (url, seen) = serve_scripted(vec![(400, BAD_KEY_NATIVE_LIST_400.into())]);
        let c = check_key_at(KEY_A, &urls(url)).await;
        assert_eq!(c.outcome, KeyOutcome::Rejected);
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    // ---- scans ---------------------------------------------------------------------------------------------

    fn production() -> &'static str {
        include_str!("gemini_direct.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap()
    }

    /// The text of one top-level fn: from its signature to the closing brace in column 0.
    fn fn_text<'a>(src: &'a str, signature: &str) -> &'a str {
        let start = src.find(signature).unwrap_or_else(|| panic!("{signature}"));
        let rest = &src[start..];
        &rest[..rest.find("\n}\n").expect("the fn ends") + 2]
    }

    #[test]
    fn the_send_seam_is_the_first_thing_gemini_does() {
        for sig in [
            "pub(crate) async fn complete(",
            "pub(crate) async fn stream_chat<",
        ] {
            let body = fn_text(production(), sig);
            let seam_at = body.find("seam(").expect("seam");
            assert!(seam_at < body.find("requested_model(").unwrap(), "{sig}");
            assert!(
                seam_at < body.find("cloud_openai::stream(").unwrap(),
                "{sig}"
            );
            assert!(
                !body[..seam_at].contains(".await"),
                "{sig}: nothing awaited before the seam"
            );
        }
    }

    #[test]
    fn no_db_guard_in_the_gemini_wire_path() {
        let src = production();
        assert!(
            !src.contains(concat!(".con", "n()")),
            "the passive flip is the inbox only"
        );
        assert!(!src.contains(concat!("st", "ate.")));
    }

    #[test]
    fn complete_is_a_drained_stream_with_no_send_of_its_own() {
        let body = fn_text(production(), "pub(crate) async fn complete(");
        assert!(body.contains("cloud_openai::stream(") && !body.contains(".send()"));
    }

    #[test]
    fn every_gemini_check_request_has_a_timeout_and_the_key_is_never_in_a_url() {
        let src = production();
        for f in ["async fn send_probe(", "async fn check_key_at("] {
            let body = fn_text(src, f);
            assert!(body.contains("goog_key("), "{f}");
            // The custom key header is not stripped on a redirect, so the check never follows one.
            assert!(
                body.contains("check_http()") && !body.contains("cloud_openai::http()"),
                "{f}"
            );
            let at = body.find("goog_key(").unwrap();
            assert!(
                body[at..body.find(".send()").unwrap()].contains(".timeout("),
                "{f}"
            );
        }
        assert!(!src.contains(concat!("?ke", "y=")) && !src.contains(concat!("&ke", "y=")));
        assert!(fn_text(src, "fn check_http(").contains("Policy::none()"));
    }

    // ---- review round 1 ------------------------------------------------------------------------

    /// A total with no prompt count can't be priced exactly.
    #[test]
    fn a_total_with_no_prompt_count_is_shown_as_partial() {
        let u = usage_from(&json!({"usage": {"total_tokens": 50}}));
        assert!(u.output_partial);
        let ok = usage_from(&json!({"usage": {"prompt_tokens": 10, "total_tokens": 50}}));
        assert!(!ok.output_partial && ok.completion_tokens == Some(40));
    }

    /// An unrelated FAILED_PRECONDITION is not "no free keys where you are".
    #[test]
    fn only_a_free_tier_precondition_reads_as_no_free_keys_here() {
        let other = r#"{"error":{"code":400,"message":"Billing account is closed.","status":"FAILED_PRECONDITION"}}"#;
        assert!(!matches!(
            classify(&parse_error(other)),
            Failure::NoFreeTierHere
        ));
        let free = r#"{"error":{"code":400,"message":"User location is not supported for the API use without a free tier.","status":"FAILED_PRECONDITION"}}"#;
        assert_eq!(classify(&parse_error(free)), Failure::NoFreeTierHere);
    }

    /// A held plan follows the newest verdict in BOTH directions: a Paid-stamped plan stops when a
    /// check comes back "couldn't tell" in the region, and a Free-stamped plan is no longer priced
    /// at $0.00 once a check says Paid.
    #[test]
    fn a_held_plan_follows_the_latest_verdict_whichever_way_it_moves() {
        let _g = lock();
        reset();
        let fp = direct_providers::key_fp(KEY_A);
        let paid = arm_with(KEY_A, "gemini-3.8-flash", Some(KeyTier::Paid), None);
        assert!(seam(effective_tier(&paid), &uk()).is_ok());
        direct_providers::record_verdict(CloudProvider::Gemini, &fp, KeyTier::Unknown);
        assert_eq!(effective_tier(&paid), Some(KeyTier::Unknown));
        assert!(
            seam(effective_tier(&paid), &uk()).is_err(),
            "unknown is refused in the region"
        );
        assert!(seam(effective_tier(&paid), &nyc()).is_ok());
        let free = arm_with(KEY_A, "gemini-3.8-flash", Some(KeyTier::Free), None);
        direct_providers::record_verdict(CloudProvider::Gemini, &fp, KeyTier::Paid);
        assert_eq!(effective_tier(&free), Some(KeyTier::Paid));
        reset();
    }
}
