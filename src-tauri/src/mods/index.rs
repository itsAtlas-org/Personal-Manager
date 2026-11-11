// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The signed index (Mods spec §1.10, D15): its format, its verification, and the pure rules for
//! accepting one. The per-machine record of what was accepted is [`super::index_state`]; fetching
//! and applying it are the fetcher's.
//!
//! **Where:** two assets on the fixed `index` release of `itsAtlas-org/PM-Mods` ([`INDEX_URL`],
//! [`INDEX_SIGNATURE_URL`]), never `releases/latest`, which moves. The download host is not the
//! trust root: the signature is.
//!
//! **Order:** both files are size-capped; the signature over `index.json`'s exact bytes is checked
//! with the trusted keys (prehashed minisign only, through the package verifier's one check)
//! before a byte of JSON is parsed; then the JSON is parsed strictly; then the signed trusted
//! comment must equal the one this module builds from the JSON ([`trusted_comment`]); then what it
//! revokes must be the signer's to revoke.
//!
//! **`index.json`** (format 1). One object, every field required, no other field anywhere:
//! - `format`: `1`.
//! - `seq`: an integer, 1 to 2^53 - 1. The sign job adds one for every index it publishes.
//! - `issued`, `expires`: UTC times as `YYYY-MM-DDTHH:MM:SSZ` exactly; `issued` < `expires` <=
//!   `issued` + [`MAX_VALIDITY_DAYS`].
//! - `revokedKeyIds`: key ids, 16 uppercase hex digits, ascending, at most [`MAX_REVOKED`]. Only an
//!   index signed by the rollover key may name a key this PM holds, and it may never name the
//!   rollover key; ids this PM doesn't hold are allowed and ignored. Once a rotation has happened,
//!   every later index carries it, so a fresh install learns it on its first fetch.
//! - `mods`: the catalogue, ascending by `id`, at most [`MAX_MODS`]. Each entry: `id` (the mod id
//!   grammar), `name` (plain text, 1-40 characters), `description` (plain text, up to 280, newlines
//!   allowed), `tier` (`first` or `second`, matching the id: `atlas.` is first party's), `author`
//!   (plain text, 1-64), `version` (the latest; the ledger's semver grammar: canonical, lowercase, no
//!   build metadata), `api` (`<min>-<target>`, each `major.minor`, as the package's trusted comment
//!   writes it), `url` (exactly [`package_url`] of the id and version), `sha256` (of the signed
//!   `.pmmod`, 64 lowercase hex), `permissions` (the manifest's permission ids, ascending, each
//!   known), `sourceRepo` (`owner/repo`), `commit` (40 lowercase hex), `licence`
//!   (`AGPL-3.0-or-later`).
//! - `blacklist`: at most [`MAX_BLACKLIST`] entries, each either `{"id", "versions"}` or
//!   `{"sha256"}` (never both), plus `action` (`paused` or `removed`), `reason` (plain text, 1-280,
//!   one line) and, for a pause only, an optional `fixedIn` (a version). `versions` is one of `*`,
//!   `=V`, `>=V`, `>V`, `<V`, `<=V`, or a lower then an upper bound joined by `", "` with the lower
//!   below the upper: plain version order, prereleases included ([`VersionRange`]). A `fixedIn` on
//!   an id entry lies above its range, which therefore has an upper bound.
//!
//! **Unknown fields are refused, not ignored.** One writer (the sign job) produces the index, and
//! its dry run is checked against this parser, so strictness costs nothing there and catches a
//! misspelled field (`fixed_in` read as absent would keep a fixed mod paused forever) before it
//! ships. A later format goes out as a new asset beside this one (`index-2.json`), so an older PM
//! keeps reading format 1 and never loses its blacklist to a format it can't read.
//!
//! **`index.json.minisig`**: minisign's four lines, prehashed, by an Atlas key. Its trusted comment
//! is tab-separated `key:value` fields: `pmindex:1`, `seq:<seq>`, `expires:<expires>`, then
//! `revoke:<id>` for each `revokedKeyIds` entry in order, every value exactly as the JSON writes it.
//! The leading tag keeps a mod's signature (`pmmod:1`, `package.rs`) from passing as an index's.
//!
//! **Acceptance** ([`judge`]): a seq below [`MIN_INDEX_SEQ`], below the seen seq, or equal to it with
//! other bytes is refused. A *good fetch* is a verified index with seq >= the seen seq that hasn't
//! expired, allowing [`CLOCK_SKEW_DAYS`]. An authentic newer index that has expired is still
//! accepted for what it restricts (its pauses, removals and revocations) but isn't a good fetch,
//! and its catalogue isn't used to install or update (plan decision 10). One exception to the seq
//! rule: an unexpired, rollover-signed index that revokes a key this machine still trusts is
//! accepted whatever the seen seq, so a leaked main key that signed a huge seq can't lock the
//! rollover out.

// The fetcher, the catalogue and serve checks (slice 5's second part) are the callers; until they
// land, only the tests and the state file are.
#![cfg_attr(not(test), allow(dead_code))]

use std::collections::BTreeSet;
use std::fmt;

use chrono::{DateTime, Duration, NaiveDateTime, Timelike, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::disk::VersionName;
use super::ids::{self, ModId, Tier};
use super::keys::{KeyId, KeyRole, TrustedKeys};
use super::package::{
    self, check_minisign, present, quote, ApiRange, SignatureFault, LICENCE, PERMISSIONS,
};

// ---- Constants ----------------------------------------------------------------------------------

/// The one format this PM reads; the trusted comment's tag carries it too.
pub const INDEX_FORMAT: u32 = 1;
/// The trusted comment's tag.
pub const COMMENT_TAG: &str = "pmindex";

/// The lowest seq this PM accepts, built into each release. **1 until the first real index
/// exists**: the sign job's first index is seq 1, and 0 stays "nothing seen", so the two can never
/// be confused. The release that ships the mods raises it to the live index's seq, never above it
/// (plan decision 12): a fresh install then can't be fed an older authentic index.
pub const MIN_INDEX_SEQ: u64 = 1;
/// The highest seq: the largest integer every JSON reader holds exactly.
pub const MAX_SEQ: u64 = (1 << 53) - 1;

/// `index.json`'s cap. A catalogue entry is well under 1 KiB, so this is room for about a thousand
/// mods and as many blacklist entries.
pub const INDEX_CAP: usize = 1 << 20;
/// `index.json.minisig`'s cap: four lines, the trusted comment the longest.
pub const SIGNATURE_CAP: usize = 4 << 10;
pub const MAX_MODS: usize = 1000;
pub const MAX_BLACKLIST: usize = 1000;
pub const MAX_REVOKED: usize = 16;

/// How long after `issued` an index may claim to stay good. Bobby's cadence is 60 days (2026-11-11);
/// the cap is twice that, so a sign-job mistake can't stretch the window in which a frozen copy of
/// an old index still counts as a good fetch.
pub const MAX_VALIDITY_DAYS: i64 = 120;
/// The clock skew allowed past `expires` (spec §1.10: a computer's clock can run hours off).
pub const CLOCK_SKEW_DAYS: i64 = 2;
/// After this long without a good fetch, the Mods tab says "safety list not checked since …".
pub const STALE_AFTER_DAYS: i64 = 30;

/// The repo, in the one spelling every exact-string pin uses (plan decision 15).
pub const PM_MODS_REPO: &str = "itsAtlas-org/PM-Mods";
/// Every URL the index names, and the index's own, starts exactly with this.
pub const RELEASE_DOWNLOAD_PREFIX: &str =
    "https://github.com/itsAtlas-org/PM-Mods/releases/download/";
/// The fixed release that holds the index (never `releases/latest`).
pub const INDEX_TAG: &str = "index";
pub const INDEX_FILE: &str = "index.json";
pub const INDEX_SIGNATURE_FILE: &str = "index.json.minisig";
pub const INDEX_URL: &str =
    "https://github.com/itsAtlas-org/PM-Mods/releases/download/index/index.json";
pub const INDEX_SIGNATURE_URL: &str =
    "https://github.com/itsAtlas-org/PM-Mods/releases/download/index/index.json.minisig";
/// The hosts a release download may redirect to, for the fetcher. Measured 2026-11-11 (a `HEAD` of a
/// public PM release asset answered 302 to `release-assets.githubusercontent.com`); the second is
/// the host GitHub's documentation and older redirects name. https only; the signature, not the
/// host, is what PM trusts.
pub const ASSET_REDIRECT_HOSTS: &[&str] = &[
    "release-assets.githubusercontent.com",
    "objects.githubusercontent.com",
];

/// Where a mod's signed package is published: release `<id>-v<version>` (the tag the ruleset
/// protects), asset `<id>-<version>.pmmod`. A catalogue entry's `url` must be exactly this, and a
/// repair builds the same URL for a pinned version the index no longer lists.
pub fn package_url(id: &ModId, version: &VersionName) -> String {
    let (id, v) = (id.as_str(), version.as_str());
    format!("{RELEASE_DOWNLOAD_PREFIX}{id}-v{v}/{id}-{v}.pmmod")
}

// ---- The parsed index ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Index {
    pub seq: u64,
    pub issued: DateTime<Utc>,
    pub expires: DateTime<Utc>,
    pub revoked_key_ids: Vec<KeyId>,
    pub mods: Vec<CatalogueEntry>,
    pub blacklist: Vec<BlacklistEntry>,
}

/// A mod the catalogue lists, at its latest version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogueEntry {
    pub id: ModId,
    pub name: String,
    pub description: String,
    pub tier: Tier,
    pub author: String,
    pub version: VersionName,
    pub api: ApiRange,
    pub url: String,
    /// The signed `.pmmod`'s sha256, lowercase hex.
    pub sha256: String,
    pub permissions: Vec<String>,
    pub source_repo: String,
    pub commit: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlacklistAction {
    /// Switched off, data kept, resumed when no entry matches any more (spec §1.10).
    Paused,
    /// Switched off and its files and cache-class rows deleted; its content and gifts wait for the
    /// user (I-16).
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlacklistTarget {
    /// One mod's versions in a range.
    Versions { id: ModId, range: VersionRange },
    /// One file, by its sha256 (lowercase hex), whatever it calls itself: a known-bad unsigned
    /// file is blocked too.
    Sha256(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlacklistEntry {
    pub target: BlacklistTarget,
    pub action: BlacklistAction,
    pub reason: String,
    pub fixed_in: Option<VersionName>,
}

/// A blacklist's `versions`: at most one lower and one upper bound, matched in plain version order,
/// prereleases included. Not Cargo's requirement syntax, whose rules for prereleases (`*` doesn't
/// match `1.0.0-beta`) were made for picking dependencies and would let a pre-release of a bad
/// version through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionRange {
    lower: Option<(semver::Version, bool)>,
    upper: Option<(semver::Version, bool)>,
}

impl VersionRange {
    pub fn parse(s: &str) -> Option<Self> {
        if s == "*" {
            return Some(Self {
                lower: None,
                upper: None,
            });
        }
        if let Some(v) = s.strip_prefix('=') {
            let v = version(v)?;
            return Some(Self {
                lower: Some((v.clone(), true)),
                upper: Some((v, true)),
            });
        }
        let (first, second) = match s.split_once(", ") {
            Some((a, b)) => (a, Some(b)),
            None => (s, None),
        };
        let (lower, upper) = match (bound(first)?, second.map(bound)) {
            (Bound::Lower(l), None) => (Some(l), None),
            (Bound::Upper(u), None) => (None, Some(u)),
            (Bound::Lower(l), Some(Some(Bound::Upper(u)))) if l.0 < u.0 => (Some(l), Some(u)),
            _ => return None,
        };
        Some(Self { lower, upper })
    }

    pub fn contains(&self, v: &semver::Version) -> bool {
        let above = self
            .lower
            .as_ref()
            .is_none_or(|(l, inclusive)| v > l || (*inclusive && v == l));
        let below = self
            .upper
            .as_ref()
            .is_none_or(|(u, inclusive)| v < u || (*inclusive && v == u));
        above && below
    }

    /// Whether the whole range lies below `v` (one with no upper bound never does).
    fn lies_below(&self, v: &semver::Version) -> bool {
        self.upper
            .as_ref()
            .is_some_and(|(u, inclusive)| v > u || (!*inclusive && v == u))
    }
}

enum Bound {
    Lower((semver::Version, bool)),
    Upper((semver::Version, bool)),
}

fn bound(s: &str) -> Option<Bound> {
    if let Some(v) = s.strip_prefix(">=") {
        Some(Bound::Lower((version(v)?, true)))
    } else if let Some(v) = s.strip_prefix('>') {
        Some(Bound::Lower((version(v)?, false)))
    } else if let Some(v) = s.strip_prefix("<=") {
        Some(Bound::Upper((version(v)?, true)))
    } else if let Some(v) = s.strip_prefix('<') {
        Some(Bound::Upper((version(v)?, false)))
    } else {
        None
    }
}

/// A version in the ledger's grammar ([`VersionName`]), as a semver for ordering.
fn version(s: &str) -> Option<semver::Version> {
    VersionName::parse(s).ok()?;
    semver::Version::parse(s).ok()
}

impl Index {
    /// Whether `now` is inside the index's life, allowing [`CLOCK_SKEW_DAYS`] for a clock that runs
    /// fast. Only an index for which this holds is used to install or update.
    pub fn current_at(&self, now: DateTime<Utc>) -> bool {
        now < self.expires + Duration::days(CLOCK_SKEW_DAYS)
    }

    /// The catalogue's entry for `id`.
    pub fn entry(&self, id: &str) -> Option<&CatalogueEntry> {
        self.mods.iter().find(|m| m.id.as_str() == id)
    }

    /// The strongest blacklist entry matching a mod at `version` with the package `sha256`
    /// (lowercase hex, as the ledger records it): a removal over a pause. A version that isn't a
    /// semver (no ledger row has one) matches every range for its id, so a restriction is never
    /// dodged by an unreadable version.
    pub fn blacklisted(
        &self,
        id: &str,
        version: &str,
        sha256: Option<&str>,
    ) -> Option<&BlacklistEntry> {
        let version = semver::Version::parse(version).ok();
        let matches = |e: &&BlacklistEntry| match &e.target {
            BlacklistTarget::Versions { id: eid, range } => {
                eid.as_str() == id && version.as_ref().is_none_or(|v| range.contains(v))
            }
            BlacklistTarget::Sha256(bad) => sha256.is_some_and(|s| s == bad),
        };
        let mut found = self.blacklist.iter().filter(matches);
        let first = found.next()?;
        if first.action == BlacklistAction::Removed {
            return Some(first);
        }
        Some(
            found
                .find(|e| e.action == BlacklistAction::Removed)
                .unwrap_or(first),
        )
    }
}

/// The trusted comment the index's signature must carry, built from the parsed JSON.
pub fn trusted_comment(index: &Index) -> String {
    let mut out = format!(
        "{COMMENT_TAG}:{INDEX_FORMAT}\tseq:{}\texpires:{}",
        index.seq,
        time_text(index.expires)
    );
    for id in &index.revoked_key_ids {
        out.push_str(&format!("\trevoke:{id}"));
    }
    out
}

/// `YYYY-MM-DDTHH:MM:SSZ`, the index's one time format.
pub fn time_text(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// [`time_text`]'s inverse, refusing every other spelling (offsets, fractions, a leap second).
pub fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    if s.len() != 20 {
        return None;
    }
    let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%SZ").ok()?;
    let t = naive.and_utc();
    (naive.nanosecond() == 0 && time_text(t) == s).then_some(t)
}

// ---- Verification -------------------------------------------------------------------------------

/// Which rule refused an index. Tests match on it; the log reads [`IndexError`]'s text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexRefusal {
    TooLarge,
    Signature,
    Legacy,
    UntrustedKey,
    BadSignature,
    TrustedComment,
    Json,
    Format,
    Field,
    CommentMismatch,
    Revocation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexError {
    pub refusal: IndexRefusal,
    pub detail: String,
}

impl fmt::Display for IndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the mod index was refused: {}", self.detail)
    }
}

impl std::error::Error for IndexError {}

fn refuse(refusal: IndexRefusal, detail: impl Into<String>) -> IndexError {
    IndexError {
        refusal,
        detail: detail.into(),
    }
}

fn field(detail: impl Into<String>) -> IndexError {
    refuse(IndexRefusal::Field, detail)
}

/// An index whose signature, format, comment and revocations all passed. Nothing about its seq or
/// its age has been judged yet ([`judge`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedIndex {
    pub index: Index,
    /// `index.json`'s sha256, lowercase hex.
    pub sha256: String,
    pub signer: KeyId,
    pub signer_role: KeyRole,
    /// The keys this PM holds that the index revokes.
    pub revokes: BTreeSet<KeyId>,
    /// Both files as fetched, for the per-machine cache.
    pub bytes: Vec<u8>,
    pub signature: Vec<u8>,
}

/// Verify a fetched `index.json` and its `.minisig` against `keys` (see the module doc's order).
pub fn verify_index(
    bytes: &[u8],
    signature: &[u8],
    keys: &TrustedKeys,
) -> Result<VerifiedIndex, IndexError> {
    if bytes.len() > INDEX_CAP {
        return Err(refuse(IndexRefusal::TooLarge, "index.json is too large"));
    }
    if signature.len() > SIGNATURE_CAP {
        return Err(refuse(IndexRefusal::TooLarge, "its signature is too large"));
    }
    let (at, comment) =
        check_minisign(bytes, signature, keys.public_keys()).map_err(|fault| match fault {
            SignatureFault::Malformed => {
                refuse(IndexRefusal::Signature, "its signature is malformed")
            }
            SignatureFault::Legacy => refuse(
                IndexRefusal::Legacy,
                "its signature uses minisign's legacy mode, which PM refuses",
            ),
            SignatureFault::UntrustedKey => refuse(
                IndexRefusal::UntrustedKey,
                "it isn't signed by a key PM trusts",
            ),
            SignatureFault::BadSignature => {
                refuse(IndexRefusal::BadSignature, "its signature doesn't verify")
            }
        })?;
    let signer = keys
        .signer(at)
        .ok_or_else(|| refuse(IndexRefusal::UntrustedKey, "no key at the signer's index"))?;

    // The signature is good, so the comment is authentic: first, is it an index's at all?
    let tag = format!("{COMMENT_TAG}:{INDEX_FORMAT}");
    if comment.split('\t').next() != Some(tag.as_str()) {
        return Err(refuse(
            IndexRefusal::TrustedComment,
            "its signature isn't an index's",
        ));
    }

    let index = parse_index(bytes)?;
    if comment != trusted_comment(&index) {
        return Err(refuse(
            IndexRefusal::CommentMismatch,
            "its signed seq, expiry or revocations aren't the ones in index.json",
        ));
    }
    let revokes = revocations(&index, signer.role, keys)?;

    Ok(VerifiedIndex {
        sha256: hex::encode(Sha256::digest(bytes)),
        signer: signer.id,
        signer_role: signer.role,
        revokes,
        bytes: bytes.to_vec(),
        signature: signature.to_vec(),
        index,
    })
}

/// What the index revokes, if its signer may: only the rollover key revokes a key PM holds, and
/// nothing revokes a rollover key (else a leaked main key could revoke the key meant to replace it).
fn revocations(
    index: &Index,
    signer: KeyRole,
    keys: &TrustedKeys,
) -> Result<BTreeSet<KeyId>, IndexError> {
    let mut out = BTreeSet::new();
    for id in &index.revoked_key_ids {
        match keys.role_of(*id) {
            Some(KeyRole::Rollover) => {
                return Err(refuse(
                    IndexRefusal::Revocation,
                    format!("it revokes the rollover key {id}, which nothing may revoke"),
                ))
            }
            Some(_) if signer != KeyRole::Rollover => {
                return Err(refuse(
                    IndexRefusal::Revocation,
                    format!("it revokes {id}, and only the rollover key may revoke a key"),
                ))
            }
            Some(_) => {
                out.insert(*id);
            }
            // A key this PM never held (one an older PM trusted): nothing to take away here.
            None => {}
        }
    }
    Ok(out)
}

// ---- The JSON as written ------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawIndex {
    format: u32,
    seq: u64,
    issued: String,
    expires: String,
    revoked_key_ids: Vec<String>,
    mods: Vec<RawEntry>,
    blacklist: Vec<RawBlacklistEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawEntry {
    id: String,
    name: String,
    description: String,
    tier: String,
    author: String,
    version: String,
    api: String,
    url: String,
    sha256: String,
    permissions: Vec<String>,
    source_repo: String,
    commit: String,
    licence: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawBlacklistEntry {
    #[serde(default, deserialize_with = "present")]
    id: Option<String>,
    #[serde(default, deserialize_with = "present")]
    versions: Option<String>,
    #[serde(default, deserialize_with = "present")]
    sha256: Option<String>,
    action: String,
    reason: String,
    #[serde(default, deserialize_with = "present")]
    fixed_in: Option<String>,
}

fn parse_index(bytes: &[u8]) -> Result<Index, IndexError> {
    let raw: RawIndex = serde_json::from_slice(bytes)
        .map_err(|e| refuse(IndexRefusal::Json, format!("index.json is malformed ({e})")))?;
    if raw.format != INDEX_FORMAT {
        return Err(refuse(
            IndexRefusal::Format,
            format!("index.json is format {}, and this PM reads 1", raw.format),
        ));
    }
    if !(1..=MAX_SEQ).contains(&raw.seq) {
        return Err(field(format!("seq {} is out of range", raw.seq)));
    }
    let issued =
        parse_time(&raw.issued).ok_or_else(|| field(format!("issued {}", quote(&raw.issued))))?;
    let expires = parse_time(&raw.expires)
        .ok_or_else(|| field(format!("expires {}", quote(&raw.expires))))?;
    if expires <= issued {
        return Err(field("it expires before it was issued"));
    }
    if expires - issued > Duration::days(MAX_VALIDITY_DAYS) {
        return Err(field(format!(
            "it claims to stay good for more than {MAX_VALIDITY_DAYS} days"
        )));
    }

    if raw.revoked_key_ids.len() > MAX_REVOKED {
        return Err(field("it revokes too many keys"));
    }
    let mut revoked_key_ids: Vec<KeyId> = Vec::new();
    for s in &raw.revoked_key_ids {
        let id = KeyId::parse(s).ok_or_else(|| field(format!("key id {}", quote(s))))?;
        if revoked_key_ids.last().is_some_and(|last| *last >= id) {
            return Err(field("revokedKeyIds isn't ascending, or names a key twice"));
        }
        revoked_key_ids.push(id);
    }

    if raw.mods.len() > MAX_MODS {
        return Err(field("it lists too many mods"));
    }
    let mut mods: Vec<CatalogueEntry> = Vec::with_capacity(raw.mods.len());
    for r in raw.mods {
        let entry = parse_entry(r)?;
        if mods.last().is_some_and(|last| last.id >= entry.id) {
            return Err(field("mods isn't ascending by id, or lists a mod twice"));
        }
        mods.push(entry);
    }

    if raw.blacklist.len() > MAX_BLACKLIST {
        return Err(field("its blacklist is too long"));
    }
    let blacklist = raw
        .blacklist
        .into_iter()
        .map(parse_blacklist_entry)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Index {
        seq: raw.seq,
        issued,
        expires,
        revoked_key_ids,
        mods,
        blacklist,
    })
}

fn text(
    what: &str,
    s: &str,
    max: usize,
    newlines: bool,
    may_be_empty: bool,
) -> Result<(), IndexError> {
    package::plain_text_fault(s, max, newlines, may_be_empty)
        .map_err(|fault| field(format!("{what} {fault}")))
}

fn parse_entry(r: RawEntry) -> Result<CatalogueEntry, IndexError> {
    let id = ModId::parse(&r.id).map_err(|e| field(format!("mod {}: {e}", quote(&r.id))))?;
    let at = |what: &str| format!("{id}'s {what}");
    text(&at("name"), &r.name, 40, false, false)?;
    text(&at("description"), &r.description, 280, true, true)?;
    let tier = match Tier::parse(&r.tier) {
        Some(t @ (Tier::First | Tier::Second)) => t,
        _ => return Err(field(format!("{} {}", at("tier"), quote(&r.tier)))),
    };
    ids::check_tier(&id, tier).map_err(|e| field(format!("{id}: {e}")))?;
    text(&at("author"), &r.author, 64, false, false)?;
    let version = VersionName::parse(&r.version)
        .map_err(|_| field(format!("{} {}", at("version"), quote(&r.version))))?;
    let api = r
        .api
        .split_once('-')
        .and_then(|(min, target)| {
            let (min, target) = (
                package::parse_major_minor(min)?,
                package::parse_major_minor(target)?,
            );
            (min <= target).then_some(ApiRange { min, target })
        })
        .ok_or_else(|| field(format!("{} {}", at("api range"), quote(&r.api))))?;
    if r.url != package_url(&id, &version) {
        return Err(field(format!(
            "{} isn't where PM-Mods publishes that version",
            at("url")
        )));
    }
    if package::parse_sha256(&r.sha256).is_none() {
        return Err(field(at("sha256")));
    }
    let mut previous: Option<&str> = None;
    for p in &r.permissions {
        if !PERMISSIONS.contains(&p.as_str()) {
            return Err(field(format!("{} {}", at("permission"), quote(p))));
        }
        if previous.is_some_and(|last| last >= p.as_str()) {
            return Err(field(at("permissions aren't ascending, or repeat one")));
        }
        previous = Some(p);
    }
    if !package::is_source_repo(&r.source_repo) {
        return Err(field(at("sourceRepo")));
    }
    if !package::is_commit(&r.commit) {
        return Err(field(at("commit")));
    }
    if r.licence != LICENCE {
        return Err(field(format!("{} must be {LICENCE}", at("licence"))));
    }
    Ok(CatalogueEntry {
        id,
        name: r.name,
        description: r.description,
        tier,
        author: r.author,
        version,
        api,
        url: r.url,
        sha256: r.sha256,
        permissions: r.permissions,
        source_repo: r.source_repo,
        commit: r.commit,
    })
}

fn parse_blacklist_entry(r: RawBlacklistEntry) -> Result<BlacklistEntry, IndexError> {
    let target = match (r.id, r.versions, r.sha256) {
        (Some(id), Some(versions), None) => {
            let id = ModId::parse(&id)
                .map_err(|e| field(format!("blacklisted mod {}: {e}", quote(&id))))?;
            let range = VersionRange::parse(&versions).ok_or_else(|| {
                field(format!("{id}'s blacklisted versions {}", quote(&versions)))
            })?;
            BlacklistTarget::Versions { id, range }
        }
        (None, None, Some(sha)) => {
            if package::parse_sha256(&sha).is_none() {
                return Err(field(format!("blacklisted sha256 {}", quote(&sha))));
            }
            BlacklistTarget::Sha256(sha)
        }
        _ => {
            return Err(field(
                "a blacklist entry names an id with its versions, or a sha256, and not both",
            ))
        }
    };
    let action = match r.action.as_str() {
        "paused" => BlacklistAction::Paused,
        "removed" => BlacklistAction::Removed,
        other => return Err(field(format!("blacklist action {}", quote(other)))),
    };
    text("a blacklist reason", &r.reason, 280, false, false)?;
    let fixed_in = match r.fixed_in {
        None => None,
        Some(s) => {
            if action != BlacklistAction::Paused {
                return Err(field("only a pause names the version that fixes it"));
            }
            let name =
                VersionName::parse(&s).map_err(|_| field(format!("fixedIn {}", quote(&s))))?;
            if let BlacklistTarget::Versions { id, range } = &target {
                let v = semver::Version::parse(name.as_str())
                    .map_err(|_| field(format!("fixedIn {}", quote(&s))))?;
                if !range.lies_below(&v) {
                    return Err(field(format!(
                        "{id}'s fixedIn {} isn't above the versions it pauses",
                        quote(&s)
                    )));
                }
            }
            Some(name)
        }
    };
    Ok(BlacklistEntry {
        target,
        action,
        reason: r.reason,
        fixed_in,
    })
}

// ---- Acceptance (pure) --------------------------------------------------------------------------

/// What this machine already holds, as the rules need it.
#[derive(Debug, Clone, Copy)]
pub struct Seen<'a> {
    /// The highest seq accepted (0: none).
    pub seq: u64,
    /// That index's sha256, when this machine holds it.
    pub sha256: Option<&'a str>,
    /// The key ids this machine has seen revoked.
    pub revoked: &'a BTreeSet<KeyId>,
}

/// Why a verified index was still refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stale {
    /// Below the release's [`MIN_INDEX_SEQ`].
    BelowMinimum { seq: u64, minimum: u64 },
    /// Older than the one already seen: a replay.
    Older { seq: u64, seen: u64 },
    /// The seen seq with other bytes: two indexes signed with one seq.
    Equivocation { seq: u64 },
    /// Not a fault of the index: this machine's record of what it has seen
    /// ([`super::index_state`]) is there but can't be read or trusted, and no backup copy stands
    /// in for it. Without it the seq and the revoked keys are unknown, so no index is taken until
    /// the record is readable again; [`judge`] never returns this.
    RecordUnusable,
}

impl fmt::Display for Stale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RecordUnusable => f.write_str(
                "this machine's record of the safety list can't be read, so no index is taken until it can",
            ),
            Self::BelowMinimum { seq, minimum } => {
                write!(f, "seq {seq} is below this PM's minimum, {minimum}")
            }
            Self::Older { seq, seen } => {
                write!(f, "seq {seq} is older than seq {seen}, already seen")
            }
            Self::Equivocation { seq } => {
                write!(f, "seq {seq} was already seen with other contents")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Refused(Stale),
    /// The index already held: the same seq and bytes. A good fetch when it hasn't expired.
    Same {
        good: bool,
    },
    /// It replaces the one held. `good`: it hasn't expired, so it is a good fetch and its catalogue
    /// may be used; otherwise only its restrictions apply. `rotation`: it revokes a key this
    /// machine still trusted.
    Newer {
        good: bool,
        rotation: bool,
    },
}

impl Verdict {
    /// A good fetch (spec §1.10): verified, seq >= the seen one, and not expired.
    pub fn is_good(&self) -> bool {
        matches!(
            self,
            Self::Same { good: true } | Self::Newer { good: true, .. }
        )
    }
}

/// The acceptance rules (module doc), with the release's minimum passed in so tests can vary it.
pub fn judge(seen: &Seen<'_>, v: &VerifiedIndex, now: DateTime<Utc>, minimum: u64) -> Verdict {
    let seq = v.index.seq;
    if seq < minimum {
        return Verdict::Refused(Stale::BelowMinimum { seq, minimum });
    }
    let good = v.index.current_at(now);
    let rotation = !v.revokes.is_subset(seen.revoked);
    if rotation && good {
        // A rollover-signed revocation (only the rollover key may make one, `revocations`) of a key
        // this machine still trusts. Taken whatever the seen seq: a leaked main key may have signed
        // a seq so high nothing honest could follow it. A replay of it later revokes nothing new,
        // so it meets the ordinary rules.
        return Verdict::Newer {
            good: true,
            rotation: true,
        };
    }
    if seq < seen.seq {
        return Verdict::Refused(Stale::Older {
            seq,
            seen: seen.seq,
        });
    }
    if seq == seen.seq {
        return match seen.sha256 {
            Some(sha) if sha == v.sha256 => Verdict::Same { good },
            Some(_) => Verdict::Refused(Stale::Equivocation { seq }),
            // A seq on record with no sha256 (none of PM's own writes leaves one; a restore brings
            // its index pair, `index_state::Origin::Restored`): nothing to compare, and the index
            // is authentic, so it is the one.
            None => Verdict::Newer { good, rotation },
        };
    }
    Verdict::Newer { good, rotation }
}

/// How fresh the safety list is, for the Mods tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetyList {
    /// No good fetch ever (a fresh install that has been offline).
    NeverChecked,
    Checked(DateTime<Utc>),
    /// "Safety list not checked since …": no good fetch for [`STALE_AFTER_DAYS`].
    NotCheckedSince(DateTime<Utc>),
}

pub fn safety_list(last_good_fetch: Option<DateTime<Utc>>, now: DateTime<Utc>) -> SafetyList {
    match last_good_fetch {
        None => SafetyList::NeverChecked,
        Some(t) if now - t > Duration::days(STALE_AFTER_DAYS) => SafetyList::NotCheckedSince(t),
        Some(t) => SafetyList::Checked(t),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::mods::keys::{AtlasKey, KeyRing};
    use crate::mods::package::tests::TestKey;
    use serde_json::{json, Value};

    // ---- Fixtures: keys made for this run (no key is ever committed) ---------------------------

    /// A main and a rollover key made for the run, standing in for the Atlas pair.
    pub(crate) struct Keys {
        pub(crate) main: TestKey,
        pub(crate) rollover: TestKey,
    }

    impl Keys {
        pub(crate) fn new() -> Self {
            Self {
                main: TestKey::new(),
                rollover: TestKey::new(),
            }
        }

        pub(crate) fn ring(&self) -> KeyRing {
            KeyRing::of([
                atlas_key(KeyRole::Main, &self.main),
                atlas_key(KeyRole::Rollover, &self.rollover),
            ])
        }

        pub(crate) fn trusted(&self) -> TrustedKeys {
            self.ring().trusted(&BTreeSet::new())
        }

        pub(crate) fn main_id(&self) -> KeyId {
            KeyId::from_bytes(self.main.id)
        }

        pub(crate) fn rollover_id(&self) -> KeyId {
            KeyId::from_bytes(self.rollover.id)
        }
    }

    pub(crate) fn atlas_key(role: KeyRole, k: &TestKey) -> AtlasKey {
        AtlasKey {
            role,
            id: KeyId::from_bytes(k.id),
            key: k.public(),
        }
    }

    pub(crate) fn t(s: &str) -> DateTime<Utc> {
        parse_time(s).unwrap()
    }

    pub(crate) const SHA: &str = "a3f1c2d4e5b60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";
    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    pub(crate) fn good_entry() -> Value {
        json!({
            "id": "atlas.map",
            "name": "Map",
            "description": "Your documents, laid out by meaning.",
            "tier": "first",
            "author": "Atlas",
            "version": "1.2.0",
            "api": "1.0-1.0",
            "url": "https://github.com/itsAtlas-org/PM-Mods/releases/download/atlas.map-v1.2.0/atlas.map-1.2.0.pmmod",
            "sha256": SHA,
            "permissions": ["documents.meta", "documents.vectors", "ui.tab"],
            "sourceRepo": "itsAtlas-org/Personal-Manager",
            "commit": COMMIT,
            "licence": "AGPL-3.0-or-later"
        })
    }

    /// An index at `seq`, issued 2026-12-01, expiring 60 days later.
    pub(crate) fn good_index(seq: u64) -> Value {
        json!({
            "format": 1,
            "seq": seq,
            "issued": "2026-12-01T00:00:00Z",
            "expires": "2027-01-30T00:00:00Z",
            "revokedKeyIds": [],
            "mods": [good_entry()],
            "blacklist": [
                {"id": "acme.notes", "versions": ">=1.0.0, <1.2.0", "action": "paused",
                 "reason": "It can lose your notes on sync.", "fixedIn": "1.2.0"},
                {"sha256": SHA.replace('a', "b"), "action": "removed",
                 "reason": "It sends your documents elsewhere."}
            ]
        })
    }

    /// The comment the sign job writes for `index`, built by hand rather than by
    /// [`trusted_comment`], so a fixture can't agree with a broken builder. Tolerant of a broken
    /// fixture, which the parser is there to refuse.
    pub(crate) fn comment_of(index: &Value) -> String {
        let mut c = format!(
            "pmindex:1\tseq:{}\texpires:{}",
            index["seq"],
            index["expires"].as_str().unwrap_or_default()
        );
        for id in index["revokedKeyIds"].as_array().into_iter().flatten() {
            c.push_str(&format!("\trevoke:{}", id.as_str().unwrap_or_default()));
        }
        c
    }

    /// `index` as the sign job would publish it: its bytes and a signature by `key`.
    pub(crate) fn signed(index: &Value, key: &TestKey) -> (Vec<u8>, Vec<u8>) {
        let bytes = serde_json::to_vec_pretty(index).unwrap();
        let sig = key.sign(&bytes, &comment_of(index)).into_bytes();
        (bytes, sig)
    }

    fn verify(
        index: &Value,
        key: &TestKey,
        keys: &TrustedKeys,
    ) -> Result<VerifiedIndex, IndexError> {
        let (bytes, sig) = signed(index, key);
        verify_index(&bytes, &sig, keys)
    }

    fn refusal(r: Result<VerifiedIndex, IndexError>) -> IndexRefusal {
        match r {
            Ok(_) => panic!("the index was accepted"),
            Err(e) => e.refusal,
        }
    }

    /// `good_index(1)` with `change` applied, signed by the main key.
    fn refused_with(change: impl FnOnce(&mut Value)) -> IndexRefusal {
        let keys = Keys::new();
        let mut index = good_index(1);
        change(&mut index);
        refusal(verify(&index, &keys.main, &keys.trusted()))
    }

    // ---- What passes ---------------------------------------------------------------------------

    #[test]
    fn a_well_formed_signed_index_verifies() {
        let keys = Keys::new();
        let index = good_index(7);
        let (bytes, sig) = signed(&index, &keys.main);
        let v = verify_index(&bytes, &sig, &keys.trusted()).unwrap();
        assert_eq!(v.signer, keys.main_id());
        assert_eq!(v.signer_role, KeyRole::Main);
        assert_eq!(v.sha256, hex::encode(Sha256::digest(&bytes)));
        assert_eq!((v.bytes, v.signature), (bytes, sig));
        assert!(v.revokes.is_empty());
        let i = v.index;
        assert_eq!(i.seq, 7);
        assert_eq!(i.issued, t("2026-12-01T00:00:00Z"));
        assert_eq!(i.expires, t("2027-01-30T00:00:00Z"));
        let m = &i.mods[0];
        assert_eq!(m.id.as_str(), "atlas.map");
        assert_eq!((m.tier, m.version.as_str()), (Tier::First, "1.2.0"));
        assert_eq!(
            m.api,
            ApiRange {
                min: (1, 0),
                target: (1, 0)
            }
        );
        assert_eq!(
            m.permissions,
            ["documents.meta", "documents.vectors", "ui.tab"]
        );
        assert_eq!(i.blacklist.len(), 2);
        assert_eq!(i.blacklist[0].fixed_in.as_ref().unwrap().as_str(), "1.2.0");
        assert_eq!(
            i.blacklist[1].target,
            BlacklistTarget::Sha256(SHA.replace('a', "b"))
        );
        // Whitespace and field order are the writer's choice: the bytes signed are what count.
        let compact = serde_json::to_vec(&good_index(7)).unwrap();
        let sig = keys.main.sign(&compact, &comment_of(&good_index(7)));
        assert!(verify_index(&compact, sig.as_bytes(), &keys.trusted()).is_ok());
        // Either trusted key may sign, and an empty index is an index.
        let mut empty = good_index(8);
        empty["mods"] = json!([]);
        empty["blacklist"] = json!([]);
        let v = verify(&empty, &keys.rollover, &keys.trusted()).unwrap();
        assert_eq!(v.signer_role, KeyRole::Rollover);
    }

    /// The vector the PM-Mods sign job is checked against: these exact bytes, signed with this exact
    /// trusted comment, are what PM reads. Any JSON formatting is allowed; the comment is not.
    #[test]
    fn the_index_format_and_its_trusted_comment_are_pinned() {
        const VECTOR: &str = concat!(
            r#"{"format":1,"seq":42,"issued":"2027-03-01T09:30:00Z","expires":"2027-04-30T09:30:00Z","#,
            r#""revokedKeyIds":["95B83457064D517B"],"#,
            r#""mods":[{"id":"atlas.map","name":"Map","description":"Your documents, laid out by meaning.","#,
            r#""tier":"first","author":"Atlas","version":"1.0.0","api":"1.0-1.0","#,
            r#""url":"https://github.com/itsAtlas-org/PM-Mods/releases/download/atlas.map-v1.0.0/atlas.map-1.0.0.pmmod","#,
            r#""sha256":"0000000000000000000000000000000000000000000000000000000000000000","#,
            r#""permissions":["documents.meta","documents.vectors","ui.tab"],"#,
            r#""sourceRepo":"itsAtlas-org/Personal-Manager","commit":"0123456789abcdef0123456789abcdef01234567","#,
            r#""licence":"AGPL-3.0-or-later"}],"#,
            r#""blacklist":[{"id":"acme.notes","versions":">=1.0.0, <1.2.0","action":"paused","#,
            r#""reason":"It can lose your notes on sync.","fixedIn":"1.2.0"},"#,
            r#"{"sha256":"1111111111111111111111111111111111111111111111111111111111111111","#,
            r#""action":"removed","reason":"It sends your documents elsewhere."}]}"#
        );
        const COMMENT: &str =
            "pmindex:1\tseq:42\texpires:2027-04-30T09:30:00Z\trevoke:95B83457064D517B";

        let index = parse_index(VECTOR.as_bytes()).unwrap();
        assert_eq!(trusted_comment(&index), COMMENT);
        assert_eq!(index.seq, 42);
        assert_eq!(
            index.revoked_key_ids,
            [KeyId::parse("95B83457064D517B").unwrap()]
        );
        assert_eq!(
            index.mods[0].url,
            package_url(&index.mods[0].id, &index.mods[0].version)
        );

        // Signed by a stand-in rollover key, beside a stand-in main key that carries the real main
        // key's id, so the vector's revocation names a key this test ring holds.
        let main = TestKey {
            id: [0x7b, 0x51, 0x4d, 0x06, 0x57, 0x34, 0xb8, 0x95],
            ..TestKey::new()
        };
        let rollover = TestKey::new();
        let ring = KeyRing::of([
            atlas_key(KeyRole::Main, &main),
            atlas_key(KeyRole::Rollover, &rollover),
        ]);
        let sig = rollover.sign(VECTOR.as_bytes(), COMMENT);
        let v = verify_index(
            VECTOR.as_bytes(),
            sig.as_bytes(),
            &ring.trusted(&BTreeSet::new()),
        )
        .unwrap();
        assert_eq!(v.revokes, BTreeSet::from([KeyId::from_bytes(main.id)]));
        assert_eq!(KeyId::from_bytes(main.id).to_string(), "95B83457064D517B");

        // With nothing revoked, which is every index before a rotation: three fields.
        let plain = VECTOR.replace(
            r#""revokedKeyIds":["95B83457064D517B"]"#,
            r#""revokedKeyIds":[]"#,
        );
        assert_ne!(plain, VECTOR);
        assert_eq!(
            trusted_comment(&parse_index(plain.as_bytes()).unwrap()),
            "pmindex:1\tseq:42\texpires:2027-04-30T09:30:00Z"
        );
    }

    #[test]
    fn the_published_urls_are_pinned() {
        assert_eq!(
            INDEX_URL,
            format!("{RELEASE_DOWNLOAD_PREFIX}{INDEX_TAG}/{INDEX_FILE}")
        );
        assert_eq!(
            INDEX_SIGNATURE_URL,
            format!("{RELEASE_DOWNLOAD_PREFIX}{INDEX_TAG}/{INDEX_SIGNATURE_FILE}")
        );
        assert_eq!(
            RELEASE_DOWNLOAD_PREFIX,
            format!("https://github.com/{PM_MODS_REPO}/releases/download/")
        );
        let id = ModId::parse("acme.tools").unwrap();
        let v = VersionName::parse("2.0.0-rc.1").unwrap();
        assert_eq!(
            package_url(&id, &v),
            "https://github.com/itsAtlas-org/PM-Mods/releases/download/acme.tools-v2.0.0-rc.1/acme.tools-2.0.0-rc.1.pmmod"
        );
        for host in ASSET_REDIRECT_HOSTS {
            assert!(host.ends_with(".githubusercontent.com"));
        }
    }

    // ---- Signatures ----------------------------------------------------------------------------

    #[test]
    fn a_signature_by_a_key_pm_doesnt_trust_is_refused() {
        let keys = Keys::new();
        let stranger = TestKey::new();
        assert_eq!(
            refusal(verify(&good_index(1), &stranger, &keys.trusted())),
            IndexRefusal::UntrustedKey
        );
        // A forger who copies a trusted key's id.
        let forger = TestKey {
            id: keys.main.id,
            ..TestKey::new()
        };
        assert_eq!(
            refusal(verify(&good_index(1), &forger, &keys.trusted())),
            IndexRefusal::BadSignature
        );
    }

    #[test]
    fn a_revoked_key_is_refused() {
        let keys = Keys::new();
        let after = keys.ring().trusted(&BTreeSet::from([keys.main_id()]));
        assert_eq!(
            refusal(verify(&good_index(5), &keys.main, &after)),
            IndexRefusal::UntrustedKey
        );
        assert!(verify(&good_index(5), &keys.rollover, &after).is_ok());
    }

    #[test]
    fn tampered_bytes_or_comment_break_the_signature() {
        let keys = Keys::new();
        let (bytes, sig) = signed(&good_index(3), &keys.main);
        let mut other = bytes.clone();
        let at = other.len() - 3;
        other[at] ^= 1;
        assert_eq!(
            refusal(verify_index(&other, &sig, &keys.trusted())),
            IndexRefusal::BadSignature
        );
        let edited = String::from_utf8(sig.clone())
            .unwrap()
            .replace("seq:3", "seq:4");
        assert_eq!(
            refusal(verify_index(&bytes, edited.as_bytes(), &keys.trusted())),
            IndexRefusal::BadSignature
        );
    }

    #[test]
    fn a_legacy_or_malformed_signature_is_refused() {
        let keys = Keys::new();
        let index = good_index(1);
        let bytes = serde_json::to_vec(&index).unwrap();
        let legacy = keys.main.sign_legacy(&bytes, &comment_of(&index));
        assert_eq!(
            refusal(verify_index(&bytes, legacy.as_bytes(), &keys.trusted())),
            IndexRefusal::Legacy
        );
        for garbage in [&b"not a signature"[..], &[0xff, 0xfe], b""] {
            assert_eq!(
                refusal(verify_index(&bytes, garbage, &keys.trusted())),
                IndexRefusal::Signature
            );
        }
        let mut five_lines = keys.main.sign(&bytes, &comment_of(&index));
        five_lines.push_str("trailing\n");
        assert_eq!(
            refusal(verify_index(&bytes, five_lines.as_bytes(), &keys.trusted())),
            IndexRefusal::Signature
        );
    }

    #[test]
    fn oversized_files_are_refused_before_anything_else() {
        let keys = Keys::new();
        let (bytes, sig) = signed(&good_index(1), &keys.main);
        let mut big = bytes.clone();
        big.resize(INDEX_CAP + 1, b' ');
        assert_eq!(
            refusal(verify_index(&big, &sig, &keys.trusted())),
            IndexRefusal::TooLarge
        );
        let mut big_sig = sig.clone();
        big_sig.resize(SIGNATURE_CAP + 1, b'\n');
        assert_eq!(
            refusal(verify_index(&bytes, &big_sig, &keys.trusted())),
            IndexRefusal::TooLarge
        );
        // At the cap exactly, the size rule passes (and the padding, being JSON whitespace, is fine).
        let mut padded = bytes.clone();
        padded.resize(INDEX_CAP, b' ');
        let sig = keys.main.sign(&padded, &comment_of(&good_index(1)));
        assert!(verify_index(&padded, sig.as_bytes(), &keys.trusted()).is_ok());
    }

    // ---- The trusted comment -------------------------------------------------------------------

    #[test]
    fn the_trusted_comment_must_be_an_index_s_and_agree_with_the_json() {
        let keys = Keys::new();
        let index = good_index(9);
        let bytes = serde_json::to_vec(&index).unwrap();
        let signed_with = |comment: &str| {
            let sig = keys.main.sign(&bytes, comment);
            refusal(verify_index(&bytes, sig.as_bytes(), &keys.trusted()))
        };
        let good = comment_of(&index);
        // A mod's signature, or any other kind, isn't an index's.
        assert_eq!(
            signed_with("pmmod:1\tid:atlas.map"),
            IndexRefusal::TrustedComment
        );
        for not_an_index in [
            good.replace("pmindex:1", "pmindex:2"),
            good.replace("pmindex:1", "pmindex"),
            good.replace('\t', " "),
            "timestamp:1700000000".to_owned(),
            String::new(),
        ] {
            assert_eq!(
                signed_with(&not_an_index),
                IndexRefusal::TrustedComment,
                "{not_an_index:?}"
            );
        }
        // Each field must equal the JSON's, exactly.
        for wrong in [
            good.replace("seq:9", "seq:10"),
            good.replace("seq:9", "seq:09"),
            good.replace("2027-01-30T00:00:00Z", "2027-01-31T00:00:00Z"),
            format!("{good}\trevoke:95B83457064D517B"),
            format!("{good}\t"),
            format!("{good} "),
            good.replace("\texpires", "\tissued:2026-12-01T00:00:00Z\texpires"),
        ] {
            assert_eq!(
                signed_with(&wrong),
                IndexRefusal::CommentMismatch,
                "{wrong:?}"
            );
        }
    }

    #[test]
    fn a_revocation_s_comment_lists_it_too() {
        let keys = Keys::new();
        let mut index = good_index(2);
        index["revokedKeyIds"] = json!([keys.main_id().to_string()]);
        let bytes = serde_json::to_vec(&index).unwrap();
        // Signed without the revoke field: refused.
        let sig = keys
            .rollover
            .sign(&bytes, "pmindex:1\tseq:2\texpires:2027-01-30T00:00:00Z");
        assert_eq!(
            refusal(verify_index(&bytes, sig.as_bytes(), &keys.trusted())),
            IndexRefusal::CommentMismatch
        );
        let sig = keys.rollover.sign(&bytes, &comment_of(&index));
        assert!(verify_index(&bytes, sig.as_bytes(), &keys.trusted()).is_ok());
    }

    // ---- Revocations ---------------------------------------------------------------------------

    #[test]
    fn only_the_rollover_key_revokes_and_nothing_revokes_it() {
        let keys = Keys::new();
        let revoking = |ids: Vec<String>| {
            let mut index = good_index(4);
            index["revokedKeyIds"] = json!(ids);
            index
        };
        let main = keys.main_id().to_string();
        let rollover = keys.rollover_id().to_string();

        let v = verify(
            &revoking(vec![main.clone()]),
            &keys.rollover,
            &keys.trusted(),
        )
        .unwrap();
        assert_eq!(v.revokes, BTreeSet::from([keys.main_id()]));
        // The main key can't revoke anything it holds, itself included.
        assert_eq!(
            refusal(verify(
                &revoking(vec![main.clone()]),
                &keys.main,
                &keys.trusted()
            )),
            IndexRefusal::Revocation
        );
        // Nobody revokes the rollover key.
        for signer in [&keys.main, &keys.rollover] {
            assert_eq!(
                refusal(verify(
                    &revoking(vec![rollover.clone()]),
                    signer,
                    &keys.trusted()
                )),
                IndexRefusal::Revocation
            );
        }
        // A key this PM never held is ignored, whoever signed.
        let unknown = "0123456789ABCDEF".to_owned();
        let v = verify(
            &revoking(vec![unknown.clone()]),
            &keys.main,
            &keys.trusted(),
        )
        .unwrap();
        assert!(v.revokes.is_empty());
        // A key already revoked is still the rollover's to name (every later index carries it).
        let after = keys.ring().trusted(&BTreeSet::from([keys.main_id()]));
        let mut both = vec![main, unknown];
        both.sort();
        let v = verify(&revoking(both), &keys.rollover, &after).unwrap();
        assert_eq!(v.revokes, BTreeSet::from([keys.main_id()]));
    }

    #[test]
    fn a_dev_test_key_signs_but_never_revokes() {
        let keys = Keys::new();
        let dev = TestKey::new();
        let ring = KeyRing::of([
            atlas_key(KeyRole::Main, &keys.main),
            atlas_key(KeyRole::Rollover, &keys.rollover),
            atlas_key(KeyRole::DevTest, &dev),
        ]);
        let trusted = ring.trusted(&BTreeSet::new());
        assert_eq!(
            verify(&good_index(1), &dev, &trusted).unwrap().signer_role,
            KeyRole::DevTest
        );
        let mut index = good_index(1);
        index["revokedKeyIds"] = json!([keys.main_id().to_string()]);
        assert_eq!(
            refusal(verify(&index, &dev, &trusted)),
            IndexRefusal::Revocation
        );
    }

    // ---- Strict parsing ------------------------------------------------------------------------

    #[test]
    fn malformed_json_and_unknown_or_missing_fields_are_refused() {
        let keys = Keys::new();
        for bytes in [&b"not json"[..], b"[]", b"{}", b"", b"{\"format\":1"] {
            let sig = keys
                .main
                .sign(bytes, "pmindex:1\tseq:1\texpires:2027-01-30T00:00:00Z");
            assert_eq!(
                refusal(verify_index(bytes, sig.as_bytes(), &keys.trusted())),
                IndexRefusal::Json,
                "{bytes:?}"
            );
        }
        assert_eq!(refused_with(|i| i["extra"] = json!(1)), IndexRefusal::Json);
        assert_eq!(
            refused_with(|i| i["mods"][0]["extra"] = json!(1)),
            IndexRefusal::Json
        );
        assert_eq!(
            refused_with(|i| i["blacklist"][0]["fixed_in"] = json!("1.2.0")),
            IndexRefusal::Json
        );
        for required in [
            "format",
            "seq",
            "issued",
            "expires",
            "revokedKeyIds",
            "mods",
            "blacklist",
        ] {
            assert_eq!(
                refused_with(|i| {
                    i.as_object_mut().unwrap().remove(required);
                }),
                IndexRefusal::Json,
                "{required}"
            );
        }
        assert_eq!(
            refused_with(|i| {
                i["mods"][0].as_object_mut().unwrap().remove("licence");
            }),
            IndexRefusal::Json
        );
        // A null where a value goes, and an optional field set to null.
        assert_eq!(
            refused_with(|i| i["mods"] = Value::Null),
            IndexRefusal::Json
        );
        assert_eq!(
            refused_with(|i| i["blacklist"][0]["fixedIn"] = Value::Null),
            IndexRefusal::Json
        );
        // A key written twice.
        let keys = Keys::new();
        let text = serde_json::to_string(&good_index(1)).unwrap();
        let twice = text.replacen("\"seq\":1", "\"seq\":1,\"seq\":1", 1);
        let sig = keys
            .main
            .sign(twice.as_bytes(), &comment_of(&good_index(1)));
        assert_eq!(
            refusal(verify_index(
                twice.as_bytes(),
                sig.as_bytes(),
                &keys.trusted()
            )),
            IndexRefusal::Json
        );
    }

    #[test]
    fn only_format_1_is_read() {
        assert_eq!(
            refused_with(|i| i["format"] = json!(2)),
            IndexRefusal::Format
        );
        assert_eq!(
            refused_with(|i| i["format"] = json!(0)),
            IndexRefusal::Format
        );
        assert_eq!(
            refused_with(|i| i["format"] = json!("1")),
            IndexRefusal::Json
        );
    }

    #[test]
    fn seq_is_a_whole_number_in_range() {
        let keys = Keys::new();
        let sign_seq = |seq: Value| {
            let mut index = good_index(1);
            index["seq"] = seq.clone();
            let bytes = serde_json::to_vec(&index).unwrap();
            let comment = format!("pmindex:1\tseq:{seq}\texpires:2027-01-30T00:00:00Z");
            let sig = keys.main.sign(&bytes, &comment);
            refusal(verify_index(&bytes, sig.as_bytes(), &keys.trusted()))
        };
        assert_eq!(sign_seq(json!(0)), IndexRefusal::Field);
        assert_eq!(sign_seq(json!(MAX_SEQ + 1)), IndexRefusal::Field);
        assert_eq!(sign_seq(json!(-1)), IndexRefusal::Json);
        assert_eq!(sign_seq(json!(1.5)), IndexRefusal::Json);
        assert_eq!(sign_seq(json!("1")), IndexRefusal::Json);
        let mut top = good_index(MAX_SEQ);
        top["mods"] = json!([]);
        assert!(verify(&top, &keys.main, &keys.trusted()).is_ok());
    }

    #[test]
    fn times_have_one_spelling_and_a_bounded_life() {
        for bad in [
            "2027-01-30T00:00:00+00:00",
            "2027-01-30T00:00:00.000Z",
            "2027-01-30 00:00:00Z",
            "2027-01-30t00:00:00z",
            "2027-1-30T00:00:00Z",
            "2027-02-30T00:00:00Z",
            "2027-01-30T24:00:00Z",
            "2016-12-31T23:59:60Z",
            "+2027-01-30T00:00:00",
            "",
        ] {
            assert_eq!(parse_time(bad), None, "{bad:?}");
            assert_eq!(
                refused_with(|i| i["issued"] = json!(bad)),
                IndexRefusal::Field,
                "{bad:?}"
            );
        }
        assert_eq!(time_text(t("2027-01-30T00:00:00Z")), "2027-01-30T00:00:00Z");
        // Expiry after issue, within the cap.
        assert_eq!(
            refused_with(|i| i["expires"] = json!("2026-12-01T00:00:00Z")),
            IndexRefusal::Field
        );
        assert_eq!(
            refused_with(|i| i["issued"] = json!("2027-02-01T00:00:00Z")),
            IndexRefusal::Field
        );
        let keys = Keys::new();
        let mut longest = good_index(1);
        longest["expires"] = json!("2027-03-31T00:00:00Z"); // 120 days after issue
        assert!(verify(&longest, &keys.main, &keys.trusted()).is_ok());
        longest["expires"] = json!("2027-03-31T00:00:01Z");
        assert_eq!(
            refusal(verify(&longest, &keys.main, &keys.trusted())),
            IndexRefusal::Field
        );
    }

    #[test]
    fn revoked_key_ids_are_well_formed_ascending_and_few() {
        let keys = Keys::new();
        let with = |ids: Value| {
            let mut index = good_index(1);
            index["revokedKeyIds"] = ids;
            refusal(verify(&index, &keys.rollover, &keys.trusted()))
        };
        assert_eq!(with(json!(["95b83457064d517b"])), IndexRefusal::Field);
        assert_eq!(with(json!(["95B83457064D517"])), IndexRefusal::Field);
        assert_eq!(
            with(json!(["B000000000000000", "A000000000000000"])),
            IndexRefusal::Field
        );
        assert_eq!(
            with(json!(["A000000000000000", "A000000000000000"])),
            IndexRefusal::Field
        );
        let many: Vec<String> = (1..=MAX_REVOKED as u64 + 1)
            .map(|n| format!("{n:016X}"))
            .collect();
        assert_eq!(with(json!(many)), IndexRefusal::Field);
        assert_eq!(with(json!([1])), IndexRefusal::Json);
    }

    #[test]
    fn catalogue_entries_are_checked_field_by_field() {
        let entry = |key: &str, value: Value| refused_with(|i| i["mods"][0][key] = value);
        let url = |s: &str| json!(format!("https://{s}"));
        for (what, key, value) in [
            ("id grammar", "id", json!("Atlas.Map")),
            ("device id", "id", json!("acme.con")),
            ("empty name", "name", json!("")),
            ("long name", "name", json!("x".repeat(41))),
            ("hidden name", "name", json!("Map\u{202E}")),
            ("padded author", "author", json!(" Atlas")),
            ("long description", "description", json!("x".repeat(281))),
            ("third party", "tier", json!("third")),
            ("unknown tier", "tier", json!("zeroth")),
            ("second party atlas", "tier", json!("second")),
            ("upper version", "version", json!("1.2.0-RC1")),
            ("build metadata", "version", json!("1.2.0+b1")),
            ("v prefix", "version", json!("v1.2.0")),
            ("api shape", "api", json!("1.0")),
            ("api order", "api", json!("1.2-1.0")),
            ("api zeros", "api", json!("01.0-1.0")),
            ("http", "url", json!("http://github.com/itsAtlas-org/PM-Mods/releases/download/atlas.map-v1.2.0/atlas.map-1.2.0.pmmod")),
            ("other spelling", "url", url("github.com/itsAtlas-org/pm-mods/releases/download/atlas.map-v1.2.0/atlas.map-1.2.0.pmmod")),
            ("latest", "url", url("github.com/itsAtlas-org/PM-Mods/releases/latest/download/atlas.map-1.2.0.pmmod")),
            ("other version", "url", url("github.com/itsAtlas-org/PM-Mods/releases/download/atlas.map-v1.1.0/atlas.map-1.1.0.pmmod")),
            ("query", "url", url("github.com/itsAtlas-org/PM-Mods/releases/download/atlas.map-v1.2.0/atlas.map-1.2.0.pmmod?x=1")),
            ("other host", "url", url("evil.example/itsAtlas-org/PM-Mods/releases/download/atlas.map-v1.2.0/atlas.map-1.2.0.pmmod")),
            ("upper sha", "sha256", json!(SHA.to_uppercase())),
            ("short sha", "sha256", json!(&SHA[1..])),
            ("unknown permission", "permissions", json!(["ui.tab", "everything"])),
            ("unsorted permissions", "permissions", json!(["ui.tab", "documents.meta"])),
            ("repeated permission", "permissions", json!(["ui.tab", "ui.tab"])),
            ("source repo", "sourceRepo", json!("itsAtlas-org")),
            ("commit", "commit", json!(COMMIT.to_uppercase())),
            ("licence", "licence", json!("MIT")),
        ] {
            assert_eq!(entry(key, value), IndexRefusal::Field, "{what}");
        }
        // Wrong JSON types are the parser's.
        assert_eq!(entry("permissions", json!("ui.tab")), IndexRefusal::Json);
        // A second-party entry outside atlas. passes.
        let keys = Keys::new();
        let mut index = good_index(1);
        let e = &mut index["mods"][0];
        e["id"] = json!("acme.tools");
        e["tier"] = json!("second");
        e["author"] = json!("Acme");
        e["permissions"] = json!([]);
        e["url"] = json!("https://github.com/itsAtlas-org/PM-Mods/releases/download/acme.tools-v1.2.0/acme.tools-1.2.0.pmmod");
        assert!(verify(&index, &keys.main, &keys.trusted()).is_ok());
    }

    #[test]
    fn the_catalogue_is_ascending_unique_and_bounded() {
        let mut second = good_entry();
        second["id"] = json!("atlas.pinboard");
        second["url"] = json!("https://github.com/itsAtlas-org/PM-Mods/releases/download/atlas.pinboard-v1.2.0/atlas.pinboard-1.2.0.pmmod");
        let (a, b) = (good_entry(), second);
        assert_eq!(
            refused_with(|i| i["mods"] = json!([b.clone(), a.clone()])),
            IndexRefusal::Field
        );
        assert_eq!(
            refused_with(|i| i["mods"] = json!([a.clone(), a.clone()])),
            IndexRefusal::Field
        );
        let keys = Keys::new();
        let mut index = good_index(1);
        index["mods"] = json!([a, b]);
        let v = verify(&index, &keys.main, &keys.trusted()).unwrap();
        assert!(v.index.entry("atlas.pinboard").is_some());
        assert!(v.index.entry("atlas.nothing").is_none());
        // The counts are capped before any entry is read.
        let too_many: Vec<Value> = (0..=MAX_MODS).map(|_| good_entry()).collect();
        assert_eq!(
            refused_with(|i| i["mods"] = json!(too_many)),
            IndexRefusal::Field
        );
        let entry = good_index(1)["blacklist"][1].clone();
        let too_long: Vec<Value> = (0..=MAX_BLACKLIST).map(|_| entry.clone()).collect();
        assert_eq!(
            refused_with(|i| i["blacklist"] = json!(too_long)),
            IndexRefusal::Field
        );
        let at_cap: Vec<Value> = (0..MAX_BLACKLIST).map(|_| entry.clone()).collect();
        let mut index = good_index(1);
        index["blacklist"] = json!(at_cap);
        assert!(verify(&index, &keys.main, &keys.trusted()).is_ok());
    }

    #[test]
    fn blacklist_entries_are_one_shape_each() {
        let entry = |e: Value| refused_with(|i| i["blacklist"] = json!([e]));
        let sha = SHA;
        for (what, bad) in [
            (
                "id without versions",
                json!({"id": "acme.x", "action": "paused", "reason": "r"}),
            ),
            (
                "versions without id",
                json!({"versions": "*", "action": "paused", "reason": "r"}),
            ),
            (
                "both kinds",
                json!({"id": "acme.x", "versions": "*", "sha256": sha, "action": "paused", "reason": "r"}),
            ),
            (
                "id and sha",
                json!({"id": "acme.x", "sha256": sha, "action": "paused", "reason": "r"}),
            ),
            ("neither", json!({"action": "paused", "reason": "r"})),
            (
                "bad id",
                json!({"id": "ACME.x", "versions": "*", "action": "paused", "reason": "r"}),
            ),
            (
                "bad sha",
                json!({"sha256": "abc", "action": "paused", "reason": "r"}),
            ),
            (
                "action",
                json!({"sha256": sha, "action": "deleted", "reason": "r"}),
            ),
            (
                "empty reason",
                json!({"sha256": sha, "action": "paused", "reason": ""}),
            ),
            (
                "two-line reason",
                json!({"sha256": sha, "action": "paused", "reason": "a\nb"}),
            ),
            (
                "long reason",
                json!({"sha256": sha, "action": "paused", "reason": "r".repeat(281)}),
            ),
            (
                "removed with a fix",
                json!({"sha256": sha, "action": "removed", "reason": "r", "fixedIn": "2.0.0"}),
            ),
            (
                "bad fix",
                json!({"sha256": sha, "action": "paused", "reason": "r", "fixedIn": "2.0"}),
            ),
            (
                "fix inside",
                json!({"id": "acme.x", "versions": ">=1.0.0, <2.0.0", "action": "paused", "reason": "r", "fixedIn": "1.5.0"}),
            ),
            (
                "fix below",
                json!({"id": "acme.x", "versions": ">=1.0.0, <2.0.0", "action": "paused", "reason": "r", "fixedIn": "0.9.0"}),
            ),
            (
                "fix at an inclusive top",
                json!({"id": "acme.x", "versions": "<=2.0.0", "action": "paused", "reason": "r", "fixedIn": "2.0.0"}),
            ),
            (
                "fix with no top",
                json!({"id": "acme.x", "versions": ">=1.0.0", "action": "paused", "reason": "r", "fixedIn": "3.0.0"}),
            ),
            (
                "fix of everything",
                json!({"id": "acme.x", "versions": "*", "action": "paused", "reason": "r", "fixedIn": "3.0.0"}),
            ),
        ] {
            assert_eq!(entry(bad), IndexRefusal::Field, "{what}");
        }
        let keys = Keys::new();
        for good in [
            json!({"id": "acme.x", "versions": "<2.0.0", "action": "paused", "reason": "r", "fixedIn": "2.0.0"}),
            json!({"id": "acme.x", "versions": "=1.0.0", "action": "removed", "reason": "r"}),
            json!({"id": "acme.x", "versions": "*", "action": "removed", "reason": "r"}),
            json!({"sha256": sha, "action": "paused", "reason": "r", "fixedIn": "2.0.0"}),
        ] {
            let mut index = good_index(1);
            index["blacklist"] = json!([good.clone()]);
            assert!(
                verify(&index, &keys.main, &keys.trusted()).is_ok(),
                "{good}"
            );
        }
    }

    #[test]
    fn version_ranges_have_a_small_strict_grammar() {
        for bad in [
            "",
            " ",
            "1.0.0",
            "^1.0.0",
            "~1.0.0",
            "1.*",
            ">=1.0",
            ">=1.0.0,<2.0.0",
            "<2.0.0, >=1.0.0",
            ">=2.0.0, <1.0.0",
            ">=1.0.0, <1.0.0",
            ">=1.0.0, >1.1.0",
            "<1.0.0, <2.0.0",
            ">=1.0.0, <2.0.0, <3.0.0",
            "=1.0.0, <2.0.0",
            ">= 1.0.0",
            ">=V1.0.0",
            ">=1.0.0-RC1",
            ">=1.0.0+b",
            "* ",
            "**",
        ] {
            assert_eq!(VersionRange::parse(bad), None, "{bad:?}");
            assert_eq!(
                refused_with(|i| i["blacklist"][0]["versions"] = json!(bad)),
                IndexRefusal::Field,
                "{bad:?}"
            );
        }
        let v = |s: &str| semver::Version::parse(s).unwrap();
        let r = VersionRange::parse(">=1.0.0, <1.2.0").unwrap();
        assert!(r.contains(&v("1.0.0")) && r.contains(&v("1.1.9")) && r.contains(&v("1.2.0-rc.1")));
        assert!(
            !r.contains(&v("0.9.9")) && !r.contains(&v("1.2.0")) && !r.contains(&v("1.0.0-rc.1"))
        );
        // Unlike Cargo's `*`, every version, prereleases included.
        let all = VersionRange::parse("*").unwrap();
        assert!(all.contains(&v("0.0.1-alpha")) && all.contains(&v("99.0.0")));
        let one = VersionRange::parse("=1.1.0").unwrap();
        assert!(
            one.contains(&v("1.1.0"))
                && !one.contains(&v("1.1.1"))
                && !one.contains(&v("1.1.0-rc.1"))
        );
        let open = VersionRange::parse(">1.0.0").unwrap();
        assert!(!open.contains(&v("1.0.0")) && open.contains(&v("1.0.1-alpha")));
        let upto = VersionRange::parse("<=2.0.0").unwrap();
        assert!(upto.contains(&v("2.0.0")) && !upto.contains(&v("2.0.1")));
        assert!(VersionRange::parse(">1.0.0, <=1.0.1").is_some());
    }

    #[test]
    fn the_blacklist_matches_by_version_or_sha_and_removal_wins() {
        let keys = Keys::new();
        let mut index = good_index(1);
        let bad_sha = SHA.replace('a', "b");
        index["blacklist"] = json!([
            {"id": "acme.notes", "versions": ">=1.0.0, <1.2.0", "action": "paused", "reason": "pause", "fixedIn": "1.2.0"},
            {"id": "acme.notes", "versions": "=1.1.0", "action": "removed", "reason": "remove"},
            {"sha256": bad_sha, "action": "removed", "reason": "file"},
        ]);
        let i = verify(&index, &keys.main, &keys.trusted()).unwrap().index;
        let reason = |id: &str, v: &str, sha: Option<&str>| {
            i.blacklisted(id, v, sha).map(|e| e.reason.as_str())
        };
        assert_eq!(reason("acme.notes", "1.0.0", None), Some("pause"));
        assert_eq!(reason("acme.notes", "1.2.0-rc.1", None), Some("pause"));
        assert_eq!(reason("acme.notes", "1.1.0", None), Some("remove"));
        assert_eq!(reason("acme.notes", "1.2.0", None), None);
        assert_eq!(reason("acme.notes", "0.9.0", None), None);
        assert_eq!(reason("acme.other", "1.1.0", None), None);
        // By file, whatever it claims to be.
        assert_eq!(reason("anyone.else", "9.9.9", Some(&bad_sha)), Some("file"));
        assert_eq!(reason("anyone.else", "9.9.9", Some(SHA)), None);
        assert_eq!(reason("acme.notes", "1.0.0", Some(&bad_sha)), Some("file"));
        // An unreadable version never dodges an entry for its id.
        assert_eq!(reason("acme.notes", "not a version", None), Some("remove"));
        assert_eq!(reason("acme.other", "not a version", None), None);
    }

    // ---- Acceptance ----------------------------------------------------------------------------

    pub(crate) fn verified_by(index: &Value, key: &TestKey, keys: &Keys) -> VerifiedIndex {
        let (bytes, sig) = signed(index, key);
        verify_index(&bytes, &sig, &keys.ring().trusted(&BTreeSet::new())).unwrap()
    }

    const BEFORE_EXPIRY: &str = "2027-01-15T00:00:00Z";

    fn seen<'a>(seq: u64, sha256: Option<&'a str>, revoked: &'a BTreeSet<KeyId>) -> Seen<'a> {
        Seen {
            seq,
            sha256,
            revoked,
        }
    }

    #[test]
    fn acceptance_follows_the_seq() {
        let keys = Keys::new();
        let none = BTreeSet::new();
        let v5 = verified_by(&good_index(5), &keys.main, &keys);
        let now = t(BEFORE_EXPIRY);
        let seen = |seq, sha| seen(seq, sha, &none);

        // Nothing seen yet: anything at or above the minimum.
        assert_eq!(
            judge(&seen(0, None), &v5, now, 1),
            Verdict::Newer {
                good: true,
                rotation: false
            }
        );
        assert_eq!(
            judge(&seen(0, None), &v5, now, 6),
            Verdict::Refused(Stale::BelowMinimum { seq: 5, minimum: 6 })
        );
        assert_eq!(
            judge(&seen(0, None), &v5, now, 5),
            Verdict::Newer {
                good: true,
                rotation: false
            }
        );
        // Newer, same, older.
        assert_eq!(
            judge(&seen(4, Some("x")), &v5, now, 1),
            Verdict::Newer {
                good: true,
                rotation: false
            }
        );
        assert_eq!(
            judge(&seen(5, Some(&v5.sha256)), &v5, now, 1),
            Verdict::Same { good: true }
        );
        assert_eq!(
            judge(&seen(6, Some("x")), &v5, now, 1),
            Verdict::Refused(Stale::Older { seq: 5, seen: 6 })
        );
        // The same seq with other bytes.
        assert_eq!(
            judge(&seen(5, Some("other")), &v5, now, 1),
            Verdict::Refused(Stale::Equivocation { seq: 5 })
        );
        // A seq on record with no sha256 to compare: the authentic index at that seq is taken.
        assert_eq!(
            judge(&seen(5, None), &v5, now, 1),
            Verdict::Newer {
                good: true,
                rotation: false
            }
        );
        assert!(Verdict::Same { good: true }.is_good());
        assert!(!Verdict::Same { good: false }.is_good());
        assert!(!Verdict::Refused(Stale::Equivocation { seq: 1 }).is_good());
    }

    #[test]
    fn an_expired_index_restricts_but_is_no_good_fetch() {
        let keys = Keys::new();
        let none = BTreeSet::new();
        let v = verified_by(&good_index(5), &keys.main, &keys);
        let seen = Seen {
            seq: 4,
            sha256: Some("x"),
            revoked: &none,
        };
        // Expires 2027-01-30T00:00:00Z; two days of skew are allowed, to 2027-02-01T00:00:00Z.
        assert!(judge(&seen, &v, t("2027-01-29T23:59:59Z"), 1).is_good());
        assert!(judge(&seen, &v, t("2027-01-31T23:59:59Z"), 1).is_good());
        assert_eq!(
            judge(&seen, &v, t("2027-02-01T00:00:00Z"), 1),
            Verdict::Newer {
                good: false,
                rotation: false
            }
        );
        assert_eq!(
            judge(
                &Seen {
                    seq: 5,
                    sha256: Some(&v.sha256),
                    revoked: &none
                },
                &v,
                t("2027-03-01T00:00:00Z"),
                1
            ),
            Verdict::Same { good: false }
        );
        // An expired older one is still a replay.
        assert_eq!(
            judge(&Seen { seq: 6, ..seen }, &v, t("2027-03-01T00:00:00Z"), 1),
            Verdict::Refused(Stale::Older { seq: 5, seen: 6 })
        );
        assert!(v.index.current_at(t("2027-01-31T23:59:59Z")));
        assert!(!v.index.current_at(t("2027-02-01T00:00:00Z")));
    }

    #[test]
    fn a_rotation_is_taken_whatever_the_seen_seq_but_only_once() {
        let keys = Keys::new();
        let none = BTreeSet::new();
        let mut rotation = good_index(21);
        rotation["revokedKeyIds"] = json!([keys.main_id().to_string()]);
        let r = verified_by(&rotation, &keys.rollover, &keys);
        let now = t(BEFORE_EXPIRY);
        // A leaked main key signed seq 2^53 - 1; the rollover's revocation still lands.
        let pumped = Seen {
            seq: MAX_SEQ,
            sha256: Some("x"),
            revoked: &none,
        };
        assert_eq!(
            judge(&pumped, &r, now, 1),
            Verdict::Newer {
                good: true,
                rotation: true
            }
        );
        // The same seq with other bytes, too.
        assert_eq!(
            judge(
                &Seen {
                    seq: 21,
                    sha256: Some("x"),
                    revoked: &none
                },
                &r,
                now,
                1
            ),
            Verdict::Newer {
                good: true,
                rotation: true
            }
        );
        // Once the key is revoked here, the same index is an ordinary one: a replay of it can't
        // lower the floor again.
        let revoked = BTreeSet::from([keys.main_id()]);
        assert_eq!(
            judge(
                &Seen {
                    seq: 30,
                    sha256: Some("x"),
                    revoked: &revoked
                },
                &r,
                now,
                1
            ),
            Verdict::Refused(Stale::Older { seq: 21, seen: 30 })
        );
        assert_eq!(
            judge(
                &Seen {
                    seq: 21,
                    sha256: Some(&r.sha256),
                    revoked: &revoked
                },
                &r,
                now,
                1
            ),
            Verdict::Same { good: true }
        );
        // Still never below the release's minimum.
        assert_eq!(
            judge(&pumped, &r, now, 22),
            Verdict::Refused(Stale::BelowMinimum {
                seq: 21,
                minimum: 22
            })
        );
        // An expired rotation meets the ordinary rules: newer is taken for what it restricts.
        let late = t("2027-03-01T00:00:00Z");
        assert_eq!(
            judge(&pumped, &r, late, 1),
            Verdict::Refused(Stale::Older {
                seq: 21,
                seen: MAX_SEQ
            })
        );
        assert_eq!(
            judge(
                &Seen {
                    seq: 20,
                    sha256: Some("x"),
                    revoked: &none
                },
                &r,
                late,
                1
            ),
            Verdict::Newer {
                good: false,
                rotation: true
            }
        );
    }

    #[test]
    fn the_safety_list_goes_stale_after_thirty_days() {
        let checked = t("2027-01-01T00:00:00Z");
        assert_eq!(safety_list(None, checked), SafetyList::NeverChecked);
        assert_eq!(
            safety_list(Some(checked), checked + Duration::days(30)),
            SafetyList::Checked(checked)
        );
        assert_eq!(
            safety_list(
                Some(checked),
                checked + Duration::days(30) + Duration::seconds(1)
            ),
            SafetyList::NotCheckedSince(checked)
        );
        // A clock that went back reads as checked.
        assert_eq!(
            safety_list(Some(checked), checked - Duration::days(3)),
            SafetyList::Checked(checked)
        );
    }
}
