// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `net.fetch` (Mods spec §1.5, §1.7, plan X6): a mod asks PM to fetch an https URL on a host its
//! manifest lists, and gets the status, a few headers and the body. Experimental (D13): outside the
//! API freeze, listed so in the Developer tab. The rules about WHAT may be sent and seen are
//! [`super::net`]'s (all pure), the loop that follows redirects hop by hop, screens and pins is
//! `netfetch`'s; this file joins them with the grant, the stored key and the call's lifetime.
//!
//! **The flow**, each refusal one fixed PM sentence under a `MOD-####` code (D-25):
//! 1. The arguments are parsed strictly (`{url, method?, headers?, body? | bodyBase64?, secret?}`,
//!    unknown fields refused) and the body decoded.
//! 2. ONE blocking step under the DB guard, no await inside: [`super::recheck`] (the grant, D3
//!    included: `internet` is egress class, so an unsigned mod holding a read grant is refused here
//!    like any other egress call), the manifest's `internetHosts`, and, when the call names a
//!    `secret`, the stored key ([`secrets::read_for_use`], which only returns a key still bound to
//!    its slot's present host and header). The guard is dropped before anything waits.
//! 3. [`net::build_request`] judges the whole request on its parsed form. A key goes on the FIRST
//!    hop only (D-7) and only to the slot's own host: another listed host is refused before
//!    anything is dialled, because the person approved the key for one destination.
//! 4. The fetch, under [`limits::CALL_TIMEOUT`] outside and the policy's own 30 s inside, with
//!    the call's session watched for its whole life: [`netfetch::fetch`] asks "is this call still
//!    current" before every hop (so a session that ended during one hop sends no second), and the
//!    watcher asks every [`WATCH_EVERY`] while anything waits (a lookup, a connection, a slow body).
//!    The runtime has no cancel signal, so a call whose session ended is simply DROPPED, which
//!    drops the in-flight request.
//! 5. `still_current` and the store epoch once more, then the reply `{status, headers, bodyText |
//!    bodyBase64}`. Nothing of the URL, the final address or the network's own words is in an
//!    error, and the final URL is not in the answer.
//!
//! **What this does not stop** is a mod that wrote the path, query or body of a request to a host
//! it declared (an exfiltration channel D3 bars only for an unsigned mod that also reads); first
//! and second party mods rely on review (spec §1.5). And a key goes to the slot's host, whose
//! server may echo it back: the person approved that destination for that key.
//!
//! The key's text lives in memory only as long as this call (it is never logged, never in an error,
//! and `SlotSecret`/`SecretValue` print as nothing).

use std::collections::BTreeMap;
use std::future::Future;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::Url;
use serde::Deserialize;
use serde_json::Value;

use super::handlers::{parse, within_call_timeout};
use super::net::{self, UrlRefusal};
use super::{blocking, limits, recheck, still_current, Authorised, Host, ModError, Reply};
use crate::mods::netfetch::{self, Failure, Policy};
use crate::mods::secrets;

/// How often a fetch in flight is asked whether its session is still the mod's. A hop boundary is
/// checked exactly (`netfetch::fetch`'s per-hop rule); this bounds how long a lookup, a connection
/// or a body that stalls can outlive its session.
const WATCH_EVERY: Duration = Duration::from_millis(100);

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Args {
    url: String,
    #[serde(default)]
    method: Option<String>,
    /// Name to value. Names are matched case-insensitively and may not repeat.
    #[serde(default)]
    headers: BTreeMap<String, String>,
    /// A text body.
    #[serde(default)]
    body: Option<String>,
    /// A binary body, as base64.
    #[serde(default)]
    body_base64: Option<String>,
    /// The name of one of the manifest's secret slots: its key is added to this request as the
    /// slot's header.
    #[serde(default)]
    secret: Option<String>,
}

/// The longest base64 text that can decode to [`limits::MAX_FETCH_REQUEST_BYTES`] (four characters
/// for every three bytes, rounded up), so an oversize body is refused before it is decoded.
const MAX_BODY_BASE64_CHARS: usize = limits::MAX_FETCH_REQUEST_BYTES.div_ceil(3) * 4;

/// The request body the arguments name: `body` as UTF-8, or `bodyBase64` decoded, or none. Both is
/// the mod's mistake. The size cap proper is [`net::check_body`]'s; this only refuses to decode
/// something that cannot fit.
fn request_body(text: Option<String>, base64: Option<String>) -> Result<Option<Vec<u8>>, ModError> {
    match (text, base64) {
        (None, None) => Ok(None),
        (Some(t), None) => Ok(Some(t.into_bytes())),
        (None, Some(b)) => {
            if b.len() > MAX_BODY_BASE64_CHARS {
                return Err(ModError::bad_args("That request's body is too large."));
            }
            STANDARD
                .decode(b.as_bytes())
                .map(Some)
                .map_err(|_| ModError::bad_args("bodyBase64 isn't valid base64."))
        }
        (Some(_), Some(_)) => Err(ModError::bad_args("Send a body or a bodyBase64, not both.")),
    }
}

/// The hosts a stored manifest lets the mod fetch, as written (tolerant: an entry that is not
/// text is not a host, and a manifest that does not read has none, so nothing can be fetched).
fn internet_hosts(manifest: &str) -> Vec<String> {
    serde_json::from_str::<Value>(manifest)
        .ok()
        .and_then(|v| {
            v["internetHosts"].as_array().map(|list| {
                list.iter()
                    .filter_map(|h| h.as_str().map(str::to_owned))
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// What step 2 reads under the guard: the hosts, and the key if one was named.
struct Prepared {
    hosts: Vec<String>,
    secret: Option<net::SlotSecret>,
}

/// Step 2, in a blocking task. Nothing here waits, so the DB guard is held for reads only.
fn prepare<H: Host>(
    host: &H,
    auth: &Authorised,
    secret: Option<&str>,
) -> Result<Prepared, ModError> {
    #[cfg(test)]
    host.at(super::Point::BeforeData);
    let conn = host.store().conn().map_err(|_| ModError::locked())?;
    recheck(host, &conn, auth)?;
    let id = auth.ticket.mod_id.as_str();
    let manifest: String = conn
        .query_row("SELECT manifest FROM mods WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .map_err(ModError::internal)?;
    let secret = match secret {
        None => None,
        Some(name) => {
            // No key, a slot the manifest never declared and a row bound to another host or header
            // are all the same answer: "not set" (`read_for_use` binds in its query).
            let bound = secrets::read_for_use(&conn, id, name)
                .map_err(ModError::internal)?
                .ok_or_else(ModError::key_not_stored)?;
            Some(net::slot_secret(
                &bound.slot.host,
                &bound.slot.header,
                bound.value.expose(),
            )?)
        }
    };
    Ok(Prepared {
        hosts: internet_hosts(&manifest),
        secret,
    })
}

/// Why a hop was not made: the URL rule said no, or the call's session is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    Url(UrlRefusal),
    Ended,
}

/// A failed fetch as the fixed sentence a mod gets.
fn failure(f: Failure<Stop>) -> ModError {
    match f {
        Failure::Refused(Stop::Ended) => ModError::session_ended(),
        Failure::Refused(Stop::Url(refusal)) => net::failure_error(Failure::Refused(refusal)),
        Failure::Fetch(e) => net::failure_error(Failure::Fetch(e)),
    }
}

/// `work` run to its end, or dropped the moment the call stops being current (asked every
/// [`WATCH_EVERY`]): dropping an in-flight fetch closes its connection.
async fn until_over<H: Host, T>(
    host: &H,
    auth: &Authorised,
    work: impl Future<Output = T>,
) -> Result<T, ModError> {
    tokio::pin!(work);
    let mut tick = tokio::time::interval(WATCH_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            done = &mut work => return Ok(done),
            _ = tick.tick() => still_current(host, auth)?,
        }
    }
}

/// `net.fetch({url, method?, headers?, body?, bodyBase64?, secret?})` →
/// `{status, headers, bodyText | bodyBase64}`.
pub(super) async fn fetch<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    within_call_timeout(run(host, auth, args, Policy::for_mods())).await
}

/// [`fetch`] under `policy` (a test's short or small one).
pub(super) async fn run<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
    policy: Policy,
) -> Result<Reply, ModError> {
    let Args {
        url,
        method,
        headers,
        body,
        body_base64,
        secret,
    } = parse(args)?;
    let body = request_body(body, body_base64)?;

    // 2. What only the store can say, under the guard, with no await inside.
    let Prepared { hosts, secret } = {
        let (host, auth) = (host.clone(), auth.clone());
        blocking(move || prepare(&host, &auth, secret.as_deref())).await?
    };
    still_current(host, auth)?;

    // 3. The whole request, judged on its parsed form.
    let headers: Vec<(String, String)> = headers.into_iter().collect();
    let request = net::build_request(
        &url,
        method.as_deref().unwrap_or("GET"),
        &headers,
        body,
        &hosts,
        secret,
    )?;

    // 4. The fetch. Each hop is checked against the manifest's hosts AND asked whether the call is
    // still the mod's before anything is looked up.
    let on_list = net::hop_check(&hosts);
    let check = |next: &Url| {
        still_current(host, auth).map_err(|_| Stop::Ended)?;
        on_list(next).map_err(Stop::Url)
    };
    let answered = until_over(
        host,
        auth,
        netfetch::fetch(host.hops(), request, &policy, check),
    )
    .await?;

    // 5. Only a call that is still the mod's, over the store it began in, is answered.
    still_current(host, auth)?;
    if host.store().store_epoch() != auth.ticket.store_epoch {
        return Err(ModError::session_ended());
    }
    match answered {
        Ok(reply) => Ok(Reply::value(net::reply_value(reply))),
        Err(f) => Err(failure(f)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::api::Code;
    use crate::mods::netfetch::FetchError;

    #[test]
    fn a_body_is_text_or_base64_never_both_and_base64_is_decoded() {
        assert_eq!(request_body(None, None).unwrap(), None);
        assert_eq!(
            request_body(Some("héllo".into()), None).unwrap(),
            Some("héllo".as_bytes().to_vec())
        );
        assert_eq!(
            request_body(None, Some("AAEC/w==".into())).unwrap(),
            Some(vec![0, 1, 2, 255])
        );
        for (text, b64) in [
            (Some("a".to_string()), Some("AA==".to_string())),
            (None, Some("not base64!".to_string())),
            (None, Some("AA".to_string())),
        ] {
            assert_eq!(
                request_body(text, b64).unwrap_err().code,
                Code::BadArgs,
                "refused"
            );
        }
    }

    #[test]
    fn base64_that_cannot_fit_the_request_cap_is_refused_before_it_is_decoded() {
        let largest = "A".repeat(MAX_BODY_BASE64_CHARS);
        assert!(request_body(None, Some(largest)).is_ok());
        let past = "A".repeat(MAX_BODY_BASE64_CHARS + 4);
        assert_eq!(
            request_body(None, Some(past)).unwrap_err().code,
            Code::BadArgs
        );
        // And the cap is the request cap: what decodes from the longest accepted text can exceed
        // it only by the rounding (`check_body` then refuses the excess).
        const {
            assert!(MAX_BODY_BASE64_CHARS / 4 * 3 >= limits::MAX_FETCH_REQUEST_BYTES);
            assert!(MAX_BODY_BASE64_CHARS / 4 * 3 < limits::MAX_FETCH_REQUEST_BYTES + 3);
        }
    }

    #[test]
    fn the_hosts_are_what_the_manifest_lists_and_nothing_when_it_does_not_read() {
        let m = serde_json::json!({"internetHosts": ["a.example.com", 7, "b.example.com"]});
        assert_eq!(
            internet_hosts(&m.to_string()),
            ["a.example.com", "b.example.com"]
        );
        for bad in [
            "",
            "not json",
            "{}",
            r#"{"internetHosts": "a.example.com"}"#,
        ] {
            assert!(internet_hosts(bad).is_empty(), "{bad:?}");
        }
    }

    #[test]
    fn every_failure_is_a_fixed_sentence_under_its_own_code() {
        let cases = [
            (
                Failure::Refused(Stop::Url(UrlRefusal::NotAllowed)),
                Code::HostNotAllowed,
            ),
            (
                Failure::Refused(Stop::Url(UrlRefusal::Malformed)),
                Code::BadArgs,
            ),
            (Failure::Refused(Stop::Ended), Code::SessionEnded),
            (Failure::Fetch(FetchError::NotPublic), Code::HostNotPublic),
            (Failure::Fetch(FetchError::Unresolved), Code::HostUnresolved),
            (
                Failure::Fetch(FetchError::Timeout),
                Code::NetworkUnavailable,
            ),
            (
                Failure::Fetch(FetchError::TooManyRedirects),
                Code::TooManyRedirects,
            ),
            (Failure::Fetch(FetchError::TooLarge), Code::ResponseTooLarge),
        ];
        for (f, code) in cases {
            assert_eq!(failure(f).code, code, "{f:?}");
        }
    }
}
