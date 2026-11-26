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
//! Two lock rules. The keychain is read BEFORE the DB guard is taken (on macOS a keychain read can
//! wait on a prompt), and nothing here holds a guard across an `.await`.

use serde::{Deserialize, Serialize};

use rusqlite::Connection;

use crate::direct_models::{self, DirectModel};
use crate::error::{Error, Result};
use crate::llm_gateway::{
    cloud_provider_key, role_cloud_provider, stored_role_provider, CloudProvider, KeyPresence, Role,
};
use crate::power::{DirectConsent, DIRECT_CONSENT_KEY};
use crate::secret::Secret;
use crate::{db, openai_direct, secrets};

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
    pub shares_data: &'static str,
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
    shares_data: "direct_openai_shares_data",
    get_key: secrets::get_openai_key,
    set_key: secrets::set_openai_key,
    clear_key: secrets::clear_openai_key,
    well_formed: openai_direct::well_formed,
};

/// Every direct provider. 2b and 3 append a row.
pub(crate) const PROVIDERS: &[DirectProvider] = &[OPENAI];

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
}

impl KeyCheck {
    pub(crate) fn of(outcome: KeyOutcome) -> Self {
        Self {
            outcome,
            listed: Vec::new(),
            listed_total: 0,
            note: None,
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
        CloudProvider::OpenRouter | CloudProvider::Gemini | CloudProvider::Anthropic => {
            KeyCheck::unsupported()
        }
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
}

/// Remember a verdict. `at` comes from the open connection, never a helper that would take the DB
/// guard again.
pub(crate) fn store_key_check(conn: &Connection, p: CloudProvider, check: &KeyCheck) -> Result<()> {
    let stored = StoredKeyCheck {
        at: crate::ingest::iso_now(conn)?,
        outcome: check.outcome,
        listed: check.listed.clone(),
        listed_total: check.listed_total,
        note: check.note,
    };
    let json = serde_json::to_string(&stored).map_err(|e| Error::Other(e.to_string()))?;
    db::set_setting(conn, row(p)?.key_check, &json)
}

pub(crate) fn clear_key_check(conn: &Connection, p: CloudProvider) -> Result<()> {
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

/// How the keychain answers for a provider, read WITHOUT the DB guard. A read that fails is
/// `Unreadable`, never `Absent`: the key may well be there.
pub(crate) fn key_presence_of(d: &DirectProvider) -> KeyPresence {
    match (d.get_key)() {
        Ok(Some(_)) => KeyPresence::Present,
        Ok(None) => KeyPresence::Absent,
        Err(_) => KeyPresence::Unreadable,
    }
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
}

/// The one read `cloud_arm` makes for a direct provider. The arm carries ONE model: auto-switch is
/// an OpenRouter server feature, so a stored fallback list would look like working fallbacks and
/// isn't. The storage is already an array so a later PR can lift the cap.
pub(crate) fn arm_inputs(conn: &Connection, p: CloudProvider, role: Role) -> Result<ArmInputs> {
    let first = models_for(conn, p, role)?.remove(0);
    Ok(ArmInputs {
        models: vec![first],
        effort: effort_for(conn, p, role)?,
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
    let kept = DirectConsent::from_setting(db::get_setting(conn, DIRECT_CONSENT_KEY)?.as_deref());
    match kept.without(p).as_setting() {
        Some(json) => db::set_setting(conn, DIRECT_CONSENT_KEY, &json)?,
        None => db::delete_setting(conn, DIRECT_CONSENT_KEY)?,
    }
    let mut moved = Vec::new();
    for role in [Role::Chat, Role::Background] {
        if stored_role_provider(conn, role)? == p {
            db::delete_setting(conn, cloud_provider_key(role))?;
            moved.push(role);
        }
    }
    Ok(moved)
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

pub(crate) fn shares_data(conn: &Connection, p: CloudProvider) -> Result<bool> {
    Ok(db::get_setting(conn, row(p)?.shares_data)?.as_deref() == Some("true"))
}

pub(crate) fn set_shares_data(conn: &Connection, p: CloudProvider, shares: bool) -> Result<()> {
    let key = row(p)?.shares_data;
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
}

impl From<&DirectModel> for DirectModelView {
    fn from(m: &DirectModel) -> Self {
        Self {
            id: m.id.clone(),
            name: m.name.clone(),
            note: m.note.clone(),
            context: m.context,
            max_output: m.max_output,
            price_in: m.price_in,
            price_cached_in: m.price_cached_in,
            price_out: m.price_out,
            effort_levels: m.effort_levels.clone(),
            effort_default: m.effort_default.clone(),
            input_modalities: m.input_modalities.clone(),
            checked_on: m.checked_on.clone(),
            retires_on: m.retires_on.clone(),
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

/// The whole read model, from the store and the keychain answers the caller already has. PURE over
/// its inputs, so a test feeds it an unreadable keychain. A provider missing from `presences` reads
/// as having no key.
pub(crate) fn build_state(
    conn: &Connection,
    presences: &[(CloudProvider, KeyPresence)],
) -> Result<DirectProvidersState> {
    let mut providers = Vec::new();
    for d in PROVIDERS {
        let key = presences
            .iter()
            .find(|(p, _)| *p == d.provider)
            .map_or(KeyPresence::Absent, |(_, k)| *k);
        let section = direct_models::section(d.provider)
            .ok_or_else(|| Error::Other(format!("PM has no models for {}", d.name)))?;
        providers.push(DirectProviderView {
            provider: d.provider,
            name: d.name.to_string(),
            opted_in: opted_in(conn, d.provider)?,
            key,
            key_check: if key == KeyPresence::Present {
                read_key_check(conn, d.provider)?
            } else {
                None
            },
            shares_data: shares_data(conn, d.provider)?,
            catalogue_checked_on: direct_models::checked_on().to_string(),
            terms_checked_on: section.terms_checked_on.clone(),
            roles: DirectRolesView {
                chat: role_view(conn, d.provider, Role::Chat)?,
                background: role_view(conn, d.provider, Role::Background)?,
            },
            models: section.models.iter().map(DirectModelView::from).collect(),
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
        for raw in ["", "junk", "gemini", "anthropic", "OPENROUTER"] {
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
            outcome: KeyOutcome::Ok,
            listed: vec!["openai/gpt-6.2-sol".into()],
            listed_total: 9,
            note: None,
        };
        store_key_check(&conn, OA, &check).unwrap();

        // A key present: the verdict shows.
        let s = build_state(&conn, &[(OA, KeyPresence::Present)]).unwrap();
        let p = &s.providers[0];
        assert!(p.opted_in && p.key == KeyPresence::Present);
        assert_eq!(p.key_check.as_ref().unwrap().listed_total, 9);
        assert!(p.roles.chat.uses_this_provider && !p.roles.background.uses_this_provider);
        assert_eq!(p.models.len(), 4);
        assert_eq!(s.roles.chat.stored, OA);
        assert_eq!(s.roles.chat.effective, OA);

        // A verdict carried in a backup, no key on this device: never shown.
        for presence in [KeyPresence::Absent, KeyPresence::Unreadable] {
            let s = build_state(&conn, &[(OA, presence)]).unwrap();
            assert!(s.providers[0].key_check.is_none(), "{presence:?}");
            assert_eq!(s.providers[0].key, presence);
        }

        // Turned off by hand underneath a stored role: both are reported, and they differ.
        db::delete_setting(&conn, "direct_openai_enabled").unwrap();
        let s = build_state(&conn, &[(OA, KeyPresence::Present)]).unwrap();
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
        };
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["outcome"], "no_credit");
        assert_eq!(v["note"], "chat_untried");
        assert_eq!(serde_json::from_value::<StoredKeyCheck>(v).unwrap(), c);
    }

    #[test]
    fn only_a_working_or_restricted_key_is_kept() {
        for (o, kept) in [
            (KeyOutcome::Ok, true),
            (KeyOutcome::Restricted, true),
            (KeyOutcome::NoCredit, true),
            (KeyOutcome::Rejected, false),
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
}
