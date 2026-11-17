// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Fetching the signed index and the packages it lists (Mods spec §1.10, D6), and when to do it.
//!
//! **The signature is the trust root, not the host or the transport.** What is pinned here only
//! keeps PM from being sent somewhere else and keeps each request small:
//! - A request starts at a URL that begins, exactly and after URL normalisation (so `..` and
//!   `%2e%2e` can't climb out), with [`index::RELEASE_DOWNLOAD_PREFIX`]: https, host `github.com`,
//!   the one repo, no credentials, no query. The index is two assets on the fixed `index` release
//!   (never `releases/latest`, which moves).
//! - GitHub answers a release download with a redirect to its asset CDN (measured 2026-11-11:
//!   `release-assets.githubusercontent.com`). PM follows the redirects itself, one request at a
//!   time with reqwest's own following off, at most [`MAX_REDIRECTS`], and only to
//!   [`index::ASSET_REDIRECT_HOSTS`] over https on the default port. Any other host is refused,
//!   whatever it sends.
//! - Every body is capped while it is read (a lying `Content-Length` doesn't matter), and every
//!   request has a deadline. The user agent is generic: no account, no email, no version.
//!
//! **Dev builds only** ([`Endpoints::machine`]): `PM_MODS_INDEX_URL` (read through
//! `keys::dev_setting`, so only with `PM_DATA_DIR` set) replaces the release prefix with a
//! directory laid out the same way, for the live tests L6 and L14, so they never touch the public
//! index. It may be plain http and may redirect inside its own origin. A release build has no such
//! code at all: [`Endpoints::machine`] is a different function there, which returns the pinned one.
//!
//! **Schedule** ([`due`], [`delay_after`]): pure. At launch, then every [`REFRESH_EVERY`] while PM
//! runs; a failed attempt (offline, a torn pair, a 5xx) is retried sooner on a short back-off, so a
//! laptop that wakes up offline is not blind for six hours. A clock that went backwards counts as
//! due: the cost of a check is two small requests, the cost of waiting out a mis-set clock is a
//! blind safety list.

// The refresher and the updater are the callers.
#![cfg_attr(not(test), allow(dead_code))]

use std::fmt;
use std::future::Future;
use std::time::Duration;

use chrono::{DateTime, Utc};
use reqwest::Url;
use sha2::{Digest, Sha256};

use super::index::{
    ASSET_REDIRECT_HOSTS, INDEX_CAP, INDEX_FILE, INDEX_SIGNATURE_FILE, INDEX_TAG,
    RELEASE_DOWNLOAD_PREFIX, SIGNATURE_CAP,
};
use super::package::Limits;

/// The dev-only variable that replaces [`RELEASE_DOWNLOAD_PREFIX`] (module doc). Compiled out of a
/// release build, so its name isn't even in the binary.
#[cfg(debug_assertions)]
pub const INDEX_BASE_VAR: &str = "PM_MODS_INDEX_URL";

/// Generic on purpose: nothing that names a person, an account or a machine.
pub const USER_AGENT: &str = "PM-mods/1";
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Both index files are well under a megabyte.
pub const INDEX_TIMEOUT: Duration = Duration::from_secs(30);
/// A package may be [`Limits::DEFAULT`]`.file` (32 MiB), on a slow line.
pub const PACKAGE_TIMEOUT: Duration = Duration::from_secs(300);
/// GitHub's one redirect to the asset CDN, with room for one more.
pub const MAX_REDIRECTS: usize = 3;

/// How often the index is fetched while PM runs (D6).
pub const REFRESH_EVERY: Duration = Duration::from_secs(6 * 3600);
/// How long after launch the first fetch waits, so it doesn't compete with the open.
pub const LAUNCH_DELAY: Duration = Duration::from_secs(20);
/// How often the scheduler looks at the clock.
pub const TICK: Duration = Duration::from_secs(60);

// ---- Where to fetch from -------------------------------------------------------------------------

/// The release prefix every URL must start with, and what a redirect may reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    prefix: String,
    /// A dev override: plain http and same-origin redirects are allowed. Never set in a release
    /// build (the only constructor is compiled out there).
    dev: bool,
}

impl Endpoints {
    /// The public index's release prefix.
    pub fn production() -> Self {
        Self {
            prefix: RELEASE_DOWNLOAD_PREFIX.to_owned(),
            dev: false,
        }
    }

    /// What this build fetches from: the pinned prefix, or in a dev build with `PM_DATA_DIR` set,
    /// the override.
    #[cfg(debug_assertions)]
    pub fn machine() -> Self {
        Self::from_setting(super::keys::dev_setting(INDEX_BASE_VAR))
    }

    /// A release build never reads the variable.
    #[cfg(not(debug_assertions))]
    pub fn machine() -> Self {
        Self::production()
    }

    #[cfg(debug_assertions)]
    fn from_setting(setting: Option<String>) -> Self {
        let Some(base) = setting else {
            return Self::production();
        };
        match Self::dev(&base) {
            Ok(e) => {
                eprintln!(
                    "mods: fetching the safety list from {} (dev build)",
                    e.prefix
                );
                e
            }
            Err(why) => {
                eprintln!("mods: {INDEX_BASE_VAR} ignored: {why}");
                Self::production()
            }
        }
    }

    /// A directory standing in for [`RELEASE_DOWNLOAD_PREFIX`]: `<base>index/index.json`,
    /// `<base>index/index.json.minisig` and `<base><id>-v<version>/<id>-<version>.pmmod`.
    #[cfg(debug_assertions)]
    pub(crate) fn dev(base: &str) -> Result<Self, &'static str> {
        let mut base = base.trim().to_owned();
        if !base.ends_with('/') {
            base.push('/');
        }
        let url = Url::parse(&base).map_err(|_| "not a URL")?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err("it must be an http or https URL");
        }
        if url.query().is_some() || url.fragment().is_some() || !url.username().is_empty() {
            return Err("it must have no query, fragment or credentials");
        }
        Ok(Self {
            prefix: url.to_string(),
            dev: true,
        })
    }

    pub fn is_production(&self) -> bool {
        !self.dev
    }

    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Whether plain http may be used (a dev override only).
    pub fn plain_http(&self) -> bool {
        self.dev
    }

    pub fn index_url(&self) -> String {
        format!("{}{INDEX_TAG}/{INDEX_FILE}", self.prefix)
    }

    pub fn signature_url(&self) -> String {
        format!("{}{INDEX_TAG}/{INDEX_SIGNATURE_FILE}", self.prefix)
    }

    /// A catalogue entry's URL (or [`super::index::package_url`]'s) as this build fetches it: the
    /// release prefix swapped for the override's, a dev build's only difference. A URL that isn't
    /// under the release prefix is left as it is, and [`Endpoints::start_url`] then refuses it.
    pub fn package_url(&self, url: &str) -> String {
        match url.strip_prefix(RELEASE_DOWNLOAD_PREFIX) {
            Some(rest) => format!("{}{rest}", self.prefix),
            None => url.to_owned(),
        }
    }

    /// The URL a request may start at: under the prefix after normalisation, with the right scheme
    /// and nothing that changes where it goes.
    pub fn start_url(&self, url: &str) -> Result<Url, FetchError> {
        let bad = |why: &str| FetchError::NotPinned(why.to_owned());
        let parsed = Url::parse(url).map_err(|_| bad("it isn't a URL"))?;
        if !self.scheme_ok(&parsed) {
            return Err(bad("it isn't https"));
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(bad("it carries credentials"));
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(bad("it has a query or a fragment"));
        }
        // The normalised form, so `..` and its encodings have already been resolved.
        if !parsed.as_str().starts_with(&self.prefix) {
            return Err(bad("it isn't under PM-Mods' release downloads"));
        }
        Ok(parsed)
    }

    /// Where a redirect from `from` may go: https, no credentials, the default port, and a host on
    /// [`ASSET_REDIRECT_HOSTS`] (a dev override's own origin as well).
    pub fn follow(&self, from: &Url, location: &str) -> Result<Url, FetchError> {
        let bad = |why: &str| FetchError::Redirect(why.to_owned());
        let next = from
            .join(location)
            .map_err(|_| bad("its Location isn't a URL"))?;
        if !self.scheme_ok(&next) {
            return Err(bad("it leaves https"));
        }
        if !next.username().is_empty() || next.password().is_some() {
            return Err(bad("it carries credentials"));
        }
        let host = next.host_str().unwrap_or_default();
        let on_list = next.port().is_none() && ASSET_REDIRECT_HOSTS.contains(&host);
        let own_origin = self.dev
            && Url::parse(&self.prefix).is_ok_and(|p| {
                p.host_str() == next.host_str()
                    && p.port_or_known_default() == next.port_or_known_default()
            });
        if on_list || own_origin {
            Ok(next)
        } else {
            Err(bad("it goes to a host PM doesn't download from"))
        }
    }

    fn scheme_ok(&self, url: &Url) -> bool {
        url.scheme() == "https" || (self.dev && url.scheme() == "http")
    }
}

// ---- Fetching ------------------------------------------------------------------------------------

/// Why a fetch gave nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// No answer: offline, DNS, a refused connection, a timeout, a body cut short. Quiet.
    Offline(String),
    /// The server answered with something other than the file (not published yet, a 5xx). Quiet.
    Status(u16),
    /// The body is past its cap.
    TooLarge,
    /// A redirect PM won't follow.
    Redirect(String),
    /// The request's own URL isn't one PM may start at (a catalogue entry that isn't under the
    /// pinned prefix).
    NotPinned(String),
    /// A download whose sha256 isn't the one the signed index lists.
    ShaMismatch { want: String, got: String },
}

impl FetchError {
    /// Whether this is the ordinary weather (offline, not published yet) that PM doesn't log.
    pub fn is_quiet(&self) -> bool {
        matches!(self, Self::Offline(_) | Self::Status(_))
    }
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Offline(why) => write!(f, "no answer ({why})"),
            Self::Status(s) => write!(f, "the server answered {s}"),
            Self::TooLarge => f.write_str("the file is larger than PM accepts"),
            Self::Redirect(why) => write!(f, "a redirect was refused: {why}"),
            Self::NotPinned(why) => write!(f, "the URL isn't one PM fetches from: {why}"),
            Self::ShaMismatch { want, got } => {
                write!(f, "the download's sha256 {got} isn't the listed {want}")
            }
        }
    }
}

impl std::error::Error for FetchError {}

/// One answer from the network, with no redirect followed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hop {
    pub status: u16,
    pub location: Option<String>,
    /// Only a 2xx answer's body, already capped.
    pub body: Vec<u8>,
}

/// One HTTP GET, no redirect followed, the body read up to `cap` bytes within `timeout`. The real
/// one is [`ReqwestTransport`]; the tests inject their own.
pub trait Transport: Send + Sync {
    fn get(
        &self,
        url: &Url,
        cap: usize,
        timeout: Duration,
    ) -> impl Future<Output = Result<Hop, FetchError>> + Send;
}

/// GET `url` (which must start under the prefix), following the allowed redirects, and return the
/// body.
pub async fn fetch<T: Transport>(
    transport: &T,
    endpoints: &Endpoints,
    url: &str,
    cap: usize,
    timeout: Duration,
) -> Result<Vec<u8>, FetchError> {
    let mut current = endpoints.start_url(url)?;
    for hops in 0..=MAX_REDIRECTS {
        let hop = transport.get(&current, cap, timeout).await?;
        match hop.status {
            200..=299 => {
                if hop.body.len() > cap {
                    return Err(FetchError::TooLarge);
                }
                return Ok(hop.body);
            }
            301 | 302 | 303 | 307 | 308 => {
                if hops == MAX_REDIRECTS {
                    return Err(FetchError::Redirect("too many redirects".into()));
                }
                let location = hop
                    .location
                    .ok_or_else(|| FetchError::Redirect("it has no Location".into()))?;
                current = endpoints.follow(&current, &location)?;
            }
            status => return Err(FetchError::Status(status)),
        }
    }
    Err(FetchError::Redirect("too many redirects".into()))
}

/// `index.json` and its signature, each within its cap.
pub async fn fetch_index_pair<T: Transport>(
    transport: &T,
    endpoints: &Endpoints,
) -> Result<(Vec<u8>, Vec<u8>), FetchError> {
    let index = fetch(
        transport,
        endpoints,
        &endpoints.index_url(),
        INDEX_CAP,
        INDEX_TIMEOUT,
    )
    .await?;
    let signature = fetch(
        transport,
        endpoints,
        &endpoints.signature_url(),
        SIGNATURE_CAP,
        INDEX_TIMEOUT,
    )
    .await?;
    Ok((index, signature))
}

/// A package at `url` (its catalogue entry's, or [`super::index::package_url`] for a repair), whose
/// sha256 must be `sha256` (lowercase hex) before anything reads its contents: the signed index's
/// word for what the file is. Capped at the package limit.
pub async fn fetch_package<T: Transport>(
    transport: &T,
    endpoints: &Endpoints,
    url: &str,
    sha256: &str,
) -> Result<Vec<u8>, FetchError> {
    let bytes = fetch(
        transport,
        endpoints,
        &endpoints.package_url(url),
        Limits::DEFAULT.file,
        PACKAGE_TIMEOUT,
    )
    .await?;
    let got = hex::encode(Sha256::digest(&bytes));
    if got != sha256 {
        return Err(FetchError::ShaMismatch {
            want: sha256.to_owned(),
            got,
        });
    }
    Ok(bytes)
}

/// The real transport: reqwest with its own redirect following off, https only unless the dev
/// override allows plain http, a generic user agent, and the body read up to the cap.
pub struct ReqwestTransport {
    plain_http: bool,
}

impl ReqwestTransport {
    pub fn new(endpoints: &Endpoints) -> Self {
        Self {
            plain_http: endpoints.plain_http(),
        }
    }
}

impl Transport for ReqwestTransport {
    fn get(
        &self,
        url: &Url,
        cap: usize,
        timeout: Duration,
    ) -> impl Future<Output = Result<Hop, FetchError>> + Send {
        let url = url.clone();
        let plain_http = self.plain_http;
        async move {
            use futures_util::StreamExt;
            let client = reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .redirect(reqwest::redirect::Policy::none())
                .https_only(!plain_http)
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(timeout)
                .build()
                .map_err(|e| FetchError::NotPinned(e.to_string()))?;
            let resp = client
                .get(url)
                .send()
                .await
                .map_err(|e| FetchError::Offline(e.to_string()))?;
            let status = resp.status();
            if status.is_redirection() {
                let location = resp
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                return Ok(Hop {
                    status: status.as_u16(),
                    location,
                    body: Vec::new(),
                });
            }
            if !status.is_success() {
                return Ok(Hop {
                    status: status.as_u16(),
                    location: None,
                    body: Vec::new(),
                });
            }
            // The header is only a hint; the stream below is what is counted.
            if resp.content_length().is_some_and(|n| n > cap as u64) {
                return Err(FetchError::TooLarge);
            }
            let mut body = Vec::new();
            let mut stream = resp.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| FetchError::Offline(e.to_string()))?;
                if body.len() + chunk.len() > cap {
                    return Err(FetchError::TooLarge);
                }
                body.extend_from_slice(&chunk);
            }
            Ok(Hop {
                status: status.as_u16(),
                location: None,
                body,
            })
        }
    }
}

// ---- The schedule (pure) -------------------------------------------------------------------------

/// How long to wait after an attempt, given how many in a row have failed (0: the last one worked).
pub fn delay_after(failures: u32) -> Duration {
    const STEPS: [u64; 4] = [5 * 60, 15 * 60, 60 * 60, 3 * 3600];
    match failures {
        0 => REFRESH_EVERY,
        n => Duration::from_secs(STEPS[(n as usize - 1).min(STEPS.len() - 1)]).min(REFRESH_EVERY),
    }
}

/// `delay` spread by up to 10% either way, from `r` (any random-ish number), so every PM that
/// started at the same moment doesn't ask GitHub at the same moment forever.
pub fn jittered(delay: Duration, r: u32) -> Duration {
    let tenth = delay.as_millis() as u64 / 10;
    if tenth == 0 {
        return delay;
    }
    let offset = r as u64 % (2 * tenth + 1);
    delay - Duration::from_millis(tenth) + Duration::from_millis(offset)
}

/// Whether an attempt is due: none yet, or `delay` has passed since the last. A clock that went
/// backwards (`now` before the last attempt) is due, since how long it has really been is unknown.
pub fn due(last_attempt: Option<DateTime<Utc>>, now: DateTime<Utc>, delay: Duration) -> bool {
    match last_attempt {
        None => true,
        Some(last) if now < last => true,
        Some(last) => (now - last).to_std().is_ok_and(|elapsed| elapsed >= delay),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    /// A transport that answers from a script and records every URL asked.
    struct Script {
        answers: Mutex<VecDeque<Result<Hop, FetchError>>>,
        asked: Mutex<Vec<String>>,
    }

    impl Script {
        fn new(answers: Vec<Result<Hop, FetchError>>) -> Self {
            Self {
                answers: Mutex::new(answers.into()),
                asked: Mutex::new(Vec::new()),
            }
        }
        fn asked(&self) -> Vec<String> {
            self.asked.lock().unwrap().clone()
        }
    }

    impl Transport for Script {
        fn get(
            &self,
            url: &Url,
            _cap: usize,
            _timeout: Duration,
        ) -> impl Future<Output = Result<Hop, FetchError>> + Send {
            self.asked.lock().unwrap().push(url.to_string());
            let next = self
                .answers
                .lock()
                .unwrap()
                .pop_front()
                .expect("the script ran out of answers");
            async move { next }
        }
    }

    fn ok(body: &[u8]) -> Result<Hop, FetchError> {
        Ok(Hop {
            status: 200,
            location: None,
            body: body.to_vec(),
        })
    }

    fn redirect(to: &str) -> Result<Hop, FetchError> {
        Ok(Hop {
            status: 302,
            location: Some(to.to_owned()),
            body: Vec::new(),
        })
    }

    const CDN: &str =
        "https://release-assets.githubusercontent.com/github-production-release-asset/1/abc?sig=x";

    // ---- The pinned URLs -----------------------------------------------------------------------

    #[test]
    fn the_index_is_two_assets_on_the_fixed_index_release() {
        let e = Endpoints::production();
        assert_eq!(
            e.index_url(),
            "https://github.com/itsAtlas-org/PM-Mods/releases/download/index/index.json"
        );
        assert_eq!(e.signature_url(), format!("{}.minisig", e.index_url()));
        assert!(!e.index_url().contains("latest"));
        assert_eq!(e.index_url(), super::super::index::INDEX_URL);
        assert_eq!(e.signature_url(), super::super::index::INDEX_SIGNATURE_URL);
    }

    #[test]
    fn a_request_may_only_start_under_the_pinned_prefix() {
        let e = Endpoints::production();
        let p = RELEASE_DOWNLOAD_PREFIX;
        assert!(e.start_url(&format!("{p}index/index.json")).is_ok());
        for bad in [
            // Another host, another repo, another scheme, the wrong case of the repo, a look-alike.
            "https://evil.example/itsAtlas-org/PM-Mods/releases/download/index/index.json",
            "https://github.com/itsAtlas-org/pm-mods/releases/download/index/index.json",
            "https://github.com/itsAtlas-org/PM-Mods/releases/latest/download/index.json",
            "http://github.com/itsAtlas-org/PM-Mods/releases/download/index/index.json",
            "https://github.com.evil.example/itsAtlas-org/PM-Mods/releases/download/x",
            "https://github.com@evil.example/itsAtlas-org/PM-Mods/releases/download/x",
            "https://user:pw@github.com/itsAtlas-org/PM-Mods/releases/download/x",
            // Climbing out with dot segments, plain and encoded.
            "https://github.com/itsAtlas-org/PM-Mods/releases/download/../../../../evil/x",
            "https://github.com/itsAtlas-org/PM-Mods/releases/download/%2e%2e/%2e%2e/%2e%2e/x",
            // A query or a fragment.
            "https://github.com/itsAtlas-org/PM-Mods/releases/download/index/index.json?x=1",
            "https://github.com/itsAtlas-org/PM-Mods/releases/download/index/index.json#x",
            "not a url",
            "",
        ] {
            assert!(
                matches!(e.start_url(bad), Err(FetchError::NotPinned(_))),
                "{bad} must be refused"
            );
        }
    }

    #[test]
    fn a_redirect_may_only_reach_githubs_asset_hosts_over_https() {
        let e = Endpoints::production();
        let from = e.start_url(&e.index_url()).unwrap();
        for host in ASSET_REDIRECT_HOSTS {
            assert!(e.follow(&from, &format!("https://{host}/a/b?c=d")).is_ok());
        }
        assert!(e.follow(&from, CDN).is_ok());
        for bad in [
            "https://evil.example/a",
            "https://github.com/itsAtlas-org/PM-Mods/releases/download/x",
            "https://release-assets.githubusercontent.com.evil.example/a",
            "https://evil.example/release-assets.githubusercontent.com/a",
            "http://release-assets.githubusercontent.com/a",
            "https://user@release-assets.githubusercontent.com/a",
            "https://release-assets.githubusercontent.com:8443/a",
            "https://127.0.0.1/a",
            "file:///etc/passwd",
            "//evil.example/a",
            "/relative/stays/on/github",
        ] {
            assert!(
                matches!(e.follow(&from, bad), Err(FetchError::Redirect(_))),
                "{bad} must be refused"
            );
        }
        // An explicit default port is the default port.
        assert!(e
            .follow(&from, "https://release-assets.githubusercontent.com:443/a")
            .is_ok());
    }

    #[test]
    fn a_dev_override_keeps_to_its_own_origin_and_a_release_build_has_none() {
        let e = Endpoints::dev("http://127.0.0.1:8099/mods").unwrap();
        assert!(!e.is_production() && e.plain_http());
        assert_eq!(e.prefix(), "http://127.0.0.1:8099/mods/");
        assert_eq!(e.index_url(), "http://127.0.0.1:8099/mods/index/index.json");
        // The release prefix in an entry's URL is swapped for the override's.
        assert_eq!(
            e.package_url(&format!(
                "{RELEASE_DOWNLOAD_PREFIX}a.b-v1.0.0/a.b-1.0.0.pmmod"
            )),
            "http://127.0.0.1:8099/mods/a.b-v1.0.0/a.b-1.0.0.pmmod"
        );
        let from = e.start_url(&e.index_url()).unwrap();
        assert!(e.follow(&from, "/mods/other").is_ok());
        assert!(e.follow(&from, "http://127.0.0.1:8100/x").is_err());
        assert!(e.follow(&from, "http://evil.example/x").is_err());
        // The override can't smuggle a credential or a query in.
        assert!(Endpoints::dev("http://u:p@127.0.0.1/").is_err());
        assert!(Endpoints::dev("http://127.0.0.1/?x=1").is_err());
        assert!(Endpoints::dev("ftp://127.0.0.1/").is_err());
        assert!(Endpoints::dev("nonsense").is_err());
        // Production never changes an entry's URL.
        let p = Endpoints::production();
        let url = format!("{RELEASE_DOWNLOAD_PREFIX}a.b-v1.0.0/a.b-1.0.0.pmmod");
        assert_eq!(p.package_url(&url), url);
        assert!(!p.plain_http());
        // With no setting, a dev build fetches from the public prefix.
        assert!(Endpoints::from_setting(None).is_production());
        assert!(Endpoints::from_setting(Some("nonsense".into())).is_production());
        assert!(!Endpoints::from_setting(Some("http://127.0.0.1:1/".into())).is_production());
    }

    /// Decision 9, as far as a debug test run can check it: the override is read only through
    /// `keys::dev_setting`, whose own tests pin that a release build and a dev build without
    /// `PM_DATA_DIR` get nothing. This pins the other half: in this build the variable reaches the
    /// fetcher only by that route.
    #[test]
    fn the_override_is_read_only_through_the_dev_gate() {
        assert!(super::super::keys::dev_overrides_allowed(true, true));
        assert!(!super::super::keys::dev_overrides_allowed(true, false));
        assert!(!super::super::keys::dev_overrides_allowed(false, true));
        assert!(!super::super::keys::dev_overrides_allowed(false, false));
        assert_eq!(INDEX_BASE_VAR, "PM_MODS_INDEX_URL");
    }

    // ---- fetch ---------------------------------------------------------------------------------

    #[tokio::test]
    async fn a_release_download_follows_githubs_redirect_to_the_asset_host() {
        let e = Endpoints::production();
        let script = Script::new(vec![redirect(CDN), ok(b"{}")]);
        let got = fetch(&script, &e, &e.index_url(), 100, INDEX_TIMEOUT)
            .await
            .unwrap();
        assert_eq!(got, b"{}");
        assert_eq!(script.asked(), vec![e.index_url(), CDN.to_owned()]);
    }

    #[tokio::test]
    async fn a_redirect_to_a_foreign_host_is_refused_before_it_is_asked() {
        let e = Endpoints::production();
        let script = Script::new(vec![redirect("https://evil.example/index.json")]);
        let err = fetch(&script, &e, &e.index_url(), 100, INDEX_TIMEOUT)
            .await
            .unwrap_err();
        assert!(matches!(err, FetchError::Redirect(_)), "{err}");
        assert_eq!(
            script.asked().len(),
            1,
            "the foreign host is never contacted"
        );
        // Also on the second hop.
        let script = Script::new(vec![redirect(CDN), redirect("https://evil.example/x")]);
        let err = fetch(&script, &e, &e.index_url(), 100, INDEX_TIMEOUT)
            .await
            .unwrap_err();
        assert!(matches!(err, FetchError::Redirect(_)), "{err}");
        assert_eq!(script.asked().len(), 2);
    }

    #[tokio::test]
    async fn too_many_redirects_and_a_redirect_with_no_target_are_refused() {
        let e = Endpoints::production();
        let looping: Vec<_> = (0..=MAX_REDIRECTS).map(|_| redirect(CDN)).collect();
        let err = fetch(
            &Script::new(looping),
            &e,
            &e.index_url(),
            100,
            INDEX_TIMEOUT,
        )
        .await
        .unwrap_err();
        assert_eq!(err, FetchError::Redirect("too many redirects".into()));
        let none = Script::new(vec![Ok(Hop {
            status: 302,
            location: None,
            body: Vec::new(),
        })]);
        let err = fetch(&none, &e, &e.index_url(), 100, INDEX_TIMEOUT)
            .await
            .unwrap_err();
        assert!(matches!(err, FetchError::Redirect(_)));
    }

    #[tokio::test]
    async fn an_unpinned_start_is_refused_without_any_request() {
        let e = Endpoints::production();
        let script = Script::new(vec![]);
        let err = fetch(
            &script,
            &e,
            "https://evil.example/index.json",
            100,
            INDEX_TIMEOUT,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, FetchError::NotPinned(_)));
        assert!(script.asked().is_empty());
    }

    #[tokio::test]
    async fn offline_and_not_found_and_server_errors_are_quiet_failures() {
        let e = Endpoints::production();
        let offline = Script::new(vec![Err(FetchError::Offline("dns".into()))]);
        let err = fetch(&offline, &e, &e.index_url(), 100, INDEX_TIMEOUT)
            .await
            .unwrap_err();
        assert!(err.is_quiet());
        for status in [404u16, 403, 500, 503] {
            let s = Script::new(vec![Ok(Hop {
                status,
                location: None,
                body: Vec::new(),
            })]);
            let err = fetch(&s, &e, &e.index_url(), 100, INDEX_TIMEOUT)
                .await
                .unwrap_err();
            assert_eq!(err, FetchError::Status(status));
            assert!(err.is_quiet());
        }
        assert!(!FetchError::TooLarge.is_quiet());
        assert!(!FetchError::Redirect("x".into()).is_quiet());
    }

    #[tokio::test]
    async fn a_body_past_the_cap_is_refused_even_if_the_transport_let_it_through() {
        let e = Endpoints::production();
        let s = Script::new(vec![ok(&[0u8; 11])]);
        let err = fetch(&s, &e, &e.index_url(), 10, INDEX_TIMEOUT)
            .await
            .unwrap_err();
        assert_eq!(err, FetchError::TooLarge);
        let s = Script::new(vec![ok(&[0u8; 10])]);
        assert!(fetch(&s, &e, &e.index_url(), 10, INDEX_TIMEOUT)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn the_index_pair_is_two_fetches_each_with_its_own_cap_and_a_missing_half_fails() {
        let e = Endpoints::production();
        let s = Script::new(vec![ok(b"index"), ok(b"sig")]);
        let (i, g) = fetch_index_pair(&s, &e).await.unwrap();
        assert_eq!((i, g), (b"index".to_vec(), b"sig".to_vec()));
        assert_eq!(s.asked(), vec![e.index_url(), e.signature_url()]);
        let torn = Script::new(vec![
            ok(b"index"),
            Ok(Hop {
                status: 404,
                location: None,
                body: Vec::new(),
            }),
        ]);
        assert_eq!(
            fetch_index_pair(&torn, &e).await.unwrap_err(),
            FetchError::Status(404)
        );
    }

    #[tokio::test]
    async fn a_package_whose_sha256_isnt_the_listed_one_is_refused_before_verification() {
        let e = Endpoints::production();
        let url = format!("{RELEASE_DOWNLOAD_PREFIX}a.b-v1.0.0/a.b-1.0.0.pmmod");
        let bytes = b"package bytes";
        let right = hex::encode(Sha256::digest(bytes));
        let s = Script::new(vec![ok(bytes)]);
        assert_eq!(
            fetch_package(&s, &e, &url, &right).await.unwrap(),
            bytes.to_vec()
        );
        let s = Script::new(vec![ok(bytes)]);
        let wrong = "0".repeat(64);
        let err = fetch_package(&s, &e, &url, &wrong).await.unwrap_err();
        assert_eq!(
            err,
            FetchError::ShaMismatch {
                want: wrong.clone(),
                got: right
            }
        );
        assert!(!err.is_quiet(), "a bad download is worth a log line");
        // And an entry that isn't under the prefix is never fetched.
        let s = Script::new(vec![]);
        assert!(matches!(
            fetch_package(&s, &e, "https://evil.example/a.pmmod", &wrong)
                .await
                .unwrap_err(),
            FetchError::NotPinned(_)
        ));
    }

    // ---- The real transport, against an in-process server --------------------------------------

    /// Serve each response in turn to one connection each, returning the request heads seen.
    fn serve(responses: Vec<Vec<u8>>) -> (u16, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let mut heads = Vec::new();
            for response in responses {
                let (mut sock, _) = listener.accept().unwrap();
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if sock.read(&mut byte).unwrap_or(0) == 0 {
                        break;
                    }
                    head.push(byte[0]);
                }
                heads.push(String::from_utf8_lossy(&head).into_owned());
                let _ = sock.write_all(&response);
                let _ = sock.flush();
                // A moment for the client to read before the socket closes.
                std::thread::sleep(Duration::from_millis(50));
            }
            heads
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

    fn local(port: u16) -> (Endpoints, ReqwestTransport, Url) {
        let e = Endpoints::dev(&format!("http://127.0.0.1:{port}/")).unwrap();
        let t = ReqwestTransport::new(&e);
        let u = e.start_url(&e.index_url()).unwrap();
        (e, t, u)
    }

    #[tokio::test]
    async fn the_real_transport_reads_a_body_and_sends_only_a_generic_user_agent() {
        let (port, server) = serve(vec![http("200 OK", &["Content-Length: 5"], b"hello")]);
        let (_, t, u) = local(port);
        let hop = t.get(&u, 100, Duration::from_secs(5)).await.unwrap();
        assert_eq!((hop.status, hop.body.as_slice()), (200, &b"hello"[..]));
        let head = server.join().unwrap().remove(0).to_lowercase();
        assert!(head.contains("user-agent: pm-mods/1"), "{head}");
        for private in ["@", "cookie", "authorization", "referer"] {
            assert!(!head.contains(private), "{private} in {head}");
        }
    }

    #[tokio::test]
    async fn the_real_transport_does_not_follow_a_redirect_itself() {
        let (port, server) = serve(vec![http(
            "302 Found",
            &["Location: https://evil.example/x", "Content-Length: 0"],
            b"",
        )]);
        let (_, t, u) = local(port);
        let hop = t.get(&u, 100, Duration::from_secs(5)).await.unwrap();
        assert_eq!(hop.status, 302);
        assert_eq!(hop.location.as_deref(), Some("https://evil.example/x"));
        assert!(hop.body.is_empty());
        assert_eq!(server.join().unwrap().len(), 1, "one request, no follow");
    }

    #[tokio::test]
    async fn the_real_transport_caps_a_body_by_header_and_by_what_actually_arrives() {
        // A declared length past the cap is refused before the body is read.
        let (port, server) = serve(vec![http(
            "200 OK",
            &["Content-Length: 1000"],
            &[b'x'; 1000],
        )]);
        let (_, t, u) = local(port);
        assert_eq!(
            t.get(&u, 100, Duration::from_secs(5)).await.unwrap_err(),
            FetchError::TooLarge
        );
        server.join().unwrap();
        // No length at all: the stream is counted as it arrives.
        let (port, server) = serve(vec![http("200 OK", &[], &[b'x'; 1000])]);
        let (_, t, u) = local(port);
        assert_eq!(
            t.get(&u, 100, Duration::from_secs(5)).await.unwrap_err(),
            FetchError::TooLarge
        );
        server.join().unwrap();
        // A body exactly at the cap is fine.
        let (port, server) = serve(vec![http("200 OK", &[], &[b'x'; 100])]);
        let (_, t, u) = local(port);
        assert_eq!(
            t.get(&u, 100, Duration::from_secs(5))
                .await
                .unwrap()
                .body
                .len(),
            100
        );
        server.join().unwrap();
    }

    #[tokio::test]
    async fn the_real_transport_reports_a_status_a_dead_port_and_a_stall_as_quiet_failures() {
        let (port, server) = serve(vec![http("404 Not Found", &["Content-Length: 0"], b"")]);
        let (_, t, u) = local(port);
        assert_eq!(
            t.get(&u, 100, Duration::from_secs(5)).await.unwrap().status,
            404
        );
        server.join().unwrap();

        // Nothing listening.
        let dead = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let (_, t, u) = local(dead);
        let err = t.get(&u, 100, Duration::from_secs(5)).await.unwrap_err();
        assert!(
            matches!(err, FetchError::Offline(_)) && err.is_quiet(),
            "{err}"
        );

        // A server that accepts and never answers hits the deadline.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let stall = std::thread::spawn(move || {
            let (_sock, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(1500));
        });
        let (_, t, u) = local(port);
        let err = t
            .get(&u, 100, Duration::from_millis(300))
            .await
            .unwrap_err();
        assert!(matches!(err, FetchError::Offline(_)), "{err}");
        stall.join().unwrap();
    }

    #[tokio::test]
    async fn plain_http_is_refused_by_a_transport_built_for_production() {
        let prod = Endpoints::production();
        let t = ReqwestTransport::new(&prod);
        let (port, server) = serve(vec![http("200 OK", &["Content-Length: 1"], b"x")]);
        let u = Url::parse(&format!("http://127.0.0.1:{port}/x")).unwrap();
        let err = t.get(&u, 100, Duration::from_secs(2)).await.unwrap_err();
        assert!(matches!(err, FetchError::Offline(_)), "{err}");
        // Nothing was sent: the server's accept never happens, so unblock it.
        let _ = std::net::TcpStream::connect(("127.0.0.1", port));
        let _ = server.join();
    }

    // ---- The schedule --------------------------------------------------------------------------

    #[test]
    fn the_index_is_due_at_launch_then_every_six_hours() {
        let now = t("2026-12-01T10:00:00Z");
        assert!(due(None, now, REFRESH_EVERY), "at launch");
        let last = Some(now);
        assert!(!due(last, now, REFRESH_EVERY));
        assert!(!due(last, t("2026-12-01T15:59:59Z"), REFRESH_EVERY));
        assert!(due(last, t("2026-12-01T16:00:00Z"), REFRESH_EVERY));
        assert!(due(last, t("2026-12-02T10:00:00Z"), REFRESH_EVERY));
        assert_eq!(delay_after(0), Duration::from_secs(6 * 3600));
    }

    #[test]
    fn a_clock_that_went_backwards_is_due() {
        let last = Some(t("2026-12-01T10:00:00Z"));
        assert!(due(last, t("2026-12-01T03:00:00Z"), REFRESH_EVERY));
    }

    #[test]
    fn a_failing_fetch_backs_off_in_steps_and_never_past_six_hours() {
        let secs: Vec<u64> = (0..8).map(|n| delay_after(n).as_secs()).collect();
        assert_eq!(
            secs,
            [21600, 300, 900, 3600, 10800, 10800, 10800, 10800],
            "6 h when it works, 5 min, 15 min, 1 h, then 3 h"
        );
        assert!(delay_after(u32::MAX) <= REFRESH_EVERY);
    }

    #[test]
    fn jitter_stays_within_ten_percent() {
        let d = Duration::from_secs(6 * 3600);
        let tenth = d / 10;
        for r in [0u32, 1, 12345, u32::MAX, 7_777_777] {
            let j = jittered(d, r);
            assert!(j >= d - tenth && j <= d + tenth, "{j:?}");
        }
        assert_eq!(
            jittered(Duration::from_millis(5), 99),
            Duration::from_millis(5)
        );
        assert_eq!(jittered(d, 0), d - tenth);
    }
}
