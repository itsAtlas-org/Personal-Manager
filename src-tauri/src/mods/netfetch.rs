// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The shared fetch core: one HTTPS request that follows its redirects ITSELF, so that every hop,
//! the first request and each redirect target, is checked, resolved, screened and pinned before it
//! is dialled. Calendar feeds (a URL the user pasted) and a mod's `net.fetch` (a URL the mod
//! wrote) both fetch through [`fetch`]; each brings its own rule for which URLs it accepts
//! (`check`) and its own [`Policy`].
//!
//! What every hop gets, in this order, and why:
//! 1. `check(url)`: the caller's rule. A mod's is "https, port 443, no userinfo, a host its
//!    manifest lists" (`api/net.rs`), so a redirect off the list is refused like a first request.
//! 2. [`HopFetcher::resolve`] under the fetch's remaining time: the public-internet screen
//!    (`net_screen`). `getaddrinfo` has no deadline of its own and cannot be cancelled, so the
//!    wait is bounded here (the lookup thread may outlive it, which is why a mod's fetches are
//!    capped at a few at once).
//! 3. [`HopFetcher::send`] to those exact addresses (the PIN: the address screened is the address
//!    dialled, with no rebinding window), no automatic redirect, the body read only up to its cap.
//!
//! The fetch as a whole has one deadline ([`Policy::total`]) covering every hop and body, and the
//! body is capped again here whatever the fetcher did, so a fetcher that forgets the cap cannot let
//! an oversize body through.
//!
//! **Redirect rules** (the WHATWG fetch ones, because that is what a mod author expects): 301 and
//! 302 turn a POST into a body-less GET, 303 turns anything but GET and HEAD into one, 307 and 308
//! keep the method and the body. A body-less hop drops the headers that described the body. A
//! header named `authorization` is dropped when the next hop is another origin. [`FetchRequest::
//! first_hop_headers`] (a mod's secret) go on the FIRST request only, never on any redirect, even
//! to the same host (D-7).
//!
//! **No proxy for a mod.** [`Policy::system_proxy`] is false for `net.fetch`: through an
//! environment proxy the proxy would resolve the NAME again, so the pin and the screen would mean
//! nothing (Q6, `net_screen`'s header). Calendar feeds keep the system proxy, as they always did.
//!
//! The network edge is the [`HopFetcher`] trait: [`SystemHops`] is the real one (reqwest), and the
//! tests (`fake`) script the answers, so the rules above are tested with no network. The app
//! reaches its one through [`super::api::Host::hops`].

use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;

use reqwest::Url;
use tokio::time::{timeout, Instant};

use super::api::limits;
use crate::net_screen::{self, Refusal};

/// Whether `net.fetch` may go through a proxy named by the environment (`HTTPS_PROXY` and the OS
/// settings). False, so the DNS pin means something. One named place to change it (Q6).
// Nothing in a build reads this until the `net.fetch` handler lands (X6): `Policy::for_mods` does.
#[cfg_attr(not(test), allow(dead_code))]
pub const NET_FETCH_SYSTEM_PROXY: bool = false;

/// The `User-Agent` of a mod's fetch: fixed, no PM version and no mod id (D-22).
#[cfg_attr(not(test), allow(dead_code))]
pub const MOD_USER_AGENT: &str = "PM-mod/1";

/// The methods a fetch can use. Which of them a MOD may name is `api/net.rs`'s rule; this is the
/// set the hop sender understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
    Post,
    Put,
    Patch,
    Delete,
}

impl Method {
    /// Whether a request with this method may carry a body.
    pub fn takes_body(self) -> bool {
        !matches!(self, Self::Get | Self::Head)
    }
}

/// Header name and value pairs, names lowercase. Built by the caller's rules; this module only
/// forwards them.
pub type Headers = Vec<(String, String)>;

/// What one fetch is allowed to do.
#[derive(Debug, Clone, Copy)]
pub struct Policy {
    /// Redirects followed before the next one is refused.
    pub max_redirects: usize,
    /// The whole fetch: every lookup, connection, redirect and body.
    pub total: Duration,
    /// Per hop, how long a connection may take (never more than what is left of `total`).
    pub connect: Duration,
    /// The final body, in bytes: read to this and no further.
    pub max_body: usize,
    pub user_agent: Option<&'static str>,
    /// May an environment proxy carry this fetch? See [`NET_FETCH_SYSTEM_PROXY`].
    pub system_proxy: bool,
}

impl Policy {
    /// A mod's `net.fetch`: the limits are the Developer kit's numbers (`api/limits.rs`).
    #[cfg_attr(not(test), allow(dead_code))]
    pub const fn for_mods() -> Self {
        Self {
            max_redirects: limits::MAX_FETCH_REDIRECTS,
            total: limits::FETCH_TOTAL_TIMEOUT,
            connect: limits::FETCH_CONNECT_TIMEOUT,
            max_body: limits::MAX_FETCH_RESPONSE_BYTES,
            user_agent: Some(MOD_USER_AGENT),
            system_proxy: NET_FETCH_SYSTEM_PROXY,
        }
    }
}

/// A request, before its first hop.
#[derive(Clone)]
pub struct FetchRequest {
    pub method: Method,
    pub url: Url,
    /// Sent on every hop.
    pub headers: Headers,
    /// Sent on the first hop only: a mod's secret. Never on a redirect, never logged.
    pub first_hop_headers: Headers,
    pub body: Option<Vec<u8>>,
}

// By hand: a derived Debug would print `first_hop_headers`, which holds a secret, and any header
// value a mod chose. Names and sizes are what a log line needs.
impl fmt::Debug for FetchRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FetchRequest")
            .field("method", &self.method)
            .field("host", &self.url.host_str())
            .field("headers", &names(&self.headers))
            .field("first_hop_headers", &names(&self.first_hop_headers))
            .field("body_bytes", &self.body.as_ref().map(Vec::len))
            .finish()
    }
}

fn names(headers: &[(String, String)]) -> Vec<&str> {
    headers.iter().map(|(n, _)| n.as_str()).collect()
}

/// The final answer: the first response that is not a redirect, whatever its status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    /// Every header the server sent that is visible ASCII, names lowercase. The caller filters.
    pub headers: Headers,
    pub body: Vec<u8>,
    /// The URL the answer came from, after the redirects.
    pub url: Url,
}

/// Why a fetch ended without an answer. Nothing here carries a URL, an address or a message from
/// the network: each caller words it in its own voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchError {
    /// A URL with no host.
    BadUrl,
    /// A header or the method could not be sent as given.
    BadRequest,
    /// The host is, or resolved to, an address outside the public internet.
    NotPublic,
    /// The host resolved to nothing.
    Unresolved,
    /// Refused, reset, a failed TLS handshake, offline: no answer.
    Unavailable,
    /// The fetch's time ran out (a lookup, a connection, a body that stalled).
    Timeout,
    /// More redirects than the policy follows.
    TooManyRedirects,
    /// A redirect with no usable `Location`.
    BadRedirect,
    /// The body is larger than the policy's cap.
    TooLarge,
}

/// A fetch's failure: the caller's own rule said no ([`Failure::Refused`], with its own reason),
/// or the fetch itself failed ([`Failure::Fetch`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure<E> {
    Refused(E),
    Fetch(FetchError),
}

/// One hop on its way: what [`HopFetcher::send`] is asked to do.
pub struct HopRequest<'a> {
    pub method: Method,
    pub url: &'a Url,
    pub headers: &'a [(String, String)],
    pub body: Option<&'a [u8]>,
    pub policy: &'a Policy,
    /// What is left of the policy's total for this hop, connection and body included.
    pub timeout: Duration,
}

impl fmt::Debug for HopRequest<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HopRequest")
            .field("method", &self.method)
            .field("host", &self.url.host_str())
            .field("headers", &names(self.headers))
            .field("body_bytes", &self.body.map(<[u8]>::len))
            .finish()
    }
}

/// One answer, redirect not followed. A redirect's body is not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HopReply {
    pub status: u16,
    pub headers: Headers,
    pub body: Vec<u8>,
}

/// The network edge of a fetch. [`SystemHops`] is the real one; tests script their own.
pub trait HopFetcher: Send + Sync {
    /// The addresses to dial for `host`, every one screened. The real one is
    /// `net_screen::resolve_and_screen`. The caller bounds the wait.
    fn resolve(
        &self,
        host: &str,
        port: u16,
    ) -> impl Future<Output = Result<Vec<SocketAddr>, Refusal>> + Send;

    /// One request to exactly `pinned`, with no redirect followed and the body read up to
    /// `policy.max_body` ([`FetchError::TooLarge`] past it).
    fn send(
        &self,
        hop: &HopRequest<'_>,
        pinned: &[SocketAddr],
    ) -> impl Future<Output = Result<HopReply, FetchError>> + Send;
}

/// The statuses that name a next URL.
pub fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// What a redirect does to the method and the body (the fetch standard's rules).
fn method_after(status: u16, method: Method) -> Method {
    match (status, method) {
        (301 | 302, Method::Post) => Method::Get,
        (303, m) if !matches!(m, Method::Get | Method::Head) => Method::Get,
        (_, m) => m,
    }
}

/// The headers that describe a request body: they go when the body does.
const BODY_HEADERS: &[&str] = &[
    "content-type",
    "content-encoding",
    "content-language",
    "content-location",
];

/// Fetch `request`, following redirects under `policy`, every hop through `check`, a resolve and a
/// pinned send. See the module docs for the order and the redirect rules.
///
/// `check` is the caller's rule for a URL it will dial (the first one and each redirect target);
/// its error comes back as [`Failure::Refused`], before anything is looked up.
pub async fn fetch<F, E>(
    fetcher: &F,
    request: FetchRequest,
    policy: &Policy,
    check: impl Fn(&Url) -> Result<(), E> + Send,
) -> Result<Reply, Failure<E>>
where
    F: HopFetcher,
{
    let deadline = Instant::now() + policy.total;
    let FetchRequest {
        mut method,
        mut url,
        mut headers,
        first_hop_headers,
        mut body,
    } = request;
    if body.is_some() && !method.takes_body() {
        return Err(Failure::Fetch(FetchError::BadRequest));
    }
    let mut redirects = 0usize;
    loop {
        check(&url).map_err(Failure::Refused)?;
        let host = url
            .host_str()
            .ok_or(Failure::Fetch(FetchError::BadUrl))?
            .to_owned();
        let port = url.port_or_known_default().unwrap_or(443);

        let left = remaining(deadline)?;
        let pinned = match timeout(left, fetcher.resolve(&host, port)).await {
            Ok(Ok(addrs)) => addrs,
            Ok(Err(Refusal::NotPublic)) => return Err(Failure::Fetch(FetchError::NotPublic)),
            Ok(Err(Refusal::Unresolved)) => return Err(Failure::Fetch(FetchError::Unresolved)),
            Err(_) => return Err(Failure::Fetch(FetchError::Timeout)),
        };

        let mut on_the_wire = headers.clone();
        if redirects == 0 {
            on_the_wire.extend(first_hop_headers.iter().cloned());
        }
        let left = remaining(deadline)?;
        let hop = HopRequest {
            method,
            url: &url,
            headers: &on_the_wire,
            body: body.as_deref(),
            policy,
            timeout: left,
        };
        let reply = match timeout(left, fetcher.send(&hop, &pinned)).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(e)) => return Err(Failure::Fetch(e)),
            Err(_) => return Err(Failure::Fetch(FetchError::Timeout)),
        };

        if is_redirect(reply.status) {
            if redirects >= policy.max_redirects {
                return Err(Failure::Fetch(FetchError::TooManyRedirects));
            }
            let location = header(&reply.headers, "location")
                .ok_or(Failure::Fetch(FetchError::BadRedirect))?;
            // Resolved against the URL just fetched, so a relative target works. The next turn of
            // the loop checks, screens and pins it exactly like the first request.
            let next = url
                .join(location)
                .map_err(|_| Failure::Fetch(FetchError::BadRedirect))?;
            let next_method = method_after(reply.status, method);
            if next_method != method {
                method = next_method;
                body = None;
                headers.retain(|(n, _)| !BODY_HEADERS.contains(&n.as_str()));
            }
            if url.origin() != next.origin() {
                // The fetch standard's rule, so a mod's own bearer token never follows a
                // redirect to another origin.
                headers.retain(|(n, _)| n != "authorization");
            }
            url = next;
            redirects += 1;
            continue;
        }

        // A fetcher that did not cap the body does not get to hand over more than the policy.
        if reply.body.len() > policy.max_body {
            return Err(Failure::Fetch(FetchError::TooLarge));
        }
        return Ok(Reply {
            status: reply.status,
            headers: reply.headers,
            body: reply.body,
            url,
        });
    }
}

fn remaining<E>(deadline: Instant) -> Result<Duration, Failure<E>> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err(Failure::Fetch(FetchError::Timeout))
    } else {
        Ok(left)
    }
}

/// The first header called `name` (lowercase).
fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

/// The real network edge: the public-internet screen, then a reqwest client built for that one hop
/// and pinned to the screened addresses.
pub struct SystemHops {
    https_only: bool,
}

impl Default for SystemHops {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemHops {
    /// https only: a hop to an `http://` URL is refused by the client itself.
    pub const fn new() -> Self {
        Self { https_only: true }
    }

    /// Plain http, so a test can talk to a loopback server it owns. Not in a build.
    #[cfg(test)]
    pub(crate) fn plain_http_for_tests() -> Self {
        Self { https_only: false }
    }
}

impl HopFetcher for SystemHops {
    fn resolve(
        &self,
        host: &str,
        port: u16,
    ) -> impl Future<Output = Result<Vec<SocketAddr>, Refusal>> + Send {
        let host = host.to_owned();
        async move { net_screen::resolve_and_screen(&host, port).await }
    }

    fn send(
        &self,
        hop: &HopRequest<'_>,
        pinned: &[SocketAddr],
    ) -> impl Future<Output = Result<HopReply, FetchError>> + Send {
        let https_only = self.https_only;
        let url = hop.url.clone();
        let method = hop.method;
        let headers = hop.headers.to_vec();
        let body = hop.body.map(<[u8]>::to_vec);
        let policy = *hop.policy;
        let left = hop.timeout;
        let pinned = pinned.to_vec();
        async move {
            let host = url.host_str().ok_or(FetchError::BadUrl)?.to_owned();
            // One client per hop: its resolver override is this hop's screened addresses, so the
            // address dialled is the address screened and there is no pool to carry a connection
            // across hops. The URL is left as it is, so Host and the TLS name stay right.
            let mut builder = reqwest::Client::builder()
                .https_only(https_only)
                .resolve_to_addrs(&host, &pinned)
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(policy.connect.min(left))
                .timeout(left);
            if !policy.system_proxy {
                builder = builder.no_proxy();
            }
            if let Some(agent) = policy.user_agent {
                builder = builder.user_agent(agent);
            }
            let client = builder.build().map_err(|_| FetchError::Unavailable)?;

            let mut request = client.request(reqwest_method(method), url);
            for (name, value) in &headers {
                // Checked here, not left to a builder error that arrives with `send`'s.
                let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| FetchError::BadRequest)?;
                let mut value = reqwest::header::HeaderValue::from_str(value)
                    .map_err(|_| FetchError::BadRequest)?;
                // Debug-printing a request must never show a value (a secret rides here).
                value.set_sensitive(true);
                request = request.header(name, value);
            }
            if let Some(body) = body {
                request = request.body(body);
            }

            let mut response = request.send().await.map_err(classify)?;
            let status = response.status().as_u16();
            let headers: Headers = response
                .headers()
                .iter()
                // A value that is not visible ASCII is dropped, never lossily converted.
                .filter_map(|(n, v)| Some((n.as_str().to_owned(), v.to_str().ok()?.to_owned())))
                .collect();
            if is_redirect(status) {
                return Ok(HopReply {
                    status,
                    headers,
                    body: Vec::new(),
                });
            }
            let cap = policy.max_body;
            // A declared length past the cap is refused before a byte of it is read.
            if response.content_length().is_some_and(|n| n > cap as u64) {
                return Err(FetchError::TooLarge);
            }
            let mut body: Vec<u8> = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(classify)? {
                if body.len() + chunk.len() > cap {
                    return Err(FetchError::TooLarge);
                }
                body.extend_from_slice(&chunk);
            }
            Ok(HopReply {
                status,
                headers,
                body,
            })
        }
    }
}

fn reqwest_method(method: Method) -> reqwest::Method {
    match method {
        Method::Get => reqwest::Method::GET,
        Method::Head => reqwest::Method::HEAD,
        Method::Post => reqwest::Method::POST,
        Method::Put => reqwest::Method::PUT,
        Method::Patch => reqwest::Method::PATCH,
        Method::Delete => reqwest::Method::DELETE,
    }
}

/// A transport failure as the fetch reports it: a stall is a timeout, every other cause (refused,
/// reset, TLS, offline) is "no answer". The cause itself never travels (it can name the URL).
fn classify(e: reqwest::Error) -> FetchError {
    if e.is_timeout() {
        FetchError::Timeout
    } else {
        FetchError::Unavailable
    }
}

/// The scripted network edge the tests share: `net.fetch`'s handler tests (X6) and calendar's use
/// it too, through the host's [`super::api::Host::hops`].
#[cfg(test)]
pub(crate) mod fake {
    use std::collections::HashMap;
    use std::net::IpAddr;
    use std::sync::Mutex;

    use super::*;

    /// One hop as the fake saw it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Seen {
        pub method: Method,
        pub url: String,
        pub headers: Headers,
        pub body: Option<Vec<u8>>,
        pub pinned: Vec<SocketAddr>,
    }

    type Answer = Box<dyn Fn(&Seen) -> Result<HopReply, FetchError> + Send + Sync>;

    /// A DNS table, a handler for the requests and a log of both. The lookup answers go through
    /// the real screen (`net_screen::screen_addrs`), so a scripted private answer is refused by
    /// the code that refuses it in the app.
    #[derive(Default)]
    pub struct ScriptedHops {
        dns: Mutex<HashMap<String, Vec<IpAddr>>>,
        answer: Mutex<Option<Answer>>,
        seen: Mutex<Vec<Seen>>,
        looked_up: Mutex<Vec<(String, u16)>>,
        resolve_delay: Mutex<Duration>,
        send_delay: Mutex<Duration>,
    }

    impl ScriptedHops {
        pub fn new() -> Self {
            Self::default()
        }

        /// `host` resolves to `ips` (strings, so a test reads like the address it is about).
        pub fn dns(&self, host: &str, ips: &[&str]) -> &Self {
            self.dns.lock().unwrap().insert(
                host.to_owned(),
                ips.iter().map(|ip| ip.parse().unwrap()).collect(),
            );
            self
        }

        /// What every request is answered with.
        pub fn answer(
            &self,
            f: impl Fn(&Seen) -> Result<HopReply, FetchError> + Send + Sync + 'static,
        ) -> &Self {
            *self.answer.lock().unwrap() = Some(Box::new(f));
            self
        }

        pub fn resolve_takes(&self, d: Duration) -> &Self {
            *self.resolve_delay.lock().unwrap() = d;
            self
        }

        pub fn send_takes(&self, d: Duration) -> &Self {
            *self.send_delay.lock().unwrap() = d;
            self
        }

        pub fn seen(&self) -> Vec<Seen> {
            self.seen.lock().unwrap().clone()
        }

        pub fn looked_up(&self) -> Vec<(String, u16)> {
            self.looked_up.lock().unwrap().clone()
        }
    }

    impl HopFetcher for ScriptedHops {
        fn resolve(
            &self,
            host: &str,
            port: u16,
        ) -> impl Future<Output = Result<Vec<SocketAddr>, Refusal>> + Send {
            let delay = *self.resolve_delay.lock().unwrap();
            self.looked_up.lock().unwrap().push((host.to_owned(), port));
            let addrs: Vec<SocketAddr> = match net_screen::literal_ip(host) {
                Some(ip) => vec![SocketAddr::new(ip, port)],
                None => self
                    .dns
                    .lock()
                    .unwrap()
                    .get(host)
                    .map(|ips| ips.iter().map(|ip| SocketAddr::new(*ip, port)).collect())
                    .unwrap_or_default(),
            };
            async move {
                tokio::time::sleep(delay).await;
                net_screen::screen_addrs(addrs)
            }
        }

        fn send(
            &self,
            hop: &HopRequest<'_>,
            pinned: &[SocketAddr],
        ) -> impl Future<Output = Result<HopReply, FetchError>> + Send {
            let seen = Seen {
                method: hop.method,
                url: hop.url.to_string(),
                headers: hop.headers.to_vec(),
                body: hop.body.map(<[u8]>::to_vec),
                pinned: pinned.to_vec(),
            };
            let delay = *self.send_delay.lock().unwrap();
            let result = match self.answer.lock().unwrap().as_ref() {
                Some(answer) => answer(&seen),
                None => Err(FetchError::Unavailable),
            };
            self.seen.lock().unwrap().push(seen);
            async move {
                tokio::time::sleep(delay).await;
                result
            }
        }
    }

    /// A reply with a status, headers and a body.
    pub fn reply(status: u16, headers: &[(&str, &str)], body: &[u8]) -> HopReply {
        HopReply {
            status,
            headers: headers
                .iter()
                .map(|(n, v)| (n.to_ascii_lowercase(), (*v).to_owned()))
                .collect(),
            body: body.to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use super::fake::{reply, ScriptedHops};
    use super::*;

    const WEB: &str = "93.184.216.34";

    fn policy() -> Policy {
        Policy {
            max_redirects: 5,
            total: Duration::from_secs(10),
            connect: Duration::from_secs(5),
            max_body: 100,
            user_agent: Some("PM-test/1"),
            system_proxy: false,
        }
    }

    fn get(url: &str) -> FetchRequest {
        FetchRequest {
            method: Method::Get,
            url: Url::parse(url).unwrap(),
            headers: vec![],
            first_hop_headers: vec![],
            body: None,
        }
    }

    /// The loop's own rule, so these tests are about the loop: https and a listed host.
    fn listed(hosts: &'static [&'static str]) -> impl Fn(&Url) -> Result<(), &'static str> + Send {
        move |u| {
            if u.scheme() != "https" {
                return Err("scheme");
            }
            match u.host_str() {
                Some(h) if hosts.contains(&h) => Ok(()),
                _ => Err("host"),
            }
        }
    }

    fn net() -> ScriptedHops {
        let net = ScriptedHops::new();
        net.dns("a.example.com", &[WEB])
            .dns("b.example.com", &["93.184.216.35"]);
        net
    }

    async fn run(
        net: &ScriptedHops,
        request: FetchRequest,
        policy: &Policy,
    ) -> Result<Reply, Failure<&'static str>> {
        fetch(
            net,
            request,
            policy,
            listed(&["a.example.com", "b.example.com"]),
        )
        .await
    }

    fn fail(r: Result<Reply, Failure<&'static str>>) -> Failure<&'static str> {
        r.expect_err("the fetch should have failed")
    }

    fn redirect(status: u16, to: &str) -> Result<HopReply, FetchError> {
        Ok(reply(status, &[("Location", to)], b""))
    }

    // ---- the happy path and what is sent ----------------------------------------------------

    #[tokio::test]
    async fn a_plain_answer_comes_back_with_its_status_headers_and_body() {
        let net = net();
        net.answer(|_| Ok(reply(200, &[("Content-Type", "text/plain")], b"hello")));
        let got = run(&net, get("https://a.example.com/x?q=1"), &policy())
            .await
            .unwrap();
        assert_eq!(got.status, 200);
        assert_eq!(got.body, b"hello");
        assert_eq!(
            got.headers,
            vec![("content-type".to_owned(), "text/plain".to_owned())]
        );
        assert_eq!(got.url.as_str(), "https://a.example.com/x?q=1");
        // The screened address, with the URL's port, is what is pinned.
        let seen = net.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].pinned, vec![format!("{WEB}:443").parse().unwrap()]);
        assert_eq!(net.looked_up(), vec![("a.example.com".to_owned(), 443)]);
    }

    #[tokio::test]
    async fn an_error_status_is_an_answer_not_a_failure() {
        let net = net();
        net.answer(|_| Ok(reply(404, &[], b"nope")));
        let got = run(&net, get("https://a.example.com/"), &policy())
            .await
            .unwrap();
        assert_eq!((got.status, got.body.as_slice()), (404, &b"nope"[..]));
    }

    #[tokio::test]
    async fn a_body_with_a_method_that_takes_none_is_refused_before_anything_is_looked_up() {
        let net = net();
        net.answer(|_| Ok(reply(200, &[], b"")));
        let mut request = get("https://a.example.com/");
        request.body = Some(b"x".to_vec());
        assert_eq!(
            fail(run(&net, request, &policy()).await),
            Failure::Fetch(FetchError::BadRequest)
        );
        assert!(net.looked_up().is_empty() && net.seen().is_empty());
    }

    #[tokio::test]
    async fn the_callers_rule_is_asked_first_and_its_reason_comes_back_before_any_lookup() {
        let net = net();
        net.answer(|_| Ok(reply(200, &[], b"")));
        for url in ["http://a.example.com/", "https://c.example.com/"] {
            let got = fail(run(&net, get(url), &policy()).await);
            assert!(matches!(got, Failure::Refused(_)), "{url}");
        }
        assert!(net.looked_up().is_empty() && net.seen().is_empty());
    }

    // ---- the screen, per hop ----------------------------------------------------------------

    #[tokio::test]
    async fn a_listed_name_that_resolves_off_the_internet_is_refused_and_never_dialled() {
        for ip in [
            "127.0.0.1",
            "10.0.0.5",
            "169.254.169.254",
            "100.64.0.1",
            "::1",
            "fd00::1",
            // IPv4 inside IPv6, NAT64, 6to4: judged as the IPv4 they carry.
            "::ffff:127.0.0.1",
            "::ffff:7f00:1",
            "64:ff9b::7f00:1",
            "64:ff9b::a9fe:a9fe",
            "2002:7f00:1::1",
            // The local-use NAT64 prefix can't be read, so it is refused whole.
            "64:ff9b:1::808:808",
        ] {
            let net = ScriptedHops::new();
            net.dns("a.example.com", &[ip])
                .answer(|_| Ok(reply(200, &[], b"secret")));
            assert_eq!(
                fail(run(&net, get("https://a.example.com/"), &policy()).await),
                Failure::Fetch(FetchError::NotPublic),
                "{ip}"
            );
            assert!(net.seen().is_empty(), "{ip} was dialled");
        }
    }

    #[tokio::test]
    async fn one_private_answer_among_public_ones_refuses_the_whole_host() {
        let net = ScriptedHops::new();
        net.dns("a.example.com", &[WEB, "192.168.1.1"])
            .answer(|_| Ok(reply(200, &[], b"")));
        assert_eq!(
            fail(run(&net, get("https://a.example.com/"), &policy()).await),
            Failure::Fetch(FetchError::NotPublic)
        );
        assert!(net.seen().is_empty());
    }

    #[tokio::test]
    async fn a_name_that_resolves_to_nothing_is_unresolved() {
        let net = ScriptedHops::new();
        net.answer(|_| Ok(reply(200, &[], b"")));
        assert_eq!(
            fail(run(&net, get("https://a.example.com/"), &policy()).await),
            Failure::Fetch(FetchError::Unresolved)
        );
        assert!(net.seen().is_empty());
    }

    #[tokio::test]
    async fn a_redirect_is_screened_like_the_first_request() {
        // The first host is fine; it sends the fetch to a listed name that now resolves to the LAN.
        let net = net();
        net.dns("b.example.com", &["192.168.0.1"]).answer(|s| {
            if s.url.contains("a.example.com") {
                redirect(302, "https://b.example.com/in")
            } else {
                Ok(reply(200, &[], b"lan"))
            }
        });
        assert_eq!(
            fail(run(&net, get("https://a.example.com/"), &policy()).await),
            Failure::Fetch(FetchError::NotPublic)
        );
        assert_eq!(net.seen().len(), 1, "the second hop was never dialled");
    }

    // ---- redirects --------------------------------------------------------------------------

    #[tokio::test]
    async fn a_redirect_is_followed_relative_and_absolute() {
        let net = net();
        net.answer(|s| match s.url.as_str() {
            "https://a.example.com/start" => redirect(301, "/two"),
            "https://a.example.com/two" => redirect(307, "https://b.example.com/three"),
            _ => Ok(reply(200, &[], b"done")),
        });
        let got = run(&net, get("https://a.example.com/start"), &policy())
            .await
            .unwrap();
        assert_eq!(got.body, b"done");
        assert_eq!(got.url.as_str(), "https://b.example.com/three");
        let urls: Vec<String> = net.seen().into_iter().map(|s| s.url).collect();
        assert_eq!(
            urls,
            [
                "https://a.example.com/start",
                "https://a.example.com/two",
                "https://b.example.com/three"
            ]
        );
    }

    #[tokio::test]
    async fn a_redirect_off_the_callers_rule_is_refused_whatever_its_form() {
        for to in [
            "https://evil.example.org/x",
            "http://a.example.com/x",
            "//evil.example.org/x",
            "https://user@a.example.com/x",
            "ftp://a.example.com/x",
        ] {
            let net = net();
            net.dns("evil.example.org", &[WEB]);
            net.answer(move |s| {
                if s.url == "https://a.example.com/" {
                    redirect(302, to)
                } else {
                    Ok(reply(200, &[], b"reached"))
                }
            });
            let got = fail(
                fetch(&net, get("https://a.example.com/"), &policy(), |u: &Url| {
                    // The rule a mod's fetch has, in miniature: https, no userinfo, listed host.
                    if u.scheme() == "https"
                        && u.username().is_empty()
                        && u.host_str() == Some("a.example.com")
                    {
                        Ok(())
                    } else {
                        Err("no")
                    }
                })
                .await,
            );
            assert_eq!(got, Failure::Refused("no"), "{to}");
            assert_eq!(net.seen().len(), 1, "{to} was dialled");
        }
    }

    #[tokio::test]
    async fn the_sixth_redirect_is_refused_and_the_fifth_is_followed() {
        let counted = |n: usize| {
            let net = net();
            net.answer(move |s| {
                let at: usize = s.url.rsplit('/').next().unwrap().parse().unwrap_or(0);
                if at < n {
                    redirect(302, &format!("/{}", at + 1))
                } else {
                    Ok(reply(200, &[], b"end"))
                }
            });
            net
        };
        let five = counted(5);
        assert!(run(&five, get("https://a.example.com/0"), &policy())
            .await
            .is_ok());
        assert_eq!(five.seen().len(), 6);
        let six = counted(6);
        assert_eq!(
            fail(run(&six, get("https://a.example.com/0"), &policy()).await),
            Failure::Fetch(FetchError::TooManyRedirects)
        );
        assert_eq!(six.seen().len(), 6, "no seventh request");
    }

    #[tokio::test]
    async fn a_redirect_loop_ends_at_the_cap() {
        let net = net();
        net.answer(|_| redirect(302, "https://a.example.com/"));
        assert_eq!(
            fail(run(&net, get("https://a.example.com/"), &policy()).await),
            Failure::Fetch(FetchError::TooManyRedirects)
        );
    }

    #[tokio::test]
    async fn a_redirect_with_no_usable_location_is_a_bad_redirect() {
        for answer in [
            Ok(reply(302, &[], b"")),
            // Not a URL a base can resolve.
            redirect(302, "https://"),
        ] {
            let net = net();
            net.answer(move |_| answer.clone());
            assert_eq!(
                fail(run(&net, get("https://a.example.com/"), &policy()).await),
                Failure::Fetch(FetchError::BadRedirect)
            );
        }
    }

    #[tokio::test]
    async fn statuses_300_304_and_305_are_answers_not_redirects() {
        for status in [300u16, 304, 305] {
            let net = net();
            net.answer(move |_| {
                Ok(reply(
                    status,
                    &[("Location", "https://evil.example.org/")],
                    b"",
                ))
            });
            let got = run(&net, get("https://a.example.com/"), &policy())
                .await
                .unwrap();
            assert_eq!(got.status, status);
            assert_eq!(net.seen().len(), 1);
        }
    }

    fn post_with_body() -> FetchRequest {
        FetchRequest {
            method: Method::Post,
            url: Url::parse("https://a.example.com/p").unwrap(),
            headers: vec![
                ("content-type".into(), "application/json".into()),
                ("x-trace".into(), "1".into()),
            ],
            first_hop_headers: vec![],
            body: Some(b"{}".to_vec()),
        }
    }

    #[tokio::test]
    async fn redirects_change_the_method_and_body_as_the_fetch_standard_says() {
        // (status, method sent, method after)
        let table = [
            (301, Method::Post, Method::Get),
            (302, Method::Post, Method::Get),
            (303, Method::Post, Method::Get),
            (303, Method::Put, Method::Get),
            (303, Method::Delete, Method::Get),
            (307, Method::Post, Method::Post),
            (308, Method::Put, Method::Put),
            (301, Method::Put, Method::Put),
            (302, Method::Patch, Method::Patch),
            (303, Method::Head, Method::Head),
            (303, Method::Get, Method::Get),
        ];
        for (status, sent, after) in table {
            let net = net();
            net.answer(move |s| {
                if s.url.ends_with("/p") {
                    redirect(status, "/q")
                } else {
                    Ok(reply(200, &[], b""))
                }
            });
            let mut request = post_with_body();
            request.method = sent;
            if !sent.takes_body() {
                request.body = None;
            }
            run(&net, request, &policy()).await.unwrap();
            let seen = net.seen();
            assert_eq!(seen[1].method, after, "{status} {sent:?}");
            let changed = sent != after;
            // The body goes with a method that became GET, and stays otherwise.
            assert_eq!(
                seen[1].body.is_some(),
                !changed && sent.takes_body(),
                "{status} {sent:?}"
            );
            // A hop that lost its body drops the headers that described it, and keeps the rest.
            let names: Vec<&str> = seen[1].headers.iter().map(|(n, _)| n.as_str()).collect();
            assert_eq!(
                names.contains(&"content-type"),
                !changed,
                "{status} {sent:?}"
            );
            assert!(names.contains(&"x-trace"));
        }
    }

    #[tokio::test]
    async fn the_secret_goes_on_the_first_request_only_never_on_a_redirect() {
        // Same host, then another listed host: neither redirect carries it.
        for second in ["/two", "https://b.example.com/two"] {
            let net = net();
            net.answer(move |s| {
                if s.url.ends_with("/one") {
                    redirect(302, second)
                } else {
                    Ok(reply(200, &[], b""))
                }
            });
            let mut request = get("https://a.example.com/one");
            request.headers = vec![("accept".into(), "*/*".into())];
            request.first_hop_headers = vec![("x-api-key".into(), "KEYVALUE".into())];
            run(&net, request, &policy()).await.unwrap();
            let seen = net.seen();
            let has = |i: usize| seen[i].headers.iter().any(|(n, _)| n == "x-api-key");
            assert!(has(0), "the first request carries it");
            assert!(!has(1), "{second}: a redirect must not");
            assert!(seen[1].headers.iter().any(|(n, _)| n == "accept"));
        }
    }

    #[tokio::test]
    async fn authorization_is_dropped_when_a_redirect_changes_origin_and_kept_when_it_does_not() {
        for (to, kept) in [("/two", true), ("https://b.example.com/two", false)] {
            let net = net();
            net.answer(move |s| {
                if s.url.ends_with("/one") {
                    redirect(302, to)
                } else {
                    Ok(reply(200, &[], b""))
                }
            });
            let mut request = get("https://a.example.com/one");
            request.headers = vec![
                ("authorization".into(), "Bearer t".into()),
                ("x-other".into(), "1".into()),
            ];
            run(&net, request, &policy()).await.unwrap();
            let second = &net.seen()[1];
            let has = |n: &str| second.headers.iter().any(|(h, _)| h == n);
            assert_eq!(has("authorization"), kept, "{to}");
            assert!(has("x-other"));
        }
    }

    // ---- size and time ----------------------------------------------------------------------

    #[tokio::test]
    async fn a_body_past_the_cap_is_too_large_even_when_the_fetcher_did_not_cap_it() {
        let net = net();
        net.answer(|_| Ok(reply(200, &[], &[b'x'; 101])));
        assert_eq!(
            fail(run(&net, get("https://a.example.com/"), &policy()).await),
            Failure::Fetch(FetchError::TooLarge)
        );
        // Exactly at the cap is an answer.
        let net = self::net();
        net.answer(|_| Ok(reply(200, &[], &[b'x'; 100])));
        assert!(run(&net, get("https://a.example.com/"), &policy())
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn a_fetchers_own_failures_pass_through_unchanged() {
        for e in [
            FetchError::Unavailable,
            FetchError::Timeout,
            FetchError::TooLarge,
            FetchError::BadRequest,
        ] {
            let net = net();
            net.answer(move |_| Err(e));
            assert_eq!(
                fail(run(&net, get("https://a.example.com/"), &policy()).await),
                Failure::Fetch(e)
            );
        }
    }

    #[tokio::test]
    async fn a_lookup_that_does_not_come_back_in_time_is_a_timeout() {
        let net = net();
        net.resolve_takes(Duration::from_secs(30))
            .answer(|_| Ok(reply(200, &[], b"")));
        let mut p = policy();
        p.total = Duration::from_millis(60);
        assert_eq!(
            fail(run(&net, get("https://a.example.com/"), &p).await),
            Failure::Fetch(FetchError::Timeout)
        );
        assert!(net.seen().is_empty());
    }

    #[tokio::test]
    async fn a_hop_that_stalls_is_a_timeout() {
        let net = net();
        net.send_takes(Duration::from_secs(30))
            .answer(|_| Ok(reply(200, &[], b"")));
        let mut p = policy();
        p.total = Duration::from_millis(60);
        assert_eq!(
            fail(run(&net, get("https://a.example.com/"), &p).await),
            Failure::Fetch(FetchError::Timeout)
        );
    }

    #[tokio::test]
    async fn the_time_limit_is_for_the_whole_fetch_not_each_hop() {
        // Each hop takes 40 ms; four of them is past a 100 ms total though none is alone.
        let net = net();
        net.send_takes(Duration::from_millis(40)).answer(|s| {
            let at: usize = s.url.rsplit('/').next().unwrap().parse().unwrap_or(0);
            if at < 3 {
                redirect(302, &format!("/{}", at + 1))
            } else {
                Ok(reply(200, &[], b""))
            }
        });
        let mut p = policy();
        p.total = Duration::from_millis(100);
        assert_eq!(
            fail(run(&net, get("https://a.example.com/0"), &p).await),
            Failure::Fetch(FetchError::Timeout)
        );
    }

    #[tokio::test]
    async fn a_url_with_no_host_is_a_bad_url() {
        let net = net();
        let got = fetch(&net, get("mailto:a@example.com"), &policy(), |_: &Url| {
            Ok::<(), ()>(())
        })
        .await
        .expect_err("no host");
        assert_eq!(got, Failure::Fetch(FetchError::BadUrl));
    }

    // ---- Debug never shows a value ----------------------------------------------------------

    #[test]
    fn debug_output_names_headers_but_never_shows_a_value() {
        let mut request = get("https://a.example.com/path?token=URLSECRET");
        request.headers = vec![("authorization".into(), "HEADERSECRET".into())];
        request.first_hop_headers = vec![("x-api-key".into(), "KEYSECRET".into())];
        request.body = Some(b"BODYSECRET".to_vec());
        let shown = format!("{request:?}");
        // `needle`, not `secret`: CodeQL reads a variable of that name as a credential, and the
        // message below as a log. These are test markers, never a real secret.
        for needle in ["HEADERSECRET", "KEYSECRET", "BODYSECRET", "URLSECRET"] {
            assert!(!shown.contains(needle), "{needle} in {shown}");
        }
        assert!(shown.contains("x-api-key") && shown.contains("a.example.com"));
        let url = Url::parse("https://a.example.com/").unwrap();
        let hop = HopRequest {
            method: Method::Get,
            url: &url,
            headers: &request.first_hop_headers,
            body: None,
            policy: &policy(),
            timeout: Duration::from_secs(1),
        };
        assert!(!format!("{hop:?}").contains("KEYSECRET"));
    }

    #[test]
    fn the_mod_policy_is_the_kits_numbers_and_no_proxy() {
        let p = Policy::for_mods();
        assert_eq!(p.max_redirects, limits::MAX_FETCH_REDIRECTS);
        assert_eq!(p.total, limits::FETCH_TOTAL_TIMEOUT);
        assert_eq!(p.connect, limits::FETCH_CONNECT_TIMEOUT);
        assert_eq!(p.max_body, limits::MAX_FETCH_RESPONSE_BYTES);
        assert_eq!(p.user_agent, Some("PM-mod/1"));
        assert!(!p.system_proxy && !NET_FETCH_SYSTEM_PROXY);
    }

    // ---- the real edge, against an in-process server -----------------------------------------

    /// Serve one connection: read its request head, write `first`, stall for `stall`, then write
    /// `rest`. The thread returns the request head it read.
    fn serve(
        first: Vec<u8>,
        stall: Duration,
        rest: Vec<u8>,
    ) -> (u16, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                if sock.read(&mut byte).unwrap_or(0) == 0 {
                    break;
                }
                head.push(byte[0]);
            }
            let _ = sock.write_all(&first);
            let _ = sock.flush();
            std::thread::sleep(stall);
            let _ = sock.write_all(&rest);
            let _ = sock.flush();
            std::thread::sleep(Duration::from_millis(50));
            String::from_utf8_lossy(&head).into_owned()
        });
        (port, handle)
    }

    fn http(status: &str, headers: &[&str], body: &[u8]) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {status}\r\nConnection: close\r\n").into_bytes();
        for h in headers {
            out.extend_from_slice(format!("{h}\r\n").as_bytes());
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(body);
        out
    }

    /// `pinned.example` has no DNS record anywhere: it is dialled only because the pin says so.
    fn pinned_request(port: u16) -> (Url, Vec<SocketAddr>) {
        (
            Url::parse(&format!("http://pinned.example:{port}/thing")).unwrap(),
            vec![SocketAddr::from(([127, 0, 0, 1], port))],
        )
    }

    async fn hop(
        port: u16,
        method: Method,
        headers: Headers,
        body: Option<Vec<u8>>,
        policy: &Policy,
        timeout: Duration,
    ) -> Result<HopReply, FetchError> {
        let (url, pinned) = pinned_request(port);
        SystemHops::plain_http_for_tests()
            .send(
                &HopRequest {
                    method,
                    url: &url,
                    headers: &headers,
                    body: body.as_deref(),
                    policy,
                    timeout,
                },
                &pinned,
            )
            .await
    }

    #[tokio::test]
    async fn the_real_edge_dials_the_pinned_address_and_sends_what_it_was_given() {
        let (port, server) = serve(
            http(
                "200 OK",
                &[
                    "Content-Length: 5",
                    "Set-Cookie: s=1",
                    "Content-Type: text/x",
                    // Not visible ASCII (UTF-8 bytes): dropped, never lossily converted.
                    "X-Odd: caf\u{e9}",
                ],
                b"hello",
            ),
            Duration::ZERO,
            vec![],
        );
        let got = hop(
            port,
            Method::Post,
            vec![("x-trace".into(), "7".into())],
            Some(b"payload".to_vec()),
            &policy(),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!((got.status, got.body.as_slice()), (200, &b"hello"[..]));
        // Everything visible comes back; filtering what a mod sees is the caller's rule.
        assert!(got.headers.iter().any(|(n, _)| n == "set-cookie"));
        assert!(got
            .headers
            .iter()
            .any(|(n, v)| n == "content-type" && v == "text/x"));
        assert!(!got.headers.iter().any(|(n, _)| n == "x-odd"));
        let head = server.join().unwrap().to_lowercase();
        assert!(head.starts_with("post /thing "), "{head}");
        assert!(head.contains("x-trace: 7"), "{head}");
        assert!(head.contains("user-agent: pm-test/1"), "{head}");
        assert!(
            head.contains(&format!("host: pinned.example:{port}")),
            "{head}"
        );
        for private in ["cookie", "authorization", "referer", "origin"] {
            assert!(!head.contains(private), "{private} in {head}");
        }
    }

    #[tokio::test]
    async fn the_real_edge_sends_no_user_agent_when_the_policy_has_none() {
        let (port, server) = serve(
            http("200 OK", &["Content-Length: 0"], b""),
            Duration::ZERO,
            vec![],
        );
        let mut p = policy();
        p.user_agent = None;
        hop(port, Method::Get, vec![], None, &p, Duration::from_secs(5))
            .await
            .unwrap();
        assert!(!server.join().unwrap().to_lowercase().contains("user-agent"));
    }

    #[tokio::test]
    async fn the_production_edge_will_not_speak_plain_http() {
        // The test edge reaches a plain-http loopback server; the one the app uses must not.
        let (port, server) = serve(
            http("200 OK", &["Content-Length: 2"], b"hi"),
            Duration::ZERO,
            vec![],
        );
        let (url, pinned) = pinned_request(port);
        let refused = SystemHops::new()
            .send(
                &HopRequest {
                    method: Method::Get,
                    url: &url,
                    headers: &[],
                    body: None,
                    policy: &policy(),
                    timeout: Duration::from_secs(5),
                },
                &pinned,
            )
            .await;
        assert_eq!(refused.unwrap_err(), FetchError::Unavailable);
        // The client refused before connecting; one connection from here frees the server thread.
        let _ = std::net::TcpStream::connect(("127.0.0.1", port));
        assert_eq!(server.join().unwrap(), "", "a request reached the server");
    }

    #[tokio::test]
    async fn the_real_edge_does_not_follow_a_redirect_and_does_not_read_its_body() {
        let (port, server) = serve(
            http(
                "302 Found",
                &["Location: https://evil.example/x", "Content-Length: 5000"],
                b"",
            ),
            Duration::ZERO,
            vec![],
        );
        let got = hop(
            port,
            Method::Get,
            vec![],
            None,
            &policy(),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(got.status, 302);
        assert!(got.body.is_empty());
        assert!(got
            .headers
            .iter()
            .any(|(n, v)| n == "location" && v == "https://evil.example/x"));
        server.join().unwrap();
    }

    #[tokio::test]
    async fn the_real_edge_caps_a_body_by_header_and_by_what_arrives() {
        let (port, server) = serve(
            http("200 OK", &["Content-Length: 1000"], &[b'x'; 1000]),
            Duration::ZERO,
            vec![],
        );
        assert_eq!(
            hop(
                port,
                Method::Get,
                vec![],
                None,
                &policy(),
                Duration::from_secs(5)
            )
            .await
            .unwrap_err(),
            FetchError::TooLarge
        );
        server.join().unwrap();
        // A declared length past the cap is refused before the body is waited for: this server
        // sends 10 of the 1000 bytes it promised and then stalls.
        let (port, server) = serve(
            http("200 OK", &["Content-Length: 1000"], b"0123456789"),
            Duration::from_secs(2),
            vec![],
        );
        assert_eq!(
            hop(
                port,
                Method::Get,
                vec![],
                None,
                &policy(),
                Duration::from_millis(500)
            )
            .await
            .unwrap_err(),
            FetchError::TooLarge
        );
        server.join().unwrap();
        // No declared length: counted as it arrives.
        let (port, server) = serve(http("200 OK", &[], &[b'x'; 1000]), Duration::ZERO, vec![]);
        assert_eq!(
            hop(
                port,
                Method::Get,
                vec![],
                None,
                &policy(),
                Duration::from_secs(5)
            )
            .await
            .unwrap_err(),
            FetchError::TooLarge
        );
        server.join().unwrap();
        // Exactly the cap is fine.
        let (port, server) = serve(http("200 OK", &[], &[b'x'; 100]), Duration::ZERO, vec![]);
        assert_eq!(
            hop(
                port,
                Method::Get,
                vec![],
                None,
                &policy(),
                Duration::from_secs(5)
            )
            .await
            .unwrap()
            .body
            .len(),
            100
        );
        server.join().unwrap();
    }

    #[tokio::test]
    async fn the_real_edge_times_out_on_a_body_that_stalls_and_reports_a_dead_port_quietly() {
        // Headers, then silence while the body is owed.
        let (port, server) = serve(
            http("200 OK", &["Content-Length: 10"], b"12345"),
            Duration::from_secs(3),
            b"67890".to_vec(),
        );
        assert_eq!(
            hop(
                port,
                Method::Get,
                vec![],
                None,
                &policy(),
                Duration::from_millis(300)
            )
            .await
            .unwrap_err(),
            FetchError::Timeout
        );
        server.join().unwrap();
        // Nothing listening: refused, not a panic and not a message.
        let dead = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        assert_eq!(
            hop(
                dead,
                Method::Get,
                vec![],
                None,
                &policy(),
                Duration::from_secs(5)
            )
            .await
            .unwrap_err(),
            FetchError::Unavailable
        );
    }

    #[tokio::test]
    async fn the_real_edge_refuses_a_header_it_cannot_send() {
        for (name, value) in [("bad name", "v"), ("x-a", "line\r\nbreak: 1")] {
            assert_eq!(
                hop(
                    1,
                    Method::Get,
                    vec![(name.into(), value.into())],
                    None,
                    &policy(),
                    Duration::from_secs(5)
                )
                .await
                .unwrap_err(),
                FetchError::BadRequest,
                "{name:?}"
            );
        }
    }

    #[tokio::test]
    async fn the_whole_loop_over_the_real_edge_follows_a_redirect_with_the_pin() {
        // Two servers: the first redirects to the second. Both names are made up; only the fake
        // resolver "knows" them, and it hands out loopback, so the screen is skipped by using a
        // plain-http real edge behind a resolver of our own.
        struct Loopback(SystemHops);
        impl HopFetcher for Loopback {
            async fn resolve(&self, _host: &str, port: u16) -> Result<Vec<SocketAddr>, Refusal> {
                Ok(vec![SocketAddr::from(([127, 0, 0, 1], port))])
            }
            fn send(
                &self,
                hop: &HopRequest<'_>,
                pinned: &[SocketAddr],
            ) -> impl Future<Output = Result<HopReply, FetchError>> + Send {
                self.0.send(hop, pinned)
            }
        }
        let (second, second_server) = serve(http("200 OK", &[], b"final"), Duration::ZERO, vec![]);
        let (first, first_server) = serve(
            http(
                "302 Found",
                &[
                    &format!("Location: http://two.example:{second}/there"),
                    "Content-Length: 0",
                ],
                b"",
            ),
            Duration::ZERO,
            vec![],
        );
        let mut request = get(&format!("http://one.example:{first}/start"));
        request.first_hop_headers = vec![("x-api-key".into(), "ONLYFIRST".into())];
        let got = fetch(
            &Loopback(SystemHops::plain_http_for_tests()),
            request,
            &policy(),
            |_: &Url| Ok::<(), ()>(()),
        )
        .await
        .unwrap();
        assert_eq!(got.body, b"final");
        assert!(first_server
            .join()
            .unwrap()
            .to_lowercase()
            .contains("x-api-key: onlyfirst"));
        assert!(!second_server
            .join()
            .unwrap()
            .to_lowercase()
            .contains("x-api-key"));
    }

    // ---- no proxy: the pin has to mean something -----------------------------------------------

    /// The child half of [`an_environment_proxy_is_never_used_for_a_mod_fetch`]: a fetch to the
    /// real server named in `PM_NETFETCH_PROBE_PORT`, in a process whose environment names a
    /// proxy. A test run on its own (no variable) does nothing.
    #[tokio::test]
    async fn proxy_probe_child() {
        let Ok(port) = std::env::var("PM_NETFETCH_PROBE_PORT") else {
            return;
        };
        let port: u16 = port.parse().unwrap();
        let mut p = policy();
        p.system_proxy = std::env::var("PM_NETFETCH_PROBE_PROXY_OK").is_ok();
        let got = hop(port, Method::Get, vec![], None, &p, Duration::from_secs(2)).await;
        assert_eq!(
            got.unwrap().body,
            b"direct",
            "the proxy carried the request"
        );
    }

    #[test]
    fn an_environment_proxy_is_never_used_for_a_mod_fetch() {
        // A proxy that records any connection it gets, named by the environment of a child test
        // process (the environment cannot be changed safely inside this one).
        let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
        proxy.set_nonblocking(true).unwrap();
        let proxy_port = proxy.local_addr().unwrap().port();
        let me = std::env::current_exe().unwrap();
        let run_child = |system_proxy_ok: bool| {
            let (port, server) = serve(
                http("200 OK", &["Content-Length: 6"], b"direct"),
                Duration::ZERO,
                vec![],
            );
            let mut cmd = std::process::Command::new(&me);
            cmd.args([
                "mods::netfetch::tests::proxy_probe_child",
                "--exact",
                "--test-threads=1",
            ])
            .env("PM_NETFETCH_PROBE_PORT", port.to_string())
            .env("HTTP_PROXY", format!("http://127.0.0.1:{proxy_port}"))
            .env("http_proxy", format!("http://127.0.0.1:{proxy_port}"))
            .env_remove("NO_PROXY")
            .env_remove("no_proxy");
            if system_proxy_ok {
                cmd.env("PM_NETFETCH_PROBE_PROXY_OK", "1");
            }
            let out = cmd.output().unwrap();
            // A child that never reached the server (a filter that matched nothing) would leave
            // its thread waiting in `accept`: one connection from here frees it, and what it read
            // is then empty, which the caller's assertion on the request head catches.
            let _ = std::net::TcpStream::connect(("127.0.0.1", port));
            (out, server.join().unwrap())
        };

        let (out, head) = run_child(false);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert!(
            head.starts_with("GET /thing"),
            "the child never fetched: {head:?}"
        );
        assert!(
            proxy.accept().is_err(),
            "a mod fetch went to the environment's proxy"
        );

        // The check is real: the same child with the proxy allowed does go to it.
        let (out, _) = run_child(true);
        assert!(
            !out.status.success(),
            "the probe can't tell a proxy from none"
        );
        assert!(proxy.accept().is_ok(), "the proxy was never contacted");
    }
}
