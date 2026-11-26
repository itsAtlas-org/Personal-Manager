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
    self, DirectProvider, DirectProvidersState, KeyOutcome, RoleMove, PROVIDERS,
};
use crate::error::{Error, Result};
use crate::llm_gateway::{self, CloudProvider, KeyPresence, Role};
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
}

/// The whole read model for the "Your own keys" section and the role blocks. Read-only, no
/// network. A keychain that can't be read NEVER fails the command: that provider's `key` reads
/// `unreadable` and its verdict is hidden.
#[tauri::command]
pub fn direct_providers_state(app: AppHandle) -> Result<DirectProvidersState> {
    // The keychain first, with no guard held.
    let presences: Vec<(CloudProvider, KeyPresence)> = PROVIDERS
        .iter()
        .map(|d| (d.provider, direct_providers::key_presence_of(d)))
        .collect();
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    direct_providers::build_state(&conn, &presences)
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
    // Pasting the OpenRouter key here would send it to another company. No network call.
    if llm_gateway::is_a_stored_openrouter_key(&key) {
        return Ok(refused(KeyOutcome::Malformed));
    }
    // The network, with no lock held.
    let check = direct_providers::check_key(d.provider, &key).await;
    if !check.outcome.keeps_key() {
        return Ok(KeyCheckResult {
            outcome: check.outcome,
            saved: false,
            listed: Vec::new(),
            listed_total: 0,
        });
    }
    // The keychain, then the store.
    (d.set_key)(&key)?;
    {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        direct_providers::store_key_check(&conn, d.provider, &check)?;
    }
    Ok(KeyCheckResult {
        outcome: check.outcome,
        saved: true,
        listed: check.listed,
        listed_total: check.listed_total,
    })
}

fn refused(outcome: KeyOutcome) -> KeyCheckResult {
    KeyCheckResult {
        outcome,
        saved: false,
        listed: Vec::new(),
        listed_total: 0,
    }
}

/// Check the stored key again (a revoked key, a project that gained access to a model). The key
/// stays whatever the answer, so `saved` is true; the verdict is stored for the answers that say
/// something about the key (works, restricted, rejected, region), and not for a moment's
/// trouble reaching the provider.
#[tauri::command]
pub async fn recheck_direct_key(app: AppHandle, provider: String) -> Result<KeyCheckResult> {
    let d = parse_provider(&provider)?;
    let key = (d.get_key)()?
        .ok_or_else(|| Error::Other(format!("There is no {} key to check.", d.name)))?;
    let check = direct_providers::check_key(d.provider, key.expose()).await;
    let says_something = matches!(
        check.outcome,
        KeyOutcome::Ok | KeyOutcome::Restricted | KeyOutcome::Rejected | KeyOutcome::RegionBlocked
    );
    if says_something {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        direct_providers::store_key_check(&conn, d.provider, &check)?;
    }
    Ok(KeyCheckResult {
        outcome: check.outcome,
        saved: true,
        listed: check.listed,
        listed_total: check.listed_total,
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
) -> Result<()> {
    if !opted_in {
        return Err(Error::Other(format!("Turn on {} first.", d.name)));
    }
    match key {
        KeyPresence::Present => Ok(()),
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
        // The keychain first, with no guard held.
        let key = direct_providers::key_presence_of(d);
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let opted = direct_providers::opted_in(&conn, d.provider)?;
        gate_role_provider(d, role, opted, key)?;
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
        for bad in ["openrouter", "gemini", "anthropic", "", "OpenAI"] {
            assert!(parse_provider(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_role_moves_to_a_provider_only_when_it_is_on_and_keyed() {
        let d = parse_provider("openai").unwrap();
        assert!(gate_role_provider(d, Role::Chat, true, KeyPresence::Present).is_ok());
        let off = gate_role_provider(d, Role::Chat, false, KeyPresence::Present).unwrap_err();
        assert!(off.to_string().contains("Turn on OpenAI"), "{off}");
        let none = gate_role_provider(d, Role::Chat, true, KeyPresence::Absent).unwrap_err();
        assert!(none.to_string().contains("Add your OpenAI key"), "{none}");
        // An unreadable keychain refuses too, in its own words: it is not "no key".
        let unreadable =
            gate_role_provider(d, Role::Background, true, KeyPresence::Unreadable).unwrap_err();
        assert!(
            unreadable.to_string().contains("can't read"),
            "{unreadable}"
        );
        assert!(
            unreadable.to_string().contains("Background"),
            "{unreadable}"
        );
    }

    #[test]
    fn the_result_serialises_with_the_frozen_names() {
        let v = serde_json::to_value(KeyCheckResult {
            outcome: KeyOutcome::Unexpected(418),
            saved: false,
            listed: vec!["openai/gpt-6.2-sol".into()],
            listed_total: 3,
        })
        .unwrap();
        assert_eq!(v["outcome"]["unexpected"], 418);
        assert_eq!(v["saved"], false);
        assert_eq!(v["listed_total"], 3);
        assert_eq!(v["listed"][0], "openai/gpt-6.2-sol");
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
