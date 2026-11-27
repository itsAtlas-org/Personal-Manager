// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Settings commands for a provider the person reaches with their own key.
//!
//! The webview learns presence and a check verdict, never a key: a typed key goes once to Rust, as
//! the OpenRouter key does (AGENTS rule 5), and nothing here returns one.
//!
//! The lock rules: the keychain is read BEFORE the DB guard is taken, and no guard (or any other
//! lock) is held across an `.await`. Naming: writes are `set_*` so they get the Settings "Saved"
//! tick; the key commands are NOT, so a refused key never claims "Saved".

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::direct_providers::{
    self, DirectProvider, DirectProvidersState, KeyNote, KeyOutcome, KeyState, KeyTier, RoleMove,
    PROVIDERS,
};
use crate::error::{Error, Result};
use crate::llm_gateway::{self, CloudProvider, KeyPresence, Role};
use crate::region::{self, Region, RegionRefusal};
use crate::AppState;

/// "chat" or "background". Anything else is refused rather than guessed.
fn parse_role(role: &str) -> Result<Role> {
    match role.trim() {
        "chat" => Ok(Role::Chat),
        "background" => Ok(Role::Background),
        _ => Err(Error::Other("unknown role".into())),
    }
}

/// The table row for a provider token. OpenRouter, an unknown word and a provider with no call
/// yet are all "unknown provider": none of them has a card.
fn parse_provider(provider: &str) -> Result<&'static DirectProvider> {
    direct_providers::provider(CloudProvider::from_setting(Some(provider)))
        .ok_or_else(|| Error::Other("unknown provider".into()))
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::Chat => "Chat",
        Role::Background => "Background",
    }
}

/// What a key check returned, for the card. `saved` says whether the key is now kept. `listed` is
/// the OpenRouter-style catalogue ids the key can see.
#[derive(Debug, Serialize)]
pub struct KeyCheckResult {
    pub outcome: KeyOutcome,
    pub saved: bool,
    pub listed: Vec<String>,
    pub listed_total: usize,
    /// Set when the verdict needs a word the outcome can't carry (the test chat couldn't be tried,
    /// the key is read-only). Absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<KeyNote>,
    /// On whose terms the key is (Gemini): paid, free or unknown. Absent for a provider with no tiers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<KeyTier>,
    /// Set with `free_key_refused`, and with a re-check of a saved key that is paused: where PM thinks
    /// this computer is and why that matters. Absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<RegionRefusal>,
}

/// The whole read model for the "Your own keys" section and the role blocks. Read-only, no
/// network. A keychain that can't be read NEVER fails the command: that provider's `key` reads
/// `unreadable` and its verdict is hidden.
#[tauri::command]
pub fn direct_providers_state(app: AppHandle) -> Result<DirectProvidersState> {
    // The keychain first, with no guard held; then where this computer is (the OS time zone, no
    // network and no database); THEN the guard.
    let states: Vec<KeyState> = PROVIDERS
        .iter()
        .map(|d| {
            let (presence, fp) = direct_providers::key_state_of(d);
            (d.provider, presence, fp)
        })
        .collect();
    // Only read when a provider with tiers has a key: the verdict on a provider without them (or
    // without a key) never depends on the place, and `Outside` in its stead changes nothing.
    let region = if PROVIDERS
        .iter()
        .zip(&states)
        .any(|(d, (_, presence, _))| d.tiers && *presence == KeyPresence::Present)
    {
        region::assumed()
    } else {
        Region::Outside
    };
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    // What failed calls learned since the card was last read is written first, so the card shows it.
    direct_providers::apply_seen(&conn)?;
    direct_providers::build_state(&conn, &states, &region)
}

/// Turn a provider on or off. Off moves every role that was on it back to OpenRouter in the same
/// transaction as the flag, and returns the roles moved so the card can say so. The key is kept.
#[tauri::command]
pub fn set_direct_opt_in(app: AppHandle, provider: String, enabled: bool) -> Result<Vec<RoleMove>> {
    let d = parse_provider(&provider)?;
    let moved = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        direct_providers::set_opt_in(&conn, d.provider, enabled)?
    };
    llm_gateway::ping_status(&app);
    Ok(direct_providers::moves_to_openrouter(d.provider, &moved))
}

/// Set, or with `None` clear, a provider's daily spend limit (OpenRouter included). Copied from
/// [`set_direct_opt_in`]: written only here, never by the webview's own settings door, and refused
/// when the amount isn't above zero (turn it off with `None`, never with 0). A new limit lifts a
/// pause that the old one had set, at once.
#[tauri::command]
pub fn set_daily_limit(app: AppHandle, provider: String, usd: Option<f64>) -> Result<()> {
    // Exact words only: an unknown provider must not fall through to OpenRouter's limit.
    let p = match provider.trim() {
        "openrouter" => CloudProvider::OpenRouter,
        "openai" => CloudProvider::OpenAi,
        "gemini" => CloudProvider::Gemini,
        "anthropic" => CloudProvider::Anthropic,
        _ => return Err(Error::Other("unknown provider".into())),
    };
    {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        crate::daily_limit::write(&conn, p, usd)?;
    }
    // The On battery status judges a role on a provider the limit just paused or freed afresh.
    llm_gateway::ping_status(&app);
    Ok(())
}

/// OpenRouter's daily limit as the key block shows it. OpenRouter has no card of its own, so its
/// state rides here; the three direct providers' ride in [`direct_providers_state`].
#[tauri::command]
pub fn openrouter_limit_state(app: AppHandle) -> Result<crate::daily_limit::LimitView> {
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    Ok(crate::daily_limit::view_now(
        &conn,
        CloudProvider::OpenRouter,
        false,
    ))
}

/// Check a pasted key and keep it only if the provider accepts it. A key that isn't the shape of a
/// key is never sent; a rejected, region-blocked or unreachable result is never stored (a mistyped
/// key is never kept). A restricted key is kept: it can't list models but may still chat.
#[tauri::command]
pub async fn save_direct_key(
    app: AppHandle,
    provider: String,
    key: String,
) -> Result<KeyCheckResult> {
    let d = parse_provider(&provider)?;
    {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        if !direct_providers::opted_in(&conn, d.provider)? {
            return Err(Error::Other(format!("Turn on {} first.", d.name)));
        }
    }
    let key = key.trim().to_string();
    if !(d.well_formed)(&key) {
        return Ok(refused(KeyOutcome::Malformed));
    }
    // Pasting a key PM already holds for another provider (the OpenRouter keys, or another direct
    // provider's) would send it to another company. No network call.
    if llm_gateway::is_a_stored_key_of_another_provider(d.provider, &key) {
        return Ok(refused(KeyOutcome::Malformed));
    }
    // Nor does one company's key go to another (`sk-` to Google, `AIza` to OpenAI or Anthropic,
    // `sk-ant-` to OpenAI). No network call.
    if direct_providers::looks_like_another_providers_key(d.provider, &key) {
        return Ok(refused(KeyOutcome::Malformed));
    }
    // Where this computer is: the OS time zone, read before any guard, no network. Only for a
    // provider with tiers; the rule is the identity for any other.
    let region = if d.tiers {
        region::assumed()
    } else {
        Region::Outside
    };
    // The network, with no lock held. The key goes to the provider here, BEFORE PM knows the
    // verdict: that is how it learns the tier, and a free key can't use the probe model.
    let check = direct_providers::check_key(d.provider, &key).await;
    // A free or unknown key where PM only uses paid ones is refused: nothing is stored, and the typed
    // key stays in the field so Check again is the same submit.
    let check = direct_providers::apply_region_rule(d.provider, check, &region);
    if !check.outcome.keeps_key() {
        return Ok(KeyCheckResult {
            outcome: check.outcome,
            saved: false,
            listed: Vec::new(),
            listed_total: 0,
            note: None,
            tier: check.tier,
            refusal: check.refusal,
        });
    }
    // The keychain, then the store.
    (d.set_key)(&key)?;
    {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        direct_providers::store_key_check(
            &conn,
            d.provider,
            &check,
            &direct_providers::key_fp(&key),
        )?;
    }
    Ok(KeyCheckResult {
        outcome: check.outcome,
        saved: true,
        listed: check.listed,
        listed_total: check.listed_total,
        note: check.note,
        tier: check.tier,
        refusal: None,
    })
}

fn refused(outcome: KeyOutcome) -> KeyCheckResult {
    KeyCheckResult {
        outcome,
        saved: false,
        listed: Vec::new(),
        listed_total: 0,
        note: None,
        tier: None,
        refusal: None,
    }
}

/// Whether a re-check's answer replaces the stored verdict. PURE. Only an answer that says
/// something about the key is stored (works, restricted, no credit, rejected, region), not a
/// moment's trouble reaching the provider. And a test chat that couldn't be tried must not erase a
/// known-bad verdict (no credit, a read-only key): a rate limit during Check again says nothing
/// about whether credit was added, so the stored line and the chip stay until a real answer comes.
fn should_store_recheck(
    prev: Option<&direct_providers::StoredKeyCheck>,
    check: &direct_providers::KeyCheck,
) -> bool {
    // `Busy` is shown and stores nothing: Google had a bad second and decided nothing.
    let says_something = matches!(
        check.outcome,
        KeyOutcome::Ok
            | KeyOutcome::Restricted
            | KeyOutcome::NoCredit
            | KeyOutcome::NeedsVerification
            | KeyOutcome::Rejected
            | KeyOutcome::RegionBlocked
    );
    if !says_something {
        return false;
    }
    if check.note == Some(KeyNote::ChatUntried) {
        let known_bad = prev.is_some_and(|p| {
            p.outcome == KeyOutcome::NoCredit
                || (p.outcome == KeyOutcome::Restricted
                    && matches!(p.note, Some(KeyNote::ReadOnly | KeyNote::NoAccess)))
        });
        return !known_bad;
    }
    true
}

/// Check the stored key again (a revoked key, a project that gained access to a model). The key
/// stays whatever the answer, so `saved` is true; the verdict is stored for the answers that say
/// something about the key (works, restricted, no credit, rejected, region), and not for a moment's
/// trouble reaching the provider.
#[tauri::command]
pub async fn recheck_direct_key(app: AppHandle, provider: String) -> Result<KeyCheckResult> {
    let d = parse_provider(&provider)?;
    let key = (d.get_key)()?
        .ok_or_else(|| Error::Other(format!("There is no {} key to check.", d.name)))?;
    let fp = direct_providers::key_fp(key.expose());
    let region = if d.tiers {
        region::assumed()
    } else {
        Region::Outside
    };
    let check = direct_providers::check_key(d.provider, key.expose()).await;
    // The region rule is NOT applied here: the key is already saved, and a stored free key in the
    // region is paused at read, never deleted. A fresh verdict replaces the stored one (clearing a
    // flip), and the result says whether the key is still paused.
    let refusal = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let prev = direct_providers::read_key_check(&conn, d.provider)?;
        if should_store_recheck(prev.as_ref(), &check) {
            direct_providers::store_key_check(&conn, d.provider, &check, &fp)?;
        }
        if d.tiers && check.outcome != KeyOutcome::Rejected {
            let tier = direct_providers::tier_now(&conn, d.provider, Some(&fp))?;
            direct_providers::refuses(d.provider, tier, &region)
        } else {
            None
        }
    };
    Ok(KeyCheckResult {
        outcome: check.outcome,
        saved: true,
        listed: check.listed,
        listed_total: check.listed_total,
        note: check.note,
        tier: check.tier,
        refusal,
    })
}

/// Forget the key. The keychain delete comes first and errors if PM can't tell the key is gone
/// (never a quiet Ok); then every role that was on the provider goes back to OpenRouter and the
/// stored verdict is cleared. The opt-in stays on.
#[tauri::command]
pub fn clear_direct_key(app: AppHandle, provider: String) -> Result<Vec<RoleMove>> {
    let d = parse_provider(&provider)?;
    (d.clear_key)()?;
    let moved = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        direct_providers::clear_key_check(&conn, d.provider)?;
        direct_providers::retire_roles(&conn, d.provider)?
    };
    llm_gateway::ping_status(&app);
    Ok(direct_providers::moves_to_openrouter(d.provider, &moved))
}

/// Why a role can't be pointed at a provider, or `Ok`. PURE. OpenRouter is always allowed and
/// never reaches this. The sentences name what is missing; an unreadable keychain refuses with its
/// own, because "no key" would be a guess.
fn gate_role_provider(
    d: &DirectProvider,
    role: Role,
    opted_in: bool,
    key: KeyPresence,
    blocked: Option<&RegionRefusal>,
) -> Result<()> {
    if !opted_in {
        return Err(Error::Other(format!("Turn on {} first.", d.name)));
    }
    match key {
        // A role is never MOVED onto a provider that is paused: it would fail every call.
        KeyPresence::Present => match blocked {
            Some(r) => Err(Error::Other(r.sentence(d.provider))),
            None => Ok(()),
        },
        KeyPresence::Absent => Err(Error::Other(format!("Add your {} key first.", d.name))),
        KeyPresence::Unreadable => Err(Error::Other(format!(
            "PM can't read your {} key right now, so {} can't be moved to it.",
            d.name,
            role_name(role)
        ))),
    }
}

/// Choose which cloud provider answers a role. "openrouter" is always allowed (and deletes the
/// row); a direct provider only when opted in AND its key is there.
#[tauri::command]
pub fn set_role_cloud_provider(app: AppHandle, role: String, provider: String) -> Result<()> {
    let role = parse_role(&role)?;
    if provider.trim() == "openrouter" {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        direct_providers::set_role_provider(&conn, role, CloudProvider::OpenRouter)?;
    } else {
        let d = parse_provider(&provider)?;
        // The keychain first, then where this computer is, with no guard held.
        let (key, fp) = direct_providers::key_state_of(d);
        let region = if d.tiers {
            region::assumed()
        } else {
            Region::Outside
        };
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let opted = direct_providers::opted_in(&conn, d.provider)?;
        let blocked = if d.tiers && key == KeyPresence::Present {
            let tier = direct_providers::tier_now(&conn, d.provider, fp.as_deref())?;
            direct_providers::refuses(d.provider, tier, &region)
        } else {
            None
        };
        gate_role_provider(d, role, opted, key, blocked.as_ref())?;
        direct_providers::set_role_provider(&conn, role, d.provider)?;
    }
    llm_gateway::ping_status(&app);
    Ok(())
}

/// Choose a role's model and effort on a provider in ONE write, so the effort is checked against
/// the new model. One model per role in this version; `effort` is `null` (the role's default) or
/// a level that model offers. The word "default" is refused: it is not a level.
#[tauri::command]
pub fn set_direct_role(
    app: AppHandle,
    provider: String,
    role: String,
    models: Vec<String>,
    effort: Option<String>,
) -> Result<()> {
    let d = parse_provider(&provider)?;
    let role = parse_role(&role)?;
    {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        direct_providers::set_role_model(&conn, d.provider, role, &models, effort.as_deref())?;
    }
    llm_gateway::ping_status(&app);
    Ok(())
}

/// The person's own "my project shares data with this provider" tick-box. Display only: it changes
/// the warning's headline and the chip, and never gates a call.
#[tauri::command]
pub fn set_direct_data_sharing(app: AppHandle, provider: String, shares: bool) -> Result<()> {
    let d = parse_provider(&provider)?;
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    direct_providers::set_shares_data(&conn, d.provider, shares)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_role_and_a_provider_are_read_through_allow_lists() {
        assert_eq!(parse_role("chat").unwrap(), Role::Chat);
        assert_eq!(parse_role(" background ").unwrap(), Role::Background);
        assert!(parse_role("Chat").is_err());
        assert!(parse_role("").is_err());
        assert_eq!(parse_provider("openai").unwrap().name, "OpenAI");
        assert_eq!(parse_provider("gemini").unwrap().name, "Google Gemini");
        assert_eq!(parse_provider("anthropic").unwrap().name, "Anthropic");
        for bad in ["openrouter", "Anthropic", "", "OpenAI", "Gemini", "mythos"] {
            assert!(parse_provider(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_role_moves_to_a_provider_only_when_it_is_on_and_keyed() {
        let d = parse_provider("openai").unwrap();
        assert!(gate_role_provider(d, Role::Chat, true, KeyPresence::Present, None).is_ok());
        let off = gate_role_provider(d, Role::Chat, false, KeyPresence::Present, None).unwrap_err();
        assert!(off.to_string().contains("Turn on OpenAI"), "{off}");
        let none = gate_role_provider(d, Role::Chat, true, KeyPresence::Absent, None).unwrap_err();
        assert!(none.to_string().contains("Add your OpenAI key"), "{none}");
        // An unreadable keychain refuses too, in its own words: it is not "no key".
        let unreadable =
            gate_role_provider(d, Role::Background, true, KeyPresence::Unreadable, None)
                .unwrap_err();
        assert!(
            unreadable.to_string().contains("can't read"),
            "{unreadable}"
        );
        assert!(
            unreadable.to_string().contains("Background"),
            "{unreadable}"
        );
    }

    /// A role is never MOVED onto a paused provider, and the refusal is the same sentence a paused
    /// send gives.
    #[test]
    fn a_role_is_not_moved_onto_a_paused_provider() {
        let d = parse_provider("gemini").unwrap();
        let uk = region::region_of_on(Some("Europe/London"), false);
        let blocked = direct_providers::refuses(CloudProvider::Gemini, Some(KeyTier::Free), &uk);
        assert!(blocked.is_some());
        let e = gate_role_provider(d, Role::Chat, true, KeyPresence::Present, blocked.as_ref())
            .unwrap_err()
            .to_string();
        assert!(
            e.starts_with("Gemini is paused:") && e.contains("Europe/London"),
            "{e}"
        );
        // Paid: nothing blocks it. Turned off still says "turn on" first.
        assert!(gate_role_provider(d, Role::Chat, true, KeyPresence::Present, None).is_ok());
        let off = gate_role_provider(d, Role::Chat, false, KeyPresence::Present, blocked.as_ref())
            .unwrap_err();
        assert!(off.to_string().contains("Turn on Google Gemini"), "{off}");
    }

    /// Gemini has no tick-box: the tier is detected, so the command that writes it refuses.
    #[test]
    fn gemini_has_no_data_sharing_option() {
        let (_dir, conn) = {
            let dir = tempfile::tempdir().unwrap();
            let conn = crate::db::open(
                &dir.path().join("t.sqlite"),
                "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff",
            )
            .unwrap();
            (dir, conn)
        };
        let e = direct_providers::set_shares_data(&conn, CloudProvider::Gemini, true).unwrap_err();
        assert!(e.to_string().contains("no such option"), "{e}");
        assert!(!direct_providers::shares_data(&conn, CloudProvider::Gemini).unwrap());
        assert!(direct_providers::set_shares_data(&conn, CloudProvider::OpenAi, true).is_ok());
    }

    #[test]
    fn a_key_for_another_company_is_never_sent() {
        let gem = CloudProvider::Gemini;
        let oa = CloudProvider::OpenAi;
        assert!(direct_providers::looks_like_another_providers_key(
            gem,
            "sk-proj-abcdefghijklmnopqrst"
        ));
        assert!(direct_providers::looks_like_another_providers_key(
            oa,
            "AIzaSyAbcdefghijklmnopqrstuvwxyz0123456"
        ));
        assert!(!direct_providers::looks_like_another_providers_key(
            gem,
            "AIzaSyAbcdefghijklmnopqrstuvwxyz0123456"
        ));
        assert!(!direct_providers::looks_like_another_providers_key(
            oa,
            "sk-proj-abcdefghijklmnopqrst"
        ));
        // The shape check refuses them too, so `save_direct_key` answers `malformed` with no network.
        assert!(!(parse_provider("gemini").unwrap().well_formed)(
            "sk-proj-abcdefghijklmnopqrst"
        ));
        assert!(!(parse_provider("openai").unwrap().well_formed)(
            "AIzaSyAbcdefghijklmnopqrstuvwxyz0123456"
        ));
        assert!((parse_provider("gemini").unwrap().well_formed)(
            "AIzaSyAbcdefghijklmnopqrstuvwxyz0123456"
        ));
        // Anthropic: `sk-ant-` is its own and turned away by OpenAI and Google; `AIza` is turned away by
        // Anthropic. No prefix is REQUIRED of an Anthropic key.
        let ant = CloudProvider::Anthropic;
        assert!(direct_providers::looks_like_another_providers_key(
            oa,
            "sk-ant-api03-abcdefghijklmnopqrst"
        ));
        assert!(direct_providers::looks_like_another_providers_key(
            gem,
            "sk-ant-api03-abcdefghijklmnopqrst"
        ));
        assert!(direct_providers::looks_like_another_providers_key(
            ant,
            "AIzaSyAbcdefghijklmnopqrstuvwxyz0123456"
        ));
        assert!(!direct_providers::looks_like_another_providers_key(
            ant,
            "sk-ant-api03-abcdefghijklmnopqrst"
        ));
        assert!(!(parse_provider("openai").unwrap().well_formed)(
            "sk-ant-api03-abcdefghijklmnopqrst"
        ));
        assert!(!(parse_provider("anthropic").unwrap().well_formed)(
            "AIzaSyAbcdefghijklmnopqrstuvwxyz0123456"
        ));
        assert!((parse_provider("anthropic").unwrap().well_formed)(
            "sk-ant-api03-abcdefghijklmnopqrst"
        ));
        assert!((parse_provider("anthropic").unwrap().well_formed)(
            "a-key-shape-nobody-has-seen-yet-0123"
        ));
        // An UNSAVED OpenRouter key (`sk-or-`) is nobody's but OpenRouter's, and an OpenAI project
        // key (`sk-proj-`) is OpenAI's: neither goes to Anthropic, and OpenRouter's not to OpenAI.
        for (p, key, other) in [
            (ant, "sk-or-v1-abcdefghijklmnop", true),
            (ant, "sk-proj-abcdefghijklmnopqrst", true),
            (oa, "sk-or-v1-abcdefghijklmnop", true),
            (oa, "sk-proj-abcdefghijklmnopqrst", false),
            // Every OpenAI shape is turned away from Anthropic's box, not only the project key: the
            // service-account and admin keys, and a plain legacy `sk-` key.
            (ant, "sk-svcacct-abcdefghijklmnop", true),
            (ant, "sk-admin-abcdefghijklmnop", true),
            (ant, "sk-abcdefghijklmnopqrstuvwx", true),
            // OpenRouter's own boxes turn away the other companies' prefixed keys and take their own.
            (CloudProvider::OpenRouter, "sk-proj-abcdefghijklmnop", true),
            (
                CloudProvider::OpenRouter,
                "sk-svcacct-abcdefghijklmnop",
                true,
            ),
            (CloudProvider::OpenRouter, "sk-admin-abcdefghijklmnop", true),
            (
                CloudProvider::OpenRouter,
                "sk-ant-api03-abcdefghijklmnop",
                true,
            ),
            (
                CloudProvider::OpenRouter,
                "AIzaSyAbcdefghijklmnopqrstuvwxyz0123456",
                true,
            ),
            (
                CloudProvider::OpenRouter,
                "sk-or-v1-abcdefghijklmnop",
                false,
            ),
        ] {
            assert_eq!(
                direct_providers::looks_like_another_providers_key(p, key),
                other,
                "{key}"
            );
        }
        // The shape check turns them away too, so `save_direct_key` answers `malformed` with no
        // network, whatever sk- key was pasted into Anthropic's box.
        for key in [
            "sk-svcacct-abcdefghijklmnop",
            "sk-admin-abcdefghijklmnop",
            "sk-abcdefghijklmnopqrstuvwxyz0123",
        ] {
            assert!(
                !(parse_provider("anthropic").unwrap().well_formed)(key),
                "{key}"
            );
        }
        assert!(!(parse_provider("anthropic").unwrap().well_formed)(
            "sk-or-v1-abcdefghijklmnop"
        ));
        assert!(!(parse_provider("openai").unwrap().well_formed)(
            "sk-or-v1-abcdefghijklmnop"
        ));
    }

    /// Anthropic has no data-sharing option, and no tiers.
    #[test]
    fn anthropic_has_no_data_sharing_option_and_no_tiers() {
        let d = parse_provider("anthropic").unwrap();
        assert!(d.shares_data.is_none() && !d.tiers);
        let (_dir, conn) = {
            let dir = tempfile::tempdir().unwrap();
            let conn = crate::db::open(
                &dir.path().join("t.sqlite"),
                "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff",
            )
            .unwrap();
            (dir, conn)
        };
        let e =
            direct_providers::set_shares_data(&conn, CloudProvider::Anthropic, true).unwrap_err();
        assert!(e.to_string().contains("no such option"), "{e}");
    }

    fn stored(outcome: KeyOutcome, note: Option<KeyNote>) -> direct_providers::StoredKeyCheck {
        direct_providers::StoredKeyCheck {
            at: "2026-11-26T00:00:00Z".into(),
            outcome,
            listed: Vec::new(),
            listed_total: 0,
            note,
            tier: None,
            flipped_free_at: None,
            key_fp: None,
        }
    }

    fn checked(outcome: KeyOutcome, note: Option<KeyNote>) -> direct_providers::KeyCheck {
        let mut c = direct_providers::KeyCheck::of(outcome);
        c.note = note;
        c
    }

    #[test]
    fn a_chat_that_could_not_be_tried_never_erases_a_known_bad_verdict() {
        let untried = checked(KeyOutcome::Ok, Some(KeyNote::ChatUntried));
        for prev in [
            stored(KeyOutcome::NoCredit, None),
            stored(KeyOutcome::Restricted, Some(KeyNote::ReadOnly)),
            stored(KeyOutcome::Restricted, Some(KeyNote::NoAccess)),
        ] {
            assert!(!should_store_recheck(Some(&prev), &untried), "{prev:?}");
        }
        // Nothing known bad to protect: the answer is stored as usual.
        assert!(should_store_recheck(None, &untried));
        assert!(should_store_recheck(
            Some(&stored(KeyOutcome::Ok, None)),
            &untried
        ));
        // A real answer replaces a bad one: credit was added.
        let ok = checked(KeyOutcome::Ok, None);
        assert!(should_store_recheck(
            Some(&stored(KeyOutcome::NoCredit, None)),
            &ok
        ));
        assert!(should_store_recheck(
            Some(&stored(KeyOutcome::Ok, None)),
            &checked(KeyOutcome::NoCredit, None)
        ));
        // Trouble reaching the provider is never stored, and neither is a busy Google: it is shown and
        // decides nothing.
        assert!(!should_store_recheck(
            None,
            &checked(KeyOutcome::Unreachable, None)
        ));
        assert!(!should_store_recheck(
            Some(&stored(KeyOutcome::Ok, None)),
            &checked(KeyOutcome::Busy, None)
        ));
        // A project Google wants verified IS a verdict about the key.
        assert!(should_store_recheck(
            Some(&stored(KeyOutcome::Ok, None)),
            &checked(KeyOutcome::NeedsVerification, None)
        ));
        assert!(!should_store_recheck(
            None,
            &checked(KeyOutcome::Unexpected(502), None)
        ));
    }

    #[test]
    fn the_result_serialises_with_the_frozen_names() {
        let v = serde_json::to_value(KeyCheckResult {
            outcome: KeyOutcome::Unexpected(418),
            saved: false,
            listed: vec!["openai/gpt-6.2-sol".into()],
            listed_total: 3,
            note: None,
            tier: None,
            refusal: None,
        })
        .unwrap();
        assert_eq!(v["outcome"]["unexpected"], 418);
        assert_eq!(v["saved"], false);
        assert_eq!(v["listed_total"], 3);
        assert_eq!(v["listed"][0], "openai/gpt-6.2-sol");
        assert!(v.get("note").is_none(), "no note, no field");
        assert!(
            v.get("tier").is_none() && v.get("refusal").is_none(),
            "OpenAI's result is unchanged"
        );
        let noted = serde_json::to_value(KeyCheckResult {
            outcome: KeyOutcome::Restricted,
            saved: true,
            listed: vec![],
            listed_total: 0,
            note: Some(KeyNote::ReadOnly),
            tier: None,
            refusal: None,
        })
        .unwrap();
        assert_eq!(noted["note"], "read_only");
        // The Gemini words: a tier, a refusal that names the place and the zone, and the new outcomes.
        let uk = region::region_of_on(Some("Europe/London"), false);
        let refused_free = serde_json::to_value(KeyCheckResult {
            outcome: KeyOutcome::FreeKeyRefused,
            saved: false,
            listed: vec![],
            listed_total: 0,
            note: None,
            tier: Some(KeyTier::Free),
            refusal: uk.refusal(region::Basis::Free),
        })
        .unwrap();
        assert_eq!(refused_free["outcome"], "free_key_refused");
        assert_eq!(refused_free["tier"], "free");
        assert_eq!(refused_free["refusal"]["kind"], "uk");
        assert_eq!(refused_free["refusal"]["label"], "the United Kingdom");
        assert_eq!(refused_free["refusal"]["zone"], "Europe/London");
        assert_eq!(refused_free["refusal"]["basis"], "free");
        for (o, word) in [
            (KeyOutcome::NoCredit, "no_credit"),
            (KeyOutcome::NeedsVerification, "needs_verification"),
            (KeyOutcome::Busy, "busy"),
        ] {
            assert_eq!(serde_json::to_value(o).unwrap(), word);
        }
        assert_eq!(serde_json::to_value(KeyTier::Unknown).unwrap(), "unknown");
        assert_eq!(
            serde_json::to_value(refused(KeyOutcome::Malformed)).unwrap()["outcome"],
            "malformed"
        );
    }

    #[test]
    fn a_role_move_serialises_as_role_from_to() {
        let moved = direct_providers::moves_to_openrouter(CloudProvider::OpenAi, &[Role::Chat]);
        let v = serde_json::to_value(&moved).unwrap();
        assert_eq!(v[0]["role"], "chat");
        assert_eq!(v[0]["from"], "openai");
        assert_eq!(v[0]["to"], "openrouter");
    }

    /// No command in this file hands a key back: nothing it returns can carry one. A source check,
    /// since the types are what a key would have to ride on.
    #[test]
    fn no_command_returns_a_key() {
        let src = include_str!("direct_keys.rs");
        let body = src.split("#[cfg(test)]").next().unwrap();
        assert!(!body.contains("-> Result<Secret"));
        assert!(!body.contains("-> Result<String>"));
    }
}
