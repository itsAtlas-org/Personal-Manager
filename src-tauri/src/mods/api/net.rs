// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The mod-facing rules of `net.fetch` (Mods spec §1.5, §1.7), all pure: which URL, method,
//! headers and body a mod may send, which of the answer's headers it may see, how the answer is
//! shaped, and which fixed sentence each failure becomes. The fetch itself (redirects, the
//! public-internet screen, the pin, the caps) is `mods/netfetch.rs`; the handler that joins the
//! two with the grant, the secret and the limits is `net_fetch.rs`.
//!
//! **The URL** is parsed once, by the `url` crate (WHATWG: it lowercases the host, turns an
//! internationalised name into punycode, reads `\` as `/`, parses `2130706433` and `0x7f.1` as the
//! IPv4 they are, and drops a default port), and every rule reads the PARSED form, which is also
//! the form that is dialled: what is checked is what is sent. The rules:
//! - `https`, and no other scheme;
//! - no userinfo (`https://user@host/` is how a URL says one thing and dials another);
//! - the default port only, since an explicit one is a way to a service the host's name did not
//!   promise;
//! - a DNS NAME, never an IP literal, that is in the manifest's `internetHosts` exactly: no
//!   wildcards, no suffix match, and `example.com.` (a trailing dot) is not `example.com`;
//! - the fragment is dropped (it is never sent).
//!
//! The same rule judges every redirect target ([`hop_check`]). Whether the name then resolves to
//! the public internet is the screen's job, per hop (`net_screen`).
//!
//! **Headers** follow the fetch standard's forbidden names (the browser owns those), plus the
//! ones PM owns: `User-Agent`, `Accept-Encoding` (PM sends none, so an answer is never compressed
//! and the size cap counts what arrived), `Cookie` and its kin (a mod's fetch has no cookies and PM
//! credentials never ride on it), and the method-override headers. Anything that could split a
//! header line is refused rather than escaped. No refusal echoes what the mod sent (D-25).
//!
//! **What a mod sees of the answer**: the status, an allowlist of headers (never `Set-Cookie`, never
//! a header that names PM's network), and the body as text when it is valid UTF-8 and its JSON form
//! is not huge (escaping can make a 4 MiB body of newlines or NULs several times larger), else
//! base64.
//!
//! **A key** ([`SlotSecret`]) travels with the host its slot is bound to, and [`build_request`]
//! refuses to put it on the first hop of any other listed host: the person approved the key for one
//! destination, and the list may hold several.

use reqwest::Url;
use serde_json::{json, Map, Value};

use super::error::ModError;
use super::limits;
use crate::mods::netfetch::{Failure, FetchError, FetchRequest, Headers, Method, Reply};

/// A URL longer than this is refused unread: it is an exfiltration channel and a parser's work.
pub const MAX_URL_BYTES: usize = 4096;
/// Request headers a mod may set on one fetch.
pub const MAX_REQUEST_HEADERS: usize = 32;
/// A header name, in bytes.
pub const MAX_HEADER_NAME_BYTES: usize = 64;
/// A header value, in bytes.
pub const MAX_HEADER_VALUE_BYTES: usize = 4096;
/// Every request header's name and value together.
pub const MAX_HEADER_BYTES_TOTAL: usize = 16 << 10;
/// A response header's value longer than this is not passed on (cut, it would be a different
/// value; a mod that needs more can read it from the body).
pub const MAX_RESPONSE_HEADER_VALUE_BYTES: usize = 1024;

/// Why a URL was refused. Not worded here: a bad URL is the mod's mistake ([`ModError::bad_args`]),
/// a URL that is well-formed but off the list is a refusal of the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlRefusal {
    /// Not a URL PM can read: not absolute, empty host, control characters, too long.
    Malformed,
    /// A URL, but not one this mod may fetch.
    NotAllowed,
}

/// Whether `url`, already parsed, is one the manifest's `hosts` let a mod fetch.
fn url_allowed(url: &Url, hosts: &[String]) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        // `port()` is None for the scheme's own default, so an explicit `:443` passes and `:8443`,
        // or `:80` on https, does not.
        && url.port().is_none()
        // `domain()` is None for an IP literal, whichever way it was written.
        && url
            .domain()
            .is_some_and(|name| hosts.iter().any(|h| h == name))
}

/// The URL a mod asked for, parsed, normalised and judged against its manifest's hosts.
pub fn parse_url(raw: &str, hosts: &[String]) -> Result<Url, UrlRefusal> {
    // The parser silently strips tabs and newlines and trims surrounding control characters and
    // spaces. A URL that only means what it means after that is refused, not repaired.
    if raw.len() > MAX_URL_BYTES || raw.trim() != raw || raw.chars().any(char::is_control) {
        return Err(UrlRefusal::Malformed);
    }
    let mut url = Url::parse(raw).map_err(|_| UrlRefusal::Malformed)?;
    if !url_allowed(&url, hosts) {
        return Err(UrlRefusal::NotAllowed);
    }
    url.set_fragment(None);
    Ok(url)
}

/// The rule every hop of a fetch is held to, the first request and each redirect: the netfetch
/// loop calls it before it looks anything up. A redirect target that is not a URL a mod may fetch
/// is [`UrlRefusal::NotAllowed`], whatever is wrong with it.
pub fn hop_check(hosts: &[String]) -> impl Fn(&Url) -> Result<(), UrlRefusal> + Send + '_ {
    move |url| {
        if url_allowed(url, hosts) {
            Ok(())
        } else {
            Err(UrlRefusal::NotAllowed)
        }
    }
}

/// The method a mod named. Any case, as `fetch()` takes it; only these six.
pub fn parse_method(raw: &str) -> Result<Method, ModError> {
    match raw.to_ascii_uppercase().as_str() {
        "GET" => Ok(Method::Get),
        "HEAD" => Ok(Method::Head),
        "POST" => Ok(Method::Post),
        "PUT" => Ok(Method::Put),
        "PATCH" => Ok(Method::Patch),
        "DELETE" => Ok(Method::Delete),
        _ => Err(ModError::bad_args("That isn't a method a mod may use.")),
    }
}

/// Request headers a mod may not set, lowercase: the fetch standard's forbidden names, then the
/// ones PM owns (see the module docs).
const FORBIDDEN_HEADERS: &[&str] = &[
    "accept-charset",
    "accept-encoding",
    "access-control-request-headers",
    "access-control-request-method",
    "connection",
    "content-length",
    "cookie",
    "cookie2",
    "date",
    "dnt",
    "expect",
    "host",
    "keep-alive",
    "origin",
    "referer",
    "set-cookie",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "user-agent",
    "via",
    // A server that honours these runs a different method than the one PM allowed.
    "x-http-method",
    "x-http-method-override",
    "x-method-override",
];

/// Prefixes the fetch standard forbids whole.
const FORBIDDEN_HEADER_PREFIXES: &[&str] = &["proxy-", "sec-"];

fn header_name_forbidden(lower: &str) -> bool {
    FORBIDDEN_HEADERS.contains(&lower)
        || FORBIDDEN_HEADER_PREFIXES
            .iter()
            .any(|p| lower.starts_with(p))
}

/// RFC 9110's `token`: the characters a header name may have.
fn is_token(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// A value PM will put on a line: printable ASCII and tab, nothing that could end the line, carry
/// a second header or be read two ways.
fn is_line_safe(value: &str) -> bool {
    value
        .bytes()
        .all(|b| b == b'\t' || (0x20..=0x7e).contains(&b))
}

/// The headers a mod set, checked and normalised (lowercase names, surrounding blanks trimmed from
/// values). `reserved` is the header PM adds itself (a secret's): a mod may not set it, so it can
/// neither pre-empt the secret nor learn whether one exists.
pub fn check_headers(
    raw: &[(String, String)],
    reserved: Option<&str>,
) -> Result<Headers, ModError> {
    let refuse = |why: &'static str| Err(ModError::bad_args(why));
    if raw.len() > MAX_REQUEST_HEADERS {
        return refuse("That request has too many headers.");
    }
    let mut out: Headers = Vec::with_capacity(raw.len());
    let mut total = 0usize;
    for (name, value) in raw {
        if name.len() > MAX_HEADER_NAME_BYTES || !is_token(name) {
            return refuse("A header's name isn't one PM can send.");
        }
        let lower = name.to_ascii_lowercase();
        if header_name_forbidden(&lower) || reserved.is_some_and(|r| r.eq_ignore_ascii_case(&lower))
        {
            return refuse("A mod can't set one of those headers.");
        }
        let value = value.trim_matches(|c| c == ' ' || c == '\t');
        if value.len() > MAX_HEADER_VALUE_BYTES || !is_line_safe(value) {
            return refuse("A header's value isn't one PM can send.");
        }
        if out.iter().any(|(n, _)| *n == lower) {
            return refuse("A header is set twice.");
        }
        total += lower.len() + value.len();
        if total > MAX_HEADER_BYTES_TOTAL {
            return refuse("That request's headers are too large.");
        }
        out.push((lower, value.to_owned()));
    }
    Ok(out)
}

/// The header PM adds from a stored secret: its manifest slot's name and the value the user typed.
/// A slot name the filter would refuse, or a value that cannot go on a header line, is as good as
/// no key: [`ModError::secret_not_set`], so the stored text is never judged out loud.
pub fn secret_header(name: &str, value: &str) -> Result<(String, String), ModError> {
    let lower = name.to_ascii_lowercase();
    if !is_token(&lower)
        || header_name_forbidden(&lower)
        || value.is_empty()
        || value.len() > MAX_HEADER_VALUE_BYTES
        || value != value.trim_matches(|c| c == ' ' || c == '\t')
        || !is_line_safe(value)
    {
        return Err(ModError::secret_not_set());
    }
    Ok((lower, value.to_owned()))
}

/// A body: only on a method that takes one, and within the request cap.
pub fn check_body(method: Method, body: Option<&[u8]>) -> Result<(), ModError> {
    match body {
        None => Ok(()),
        Some(_) if !method.takes_body() => Err(ModError::bad_args(
            "That method can't carry a request body.",
        )),
        Some(b) if b.len() > limits::MAX_FETCH_REQUEST_BYTES => {
            Err(ModError::bad_args("That request's body is too large."))
        }
        Some(_) => Ok(()),
    }
}

/// A stored key ready for one request: the host its slot is bound to, the header it goes in, the
/// value. The host travels WITH the key so [`build_request`] can hold the first hop to it: a
/// manifest may list several `internetHosts`, but the person approved the key for one.
pub struct SlotSecret {
    host: String,
    header: String,
    value: String,
}

// By hand: a derived Debug would print the key.
impl std::fmt::Debug for SlotSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlotSecret")
            .field("host", &self.host)
            .field("header", &self.header)
            .finish_non_exhaustive()
    }
}

/// The key of a slot `host`/`name` holding `value`, as [`secret_header`] judges it.
pub fn slot_secret(host: &str, name: &str, value: &str) -> Result<SlotSecret, ModError> {
    let (header, value) = secret_header(name, value)?;
    Ok(SlotSecret {
        host: host.to_owned(),
        header,
        value,
    })
}

/// A mod's request, every part checked: ready for `netfetch::fetch` with [`hop_check`] and
/// `Policy::for_mods()`. `secret` goes on the first hop only (D-7), and only when the URL's host IS
/// the slot's host: a key sent to another listed host is refused here, before anything is dialled,
/// whatever the handler did or did not check.
pub fn build_request(
    url: &str,
    method: &str,
    headers: &[(String, String)],
    body: Option<Vec<u8>>,
    hosts: &[String],
    secret: Option<SlotSecret>,
) -> Result<FetchRequest, ModError> {
    let method = parse_method(method)?;
    let url = parse_url(url, hosts).map_err(url_error)?;
    if let Some(s) = &secret {
        // `parse_url` passed, so `domain()` is a name; compared exactly, as the list is.
        if url.domain() != Some(s.host.as_str()) {
            return Err(ModError::host_not_allowed());
        }
    }
    let headers = check_headers(headers, secret.as_ref().map(|s| s.header.as_str()))?;
    check_body(method, body.as_deref())?;
    Ok(FetchRequest {
        method,
        url,
        headers,
        first_hop_headers: secret.into_iter().map(|s| (s.header, s.value)).collect(),
        body,
    })
}

/// A URL refusal, as the fixed sentence a mod gets.
pub fn url_error(refusal: UrlRefusal) -> ModError {
    match refusal {
        UrlRefusal::Malformed => ModError::bad_args("That isn't a web address PM can read."),
        UrlRefusal::NotAllowed => ModError::host_not_allowed(),
    }
}

/// A failed fetch, as the fixed sentence a mod gets. Never the network's own words, a URL or an
/// address (D-25).
pub fn failure_error(failure: Failure<UrlRefusal>) -> ModError {
    match failure {
        Failure::Refused(refusal) => url_error(refusal),
        Failure::Fetch(e) => fetch_error(e),
    }
}

pub fn fetch_error(e: FetchError) -> ModError {
    match e {
        FetchError::BadUrl => ModError::host_not_allowed(),
        FetchError::BadRequest => ModError::bad_args("That request can't be sent."),
        FetchError::NotPublic => ModError::host_not_public(),
        FetchError::Unresolved => ModError::host_unresolved(),
        // A server that stalls, refuses, resets, fails TLS or sends a redirect with nowhere to go
        // is, to a mod, a website PM could not reach.
        FetchError::Unavailable | FetchError::Timeout | FetchError::BadRedirect => {
            ModError::network_unavailable()
        }
        FetchError::TooManyRedirects => ModError::too_many_redirects(),
        FetchError::TooLarge => ModError::response_too_large(),
    }
}

/// The response headers a mod may see, lowercase, in this order. Not `Set-Cookie` (a mod's fetch
/// has no cookie jar, and a session cookie is the user's), not `Content-Length` (it describes the
/// wire, not the text a mod receives), not `Location`, `Server` or anything else that is about the
/// network rather than the answer.
pub const PUBLIC_RESPONSE_HEADERS: &[&str] = &[
    "cache-control",
    "content-language",
    "content-type",
    "etag",
    "expires",
    "last-modified",
    "ratelimit-limit",
    "ratelimit-remaining",
    "ratelimit-reset",
    "retry-after",
    "x-ratelimit-limit",
    "x-ratelimit-remaining",
    "x-ratelimit-reset",
];

/// The allowlisted headers of an answer: a header sent twice is joined with `, ` (as `fetch`
/// does); one whose value is too long, or is empty, is left out.
pub fn public_headers(headers: &[(String, String)]) -> Vec<(String, String)> {
    PUBLIC_RESPONSE_HEADERS
        .iter()
        .filter_map(|name| {
            let values: Vec<&str> = headers
                .iter()
                .filter(|(n, _)| n == name)
                .map(|(_, v)| v.trim())
                .collect();
            if values.is_empty() {
                return None;
            }
            let joined = values.join(", ");
            let ok = !joined.is_empty()
                && joined.len() <= MAX_RESPONSE_HEADER_VALUE_BYTES
                && is_line_safe(&joined);
            ok.then(|| ((*name).to_owned(), joined))
        })
        .collect()
}

/// An answer's body for the frame: text when it is valid UTF-8, else base64.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    Text(String),
    Base64(String),
}

/// What `text` weighs as a JSON string (its quotes not counted), as `serde_json` writes it: `"`,
/// `\` and the control characters are escaped (two bytes, or six for most controls), nothing else
/// is. Stops counting once past `cap`, so a hostile body costs one pass and no allocation.
fn json_string_len(text: &str, cap: usize) -> usize {
    let mut n = 0usize;
    for c in text.chars() {
        n += match c {
            '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
            c if c < ' ' => 6,
            c => c.len_utf8(),
        };
        if n > cap {
            break;
        }
    }
    n
}

/// Text when the body is valid UTF-8 AND its JSON form stays within
/// [`limits::MAX_FETCH_TEXT_JSON_BYTES`]; else base64. The second condition keeps a body that was
/// within the fetch cap from becoming an answer too large to send, because escaping (`\u0000` is
/// six bytes for one) can grow text far past its length, while base64 is a fixed four for three.
pub fn encode_body(bytes: Vec<u8>) -> Body {
    use base64::{engine::general_purpose::STANDARD, Engine};
    match String::from_utf8(bytes) {
        Ok(text)
            if json_string_len(&text, limits::MAX_FETCH_TEXT_JSON_BYTES)
                <= limits::MAX_FETCH_TEXT_JSON_BYTES =>
        {
            Body::Text(text)
        }
        Ok(text) => Body::Base64(STANDARD.encode(text.into_bytes())),
        Err(e) => Body::Base64(STANDARD.encode(e.into_bytes())),
    }
}

/// The value `net.fetch` answers with: `{status, headers, bodyText | bodyBase64}`. The final URL is
/// not in it (the mod knows where it asked, and a redirect target is none of its business).
pub fn reply_value(reply: Reply) -> Value {
    let headers: Map<String, Value> = public_headers(&reply.headers)
        .into_iter()
        .map(|(n, v)| (n, Value::String(v)))
        .collect();
    let mut out = json!({ "status": reply.status, "headers": headers });
    match encode_body(reply.body) {
        Body::Text(t) => out["bodyText"] = Value::String(t),
        Body::Base64(b) => out["bodyBase64"] = Value::String(b),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::api::Code;

    fn hosts() -> Vec<String> {
        vec!["api.example.com".into(), "cdn.example.com".into()]
    }

    fn url(raw: &str) -> Result<Url, UrlRefusal> {
        parse_url(raw, &hosts())
    }

    fn pairs<N: AsRef<str>, V: AsRef<str>>(list: &[(N, V)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(n, v)| (n.as_ref().to_owned(), v.as_ref().to_owned()))
            .collect()
    }

    fn bad_args<T: std::fmt::Debug>(r: Result<T, ModError>) -> ModError {
        let e = r.expect_err("should be refused");
        assert_eq!(e.code, Code::BadArgs, "{e:?}");
        e
    }

    // ---- the URL -------------------------------------------------------------------------------

    #[test]
    fn a_listed_https_url_passes_in_its_normal_form() {
        for (raw, want) in [
            (
                "https://api.example.com/x?y=1",
                "https://api.example.com/x?y=1",
            ),
            ("https://api.example.com", "https://api.example.com/"),
            (
                "https://cdn.example.com/a/b.png",
                "https://cdn.example.com/a/b.png",
            ),
            // The host is lowercased, an explicit default port dropped, the fragment never sent.
            (
                "HTTPS://API.EXAMPLE.COM/Path",
                "https://api.example.com/Path",
            ),
            ("https://api.example.com:443/", "https://api.example.com/"),
            (
                "https://api.example.com/x#frag",
                "https://api.example.com/x",
            ),
            // `\` is a `/` to the parser, so this dials api.example.com (path `/@evil.com/`).
            (
                "https://api.example.com\\@evil.com/",
                "https://api.example.com/@evil.com/",
            ),
            // A fullwidth letter is the ASCII letter after IDNA: what is dialled is what is listed.
            (
                "https://\u{ff41}pi.example.com/",
                "https://api.example.com/",
            ),
        ] {
            assert_eq!(url(raw).unwrap().as_str(), want, "{raw}");
        }
    }

    #[track_caller]
    fn assert_refused(refusal: UrlRefusal, list: &[&str]) {
        for raw in list {
            assert_eq!(url(raw), Err(refusal), "{raw:?}");
        }
    }

    #[test]
    fn only_https_is_allowed() {
        assert_refused(
            UrlRefusal::NotAllowed,
            &[
                "http://api.example.com/",
                "ftp://api.example.com/",
                "wss://api.example.com/",
                "file:///etc/passwd",
                "javascript:alert(1)",
                "data:text/plain,hi",
            ],
        );
    }

    #[test]
    fn only_the_default_port_is_allowed() {
        assert_refused(
            UrlRefusal::NotAllowed,
            &[
                "https://api.example.com:8443/",
                "https://api.example.com:80/",
                "https://api.example.com:444/",
            ],
        );
    }

    #[test]
    fn userinfo_is_refused_on_either_side_of_the_host() {
        assert_refused(
            UrlRefusal::NotAllowed,
            &[
                "https://user@api.example.com/",
                "https://user:pw@api.example.com/",
                "https://:pw@api.example.com/",
                "https://api.example.com@evil.com/",
                "https://api.example.com:443@evil.com/",
            ],
        );
    }

    #[test]
    fn only_a_listed_name_exactly_is_allowed() {
        assert_refused(
            UrlRefusal::NotAllowed,
            &[
                "https://evil.com/",
                "https://example.com/",
                "https://sub.api.example.com/",
                "https://api.example.com.evil.com/",
                "https://api.example.com%2eevil.com/",
                "https://xapi.example.com/",
                "https://api.example.com./",
                "https://API.EXAMPLE.COM.:443/",
                // extra slashes make the next word the host
                "https:///evil.com/",
            ],
        );
    }

    #[test]
    fn an_ip_literal_is_never_a_listed_name_however_it_is_written() {
        assert_refused(
            UrlRefusal::NotAllowed,
            &[
                "https://127.0.0.1/",
                "https://2130706433/",
                "https://0x7f.1/",
                "https://017700000001/",
                "https://[::1]/",
                "https://[::ffff:127.0.0.1]/",
                "https://[::ffff:7f00:1]/",
                "https://[64:ff9b::7f00:1]/",
                "https://[2002:7f00:1::1]/",
                "https://169.254.169.254/latest/meta-data",
                "https://localhost/",
            ],
        );
    }

    #[test]
    fn something_that_is_not_an_absolute_url_is_malformed() {
        assert_refused(
            UrlRefusal::Malformed,
            &[
                "",
                " ",
                "not a url",
                "api.example.com",
                "api.example.com/x",
                "//api.example.com/x",
                "/x",
                "https://",
            ],
        );
    }

    #[test]
    fn surrounding_blanks_are_refused_not_trimmed() {
        assert_refused(
            UrlRefusal::Malformed,
            &[
                " https://api.example.com/",
                "https://api.example.com/ ",
                "\thttps://api.example.com/",
                "https://api.example.com/\n",
            ],
        );
    }

    #[test]
    fn control_characters_are_refused_not_stripped() {
        assert_refused(
            UrlRefusal::Malformed,
            &[
                "https://api.exa\nmple.com/",
                "https://api.exa\tmple.com/",
                "ht\ttps://api.example.com/",
                "https://api.example.com/\r\nHost: evil.com",
                "https://api.example.com/\0",
                "https://api.example.com/\u{7f}",
                "https://api.example.com/\u{85}",
            ],
        );
    }

    #[test]
    fn a_url_is_capped_in_bytes_and_one_at_the_cap_is_fine() {
        let too_long = format!("https://api.example.com/{}", "a".repeat(MAX_URL_BYTES));
        assert_refused(UrlRefusal::Malformed, &[too_long.as_str()]);
        let base = "https://api.example.com/";
        let at_cap = format!("{base}{}", "a".repeat(MAX_URL_BYTES - base.len()));
        assert!(url(&at_cap).is_ok());
        let one_over = format!("{at_cap}a");
        assert_refused(UrlRefusal::Malformed, &[one_over.as_str()]);
    }

    #[test]
    fn spaces_and_non_ascii_inside_a_url_are_encoded_not_refused() {
        let u = url("https://api.example.com/a b?q=\u{e9}").unwrap();
        assert_eq!(u.as_str(), "https://api.example.com/a%20b?q=%C3%A9");
    }

    #[test]
    fn no_listed_hosts_means_nothing_is_allowed() {
        assert_eq!(
            parse_url("https://api.example.com/", &[]),
            Err(UrlRefusal::NotAllowed)
        );
    }

    #[test]
    fn every_redirect_target_is_held_to_the_same_rule() {
        let hosts = hosts();
        let check = hop_check(&hosts);
        let ok = |raw: &str| check(&Url::parse(raw).unwrap());
        assert_eq!(ok("https://api.example.com/next"), Ok(()));
        assert_eq!(ok("https://cdn.example.com/x?y"), Ok(()));
        for raw in [
            "https://evil.com/",
            "http://api.example.com/",
            "https://api.example.com:8443/",
            "https://user@api.example.com/",
            "https://api.example.com./",
            "https://127.0.0.1/",
            "https://[::ffff:127.0.0.1]/",
            "https://[64:ff9b::7f00:1]/",
        ] {
            assert_eq!(ok(raw), Err(UrlRefusal::NotAllowed), "{raw}");
        }
    }

    // ---- the method ----------------------------------------------------------------------------

    #[test]
    fn six_methods_in_any_case_and_nothing_else() {
        for (raw, want) in [
            ("GET", Method::Get),
            ("get", Method::Get),
            ("Head", Method::Head),
            ("POST", Method::Post),
            ("put", Method::Put),
            ("pAtCh", Method::Patch),
            ("DELETE", Method::Delete),
        ] {
            assert_eq!(parse_method(raw).unwrap(), want, "{raw}");
        }
        for raw in [
            "", "CONNECT", "TRACE", "TRACK", "OPTIONS", "GE T", "GET ", " GET", "GET\n", "FETCH",
            "PROPFIND", "get,post",
        ] {
            bad_args(parse_method(raw));
        }
    }

    #[test]
    fn a_body_needs_a_method_that_takes_one_and_a_size_within_the_cap() {
        let cap = limits::MAX_FETCH_REQUEST_BYTES;
        for m in [Method::Post, Method::Put, Method::Patch, Method::Delete] {
            assert!(check_body(m, None).is_ok());
            assert!(check_body(m, Some(b"x")).is_ok());
            assert!(check_body(m, Some(b"")).is_ok());
            assert!(check_body(m, Some(&vec![0; cap])).is_ok());
            bad_args(check_body(m, Some(&vec![0; cap + 1])));
        }
        for m in [Method::Get, Method::Head] {
            assert!(check_body(m, None).is_ok());
            bad_args(check_body(m, Some(b"x")));
            bad_args(check_body(m, Some(b"")));
        }
    }

    // ---- request headers -----------------------------------------------------------------------

    #[test]
    fn ordinary_headers_pass_lowercased_and_trimmed() {
        let got = check_headers(
            &pairs(&[
                ("Authorization", "  Bearer abc "),
                ("Accept", "application/json"),
                ("Content-Type", "text/plain; charset=utf-8"),
                ("X-Api-Version", "2"),
                ("If-None-Match", "\"v1\""),
                ("Range", "bytes=0-9"),
                ("x_under.score", "tab\tinside"),
                ("empty", ""),
            ]),
            None,
        )
        .unwrap();
        assert_eq!(
            got,
            pairs(&[
                ("authorization", "Bearer abc"),
                ("accept", "application/json"),
                ("content-type", "text/plain; charset=utf-8"),
                ("x-api-version", "2"),
                ("if-none-match", "\"v1\""),
                ("range", "bytes=0-9"),
                ("x_under.score", "tab\tinside"),
                ("empty", ""),
            ])
        );
        assert!(check_headers(&[], None).unwrap().is_empty());
    }

    #[test]
    fn the_names_a_browser_owns_and_the_names_pm_owns_are_refused_in_any_case() {
        for name in [
            "HOST",
            "cOnTeNt-LeNgTh",
            "Accept-Charset",
            "Accept-Encoding",
            "Access-Control-Request-Headers",
            "Access-Control-Request-Method",
            "Connection",
            "Content-Length",
            "Cookie",
            "Cookie2",
            "Date",
            "DNT",
            "Expect",
            "Host",
            "Keep-Alive",
            "Origin",
            "Referer",
            "Set-Cookie",
            "TE",
            "Trailer",
            "Transfer-Encoding",
            "Upgrade",
            "User-Agent",
            "Via",
        ] {
            bad_args(check_headers(&pairs(&[(name, "v")]), None));
        }
    }

    #[test]
    fn the_proxy_and_sec_families_are_refused_whole() {
        for name in [
            "Proxy-Authorization",
            "Proxy-Authenticate",
            "Proxy-Connection",
            "Proxy-Anything",
            "Sec-Fetch-Mode",
            "sec-ch-ua",
            "SEC-WEBSOCKET-KEY",
        ] {
            bad_args(check_headers(&pairs(&[(name, "v")]), None));
        }
        // A name that only contains one of the prefixes is an ordinary header.
        assert!(check_headers(&pairs(&[("x-proxy-id", "1"), ("second", "2")]), None).is_ok());
    }

    #[test]
    fn the_method_override_headers_are_refused() {
        for name in [
            "X-HTTP-Method",
            "X-HTTP-Method-Override",
            "X-Method-Override",
        ] {
            bad_args(check_headers(&pairs(&[(name, "DELETE")]), None));
        }
    }

    #[test]
    fn every_header_a_manifest_forbids_a_secret_in_is_one_a_mod_may_not_set() {
        // The manifest's own list (package.rs) and this filter must not drift apart.
        for name in crate::mods::package::FORBIDDEN_SECRET_HEADERS {
            bad_args(check_headers(&pairs(&[(name, "v")]), None));
            assert_eq!(
                secret_header(name, "value").unwrap_err().code,
                Code::SecretNotSet,
                "{name}"
            );
        }
    }

    #[test]
    fn a_header_name_must_be_a_token() {
        for name in [
            "",
            "bad name",
            "bad:name",
            "bad\nname",
            "n\u{e4}me",
            "a,b",
            "a;b",
            "a/b",
        ] {
            bad_args(check_headers(&pairs(&[(name, "v")]), None));
        }
    }

    #[test]
    fn a_header_value_must_be_one_safe_line() {
        for value in [
            "line\r\nX-Evil: 1",
            "line\nbreak",
            "carriage\rreturn",
            "nul\0byte",
            "del\u{7f}",
            "caf\u{e9}",
            "esc\u{1b}[0m",
        ] {
            bad_args(check_headers(&pairs(&[("name", value)]), None));
        }
    }

    #[test]
    fn the_number_of_headers_is_capped() {
        let many: Vec<(String, String)> = (0..=MAX_REQUEST_HEADERS)
            .map(|i| (format!("x-{i}"), "v".into()))
            .collect();
        bad_args(check_headers(&many, None));
        assert!(check_headers(&many[..MAX_REQUEST_HEADERS], None).is_ok());
    }

    #[test]
    fn a_header_names_size_is_capped() {
        let long_name = "n".repeat(MAX_HEADER_NAME_BYTES + 1);
        bad_args(check_headers(&pairs(&[(&long_name, "v")]), None));
        let name_at_cap = "n".repeat(MAX_HEADER_NAME_BYTES);
        assert!(check_headers(&pairs(&[(&name_at_cap, "v")]), None).is_ok());
    }

    #[test]
    fn a_header_values_size_is_capped() {
        let long_value = "v".repeat(MAX_HEADER_VALUE_BYTES + 1);
        bad_args(check_headers(&pairs(&[("a", &long_value)]), None));
        let value_at_cap = "v".repeat(MAX_HEADER_VALUE_BYTES);
        assert!(check_headers(&pairs(&[("a", &value_at_cap)]), None).is_ok());
    }

    #[test]
    fn all_the_headers_together_are_capped() {
        let value_at_cap = "v".repeat(MAX_HEADER_VALUE_BYTES);
        // Three of the largest value fit under the total; five are past it though each header is
        // within its own caps.
        let three: Vec<(String, String)> = (0..3)
            .map(|i| (format!("x-{i}"), value_at_cap.clone()))
            .collect();
        assert!(check_headers(&three, None).is_ok());
        let five: Vec<(String, String)> = (0..5)
            .map(|i| (format!("x-{i}"), value_at_cap.clone()))
            .collect();
        bad_args(check_headers(&five, None));
    }

    #[test]
    fn the_same_header_twice_is_refused_in_any_case() {
        bad_args(check_headers(&pairs(&[("a", "1"), ("A", "2")]), None));
        bad_args(check_headers(&pairs(&[("a", "1"), ("a", "1")]), None));
        assert!(check_headers(&pairs(&[("a", "1"), ("b", "1")]), None).is_ok());
    }

    #[test]
    fn a_mod_cannot_set_the_header_pm_adds_for_its_secret() {
        for clash in ["x-api-key", "X-Api-Key"] {
            bad_args(check_headers(&pairs(&[(clash, "mine")]), Some("x-api-key")));
        }
        // Without a secret in play the same header is an ordinary one.
        assert!(check_headers(&pairs(&[("x-api-key", "mine")]), None).is_ok());
        // And another header is untouched by the reservation.
        assert!(check_headers(&pairs(&[("x-other", "1")]), Some("x-api-key")).is_ok());
    }

    #[test]
    fn a_refusal_never_echoes_what_the_mod_sent() {
        let probe = "ZZSECRETZZ";
        let bad_name = format!("{probe} bad");
        let bad_value = format!("{probe}\n");
        let errs = [
            bad_args(check_headers(&pairs(&[(&bad_name, "v")]), None)),
            bad_args(check_headers(&pairs(&[("cookie", probe)]), None)),
            bad_args(check_headers(&pairs(&[("x", &bad_value)]), None)),
            bad_args(check_headers(&pairs(&[("a", probe), ("a", probe)]), None)),
            bad_args(parse_method(probe)),
            url_error(parse_url(probe, &hosts()).unwrap_err()),
            url_error(parse_url(&format!("https://{probe}.com/"), &hosts()).unwrap_err()),
        ];
        for e in errs {
            assert!(!e.message.contains(probe), "{e:?}");
        }
    }

    // ---- the secret's header -------------------------------------------------------------------

    #[test]
    fn a_secret_header_is_the_slots_name_and_the_typed_value() {
        assert_eq!(
            secret_header("X-Api-Key", "abc123").unwrap(),
            ("x-api-key".to_owned(), "abc123".to_owned())
        );
        assert_eq!(
            secret_header("Authorization", "Bearer abc").unwrap().0,
            "authorization"
        );
    }

    #[test]
    fn a_stored_value_that_cannot_go_on_a_header_line_counts_as_no_key() {
        for value in [
            "",
            " leading",
            "trailing ",
            "line\r\nX-Evil: 1",
            "nl\n",
            "nul\0",
            "caf\u{e9}",
            &"v".repeat(MAX_HEADER_VALUE_BYTES + 1),
        ] {
            let e = secret_header("x-api-key", value).unwrap_err();
            assert_eq!(e.code, Code::SecretNotSet, "{value:?}");
        }
        for name in ["", "bad name", "cookie", "host", "proxy-x", "user-agent"] {
            assert_eq!(
                secret_header(name, "v").unwrap_err().code,
                Code::SecretNotSet,
                "{name}"
            );
        }
        assert!(secret_header("x-api-key", &"v".repeat(MAX_HEADER_VALUE_BYTES)).is_ok());
    }

    // ---- the whole request ---------------------------------------------------------------------

    #[test]
    fn a_request_is_built_checked_with_the_secret_on_the_first_hop_only() {
        let declared = slot_secret("api.example.com", "X-Api-Key", "abc123").unwrap();
        let req = build_request(
            "https://api.example.com/v1/items?limit=2#x",
            "post",
            &pairs(&[("Content-Type", "application/json")]),
            Some(b"{}".to_vec()),
            &hosts(),
            Some(declared),
        )
        .unwrap();
        assert_eq!(req.method, Method::Post);
        assert_eq!(req.url.as_str(), "https://api.example.com/v1/items?limit=2");
        assert_eq!(req.headers, pairs(&[("content-type", "application/json")]));
        assert_eq!(req.first_hop_headers, pairs(&[("x-api-key", "abc123")]));
        assert_eq!(req.body.as_deref(), Some(&b"{}"[..]));

        let plain = build_request(
            "https://cdn.example.com/a",
            "GET",
            &[],
            None,
            &hosts(),
            None,
        )
        .unwrap();
        assert!(plain.first_hop_headers.is_empty() && plain.body.is_none());
    }

    #[test]
    fn a_request_with_any_part_refused_is_refused_with_the_right_code() {
        let ok = |url: &str, method: &str, headers: &[(&str, &str)], body: Option<&[u8]>| {
            build_request(
                url,
                method,
                &pairs(headers),
                body.map(<[u8]>::to_vec),
                &hosts(),
                None,
            )
        };
        assert_eq!(
            ok("https://evil.com/", "GET", &[], None).unwrap_err().code,
            Code::HostNotAllowed
        );
        assert_eq!(
            ok("nonsense", "GET", &[], None).unwrap_err().code,
            Code::BadArgs
        );
        assert_eq!(
            ok("https://api.example.com/", "TRACE", &[], None)
                .unwrap_err()
                .code,
            Code::BadArgs
        );
        assert_eq!(
            ok("https://api.example.com/", "GET", &[("host", "x")], None)
                .unwrap_err()
                .code,
            Code::BadArgs
        );
        assert_eq!(
            ok("https://api.example.com/", "GET", &[], Some(b"x"))
                .unwrap_err()
                .code,
            Code::BadArgs
        );
        // The header PM adds for the secret can't be set by the mod.
        let with_secret = build_request(
            "https://api.example.com/",
            "GET",
            &pairs(&[("X-API-KEY", "mine")]),
            None,
            &hosts(),
            Some(slot_secret("api.example.com", "x-api-key", "real").unwrap()),
        );
        assert_eq!(with_secret.unwrap_err().code, Code::BadArgs);
    }

    // ---- what comes back -----------------------------------------------------------------------

    #[test]
    fn a_mod_sees_only_the_allowlisted_response_headers() {
        let got = public_headers(&pairs(&[
            ("content-type", "application/json"),
            ("set-cookie", "session=abc; HttpOnly"),
            ("set-cookie2", "x=1"),
            ("content-length", "10"),
            ("server", "nginx"),
            ("location", "https://evil.example/"),
            ("x-powered-by", "PHP"),
            ("strict-transport-security", "max-age=1"),
            ("etag", "\"abc\""),
            ("retry-after", "30"),
            ("x-ratelimit-remaining", "7"),
            ("www-authenticate", "Bearer"),
            ("authorization", "echoed"),
        ]));
        assert_eq!(
            got,
            pairs(&[
                ("content-type", "application/json"),
                ("etag", "\"abc\""),
                ("retry-after", "30"),
                ("x-ratelimit-remaining", "7"),
            ])
        );
        for never in ["set-cookie", "set-cookie2", "authorization"] {
            assert!(!PUBLIC_RESPONSE_HEADERS.contains(&never));
        }
    }

    #[test]
    fn a_repeated_response_header_is_joined_and_an_overlong_one_is_left_out() {
        let too_long = "e".repeat(MAX_RESPONSE_HEADER_VALUE_BYTES + 1);
        let at_cap = "l".repeat(MAX_RESPONSE_HEADER_VALUE_BYTES);
        let got = public_headers(&pairs(&[
            ("cache-control", "no-cache"),
            ("cache-control", "no-store"),
            ("etag", too_long.as_str()),
            ("expires", ""),
            ("last-modified", at_cap.as_str()),
        ]));
        assert_eq!(
            got[0],
            ("cache-control".into(), "no-cache, no-store".into())
        );
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[1].0, "last-modified");
    }

    #[test]
    fn a_body_is_text_when_it_is_utf8_and_base64_when_it_is_not() {
        assert_eq!(
            encode_body(b"hello \xc3\xa9".to_vec()),
            Body::Text("hello \u{e9}".into())
        );
        assert_eq!(encode_body(vec![]), Body::Text(String::new()));
        // A NUL is valid UTF-8 and a valid JSON string.
        assert_eq!(encode_body(b"a\0b".to_vec()), Body::Text("a\0b".into()));
        assert_eq!(
            encode_body(vec![0xff, 0xfe, 0x00]),
            Body::Base64("//4A".into())
        );
        // A truncated multi-byte sequence is not text.
        assert!(matches!(encode_body(vec![b'a', 0xc3]), Body::Base64(_)));
    }

    #[test]
    fn the_reply_has_a_status_headers_and_one_body_field_and_no_url() {
        let reply = |body: &[u8]| Reply {
            status: 201,
            headers: pairs(&[("content-type", "text/plain"), ("set-cookie", "s=1")]),
            body: body.to_vec(),
            url: Url::parse("https://cdn.example.com/after-redirect").unwrap(),
        };
        let text = reply_value(reply(b"hi"));
        assert_eq!(
            text,
            json!({"status": 201, "headers": {"content-type": "text/plain"}, "bodyText": "hi"})
        );
        let binary = reply_value(reply(&[0xff, 0x00]));
        assert_eq!(binary["bodyBase64"], "/wA=");
        assert!(binary.get("bodyText").is_none());
        assert!(!text.to_string().contains("after-redirect"));
    }

    #[test]
    fn the_largest_answer_still_fits_in_a_call_result() {
        // Worst case: the whole cap as base64, every allowlisted header at its largest.
        let worst = reply_value(Reply {
            status: 200,
            headers: PUBLIC_RESPONSE_HEADERS
                .iter()
                .map(|n| ((*n).to_owned(), "h".repeat(MAX_RESPONSE_HEADER_VALUE_BYTES)))
                .collect(),
            body: vec![0xff; limits::MAX_FETCH_RESPONSE_BYTES],
            url: Url::parse("https://cdn.example.com/").unwrap(),
        });
        assert!(worst.to_string().len() < limits::MAX_RESULT_BYTES);
    }

    /// The same, for text: valid UTF-8 whose JSON form is far longer than the bytes (newlines are
    /// two, a NUL is six). It must go as base64 and fit, not be sent as text and refused as too
    /// large after the whole fetch ran.
    #[test]
    fn a_largest_text_body_that_escapes_long_is_sent_as_base64_and_fits() {
        let headers: Vec<(String, String)> = PUBLIC_RESPONSE_HEADERS
            .iter()
            .map(|n| ((*n).to_owned(), "h".repeat(MAX_RESPONSE_HEADER_VALUE_BYTES)))
            .collect();
        for unit in ["\n", "\"", "\0", "\u{1}", "\\"] {
            let body = unit.repeat(limits::MAX_FETCH_RESPONSE_BYTES).into_bytes();
            assert_eq!(body.len(), limits::MAX_FETCH_RESPONSE_BYTES);
            let out = reply_value(Reply {
                status: 200,
                headers: headers.clone(),
                body: body.clone(),
                url: Url::parse("https://cdn.example.com/").unwrap(),
            });
            assert!(out.get("bodyText").is_none(), "{unit:?}");
            assert!(out.to_string().len() < limits::MAX_RESULT_BYTES, "{unit:?}");
            // Nothing is lost: the base64 is the body.
            use base64::{engine::general_purpose::STANDARD, Engine};
            let back = STANDARD
                .decode(out["bodyBase64"].as_str().unwrap())
                .unwrap();
            assert!(back == body, "{unit:?}");
        }
        // Text whose JSON form is within the budget stays text, including the biggest plain body,
        // and its counted size is the size serde_json writes.
        for sample in ["plain", "a\"b\\c\nd\te\r\u{8}\u{c}\u{1f}\u{7f}é€😀"] {
            assert_eq!(
                json_string_len(sample, usize::MAX),
                serde_json::to_string(sample).unwrap().len() - 2,
                "{sample:?}"
            );
        }
        let plain = vec![b'a'; limits::MAX_FETCH_RESPONSE_BYTES];
        assert!(matches!(encode_body(plain), Body::Text(_)));
    }

    // ---- failures, as the sentences a mod gets -------------------------------------------------

    #[test]
    fn every_failure_is_one_code_and_one_fixed_sentence() {
        for (e, code) in [
            (FetchError::BadUrl, Code::HostNotAllowed),
            (FetchError::BadRequest, Code::BadArgs),
            (FetchError::NotPublic, Code::HostNotPublic),
            (FetchError::Unresolved, Code::HostUnresolved),
            (FetchError::Unavailable, Code::NetworkUnavailable),
            (FetchError::Timeout, Code::NetworkUnavailable),
            (FetchError::BadRedirect, Code::NetworkUnavailable),
            (FetchError::TooManyRedirects, Code::TooManyRedirects),
            (FetchError::TooLarge, Code::ResponseTooLarge),
        ] {
            let got = fetch_error(e);
            assert_eq!(got.code, code, "{e:?}");
            assert_eq!(failure_error(Failure::Fetch(e)), got);
        }
        assert_eq!(
            failure_error(Failure::Refused(UrlRefusal::NotAllowed)).code,
            Code::HostNotAllowed
        );
        assert_eq!(
            failure_error(Failure::Refused(UrlRefusal::Malformed)).code,
            Code::BadArgs
        );
    }

    // ---- the whole pure core, through a host's scripted network --------------------------------

    use crate::mods::api::test_support::TestHost;
    use crate::mods::api::Host;
    use crate::mods::netfetch::{self, fake::reply as scripted, Policy};

    /// What X6's handler will do with these pieces, minus the grant, the limits and the secret's
    /// lookup: build the request, fetch it through the host's network, shape the answer.
    async fn mod_fetch(
        host: &TestHost,
        url: &str,
        secret: Option<SlotSecret>,
    ) -> Result<Value, ModError> {
        let hosts = hosts();
        let request = build_request(url, "GET", &[], None, &hosts, secret)?;
        let reply = netfetch::fetch(host.hops(), request, &Policy::for_mods(), hop_check(&hosts))
            .await
            .map_err(failure_error)?;
        Ok(reply_value(reply))
    }

    fn scripted_host() -> TestHost {
        let host = TestHost::new();
        host.hops()
            .dns("api.example.com", &["93.184.216.34"])
            .dns("cdn.example.com", &["93.184.216.35"]);
        host
    }

    #[tokio::test]
    async fn a_listed_host_is_fetched_pinned_and_answered_without_its_cookies() {
        let host = scripted_host();
        host.hops().answer(|_| {
            Ok(scripted(
                200,
                &[("Content-Type", "application/json"), ("Set-Cookie", "s=1")],
                b"{\"ok\":true}",
            ))
        });
        let got = mod_fetch(&host, "https://api.example.com/v1/x", None)
            .await
            .unwrap();
        assert_eq!(
            got,
            json!({"status": 200, "headers": {"content-type": "application/json"},
                   "bodyText": "{\"ok\":true}"})
        );
        let seen = host.hops().seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].pinned, vec!["93.184.216.34:443".parse().unwrap()]);
    }

    #[tokio::test]
    async fn the_secret_reaches_the_listed_host_once_and_no_redirect_gets_it() {
        let host = scripted_host();
        host.hops().answer(|s| {
            if s.url.contains("api.example.com") {
                Ok(scripted(
                    302,
                    &[("Location", "https://cdn.example.com/f")],
                    b"",
                ))
            } else {
                Ok(scripted(200, &[], b"file"))
            }
        });
        let declared = slot_secret("api.example.com", "X-Api-Key", "KEYVALUE").unwrap();
        let got = mod_fetch(&host, "https://api.example.com/start", Some(declared))
            .await
            .unwrap();
        assert_eq!(got["bodyText"], "file");
        let seen = host.hops().seen();
        let carried = |i: usize| seen[i].headers.iter().any(|(n, _)| n == "x-api-key");
        assert!(carried(0));
        assert!(!carried(1));
        // And not in anything the mod is given back.
        assert!(!got.to_string().contains("KEYVALUE"));
    }

    #[tokio::test]
    async fn a_key_is_refused_on_any_listed_host_but_its_slots_and_nothing_is_dialled() {
        // Both hosts are listed; the slot is bound to api only. The key must not reach cdn on the
        // first hop, whatever the handler checked.
        let host = scripted_host();
        host.hops().answer(|_| Ok(scripted(200, &[], b"ok")));
        let declared = slot_secret("api.example.com", "X-Api-Key", "KEYVALUE").unwrap();
        let e = mod_fetch(&host, "https://cdn.example.com/x", Some(declared))
            .await
            .unwrap_err();
        assert_eq!(e.code, Code::HostNotAllowed);
        assert!(host.hops().seen().is_empty());
        assert!(host.hops().looked_up().is_empty());
        // The refusal does not quote the key, and the Debug of a key does not print it.
        assert!(!format!("{e:?}").contains("KEYVALUE"));
        let again = slot_secret("api.example.com", "X-Api-Key", "KEYVALUE").unwrap();
        assert!(!format!("{again:?}").contains("KEYVALUE"));
        // A key with no host binding at all cannot be made: a bad key is "not set".
        assert_eq!(
            slot_secret("api.example.com", "cookie", "v")
                .unwrap_err()
                .code,
            Code::SecretNotSet
        );
        // Its own host still works.
        let ok = slot_secret("api.example.com", "X-Api-Key", "KEYVALUE").unwrap();
        assert!(mod_fetch(&host, "https://api.example.com/x", Some(ok))
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn a_redirect_off_the_manifests_list_is_refused_and_never_looked_up() {
        for to in [
            "https://evil.example.org/x",
            "http://cdn.example.com/x",
            "https://cdn.example.com:8443/x",
            "//evil.example.org/x",
        ] {
            let host = scripted_host();
            host.hops().dns("evil.example.org", &["93.184.216.40"]);
            host.hops()
                .answer(move |_| Ok(scripted(301, &[("Location", to)], b"")));
            let e = mod_fetch(&host, "https://api.example.com/", None)
                .await
                .unwrap_err();
            assert_eq!(e.code, Code::HostNotAllowed, "{to}");
            assert_eq!(host.hops().seen().len(), 1, "{to}");
            assert_eq!(
                host.hops().looked_up(),
                vec![("api.example.com".to_owned(), 443)],
                "{to}"
            );
        }
    }

    #[tokio::test]
    async fn a_listed_name_that_resolves_to_this_computer_or_the_lan_is_never_dialled() {
        for ip in [
            "127.0.0.1",
            "192.168.1.1",
            "::ffff:127.0.0.1",
            "64:ff9b::a00:1",
        ] {
            let host = scripted_host();
            host.hops().dns("api.example.com", &[ip]);
            host.hops().answer(|_| Ok(scripted(200, &[], b"inside")));
            let e = mod_fetch(&host, "https://api.example.com/", None)
                .await
                .unwrap_err();
            assert_eq!(e.code, Code::HostNotPublic, "{ip}");
            assert!(host.hops().seen().is_empty(), "{ip}");
        }
        let host = scripted_host();
        host.hops().dns("api.example.com", &[]);
        assert_eq!(
            mod_fetch(&host, "https://api.example.com/", None)
                .await
                .unwrap_err()
                .code,
            Code::HostUnresolved
        );
    }

    #[tokio::test]
    async fn the_other_failures_reach_a_mod_as_their_codes() {
        let host = scripted_host();
        host.hops()
            .answer(|_| Ok(scripted(302, &[("Location", "/again")], b"")));
        assert_eq!(
            mod_fetch(&host, "https://api.example.com/", None)
                .await
                .unwrap_err()
                .code,
            Code::TooManyRedirects
        );
        assert_eq!(host.hops().seen().len(), limits::MAX_FETCH_REDIRECTS + 1);

        let host = scripted_host();
        host.hops().answer(|_| {
            Ok(scripted(
                200,
                &[],
                &vec![b'x'; limits::MAX_FETCH_RESPONSE_BYTES + 1],
            ))
        });
        assert_eq!(
            mod_fetch(&host, "https://api.example.com/", None)
                .await
                .unwrap_err()
                .code,
            Code::ResponseTooLarge
        );

        let host = scripted_host();
        host.hops()
            .answer(|_| Err(crate::mods::netfetch::FetchError::Unavailable));
        assert_eq!(
            mod_fetch(&host, "https://api.example.com/", None)
                .await
                .unwrap_err()
                .code,
            Code::NetworkUnavailable
        );
    }
}
