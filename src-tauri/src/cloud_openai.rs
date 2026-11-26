// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The OpenAI chat-completions dialect, shared by every cloud provider that speaks it.
//!
//! OpenRouter was the only one, so its request body, its server-sent-events loop (with the size caps
//! on untrusted model output, the mid-stream error chunk and the `length` stop) and its one-shot
//! completion lived in `openrouter.rs`. Your own OpenAI and Gemini keys speak the same dialect, so
//! that machinery moved here, unchanged, and what differs between providers is one [`Flavour`]: the
//! endpoint, how the key is sent, the few body fields only that provider wants, and how a failed
//! response reads. `openrouter::FLAVOUR` is the one flavour that exists today, and its bodies are held
//! byte for byte by the tests in `openrouter.rs`.
//!
//! The types a caller sees (`ChatMessage`, `Completion`, `Usage`) stay in `openrouter.rs`, where
//! every caller already imports them from.

use futures_util::StreamExt;

use crate::error::{Error, Result};
use crate::openrouter::{drain_lines, ChatMessage, Completion, Usage};

/// What differs between two providers that speak the same dialect. Plain data and function pointers,
/// so a flavour is a `const` next to the provider's own code.
pub(crate) struct Flavour {
    /// The provider as a person names it, for the errors that name it.
    pub name: &'static str,
    /// The chat-completions URL.
    pub endpoint: &'static str,
    /// Put the key on a request: the header the provider wants it in.
    pub auth: fn(reqwest::RequestBuilder, &str) -> reqwest::RequestBuilder,
    /// Whether a message marked as the end of the stable prefix is sent as a content block carrying
    /// `cache_control`. A provider that doesn't know the field would reject it, so it gets a plain
    /// string and the mark is dropped.
    pub cache_control_blocks: bool,
    /// Add the provider's own fields to the body, given the ordered model list: how the model is
    /// named (one, or a fallback list), and anything that provider pins or asks for.
    pub decorate: fn(&mut serde_json::Value, &[String]),
    /// Turn a failed response into the error the person sees.
    pub request_error: fn(reqwest::StatusCode, &str) -> Error,
}

/// `Authorization: Bearer <key>`, which is how OpenRouter, OpenAI and Gemini's compatible endpoint
/// all take a key.
pub(crate) fn bearer(request: reqwest::RequestBuilder, key: &str) -> reqwest::RequestBuilder {
    request.bearer_auth(key)
}

/// One shared HTTP client for every cloud call (F-16). A fresh `Client::builder().build()` per
/// request threw away the connection pool each time; a single `LazyLock` client reuses pooled TLS
/// connections across catalogue fetches, streamed chats, and background completions. It carries only
/// the two client-level bounds that suit all three sites — `connect_timeout` (establishing the socket)
/// and `read_timeout` (silence *between* chunks, reset on every byte received). The per-request TOTAL
/// deadline is deliberately NOT here: it differs per call (30 s catalogue, 120 s `complete`, and none
/// for a healthy stream — see [`stream`]), so each site sets its own `.timeout()` on the request
/// builder. Building a client that only sets timeouts is effectively infallible, hence `expect`.
///
/// No request carries an app-attribution header (`HTTP-Referer`, `X-Title`, or any
/// `X-OpenRouter-*`): OpenRouter uses them to put an app in its public usage rankings and to keep
/// per-app analytics, so they would publish how much PM's users use which models, and no other
/// provider has any use for them. PM sets no `User-Agent` either, and reqwest sends none of its own.
/// `no_request_carries_app_attribution` (here and in `openrouter.rs`) keeps it that way.
static HTTP: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .read_timeout(std::time::Duration::from_secs(120))
        .build()
        .expect("reqwest client with static timeouts should build")
});

pub(crate) fn http() -> &'static reqwest::Client {
    &HTTP
}

/// Build the request body: the dialect's own fields, then whatever the flavour adds.
///
/// Build the messages array by hand so we can optionally mark a stable system prefix with an
/// ephemeral prompt-cache breakpoint. `cache_through` is the index of the LAST message of the
/// stable prefix: a provider that supports prompt caching caches everything up to AND INCLUDING that
/// message and bills the reused prefix at the cheap cache-read rate — a big saving when the same
/// prefix repeats across calls (the per-document review loop marks index 0; chat marks the last of
/// its profile/agenda/summary block, keeping the per-turn grounding + history AFTER the breakpoint
/// where they stay uncached). Non-marked messages serialise exactly as before.
///
/// `max_tokens` is `None` for every caller PM has had: the body is then exactly what it always was
/// (a test holds it byte for byte). A mod's `ai.complete` is the one caller that sets it, because
/// the worst-case price PM books before the call is only a bound if the answer is bounded.
pub(crate) fn build_body(
    flavour: &Flavour,
    models: &[String],
    messages: &[ChatMessage],
    stream: bool,
    cache_through: Option<usize>,
    max_tokens: Option<u32>,
) -> serde_json::Value {
    let msgs: Vec<serde_json::Value> = messages
        .iter()
        .enumerate()
        .map(|(i, m)| {
            if flavour.cache_control_blocks && cache_through == Some(i) {
                serde_json::json!({
                    "role": m.role,
                    "content": [{
                        "type": "text",
                        "text": m.content,
                        "cache_control": { "type": "ephemeral" },
                    }],
                })
            } else {
                serde_json::json!({ "role": m.role, "content": m.content })
            }
        })
        .collect();

    let mut body = serde_json::json!({
        "messages": msgs,
        "stream": stream,
    });
    if stream {
        // Ask for a final usage chunk so we can log token spend.
        body["stream_options"] = serde_json::json!({ "include_usage": true });
    }
    if let Some(limit) = max_tokens {
        body["max_tokens"] = serde_json::json!(limit);
    }
    (flavour.decorate)(&mut body, models);
    body
}

/// The error a stream chunk reports, if any.
///
/// Mid-stream failures do NOT arrive as an HTTP status — the response was already 200 and some
/// tokens may already have been emitted. They arrive as a `data:` event carrying an `error` object.
/// Ignoring it meant the loop just ran out of chunks and returned the truncated text as a
/// SUCCESSFUL completion, which the caller then persisted as a complete assistant turn.
///
/// Pure, so the shape-matching — the part that can silently be wrong — is testable without a socket.
pub(crate) fn chunk_error(flavour: &Flavour, value: &serde_json::Value) -> Option<Error> {
    let err = value.get("error")?;
    let detail = err
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("the model stream reported an error");
    // The code is advisory here (the transport said 200). Routing through the flavour's
    // `request_error` means a refusal that arrives mid-stream gets the same actionable copy as one
    // at request time.
    let status = err
        .get("code")
        .and_then(|c| c.as_u64())
        .and_then(|c| u16::try_from(c).ok())
        .and_then(|c| reqwest::StatusCode::from_u16(c).ok())
        .unwrap_or(reqwest::StatusCode::BAD_GATEWAY);
    Some((flavour.request_error)(status, detail))
}

/// Whether a chunk says the model stopped because it ran out of room, rather than because it had
/// finished. Any other reason ("stop", "tool_calls", …) is a normal end.
pub(crate) fn is_length_stop(value: &serde_json::Value) -> bool {
    value["choices"][0]["finish_reason"].as_str() == Some("length")
}

/// Extract token usage from a response/chunk's `usage` object (absent fields → None). `cost` is
/// present only when the provider reports one (OpenRouter does when asked, via `usage.include`).
pub(crate) fn parse_usage(value: &serde_json::Value) -> Usage {
    Usage {
        prompt_tokens: value["usage"]["prompt_tokens"].as_i64(),
        completion_tokens: value["usage"]["completion_tokens"].as_i64(),
        cost: value["usage"]["cost"].as_f64(),
    }
}

/// POST a streaming chat completion with a body from [`build_body`]. `on_token` is called with each
/// text delta as it arrives. Returns the assembled reply plus the model that actually served it
/// (which can differ from the first requested model when a fallback fires).
pub(crate) async fn stream<F>(
    flavour: &Flavour,
    api_key: &str,
    body: &serde_json::Value,
    mut on_token: F,
) -> Result<Completion>
where
    F: FnMut(&str),
{
    // A long, healthy streamed reply can legitimately run for many minutes (a slow or reasoning model,
    // a large grounded answer). Bounding the WHOLE request with a total `.timeout` therefore severs a
    // reply mid-stream — the user watches tokens arrive, then gets an error, and the aborted turn is
    // never persisted → via F-02 the conversation wedges (F-16 / B4-5). The total deadline was only ever
    // meant to bound a hung connection, not a healthy slow stream. Replace it with the two signals that
    // actually indicate a dead connection: `connect_timeout` bounds establishing the socket, and
    // `read_timeout` bounds *silence between chunks* — it resets on every byte received, so steady tokens
    // never trip it however long the reply, while a genuinely stalled stream still aborts. Those two
    // bounds now live on the shared `HTTP` client (F-16), so this request just adds NO total `.timeout()`.
    // Non-streaming `complete` keeps its total deadline via its own per-request `.timeout()`: there the
    // whole answer arrives at once, so a total bound fits.
    let response = (flavour.auth)(HTTP.post(flavour.endpoint), api_key)
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .json(body)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let detail = crate::error::truncate_detail(&response.text().await.unwrap_or_default());
        return Err((flavour.request_error)(status, &detail));
    }

    // Untrusted model output (rule #6): bound both the assembled reply and any
    // single unterminated SSE line so a malicious/runaway endpoint can't grow
    // memory without limit within the request timeout. (The ICS feed path caps
    // fetched bytes the same way.) Both limits sit far above any real reply.
    const MAX_REPLY_BYTES: usize = 8 * 1024 * 1024;
    const MAX_SSE_LINE_BYTES: usize = 2 * 1024 * 1024;

    let mut full = String::new();
    let mut served: Option<String> = None;
    let mut usage = Usage::default();
    // Raw bytes, decoded only one complete line at a time: a multi-byte UTF-8
    // char split across two network chunks must not be lossily decoded in halves.
    let mut truncated = false;
    let mut buffer: Vec<u8> = Vec::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        buffer.extend_from_slice(&chunk?);

        // Server-Sent Events are newline-delimited; process whole lines only.
        for line in drain_lines(&mut buffer) {
            let Some(data) = line.strip_prefix("data:") else {
                continue; // SSE comments / keep-alives
            };
            let data = data.trim();
            if data == "[DONE]" {
                return Ok(Completion {
                    text: full,
                    model: served,
                    usage,
                    truncated,
                });
            }
            if data.is_empty() {
                continue;
            }
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(data) {
                // A mid-stream failure. Erroring routes into `send_message`'s existing error path
                // (ChatEvent::Error), whose dangling-user-turn self-heal already covers the user row
                // persisted before the stream began. `full` is dropped on purpose: a partial reply
                // from a failed call is exactly what must not be kept.
                if let Some(e) = chunk_error(flavour, &value) {
                    return Err(e);
                }
                // The chunk carries which model actually served the request — keep
                // the first one we see so the stored message reflects any fallback.
                if served.is_none() {
                    if let Some(m) = value["model"].as_str() {
                        served = Some(m.to_string());
                    }
                }
                // Why the reply stopped. Flag, not error — the text is worth keeping; the caller
                // marks the stored turn honest (see `Completion::truncated`).
                if is_length_stop(&value) {
                    truncated = true;
                }
                // The final chunk (empty choices) carries token usage — we asked for
                // it via stream_options; keep it whenever present.
                let u = parse_usage(&value);
                if u.prompt_tokens.is_some() || u.completion_tokens.is_some() {
                    usage = u;
                }
                if let Some(token) = value["choices"][0]["delta"]["content"].as_str() {
                    full.push_str(token);
                    if full.len() > MAX_REPLY_BYTES {
                        return Err(Error::Other(
                            "the model reply exceeded the size limit".into(),
                        ));
                    }
                    on_token(token);
                }
            }
        }

        // After draining complete lines, only an unfinished line remains; if it has
        // grown past the cap there's no newline coming — bail rather than buffer on.
        if buffer.len() > MAX_SSE_LINE_BYTES {
            return Err(Error::Other(
                "the model stream sent an oversized line".into(),
            ));
        }
    }

    Ok(Completion {
        text: full,
        model: served,
        usage,
        truncated,
    })
}

/// A single non-streaming chat completion with a body from [`build_body`] — used for background work
/// (sorting proposals, the Learning-You profile) where we want the whole answer at once, not a token
/// stream. Returns the assistant message content.
pub(crate) async fn complete(
    flavour: &Flavour,
    api_key: &str,
    body: &serde_json::Value,
) -> Result<Completion> {
    // Through `wire_mark::send` so a mod's `ai.complete` can tell a request that never connected
    // (its booking is given back) from one that may have left; a plain send for everyone else.
    let response = crate::wire_mark::send(
        (flavour.auth)(
            HTTP.post(flavour.endpoint)
                // A total deadline fits here (unlike `stream`): the whole answer arrives at once.
                .timeout(std::time::Duration::from_secs(120)),
            api_key,
        )
        .json(body),
    )
    .await?;

    if !response.status().is_success() {
        let status = response.status();
        let detail = crate::error::truncate_detail(&response.text().await.unwrap_or_default());
        return Err((flavour.request_error)(status, &detail));
    }

    let value: serde_json::Value = response.json().await?;
    let text = value["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| Error::Other(format!("{} response had no message content", flavour.name)))?;
    Ok(Completion {
        text,
        model: value["model"].as_str().map(str::to_string),
        usage: parse_usage(&value),
        // Same signal, same meaning — the non-streaming path gets it in one response rather than a
        // chunk, and callers must not have to care which path produced the Completion.
        truncated: value["choices"][0]["finish_reason"].as_str() == Some("length"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No cloud request carries an app-attribution header or a `User-Agent` (see `HTTP` above). Read
    /// case-insensitively from this file, which builds every request of the dialect, with the names
    /// built from parts so the test can't match itself. `openrouter.rs` holds the same line for the
    /// requests it builds itself (the catalogue and the ZDR feed).
    #[test]
    fn no_request_carries_app_attribution() {
        let src = include_str!("cloud_openai.rs").to_ascii_lowercase();
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

    /// A made-up flavour that adds nothing and takes no cache marks: the flavour is what shapes the
    /// body, and the dialect's own fields are the same under any of them.
    fn plain_flavour() -> Flavour {
        fn decorate(body: &mut serde_json::Value, models: &[String]) {
            body["model"] = serde_json::json!(models[0]);
        }
        fn request_error(status: reqwest::StatusCode, detail: &str) -> Error {
            Error::Other(format!("Plain request failed ({status}): {detail}"))
        }
        Flavour {
            name: "Plain",
            endpoint: "https://example.invalid/v1/chat/completions",
            auth: bearer,
            cache_control_blocks: false,
            decorate,
            request_error,
        }
    }

    #[test]
    fn the_flavour_decides_the_provider_fields_and_whether_cache_marks_are_sent() {
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
        let models = ["m1".to_string(), "m2".to_string()];
        let body = build_body(&plain_flavour(), &models, &msgs, true, Some(0), Some(50));
        assert_eq!(
            serde_json::to_string(&body).unwrap(),
            r#"{"max_tokens":50,"messages":[{"content":"s","role":"system"},{"content":"u","role":"user"}],"model":"m1","stream":true,"stream_options":{"include_usage":true}}"#,
            "no cache block, no ZDR pin, no usage request: those are OpenRouter's, not the dialect's"
        );
    }

    #[test]
    fn a_mid_stream_error_reads_in_the_flavours_own_words() {
        let chunk = serde_json::json!({"error": {"code": 429, "message": "slow down"}});
        let err = chunk_error(&plain_flavour(), &chunk).expect("an error chunk");
        assert!(err.to_string().starts_with("Plain request failed (429"));
        assert!(chunk_error(&plain_flavour(), &serde_json::json!({"choices": []})).is_none());
    }
}
