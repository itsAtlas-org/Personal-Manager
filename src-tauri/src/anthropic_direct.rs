// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Your own Anthropic key: the one module that knows Anthropic's wire.
//!
//! This is NOT a [`crate::cloud_openai::Flavour`]. Anthropic's body (a top-level `system` array, a
//! mandatory `max_tokens`, `output_config.effort`, content blocks that carry `cache_control`), its
//! stream (typed events, cumulative usage, thinking blocks, an `error` event after a 200, no
//! `[DONE]`) and its errors share nothing with the OpenAI dialect, and bending a flavour around them
//! would put OpenRouter's byte-for-byte body tests at risk. So this module exports the same two
//! functions the gateway already calls for OpenAI ([`complete`], [`stream_chat`]) and reuses only
//! helpers: the line splitter, the two stream size caps, the patient client, `redact_detail`, the
//! bounded error body and the wire mark.
//!
//! What PM sends is the "safe core" and nothing else: `model`, `max_tokens`, `stream`, `system`,
//! `messages`, `output_config` (only `{effort}`), and `thinking` (only `{"type":"disabled"}`, only for
//! the cheap background job on a model whose row says it can). Anything else is either a 400 on the
//! current API version (sampling fields, a trailing assistant message, an empty text block) or a
//! decision that is not this PR's (tools, server-side fallbacks, a second cache breakpoint).
//!
//! Three things worth knowing before editing:
//!   * Thinking tokens count toward `max_tokens` and are billed as output, so a bounded caller on a
//!     direct arm could end with empty text; that already reaches `empty_reply_error`. Today only
//!     mods bound a call, and mods never reach this arm (`serves_a_mod`).
//!   * A NEW paid arm must announce its requests on the wire mark, as every send here does.
//!   * No request carries an `Origin` header: an organisation with zero data retention answers a
//!     request that has one with a 401. reqwest sends none, and a scan keeps it so.
//!
//! This is not zero data retention. Anthropic can keep what is sent for up to 30 days (Claude Fable
//! always), and up to two years if it flags a request; it does not train on it without permission.
//! The Settings card says so wherever the route is chosen.

use std::collections::HashSet;
use std::future::Future;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::cloud_openai::{bounded_error_body, redact_detail, MAX_REPLY_BYTES, MAX_SSE_LINE_BYTES};
use crate::direct_models::{self, names_mythos, DirectModel};
use crate::direct_providers::{self, KeyCheck, KeyNote, KeyOutcome};
use crate::error::{Error, FailureClass, Result};
use crate::llm_gateway::{CloudArm, CloudProvider, Role};
use crate::openrouter::{drain_lines, ChatMessage, Completion, CostNote, Usage};
use crate::usage_keys;

/// The messages URL. A const, so the key can only ever go to this host.
const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
/// What the key check asks first: the models this key can see (one page covers the catalogue).
const MODELS_ENDPOINT: &str = "https://api.anthropic.com/v1/models?limit=1000";
/// The API version, pinned. Anthropic retiring it would break every call at once (the provider
/// canary of PR 4 is where that shows).
const VERSION: &str = "2026-10-21";

/// The key and the version, nothing else of PM's. `x-api-key` is what was VERIFIED on a live key for
/// both the listing and the messages call (2026-11-26); Anthropic's pages call `Authorization:
/// Bearer` current and this one "legacy, still supported", and a verified header beats a documented
/// preference. Never sent: `Authorization`, `User-Agent`, any attribution header, a beta header,
/// a workspace header (an ordinary Console key is scoped to one workspace).
fn auth(request: reqwest::RequestBuilder, key: &str) -> reqwest::RequestBuilder {
    request
        .header("x-api-key", key)
        .header("anthropic-version", VERSION)
}

/// The clients every Anthropic request goes through. They follow NO redirect: reqwest strips only
/// `Authorization`, `Cookie`, `WWW-Authenticate` and `Proxy-Authorization` when a redirect leaves the
/// host, never a custom header, so a 3xx from the endpoint would carry `x-api-key` to wherever it
/// pointed (the same hole 2b closed for Google's header). A 3xx is then an ordinary non-success
/// answer: mapped to a sentence, its body redacted. The chat's has 30 s to connect and 15 minutes of
/// silence allowed (as `cloud_openai::patient_http`); the key check's requests carry their own 30 s
/// total `.timeout` each.
fn no_redirect_client(read_timeout: Option<Duration>) -> reqwest::Client {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none());
    if let Some(silence) = read_timeout {
        builder = builder.read_timeout(silence);
    }
    builder
        .build()
        .expect("reqwest client with static settings should build")
}

fn patient_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::LazyLock<reqwest::Client> =
        std::sync::LazyLock::new(|| no_redirect_client(Some(Duration::from_secs(900))));
    &CLIENT
}

fn check_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::LazyLock<reqwest::Client> =
        std::sync::LazyLock::new(|| no_redirect_client(None));
    &CLIENT
}

// ---- what is asked for ------------------------------------------------------------------------

/// The catalogue row for the arm's first model, or the refusal that stops a stale setting before any
/// request leaves. A direct arm calls only its first model in this version. MYTHOS is refused here
/// by name, the second of three layers (the catalogue has no such row, and `models_for` drops a
/// stored id the catalogue doesn't know): even if a later edit adds a row, nothing is sent.
fn requested_model(arm: &CloudArm) -> Result<&'static DirectModel> {
    let id = arm.models.first().map(String::as_str).unwrap_or_default();
    if names_mythos(id) {
        return Err(never_mythos());
    }
    let m = direct_models::find(id)
        .filter(|m| m.id.starts_with("anthropic/"))
        .ok_or_else(|| {
            Error::Other(format!(
                "PM no longer lists {id} for Anthropic. Pick another model in Settings > AI & Models."
            ))
        })?;
    if names_mythos(&m.wire) || names_mythos(&m.name) {
        return Err(never_mythos());
    }
    Ok(m)
}

fn never_mythos() -> Error {
    Error::Other(
        "PM never uses Anthropic's Mythos models. Pick another model in Settings > AI & Models."
            .into(),
    )
}

/// The output limit: always sent (the API requires it). The model's maximum, lowered to a caller's
/// bound, and never below 1.
fn output_limit(m: &DirectModel, bound: Option<u32>) -> u32 {
    bound.map_or(m.max_output, |b| b.min(m.max_output)).max(1)
}

/// The effort that goes on the wire: the arm's, if the model still offers it. Checked again here so
/// a stale setting can never produce a 400.
fn wire_effort<'a>(m: &DirectModel, arm: &'a CloudArm) -> Option<&'a str> {
    arm.effort.as_deref().filter(|e| m.offers(e))
}

/// Whether to switch thinking off, and how. PURE. Thinking is on by default and adaptive on all four
/// models, so PM sends NOTHING except for one case: the cheap background job, on a row whose
/// `thinking_off` says it can, at the EFFECTIVE effort `low` (Bobby's scope was "Haiku may disable at
/// low"; a person who raises Haiku's background effort gets the reasoning the Select promises, and
/// Anthropic itself answers a 400 above `high`). `effort` is what [`wire_effort`] returned: with no
/// stored level the model's own default applies, which is not `low`, so thinking stays on. Chat never
/// disables (answer quality, and the chat cache lane stays constant).
fn thinking_setting(
    m: &DirectModel,
    role: Option<Role>,
    effort: Option<&str>,
) -> Option<&'static str> {
    (role == Some(Role::Background)
        && m.thinking_off.as_deref() == Some("disabled")
        && effort == Some("low"))
    .then_some("disabled")
}

// ---- the messages ------------------------------------------------------------------------------

/// The `system` blocks and the `messages` of one request.
struct Wire {
    system: Vec<Value>,
    messages: Vec<Value>,
}

/// What PM writes in front of a list that starts with an assistant turn. The chat window can cut
/// mid-pair when there is no context message. A constant, PM-written, never deleting what was said.
/// (The live probe showed Anthropic accepts an assistant-first list; the stub stays: rare, harmless.)
const LEADING_STUB: &str = "(Earlier messages in this conversation are not shown.)";

/// One message's text, and whether the cache breakpoint sits on it.
struct Piece {
    text: String,
    mark: bool,
}

/// The cache breakpoint: the ONE the existing seam asks for, five minutes, never a second one.
fn ephemeral() -> Value {
    json!({"type": "ephemeral"})
}

/// PM's messages in Anthropic's shape. PURE.
///   1. Every `system` message, in order, becomes a block of the top-level `system` ARRAY (omitted
///      altogether when there are none: an empty array may 400).
///   2. A message whose content is blank is dropped: an empty text block is a 400, and an empty
///      assistant turn left by a failed reply must not break the next chat.
///   3. `user` and `assistant` map across; any other role is a bug in PM, so it is an `Err`.
///   4. Consecutive same-role messages are MERGED into one message of several text blocks, always.
///      The API says it combines them, but the live probe never captured that case, and chat sends
///      `user(context)` then `user(turn)` on every grounded turn, so PM never depends on it.
///   5. A list that starts with an assistant turn gets [`LEADING_STUB`] in front.
///   6. The last message must be `user`: a trailing assistant turn is prefill, a 400 on every model
///      PM offers. A loud failure here beats a mystery 400 in a future caller.
///
/// `cache_through = Some(i)` puts the one breakpoint on the `system` block that came from message `i`;
/// if that message was dropped as blank, on the nearest earlier surviving system block, else nowhere;
/// if it is not a system message, on its last text block.
fn to_wire(messages: &[ChatMessage], cache_through: Option<usize>) -> Result<Wire> {
    let mut system: Vec<Value> = Vec::new();
    // (message index it came from, block index in `system`) for every surviving system block.
    let mut system_at: Vec<(usize, usize)> = Vec::new();
    let mut turns: Vec<(&'static str, Piece, usize)> = Vec::new();
    for (i, m) in messages.iter().enumerate() {
        let blank = m.content.trim().is_empty();
        match m.role.as_str() {
            "system" if !blank => {
                system_at.push((i, system.len()));
                system.push(json!({"type": "text", "text": m.content}));
            }
            "user" | "assistant" if !blank => {
                let role = if m.role == "user" {
                    "user"
                } else {
                    "assistant"
                };
                turns.push((
                    role,
                    Piece {
                        text: m.content.clone(),
                        mark: false,
                    },
                    i,
                ));
            }
            "system" | "user" | "assistant" => {}
            _ => {
                return Err(Error::Other(
                    "PM built a message Anthropic can't take, so it didn't send this.".into(),
                ))
            }
        }
    }

    if let Some(i) = cache_through {
        if let Some(t) = turns.iter_mut().find(|(_, _, at)| *at == i) {
            // A surviving non-system message: its last text block carries the mark.
            t.1.mark = true;
        } else if let Some(&(_, block)) = system_at
            .iter()
            .find(|(at, _)| *at == i)
            .or_else(|| system_at.iter().rev().find(|(at, _)| *at < i))
        {
            system[block]["cache_control"] = ephemeral();
        }
    }

    if turns
        .first()
        .is_some_and(|(role, _, _)| *role == "assistant")
    {
        turns.insert(
            0,
            (
                "user",
                Piece {
                    text: LEADING_STUB.into(),
                    mark: false,
                },
                usize::MAX,
            ),
        );
    }
    if turns.last().is_none_or(|(role, _, _)| *role != "user") {
        return Err(Error::Other(
            "Anthropic can't continue a reply, so PM didn't send this.".into(),
        ));
    }

    // Merge neighbours of one role into one message.
    let mut grouped: Vec<(&'static str, Vec<Piece>)> = Vec::new();
    for (role, piece, _) in turns {
        match grouped.last_mut() {
            Some((last, pieces)) if *last == role => pieces.push(piece),
            _ => grouped.push((role, vec![piece])),
        }
    }
    let messages = grouped
        .into_iter()
        .map(|(role, pieces)| {
            // A lone unmarked piece stays a plain string; anything else is a list of text blocks.
            let content = if pieces.len() == 1 && !pieces[0].mark {
                json!(pieces[0].text)
            } else {
                Value::Array(
                    pieces
                        .into_iter()
                        .map(|p| {
                            let mut block = json!({"type": "text", "text": p.text});
                            if p.mark {
                                block["cache_control"] = ephemeral();
                            }
                            block
                        })
                        .collect(),
                )
            };
            json!({"role": role, "content": content})
        })
        .collect();
    Ok(Wire { system, messages })
}

/// The request body. PURE: the allow-list of keys is the contract, and a matrix test walks every
/// catalogue row, role and level against it. `stream` is a key only when true (the key check's probe
/// leaves it out and takes one small JSON).
fn chat_body(
    m: &DirectModel,
    effort: Option<&str>,
    thinking: Option<&str>,
    wire: Wire,
    stream: bool,
    limit: u32,
) -> Value {
    let mut body = json!({
        "model": m.wire,
        "max_tokens": limit,
        "messages": wire.messages,
    });
    if stream {
        body["stream"] = json!(true);
    }
    if !wire.system.is_empty() {
        body["system"] = Value::Array(wire.system);
    }
    if let Some(level) = effort.filter(|l| m.offers(l)) {
        body["output_config"] = json!({"effort": level});
    }
    if let Some(kind) = thinking {
        body["thinking"] = json!({"type": kind});
    }
    body
}

/// Everything a call needs once its arguments are checked, all before any network. No key is in it.
#[derive(Debug)]
struct Prepared {
    model: &'static DirectModel,
    effort: Option<String>,
    body: Value,
}

fn prepare(
    arm: &CloudArm,
    messages: &[ChatMessage],
    cache_through: Option<usize>,
    bound: Option<u32>,
) -> Result<Prepared> {
    let model = requested_model(arm)?;
    let wire = to_wire(messages, cache_through)?;
    let effort = wire_effort(model, arm);
    let thinking = thinking_setting(model, arm.role, effort);
    let body = chat_body(
        model,
        effort,
        thinking,
        wire,
        true,
        output_limit(model, bound),
    );
    Ok(Prepared {
        model,
        effort: effort.map(str::to_string),
        body,
    })
}

// ---- the calls ---------------------------------------------------------------------------------

/// A one-shot completion. Runs the stream and discards the deltas: 128K of output needs streaming
/// (a long idle connection is dropped), and one code path is thinner than two. The request is
/// bounded by silence alone (the patient client).
pub(crate) async fn complete(
    arm: &CloudArm,
    messages: &[ChatMessage],
    cache_prefix: bool,
    max_tokens: Option<u32>,
) -> Result<Completion> {
    let prepared = prepare(arm, messages, cache_prefix.then_some(0), max_tokens)?;
    let completion = run(
        ENDPOINT,
        arm.key.expose(),
        &prepared,
        |_| {},
        tokio::time::sleep,
    )
    .await?;
    Ok(settle(completion, prepared.model))
}

/// A streamed chat, each text delta to `on_token`.
pub(crate) async fn stream_chat<F: FnMut(&str)>(
    arm: &CloudArm,
    messages: &[ChatMessage],
    cache_through: Option<usize>,
    on_token: F,
) -> Result<Completion> {
    let prepared = prepare(arm, messages, cache_through, None)?;
    let completion = run(
        ENDPOINT,
        arm.key.expose(),
        &prepared,
        on_token,
        tokio::time::sleep,
    )
    .await?;
    Ok(settle(completion, prepared.model))
}

/// What a reply cost, named by its catalogue id: always `anthropic/<id>`, never None and never a raw
/// wire id, so a local-then-cloud plan never books Anthropic's tokens under the local model.
pub(crate) fn settle(completion: Completion, requested: &DirectModel) -> Completion {
    let mut settled = direct_models::settle_on(completion, requested, &direct_models::utc_today());
    // An exact-looking cost needs BOTH sides: a stream that reported an output count but no input
    // fields (or the reverse) would price the missing side as 0, a confident under-count. "No cost"
    // shows as a dash, and a dash is honest.
    // The same for a reply that wrote one-hour cache entries (see `Turn::finish`): `output_partial`
    // is how the usage says "the count can't be priced exactly".
    if settled.usage.prompt_tokens.is_none()
        || settled.usage.completion_tokens.is_none()
        || settled.usage.output_partial
    {
        settled.usage.cost = None;
    }
    settled
}

/// Send the request, with ONE retry of a request refused BEFORE its stream starts (a 429, 529 or
/// other 5xx answered as the HTTP response: nothing streamed, nothing billed). The retry waits
/// `retry-after` seconds (a plain integer, default 2, capped at 20). A failure after the first body
/// byte is never retried: it is billed, and PM never doubles a bill. `sleeper` is the wait, so a test
/// never sleeps.
async fn send_with_one_retry<S, Fut>(
    endpoint: &str,
    key: &str,
    prepared: &Prepared,
    sleeper: &S,
) -> Result<reqwest::Response>
where
    S: Fn(Duration) -> Fut,
    Fut: Future<Output = ()>,
{
    let mut attempt = 0;
    loop {
        // Through the wire mark: every paid request announces itself (see the module header).
        let response = crate::wire_mark::send(
            auth(patient_client().post(endpoint), key)
                .header(reqwest::header::ACCEPT, "text/event-stream")
                .json(&prepared.body),
        )
        .await?;
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        // The raw body, bounded: the sentence is mapped from it and never echoes it whole.
        let raw = bounded_error_body(&response.text().await.unwrap_or_default());
        let fields = fields_from_body(&raw);
        let failure = classify(Some(status), &fields);
        if attempt == 0 {
            if let Some(wait) = retry_delay(status, retry_after.as_deref(), &failure) {
                attempt += 1;
                sleeper(wait).await;
                continue;
            }
        }
        // The wait it asked for (after the one retry this function made), for the entry-health memory.
        if let Some(wait) = retry_after
            .as_deref()
            .and_then(crate::wire_mark::parse_retry_after)
        {
            crate::wire_mark::note_retry_after_for(wait);
        }
        // A failed STATUS with no body streamed: the one site that may call it unbilled.
        return Err(failure_error(
            &failure,
            Some(status),
            &fields,
            prepared.model,
            prepared.effort.as_deref(),
        )
        .with_response_status(status));
    }
}

/// How long to wait before the one retry, or `None` for a failure that is not retried. PURE. Only a
/// rate limit, a ramp limit and an overload, and only when the status says the request was refused
/// outright. A spend cap is a 429 too but never clears by waiting a minute, and a credit, retention
/// or other 4xx answer does not change on a second try.
fn retry_delay(status: u16, retry_after: Option<&str>, failure: &Failure) -> Option<Duration> {
    let refused_outright = status == 429 || status >= 500;
    let waits = matches!(
        failure,
        Failure::RateLimited | Failure::RampLimit | Failure::Overloaded
    );
    if !(refused_outright && waits) {
        return None;
    }
    let secs = retry_after
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(2)
        .min(20);
    Some(Duration::from_secs(secs))
}

/// The request and its stream, against `endpoint` (the real one, or a loopback in a test).
async fn run<F, S, Fut>(
    endpoint: &str,
    key: &str,
    prepared: &Prepared,
    mut on_token: F,
    sleeper: S,
) -> Result<Completion>
where
    F: FnMut(&str),
    S: Fn(Duration) -> Fut,
    Fut: Future<Output = ()>,
{
    let response = send_with_one_retry(endpoint, key, prepared, &sleeper).await?;
    let mut turn = Turn::new(prepared.model, prepared.effort.clone());
    // Raw bytes, decoded one whole line at a time: a multi-byte character split across two network
    // chunks must not be decoded in halves.
    let mut buffer: Vec<u8> = Vec::new();
    let mut bytes = response.bytes_stream();
    while let Some(chunk) = bytes.next().await {
        buffer.extend_from_slice(&chunk?);
        for line in drain_lines(&mut buffer) {
            if let Some(token) = turn.feed_line(&line)? {
                on_token(&token);
            }
        }
        // Only an unfinished line remains; past the cap there is no newline coming.
        if buffer.len() > MAX_SSE_LINE_BYTES {
            return Err(Error::Other(
                "the model stream sent an oversized line".into(),
            ));
        }
    }
    // A final line with no newline after it is still a line.
    buffer.push(b'\n');
    for line in drain_lines(&mut buffer) {
        if let Some(token) = turn.feed_line(&line)? {
            on_token(&token);
        }
    }
    turn.finish()
}

// ---- the stream --------------------------------------------------------------------------------

/// The four counters Anthropic reports, each possibly absent.
#[derive(Default, Clone, Copy)]
struct Counters {
    input: Option<i64>,
    creation: Option<i64>,
    read: Option<i64>,
    output: Option<i64>,
    /// The part of the cache writes made for the ONE-HOUR lifetime (`cache_creation.
    /// ephemeral_1h_input_tokens`), which is already inside `creation`. Only its being non-zero
    /// matters: see `finish`.
    long_write: Option<i64>,
    /// `output_tokens_details.thinking_tokens`: the part of `output` spent thinking. Already inside
    /// `output` and priced there, so it is only ever reported, never added.
    thinking: Option<i64>,
}

impl Counters {
    fn of(usage: &Value) -> Self {
        Self {
            input: usage["input_tokens"].as_i64(),
            creation: usage["cache_creation_input_tokens"].as_i64(),
            read: usage["cache_read_input_tokens"].as_i64(),
            output: usage["output_tokens"].as_i64(),
            long_write: usage["cache_creation"]["ephemeral_1h_input_tokens"].as_i64(),
            thinking: usage["output_tokens_details"]["thinking_tokens"].as_i64(),
        }
    }

    /// Field by field, the larger value wins: every counter is cumulative, so the newest is the
    /// biggest, and a `0` placeholder in a later event must never lower a figure.
    fn merged(self, other: Counters) -> Counters {
        Counters {
            input: larger(self.input, other.input),
            creation: larger(self.creation, other.creation),
            read: larger(self.read, other.read),
            output: larger(self.output, other.output),
            long_write: larger(self.long_write, other.long_write),
            thinking: larger(self.thinking, other.thinking),
        }
    }
}

fn larger(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// One reply, read event by event. `feed_line` takes a line of the stream; [`Turn::finish`] turns what
/// was read into a [`Completion`].
struct Turn {
    model: &'static DirectModel,
    effort: Option<String>,
    text: String,
    served: Option<String>,
    /// `message_start`'s usage: its `output_tokens` is a PLACEHOLDER (the live probe showed 8 before
    /// any text), so the output count comes from the deltas.
    start: Counters,
    /// Every `message_delta`'s usage, merged field by field with the larger value winning: all four
    /// counters are cumulative, so the newest is the biggest, and a `0` placeholder in a later event
    /// must never lower a figure. Never added.
    delta: Counters,
    stop: Option<String>,
    /// Block indexes that are NOT text (thinking, redacted thinking, unknown kinds): their deltas
    /// never feed the reply.
    not_text: HashSet<u64>,
    ended: bool,
    /// The most telling token type PM has no price for, seen in either usage object
    /// (`usage_keys`). Only the note is kept, never a key's name.
    unpriced: Option<CostNote>,
}

impl Turn {
    fn new(model: &'static DirectModel, effort: Option<String>) -> Self {
        Self {
            model,
            effort,
            text: String::new(),
            served: None,
            start: Counters::default(),
            delta: Counters::default(),
            stop: None,
            not_text: HashSet::new(),
            ended: false,
            unpriced: None,
        }
    }

    /// One line of the stream. Only `data:` lines carry anything: `event:` lines, comments and
    /// blank lines are ignored, and so is a data line that is not JSON (the lines carry whitespace
    /// padding inside the JSON before the last brace, so a real parser reads them, never a string
    /// match).
    fn feed_line(&mut self, line: &str) -> Result<Option<String>> {
        let Some(data) = line.strip_prefix("data:") else {
            return Ok(None);
        };
        match serde_json::from_str::<Value>(data.trim()) {
            Ok(event) => self.feed(&event),
            Err(_) => Ok(None),
        }
    }

    /// One event. Returns the text delta to forward, if any. New event, block and delta types may
    /// appear, so an unknown one is ignored, never an error.
    fn feed(&mut self, event: &Value) -> Result<Option<String>> {
        match event["type"].as_str().unwrap_or_default() {
            "message_start" => {
                if self.served.is_none() {
                    self.served = event["message"]["model"].as_str().map(str::to_string);
                }
                self.start = self.start.merged(Counters::of(&event["message"]["usage"]));
                self.note_unpriced(&event["message"]["usage"]);
                Ok(None)
            }
            "content_block_start" => {
                let index = event["index"].as_u64().unwrap_or(0);
                let block = &event["content_block"];
                if block["type"].as_str() == Some("text") {
                    // A text block's own initial text is normally "", and is forwarded when it isn't.
                    let first = block["text"].as_str().unwrap_or_default();
                    if first.is_empty() {
                        return Ok(None);
                    }
                    return self.push(first);
                }
                self.not_text.insert(index);
                Ok(None)
            }
            "content_block_delta" => {
                let index = event["index"].as_u64().unwrap_or(0);
                let delta = &event["delta"];
                if delta["type"].as_str() == Some("text_delta") && !self.not_text.contains(&index) {
                    if let Some(text) = delta["text"].as_str() {
                        return self.push(text);
                    }
                }
                // thinking_delta, signature_delta, input_json_delta and anything new: not the reply.
                Ok(None)
            }
            "message_delta" => {
                if let Some(reason) = event["delta"]["stop_reason"].as_str() {
                    self.stop = Some(reason.to_string());
                }
                self.delta = self.delta.merged(Counters::of(&event["usage"]));
                self.note_unpriced(&event["usage"]);
                Ok(None)
            }
            "message_stop" => {
                self.ended = true;
                Ok(None)
            }
            "error" => {
                // A failure after the 200: no status, so one is derived from the type and the SAME
                // table answers. The partial reply is dropped with the turn.
                let fields = fields_from_error(&event["error"]);
                let status = status_of_type(&fields.error_type);
                let failure = classify(status, &fields);
                Err(failure_error(
                    &failure,
                    status,
                    &fields,
                    self.model,
                    self.effort.as_deref(),
                ))
            }
            // `ping` and anything else.
            _ => Ok(None),
        }
    }

    /// Remember a token type PM can't price if this usage object names one. The counters are
    /// cumulative, so a type seen in any event counts for the reply.
    fn note_unpriced(&mut self, usage: &Value) {
        self.unpriced = usage_keys::worse(
            self.unpriced,
            usage_keys::note_for(CloudProvider::Anthropic, usage),
        );
    }

    fn push(&mut self, text: &str) -> Result<Option<String>> {
        self.text.push_str(text);
        if self.text.len() > MAX_REPLY_BYTES {
            return Err(Error::Other(
                "the model reply exceeded the size limit".into(),
            ));
        }
        Ok(Some(text.to_string()))
    }

    /// The reply as a [`Completion`]. A stream that ends without `message_stop` is an ERROR, not a
    /// kept half-answer: a dropped connection gives no event, the end marker is reliable, and a half
    /// answer stored as complete is the worse failure. A refusal is NOT an error: it is HTTP 200 and
    /// billed, so it returns its usage with `refused` set and the partial text dropped.
    fn finish(self) -> Result<Completion> {
        if !self.ended {
            return Err(Error::Other(
                "Anthropic's reply was cut off before the end, so PM didn't keep it. Try again."
                    .into(),
            ));
        }
        // Input and cache counts: the larger of the start and any repeat. Output: the deltas' ONLY.
        // The start's is a placeholder (8 before any text), so a stream that ended with no delta has
        // NO output count: `settle` then prices it as unknown (a dash), never as the placeholder, which
        // would be a confident under-count.
        let input = larger(self.start.input, self.delta.input);
        let creation = larger(self.start.creation, self.delta.creation);
        let read = larger(self.start.read, self.delta.read);
        let output = self.delta.output;
        let long_write = larger(self.start.long_write, self.delta.long_write);
        let thinking = larger(self.start.thinking, self.delta.thinking);
        let any = [input, creation, read, output].iter().any(Option::is_some);
        let mut usage = Usage {
            cost_note: self.unpriced,
            ..Usage::default()
        };
        if any {
            // TOTAL input, the meaning OpenAI's `prompt_tokens` has: `input_tokens` alone is only the
            // uncached part. Thinking is already inside `output_tokens`. Every cache write is priced
            // at the five-minute rate, because PM only ever asks for that lifetime.
            let prompt =
                input.unwrap_or(0).max(0) + creation.unwrap_or(0).max(0) + read.unwrap_or(0).max(0);
            usage.prompt_tokens =
                (input.is_some() || creation.is_some() || read.is_some()).then_some(prompt);
            usage.completion_tokens = output;
            usage.cached_tokens = Some(read.unwrap_or(0));
            usage.cache_write_tokens = Some(creation.unwrap_or(0));
            // PM only asks for the five-minute lifetime and the catalogue holds no one-hour write
            // price (it is 2x, not the 1.25x PM would charge). If Anthropic ever changes its default
            // lifetime and reports one-hour writes, a price worked out here would under-count, so the
            // cost is left unknown instead (`settle` reads the flag as a dash).
            usage.output_partial = long_write.is_some_and(|n| n > 0);
            usage.thinking_tokens = thinking;
        }
        let stop = self.stop.as_deref();
        if stop == Some("refusal") {
            // `stop_details` is never read or echoed: untrusted text, and its stream placement is
            // unverified.
            usage.refused = true;
            return Ok(Completion {
                text: String::new(),
                model: self.served,
                usage,
                truncated: false,
            });
        }
        Ok(Completion {
            truncated: matches!(stop, Some("max_tokens" | "model_context_window_exceeded")),
            text: self.text,
            model: self.served,
            usage,
        })
    }
}

// ---- errors ------------------------------------------------------------------------------------

/// What Anthropic's error body says, with every field optional.
#[derive(Default)]
struct ErrorFields {
    /// `error.type`: `invalid_request_error`, `rate_limit_error`, ...
    error_type: String,
    message: String,
    /// `error.details.error_code`: `enforced_spend_limit_reached`, `ramp_limit_exceeded`, ...
    code: String,
}

fn fields_from_error(err: &Value) -> ErrorFields {
    ErrorFields {
        error_type: err["type"].as_str().unwrap_or_default().to_string(),
        message: err["message"].as_str().unwrap_or_default().to_string(),
        code: err["details"]["error_code"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    }
}

/// The raw (bounded) body of a failed response. Anthropic's own JSON is read by field; anything else
/// (a proxy's page, plain text) keeps its text as the message so the phrase scan still sees it.
fn fields_from_body(raw: &str) -> ErrorFields {
    match serde_json::from_str::<Value>(raw) {
        Ok(v) if v["error"].is_object() => fields_from_error(&v["error"]),
        _ => ErrorFields {
            message: raw.trim().to_string(),
            ..ErrorFields::default()
        },
    }
}

/// The status a mid-stream `error` event stands for, so the one table answers both.
fn status_of_type(error_type: &str) -> Option<u16> {
    Some(match error_type {
        "invalid_request_error" => 400,
        "authentication_error" => 401,
        "billing_error" => 402,
        "permission_error" => 403,
        "not_found_error" => 404,
        "request_too_large" => 413,
        "rate_limit_error" => 429,
        "api_error" => 500,
        "timeout_error" => 504,
        "overloaded_error" => 529,
        _ => return None,
    })
}

#[derive(Debug, PartialEq)]
enum Failure {
    KeyRejected,
    OutOfCredit,
    SpendCap,
    OwnSpendLimit,
    RampLimit,
    RateLimited,
    NeedsRetention,
    OnHold,
    OutputLimitStale,
    ContextTooLong,
    EffortRejected,
    SetupStale,
    TooLarge,
    NotAllowed,
    ModelUnavailable,
    Overloaded,
    Other,
}

/// What a failure means. PURE, and ORDER MATTERS. The spend cap is a 429 like a plain rate limit but
/// has no `retry-after` and never clears by waiting a minute, so the CODE is read before the status.
/// The exhausted-balance STATUS is unverified, so it is matched on 402 AND on its words. Context is
/// checked before the output limit because Anthropic's context message can also name `max_tokens`.
/// The wording of the 400 phrases is unverified for these models, so each falls through to `Other`
/// (a generic sentence with the redacted detail) when it doesn't match.
fn classify(status: Option<u16>, f: &ErrorFields) -> Failure {
    let lower = f.message.to_lowercase();
    match f.code.as_str() {
        "enforced_spend_limit_reached" => return Failure::SpendCap,
        "ramp_limit_exceeded" => return Failure::RampLimit,
        "organization_on_hold" => return Failure::OnHold,
        _ => {}
    }
    if status == Some(402)
        || lower.contains("credit balance")
        || lower.contains("purchase credits")
        || lower.contains("plans & billing")
    {
        return Failure::OutOfCredit;
    }
    let bad_request = status == Some(400);
    if bad_request
        && lower
            .trim_start()
            .starts_with("you have reached your specified")
    {
        return Failure::OwnSpendLimit;
    }
    if bad_request && lower.contains("data retention") {
        return Failure::NeedsRetention;
    }
    match status {
        Some(401) => return Failure::KeyRejected,
        Some(403) => return Failure::NotAllowed,
        Some(404) => return Failure::ModelUnavailable,
        Some(413) => return Failure::TooLarge,
        Some(429) => return Failure::RateLimited,
        _ => {}
    }
    if bad_request {
        if lower.contains("too long") || lower.contains("context") {
            return Failure::ContextTooLong;
        }
        if lower.contains("max_tokens") {
            return Failure::OutputLimitStale;
        }
        if lower.contains("effort") {
            return Failure::EffortRejected;
        }
        if lower.contains("thinking") || lower.contains("output_config") {
            return Failure::SetupStale;
        }
    }
    if status.is_some_and(|s| s >= 500)
        || matches!(
            f.error_type.as_str(),
            "overloaded_error" | "api_error" | "timeout_error"
        )
    {
        return Failure::Overloaded;
    }
    Failure::Other
}

/// The sentence a person reads. `{name}` and the level come from the catalogue row and the arm,
/// never from the error text, which is untrusted; the one place provider text is echoed (`Other`)
/// goes through `redact_detail` BEFORE the 200-character cut. No `request_id` and no `appeal_url`
/// ever reach it.
fn failure_text(
    failure: &Failure,
    status: Option<u16>,
    f: &ErrorFields,
    m: &DirectModel,
    effort: Option<&str>,
) -> String {
    let name = &m.name;
    match failure {
        Failure::KeyRejected => "Anthropic rejected your key (mistyped, revoked, expired, or from another organisation). Replace it in Settings > AI & Models.".into(),
        Failure::OutOfCredit => "Your Anthropic account is out of credit. Add credit in the Anthropic Console (platform.claude.com), then try again.".into(),
        Failure::SpendCap => "Your Anthropic organisation has reached its monthly spend cap, so Anthropic won't answer until it resets. Check the date in the Anthropic Console, or ask Anthropic to raise the cap.".into(),
        Failure::OwnSpendLimit => "You've reached the usage limit you set in the Anthropic Console. Raise it there, then try again.".into(),
        Failure::RampLimit => "Anthropic is still raising your account's limits. Try again in a minute.".into(),
        Failure::RateLimited => "Anthropic is rate-limiting this key. Try again in a minute.".into(),
        Failure::NeedsRetention => format!("Anthropic only runs {name} on a workspace with 30-day data retention switched on. In the Anthropic Console, put your key in a workspace with Standard data retention (Settings > Data retention), or pick another model here."),
        Failure::OnHold => "Anthropic has put your organisation on hold. Contact Anthropic.".into(),
        Failure::OutputLimitStale => "PM's output limit for this model is out of date. Update PM.".into(),
        Failure::ContextTooLong => format!("This conversation is longer than {name}'s limit. Compress it or start a new chat."),
        Failure::EffortRejected => format!("{name} doesn't take effort '{}' (PM's list may be out of date). Set Effort back to its default in Settings > AI & Models.", effort.unwrap_or("default")),
        Failure::SetupStale => format!("PM's request for {name} is out of date. Update PM."),
        Failure::TooLarge => "This conversation is too large to send. Compress it or start a new chat.".into(),
        Failure::NotAllowed => "Your Anthropic key isn't allowed to do this (its workspace or role blocks it).".into(),
        Failure::ModelUnavailable => format!("Your Anthropic account can't use {name}. Pick another model, or check your workspace's model access."),
        Failure::Overloaded => "Anthropic is having trouble right now. Try again shortly.".into(),
        Failure::Other => {
            let detail: String = redact_detail(&f.message).chars().take(200).collect();
            let at = status.map_or_else(|| "stream".to_string(), |s| s.to_string());
            format!("Anthropic request failed ({at}): {detail}")
        }
    }
}

fn failure_error(
    failure: &Failure,
    status: Option<u16>,
    f: &ErrorFields,
    m: &DirectModel,
    effort: Option<&str>,
) -> Error {
    Error::provider(
        CloudProvider::Anthropic,
        class_of(failure),
        failure_text(failure, status, f, m, effort),
    )
}

/// The class a failure is told as. The sentence is `failure_text`'s and does not depend on this.
/// Billing is the safe default here: only the response-status site in [`send_with_one_retry`] may
/// lower it, and a mid-stream `error` event never reaches that site.
fn class_of(f: &Failure) -> FailureClass {
    match f {
        Failure::KeyRejected | Failure::OnHold | Failure::NotAllowed => FailureClass::KeyBad,
        Failure::OutOfCredit | Failure::SpendCap | Failure::OwnSpendLimit => FailureClass::NoCredit,
        Failure::RampLimit | Failure::RateLimited => FailureClass::RateLimited,
        Failure::NeedsRetention => FailureClass::NeedsRetention,
        Failure::OutputLimitStale | Failure::EffortRejected | Failure::SetupStale => {
            FailureClass::SetupStale
        }
        Failure::ContextTooLong | Failure::TooLarge => FailureClass::ContextTooLong,
        Failure::ModelUnavailable => FailureClass::ModelGone,
        Failure::Overloaded => FailureClass::Busy,
        Failure::Other => FailureClass::Other,
    }
}

// ---- the key check -----------------------------------------------------------------------------

/// Whether a pasted key is the right shape to try: trimmed, 20 to 300 characters, visible ASCII only
/// (so a header can't fail on a control character). No prefix is REQUIRED: key shapes change and a
/// requirement would lock everyone out. What is turned away is another company's key: Google's
/// `AIza` and every `sk-` key that is not `sk-ant-` (OpenAI's project, service-account and admin
/// keys, OpenRouter's), so such a key is never sent to this one.
pub(crate) fn well_formed(key: &str) -> bool {
    let key = key.trim();
    (20..=300).contains(&key.len())
        && key.chars().all(|c| c.is_ascii_graphic())
        && !direct_providers::looks_like_another_providers_key(CloudProvider::Anthropic, key)
}

/// Whether a body is Anthropic's own error JSON. Only that is a verdict on the key: a captive
/// portal's or proxy's HTML 401 or 403 says nothing about it, and must not get a mistyped key kept.
fn from_anthropic(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .is_ok_and(|v| v["type"].as_str() == Some("error") && v["error"].is_object())
}

/// What the key check's LISTING means. PURE: the status and body in, the verdict out.
fn interpret(status: u16, body: &str) -> KeyOutcome {
    let ours = from_anthropic(body);
    let lower = body.to_lowercase();
    let credit_words = lower.contains("credit balance")
        || lower.contains("purchase credits")
        || lower.contains("plans & billing");
    if status == 200 {
        return KeyOutcome::Ok;
    }
    if status == 402 || (ours && credit_words) {
        return KeyOutcome::NoCredit;
    }
    match status {
        401 if ours => KeyOutcome::Rejected,
        403 if ours => KeyOutcome::Restricted,
        // A rate limit or an overloaded Anthropic is not a connection problem: "busy, wait a minute".
        429 | 500..=599 => KeyOutcome::Busy,
        other => KeyOutcome::Unexpected(other),
    }
}

/// The catalogue ids the key can see, from a `/v1/models` body, plus how many models it listed. A
/// model whose lifecycle is `retired` is not listed (a call naming it fails). Mythos, or anything
/// else PM doesn't list, counts in the total and never in the list.
fn listed_models(body: &str) -> (Vec<String>, usize) {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return (Vec::new(), 0);
    };
    let entries = v["data"].as_array().map(Vec::as_slice).unwrap_or_default();
    let listed = direct_models::models(CloudProvider::Anthropic)
        .iter()
        .filter(|m| {
            entries.iter().any(|e| {
                e["id"].as_str() == Some(m.wire.as_str())
                    && e["lifecycle"].as_str() != Some("retired")
            })
        })
        .map(|m| m.id.clone())
        .collect();
    (listed, entries.len())
}

/// The two URLs a key check talks to. The shipped pair is a const, so the key can only ever go to
/// Anthropic; tests hand the private [`check_key_at`] a loopback pair.
struct CheckUrls {
    models: &'static str,
    chat: &'static str,
}

const CHECK_URLS: CheckUrls = CheckUrls {
    models: MODELS_ENDPOINT,
    chat: ENDPOINT,
};

/// What the one test chat showed.
#[derive(Debug, PartialEq)]
enum Probe {
    /// 200: the key can chat, and has credit.
    Works,
    /// No credit, or a spend limit: every chat fails until the person acts. The key is valid.
    NoCredit,
    /// 401 or 403 from Anthropic's own JSON: valid, but not allowed to chat.
    Refused,
    /// The model is gone or not this key's: try the next cheapest.
    ModelGone,
    /// A rate limit, a 5xx, a retention 400, a timeout or anything PM can't read: no verdict.
    Untried,
}

/// What a test chat's reply means. PURE. Everything unlisted is `Untried`: a rate limit or a 5xx
/// must never read as a failure of the key.
fn read_probe(status: u16, body: &str) -> Probe {
    if status == 200 {
        return Probe::Works;
    }
    let f = fields_from_body(body);
    let ours = from_anthropic(body);
    match classify(Some(status), &f) {
        Failure::OutOfCredit | Failure::SpendCap | Failure::OwnSpendLimit => Probe::NoCredit,
        Failure::ModelUnavailable => Probe::ModelGone,
        // The listing already worked, so a 401 here is a missing scope, not a mistyped key.
        Failure::NotAllowed | Failure::KeyRejected if ours => Probe::Refused,
        _ => Probe::Untried,
    }
}

/// The test chat's body: PM's own shape with the least spend that still proves a chat works. The
/// model's LOWEST effort, NO `thinking` field (the probe proves a chat works, it doesn't exercise a
/// knob), no `stream` (one small JSON answers), 32 tokens (a one-word answer is about 5; the margin
/// covers a thinking burst, and a 200 that stops at the limit with no text still proves a chat).
fn probe_body(m: &DirectModel) -> Value {
    let wire = Wire {
        system: Vec::new(),
        messages: vec![json!({"role": "user", "content": "Reply with OK."})],
    };
    chat_body(
        m,
        m.effort_levels.first().map(String::as_str),
        None,
        wire,
        false,
        32,
    )
}

/// Whether a model is one Anthropic only runs on a workspace with 30-day retention. The row says so
/// in its note; the test chat never uses such a model unless it is the only one left, because a
/// zero-retention workspace answers it with the retention 400, which must never read as a bad key.
fn needs_retention(m: &DirectModel) -> bool {
    m.wire.starts_with("claude-fable")
}

/// The models to try for the test chat, cheapest first: the catalogue rows the key lists, or every
/// row when the listing was refused (or named none PM knows). FABLE IS NEVER PROBED unless it is the
/// only row left: a Haiku pass says nothing about Fable anyway, and the Fable case is answered when
/// it matters, at chat time, by the mapped sentence.
fn probe_order(listed: &[String]) -> Vec<&'static DirectModel> {
    let mut rows: Vec<&DirectModel> = direct_models::models(CloudProvider::Anthropic)
        .iter()
        .filter(|m| listed.is_empty() || listed.contains(&m.id))
        .collect();
    rows.sort_by(|a, b| {
        (a.price_in + a.price_out)
            .partial_cmp(&(b.price_in + b.price_out))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let others: Vec<&DirectModel> = rows
        .iter()
        .copied()
        .filter(|m| !needs_retention(m))
        .collect();
    if others.is_empty() {
        rows
    } else {
        others
    }
}

/// Send the test chat. The key never appears in a URL, a log or an error. One small paid request
/// (a hundredth of a cent); it is not written to the usage log, and it is a plain send (it belongs
/// to no mod, as the listing doesn't).
async fn send_probe(key: &str, chat_url: &str, m: &DirectModel) -> Probe {
    // A reply this size is a proxy's page or a runaway; the real answer is a few hundred bytes.
    const MAX_BODY: usize = 64 * 1024;
    let request = auth(
        check_client()
            .post(chat_url)
            .timeout(Duration::from_secs(30)),
        key,
    )
    .json(&probe_body(m));
    let Ok(response) = request.send().await else {
        return Probe::Untried;
    };
    let status = response.status().as_u16();
    match response.text().await {
        Ok(t) if t.len() <= MAX_BODY => read_probe(status, &t),
        _ => Probe::Untried,
    }
}

/// Try the test chat on the cheapest model, and once more on the next if the first is gone.
async fn probe_chat(key: &str, chat_url: &str, listed: &[String]) -> Probe {
    let mut order = probe_order(listed).into_iter();
    let mut last = Probe::Untried;
    for _ in 0..2 {
        let Some(m) = order.next() else { break };
        match send_probe(key, chat_url, m).await {
            Probe::ModelGone => last = Probe::Untried,
            verdict => return verdict,
        }
    }
    last
}

/// Ask Anthropic which models the key can see, then send ONE tiny test chat so the verdict is about
/// chatting and not only about listing. A key with no credit lists models fine and fails every chat,
/// and a read-only key does the same.
pub(crate) async fn check_key(key: &str) -> KeyCheck {
    check_key_at(key, &CHECK_URLS).await
}

async fn check_key_at(key: &str, urls: &CheckUrls) -> KeyCheck {
    let request = auth(
        check_client()
            .get(urls.models)
            .timeout(Duration::from_secs(30)),
        key,
    );
    let Ok(response) = request.send().await else {
        return KeyCheck::of(KeyOutcome::Unreachable);
    };
    let status = response.status().as_u16();
    // The model list is a few dozen short rows; anything near this cap is not Anthropic.
    const MAX_BODY: usize = 4 * 1024 * 1024;
    let body = match response.text().await {
        Ok(t) if t.len() <= MAX_BODY => t,
        Ok(_) => return KeyCheck::of(KeyOutcome::Unexpected(status)),
        Err(_) => return KeyCheck::of(KeyOutcome::Unreachable),
    };
    let listing = interpret(status, &body);
    // Only a key Anthropic accepted gets a test chat: a rejected, no-credit, unreachable or odd
    // listing is already answered, and PM never spends on it.
    if !matches!(listing, KeyOutcome::Ok | KeyOutcome::Restricted) {
        return KeyCheck::of(listing);
    }
    let (listed, listed_total) = if listing == KeyOutcome::Ok {
        listed_models(&body)
    } else {
        (Vec::new(), 0)
    };
    let (outcome, note) = match probe_chat(key, urls.chat, &listed).await {
        Probe::Works => (listing, None),
        Probe::NoCredit => (KeyOutcome::NoCredit, None),
        Probe::Refused if listing == KeyOutcome::Ok => {
            (KeyOutcome::Restricted, Some(KeyNote::ReadOnly))
        }
        Probe::Refused => (KeyOutcome::Restricted, Some(KeyNote::NoAccess)),
        // The listing's verdict stands, said with the note: never a false failure.
        Probe::ModelGone | Probe::Untried => (listing, Some(KeyNote::ChatUntried)),
    };
    KeyCheck {
        outcome,
        listed,
        listed_total,
        note,
        tier: None,
        refusal: None,
    }
}

#[cfg(test)]
#[path = "anthropic_fixtures.rs"]
mod fixtures;

#[cfg(test)]
#[path = "anthropic_direct_tests.rs"]
mod tests;
