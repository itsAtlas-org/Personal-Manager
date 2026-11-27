// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The providers a person can reach with their own key, and everything PM stores about them.
//!
//! One table row per provider ([`PROVIDERS`]); the Gemini and Anthropic PRs append a row, a match
//! arm in [`check_key`] and a catalogue section, and the commands never name a provider module.
//!
//! All of it lives in the existing `settings` key/value table: no migration, no new table. Per
//! provider: whether the person opted in (off by default, every provider separately), the one model
//! and the effort level each role uses on it, the last key-check verdict, and the person's own
//! "my project shares data" tick-box. Per role, in `llm_gateway`: which provider answers it.
//!
//! Every one of those rows is DEVICE-class (SYNC-SET.md): a backup and a shared vault carry them
//! verbatim, and they prove nothing about this device. Nothing leaves the machine without a key typed
//! on THIS device, so the guards are at read: a stored verdict is shown only beside a present key,
//! [`crate::llm_gateway::role_cloud_provider`] demotes a stored direct provider that isn't opted in,
//! and a role on a keyless provider says so.
//!
//! A provider can also have TIERS (Gemini: a free key lets Google train on what you send, a paid one
//! doesn't). The tier is a second axis beside the check's outcome ([`KeyTier`]), it is bound to ONE key
//! by a fingerprint stored with it ([`key_fp`]), and it can be learned passively from a failed call
//! ([`note_tier_seen`]) without the error path ever touching the database.
//!
//! Two lock rules. The keychain is read BEFORE the DB guard is taken (on macOS a keychain read can
//! wait on a prompt), and nothing here holds a guard across an `.await`. The in-memory inbox
//! ([`SEEN`]) is taken after the DB guard, never before, and never across an `.await`.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use rusqlite::Connection;

use crate::direct_models::{self, DirectModel};
use crate::error::{Error, Result};
use crate::llm_gateway::{
    cloud_provider_key, role_cloud_provider, stored_role_provider, CloudProvider, KeyPresence, Role,
};
use crate::power::{DirectConsent, DIRECT_CONSENT_KEY};
use crate::region::{Basis, Region, RegionRefusal};
use crate::secret::Secret;
use crate::{db, gemini_direct, openai_direct, secrets};

/// The settings keys one provider keeps for one role.
pub(crate) struct RoleKeys {
    /// A JSON array of catalogue ids. One entry in this version.
    pub models: &'static str,
    /// A level string; absent is the role's default.
    pub effort: &'static str,
}

/// One provider PM can call with the person's own key.
pub(crate) struct DirectProvider {
    pub provider: CloudProvider,
    /// As a person names it.
    pub name: &'static str,
    /// `"true"` or absent. Off by default.
    pub opt_in: &'static str,
    pub chat: RoleKeys,
    pub background: RoleKeys,
    /// The last key check's verdict, JSON.
    pub key_check: &'static str,
    /// `"true"` or absent: the person says their project shares data with the provider. Display only.
    /// `None` for a provider that has no such tick-box (Gemini detects its tier instead).
    pub shares_data: Option<&'static str>,
    /// Whether the provider has tiers whose terms differ (a verdict from the key check, bound to the
    /// key). OpenAI has none.
    pub tiers: bool,
    pub get_key: fn() -> Result<Option<Secret>>,
    pub set_key: fn(&str) -> Result<()>,
    pub clear_key: fn() -> Result<()>,
    /// Whether a pasted key is the right shape to try.
    pub well_formed: fn(&str) -> bool,
}

impl DirectProvider {
    /// The keys for a role. Exhaustive, so a third role fails to compile here.
    pub(crate) fn role(&self, role: Role) -> &RoleKeys {
        match role {
            Role::Chat => &self.chat,
            Role::Background => &self.background,
        }
    }
}

const OPENAI: DirectProvider = DirectProvider {
    provider: CloudProvider::OpenAi,
    name: "OpenAI",
    opt_in: "direct_openai_enabled",
    chat: RoleKeys {
        models: "direct_openai_chat_models",
        effort: "direct_openai_chat_effort",
    },
    background: RoleKeys {
        models: "direct_openai_background_models",
        effort: "direct_openai_background_effort",
    },
    key_check: "direct_openai_key_check",
    shares_data: Some("direct_openai_shares_data"),
    tiers: false,
    get_key: secrets::get_openai_key,
    set_key: secrets::set_openai_key,
    clear_key: secrets::clear_openai_key,
    well_formed: openai_direct::well_formed,
};

const GEMINI: DirectProvider = DirectProvider {
    provider: CloudProvider::Gemini,
    name: "Google Gemini",
    opt_in: "direct_gemini_enabled",
    chat: RoleKeys {
        models: "direct_gemini_chat_models",
        effort: "direct_gemini_chat_effort",
    },
    background: RoleKeys {
        models: "direct_gemini_background_models",
        effort: "direct_gemini_background_effort",
    },
    key_check: "direct_gemini_key_check",
    shares_data: None,
    tiers: true,
    get_key: secrets::get_gemini_key,
    set_key: secrets::set_gemini_key,
    clear_key: secrets::clear_gemini_key,
    well_formed: gemini_direct::well_formed,
};

/// Every direct provider. The Anthropic PR appends a row.
pub(crate) const PROVIDERS: &[DirectProvider] = &[OPENAI, GEMINI];

/// The table row for a provider, `None` for OpenRouter and for a provider with no call yet.
pub(crate) fn provider(p: CloudProvider) -> Option<&'static DirectProvider> {
    PROVIDERS.iter().find(|d| d.provider == p)
}

fn row(p: CloudProvider) -> Result<&'static DirectProvider> {
    provider(p).ok_or_else(|| Error::Other("unknown provider".into()))
}

// ---- the key check -----------------------------------------------------------------------------

/// What a key check found. OPEN: Gemini adds tier and region variants, so a reader must show an
/// unknown word as itself and never fail on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KeyOutcome {
    /// The provider accepted the key.
    Ok,
    /// Valid, but it can't list models or can't chat. Saved anyway; see [`KeyNote`].
    Restricted,
    /// Valid and listing, but the account has no credit or is at its spend limit, so every chat
    /// fails until credit is added. Saved: the key is fine, and Check again clears it.
    NoCredit,
    /// The provider says the key isn't right.
    Rejected,
    /// The provider doesn't serve this country or region.
    RegionBlocked,
    /// Not the shape of a key. Never sent.
    Malformed,
    /// PM couldn't reach the provider to ask.
    Unreachable,
    /// The provider answered and decided nothing: a rate limit, a 5xx or a timeout from Google. Not
    /// kept, and a Check again that gets it stores nothing. Never "check your connection": the
    /// connection was fine.
    Busy,
    /// Google wants the project verified before the key can be used. Not kept.
    NeedsVerification,
    /// The DECISION, made by the command and never by a provider: a valid free or unknown-tier key,
    /// refused because of where this computer is. Never kept, never stored.
    FreeKeyRefused,
    /// Any other status, as `{"unexpected": 418}`.
    Unexpected(u16),
}

impl KeyOutcome {
    /// Whether the key is kept: a mistyped key is never stored.
    pub(crate) fn keeps_key(self) -> bool {
        matches!(
            self,
            KeyOutcome::Ok | KeyOutcome::Restricted | KeyOutcome::NoCredit
        )
    }
}

/// On whose terms a key is. A second axis beside [`KeyOutcome`]: "the key works" and "the key is
/// free" are separate facts that happen together. UNKNOWN READS AS FREE everywhere (Bobby,
/// 2026-10-22): in the refusal, the warning, the chip and the cost rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyTier {
    Paid,
    Free,
    /// The check couldn't tell.
    Unknown,
}

impl KeyTier {
    pub(crate) fn reads_as_free(self) -> bool {
        !matches!(self, KeyTier::Paid)
    }
}

/// The terms a role's route is on, for the places that name the route and its terms (the chip, the
/// footer, the role block and the On-battery question; the context meter only names the provider).
/// Unknown and "no verdict" fold into `Free`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RouteTerms {
    Paid,
    Free,
    /// A free or unknown key where PM only uses paid ones: the role is paused.
    Blocked,
}

/// A key's fingerprint: the first 16 hex characters of SHA-256 over a versioned prefix and the key. It
/// binds a stored verdict to ONE key, because the verdict row is device-class and travels in a backup
/// or a shared vault: a paid verdict carried beside a different (free) key must not apply to it. The
/// row sits in the encrypted store, and 64 bits of a digest of a 39-character key is not the key; the
/// webview never receives it.
pub(crate) fn key_fp(key: &str) -> String {
    let digest = Sha256::digest(format!("pm-key-fp-v1:{key}").as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// An extra word about a verdict, for the cases the outcome alone can't say. A newer PM's word
/// makes the stored verdict unreadable (shown as none), the same as an unknown outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KeyNote {
    /// The test chat couldn't be tried (a rate limit, a 5xx, a timeout, a model that's gone), so
    /// the model listing's verdict stands.
    ChatUntried,
    /// The key lists models but OpenAI refuses its chat: a read-only key.
    ReadOnly,
    /// The key can neither list models nor chat.
    NoAccess,
}

/// A key check's verdict plus what the key can see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeyCheck {
    pub outcome: KeyOutcome,
    /// The catalogue ids (OpenRouter-style) the key can see.
    pub listed: Vec<String>,
    /// How many models the provider listed, PM-known or not.
    pub listed_total: usize,
    pub note: Option<KeyNote>,
    /// On whose terms the key is. `None` for a provider with no tiers, and for an answer that says
    /// nothing about the key's terms.
    pub tier: Option<KeyTier>,
    /// Set by [`apply_region_rule`] alone, with `FreeKeyRefused`.
    pub refusal: Option<RegionRefusal>,
}

impl KeyCheck {
    pub(crate) fn of(outcome: KeyOutcome) -> Self {
        Self {
            outcome,
            listed: Vec::new(),
            listed_total: 0,
            note: None,
            tier: None,
            refusal: None,
        }
    }

    /// An outcome with the tier it implies (a 402 is a paid key, a region block a free one).
    pub(crate) fn of_tier(outcome: KeyOutcome, tier: KeyTier) -> Self {
        Self {
            tier: Some(tier),
            ..Self::of(outcome)
        }
    }

    /// For a provider whose check isn't built yet. Unreachable through the commands, which refuse a
    /// provider with no table row.
    fn unsupported() -> Self {
        Self::of(KeyOutcome::Unexpected(0))
    }
}

/// Check a key against its provider. ONE exhaustive match: a new provider fails to compile until it
/// has an arm, and no boxed futures are needed.
pub(crate) async fn check_key(p: CloudProvider, key: &str) -> KeyCheck {
    match p {
        CloudProvider::OpenAi => openai_direct::check_key(key).await,
        CloudProvider::Gemini => gemini_direct::check_key(key).await,
        CloudProvider::OpenRouter | CloudProvider::Anthropic => KeyCheck::unsupported(),
    }
}

/// Whether a check's answer, in this place, is a free (or unknown) key PM refuses to keep. PURE. An
/// exhaustive match on the provider: identity for every provider without tiers, so OpenAI is never
/// refused anywhere. For Gemini: a key that works (or that Google wants verified, which is a free key)
/// whose tier reads as free, in a restricted or unplaceable region, becomes `FreeKeyRefused` with the
/// models cleared. A paid key, a 402 (a paid key with no balance), a busy answer and any rejection pass
/// through unchanged.
pub(crate) fn apply_region_rule(p: CloudProvider, check: KeyCheck, region: &Region) -> KeyCheck {
    match p {
        CloudProvider::Gemini => {
            let candidate = matches!(
                check.outcome,
                KeyOutcome::Ok | KeyOutcome::NeedsVerification
            );
            let free = check.tier.is_none_or(KeyTier::reads_as_free);
            match region.refusal(basis_of(check.tier)) {
                Some(refusal) if candidate && free => KeyCheck {
                    outcome: KeyOutcome::FreeKeyRefused,
                    listed: Vec::new(),
                    listed_total: 0,
                    note: None,
                    tier: check.tier,
                    refusal: Some(refusal),
                },
                _ => check,
            }
        }
        CloudProvider::OpenAi | CloudProvider::OpenRouter | CloudProvider::Anthropic => check,
    }
}

fn basis_of(tier: Option<KeyTier>) -> Basis {
    if tier == Some(KeyTier::Free) {
        Basis::Free
    } else {
        Basis::Unknown
    }
}

/// The refusal a key with this verdict gets here, if any. PURE; exhaustive on the provider. Only
/// Gemini has a rule: a verdict that reads as free (a missing one included) where PM only uses paid
/// keys. This is the one rule the save, the state read, the role pick and every send consult.
pub(crate) fn refuses(
    p: CloudProvider,
    tier: Option<KeyTier>,
    region: &Region,
) -> Option<RegionRefusal> {
    match p {
        CloudProvider::Gemini => {
            if tier.is_none_or(KeyTier::reads_as_free) {
                region.refusal(basis_of(tier))
            } else {
                None
            }
        }
        CloudProvider::OpenAi | CloudProvider::OpenRouter | CloudProvider::Anthropic => None,
    }
}

/// Whether a cloud arm is on a paused provider right now: judged by the latest word about its key
/// (a call or a check since the plan was built), else the tier the arm was stamped with. PURE with
/// the region given. The On battery policy asks it so it never moves a role onto a provider the send
/// seam would refuse (the status already says such a role stays local).
pub(crate) fn arm_paused_in(arm: &crate::llm_gateway::CloudArm, region: &Region) -> bool {
    if !provider(arm.provider).is_some_and(|d| d.tiers) {
        return false;
    }
    let tier = latest_tier(arm.provider, &key_fp(arm.key.expose())).or(arm.tier);
    refuses(arm.provider, tier, region).is_some()
}

/// [`arm_paused_in`] where this computer is. The time zone is read only for a provider with tiers.
pub(crate) fn arm_paused(arm: &crate::llm_gateway::CloudArm) -> bool {
    provider(arm.provider).is_some_and(|d| d.tiers) && arm_paused_in(arm, &crate::region::assumed())
}

/// A key for another company is never sent to this one. No prefix rule is documented for either, so
/// PM still never REQUIRES a prefix; it only refuses the one that is plainly someone else's: `sk-`
/// is OpenAI's, OpenRouter's and Anthropic's, `AIza` is Google's. Pure.
pub(crate) fn looks_like_another_providers_key(p: CloudProvider, key: &str) -> bool {
    let key = key.trim();
    match p {
        CloudProvider::Gemini => key.starts_with("sk-"),
        CloudProvider::OpenAi => key.starts_with("AIza"),
        CloudProvider::OpenRouter | CloudProvider::Anthropic => false,
    }
}

/// The last verdict as stored: when, what, and what the key could see.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct StoredKeyCheck {
    pub at: String,
    pub outcome: KeyOutcome,
    pub listed: Vec<String>,
    pub listed_total: usize,
    /// Absent in a verdict stored before the test chat existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<KeyNote>,
    /// On whose terms the key was, when the check was made (or when a later call proved otherwise).
    /// Absent for a provider without tiers and in a verdict stored before tiers existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<KeyTier>,
    /// When a failed call told PM the project is on the free tier after the check said paid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flipped_free_at: Option<String>,
    /// [`key_fp`] of the key this verdict is about. The verdict counts only beside a key whose
    /// fingerprint matches. Blanked before the row goes to the webview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_fp: Option<String>,
}

/// Remember a verdict about the key whose fingerprint is `fp`. `at` comes from the open connection,
/// never a helper that would take the DB guard again. For a provider with tiers the verdict also
/// becomes the LATEST thing PM knows about this key ([`record_verdict`]), so a fresh check reaches a
/// plan that is already running.
pub(crate) fn store_key_check(
    conn: &Connection,
    p: CloudProvider,
    check: &KeyCheck,
    fp: &str,
) -> Result<()> {
    let d = row(p)?;
    let prev = if d.tiers {
        read_key_check(conn, p)?
    } else {
        None
    };
    // A rejection decides nothing about the project's tier (a rejected key never reached the probe),
    // so it must not erase what an earlier check of THIS key proved: a transient or misread 403
    // would otherwise cost a paid key its proof and pause it in the region until the next check.
    let kept = if d.tiers && check.tier.is_none() && check.outcome == KeyOutcome::Rejected {
        prev.clone().filter(|s| s.key_fp.as_deref() == Some(fp))
    } else {
        None
    };
    let stored = StoredKeyCheck {
        at: crate::ingest::iso_now(conn)?,
        outcome: check.outcome,
        listed: check.listed.clone(),
        listed_total: check.listed_total,
        note: check.note,
        tier: check.tier.or(kept.as_ref().and_then(|s| s.tier)),
        flipped_free_at: kept.and_then(|s| s.flipped_free_at),
        key_fp: d.tiers.then(|| fp.to_string()),
    };
    let json = serde_json::to_string(&stored).map_err(|e| Error::Other(e.to_string()))?;
    // The yes goes BEFORE the row is written: a crash between the two leaves a paid row and no yes
    // (it asks again), never a free row with a yes that was given under paid terms.
    forget_yes_on_terms_drop(conn, p, prev.and_then(|s| s.tier), stored.tier)?;
    db::set_setting(conn, d.key_check, &json)?;
    if d.tiers {
        if let Some(tier) = check.tier {
            record_verdict(p, fp, tier);
        }
    }
    Ok(())
}

pub(crate) fn clear_key_check(conn: &Connection, p: CloudProvider) -> Result<()> {
    forget_seen(p);
    db::delete_setting(conn, row(p)?.key_check)
}

/// The stored verdict, `None` when there is none or it can't be read (a newer PM's outcome word).
pub(crate) fn read_key_check(
    conn: &Connection,
    p: CloudProvider,
) -> Result<Option<StoredKeyCheck>> {
    Ok(db::get_setting(conn, row(p)?.key_check)?
        .and_then(|raw| serde_json::from_str::<StoredKeyCheck>(&raw).ok()))
}

/// How the keychain answers for a provider, and for a provider with tiers the fingerprint of the key
/// that is there, from ONE read and WITHOUT the DB guard. A read that fails is `Unreadable`, never
/// `Absent`: the key may well be there. The fingerprint is `None` unless the key is present (and
/// nothing else needs it).
pub(crate) fn key_state_of(d: &DirectProvider) -> (KeyPresence, Option<String>) {
    match (d.get_key)() {
        Ok(Some(k)) => (KeyPresence::Present, d.tiers.then(|| key_fp(k.expose()))),
        Ok(None) => (KeyPresence::Absent, None),
        Err(_) => (KeyPresence::Unreadable, None),
    }
}

// ---- what PM has learned about a key since it was checked --------------------------------------------

/// What this process knows about one provider's key beyond the stored row.
#[derive(Default, Clone)]
struct Seen {
    /// The fingerprint every field below speaks for. A note about another key is ignored.
    key_fp: Option<String>,
    /// The latest thing learned about THIS key: a fresh check (any verdict, "couldn't tell" included),
    /// a free-tier answer, or a success on a paid-only model. A held plan reads it at every send, so
    /// it must carry Paid and Unknown as well as Free: a plan stamped Paid has to be stopped by a
    /// check that came back Unknown, and a plan stamped Free has to stop pricing at $0.00 once a
    /// check says Paid.
    latest: Option<KeyTier>,
    /// A verdict learned from a call, not yet written to the stored row.
    pending: Option<KeyTier>,
    /// Per model: do not ask Google again before this moment (a spent daily free allowance).
    not_before: Vec<(String, Instant)>,
}

/// The inbox. A failed call can't hold the DB guard or the key (`Flavour::request_error` is a plain
/// function), so it only leaves a note here, and the entry points that hold the arm pass it on. A
/// plain `Vec` because `CloudProvider` has no `Hash`. Never held across an `.await`, never held while
/// taking the DB guard; the DB guard is always taken first.
static SEEN: Mutex<Vec<(CloudProvider, Seen)>> = Mutex::new(Vec::new());

/// Every test that touches [`SEEN`] (or `gemini_direct`'s free mark, which feeds it) holds this, so
/// two of them never read each other's notes: the tests run on parallel threads and the inbox is
/// process-wide by design.
#[cfg(test)]
pub(crate) static INBOX_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Waits shorter than this are the rate limit they say they are, not a spent allowance.
const LOCAL_WAIT_FLOOR_SECS: u64 = 300;

/// Run `f` on one provider's entry (made if absent). A poisoned lock is read through: a panic
/// elsewhere must not wedge the send path.
fn with_seen<R>(p: CloudProvider, f: impl FnOnce(&mut Seen) -> R) -> R {
    let mut all = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = all.iter().position(|(q, _)| *q == p) {
        return f(&mut all[i].1);
    }
    all.push((p, Seen::default()));
    let last = all.len() - 1;
    f(&mut all[last].1)
}

/// A failed call proved the project is on the free tier (a limit-0 429 on a no-free model, or a real
/// exhausted free quota). Only Free is ever noted; a note about another key's fingerprint is ignored,
/// so an old key's late failure can't flip a new key. A wait of 5 minutes or more also sets a local
/// not-before for that model, so a background tick doesn't ask every few seconds.
pub(crate) fn note_tier_seen(
    p: CloudProvider,
    tier: KeyTier,
    fp: &str,
    wire: &str,
    wait_secs: Option<u64>,
) {
    if tier != KeyTier::Free {
        return;
    }
    with_seen(p, |s| {
        if s.key_fp.as_deref().is_some_and(|other| other != fp) {
            return;
        }
        s.key_fp = Some(fp.to_string());
        s.latest = Some(KeyTier::Free);
        s.pending = Some(KeyTier::Free);
        if let Some(w) = wait_secs.filter(|w| *w >= LOCAL_WAIT_FLOOR_SECS) {
            let until = Instant::now() + Duration::from_secs(w);
            s.not_before.retain(|(m, _)| m != wire);
            s.not_before.push((wire.to_string(), until));
        }
    });
}

/// A call succeeded on a model only paid keys can use: proof the key is paid. Applies to this
/// fingerprint (or to an inbox that has none yet).
pub(crate) fn note_paid_proven(p: CloudProvider, fp: &str) {
    with_seen(p, |s| {
        if s.key_fp.as_deref().is_some_and(|other| other != fp) {
            return;
        }
        s.key_fp = Some(fp.to_string());
        s.latest = Some(KeyTier::Paid);
        s.pending = Some(KeyTier::Paid);
        s.not_before.clear();
    });
}

/// A fresh check is the latest verdict as much as a 429 is, so "Check again" reaches a plan that is
/// already running: a Paid verdict clears a flip, a Free one sets it.
pub(crate) fn record_verdict(p: CloudProvider, fp: &str, tier: KeyTier) {
    with_seen(p, |s| {
        s.key_fp = Some(fp.to_string());
        s.latest = Some(tier);
        s.pending = None;
        s.not_before.clear();
    });
}

/// Whether the latest thing learned about THIS key is that it is free.
#[cfg(test)]
pub(crate) fn effective_free(p: CloudProvider, fp: &str) -> bool {
    latest_tier(p, fp) == Some(KeyTier::Free)
}

/// The latest verdict this process holds for THIS key, whatever it is, or `None` when nothing was
/// learned since the process started (the caller then falls back to the tier its arm was stamped
/// with). The send seam judges by this, so a held plan follows the newest word in both directions.
pub(crate) fn latest_tier(p: CloudProvider, fp: &str) -> Option<KeyTier> {
    with_seen(p, |s| s.latest.filter(|_| s.key_fp.as_deref() == Some(fp)))
}

/// How long until PM may ask Google again about this model, if it said not to before then.
pub(crate) fn not_before(p: CloudProvider, wire: &str) -> Option<Duration> {
    with_seen(p, |s| {
        s.not_before
            .iter()
            .find(|(m, _)| m == wire)
            .and_then(|(_, at)| at.checked_duration_since(Instant::now()))
            .filter(|d| !d.is_zero())
    })
}

/// The key was removed.
pub(crate) fn forget_seen(p: CloudProvider) {
    let mut all = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    all.retain(|(q, _)| *q != p);
}

/// The vault was locked or replaced: nothing learned about a key survives into another vault.
pub(crate) fn forget_all_seen() {
    SEEN.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// Write what calls learned onto the stored rows. The caller already holds the DB guard and nothing
/// here awaits or takes another lock for long. A note is written only onto a row that exists AND is
/// about the same key; otherwise it stays in memory (the view reads it through [`tier_now`]). Free
/// stamps `flipped_free_at`; Paid clears it. Only `pending` is cleared afterwards: `free` is what the
/// send seam reads.
pub(crate) fn apply_seen(conn: &Connection) -> Result<()> {
    let todo: Vec<(CloudProvider, String, KeyTier)> = {
        let all = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        all.iter()
            .filter_map(|(p, s)| Some((*p, s.key_fp.clone()?, s.pending?)))
            .collect()
    };
    for (p, fp, tier) in todo {
        if let Some(mut stored) = read_key_check(conn, p)? {
            if stored.key_fp.as_deref() == Some(fp.as_str()) && stored.tier != Some(tier) {
                let old = stored.tier;
                stored.tier = Some(tier);
                stored.flipped_free_at = if tier == KeyTier::Free {
                    Some(crate::ingest::iso_now(conn)?)
                } else {
                    None
                };
                let json =
                    serde_json::to_string(&stored).map_err(|e| Error::Other(e.to_string()))?;
                // Yes first, row second: see `store_key_check`.
                forget_yes_on_terms_drop(conn, p, old, Some(tier))?;
                db::set_setting(conn, row(p)?.key_check, &json)?;
            }
        }
        with_seen(p, |s| {
            if s.pending == Some(tier) && s.key_fp.as_deref() == Some(fp.as_str()) {
                s.pending = None;
            }
        });
    }
    Ok(())
}

/// What PM currently believes about this key's tier: the in-memory latest (a seen-free note or a fresh
/// check) when it is about this key, else the stored tier when the stored fingerprint is this key's,
/// else `None`: no usable verdict, which reads as free and says "PM hasn't checked this key". Reads
/// only; the caller holds the guard.
pub(crate) fn tier_now(
    conn: &Connection,
    p: CloudProvider,
    fp: Option<&str>,
) -> Result<Option<KeyTier>> {
    let Some(fp) = fp else {
        return Ok(None);
    };
    if let Some(t) = latest_tier(p, fp) {
        return Ok(Some(t));
    }
    Ok(read_key_check(conn, p)?
        .filter(|s| s.key_fp.as_deref() == Some(fp))
        .and_then(|s| s.tier))
}

/// The tier an arm is stamped with: the stored one, but ONLY when it was stored beside THIS key. A
/// verdict carried in a backup or a shared vault (a paid row beside a different, free key) says nothing
/// about a key typed on this device, so the arm gets `None`, which reads as free. Pure; `cloud_arm`
/// calls it after its keychain read, so no second guard is needed.
pub(crate) fn tier_for_key(
    stored_tier: Option<KeyTier>,
    stored_fp: Option<&str>,
    key: &str,
) -> Option<KeyTier> {
    stored_tier.filter(|_| stored_fp == Some(key_fp(key).as_str()))
}

/// What the terms of the two roles' routes need, read BEFORE the DB guard (a keychain read can wait on
/// a prompt on macOS): the fingerprint of the present key of each provider that has tiers, and where
/// this computer is. Nothing is read, no keychain and no time zone, unless one of the roles is ON a
/// provider with tiers: a person on OpenRouter or OpenAI never pays for Gemini here, and the
/// [`Region::Outside`] left in its place is harmless because [`route_terms`] answers `None` for a
/// provider without tiers.
pub(crate) fn tier_context(
    chat: CloudProvider,
    background: CloudProvider,
) -> (Vec<(CloudProvider, Option<String>)>, Region) {
    let mut fps: Vec<(CloudProvider, Option<String>)> = Vec::new();
    for p in [chat, background] {
        let Some(d) = provider(p).filter(|d| d.tiers) else {
            continue;
        };
        if !fps.iter().any(|(q, _)| *q == p) {
            fps.push((p, key_state_of(d).1));
        }
    }
    if fps.is_empty() {
        return (fps, Region::Outside);
    }
    (fps, crate::region::assumed())
}

/// [`route_terms`] for a provider whose fingerprint is in the list [`tier_context`] returned.
pub(crate) fn route_terms_of(
    conn: &Connection,
    p: CloudProvider,
    region: &Region,
    fps: &[(CloudProvider, Option<String>)],
) -> Result<Option<RouteTerms>> {
    let fp = fps
        .iter()
        .find(|(q, _)| *q == p)
        .and_then(|(_, f)| f.as_deref());
    route_terms(conn, p, region, fp)
}

/// The terms the role's route is on, for a provider with tiers: `None` otherwise, and `None` for a
/// provider with tiers whose key isn't here (`fp` is `None`): a role with no key is "no key", which
/// readiness already says, and must not read as "paused" in a restricted region (nothing is sent, so
/// no terms apply).
pub(crate) fn route_terms(
    conn: &Connection,
    p: CloudProvider,
    region: &Region,
    fp: Option<&str>,
) -> Result<Option<RouteTerms>> {
    if !provider(p).is_some_and(|d| d.tiers) || fp.is_none() {
        return Ok(None);
    }
    let tier = tier_now(conn, p, fp)?;
    Ok(Some(if refuses(p, tier, region).is_some() {
        RouteTerms::Blocked
    } else if tier == Some(KeyTier::Paid) {
        RouteTerms::Paid
    } else {
        RouteTerms::Free
    }))
}

// ---- opt-in, models, effort ---------------------------------------------------------------------

/// Whether the person has turned this provider on. Off by default, and off for a provider PM has no
/// row for.
pub(crate) fn opted_in(conn: &Connection, p: CloudProvider) -> Result<bool> {
    match provider(p) {
        Some(d) => Ok(db::get_setting(conn, d.opt_in)?.as_deref() == Some("true")),
        None => Ok(false),
    }
}

/// The models a role has on a provider: the stored ids PM still lists (an unknown id, or one past
/// its announced shutdown date, is dropped), else the catalogue's default for the role. NEVER empty,
/// so a role first switched to a provider never shows an empty list and PM needs no seeding write.
pub(crate) fn models_for(conn: &Connection, p: CloudProvider, role: Role) -> Result<Vec<String>> {
    let d = row(p)?;
    let stored: Vec<String> = db::get_setting(conn, d.role(role).models)?
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    let today = chrono::Utc::now().date_naive().to_string();
    let mut valid: Vec<String> = Vec::new();
    for id in stored {
        let known = direct_models::models(p)
            .iter()
            .any(|m| m.id == id && !m.retired(&today));
        if known && !valid.contains(&id) {
            valid.push(id);
        }
    }
    if valid.is_empty() {
        let fallback = direct_models::default_for(p, role)
            .ok_or_else(|| Error::Other(format!("PM has no models for {}", d.name)))?;
        valid.push(fallback.id.clone());
    }
    Ok(valid)
}

/// The effort a role sends on its primary model: the stored level if the model offers it, else the
/// role's default. Chat's default is `None` (omit the field: the model's own default). Background's
/// is "low" if the model lists it, else the model's lowest level. A stored level the model doesn't
/// offer (the model changed, or the list shrank) coerces to that default.
pub(crate) fn effort_for(
    conn: &Connection,
    p: CloudProvider,
    role: Role,
) -> Result<Option<String>> {
    let d = row(p)?;
    let primary = models_for(conn, p, role)?.remove(0);
    let Some(model) = direct_models::find(&primary) else {
        return Ok(None);
    };
    let stored = db::get_setting(conn, d.role(role).effort)?;
    if let Some(level) = stored.filter(|l| model.offers(l)) {
        return Ok(Some(level));
    }
    Ok(match role {
        Role::Chat => None,
        Role::Background => background_default_effort(model),
    })
}

fn background_default_effort(model: &DirectModel) -> Option<String> {
    if model.offers("low") {
        Some("low".to_string())
    } else {
        model.effort_levels.first().cloned()
    }
}

/// What a direct arm needs from the store: the one model it calls and the effort to send.
pub(crate) struct ArmInputs {
    pub models: Vec<String>,
    pub effort: Option<String>,
    /// The STORED tier and the fingerprint it was stored with. Not judged here: `cloud_arm` compares
    /// the fingerprint with the key it reads from the keychain afterwards, so the guard is already
    /// gone and a verdict about another key never reaches the arm.
    pub tier: Option<KeyTier>,
    pub key_fp: Option<String>,
}

/// The one read `cloud_arm` makes for a direct provider. The arm carries ONE model: auto-switch is
/// an OpenRouter server feature, so a stored fallback list would look like working fallbacks and
/// isn't. The storage is already an array so a later PR can lift the cap.
pub(crate) fn arm_inputs(conn: &Connection, p: CloudProvider, role: Role) -> Result<ArmInputs> {
    let first = models_for(conn, p, role)?.remove(0);
    let stored = if row(p)?.tiers {
        read_key_check(conn, p)?
    } else {
        None
    };
    Ok(ArmInputs {
        models: vec![first],
        effort: effort_for(conn, p, role)?,
        tier: stored.as_ref().and_then(|s| s.tier),
        key_fp: stored.and_then(|s| s.key_fp),
    })
}

/// Choose a role's model and effort on a provider in ONE write, so the effort is checked against the
/// NEW model. A direct list holds exactly one model in this version; the id must be one PM lists for
/// the provider; the effort is `None` (clears the row, so the role default) or a level that model
/// offers. The word "default" is not a level and is refused.
pub(crate) fn set_role_model(
    conn: &Connection,
    p: CloudProvider,
    role: Role,
    models: &[String],
    effort: Option<&str>,
) -> Result<()> {
    let d = row(p)?;
    let [id] = models else {
        return Err(Error::Other(format!(
            "{} takes exactly one model for a role in this version of PM.",
            d.name
        )));
    };
    let model = direct_models::models(p)
        .iter()
        .find(|m| &m.id == id)
        .ok_or_else(|| Error::Other(format!("PM doesn't list {id} for {}.", d.name)))?;
    if let Some(level) = effort {
        if !model.offers(level) {
            return Err(Error::Other(format!(
                "{} doesn't take effort '{level}'.",
                model.name
            )));
        }
    }
    let keys = d.role(role);
    let json = serde_json::to_string(&[id]).map_err(|e| Error::Other(e.to_string()))?;
    let tx = conn.unchecked_transaction()?;
    db::set_setting(&tx, keys.models, &json)?;
    match effort {
        Some(level) => db::set_setting(&tx, keys.effort, level)?,
        None => db::delete_setting(&tx, keys.effort)?,
    }
    tx.commit()?;
    Ok(())
}

/// Every role whose STORED provider is `p` goes back to OpenRouter (its row is deleted), inside the
/// given transaction, and the provider's yes to the on-battery question is forgotten with them: a
/// stale yes would let a role moved to `p` later skip the question that names it. Returns the roles
/// moved.
fn retire_in(conn: &Connection, p: CloudProvider) -> Result<Vec<Role>> {
    forget_battery_yes(conn, p)?;
    let mut moved = Vec::new();
    for role in [Role::Chat, Role::Background] {
        if stored_role_provider(conn, role)? == p {
            db::delete_setting(conn, cloud_provider_key(role))?;
            moved.push(role);
        }
    }
    Ok(moved)
}

/// Take `p`'s yes to the on-battery question out of the stored consent (the row is deleted when no
/// provider has one left).
fn forget_battery_yes(conn: &Connection, p: CloudProvider) -> Result<()> {
    let kept = DirectConsent::from_setting(db::get_setting(conn, DIRECT_CONSENT_KEY)?.as_deref());
    match kept.without(p).as_setting() {
        Some(json) => db::set_setting(conn, DIRECT_CONSENT_KEY, &json)?,
        None => db::delete_setting(conn, DIRECT_CONSENT_KEY)?,
    }
    Ok(())
}

/// A yes to the on-battery question is a yes to a provider's TERMS, and a paid key's terms don't
/// train on what it sends while a free key's may. So when PM learns a key it knew as Paid now reads as
/// free (Free, Unknown, or no verdict), the provider's yes is forgotten and the question asks again,
/// in the free-key wording. Free to Paid, Paid to Paid and anything from a key PM never knew as Paid
/// keep the yes. Called by EVERY writer of a stored tier: [`store_key_check`] (the save and the
/// recheck) and [`apply_seen`] (the passive flip).
pub(crate) fn forget_yes_on_terms_drop(
    conn: &Connection,
    p: CloudProvider,
    old: Option<KeyTier>,
    new: Option<KeyTier>,
) -> Result<()> {
    if old == Some(KeyTier::Paid) && new.is_none_or(KeyTier::reads_as_free) {
        forget_battery_yes(conn, p)?;
    }
    Ok(())
}

/// [`retire_in`] in its own transaction, for a key removed while the provider stays on.
pub(crate) fn retire_roles(conn: &Connection, p: CloudProvider) -> Result<Vec<Role>> {
    let tx = conn.unchecked_transaction()?;
    let moved = retire_in(&tx, p)?;
    tx.commit()?;
    Ok(moved)
}

/// Turn a provider on or off. On is the flag alone. Off is ONE transaction: the flag and every role
/// that was on the provider moving back to OpenRouter, so there is never a moment with a role on a
/// provider that is off. Returns the roles moved, which the caller reports to the person.
pub(crate) fn set_opt_in(conn: &Connection, p: CloudProvider, enabled: bool) -> Result<Vec<Role>> {
    let d = row(p)?;
    if enabled {
        db::set_setting(conn, d.opt_in, "true")?;
        return Ok(Vec::new());
    }
    let tx = conn.unchecked_transaction()?;
    db::delete_setting(&tx, d.opt_in)?;
    let moved = retire_in(&tx, p)?;
    tx.commit()?;
    Ok(moved)
}

/// Point a role at OpenRouter (deleting its row) or at a direct provider. The caller has already
/// checked the provider is opted in and keyed.
pub(crate) fn set_role_provider(conn: &Connection, role: Role, p: CloudProvider) -> Result<()> {
    if p.is_direct() {
        db::set_setting(conn, cloud_provider_key(role), p.as_str())
    } else {
        db::delete_setting(conn, cloud_provider_key(role))
    }
}

/// Whether the person ticked "my project shares data". A provider with no such tick-box is never
/// "shared": its tier is detected, not asserted.
pub(crate) fn shares_data(conn: &Connection, p: CloudProvider) -> Result<bool> {
    match row(p)?.shares_data {
        Some(key) => Ok(db::get_setting(conn, key)?.as_deref() == Some("true")),
        None => Ok(false),
    }
}

pub(crate) fn set_shares_data(conn: &Connection, p: CloudProvider, shares: bool) -> Result<()> {
    let key = row(p)?
        .shares_data
        .ok_or_else(|| Error::Other("no such option".into()))?;
    if shares {
        db::set_setting(conn, key, "true")
    } else {
        db::delete_setting(conn, key)
    }
}

// ---- the read model ------------------------------------------------------------------------------

/// A role moved off a provider, for the card to say so.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub(crate) struct RoleMove {
    pub role: Role,
    pub from: CloudProvider,
    pub to: CloudProvider,
}

/// Roles moved back to OpenRouter off `from`.
pub(crate) fn moves_to_openrouter(from: CloudProvider, roles: &[Role]) -> Vec<RoleMove> {
    roles
        .iter()
        .map(|&role| RoleMove {
            role,
            from,
            to: CloudProvider::OpenRouter,
        })
        .collect()
}

/// One bundled catalogue row as the webview sees it. Prices are USD per MILLION tokens.
#[derive(Debug, Serialize)]
pub(crate) struct DirectModelView {
    pub id: String,
    pub name: String,
    pub note: Option<String>,
    pub context: u32,
    pub max_output: u32,
    pub price_in: f64,
    pub price_cached_in: Option<f64>,
    pub price_out: f64,
    pub effort_levels: Vec<String>,
    pub effort_default: Option<String>,
    pub input_modalities: Vec<String>,
    pub checked_on: String,
    pub retires_on: Option<String>,
    /// Null where the provider has no free tier at all, false for a model only a paid key can use.
    pub free_tier: Option<bool>,
    /// The next announced price change, so the picker can say what it will cost.
    pub next_price: Option<NextPrice>,
}

/// A price change still ahead. The current prices are on the view itself.
#[derive(Debug, Serialize, PartialEq)]
pub(crate) struct NextPrice {
    pub from: String,
    pub price_in: f64,
    pub price_out: f64,
}

impl DirectModelView {
    /// The row as it reads on `today`: the prices in force then, and the change after them.
    pub(crate) fn on(m: &DirectModel, today: &str) -> Self {
        let now = m.prices_on(today);
        Self {
            id: m.id.clone(),
            name: m.name.clone(),
            note: m.note.clone(),
            context: m.context,
            max_output: m.max_output,
            price_in: now.price_in,
            price_cached_in: now.price_cached_in,
            price_out: now.price_out,
            effort_levels: m.effort_levels.clone(),
            effort_default: m.effort_default.clone(),
            input_modalities: m.input_modalities.clone(),
            checked_on: m.checked_on.clone(),
            retires_on: m.retires_on.clone(),
            free_tier: m.free_tier,
            next_price: m.next_change(today).map(|c| NextPrice {
                from: c.from.clone(),
                price_in: c.price_in,
                price_out: c.price_out,
            }),
        }
    }
}

/// What a role does on a provider, EFFECTIVELY: the model and the effort that would be sent.
#[derive(Debug, Serialize)]
pub(crate) struct DirectRoleView {
    pub uses_this_provider: bool,
    pub model: String,
    /// `None` is "the field is omitted" (chat's default).
    pub effort: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct DirectRolesView {
    pub chat: DirectRoleView,
    pub background: DirectRoleView,
}

#[derive(Debug, Serialize)]
pub(crate) struct DirectProviderView {
    pub provider: CloudProvider,
    pub name: String,
    pub opted_in: bool,
    pub key: KeyPresence,
    /// `None` unless `key` is `present`: a verdict carried over in a backup must never read as
    /// "Key works" on a device with no key.
    pub key_check: Option<StoredKeyCheck>,
    pub shares_data: bool,
    /// On whose terms the key is NOW, from the latest of a check, a failed call and the stored row,
    /// and only when it is about THIS key. `None` for a provider without tiers, with no key, and for a
    /// key with no usable verdict: that reads as free and the card says PM hasn't checked it.
    pub tier: Option<KeyTier>,
    /// Set when the provider is on, its key is here, the verdict reads as free and this computer is
    /// somewhere PM only uses paid keys. The webview never computes this: Rust read the region.
    pub blocked: Option<RegionRefusal>,
    pub catalogue_checked_on: String,
    pub terms_checked_on: String,
    pub roles: DirectRolesView,
    pub models: Vec<DirectModelView>,
}

/// Which provider a role has stored, and which actually answers it. They differ when a stored
/// direct provider isn't opted in.
#[derive(Debug, Serialize)]
pub(crate) struct RoleProviderView {
    pub stored: CloudProvider,
    pub effective: CloudProvider,
}

#[derive(Debug, Serialize)]
pub(crate) struct RoleProvidersView {
    pub chat: RoleProviderView,
    pub background: RoleProviderView,
}

#[derive(Debug, Serialize)]
pub(crate) struct DirectProvidersState {
    pub providers: Vec<DirectProviderView>,
    pub roles: RoleProvidersView,
}

fn role_view(conn: &Connection, p: CloudProvider, role: Role) -> Result<DirectRoleView> {
    Ok(DirectRoleView {
        uses_this_provider: role_cloud_provider(conn, role)? == p,
        model: models_for(conn, p, role)?.remove(0),
        effort: effort_for(conn, p, role)?,
    })
}

fn role_provider_view(conn: &Connection, role: Role) -> Result<RoleProviderView> {
    Ok(RoleProviderView {
        stored: stored_role_provider(conn, role)?,
        effective: role_cloud_provider(conn, role)?,
    })
}

/// What the keychain said about one provider's key: presence, and for a provider with tiers the
/// fingerprint of the key that is there.
pub(crate) type KeyState = (CloudProvider, KeyPresence, Option<String>);

/// The whole read model, from the store and the keychain answers the caller already has. PURE over
/// its inputs (the region too), so a test feeds it an unreadable keychain or a place. A provider
/// missing from `states` reads as having no key. The stored fingerprint never leaves: it is blanked
/// here, and a verdict about a DIFFERENT key (a backup, a shared vault) is not shown at all.
pub(crate) fn build_state(
    conn: &Connection,
    states: &[KeyState],
    region: &Region,
) -> Result<DirectProvidersState> {
    let today = direct_models::utc_today();
    let mut providers = Vec::new();
    for d in PROVIDERS {
        let (key, fp) = states
            .iter()
            .find(|(p, _, _)| *p == d.provider)
            .map_or((KeyPresence::Absent, None), |(_, k, fp)| (*k, fp.clone()));
        let section = direct_models::section(d.provider)
            .ok_or_else(|| Error::Other(format!("PM has no models for {}", d.name)))?;
        let present = key == KeyPresence::Present;
        let opted = opted_in(conn, d.provider)?;
        let (tier, key_check) = if !present {
            (None, None)
        } else if d.tiers {
            let tier = tier_now(conn, d.provider, fp.as_deref())?;
            // A row about another key says nothing about this one.
            let about_this_key = read_key_check(conn, d.provider)?
                .filter(|s| s.key_fp.is_some() && s.key_fp == fp)
                .map(|s| StoredKeyCheck {
                    tier,
                    key_fp: None,
                    ..s
                });
            (tier, about_this_key)
        } else {
            (None, read_key_check(conn, d.provider)?)
        };
        let blocked = if present && opted && d.tiers {
            refuses(d.provider, tier, region)
        } else {
            None
        };
        providers.push(DirectProviderView {
            provider: d.provider,
            name: d.name.to_string(),
            opted_in: opted,
            key,
            key_check,
            shares_data: shares_data(conn, d.provider)?,
            tier,
            blocked,
            catalogue_checked_on: direct_models::checked_on().to_string(),
            terms_checked_on: section.terms_checked_on.clone(),
            roles: DirectRolesView {
                chat: role_view(conn, d.provider, Role::Chat)?,
                background: role_view(conn, d.provider, Role::Background)?,
            },
            models: section
                .models
                .iter()
                .map(|m| DirectModelView::on(m, &today))
                .collect(),
        });
    }
    Ok(DirectProvidersState {
        providers,
        roles: RoleProvidersView {
            chat: role_provider_view(conn, Role::Chat)?,
            background: role_provider_view(conn, Role::Background)?,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OA: CloudProvider = CloudProvider::OpenAi;

    fn temp_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sqlite");
        let key = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let conn = db::open(&path, key).unwrap();
        (dir, conn)
    }

    fn opt_in(conn: &Connection) {
        set_opt_in(conn, OA, true).unwrap();
    }

    #[test]
    fn a_fresh_store_is_off_with_the_catalogue_defaults() {
        let (_d, conn) = temp_db();
        assert!(!opted_in(&conn, OA).unwrap());
        assert!(!opted_in(&conn, CloudProvider::OpenRouter).unwrap());
        assert_eq!(
            models_for(&conn, OA, Role::Chat).unwrap(),
            vec!["openai/gpt-6.2-sol"]
        );
        assert_eq!(
            models_for(&conn, OA, Role::Background).unwrap(),
            vec!["openai/gpt-6-luna"]
        );
        assert_eq!(
            effort_for(&conn, OA, Role::Chat).unwrap(),
            None,
            "chat omits the field"
        );
        assert_eq!(
            effort_for(&conn, OA, Role::Background).unwrap().as_deref(),
            Some("low")
        );
    }

    #[test]
    fn an_unknown_stored_provider_is_openrouter_and_a_direct_one_not_opted_in_is_demoted() {
        let (_d, conn) = temp_db();
        // "gemini" is an arm now: stored, but demoted until the person opts in.
        db::set_setting(&conn, "chat_cloud_provider", "gemini").unwrap();
        assert_eq!(
            stored_role_provider(&conn, Role::Chat).unwrap(),
            CloudProvider::Gemini
        );
        assert_eq!(
            role_cloud_provider(&conn, Role::Chat).unwrap(),
            CloudProvider::OpenRouter
        );
        for raw in ["", "junk", "anthropic", "OPENROUTER"] {
            db::set_setting(&conn, "chat_cloud_provider", raw).unwrap();
            assert_eq!(
                role_cloud_provider(&conn, Role::Chat).unwrap(),
                CloudProvider::OpenRouter,
                "{raw}"
            );
        }
        db::set_setting(&conn, "chat_cloud_provider", "openai").unwrap();
        // Stored but not opted in: demoted, and visibly so.
        assert_eq!(stored_role_provider(&conn, Role::Chat).unwrap(), OA);
        assert_eq!(
            role_cloud_provider(&conn, Role::Chat).unwrap(),
            CloudProvider::OpenRouter
        );
        opt_in(&conn);
        assert_eq!(role_cloud_provider(&conn, Role::Chat).unwrap(), OA);
        assert_eq!(
            role_cloud_provider(&conn, Role::Background).unwrap(),
            CloudProvider::OpenRouter
        );
    }

    #[test]
    fn models_for_drops_unknown_ids_and_is_never_empty() {
        let (_d, conn) = temp_db();
        let d = provider(OA).unwrap();
        db::set_setting(
            &conn,
            d.chat.models,
            r#"["x/y","openai/gpt-6.2-astra","openai/gpt-6.2-astra"]"#,
        )
        .unwrap();
        assert_eq!(
            models_for(&conn, OA, Role::Chat).unwrap(),
            vec!["openai/gpt-6.2-astra"]
        );
        db::set_setting(&conn, d.chat.models, r#"["x/y"]"#).unwrap();
        assert_eq!(
            models_for(&conn, OA, Role::Chat).unwrap(),
            vec!["openai/gpt-6.2-sol"]
        );
        db::set_setting(&conn, d.chat.models, "not json").unwrap();
        assert_eq!(
            models_for(&conn, OA, Role::Chat).unwrap(),
            vec!["openai/gpt-6.2-sol"]
        );
        db::set_setting(&conn, d.chat.models, "[]").unwrap();
        assert_eq!(models_for(&conn, OA, Role::Chat).unwrap().len(), 1);
    }

    #[test]
    fn a_stored_effort_the_model_does_not_offer_coerces_to_the_role_default() {
        let (_d, conn) = temp_db();
        let d = provider(OA).unwrap();
        // Sol (chat's default model) rejects none.
        db::set_setting(&conn, d.chat.effort, "none").unwrap();
        assert_eq!(effort_for(&conn, OA, Role::Chat).unwrap(), None);
        db::set_setting(&conn, d.chat.effort, "high").unwrap();
        assert_eq!(
            effort_for(&conn, OA, Role::Chat).unwrap().as_deref(),
            Some("high")
        );
        db::set_setting(&conn, d.chat.effort, "minimal").unwrap();
        assert_eq!(
            effort_for(&conn, OA, Role::Chat).unwrap(),
            None,
            "no model takes minimal"
        );
        // Background defaults low, and a stored level that Luna offers wins.
        db::set_setting(&conn, d.background.effort, "xhigh").unwrap();
        assert_eq!(
            effort_for(&conn, OA, Role::Background).unwrap().as_deref(),
            Some("xhigh")
        );
        db::set_setting(&conn, d.background.effort, "bogus").unwrap();
        assert_eq!(
            effort_for(&conn, OA, Role::Background).unwrap().as_deref(),
            Some("low")
        );
    }

    /// What Google's compatible endpoint answered 200 to, per model, for each `reasoning_effort` word
    /// (build day, 2026-11-26, paid key, step 5): `none` and `minimal` and `xhigh` are each refused by
    /// some models with a 400 that would end a person's chat. Every level PM OFFERS, every model's own
    /// default and every role's default must sit inside the accepted set; `none` is accepted on six
    /// models but is deliberately never offered.
    #[test]
    fn every_level_pm_offers_and_every_default_is_one_google_accepted() {
        const ACCEPTED: [(&str, &[&str]); 8] = [
            (
                "gemini-4-hydrogen",
                &["none", "low", "medium", "high", "xhigh"],
            ),
            ("gemini-4-neon", &["none", "low", "medium", "high", "xhigh"]),
            ("gemini-4-carbon", &["none", "low", "medium", "high"]),
            ("gemini-4-argon", &["none", "low", "medium", "high"]),
            ("gemini-3.8-flash", &["none", "low", "medium", "high"]),
            (
                "gemini-3.5-flash-lite",
                &["minimal", "low", "medium", "high"],
            ),
            (
                "gemini-3.1-flash-lite",
                &["none", "minimal", "low", "medium", "high"],
            ),
            ("gemini-3.1-pro-preview", &["low", "medium", "high"]),
        ];
        assert_eq!(
            direct_models::models(GM).len(),
            ACCEPTED.len(),
            "a new Gemini row needs its effort matrix run first"
        );
        for (wire, accepted) in ACCEPTED {
            let m = direct_models::find_wire(wire).unwrap_or_else(|| panic!("{wire} is listed"));
            for level in &m.effort_levels {
                assert!(accepted.contains(&level.as_str()), "{wire} offers {level}");
            }
            let default = m.effort_default.as_deref().expect("a native default");
            assert!(accepted.contains(&default), "{wire} defaults to {default}");
            let background = background_default_effort(m).expect("a background default");
            assert!(
                accepted.contains(&background.as_str()),
                "{wire}: background {background}"
            );
            assert!(!m.offers("none"), "{wire}");
        }
    }

    /// Output limits from Google's own native list for the key (build day, step 1): Carbon and Argon
    /// stop at 32,768, the rest at 65,536, and every row reads 1,048,576 in. `max_output` goes on every
    /// call, and the build day sent each listed limit as `max_tokens` and got 200 on all eight.
    #[test]
    fn the_listed_output_limits_are_googles_own() {
        for m in direct_models::models(GM) {
            let want = if m.wire == "gemini-4-carbon" || m.wire == "gemini-4-argon" {
                32_768
            } else {
                65_536
            };
            assert_eq!(m.max_output, want, "{}", m.id);
            assert_eq!((m.context, m.max_input), (1_048_576, 1_048_576), "{}", m.id);
        }
    }

    #[test]
    fn a_model_without_low_gives_background_its_lowest_level() {
        let mut m = direct_models::find("openai/gpt-6.2-sol").unwrap().clone();
        m.effort_levels = vec!["medium".into(), "high".into()];
        assert_eq!(background_default_effort(&m).as_deref(), Some("medium"));
    }

    #[test]
    fn set_role_model_takes_one_catalogue_model_and_a_level_it_offers() {
        let (_d, conn) = temp_db();
        let one = |s: &str| vec![s.to_string()];
        assert!(set_role_model(&conn, OA, Role::Chat, &[], None).is_err());
        let two = vec![
            "openai/gpt-6.2-sol".to_string(),
            "openai/gpt-6-luna".to_string(),
        ];
        assert!(
            set_role_model(&conn, OA, Role::Chat, &two, None).is_err(),
            "one model per role"
        );
        let e = set_role_model(&conn, OA, Role::Chat, &one("x/y"), None).unwrap_err();
        assert!(e.to_string().contains("x/y"), "{e}");
        // "default" is not a level.
        assert!(set_role_model(
            &conn,
            OA,
            Role::Chat,
            &one("openai/gpt-6.2-sol"),
            Some("default")
        )
        .is_err());
        assert!(set_role_model(
            &conn,
            OA,
            Role::Chat,
            &one("openai/gpt-6.2-sol"),
            Some("none")
        )
        .is_err());
        // Nothing was written by the refusals.
        assert_eq!(
            models_for(&conn, OA, Role::Chat).unwrap(),
            vec!["openai/gpt-6.2-sol"]
        );
        assert!(db::get_setting(&conn, "direct_openai_chat_models")
            .unwrap()
            .is_none());

        set_role_model(
            &conn,
            OA,
            Role::Chat,
            &one("openai/gpt-6.2-astra"),
            Some("max"),
        )
        .unwrap();
        assert_eq!(
            models_for(&conn, OA, Role::Chat).unwrap(),
            vec!["openai/gpt-6.2-astra"]
        );
        assert_eq!(
            effort_for(&conn, OA, Role::Chat).unwrap().as_deref(),
            Some("max")
        );
        // None clears the effort row.
        set_role_model(&conn, OA, Role::Chat, &one("openai/gpt-6.2-astra"), None).unwrap();
        assert!(db::get_setting(&conn, "direct_openai_chat_effort")
            .unwrap()
            .is_none());
    }

    #[test]
    fn retire_roles_moves_only_the_roles_on_that_provider() {
        let (_d, conn) = temp_db();
        opt_in(&conn);
        set_role_provider(&conn, Role::Chat, OA).unwrap();
        assert_eq!(retire_roles(&conn, OA).unwrap(), vec![Role::Chat]);
        assert!(db::get_setting(&conn, "chat_cloud_provider")
            .unwrap()
            .is_none());
        assert!(
            opted_in(&conn, OA).unwrap(),
            "removing a key leaves the opt-in on"
        );
        assert!(retire_roles(&conn, OA).unwrap().is_empty());
    }

    /// A yes to the battery question that named OpenAI does not outlive OpenAI being turned off or its
    /// key removed: turning it on again and moving a role must ask again.
    #[test]
    fn opting_out_or_removing_the_key_forgets_the_battery_yes() {
        use crate::power::PowerScope;
        let yes = || {
            DirectConsent::NONE
                .with(OA, PowerScope::Both)
                .as_setting()
                .unwrap()
        };
        let covers = |conn: &Connection| {
            DirectConsent::from_setting(
                db::get_setting(conn, DIRECT_CONSENT_KEY)
                    .unwrap()
                    .as_deref(),
            )
            .get(OA)
            .covers(Role::Chat)
        };
        let (_d, conn) = temp_db();
        opt_in(&conn);
        set_role_provider(&conn, Role::Chat, OA).unwrap();
        db::set_setting(&conn, DIRECT_CONSENT_KEY, &yes()).unwrap();
        assert!(covers(&conn), "the setup has a yes");
        set_opt_in(&conn, OA, false).unwrap();
        assert!(!covers(&conn), "opt-out forgot it");
        assert!(db::get_setting(&conn, DIRECT_CONSENT_KEY)
            .unwrap()
            .is_none());
        opt_in(&conn);
        set_role_provider(&conn, Role::Chat, OA).unwrap();
        assert!(
            !covers(&conn),
            "opting back in and moving a role asks again"
        );

        // A key removed (the provider stays on) forgets it too.
        db::set_setting(&conn, DIRECT_CONSENT_KEY, &yes()).unwrap();
        retire_roles(&conn, OA).unwrap();
        assert!(!covers(&conn), "removing the key forgot it");
    }

    /// Opting out moves the roles in the same transaction as the flag, and reports which moved.
    #[test]
    fn opting_out_moves_both_roles_back_and_says_which() {
        let (_d, conn) = temp_db();
        opt_in(&conn);
        set_role_provider(&conn, Role::Chat, OA).unwrap();
        set_role_provider(&conn, Role::Background, OA).unwrap();
        let moved = set_opt_in(&conn, OA, false).unwrap();
        assert_eq!(moved, vec![Role::Chat, Role::Background]);
        assert!(!opted_in(&conn, OA).unwrap());
        for role in [Role::Chat, Role::Background] {
            assert_eq!(
                stored_role_provider(&conn, role).unwrap(),
                CloudProvider::OpenRouter
            );
        }
        let reported = moves_to_openrouter(OA, &moved);
        assert_eq!(reported.len(), 2);
        assert_eq!(reported[0].to, CloudProvider::OpenRouter);
        // Opting in moves nothing.
        assert!(set_opt_in(&conn, OA, true).unwrap().is_empty());
    }

    #[test]
    fn opting_out_with_one_role_on_it_leaves_the_other_alone() {
        let (_d, conn) = temp_db();
        opt_in(&conn);
        set_role_provider(&conn, Role::Background, OA).unwrap();
        assert_eq!(
            set_opt_in(&conn, OA, false).unwrap(),
            vec![Role::Background]
        );
    }

    #[test]
    fn the_state_reports_stored_and_effective_and_hides_a_verdict_without_a_key() {
        let (_d, conn) = temp_db();
        opt_in(&conn);
        set_role_provider(&conn, Role::Chat, OA).unwrap();
        let check = KeyCheck {
            listed: vec!["openai/gpt-6.2-sol".into()],
            listed_total: 9,
            ..KeyCheck::of(KeyOutcome::Ok)
        };
        store_key_check(&conn, OA, &check, "fp").unwrap();
        let outside = Region::Outside;
        let state = |presence| [(OA, presence, None)];

        // A key present: the verdict shows.
        let s = build_state(&conn, &state(KeyPresence::Present), &outside).unwrap();
        let p = &s.providers[0];
        assert!(p.opted_in && p.key == KeyPresence::Present);
        assert_eq!(p.key_check.as_ref().unwrap().listed_total, 9);
        assert!(p.roles.chat.uses_this_provider && !p.roles.background.uses_this_provider);
        assert_eq!(p.models.len(), 4);
        assert_eq!(s.roles.chat.stored, OA);
        assert_eq!(s.roles.chat.effective, OA);
        // OpenAI has no tiers: nothing about one, in any place.
        assert!(p.tier.is_none() && p.blocked.is_none());
        let uk = crate::region::region_of_on(Some("Europe/London"), false);
        let s = build_state(&conn, &state(KeyPresence::Present), &uk).unwrap();
        assert!(
            s.providers[0].blocked.is_none(),
            "OpenAI is never blocked by a region"
        );

        // A verdict carried in a backup, no key on this device: never shown.
        for presence in [KeyPresence::Absent, KeyPresence::Unreadable] {
            let s = build_state(&conn, &state(presence), &outside).unwrap();
            assert!(s.providers[0].key_check.is_none(), "{presence:?}");
            assert_eq!(s.providers[0].key, presence);
        }

        // Turned off by hand underneath a stored role: both are reported, and they differ.
        db::delete_setting(&conn, "direct_openai_enabled").unwrap();
        let s = build_state(&conn, &state(KeyPresence::Present), &outside).unwrap();
        assert_eq!(s.roles.chat.stored, OA);
        assert_eq!(s.roles.chat.effective, CloudProvider::OpenRouter);
        assert!(!s.providers[0].roles.chat.uses_this_provider);
    }

    #[test]
    fn the_stored_verdict_serialises_with_its_unexpected_status_as_an_object() {
        let c = StoredKeyCheck {
            at: "t".into(),
            outcome: KeyOutcome::Unexpected(418),
            listed: vec![],
            listed_total: 0,
            note: None,
            tier: None,
            flipped_free_at: None,
            key_fp: None,
        };
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["outcome"]["unexpected"], 418);
        assert_eq!(
            serde_json::to_value(KeyOutcome::RegionBlocked).unwrap(),
            "region_blocked"
        );
        let back: StoredKeyCheck = serde_json::from_value(v).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn a_verdict_stored_before_the_test_chat_still_reads_and_a_note_round_trips() {
        // The row an earlier build wrote: no `note`.
        let old = r#"{"at":"t","outcome":"ok","listed":[],"listed_total":3}"#;
        let c: StoredKeyCheck = serde_json::from_str(old).unwrap();
        assert_eq!(c.note, None);
        assert!(!serde_json::to_string(&c).unwrap().contains("note"));
        let c = StoredKeyCheck {
            at: "t".into(),
            outcome: KeyOutcome::NoCredit,
            listed: vec![],
            listed_total: 4,
            note: Some(KeyNote::ChatUntried),
            tier: None,
            flipped_free_at: None,
            key_fp: None,
        };
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["outcome"], "no_credit");
        assert_eq!(v["note"], "chat_untried");
        assert_eq!(serde_json::from_value::<StoredKeyCheck>(v).unwrap(), c);
    }

    /// The row 2a and the OpenAI follow-up wrote has no tier, flip date or fingerprint, and still
    /// reads; the three never appear in a row that doesn't carry them.
    #[test]
    fn a_verdict_stored_before_tiers_still_reads_and_the_new_fields_round_trip() {
        let old = r#"{"at":"t","outcome":"ok","listed":[],"listed_total":3,"note":"read_only"}"#;
        let c: StoredKeyCheck = serde_json::from_str(old).unwrap();
        assert_eq!(
            (c.tier, c.flipped_free_at.clone(), c.key_fp.clone()),
            (None, None, None)
        );
        let s = serde_json::to_string(&c).unwrap();
        assert!(
            !s.contains("tier") && !s.contains("flipped") && !s.contains("key_fp"),
            "{s}"
        );
        let c = StoredKeyCheck {
            tier: Some(KeyTier::Free),
            flipped_free_at: Some("2026-11-26T10:00:00Z".into()),
            key_fp: Some("0123456789abcdef".into()),
            ..c
        };
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["tier"], "free");
        assert_eq!(v["flipped_free_at"], "2026-11-26T10:00:00Z");
        assert_eq!(serde_json::from_value::<StoredKeyCheck>(v).unwrap(), c);
    }

    #[test]
    fn only_a_working_or_restricted_key_is_kept() {
        for (o, kept) in [
            (KeyOutcome::Ok, true),
            (KeyOutcome::Restricted, true),
            (KeyOutcome::NoCredit, true),
            (KeyOutcome::Rejected, false),
            (KeyOutcome::NeedsVerification, false),
            (KeyOutcome::FreeKeyRefused, false),
            (KeyOutcome::Busy, false),
            (KeyOutcome::RegionBlocked, false),
            (KeyOutcome::Malformed, false),
            (KeyOutcome::Unreachable, false),
            (KeyOutcome::Unexpected(500), false),
        ] {
            assert_eq!(o.keeps_key(), kept, "{o:?}");
        }
    }

    #[test]
    fn every_table_row_has_a_catalogue_section_and_a_provider_each() {
        for d in PROVIDERS {
            assert!(direct_models::section(d.provider).is_some(), "{}", d.name);
            assert!(d.provider.is_direct());
        }
        for s in &direct_models::catalogue().providers {
            let p = CloudProvider::from_setting(Some(&s.provider));
            assert!(provider(p).is_some(), "{} has no table row", s.provider);
        }
    }

    #[test]
    fn a_role_provider_row_round_trips_and_openrouter_deletes_it() {
        let (_d, conn) = temp_db();
        set_role_provider(&conn, Role::Background, OA).unwrap();
        assert_eq!(
            db::get_setting(&conn, "background_cloud_provider")
                .unwrap()
                .as_deref(),
            Some("openai")
        );
        set_role_provider(&conn, Role::Background, CloudProvider::OpenRouter).unwrap();
        assert!(db::get_setting(&conn, "background_cloud_provider")
            .unwrap()
            .is_none());
    }

    #[test]
    fn data_sharing_is_a_plain_flag() {
        let (_d, conn) = temp_db();
        assert!(!shares_data(&conn, OA).unwrap());
        set_shares_data(&conn, OA, true).unwrap();
        assert!(shares_data(&conn, OA).unwrap());
        set_shares_data(&conn, OA, false).unwrap();
        assert!(!shares_data(&conn, OA).unwrap());
    }

    // ---- tiers, the region rule and the inbox (Gemini) --------------------------------------------

    const GM: CloudProvider = CloudProvider::Gemini;

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        let g = INBOX_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        forget_all_seen();
        g
    }

    fn uk() -> Region {
        crate::region::region_of_on(Some("Europe/London"), false)
    }
    fn nyc() -> Region {
        crate::region::region_of_on(Some("America/New_York"), false)
    }
    fn utc() -> Region {
        crate::region::region_of_on(Some("Etc/UTC"), false)
    }

    fn ok_with(tier: Option<KeyTier>) -> KeyCheck {
        KeyCheck {
            listed: vec!["gemini/gemini-3.8-flash".into()],
            listed_total: 7,
            tier,
            ..KeyCheck::of(KeyOutcome::Ok)
        }
    }

    #[test]
    fn unknown_reads_as_free_everywhere() {
        assert!(KeyTier::Free.reads_as_free() && KeyTier::Unknown.reads_as_free());
        assert!(!KeyTier::Paid.reads_as_free());
    }

    #[test]
    fn the_region_rule_refuses_a_free_or_unknown_gemini_key_in_the_region_and_nothing_else() {
        // Free, unknown and an unchecked key are refused in the UK and where PM can't place; kept
        // outside. A paid key passes everywhere. OpenAI is the identity in any place.
        for tier in [Some(KeyTier::Free), Some(KeyTier::Unknown), None] {
            for region in [uk(), utc()] {
                let r = apply_region_rule(GM, ok_with(tier), &region);
                assert_eq!(r.outcome, KeyOutcome::FreeKeyRefused, "{tier:?}");
                assert!(r.listed.is_empty() && r.listed_total == 0);
                assert!(r.refusal.is_some() && !r.outcome.keeps_key());
            }
            assert_eq!(apply_region_rule(GM, ok_with(tier), &nyc()), ok_with(tier));
        }
        let paid = ok_with(Some(KeyTier::Paid));
        assert_eq!(apply_region_rule(GM, paid.clone(), &uk()), paid);
        // Why it was refused: Google said free, or nothing decided.
        let free = apply_region_rule(GM, ok_with(Some(KeyTier::Free)), &uk());
        assert_eq!(free.refusal.unwrap().basis, "free");
        let unk = apply_region_rule(GM, ok_with(Some(KeyTier::Unknown)), &uk());
        assert_eq!(unk.refusal.unwrap().basis, "unknown");
        // A project Google wants verified is a free key; a 402 is a paid key with no balance; a busy
        // answer decided nothing and is neither kept nor refused.
        let verif = KeyCheck::of_tier(KeyOutcome::NeedsVerification, KeyTier::Free);
        assert_eq!(
            apply_region_rule(GM, verif, &uk()).outcome,
            KeyOutcome::FreeKeyRefused
        );
        let broke = KeyCheck::of_tier(KeyOutcome::NoCredit, KeyTier::Paid);
        assert_eq!(apply_region_rule(GM, broke.clone(), &uk()), broke);
        let busy = KeyCheck::of(KeyOutcome::Busy);
        assert_eq!(apply_region_rule(GM, busy.clone(), &uk()), busy);
        // OpenAI: never refused, in any region.
        let oa = KeyCheck::of(KeyOutcome::Ok);
        for region in [uk(), utc(), nyc()] {
            assert_eq!(apply_region_rule(OA, oa.clone(), &region), oa);
        }
    }

    #[test]
    fn refuses_is_the_one_rule_for_the_seam_the_state_and_the_role_pick() {
        assert!(refuses(GM, Some(KeyTier::Free), &uk()).is_some());
        assert!(refuses(GM, Some(KeyTier::Paid), &uk()).is_none());
        assert!(
            refuses(GM, None, &utc()).is_some(),
            "no verdict is unknown is free"
        );
        assert!(refuses(GM, Some(KeyTier::Free), &nyc()).is_none());
        assert!(refuses(OA, Some(KeyTier::Free), &uk()).is_none());
        assert!(refuses(CloudProvider::Anthropic, None, &uk()).is_none());
    }

    #[test]
    fn a_key_fingerprint_binds_a_verdict_to_one_key_and_never_leaves() {
        let _g = lock();
        let (_d, conn) = temp_db();
        set_opt_in(&conn, GM, true).unwrap();
        let fp_a = key_fp("key-a-aaaaaaaaaaaaaaaaaaaaaaaa");
        let fp_b = key_fp("key-b-bbbbbbbbbbbbbbbbbbbbbbbb");
        store_key_check(&conn, GM, &ok_with(Some(KeyTier::Paid)), &fp_a).unwrap();
        forget_all_seen();
        let state = |fp: &str| [(GM, KeyPresence::Present, Some(fp.to_string()))];
        let gem =
            |s: &DirectProvidersState| s.providers.iter().position(|p| p.provider == GM).unwrap();

        // Beside the key it was stored with: trusted, and the fingerprint is blanked.
        let s = build_state(&conn, &state(&fp_a), &uk()).unwrap();
        let p = &s.providers[gem(&s)];
        assert_eq!(p.tier, Some(KeyTier::Paid));
        assert!(p.blocked.is_none());
        let shown = p.key_check.as_ref().unwrap();
        assert!(
            shown.key_fp.is_none(),
            "the fingerprint never reaches the webview"
        );
        assert_eq!(shown.tier, Some(KeyTier::Paid));
        assert!(!serde_json::to_string(&s).unwrap().contains(&fp_a));

        // The same row beside a DIFFERENT key (a backup, a shared vault): no verdict at all. The
        // provider reads as unchecked, which reads as free, which is blocked in the UK.
        let s = build_state(&conn, &state(&fp_b), &uk()).unwrap();
        let p = &s.providers[gem(&s)];
        assert_eq!(p.tier, None);
        assert!(p.key_check.is_none());
        assert_eq!(p.blocked.as_ref().unwrap().basis, "unknown");
        // Outside the region the same unchecked key is simply not blocked.
        let s = build_state(&conn, &state(&fp_b), &nyc()).unwrap();
        assert!(s.providers[gem(&s)].blocked.is_none());
    }

    #[test]
    fn blocked_is_set_only_when_the_key_is_here_the_provider_is_on_and_the_verdict_reads_free() {
        let _g = lock();
        let (_d, conn) = temp_db();
        let fp = key_fp("key-a-aaaaaaaaaaaaaaaaaaaaaaaa");
        store_key_check(&conn, GM, &ok_with(Some(KeyTier::Free)), &fp).unwrap();
        forget_all_seen();
        let at = |presence, region: &Region| {
            let s = build_state(&conn, &[(GM, presence, Some(fp.clone()))], region).unwrap();
            let i = s.providers.iter().position(|p| p.provider == GM).unwrap();
            s.providers.into_iter().nth(i).unwrap()
        };
        assert!(
            at(KeyPresence::Present, &uk()).blocked.is_none(),
            "not opted in: nothing to block"
        );
        set_opt_in(&conn, GM, true).unwrap();
        assert_eq!(at(KeyPresence::Present, &uk()).blocked.unwrap().kind, "uk");
        for presence in [KeyPresence::Absent, KeyPresence::Unreadable] {
            let p = at(presence, &uk());
            assert!(
                p.blocked.is_none() && p.key_check.is_none() && p.tier.is_none(),
                "{presence:?}"
            );
        }
        assert!(at(KeyPresence::Present, &nyc()).blocked.is_none());
        assert_eq!(at(KeyPresence::Present, &nyc()).tier, Some(KeyTier::Free));
    }

    #[test]
    fn the_passive_flip_waits_in_memory_then_lands_on_the_row_for_the_same_key() {
        let _g = lock();
        let (_d, conn) = temp_db();
        let fp = key_fp("key-a-aaaaaaaaaaaaaaaaaaaaaaaa");
        store_key_check(&conn, GM, &ok_with(Some(KeyTier::Paid)), &fp).unwrap();
        // Another key's fingerprint: a held PAID row is NOT rewritten by a note about a different key.
        note_tier_seen(
            GM,
            KeyTier::Free,
            &key_fp("other-key-xxxxxxxxxxxxxxxxxxxx"),
            "w",
            None,
        );
        // (store_key_check already recorded `fp` as the latest key, so that note is ignored.)
        assert!(!effective_free(GM, &fp));
        note_tier_seen(GM, KeyTier::Free, &fp, "gemini-3.8-flash", None);
        assert!(effective_free(GM, &fp));
        // The view reads it without writing.
        assert_eq!(tier_now(&conn, GM, Some(&fp)).unwrap(), Some(KeyTier::Free));
        assert_eq!(
            read_key_check(&conn, GM).unwrap().unwrap().tier,
            Some(KeyTier::Paid)
        );
        // apply_seen writes it, stamps the date, and still reads.
        apply_seen(&conn).unwrap();
        let row = read_key_check(&conn, GM).unwrap().unwrap();
        assert_eq!(row.tier, Some(KeyTier::Free));
        assert!(row.flipped_free_at.is_some());
        assert_eq!(row.key_fp.as_deref(), Some(fp.as_str()));
        assert!(effective_free(GM, &fp), "pending is cleared, `free` is not");
        // A second apply writes nothing new.
        let first = row.flipped_free_at.clone();
        apply_seen(&conn).unwrap();
        assert_eq!(
            read_key_check(&conn, GM).unwrap().unwrap().flipped_free_at,
            first
        );
        // A fresh PAID verdict clears the flip everywhere ("Check again").
        store_key_check(&conn, GM, &ok_with(Some(KeyTier::Paid)), &fp).unwrap();
        assert!(!effective_free(GM, &fp));
        let row = read_key_check(&conn, GM).unwrap().unwrap();
        assert_eq!((row.tier, row.flipped_free_at), (Some(KeyTier::Paid), None));
        // A fresh FREE verdict sets it, so a held Paid-stamped plan is refused at its next send.
        store_key_check(&conn, GM, &ok_with(Some(KeyTier::Free)), &fp).unwrap();
        assert!(effective_free(GM, &fp));
    }

    /// A yes to the On-battery question was given under a provider's terms: it survives only while
    /// those terms do. Table over the rule itself.
    #[test]
    fn a_yes_is_forgotten_only_when_a_known_paid_key_comes_to_read_as_free() {
        use crate::power::PowerScope;
        let yes = DirectConsent::NONE
            .with(GM, PowerScope::Both)
            .with(OA, PowerScope::Chat);
        let (_d, conn) = temp_db();
        let (p, f, u) = (
            Some(KeyTier::Paid),
            Some(KeyTier::Free),
            Some(KeyTier::Unknown),
        );
        // (old, new, forgotten)
        for (old, new, forgotten) in [
            (p, f, true),
            (p, u, true),
            (p, None, true),
            (p, p, false),
            (f, p, false),
            (f, f, false),
            (u, f, false),
            (None, f, false),
            (None, p, false),
        ] {
            db::set_setting(&conn, DIRECT_CONSENT_KEY, &yes.as_setting().unwrap()).unwrap();
            forget_yes_on_terms_drop(&conn, GM, old, new).unwrap();
            let after = DirectConsent::from_setting(
                db::get_setting(&conn, DIRECT_CONSENT_KEY)
                    .unwrap()
                    .as_deref(),
            );
            assert_eq!(
                after.get(GM) == crate::power::Consent::NONE,
                forgotten,
                "{old:?} -> {new:?}"
            );
            assert_eq!(after.get(OA), yes.get(OA), "another provider's yes stays");
        }
    }

    /// Every writer of a tier calls the rule: the save and the recheck (both through
    /// `store_key_check`) and the passive flip (`apply_seen`). A yes given while the key was paid must
    /// not carry over to a key PM now reads as free, and must survive anything that is not a drop.
    #[test]
    fn every_tier_writer_forgets_a_paid_yes_when_the_key_comes_to_read_as_free() {
        use crate::power::PowerScope;
        let _g = lock();
        let (_d, conn) = temp_db();
        let fp = key_fp("key-a-aaaaaaaaaaaaaaaaaaaaaaaa");
        let yes = || {
            db::set_setting(
                &conn,
                DIRECT_CONSENT_KEY,
                &DirectConsent::NONE
                    .with(GM, PowerScope::Both)
                    .as_setting()
                    .unwrap(),
            )
            .unwrap()
        };
        let has_yes = || {
            DirectConsent::from_setting(
                db::get_setting(&conn, DIRECT_CONSENT_KEY)
                    .unwrap()
                    .as_deref(),
            )
            .get(GM)
            .covers(Role::Chat)
        };

        // The first save has no earlier tier: nothing to forget, and Paid to Paid and Free to Paid
        // keep a yes.
        yes();
        store_key_check(&conn, GM, &ok_with(Some(KeyTier::Paid)), &fp).unwrap();
        store_key_check(&conn, GM, &ok_with(Some(KeyTier::Paid)), &fp).unwrap();
        assert!(has_yes(), "paid to paid keeps the yes");

        // The save and the recheck: Paid to Free (and to Unknown) forgets it.
        store_key_check(&conn, GM, &ok_with(Some(KeyTier::Free)), &fp).unwrap();
        assert!(!has_yes(), "a check that finds the key free forgot the yes");
        yes();
        store_key_check(&conn, GM, &ok_with(Some(KeyTier::Paid)), &fp).unwrap();
        assert!(has_yes(), "free to paid keeps a yes given meanwhile");
        store_key_check(&conn, GM, &ok_with(Some(KeyTier::Unknown)), &fp).unwrap();
        assert!(
            !has_yes(),
            "unknown reads as free, so paid to unknown forgets"
        );

        // The passive flip: a paid row, a call learns the project is free, apply_seen writes it.
        store_key_check(&conn, GM, &ok_with(Some(KeyTier::Paid)), &fp).unwrap();
        yes();
        note_tier_seen(GM, KeyTier::Free, &fp, "gemini-3.8-flash", None);
        assert!(
            has_yes(),
            "learned in memory only: the row has not changed yet"
        );
        apply_seen(&conn).unwrap();
        assert!(!has_yes(), "the passive flip forgot the yes");
        // Free to paid by a passive proof keeps it (and the row really did change).
        yes();
        note_paid_proven(GM, &fp);
        apply_seen(&conn).unwrap();
        assert_eq!(
            read_key_check(&conn, GM).unwrap().unwrap().tier,
            Some(KeyTier::Paid)
        );
        assert!(has_yes(), "a flip back to paid keeps the yes");
    }

    #[test]
    fn a_note_with_no_stored_row_or_for_another_keys_row_writes_nothing() {
        let _g = lock();
        let (_d, conn) = temp_db();
        let fp = key_fp("key-a-aaaaaaaaaaaaaaaaaaaaaaaa");
        note_tier_seen(GM, KeyTier::Free, &fp, "w", None);
        apply_seen(&conn).unwrap();
        assert!(
            read_key_check(&conn, GM).unwrap().is_none(),
            "a note never creates a row"
        );
        // A row stored beside another key keeps its tier.
        forget_all_seen();
        store_key_check(
            &conn,
            GM,
            &ok_with(Some(KeyTier::Paid)),
            &key_fp("old-key-oooooooooooooooooooo"),
        )
        .unwrap();
        forget_all_seen();
        note_tier_seen(GM, KeyTier::Free, &fp, "w", None);
        apply_seen(&conn).unwrap();
        assert_eq!(
            read_key_check(&conn, GM).unwrap().unwrap().tier,
            Some(KeyTier::Paid)
        );
    }

    #[test]
    fn an_old_keys_late_failure_cannot_flip_a_new_key_and_removing_a_key_forgets() {
        let _g = lock();
        let (_d, conn) = temp_db();
        let old = key_fp("old-key-oooooooooooooooooooo");
        let new = key_fp("new-key-nnnnnnnnnnnnnnnnnnnn");
        store_key_check(&conn, GM, &ok_with(Some(KeyTier::Paid)), &new).unwrap();
        note_tier_seen(GM, KeyTier::Free, &old, "w", None);
        assert!(!effective_free(GM, &new) && !effective_free(GM, &old));
        // Only Free is ever noted.
        note_tier_seen(GM, KeyTier::Paid, &new, "w", None);
        note_tier_seen(GM, KeyTier::Unknown, &new, "w", None);
        assert!(!effective_free(GM, &new));
        note_tier_seen(GM, KeyTier::Free, &new, "w", None);
        assert!(effective_free(GM, &new));
        clear_key_check(&conn, GM).unwrap();
        assert!(!effective_free(GM, &new), "the key was removed");
        assert!(read_key_check(&conn, GM).unwrap().is_none());
        note_tier_seen(GM, KeyTier::Free, &new, "w", None);
        forget_all_seen();
        assert!(
            !effective_free(GM, &new),
            "a locked or replaced vault forgets everything"
        );
    }

    #[test]
    fn the_latest_verdict_is_kept_whatever_it_is_and_only_for_its_own_key() {
        let _g = lock();
        let (_d, conn) = temp_db();
        let fp = key_fp("key-a-aaaaaaaaaaaaaaaaaaaaaaaa");
        let other = key_fp("key-b-bbbbbbbbbbbbbbbbbbbbbbbb");
        assert_eq!(latest_tier(GM, &fp), None);
        store_key_check(&conn, GM, &ok_with(Some(KeyTier::Unknown)), &fp).unwrap();
        assert_eq!(latest_tier(GM, &fp), Some(KeyTier::Unknown));
        assert_eq!(latest_tier(GM, &other), None);
        assert_eq!(
            tier_now(&conn, GM, Some(&fp)).unwrap(),
            Some(KeyTier::Unknown)
        );
        note_paid_proven(GM, &fp);
        assert_eq!(latest_tier(GM, &fp), Some(KeyTier::Paid));
        // A Paid proof in memory beats a stale stored Free for the view, before apply_seen writes it.
        store_key_check(&conn, GM, &ok_with(Some(KeyTier::Free)), &fp).unwrap();
        note_paid_proven(GM, &fp);
        assert_eq!(tier_now(&conn, GM, Some(&fp)).unwrap(), Some(KeyTier::Paid));
        forget_all_seen();
    }

    #[test]
    fn an_arm_is_paused_by_the_latest_word_about_its_key_and_only_for_a_provider_with_tiers() {
        use crate::llm_gateway::CloudArm;
        use crate::region::region_of_on;
        let _g = lock();
        forget_all_seen();
        let uk = region_of_on(Some("Europe/London"), false);
        let nyc = region_of_on(Some("America/New_York"), false);
        let key = "key-a-aaaaaaaaaaaaaaaaaaaaaaaa";
        let mut arm = CloudArm::new(
            GM,
            crate::secret::Secret::from(key.to_string()),
            vec!["gemini/gemini-3.8-flash".into()],
        );
        arm.tier = Some(KeyTier::Paid);
        assert!(!arm_paused_in(&arm, &uk));
        record_verdict(GM, &key_fp(key), KeyTier::Free);
        assert!(arm_paused_in(&arm, &uk) && !arm_paused_in(&arm, &nyc));
        // OpenAI is never paused, whatever the place.
        let openai = CloudArm::new(
            CloudProvider::OpenAi,
            crate::secret::Secret::from("sk-xxxxxxxxxxxxxxxxxxxxxxxx".to_string()),
            vec!["openai/gpt-6-luna".into()],
        );
        assert!(!arm_paused_in(&openai, &uk));
        forget_all_seen();
    }

    #[test]
    fn a_rejected_recheck_keeps_what_an_earlier_check_of_the_same_key_proved() {
        let _g = lock();
        let (_d, conn) = temp_db();
        let fp = key_fp("key-a-aaaaaaaaaaaaaaaaaaaaaaaa");
        store_key_check(&conn, GM, &ok_with(Some(KeyTier::Paid)), &fp).unwrap();
        store_key_check(&conn, GM, &KeyCheck::of(KeyOutcome::Rejected), &fp).unwrap();
        let row = read_key_check(&conn, GM).unwrap().unwrap();
        assert_eq!(row.outcome, KeyOutcome::Rejected);
        assert_eq!(
            row.tier,
            Some(KeyTier::Paid),
            "the proof survives a rejection"
        );
        assert_eq!(row.key_fp.as_deref(), Some(fp.as_str()));
        // A different key's rejection inherits nothing.
        let other = key_fp("key-b-bbbbbbbbbbbbbbbbbbbbbbbb");
        store_key_check(&conn, GM, &KeyCheck::of(KeyOutcome::Rejected), &other).unwrap();
        assert_eq!(read_key_check(&conn, GM).unwrap().unwrap().tier, None);
        forget_all_seen();
    }

    #[test]
    fn a_success_on_a_paid_only_model_proves_paid_and_the_next_apply_writes_it() {
        let _g = lock();
        let (_d, conn) = temp_db();
        let fp = key_fp("key-a-aaaaaaaaaaaaaaaaaaaaaaaa");
        store_key_check(&conn, GM, &ok_with(Some(KeyTier::Free)), &fp).unwrap();
        db::set_setting(
            &conn,
            "direct_gemini_key_check",
            &serde_json::to_string(&StoredKeyCheck {
                flipped_free_at: Some("2026-11-26T00:00:00Z".into()),
                ..read_key_check(&conn, GM).unwrap().unwrap()
            })
            .unwrap(),
        )
        .unwrap();
        assert!(effective_free(GM, &fp));
        note_paid_proven(GM, &fp);
        assert!(!effective_free(GM, &fp));
        apply_seen(&conn).unwrap();
        let row = read_key_check(&conn, GM).unwrap().unwrap();
        assert_eq!((row.tier, row.flipped_free_at), (Some(KeyTier::Paid), None));
    }

    #[test]
    fn a_spent_allowance_is_held_for_that_model_only_and_a_short_wait_is_not_held() {
        let _g = lock();
        let fp = key_fp("key-a-aaaaaaaaaaaaaaaaaaaaaaaa");
        note_tier_seen(GM, KeyTier::Free, &fp, "gemini-3.8-flash", Some(299));
        assert!(not_before(GM, "gemini-3.8-flash").is_none());
        note_tier_seen(GM, KeyTier::Free, &fp, "gemini-3.8-flash", Some(300));
        let held = not_before(GM, "gemini-3.8-flash").unwrap();
        assert!(held.as_secs() > 290 && held.as_secs() <= 300);
        assert!(not_before(GM, "gemini-3.1-flash-lite").is_none());
        record_verdict(GM, &fp, KeyTier::Paid);
        assert!(not_before(GM, "gemini-3.8-flash").is_none());
    }

    #[test]
    fn a_poisoned_inbox_does_not_wedge_the_send_path() {
        let _g = lock();
        let fp = key_fp("key-a-aaaaaaaaaaaaaaaaaaaaaaaa");
        let _ = std::thread::spawn(|| {
            let _held = SEEN.lock().unwrap();
            panic!("a panic while the inbox is held");
        })
        .join();
        assert!(SEEN.lock().is_err(), "the lock is poisoned");
        note_tier_seen(GM, KeyTier::Free, &fp, "w", None);
        assert!(effective_free(GM, &fp));
        forget_all_seen();
        assert!(!effective_free(GM, &fp));
        SEEN.clear_poison();
    }

    #[test]
    fn route_terms_say_paid_free_or_blocked_and_only_for_a_provider_with_tiers() {
        let _g = lock();
        let (_d, conn) = temp_db();
        let fp = key_fp("key-a-aaaaaaaaaaaaaaaaaaaaaaaa");
        let terms = |region: &Region, fp: Option<&str>| route_terms(&conn, GM, region, fp).unwrap();
        // No verdict: reads as free, blocked in the region.
        assert_eq!(terms(&nyc(), Some(&fp)), Some(RouteTerms::Free));
        assert_eq!(terms(&uk(), Some(&fp)), Some(RouteTerms::Blocked));
        store_key_check(&conn, GM, &ok_with(Some(KeyTier::Paid)), &fp).unwrap();
        assert_eq!(terms(&uk(), Some(&fp)), Some(RouteTerms::Paid));
        // A different key's fingerprint folds into free.
        assert_eq!(
            terms(&nyc(), Some("0000000000000000")),
            Some(RouteTerms::Free)
        );
        // Unknown and a seen-free flip fold into free; in the region they are blocked.
        store_key_check(&conn, GM, &ok_with(Some(KeyTier::Unknown)), &fp).unwrap();
        assert_eq!(terms(&nyc(), Some(&fp)), Some(RouteTerms::Free));
        store_key_check(&conn, GM, &ok_with(Some(KeyTier::Paid)), &fp).unwrap();
        note_tier_seen(GM, KeyTier::Free, &fp, "w", None);
        assert_eq!(terms(&nyc(), Some(&fp)), Some(RouteTerms::Free));
        assert_eq!(terms(&uk(), Some(&fp)), Some(RouteTerms::Blocked));
        assert_eq!(route_terms(&conn, OA, &uk(), None).unwrap(), None);
        assert_eq!(
            route_terms(&conn, CloudProvider::OpenRouter, &uk(), None).unwrap(),
            None
        );
        // A role on Gemini whose key isn't here has no terms ("no key" is readiness's word), and in a
        // restricted region it must not read as "paused" (review item 3).
        assert_eq!(terms(&uk(), None), None);
        assert_eq!(terms(&nyc(), None), None);
        // A person on OpenRouter or OpenAI pays nothing for Gemini: no fingerprint is read (that is a
        // keychain read) and no zone is read.
        let (fps, region) = tier_context(CloudProvider::OpenRouter, OA);
        assert!(fps.is_empty() && region == Region::Outside);
        assert_eq!(
            serde_json::to_value(RouteTerms::Blocked).unwrap(),
            "blocked"
        );
    }

    #[test]
    fn the_arm_gets_the_stored_tier_and_fingerprint_to_compare_after_the_keychain_read() {
        let _g = lock();
        let (_d, conn) = temp_db();
        let fp = key_fp("key-a-aaaaaaaaaaaaaaaaaaaaaaaa");
        let none = arm_inputs(&conn, GM, Role::Chat).unwrap();
        assert!(none.tier.is_none() && none.key_fp.is_none());
        store_key_check(&conn, GM, &ok_with(Some(KeyTier::Paid)), &fp).unwrap();
        let some = arm_inputs(&conn, GM, Role::Chat).unwrap();
        assert_eq!(some.tier, Some(KeyTier::Paid));
        assert_eq!(some.key_fp.as_deref(), Some(fp.as_str()));
        assert_eq!(some.models, ["gemini/gemini-3.8-flash"]);
        // OpenAI has no tiers: its arm never carries one.
        store_key_check(&conn, OA, &ok_with(None), "x").unwrap();
        let oa = arm_inputs(&conn, OA, Role::Chat).unwrap();
        assert!(oa.tier.is_none() && oa.key_fp.is_none());
    }

    /// A carried PAID row beside a different key must not become that key's tier: the arm is stamped
    /// only when the stored fingerprint is this key's.
    #[test]
    fn an_arm_is_stamped_with_the_stored_tier_only_for_the_key_it_was_stored_beside() {
        let key = "key-a-aaaaaaaaaaaaaaaaaaaaaaaa";
        let fp = key_fp(key);
        assert_eq!(
            tier_for_key(Some(KeyTier::Paid), Some(&fp), key),
            Some(KeyTier::Paid)
        );
        assert_eq!(
            tier_for_key(Some(KeyTier::Free), Some(&fp), key),
            Some(KeyTier::Free)
        );
        // A paid verdict carried beside a different key, a row with no fingerprint, no row at all.
        let other = key_fp("key-b-bbbbbbbbbbbbbbbbbbbbbbbb");
        assert_eq!(tier_for_key(Some(KeyTier::Paid), Some(&other), key), None);
        assert_eq!(tier_for_key(Some(KeyTier::Paid), None, key), None);
        assert_eq!(tier_for_key(None, Some(&fp), key), None);
    }

    #[test]
    fn gemini_defaults_are_free_tier_models_and_chat_omits_the_effort_while_background_says_low() {
        let (_d, conn) = temp_db();
        assert_eq!(
            models_for(&conn, GM, Role::Chat).unwrap(),
            ["gemini/gemini-3.8-flash"]
        );
        assert_eq!(
            models_for(&conn, GM, Role::Background).unwrap(),
            ["gemini/gemini-3.1-flash-lite"]
        );
        assert_eq!(effort_for(&conn, GM, Role::Chat).unwrap(), None);
        assert_eq!(
            effort_for(&conn, GM, Role::Background).unwrap().as_deref(),
            Some("low")
        );
        // A stored level the model does not list coerces to the default.
        db::set_setting(&conn, "direct_gemini_chat_effort", "xhigh").unwrap();
        assert_eq!(effort_for(&conn, GM, Role::Chat).unwrap(), None);
        db::set_setting(&conn, "direct_gemini_background_effort", "minimal").unwrap();
        assert_eq!(
            effort_for(&conn, GM, Role::Background).unwrap().as_deref(),
            Some("minimal")
        );
    }

    #[test]
    fn the_model_views_carry_the_prices_in_force_and_the_next_change() {
        let m = direct_models::find("gemini/gemini-4-carbon").unwrap();
        let before = DirectModelView::on(m, "2026-11-30");
        assert_eq!((before.price_in, before.price_out), (2.0, 10.0));
        assert_eq!(
            before.next_price,
            Some(NextPrice {
                from: "2026-12-01".into(),
                price_in: 4.0,
                price_out: 20.0
            })
        );
        let after = DirectModelView::on(m, "2026-12-01");
        assert_eq!((after.price_in, after.price_out), (4.0, 20.0));
        assert!(after.next_price.is_none());
        assert_eq!(before.free_tier, Some(false));
        assert_eq!(
            DirectModelView::on(
                direct_models::find("openai/gpt-6.2-sol").unwrap(),
                "2026-11-26"
            )
            .free_tier,
            None
        );
    }

    #[test]
    fn the_wire_ids_resolve_for_the_context_meter() {
        // The meter reads the window through `direct_models::find`: every Gemini id must resolve.
        for m in direct_models::models(GM) {
            assert_eq!(
                direct_models::find(&m.id).map(|f| f.context),
                Some(m.context)
            );
            assert_eq!(
                direct_models::find_wire(&m.wire).map(|f| &f.id),
                Some(&m.id)
            );
        }
    }
}
