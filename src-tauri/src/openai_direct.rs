// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Your own OpenAI key: the one module that knows OpenAI's wire.
//!
//! OpenAI speaks the same chat-completions dialect as OpenRouter ([`crate::cloud_openai`]), so this
//! is a [`Flavour`] plus the parts that are OpenAI's alone: the output-limit field its reasoning
//! models want, the effort level, the error codes it answers with, the price of a request worked out
//! from the tokens it reports, and the key check.
//!
//! What PM sends is deliberately thin: the model, the messages, `store: false`, an output limit, and
//! an effort level when the person chose one. Never `temperature`, `top_p`, `user`,
//! `safety_identifier`, `metadata`, tools or `service_tier` (a project can disallow a tier, so
//! pinning one could itself fail; omitting is the one value always accepted).
//!
//! Everything is named by its OpenRouter-style id (`openai/<wire id>`) outside this file. The wire id
//! lives only here, and [`settle`] turns whatever the server says served the request back into the
//! catalogue id before the caller logs it.
//!
//! This is not zero data retention. OpenAI keeps requests in abuse logs for up to 30 days, and PM
//! can only ASK it not to store them (`store: false`). The Settings card says so wherever the route
//! is chosen.

use serde_json::{json, Value};

use crate::cloud_openai::{self, redact_detail, Flavour};
use crate::direct_models::{self, DirectModel};
use crate::direct_providers::{KeyCheck, KeyOutcome};
use crate::error::{Error, Result};
use crate::llm_gateway::CloudArm;
use crate::openrouter::{ChatMessage, Completion, Usage};

/// The chat-completions URL. A const, so the key can only ever go to this host.
const ENDPOINT: &str = "https://api.openai.com/v1/chat/completions";
/// What the key check asks: the models this key can see.
const MODELS_ENDPOINT: &str = "https://api.openai.com/v1/models";

pub(crate) const FLAVOUR: Flavour = Flavour {
    name: "OpenAI",
    endpoint: ENDPOINT,
    auth: cloud_openai::bearer,
    // OpenAI caches a repeated prefix by itself; the cache mark is dropped.
    cache_control_blocks: false,
    decorate,
    request_error,
    max_tokens_field: "max_completion_tokens",
    patient: true,
    stream_error: Some(stream_error),
    stop_error: Some(stop_error),
};

/// The fields only OpenAI's body carries: the model (by its WIRE id) and `store: false`. The effort
/// is the role's, which a const flavour can't see, so [`chat_body`] adds it after.
fn decorate(body: &mut Value, models: &[String]) {
    body["model"] = json!(models[0]);
    // OpenAI's reference says false is the default and one guide says new accounts store by default;
    // PM asks for no stored output either way.
    body["store"] = json!(false);
}

/// The request body for one call. `limit` is always sent, as `max_completion_tokens` (never
/// `max_tokens`, which reasoning models reject).
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
    if let Some(level) = effort {
        body["reasoning_effort"] = json!(level);
    }
    body
}

/// The catalogue row for the arm's first model, or the refusal that stops a stale setting before
/// any request leaves. A direct arm calls only its first model in this version.
fn requested_model(arm: &CloudArm) -> Result<&'static DirectModel> {
    let id = arm.models.first().map(String::as_str).unwrap_or_default();
    direct_models::find(id).ok_or_else(|| {
        Error::Other(format!(
            "PM no longer lists {id} for OpenAI. Pick another model in Settings > AI & Models."
        ))
    })
}

/// The output limit: the model's maximum, lowered to a caller's bound.
fn output_limit(model: &DirectModel, bound: Option<u32>) -> u32 {
    bound.map_or(model.max_output, |b| b.min(model.max_output))
}

/// The effort that goes on the wire: the arm's, if the model still offers it. Checked again here so
/// a stale setting can never produce a 400.
fn wire_effort<'a>(model: &DirectModel, arm: &'a CloudArm) -> Option<&'a str> {
    arm.effort.as_deref().filter(|e| model.offers(e))
}

/// A one-shot completion. Runs the stream and discards the deltas: a reasoning model with the
/// output limit at its maximum can think for minutes, and a total deadline would kill a request
/// that is being billed. The request is bounded by silence alone (the patient client).
pub(crate) async fn complete(
    arm: &CloudArm,
    messages: &[ChatMessage],
    cache_prefix: bool,
    max_tokens: Option<u32>,
) -> Result<Completion> {
    let model = requested_model(arm)?;
    let body = chat_body(
        model,
        wire_effort(model, arm),
        messages,
        true,
        cache_prefix.then_some(0),
        output_limit(model, max_tokens),
    );
    let completion = cloud_openai::stream(&FLAVOUR, arm.key.expose(), &body, |_| {}).await?;
    Ok(settle(completion, model))
}

/// A streamed chat completion, each text delta to `on_token`.
pub(crate) async fn stream_chat<F: FnMut(&str)>(
    arm: &CloudArm,
    messages: &[ChatMessage],
    cache_through: Option<usize>,
    on_token: F,
) -> Result<Completion> {
    let model = requested_model(arm)?;
    let body = chat_body(
        model,
        wire_effort(model, arm),
        messages,
        true,
        cache_through,
        model.max_output,
    );
    let completion = cloud_openai::stream(&FLAVOUR, arm.key.expose(), &body, on_token).await?;
    Ok(settle(completion, model))
}

/// Turn what the server reported into what PM logs. PURE.
///
/// `Completion.model` is ALWAYS left as an OpenRouter-style catalogue id, never None and never a raw
/// wire id: the catalogue row the served snapshot maps to, else the model that was asked for. About
/// 25 callers log `completion.model.or(Some(plan.primary_model_id()))`, and on a local-then-cloud
/// route that primary is the LOCAL model, so a None would book OpenAI's tokens and a computed price
/// under the local model's name. A direct arm holds one model, so the requested one is the only
/// honest fallback. The cost is worked out here too (OpenAI reports none).
pub(crate) fn settle(mut completion: Completion, requested: &DirectModel) -> Completion {
    let served = completion.model.as_deref().and_then(served_model_id);
    let priced = served.unwrap_or(requested);
    completion.model = Some(priced.id.clone());
    completion.usage.cost = cost_usd(priced, &completion.usage);
    completion
}

/// The catalogue row a served model name stands for: an exact wire id, else the same with a trailing
/// `-YYYY-MM-DD` snapshot date taken off. `None` for a model PM doesn't list.
pub(crate) fn served_model_id(served_wire: &str) -> Option<&'static DirectModel> {
    if let Some(m) = direct_models::find_wire(served_wire) {
        return Some(m);
    }
    // "-" + YYYY-MM-DD is 11 bytes. The name comes off the wire, so cut with `get`: a multi-byte
    // character straddling the offset must be a miss, never a panic inside a billed stream.
    if served_wire.len() > 11 {
        let head = served_wire.get(..served_wire.len() - 11)?;
        let tail = served_wire.get(served_wire.len() - 11..)?;
        let date = tail.strip_prefix('-')?;
        if chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok() {
            return direct_models::find_wire(head);
        }
    }
    None
}

/// What a request cost, from the tokens the provider reported and PM's bundled prices (USD).
///
/// P is the prompt, C the part read from the cache, W the part written to it, O the completion
/// (reasoning included; the reasoning count is never added again). The ordinary input is
/// `max(0, P - C - W)`. Over the model's long-context line the WHOLE request is billed at the
/// higher rates. `None` when no usage came back at all, so the row shows "—" and never a made-up $0.
///
/// The cache arithmetic assumes the prompt count INCLUDES the cached and written parts. That is how
/// the Responses guide shows it and is strongly implied for chat completions, but it is unverified
/// there (live test L4).
pub(crate) fn cost_usd(m: &DirectModel, u: &Usage) -> Option<f64> {
    if u.prompt_tokens.is_none() && u.completion_tokens.is_none() {
        return None;
    }
    let prompt = u.prompt_tokens.unwrap_or(0).max(0);
    let cached = u.cached_tokens.unwrap_or(0).max(0);
    let written = u.cache_write_tokens.unwrap_or(0).max(0);
    let out = u.completion_tokens.unwrap_or(0).max(0);
    let ordinary = (prompt - cached - written).max(0);

    let lc = &m.long_context;
    let long = prompt > lc.threshold;
    let (in_m, cached_m, write_m, out_m) = if long {
        (lc.in_mult, lc.cached_mult, lc.write_mult, lc.out_mult)
    } else {
        (1.0, 1.0, 1.0, 1.0)
    };
    let cached_price = m.price_cached_in.unwrap_or(m.price_in);
    let total = ordinary as f64 * m.price_in * in_m
        + cached as f64 * cached_price * cached_m
        + written as f64 * m.price_cache_write * write_m
        + out as f64 * m.price_out * out_m;
    Some(total / 1_000_000.0)
}

// ---- errors ----------------------------------------------------------------------------------

/// The words of one OpenAI error: `{"error":{"message","type","param","code"}}`. Lowercased where
/// they are compared, kept as sent where they are shown.
#[derive(Debug, Default)]
pub(crate) struct ErrorFields {
    code: String,
    kind: String,
    param: String,
    message: String,
}

/// A JSON field that may be a string or a number (OpenAI's `code` is a string, but a mid-stream
/// error can carry a number).
fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn fields_from_value(err: &Value) -> ErrorFields {
    ErrorFields {
        code: text_of(&err["code"]).to_lowercase(),
        kind: text_of(&err["type"]).to_lowercase(),
        param: text_of(&err["param"]).to_lowercase(),
        message: text_of(&err["message"]),
    }
}

/// The code words PM knows, for a body that isn't JSON (a proxy's error page, a cut body).
const KNOWN_CODES: [&str; 14] = [
    "invalid_api_key",
    "unsupported_country_region_territory",
    "insufficient_quota",
    "credit_balance_exhausted",
    "organization_spend_limit_exceeded",
    "project_spend_limit_exceeded",
    "organization_usage_limit_exceeded",
    "rate_limit_exceeded",
    "slow_down",
    "model_not_found",
    "context_length_exceeded",
    "server_is_overloaded",
    "server_error",
    "max_completion_tokens",
];

/// Read a failed response's RAW body. A JSON body gives its fields; anything else falls back to a
/// scan of the bounded text for the code words, with the text itself as the message.
fn fields_from_body(raw: &str) -> ErrorFields {
    if let Ok(v) = serde_json::from_str::<Value>(raw) {
        if v.get("error").is_some_and(Value::is_object) {
            return fields_from_value(&v["error"]);
        }
    }
    let lower = raw.to_lowercase();
    ErrorFields {
        code: KNOWN_CODES
            .iter()
            .find(|c| lower.contains(**c))
            .map(|c| (*c).to_string())
            .unwrap_or_default(),
        message: raw.to_string(),
        ..ErrorFields::default()
    }
}

/// What went wrong, as PM tells it. Shared by the request-time and the mid-stream paths.
#[derive(Debug, PartialEq)]
enum Failure {
    KeyRejected,
    RegionBlocked,
    NotAllowed,
    OutOfCredit,
    RateLimited { too_large: bool },
    ModelUnavailable { model: String },
    ContextTooLong { model: String },
    EffortRefused { model: String, level: String },
    OutputLimitStale,
    Overloaded,
    Other { status: Option<u16>, detail: String },
}

/// The quoted pieces of a message: everything between a pair of `quote` characters.
fn quoted(message: &str, quote: char) -> Vec<&str> {
    message.split(quote).skip(1).step_by(2).collect()
}

/// The catalogue name of a model a message mentions by its wire id (in backticks or single quotes),
/// else a plain "this model".
fn model_name_in(message: &str) -> String {
    quoted(message, '`')
        .into_iter()
        .chain(quoted(message, '\''))
        .find_map(direct_models::find_wire)
        .map_or_else(|| "this model".to_string(), |m| m.name.clone())
}

/// Decide what failed. PURE: status (when the transport gave one) plus the parsed fields.
fn classify(status: Option<u16>, f: &ErrorFields) -> Failure {
    let lower = f.message.to_lowercase();
    let status_is = |s: u16| status == Some(s);

    if status_is(401) {
        return Failure::KeyRejected;
    }
    if status_is(403)
        && (f.code == "unsupported_country_region_territory"
            || lower.contains("country, region, or territory"))
    {
        return Failure::RegionBlocked;
    }
    // Out of credit is checked BEFORE the rate limit: both are a 429.
    const CREDIT_CODES: [&str; 5] = [
        "insufficient_quota",
        "credit_balance_exhausted",
        "organization_spend_limit_exceeded",
        "project_spend_limit_exceeded",
        "organization_usage_limit_exceeded",
    ];
    if CREDIT_CODES.contains(&f.code.as_str()) || f.kind == "insufficient_quota" {
        return Failure::OutOfCredit;
    }
    if status_is(429) || f.code == "rate_limit_exceeded" || f.code == "slow_down" {
        return Failure::RateLimited {
            // Only "too large": OpenAI's ordinary transient limit message also names "tokens per min".
            too_large: lower.contains("too large"),
        };
    }
    if status_is(404)
        || f.code == "model_not_found"
        || lower.contains("does not exist or you do not have access")
    {
        return Failure::ModelUnavailable {
            model: model_name_in(&f.message),
        };
    }
    if f.code == "context_length_exceeded" || lower.contains("maximum context length") {
        return Failure::ContextTooLong {
            model: model_name_in(&f.message),
        };
    }
    if f.param == "reasoning_effort" || lower.contains("reasoning_effort") {
        // OpenAI quotes the level first, then the model: "'minimal' is not supported with the
        // 'gpt-6.2-sol' model".
        let level = quoted(&f.message, '\'')
            .into_iter()
            .find(|q| direct_models::find_wire(q).is_none())
            .unwrap_or("that level");
        // Quoted straight from the provider's text, so it gets the same redaction as the fallback
        // (D27) and a cap: a "level" is a word, never a sentence.
        let level: String = redact_detail(level).chars().take(40).collect();
        return Failure::EffortRefused {
            model: model_name_in(&f.message),
            level,
        };
    }
    if f.param == "max_completion_tokens" || lower.contains("max_completion_tokens") {
        return Failure::OutputLimitStale;
    }
    if status.is_some_and(|s| s >= 500)
        || f.code == "server_error"
        || f.code == "server_is_overloaded"
        || f.kind == "server_error"
        || f.kind == "service_unavailable_error"
    {
        return Failure::Overloaded;
    }
    if status_is(403) {
        return Failure::NotAllowed;
    }
    Failure::Other {
        status,
        // Redacted BEFORE the cut, so a key can't be cut in half and slip past the run length.
        detail: redact_detail(&f.message).chars().take(200).collect(),
    }
}

fn failure_text(f: &Failure) -> String {
    match f {
        Failure::KeyRejected => "OpenAI rejected your key (mistyped, revoked, or from another organisation). Replace it in Settings > AI & Models.".into(),
        Failure::RegionBlocked => "OpenAI isn't available in your country or region, so this key can't be used here.".into(),
        Failure::NotAllowed => "Your OpenAI key isn't allowed to do this (a restricted or read-only key, or your project blocks it). The key needs permission to use chat completions.".into(),
        Failure::OutOfCredit => "Your OpenAI account is out of credit or at its spend limit. Add credit or raise the limit at platform.openai.com, then try again.".into(),
        Failure::RateLimited { too_large } => {
            let mut s = String::from("OpenAI is rate-limiting this key. Try again in a minute.");
            if *too_large {
                s.push_str(" This request is bigger than your plan's per-minute limit.");
            }
            s
        }
        Failure::ModelUnavailable { model } => format!("Your OpenAI project can't use {model}. Pick another model, or check your project's model access."),
        Failure::ContextTooLong { model } => format!("This conversation is longer than {model}'s limit. Compress it or start a new chat."),
        Failure::EffortRefused { model, level } => format!("{model} doesn't take effort '{level}' (PM's list may be out of date). Set effort to Default in Settings > AI & Models."),
        Failure::OutputLimitStale => "PM's output limit for this model is out of date. Update PM.".into(),
        Failure::Overloaded => "OpenAI is having trouble right now. Try again shortly.".into(),
        Failure::Other { status, detail } => match status {
            Some(s) => format!("OpenAI request failed ({s}): {detail}"),
            None => format!("OpenAI request failed: {detail}"),
        },
    }
}

/// How a failed response reads. Handed the RAW bounded body (see `Flavour::request_error`), so a
/// long `message` ahead of the `code` can't cut the code off and mis-split a 429.
fn request_error(status: reqwest::StatusCode, raw: &str) -> Error {
    Error::Other(failure_text(&classify(
        Some(status.as_u16()),
        &fields_from_body(raw),
    )))
}

/// How a mid-stream `error` chunk reads: the same table, with the status the chunk carries as a
/// number, else worked out from its type or code.
fn stream_error(err: &Value) -> Error {
    let f = fields_from_value(err);
    let derived = match (f.kind.as_str(), f.code.as_str()) {
        ("insufficient_quota", _) | (_, "rate_limit_exceeded") => Some(429),
        ("server_error", _) | (_, "server_error") => Some(500),
        (_, "server_is_overloaded") => Some(503),
        _ => None,
    };
    let status = text_of(&err["code"]).parse::<u16>().ok().or(derived);
    Error::Other(failure_text(&classify(status, &f)))
}

/// A stop reason that is a failure: the provider's safety filter withheld the reply.
fn stop_error(reason: &str) -> Option<Error> {
    (reason == "content_filter")
        .then(|| Error::Other("OpenAI withheld this reply (its safety filter).".into()))
}

// ---- the key check ---------------------------------------------------------------------------

/// Whether a pasted key is the right shape to try: trimmed, 20 to 300 characters, visible ASCII
/// only (so `bearer_auth` can't fail on a header character). No prefix rule: key prefixes aren't in
/// OpenAI's own pages, so PM doesn't guess them.
pub(crate) fn well_formed(key: &str) -> bool {
    let key = key.trim();
    (20..=300).contains(&key.len()) && key.chars().all(|c| c.is_ascii_graphic())
}

/// What the key check's response means. PURE: the status and body in, the verdict out.
///
/// 200 is a key that works. A 401 whose message names a permission or scope is a key that is valid
/// but restricted; any other 401 is a key OpenAI rejects. A 403 for the country is a region block;
/// any other 403 is a restricted key. A 429, a 5xx and a transport failure are "couldn't tell". The
/// restricted-key replies are unverified (live test L1), so they are matched on the message.
pub(crate) fn interpret(status: u16, body: &str) -> KeyOutcome {
    let f = fields_from_body(body);
    let lower = f.message.to_lowercase();
    // "Restricted" KEEPS the typed key, so it must be OpenAI speaking: a JSON body with an `error`
    // object. A captive portal's or proxy's HTML 401/403 is not a verdict on the key, and must not
    // get a mistyped key stored.
    let from_openai = serde_json::from_str::<Value>(body).is_ok_and(|v| v["error"].is_object());
    match status {
        200 => KeyOutcome::Ok,
        401 if f.code == "invalid_api_key" => KeyOutcome::Rejected,
        401 if from_openai && (lower.contains("permission") || lower.contains("scope")) => {
            KeyOutcome::Restricted
        }
        401 => KeyOutcome::Rejected,
        403 if f.code == "unsupported_country_region_territory"
            || lower.contains("country, region, or territory") =>
        {
            KeyOutcome::RegionBlocked
        }
        403 if from_openai => KeyOutcome::Restricted,
        429 | 500..=599 => KeyOutcome::Unreachable,
        other => KeyOutcome::Unexpected(other),
    }
}

/// The catalogue ids the key can see, from a `/v1/models` body, plus how many models it listed.
fn listed_models(body: &str) -> (Vec<String>, usize) {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return (Vec::new(), 0);
    };
    let seen: Vec<&str> = v["data"]
        .as_array()
        .map(|a| a.iter().filter_map(|m| m["id"].as_str()).collect())
        .unwrap_or_default();
    let listed = direct_models::models(crate::llm_gateway::CloudProvider::OpenAi)
        .iter()
        .filter(|m| seen.contains(&m.wire.as_str()))
        .map(|m| m.id.clone())
        .collect();
    (listed, seen.len())
}

/// Ask OpenAI which models the key can see. Not a paid request, so a plain send through the shared
/// client (no attribution, no User-Agent, only the auth header). The key never appears in a URL, a
/// log or an error.
pub(crate) async fn check_key(key: &str) -> KeyCheck {
    let request = cloud_openai::bearer(
        cloud_openai::http()
            .get(MODELS_ENDPOINT)
            .timeout(std::time::Duration::from_secs(30)),
        key,
    );
    let Ok(response) = request.send().await else {
        return KeyCheck::of(KeyOutcome::Unreachable);
    };
    let status = response.status().as_u16();
    // The model list is a few hundred short rows; anything near this cap is not OpenAI.
    const MAX_BODY: usize = 4 * 1024 * 1024;
    let body = match response.text().await {
        Ok(t) if t.len() <= MAX_BODY => t,
        Ok(_) => return KeyCheck::of(KeyOutcome::Unexpected(status)),
        Err(_) => return KeyCheck::of(KeyOutcome::Unreachable),
    };
    let outcome = interpret(status, &body);
    if outcome == KeyOutcome::Ok {
        let (listed, listed_total) = listed_models(&body);
        return KeyCheck {
            outcome,
            listed,
            listed_total,
        };
    }
    KeyCheck::of(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn sol() -> &'static DirectModel {
        direct_models::find("openai/gpt-6.2-sol").unwrap()
    }

    fn luna() -> &'static DirectModel {
        direct_models::find("openai/gpt-6-luna").unwrap()
    }

    // ---- the body --------------------------------------------------------------------------

    #[test]
    fn the_streamed_body_is_exactly_what_pm_means_to_send() {
        let body = chat_body(sol(), None, &msgs(), true, Some(0), 128_000);
        assert_eq!(
            serde_json::to_string(&body).unwrap(),
            r#"{"max_completion_tokens":128000,"messages":[{"content":"s","role":"system"},{"content":"u","role":"user"}],"model":"gpt-6.2-sol","store":false,"stream":true,"stream_options":{"include_usage":true}}"#
        );
    }

    #[test]
    fn the_effort_is_sent_only_when_one_is_set() {
        let none = chat_body(luna(), None, &msgs(), false, None, 10);
        assert!(none.get("reasoning_effort").is_none());
        let some = chat_body(luna(), Some("low"), &msgs(), false, None, 10);
        assert_eq!(some["reasoning_effort"], "low");
        // Not streaming: no usage request either.
        assert!(some.get("stream_options").is_none());
    }

    #[test]
    fn nothing_openrouter_or_unwanted_rides_on_an_openai_body() {
        let body = chat_body(sol(), Some("high"), &msgs(), true, Some(0), 1);
        // The top-level keys, exactly: nothing else rides along, whatever its name.
        let mut keys: Vec<&str> = body
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "max_completion_tokens",
                "messages",
                "model",
                "reasoning_effort",
                "store",
                "stream",
                "stream_options"
            ]
        );
        // And no cache mark inside a message.
        assert!(!body.to_string().contains("cache_control"));
    }

    #[test]
    fn the_output_limit_is_the_models_maximum_lowered_to_a_callers_bound() {
        assert_eq!(output_limit(sol(), None), 128_000);
        assert_eq!(output_limit(sol(), Some(500)), 500);
        assert_eq!(output_limit(sol(), Some(9_999_999)), 128_000);
    }

    #[test]
    fn a_stale_effort_never_reaches_the_wire() {
        let mut arm = CloudArm::new(
            crate::llm_gateway::CloudProvider::OpenAi,
            crate::secret::Secret::from("k".to_string()),
            vec![sol().id.clone()],
        );
        arm.effort = Some("none".into());
        assert_eq!(wire_effort(sol(), &arm), None, "Sol rejects none");
        arm.effort = Some("high".into());
        assert_eq!(wire_effort(sol(), &arm), Some("high"));
    }

    #[test]
    fn a_direct_arm_calls_only_its_first_model() {
        // The body names one model whatever the arm holds: no fallback list, no auto-switch.
        let body = chat_body(sol(), None, &msgs(), false, None, 1);
        assert_eq!(body["model"], "gpt-6.2-sol");
        assert!(body.get("models").is_none());
    }

    #[tokio::test]
    async fn an_openai_arm_naming_a_model_not_in_the_catalogue_is_refused_before_the_network() {
        let arm = CloudArm::new(
            crate::llm_gateway::CloudProvider::OpenAi,
            crate::secret::Secret::from("k".to_string()),
            vec!["x/y".into()],
        );
        let e = complete(&arm, &msgs(), false, None).await.err().unwrap();
        assert!(e.to_string().contains("x/y"), "{e}");
        let e = stream_chat(&arm, &msgs(), None, |_| {})
            .await
            .err()
            .unwrap();
        assert!(e.to_string().contains("x/y"), "{e}");
    }

    // ---- settle and cost -------------------------------------------------------------------

    fn completion(model: Option<&str>, usage: Usage) -> Completion {
        Completion {
            text: "t".into(),
            model: model.map(str::to_string),
            usage,
            truncated: false,
        }
    }

    fn usage(p: i64, c: i64) -> Usage {
        Usage {
            prompt_tokens: Some(p),
            completion_tokens: Some(c),
            ..Usage::default()
        }
    }

    #[test]
    fn a_dated_snapshot_settles_to_its_catalogue_id() {
        let c = settle(
            completion(Some("gpt-6.2-sol-2026-11-01"), usage(1, 1)),
            luna(),
        );
        assert_eq!(c.model.as_deref(), Some("openai/gpt-6.2-sol"));
        // Priced as the model that served it, not the one asked for.
        let want = cost_usd(sol(), &usage(1, 1));
        assert_eq!(c.usage.cost, want);
    }

    #[test]
    fn an_unknown_snapshot_settles_to_the_requested_model_never_none() {
        let c = settle(completion(Some("gpt-9-mystery"), usage(10, 10)), sol());
        assert_eq!(c.model.as_deref(), Some("openai/gpt-6.2-sol"));
        let none = settle(completion(None, usage(10, 10)), sol());
        assert_eq!(none.model.as_deref(), Some("openai/gpt-6.2-sol"));
    }

    /// A local-then-cloud route logs `completion.model.or(Some(primary))` and its primary is the
    /// LOCAL model. A settled direct completion carries its own id, so the OpenAI tokens and the
    /// computed price can never be booked under the local model's name.
    #[test]
    fn a_direct_completion_on_a_local_then_cloud_route_logs_the_openai_id() {
        use crate::llm_gateway::{CloudArm, CloudProvider, LocalArm, RoutePlan};
        let plan = RoutePlan::LocalThenCloud {
            local: LocalArm {
                base_url: "http://127.0.0.1:1".into(),
                model: "qwen-local".into(),
                token: None,
            },
            cloud: CloudArm::new(
                CloudProvider::OpenAi,
                crate::secret::Secret::from("k".to_string()),
                vec![sol().id.clone()],
            ),
        };
        let c = settle(completion(Some("something-new"), usage(1000, 100)), sol());
        let logged = c
            .model
            .clone()
            .or(Some(plan.primary_model_id().to_string()));
        assert_eq!(logged.as_deref(), Some("openai/gpt-6.2-sol"));
        assert!(c.usage.cost.is_some_and(|x| x > 0.0));
    }

    #[test]
    fn cost_is_the_reported_tokens_times_the_bundled_prices() {
        // Sol: $2 in, $10 out per million. 200k in + 100k out = $0.40 + $1.00 (under the long line).
        let c = cost_usd(sol(), &usage(200_000, 100_000)).unwrap();
        assert!((c - 1.4).abs() < 1e-9, "{c}");
        assert_eq!(
            cost_usd(sol(), &Usage::default()),
            None,
            "no usage, no price"
        );
    }

    #[test]
    fn cached_and_written_tokens_price_at_their_own_rates_and_reasoning_is_not_added() {
        // 100k prompt: 40k read from the cache ($0.10/M), 10k written ($2.50/M), 50k ordinary ($2/M),
        // plus 20k out ($10/M).
        let u = Usage {
            prompt_tokens: Some(100_000),
            completion_tokens: Some(20_000),
            cached_tokens: Some(40_000),
            cache_write_tokens: Some(10_000),
            ..Usage::default()
        };
        let want = (50_000.0 * 2.0 + 40_000.0 * 0.1 + 10_000.0 * 2.5 + 20_000.0 * 10.0) / 1e6;
        let got = cost_usd(sol(), &u).unwrap();
        assert!((got - want).abs() < 1e-9, "{got} vs {want}");
    }

    #[test]
    fn a_request_over_the_long_context_line_is_billed_at_the_higher_rates_throughout() {
        let u = usage(300_000, 10_000);
        // in x2, out x1.5 for the WHOLE request.
        let want = (300_000.0 * 2.0 * 2.0 + 10_000.0 * 10.0 * 1.5) / 1e6;
        let got = cost_usd(sol(), &u).unwrap();
        assert!((got - want).abs() < 1e-9, "{got} vs {want}");
        // At the line exactly it is still the short rate.
        let at = cost_usd(sol(), &usage(272_000, 0)).unwrap();
        assert!((at - 272_000.0 * 2.0 / 1e6).abs() < 1e-9);
    }

    #[test]
    fn a_cached_count_above_the_prompt_cannot_make_a_negative_price() {
        let u = Usage {
            prompt_tokens: Some(10),
            completion_tokens: Some(0),
            cached_tokens: Some(50),
            ..Usage::default()
        };
        assert!(cost_usd(sol(), &u).unwrap() >= 0.0);
    }

    #[test]
    fn served_model_id_handles_exact_dated_and_unknown() {
        assert!(served_model_id("gpt-6-luna").is_some());
        assert_eq!(
            served_model_id("gpt-6-luna-2026-05-18").map(|m| m.id.as_str()),
            Some("openai/gpt-6-luna")
        );
        assert!(served_model_id("gpt-6-luna-preview").is_none());
        assert!(served_model_id("nope-2026-05-18").is_none());
        assert!(served_model_id("").is_none());
    }

    #[test]
    fn a_served_name_with_a_multibyte_tail_is_a_miss_not_a_panic() {
        // 11 bytes from the end lands inside the three-byte character.
        assert!(served_model_id("gpt-6-luna-ééééé-x").is_none());
        assert!(served_model_id("日本語日本語日本語日本語").is_none());
        assert!(served_model_id("é-2026-05-18é").is_none());
    }

    // ---- errors ----------------------------------------------------------------------------

    fn err_json(code: &str, kind: &str, message: &str) -> String {
        json!({"error": {"message": message, "type": kind, "param": null, "code": code}})
            .to_string()
    }

    fn text(status: u16, body: &str) -> String {
        request_error(reqwest::StatusCode::from_u16(status).unwrap(), body).to_string()
    }

    #[test]
    fn a_401_is_a_rejected_key_whatever_it_says() {
        for body in [
            err_json(
                "invalid_api_key",
                "invalid_request_error",
                "Incorrect API key",
            ),
            "<html>nope</html>".to_string(),
            String::new(),
        ] {
            assert!(
                text(401, &body).starts_with("OpenAI rejected your key"),
                "{body}"
            );
        }
    }

    #[test]
    fn the_403s_split_on_the_country_code() {
        let region = err_json(
            "unsupported_country_region_territory",
            "request_forbidden",
            "Country, region, or territory not supported",
        );
        assert!(text(403, &region).contains("isn't available in your country"));
        assert!(text(403, "{}").contains("isn't allowed to do this"));
    }

    #[test]
    fn a_429_for_credit_is_told_from_a_rate_limit_by_its_code() {
        for code in [
            "insufficient_quota",
            "credit_balance_exhausted",
            "organization_spend_limit_exceeded",
            "project_spend_limit_exceeded",
            "organization_usage_limit_exceeded",
        ] {
            assert!(
                text(429, &err_json(code, "x", "m")).contains("out of credit"),
                "{code}"
            );
        }
        assert!(text(429, &err_json("", "insufficient_quota", "m")).contains("out of credit"));
        let rate = text(429, &err_json("rate_limit_exceeded", "requests", "slow"));
        assert!(rate.contains("rate-limiting"), "{rate}");
        assert!(!rate.contains("bigger than"));
        // OpenAI's ordinary TPM message names the limit but is not "too large".
        let tpm = text(
            429,
            &err_json(
                "rate_limit_exceeded",
                "tokens",
                "Rate limit reached on tokens per min (TPM): Limit 30000, Used 29000, Requested 2000",
            ),
        );
        assert!(!tpm.contains("bigger than"), "{tpm}");
        let big = text(
            429,
            &err_json("rate_limit_exceeded", "tokens", "Request too large"),
        );
        assert!(
            big.contains("bigger than your plan's per-minute limit"),
            "{big}"
        );
    }

    /// The regression this whole raw-body hand-off exists for: OpenAI's JSON puts `message` before
    /// `code`, and a 500-character cut lost the code, so a credit error read as a rate limit.
    #[test]
    fn a_long_message_ahead_of_the_code_still_classifies_on_the_code() {
        // Built by hand, message FIRST as OpenAI sends it (a `json!` map would sort `code` ahead).
        let long = "x ".repeat(450);
        let body = format!(
            r#"{{"error":{{"message":"{long}","type":"insufficient_quota","param":null,"code":"insufficient_quota"}}}}"#
        );
        assert!(body.len() > 900);
        assert!(text(429, &body).contains("out of credit"));
        // What the old path did to it: cut to 500 characters first. The code is gone.
        let cut: String = body.chars().take(500).collect();
        assert!(!text(429, &cut).contains("out of credit"));
    }

    #[test]
    fn model_context_effort_and_limit_failures_each_have_their_own_sentence() {
        let m = text(
            404,
            &err_json(
                "model_not_found",
                "invalid_request_error",
                "The model `gpt-6.2-sol` does not exist or you do not have access to it.",
            ),
        );
        assert!(m.contains("can't use GPT-6.2 Sol"), "{m}");

        let c = text(
            400,
            &err_json(
                "context_length_exceeded",
                "invalid_request_error",
                "too long",
            ),
        );
        assert!(c.starts_with("This conversation is longer than"), "{c}");

        let body = json!({"error": {"message": "Unsupported value: 'minimal' is not supported with the 'gpt-6-luna' model.", "type": "invalid_request_error", "param": "reasoning_effort", "code": "unsupported_value"}}).to_string();
        let e = text(400, &body);
        assert!(
            e.contains("GPT-6 Luna doesn't take effort 'minimal'"),
            "{e}"
        );

        let l = text(
            400,
            &json!({"error": {"message": "x", "type": "invalid_request_error", "param": "max_completion_tokens", "code": "invalid_value"}}).to_string(),
        );
        assert!(
            l.contains("output limit for this model is out of date"),
            "{l}"
        );
    }

    #[test]
    fn a_server_error_is_told_as_trouble_at_openai() {
        assert!(text(500, "").contains("having trouble"));
        assert!(text(
            503,
            &err_json("server_is_overloaded", "service_unavailable_error", "busy")
        )
        .contains("having trouble"));
    }

    #[test]
    fn the_fallback_names_the_status_and_never_a_key() {
        let masked = "Incorrect API key provided: sk-inval*******-key. See the docs.";
        let e = text(418, &err_json("", "x", masked));
        assert!(e.starts_with("OpenAI request failed (418): "), "{e}");
        assert!(!e.contains("sk-inval"), "{e}");
        assert!(e.contains("[key]"), "{e}");
        let proj = text(418, &err_json("", "x", "bad sk-proj-abcDEF123456 here"));
        assert!(!proj.contains("abcDEF"), "{proj}");
        let run = text(
            418,
            &err_json("", "x", &format!("token {} end", "A1b2".repeat(10))),
        );
        assert!(run.contains("[key]") && !run.contains("A1b2A1b2"), "{run}");
        // A long message is cut for display.
        let long = text(418, &err_json("", "x", &"y ".repeat(300)));
        assert!(long.chars().count() < 260, "{}", long.chars().count());
    }

    #[test]
    fn a_non_json_body_falls_back_to_the_code_words() {
        assert!(text(429, "upstream said: insufficient_quota").contains("out of credit"));
        assert!(text(404, "model_not_found").contains("can't use"));
    }

    #[test]
    fn a_mid_stream_error_reads_through_the_same_table() {
        let e = stream_error(
            &json!({"message": "m", "type": "insufficient_quota", "code": "insufficient_quota"}),
        );
        assert!(e.to_string().contains("out of credit"), "{e}");
        let e = stream_error(&json!({"message": "m", "type": "server_error", "code": null}));
        assert!(e.to_string().contains("having trouble"), "{e}");
        let e = stream_error(&json!({"message": "slow", "code": 429}));
        assert!(e.to_string().contains("rate-limiting"), "{e}");
    }

    #[test]
    fn only_a_content_filter_stop_is_an_error() {
        assert!(stop_error("content_filter").is_some());
        for ok in ["stop", "length", "tool_calls", ""] {
            assert!(stop_error(ok).is_none(), "{ok}");
        }
    }

    // ---- the key check ---------------------------------------------------------------------

    #[test]
    fn a_key_must_be_the_right_shape_to_be_tried() {
        assert!(well_formed("sk-proj-aaaaaaaaaaaaaaaaaaaaaaaa"));
        assert!(
            well_formed("  sk-proj-aaaaaaaaaaaaaaaaaaaaaaaa \n"),
            "trimmed first"
        );
        assert!(!well_formed("short"));
        assert!(!well_formed(&"a".repeat(301)));
        assert!(
            !well_formed("sk-proj-aaaa aaaaaaaaaaaaaaaaaaaaaa"),
            "inner space"
        );
        assert!(
            !well_formed("sk-proj-aaaaaaaaaaaaaaaaaaaaaaaaé"),
            "non-ASCII"
        );
        assert!(!well_formed(""));
    }

    #[test]
    fn the_key_check_reads_each_canned_reply() {
        let rejected = err_json(
            "invalid_api_key",
            "invalid_request_error",
            "Incorrect API key",
        );
        let perm = err_json(
            "",
            "invalid_request_error",
            "You lack the required permission",
        );
        let region = err_json(
            "unsupported_country_region_territory",
            "x",
            "Country, region, or territory not supported",
        );
        assert_eq!(interpret(200, "{\"data\":[]}"), KeyOutcome::Ok);
        assert_eq!(interpret(401, &rejected), KeyOutcome::Rejected);
        assert_eq!(interpret(401, &perm), KeyOutcome::Restricted);
        assert_eq!(interpret(401, ""), KeyOutcome::Rejected);
        assert_eq!(interpret(403, &region), KeyOutcome::RegionBlocked);
        assert_eq!(
            interpret(403, &err_json("", "invalid_request_error", "Forbidden")),
            KeyOutcome::Restricted
        );
        // Not OpenAI's JSON: a portal or proxy page is no verdict, and never keeps the key.
        assert_eq!(interpret(403, "{}"), KeyOutcome::Unexpected(403));
        assert_eq!(
            interpret(403, "<html>Forbidden</html>"),
            KeyOutcome::Unexpected(403)
        );
        assert_ne!(
            interpret(401, "<html>permission denied</html>"),
            KeyOutcome::Restricted
        );
        assert_eq!(interpret(429, ""), KeyOutcome::Unreachable);
        assert_eq!(interpret(503, ""), KeyOutcome::Unreachable);
        assert_eq!(interpret(418, ""), KeyOutcome::Unexpected(418));
    }

    #[test]
    fn the_check_lists_only_the_catalogue_models_the_key_can_see() {
        let body = json!({"object": "list", "data": [
            {"id": "gpt-6-luna"}, {"id": "gpt-6.2-sol"}, {"id": "gpt-4o"}, {"id": "whisper-1"}
        ]})
        .to_string();
        let (listed, total) = listed_models(&body);
        assert_eq!(listed, vec!["openai/gpt-6.2-sol", "openai/gpt-6-luna"]);
        assert_eq!(total, 4, "models PM can't price are counted, not offered");
        assert_eq!(listed_models("not json"), (Vec::new(), 0));
    }

    // ---- the real stream parser, against a canned server on loopback -------------------------

    /// Serve ONE canned event stream on a loopback port and return its URL. Plain std sockets on a
    /// thread: no outside network, no extra tokio features.
    fn serve(events: &'static str) -> &'static str {
        serve_raw(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n"
                .to_string(),
            events.to_string(),
        )
    }

    /// Serve ONE canned response, head and body as given.
    fn serve_raw(head: String, body: String) -> &'static str {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            // The request: read until its headers are in, then give the body a moment to follow.
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
            let _ = sock.write_all(head.as_bytes());
            let _ = sock.write_all(body.as_bytes());
            let _ = sock.flush();
        });
        Box::leak(format!("http://127.0.0.1:{port}/v1/chat/completions").into_boxed_str())
    }

    /// OpenAI's own flavour, pointed at a canned server: the same stop and error handling, a
    /// different URL.
    fn flavour_at(endpoint: &'static str) -> Flavour {
        Flavour {
            endpoint,
            ..Flavour {
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
            }
        }
    }

    async fn run(events: &'static str) -> Result<Completion> {
        let f = flavour_at(serve(events));
        let body = chat_body(sol(), None, &msgs(), true, None, 100);
        cloud_openai::stream(&f, "test-key-not-real", &body, |_| {}).await
    }

    /// The regression the raw-body hand-off exists for, through the REAL request path: OpenAI's JSON
    /// puts `message` before `code`, and the core used to cut the body to 500 characters before the
    /// flavour saw it, so a long message lost its `code` and a credit error read as a rate limit.
    #[tokio::test]
    async fn a_long_error_body_still_classifies_on_its_code_through_the_real_request() {
        let long = "x ".repeat(450);
        let body = format!(
            r#"{{"error":{{"message":"{long}","type":"insufficient_quota","param":null,"code":"insufficient_quota"}}}}"#
        );
        let head = format!(
            "HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json\r\nconnection: close\r\ncontent-length: {}\r\n\r\n",
            body.len()
        );
        let f = flavour_at(serve_raw(head, body));
        let req = chat_body(sol(), None, &msgs(), true, None, 100);
        let e = cloud_openai::stream(&f, "test-key-not-real", &req, |_| {})
            .await
            .err()
            .expect("a 429");
        assert!(e.to_string().contains("out of credit"), "{e}");
    }

    /// A reply the provider's safety filter cut off is an error, and the partial text is dropped:
    /// the text that arrived before the cut must not be kept as an answer.
    #[tokio::test]
    async fn a_content_filter_stop_returns_the_error_and_drops_the_partial_text() {
        let events = concat!(
            "data: {\"model\":\"gpt-6.2-sol\",\"choices\":[{\"delta\":{\"content\":\"The first par\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"content_filter\"}]}\n\n",
            "data: [DONE]\n\n"
        );
        let e = run(events)
            .await
            .err()
            .expect("an error, not a partial reply");
        assert!(e.to_string().contains("safety filter"), "{e}");
        assert!(!e.to_string().contains("first par"), "{e}");
    }

    /// A reasoning model that spends the whole output budget thinking sends no content and stops on
    /// `length`: the completion is empty AND truncated, which the chat path turns into "The model
    /// stopped before it wrote an answer" (`empty_reply_error`), never a blank saved turn.
    #[tokio::test]
    async fn length_with_no_content_is_an_empty_truncated_completion() {
        let events = concat!(
            "data: {\"model\":\"gpt-6.2-sol-2026-11-01\",\"choices\":[{\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":100,\"prompt_tokens_details\":{\"cached_tokens\":4}}}\n\n",
            "data: [DONE]\n\n"
        );
        let c = run(events).await.expect("the call itself succeeded");
        assert!(c.text.is_empty() && c.truncated);
        assert_eq!(c.usage.cached_tokens, Some(4), "the cache read is parsed");
        let settled = settle(c, sol());
        assert_eq!(
            settled.model.as_deref(),
            Some("openai/gpt-6.2-sol"),
            "the dated snapshot maps"
        );
        assert!(settled.usage.cost.is_some_and(|x| x > 0.0));
    }

    #[tokio::test]
    async fn a_mid_stream_error_chunk_reads_through_the_openai_table_and_drops_the_text() {
        let events = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"abc\"},\"finish_reason\":null}]}\n\n",
            "data: {\"error\":{\"message\":\"x\",\"type\":\"insufficient_quota\",\"code\":\"insufficient_quota\"}}\n\n"
        );
        let e = run(events).await.err().expect("an error");
        assert!(e.to_string().contains("out of credit"), "{e}");
    }
}
