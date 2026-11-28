// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! "Not working, use something else": what a failed chat reply offers instead of a dead red line.
//!
//! When a reply cannot be answered, PM shows a card with the reason in plain words and up to three
//! one-click offers, each built from what the person already has: another model on the same key, each
//! other provider they have a key for (its default model), OpenRouter's first row, and the model on
//! this computer. There is no list yet, so these are the candidates; a click is the person's own,
//! explicit choice, which is the only way a reply ever moves to another company here.
//!
//! The core is PURE ([`offers_for`], [`card_for`]) over facts gathered by [`gather`], so every rule
//! is a plain test: an offer must pass the entry gate BEFORE it is built (a free Gemini key in the UK
//! gets none), never names a provider that is paused, at its limit or already failed this turn, and
//! the reassurance line ("Nothing was sent anywhere else.") is said only when it is true.
//!
//! The card's wording that does not depend on a failure (the buttons, the announcement) lives in
//! `src/lib/notWorking.ts`; the reason lines are built here because they depend on the typed class.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::commands::direct_keys;
use crate::daily_limit;
use crate::direct_models;
use crate::direct_providers::{self, DirectProvider, PROVIDERS};
use crate::error::{Billing, Error, FailureClass, Result};
use crate::llm_gateway::{CloudProvider, Role};
use crate::AppState;

/// At most this many one-click offers: more is a menu, not a prompt.
pub(crate) const MAX_OFFERS: usize = 3;

/// A model on a provider, as the webview names it when it clicks an offer. The provider is a token
/// (`openrouter`, `openai`, `gemini`, `anthropic`, or `local`); the model is the id PM itself gave in
/// the offer. Nothing the webview sends here is trusted: the re-send checks it against the person's
/// own keys, settings and the catalogue ([`crate::llm_gateway::plan_for_entry`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryRef {
    pub provider: String,
    pub model: String,
}

/// Where an [`EntryRef`] points, once parsed. An allow-list: anything else is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    Cloud(CloudProvider),
    Local,
}

impl EntryRef {
    pub(crate) fn target(&self) -> Result<Target> {
        match self.provider.trim() {
            "local" => Ok(Target::Local),
            "openrouter" => Ok(Target::Cloud(CloudProvider::OpenRouter)),
            "openai" => Ok(Target::Cloud(CloudProvider::OpenAi)),
            "gemini" => Ok(Target::Cloud(CloudProvider::Gemini)),
            "anthropic" => Ok(Target::Cloud(CloudProvider::Anthropic)),
            _ => Err(Error::Other("PM doesn't know that provider.".into())),
        }
    }
}

/// One one-click offer on the card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Offer {
    /// A stable key for the button: `provider:model`.
    pub key: String,
    /// The token the click sends back (`openrouter`, `openai`, `gemini`, `anthropic`, `local`).
    pub provider: String,
    /// The model id the click sends back.
    pub model: String,
    /// The model as a person names it ("Gemini 3.5 Flash").
    pub label: String,
    /// Who gets the reply: "Google", "Anthropic", "OpenAI", "OpenRouter", "this computer".
    pub company: String,
    /// One line on what that company does with it, from the provider's own warning.
    pub data_note: String,
    /// `same_key`, `other_company`, `openrouter` or `local`.
    pub kind: &'static str,
}

/// How much the card can tell about a cap, when the failure was the person's own daily limit.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LimitInfo {
    pub provider: String,
    pub limit_usd: f64,
    pub spent_today_usd: f64,
    /// RFC 3339, when the pause lifts by itself.
    pub resumes_at: String,
}

/// The event the chat sends INSTEAD of a plain error when there is something to offer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NotWorking {
    /// The id of the user turn that failed: the card belongs to it.
    pub turn_id: i64,
    /// The failure class token (`busy`, `no_credit`, `refused`, ...), for the card's intro line.
    pub class: &'static str,
    pub headline: String,
    pub reason: String,
    /// The error's own sentence when the reason above is only a class line: what a person can act on
    /// ("Add credit at ...", "Make a new key ...", how long the free allowance waits). `None` when the
    /// reason already says it. Never carries a key (see `ProviderFailure::message`).
    pub detail: Option<String>,
    /// Said only when it is true. See [`reassurance`].
    pub reassurance: Option<String>,
    pub offers: Vec<Offer>,
    /// "Edit my list" is shown (it opens the role in Settings > AI & Models).
    pub can_edit_list: bool,
    pub limit: Option<LimitInfo>,
    /// Every entry already tried for this turn, this failure included. The page hands it back with the
    /// next click so the next card never offers one of them again (a chain of failures must not
    /// come back round to the first). Only an exclusion list: nothing sent to a provider reads it.
    pub tried: Vec<EntryRef>,
}

// ---- what failed ------------------------------------------------------------------------------

/// The attempt that just failed, as the card needs to know it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failed {
    /// The cloud provider that failed, or `None` for this computer's own model.
    pub provider: Option<CloudProvider>,
    /// The model id that failed, when known.
    pub model: Option<String>,
    pub class: FailureClass,
    /// Whether the failed call may have billed. `None` when PM cannot say (a local failure).
    pub billing: Option<Billing>,
    /// The sentence the error carried: used as the reason for a class with no wording of its own.
    pub message: String,
    /// How many OTHER rows of the person's list PM asked, in this same request, before this one
    /// failed (each walked past because it failed without charging). Zero for a plain single
    /// attempt. It makes the reassurance true: after a walk, "nothing was sent anywhere else" is not.
    pub earlier_asked: usize,
    /// Set when the walk STOPPED at a this-computer row it would not run (Local AI off, or no model
    /// picked yet) after this row failed: why nothing after that row was tried
    /// ([`crate::error::SkipReason::stop_sentence`]). `None` for every other failure.
    pub stopped_at_local: Option<&'static str>,
}

/// The short name of a provider in a sentence.
pub(crate) fn short_name(p: CloudProvider) -> &'static str {
    match p {
        CloudProvider::OpenRouter => "OpenRouter",
        CloudProvider::OpenAi => "OpenAI",
        CloudProvider::Gemini => "Gemini",
        CloudProvider::Anthropic => "Anthropic",
    }
}

/// Who gets a request: the company, as the data note and the button say it.
pub(crate) fn company(p: CloudProvider) -> &'static str {
    match p {
        CloudProvider::OpenRouter => "OpenRouter",
        CloudProvider::OpenAi => "OpenAI",
        CloudProvider::Gemini => "Google",
        CloudProvider::Anthropic => "Anthropic",
    }
}

/// A class with a failure's reason line, in plain words. PURE. `who` is the short name ("Anthropic",
/// "this computer"). A class with no wording of its own says the sentence the error carried.
///
/// `limit_known` is whether the card carries the limit's figures. `LimitReached` also stands for "PM
/// couldn't read today's usage, so it paused X to be safe", which is not the person's limit being
/// spent: without figures the error's own sentence is the honest reason. With them, the limit line
/// under the reason says when it starts again, so the reason does not repeat it.
pub(crate) fn reason_line(f: &Failed, limit_known: bool) -> String {
    let who = f.provider.map_or("This computer", short_name);
    match f.class {
        FailureClass::Busy => format!("{who} is busy."),
        FailureClass::RateLimited => format!("{who} is rate limiting this key."),
        FailureClass::ModelGone => format!("{who} no longer offers this model."),
        FailureClass::ContextTooLong => format!("{who} can't take this much text."),
        FailureClass::KeyBad => format!("{who} doesn't accept this key."),
        FailureClass::NoCredit => {
            format!("Your {who} key has no credit left or is at a spend limit.")
        }
        FailureClass::NeedsRetention => {
            format!("{who} needs data retention turned on for this model.")
        }
        FailureClass::SetupStale => format!("PM's settings for {who} are out of date."),
        FailureClass::LimitReached if limit_known => {
            format!("{who} is paused by your daily limit.")
        }
        FailureClass::LimitReached => f.message.clone(),
        FailureClass::Paused => match f.provider {
            Some(CloudProvider::Gemini) => "Gemini is paused where you are: free keys can't be used in the UK, EEA or Switzerland.".into(),
            _ => format!("{who} is paused where you are."),
        },
        FailureClass::Refused => format!("{who}'s safety system stopped this reply."),
        FailureClass::StoppedPartway => format!("{who} stopped partway."),
        FailureClass::Other => f.message.clone(),
    }
}

/// The sentence the error carried, when the reason is only a class line and the sentence adds to it:
/// the provider-mapped text is the one a person acts on (a leaked key, where to add credit, how long a
/// free allowance waits), so the card keeps it as a muted line under the reason. Not when the reason
/// already is that sentence, or when the limit's own figures line says the same thing. PURE.
pub(crate) fn detail_line(f: &Failed, reason: &str, limit_known: bool) -> Option<String> {
    let m = f.message.trim();
    if m.is_empty() || m == reason || (limit_known && f.class == FailureClass::LimitReached) {
        return None;
    }
    Some(m.to_string())
}

/// The line under the reason. TRUE or absent: "Nothing was sent anywhere else." needs every attempt
/// to have been refused or rejected, and a partial reply needs to say PM did not try anything else
/// on its own. A refusal and a failure PM cannot place say neither. "Stopped partway" is said only
/// for a reply that really did; a 500 or a timeout that may have billed says what PM knows (it
/// cannot tell) instead.
pub(crate) fn reassurance(f: &Failed) -> Option<String> {
    let base = checked_reassurance(f);
    match (base, f.stopped_at_local) {
        (Some(b), Some(stop)) => Some(format!("{b} {stop}")),
        (None, Some(stop)) => Some(stop.to_string()),
        (b, None) => b,
    }
}

/// [`reassurance`] without the sentence about a walk that stopped at a this-computer row.
fn checked_reassurance(f: &Failed) -> Option<String> {
    if f.class == FailureClass::Refused {
        return None;
    }
    // After a walk: PM did ask other rows of the list first. Say so, and that none of them charged
    // (a walk only ever follows a failure that did not).
    if f.earlier_asked > 0 && matches!(f.billing?, Billing::NeverLeft | Billing::Rejected) {
        return Some(match f.earlier_asked {
            1 => "PM also tried another model in your list first, and it didn't charge you.".into(),
            n => format!(
                "PM also tried {n} other models in your list first, and none of them charged you."
            ),
        });
    }
    match f.billing? {
        Billing::NeverLeft | Billing::Rejected => Some("Nothing was sent anywhere else.".into()),
        Billing::MaybeBilled if f.class == FailureClass::StoppedPartway => {
            Some("The reply stopped partway, so PM didn't try anything else on its own.".into())
        }
        Billing::MaybeBilled => Some(
            "PM can't tell whether this was charged, so it didn't try anything else on its own."
                .into(),
        ),
    }
}

/// Classes that are a "walk" class in the failure table: the card is worth showing for them even
/// with no offer to make, because the reason and "Edit my list" are still useful.
pub(crate) fn is_walk_class(c: FailureClass) -> bool {
    matches!(
        c,
        FailureClass::Busy
            | FailureClass::RateLimited
            | FailureClass::ModelGone
            | FailureClass::ContextTooLong
            | FailureClass::KeyBad
            | FailureClass::NoCredit
            | FailureClass::NeedsRetention
            | FailureClass::SetupStale
            | FailureClass::LimitReached
            | FailureClass::Paused
    )
}

// ---- what is available ------------------------------------------------------------------------

/// A model a provider can be offered with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFacts {
    pub id: String,
    pub name: String,
    /// The catalogue's note for the model (Anthropic's Fable retention line), carried into the data
    /// note.
    pub note: Option<String>,
}

/// What the person has for one direct provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectFacts {
    pub provider: CloudProvider,
    /// `Ok` only when the entry gate passed: opted in, a readable key, not paused where they are.
    pub gate_ok: bool,
    /// The daily limit has paused this provider for today.
    pub limit_reached: bool,
    /// The key reads as free (Gemini): its data note is the free-tier one.
    pub free_key: bool,
    /// Offerable models, the role's default first.
    pub models: Vec<ModelFacts>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRouterFacts {
    pub model_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFacts {
    pub model: String,
    /// False when the Local AI server is on another machine: the reply then goes to it, and the offer
    /// says so instead of "stays on this computer".
    pub on_this_computer: bool,
}

/// Everything [`offers_for`] reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    pub directs: Vec<DirectFacts>,
    /// OpenRouter's first row, only when its key is here and its limit is not reached.
    pub openrouter: Option<OpenRouterFacts>,
    /// This computer's model for the role, only when Local AI is on and a model is chosen.
    pub local: Option<LocalFacts>,
    /// The rows of the person's OWN list for the role, in order (empty for a list nobody has
    /// edited). Each carries the offer it would make, when it can answer right now.
    pub list: Vec<ListRow>,
}

/// One row of the person's own list, as the card sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListRow {
    /// The provider token (`openrouter`, `openai`, `gemini`, `anthropic`, `local`).
    pub provider: String,
    pub model: String,
    /// What clicking this row would send; `None` when the row cannot answer now (not turned on,
    /// no key, paused where the person is, at its daily limit, or remembered as not working).
    /// Built where the facts are read, so the entry gate has already been asked.
    pub offer: Option<Offer>,
}

/// One direct provider's facts from raw world readings. PURE. The entry gate is asked HERE, before an
/// offer for the provider can exist: a provider that fails it has `gate_ok == false` and is never
/// built into an offer.
pub(crate) fn direct_facts(
    d: &DirectProvider,
    role: Role,
    world: &direct_keys::EntryWorld,
    limit_reached: bool,
) -> DirectFacts {
    let gate_ok =
        direct_keys::gate_entry(d, role, world.opted_in, world.key, world.blocked.as_ref()).is_ok();
    let free_key = d.tiers
        && world
            .tier
            .is_none_or(direct_providers::KeyTier::reads_as_free);
    let mut models: Vec<ModelFacts> = Vec::new();
    // The role's default first, then the other role's default: two distinct models per provider at
    // most, which is all a "same key, another model" offer needs.
    for r in [role, other_role(role)] {
        if let Some(m) = direct_models::default_for(d.provider, r) {
            if !models.iter().any(|x| x.id == m.id) {
                models.push(ModelFacts {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    note: m.note.clone(),
                });
            }
        }
    }
    DirectFacts {
        provider: d.provider,
        gate_ok,
        limit_reached,
        free_key,
        models,
    }
}

fn other_role(role: Role) -> Role {
    match role {
        Role::Chat => Role::Background,
        Role::Background => Role::Chat,
    }
}

// ---- the data note ----------------------------------------------------------------------------

/// The provider's one-line warning, as the picker and the On battery question already say it
/// (`retention` in `src/lib/directProviders.ts`; a vitest keeps the two in step).
pub(crate) fn retention_line(p: CloudProvider, free_key: bool) -> &'static str {
    match p {
        CloudProvider::OpenAi => "OpenAI keeps requests for up to 30 days.",
        CloudProvider::Gemini if free_key => {
            "Google Gemini's free tier may train on requests, and people at Google may read them."
        }
        CloudProvider::Gemini => {
            "Google Gemini logs requests for a limited time to look for abuse, and doesn't use them to improve its products."
        }
        CloudProvider::Anthropic => {
            "Anthropic keeps requests for up to 30 days, or up to 2 years if it flags one for breaking its usage policy."
        }
        CloudProvider::OpenRouter => "OpenRouter asks for zero data retention on every request.",
    }
}

/// "{Company} gets this reply. {their warning} {the model's own note}".
fn data_note(p: CloudProvider, free_key: bool, model_note: Option<&str>) -> String {
    let mut s = format!(
        "{} gets this reply. {}",
        company(p),
        retention_line(p, free_key)
    );
    if let Some(n) = model_note {
        s.push(' ');
        s.push_str(n);
    }
    s
}

fn direct_offer(f: &DirectFacts, m: &ModelFacts, kind: &'static str) -> Offer {
    Offer {
        key: format!("{}:{}", f.provider.as_str(), m.id),
        provider: f.provider.as_str().to_string(),
        model: m.id.clone(),
        label: m.name.clone(),
        company: company(f.provider).to_string(),
        data_note: data_note(f.provider, f.free_key, m.note.as_deref()),
        kind,
    }
}

fn openrouter_offer(model_id: &str) -> Offer {
    Offer {
        key: format!("openrouter:{model_id}"),
        provider: "openrouter".into(),
        model: model_id.to_string(),
        label: model_id.to_string(),
        company: company(CloudProvider::OpenRouter).into(),
        data_note: data_note(CloudProvider::OpenRouter, false, None),
        kind: "openrouter",
    }
}

/// How a person calls the place a Local AI row sends the text: "this computer" for a loopback
/// server, "your Local AI server" for one on another machine (a LAN or public address), which is
/// true to where the reply really goes. The ONE copy of both phrases: the card, the strip, the log
/// sentences and the list chip all read it.
pub(crate) fn local_company(on_this_computer: bool) -> &'static str {
    if on_this_computer {
        "this computer"
    } else {
        "your Local AI server"
    }
}

fn local_offer(l: &LocalFacts) -> Offer {
    let company = local_company(l.on_this_computer);
    let data_note = if l.on_this_computer {
        "Stays on this computer."
    } else {
        "Goes to your Local AI server."
    };
    Offer {
        key: format!("local:{}", l.model),
        provider: "local".into(),
        model: l.model.clone(),
        label: l.model.clone(),
        company: company.into(),
        data_note: data_note.into(),
        kind: "local",
    }
}

// ---- the offers -------------------------------------------------------------------------------

/// The person's own list rows AFTER the one that failed, in list order, that can answer now and were
/// not already tried this turn. The failed row is found by provider and model (a failure that names
/// no model matches the provider's first row); when it is not in the list at all (a one-click offer
/// that failed, or a derived list) every row is a candidate. PURE.
fn list_offers(failed: &Failed, facts: &Facts, tried: &[EntryRef]) -> Vec<Offer> {
    let failed_token = failed.provider.map_or("local", CloudProvider::as_str);
    let at = facts.list.iter().position(|r| {
        r.provider == failed_token
            && (r.provider == "local"
                || failed
                    .model
                    .as_deref()
                    .is_none_or(|m| m.is_empty() || m == r.model))
    });
    let start = at.map_or(0, |i| i + 1);
    facts
        .list
        .iter()
        .skip(start)
        .filter_map(|row| {
            let mut offer = row.offer.clone()?;
            // A key-level failure is about the key, so another model on it is no way out.
            if failed.provider.is_some_and(|p| p.as_str() == row.provider)
                && failed.class.is_key_level()
            {
                return None;
            }
            if tried
                .iter()
                .any(|t| t.provider == row.provider && t.model == row.model)
            {
                return None;
            }
            if failed.provider.is_some_and(|p| p.as_str() == row.provider)
                && offer.kind == "other_company"
            {
                offer.kind = "same_key";
            }
            Some(offer)
        })
        .collect()
}

/// The offers for a failed reply, in order, at most [`MAX_OFFERS`]. PURE.
///
/// FIRST the person's own list: its rows after the one that failed, in the order they wrote them (a
/// limit stop offers the next row this way, the only way a limit moves a reply to another company).
/// Then, in order: another model on the SAME key (when the failure was about the model, not the key or the
/// account); each OTHER provider the person has a key for, with its default model; OpenRouter's first
/// row; this computer's model. Never a provider that fails the entry gate, is at its daily limit, or
/// is the one that just failed for a key-level reason; never the exact model that just failed; never
/// this computer when this computer is what failed.
///
/// `tried` is every entry already tried for this turn (see [`tried_with`]): an entry or a provider on
/// it is never offered again, so a chain of failures can't come back round to the first one.
pub(crate) fn offers_for(failed: &Failed, facts: &Facts, tried: &[EntryRef]) -> Vec<Offer> {
    let mut out: Vec<Offer> = Vec::new();
    let tried_entry = |provider: &str, model: &str| {
        tried
            .iter()
            .any(|t| t.provider == provider && t.model == model)
    };
    let tried_provider = |provider: &str| tried.iter().any(|t| t.provider == provider);

    // 0. The person's own list, after the failed row.
    out.extend(list_offers(failed, facts, tried));

    // 1. The same key, another model: only a failure about the model leaves the key worth trying.
    if let Some(p) = failed.provider {
        if !failed.class.is_key_level() {
            if let Some(f) = facts.directs.iter().find(|f| f.provider == p) {
                if f.gate_ok && !f.limit_reached {
                    if let Some(m) = f.models.iter().find(|m| {
                        failed.model.as_deref() != Some(m.id.as_str())
                            && !tried_entry(p.as_str(), &m.id)
                    }) {
                        out.push(direct_offer(f, m, "same_key"));
                    }
                }
            }
        }
    }

    // 2. Each other provider the person has a key for, its default model.
    for f in &facts.directs {
        if failed.provider == Some(f.provider)
            || !f.gate_ok
            || f.limit_reached
            || tried_provider(f.provider.as_str())
        {
            continue;
        }
        if let Some(m) = f.models.first() {
            out.push(direct_offer(f, m, "other_company"));
        }
    }

    // 3. OpenRouter's first row, unless OpenRouter is what failed (or was already tried).
    if failed.provider != Some(CloudProvider::OpenRouter) && !tried_provider("openrouter") {
        if let Some(or) = &facts.openrouter {
            out.push(openrouter_offer(&or.model_id));
        }
    }

    // 4. This computer's model, unless it is what failed or was tried first (a Local-then-cloud turn).
    if failed.provider.is_some() && !tried_provider("local") {
        if let Some(l) = &facts.local {
            out.push(local_offer(l));
        }
    }

    // A row the list already offered is not offered a second time by the key and default rules.
    let mut seen: Vec<String> = Vec::new();
    out.retain(|o| {
        let fresh = !seen.contains(&o.key);
        seen.push(o.key.clone());
        fresh
    });
    out.truncate(MAX_OFFERS);
    out
}

/// The most entries a turn's tried list keeps: the page is untrusted, and a list is only ever an
/// exclusion.
const MAX_TRIED: usize = 8;

/// The entries tried so far for this turn: what the page handed back, plus the attempt that just
/// failed. A cloud failure with no model known is named by its provider alone (an empty model), which
/// only the provider-level exclusion sees. PURE.
pub(crate) fn tried_with(failed: &Failed, earlier: &[EntryRef]) -> Vec<EntryRef> {
    let mut out: Vec<EntryRef> = earlier.iter().take(MAX_TRIED).cloned().collect();
    let this = EntryRef {
        provider: failed
            .provider
            .map_or("local", CloudProvider::as_str)
            .to_string(),
        model: failed.model.clone().unwrap_or_default(),
    };
    if !out.contains(&this) {
        out.push(this);
    }
    out
}

/// The card, or `None` when the plain error says it all: no offer to make and not a class worth a
/// card by itself. PURE. `earlier` is the entries the page says were already tried this turn.
pub(crate) fn card_for(
    turn_id: i64,
    failed: &Failed,
    facts: &Facts,
    limit: Option<LimitInfo>,
    earlier: &[EntryRef],
) -> Option<NotWorking> {
    let tried = tried_with(failed, earlier);
    let offers = offers_for(failed, facts, &tried);
    if offers.is_empty() && !is_walk_class(failed.class) {
        return None;
    }
    let limit = limit.filter(|_| failed.class == FailureClass::LimitReached);
    let headline = if failed.class == FailureClass::Refused {
        // A model that refused is working: say what happened, not that it is down.
        format!("{} didn't answer this one", subject(failed))
    } else {
        format!("{} isn't working right now", subject(failed))
    };
    let reason = reason_line(failed, limit.is_some());
    let detail = detail_line(failed, &reason, limit.is_some());
    Some(NotWorking {
        turn_id,
        class: failed.class.as_str(),
        headline,
        reason,
        detail,
        reassurance: reassurance(failed),
        offers,
        can_edit_list: true,
        limit,
        tried,
    })
}

/// What failed, as the headline names it: the model if PM knows it, else the provider.
fn subject(f: &Failed) -> String {
    if let Some(id) = f.model.as_deref() {
        if let Some(m) = direct_models::find(id) {
            return m.name.clone();
        }
        return id.rsplit('/').next().unwrap_or(id).to_string();
    }
    f.provider
        .map_or("Your model on this computer", short_name)
        .to_string()
}

// ---- gathering the facts ----------------------------------------------------------------------

/// Read what the card's offers are built from, with no database guard held across the keychain: each
/// direct provider's world first (keychain, then one short store read), then the limit read (which
/// takes the guard itself), then OpenRouter's key and first row, then this computer's model.
pub(crate) fn gather(app: &AppHandle, role: Role) -> Facts {
    let mut directs = Vec::new();
    for d in PROVIDERS {
        let Ok(world) = direct_keys::read_entry_world(app, d) else {
            continue;
        };
        // "Unavailable now": at its daily limit, or remembered as not working for the whole key.
        let limit_reached = crate::llm_gateway::limit_paused(app, d.provider)
            || crate::entry_health::global()
                .is_down(d.provider, "")
                .is_some();
        directs.push(direct_facts(d, role, &world, limit_reached));
    }

    let openrouter_limited = crate::llm_gateway::limit_paused(app, CloudProvider::OpenRouter);
    let openrouter = crate::llm_gateway::openrouter_offer_model(app, role)
        .filter(|_| !openrouter_limited)
        .filter(|m| {
            crate::entry_health::global()
                .is_down(CloudProvider::OpenRouter, m)
                .is_none()
        })
        .map(|model_id| OpenRouterFacts { model_id });

    let local = crate::llm_gateway::local_offer_model(app, role).map(|l| LocalFacts {
        model: l.model,
        on_this_computer: l.on_this_computer,
    });

    // The person's own list, when they have one: the rows the card offers FIRST once one fails.
    let stored = {
        let state = app.state::<AppState>();
        state
            .conn()
            .ok()
            .and_then(|conn| crate::route::stored_entries(&conn, role).ok().flatten())
            .unwrap_or_default()
    };
    let openrouter_keyed = !stored.is_empty()
        && crate::llm_gateway::provider_key_state(CloudProvider::OpenRouter, role)
            == crate::llm_gateway::KeyPresence::Present;
    let list = stored
        .iter()
        .map(|e| {
            list_row(
                e,
                &directs,
                local.as_ref(),
                openrouter_keyed && !openrouter_limited,
            )
        })
        .collect();

    Facts {
        directs,
        openrouter,
        local,
        list,
    }
}

/// One row of the person's list as the card sees it: the offer it would make, when it can answer now.
/// PURE over the facts already read. A direct row needs its provider to have passed the entry gate and
/// not be unavailable; an OpenRouter row needs its key and no limit; this computer's row needs Local
/// AI on with a model chosen; and none of them may be remembered as not working.
fn list_row(
    entry: &crate::route::Entry,
    directs: &[DirectFacts],
    local: Option<&LocalFacts>,
    openrouter_ok: bool,
) -> ListRow {
    use crate::route::Entry;
    let down =
        |p: CloudProvider, model: &str| crate::entry_health::global().is_down(p, model).is_some();
    match entry {
        Entry::Local => ListRow {
            provider: "local".into(),
            model: local.map(|l| l.model.clone()).unwrap_or_default(),
            offer: local.map(local_offer),
        },
        Entry::Cloud {
            provider, model, ..
        } => {
            let offer = if provider.is_direct() {
                directs
                    .iter()
                    .find(|f| f.provider == *provider)
                    .filter(|f| f.gate_ok && !f.limit_reached && !down(*provider, model))
                    .and_then(|f| {
                        direct_models::find(model).map(|m| {
                            direct_offer(
                                f,
                                &ModelFacts {
                                    id: m.id.clone(),
                                    name: m.name.clone(),
                                    note: m.note.clone(),
                                },
                                "other_company",
                            )
                        })
                    })
            } else {
                (openrouter_ok && !down(*provider, model)).then(|| openrouter_offer(model))
            };
            ListRow {
                provider: provider.as_str().to_string(),
                model: model.clone(),
                offer,
            }
        }
    }
}

/// The limit card for a provider, from the live store. `None` when its limit is off.
pub(crate) fn limit_info(app: &AppHandle, p: CloudProvider) -> Option<LimitInfo> {
    let state = app.state::<AppState>();
    let conn = state.conn().ok()?;
    let v = daily_limit::view_now(&conn, p, false);
    // A pause because PM could not read the usage has no figures worth showing (the spent amount
    // would read $0.00): the card then says the error's own sentence instead.
    if v.unreadable {
        return None;
    }
    Some(LimitInfo {
        provider: p.as_str().to_string(),
        limit_usd: v.limit_usd?,
        spent_today_usd: v.spent_today_usd,
        resumes_at: v.resumes_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_gateway::KeyPresence;

    fn failed(provider: Option<CloudProvider>, model: Option<&str>, class: FailureClass) -> Failed {
        Failed {
            provider,
            model: model.map(str::to_string),
            class,
            billing: Some(Billing::Rejected),
            message: "The sentence the error carried.".into(),
            earlier_asked: 0,
            stopped_at_local: None,
        }
    }

    fn direct(p: CloudProvider, ok: bool, limit: bool, ids: &[(&str, &str)]) -> DirectFacts {
        DirectFacts {
            provider: p,
            gate_ok: ok,
            limit_reached: limit,
            free_key: false,
            models: ids
                .iter()
                .map(|(id, name)| ModelFacts {
                    id: (*id).into(),
                    name: (*name).into(),
                    note: None,
                })
                .collect(),
        }
    }

    const CLAUDE: [(&str, &str); 2] = [
        ("anthropic/claude-sonnet-5-5", "Claude Sonnet 5.5"),
        ("anthropic/claude-haiku-5-5", "Claude Haiku 5.5"),
    ];
    const GEM: [(&str, &str); 1] = [("gemini/gemini-3.5-flash", "Gemini 3.5 Flash")];
    const OAI: [(&str, &str); 1] = [("openai/gpt-6-luna", "GPT-6 Luna")];

    fn everything() -> Facts {
        Facts {
            directs: vec![
                direct(CloudProvider::OpenAi, true, false, &OAI),
                direct(CloudProvider::Gemini, true, false, &GEM),
                direct(CloudProvider::Anthropic, true, false, &CLAUDE),
            ],
            openrouter: Some(OpenRouterFacts {
                model_id: "anthropic/claude-sonnet-5.5".into(),
            }),
            local: Some(LocalFacts {
                model: "llama3".into(),
                on_this_computer: true,
            }),
            list: Vec::new(),
        }
    }

    fn row(provider: &str, model: &str, usable: bool) -> ListRow {
        ListRow {
            provider: provider.into(),
            model: model.into(),
            offer: usable.then(|| Offer {
                key: format!("{provider}:{model}"),
                provider: provider.into(),
                model: model.into(),
                label: model.into(),
                company: "Someone".into(),
                data_note: "Someone gets this reply.".into(),
                kind: "other_company",
            }),
        }
    }

    fn with_list(rows: Vec<ListRow>) -> Facts {
        Facts {
            list: rows,
            ..everything()
        }
    }

    /// The person's own rows after the failed one come first, in the order they wrote them, ahead of
    /// every default the keys would suggest.
    #[test]
    fn the_persons_own_rows_after_the_failed_one_are_offered_first() {
        let facts = with_list(vec![
            row("anthropic", "anthropic/claude-sonnet-5-5", true),
            row("openrouter", "x/second", true),
            row("gemini", "gemini/gemini-3.5-flash", true),
        ]);
        let f = failed(
            Some(CloudProvider::Anthropic),
            Some("anthropic/claude-sonnet-5-5"),
            FailureClass::Busy,
        );
        let got = offers_for(&f, &facts);
        assert_eq!(
            keys(&got)[..2],
            ["openrouter:x/second", "gemini:gemini/gemini-3.5-flash"]
        );
        assert_eq!(got.len(), MAX_OFFERS);
    }

    /// A row ABOVE the failed one already had its turn (it is why this one was reached), so it is
    /// never offered back; a row that cannot answer now is skipped, not offered.
    #[test]
    fn rows_above_the_failed_one_and_rows_that_cannot_answer_are_never_offered() {
        let facts = with_list(vec![
            row("gemini", "gemini/gemini-3.5-flash", true),
            row("anthropic", "anthropic/claude-sonnet-5-5", true),
            row("openrouter", "x/down", false),
            row("local", "llama3", true),
        ]);
        let f = failed(
            Some(CloudProvider::Anthropic),
            Some("anthropic/claude-sonnet-5-5"),
            FailureClass::Busy,
        );
        let got = offers_for(&f, &facts);
        assert_eq!(keys(&got)[0], "local:llama3", "{got:?}");
        assert!(!keys(&got).contains(&"openrouter:x/down".to_string()));
        // The first offer is the list's own, and the same row is not offered twice by the defaults.
        assert_eq!(
            keys(&got).iter().filter(|k| *k == "local:llama3").count(),
            1
        );
    }

    /// A limit stop offers the next row of the list: the only way a daily limit moves a reply to
    /// another company, and only by the person's click.
    #[test]
    fn a_limit_stop_offers_the_next_row_of_the_list() {
        let facts = with_list(vec![
            row("anthropic", "anthropic/claude-sonnet-5-5", true),
            row("gemini", "gemini/gemini-3.5-flash", true),
        ]);
        let f = failed(
            Some(CloudProvider::Anthropic),
            Some("anthropic/claude-sonnet-5-5"),
            FailureClass::LimitReached,
        );
        assert_eq!(
            keys(&offers_for(&f, &facts))[0],
            "gemini:gemini/gemini-3.5-flash"
        );
    }

    /// A key-level failure is about the key, so the list's later rows on that key are no way out;
    /// and a row already tried this turn is never offered again.
    #[test]
    fn a_dead_key_and_an_already_tried_row_take_their_list_rows_out() {
        let facts = with_list(vec![
            row("anthropic", "anthropic/claude-sonnet-5-5", true),
            row("anthropic", "anthropic/claude-haiku-5-5", true),
            row("gemini", "gemini/gemini-3.5-flash", true),
        ]);
        let f = failed(
            Some(CloudProvider::Anthropic),
            Some("anthropic/claude-sonnet-5-5"),
            FailureClass::NoCredit,
        );
        let got = offers_for(&f, &facts);
        assert!(!keys(&got).contains(&"anthropic:anthropic/claude-haiku-5-5".to_string()));
        assert_eq!(keys(&got)[0], "gemini:gemini/gemini-3.5-flash");
        let tried = [EntryRef {
            provider: "gemini".into(),
            model: "gemini/gemini-3.5-flash".into(),
        }];
        let got = super::offers_for(&f, &facts, &tried_with(&f, &tried));
        assert!(!keys(&got).contains(&"gemini:gemini/gemini-3.5-flash".to_string()));
    }

    /// A failure that is not a row of the list (a one-click offer that failed) leaves every row a
    /// candidate; a model that failed on the same key but is a different row offers the next one as
    /// "same key".
    #[test]
    fn a_failure_outside_the_list_leaves_every_row_a_candidate() {
        let facts = with_list(vec![row("openrouter", "x/first", true)]);
        let f = failed(
            Some(CloudProvider::Gemini),
            Some("gemini/not-in-the-list"),
            FailureClass::Busy,
        );
        assert_eq!(keys(&offers_for(&f, &facts))[0], "openrouter:x/first");
    }

    fn keys(o: &[Offer]) -> Vec<String> {
        o.iter().map(|o| o.key.clone()).collect()
    }

    // The first card of a turn: nothing was tried before this failure. (These shadow the real
    // functions so the tests that are not about earlier attempts read as they did; the ones that are
    // call `super::` with an explicit list.)
    fn offers_for(f: &Failed, facts: &Facts) -> Vec<Offer> {
        super::offers_for(f, facts, &tried_with(f, &[]))
    }
    fn card_for(
        turn: i64,
        f: &Failed,
        facts: &Facts,
        limit: Option<LimitInfo>,
    ) -> Option<NotWorking> {
        super::card_for(turn, f, facts, limit, &[])
    }

    /// A model that is gone leaves the key worth trying: another model on the same key comes FIRST,
    /// then each other company's default, in the order the cards list them, capped at three.
    #[test]
    fn a_model_level_failure_offers_the_same_key_first_then_other_companies_capped_at_three() {
        let f = failed(
            Some(CloudProvider::Anthropic),
            Some("anthropic/claude-sonnet-5-5"),
            FailureClass::Busy,
        );
        let o = offers_for(&f, &everything());
        assert_eq!(
            keys(&o),
            [
                "anthropic:anthropic/claude-haiku-5-5",
                "openai:openai/gpt-6-luna",
                "gemini:gemini/gemini-3.5-flash",
            ]
        );
        assert_eq!(o[0].kind, "same_key");
        assert_eq!(o[1].kind, "other_company");
        assert!(o.len() <= MAX_OFFERS);
    }

    /// A key-level failure (bad key, no credit, a limit, a pause) makes another model on the same key
    /// pointless: the provider is out entirely, and the others carry the offers.
    #[test]
    fn a_key_level_failure_leaves_the_failed_provider_out_entirely() {
        for class in [
            FailureClass::KeyBad,
            FailureClass::NoCredit,
            FailureClass::LimitReached,
            FailureClass::Paused,
        ] {
            let f = failed(
                Some(CloudProvider::Anthropic),
                Some("anthropic/claude-sonnet-5-5"),
                class,
            );
            let o = offers_for(&f, &everything());
            assert!(
                o.iter().all(|o| o.provider != "anthropic"),
                "{class:?}: {:?}",
                keys(&o)
            );
            assert_eq!(
                keys(&o),
                [
                    "openai:openai/gpt-6-luna",
                    "gemini:gemini/gemini-3.5-flash",
                    "openrouter:anthropic/claude-sonnet-5.5",
                ],
                "{class:?}"
            );
        }
    }

    /// Never an entry that is paused or limit-reached (or fails the gate), and never the model that
    /// just failed.
    #[test]
    fn a_provider_that_fails_the_gate_or_is_at_its_limit_gets_no_offer() {
        let mut facts = everything();
        facts.directs[0].gate_ok = false; // OpenAI: no key / not opted in
        facts.directs[1].limit_reached = true; // Gemini: at its daily limit
        facts.openrouter = None;
        facts.local = None;
        let f = failed(
            Some(CloudProvider::Anthropic),
            Some("anthropic/claude-sonnet-5-5"),
            FailureClass::NoCredit,
        );
        assert!(offers_for(&f, &facts).is_empty());
        // The same model is never offered back to itself.
        let f = failed(
            Some(CloudProvider::Anthropic),
            Some("anthropic/claude-haiku-5-5"),
            FailureClass::Busy,
        );
        let o = offers_for(&f, &facts);
        assert_eq!(keys(&o), ["anthropic:anthropic/claude-sonnet-5-5"]);
    }

    /// OpenRouter is not offered to itself; the local model is not offered when it is what failed, and
    /// is offered after a cloud failure.
    #[test]
    fn openrouter_and_this_computer_are_offered_only_when_they_are_not_what_failed() {
        let f = failed(
            Some(CloudProvider::OpenRouter),
            Some("x/y"),
            FailureClass::Busy,
        );
        let o = offers_for(&f, &everything());
        assert!(o.iter().all(|o| o.provider != "openrouter"));
        let local_failed = failed(None, Some("llama3"), FailureClass::Other);
        let o = offers_for(&local_failed, &everything());
        assert!(o.iter().all(|o| o.provider != "local"));
        let only_local = Facts {
            local: Some(LocalFacts {
                model: "llama3".into(),
                on_this_computer: true,
            }),
            ..Facts::default()
        };
        let cloud = failed(Some(CloudProvider::Gemini), None, FailureClass::Busy);
        assert_eq!(keys(&offers_for(&cloud, &only_local)), ["local:llama3"]);
    }

    /// Every offer's note carries its provider's own warning (the picker's line), the Fable note rides
    /// along, and a free Gemini key gets Google's free-tier line, not the paid one.
    #[test]
    fn each_offer_carries_its_providers_warning_and_the_free_and_fable_variants() {
        let mut facts = everything();
        facts.directs[1].free_key = true;
        facts.directs[2].models[0].note =
            Some("Anthropic keeps what you send to Fable for 30 days.".into());
        let f = failed(
            Some(CloudProvider::OpenAi),
            Some("openai/x"),
            FailureClass::KeyBad,
        );
        let o = offers_for(&f, &facts);
        let gem = o.iter().find(|o| o.provider == "gemini").unwrap();
        assert_eq!(
            gem.data_note,
            "Google gets this reply. Google Gemini's free tier may train on requests, and people at Google may read them."
        );
        let claude = o.iter().find(|o| o.provider == "anthropic").unwrap();
        assert!(claude
            .data_note
            .starts_with("Anthropic gets this reply. Anthropic keeps requests for up to 30 days"));
        assert!(claude.data_note.ends_with("for 30 days."));
        assert!(claude.data_note.contains("Fable"));
        let or = facts.openrouter.clone().unwrap();
        let o = offers_for(
            &failed(Some(CloudProvider::Gemini), None, FailureClass::Busy),
            &Facts {
                openrouter: Some(or),
                ..Facts::default()
            },
        );
        assert_eq!(
            o[0].data_note,
            "OpenRouter gets this reply. OpenRouter asks for zero data retention on every request."
        );
    }

    /// The reassurance is said only when it is true: an unbilled or rejected failure says nothing was
    /// sent anywhere else; a failure that may have billed says PM did not try anything else; a
    /// refusal and a failure PM cannot place say neither.
    #[test]
    fn the_reassurance_is_only_said_when_it_is_true() {
        let mut f = failed(Some(CloudProvider::Anthropic), None, FailureClass::Busy);
        for b in [Billing::NeverLeft, Billing::Rejected] {
            f.billing = Some(b);
            assert_eq!(
                reassurance(&f).as_deref(),
                Some("Nothing was sent anywhere else.")
            );
        }
        f.billing = Some(Billing::MaybeBilled);
        f.class = FailureClass::StoppedPartway;
        assert_eq!(
            reassurance(&f).as_deref(),
            Some("The reply stopped partway, so PM didn't try anything else on its own.")
        );
        // A 500 or a timeout that may have billed did not "stop partway": no reply started.
        f.class = FailureClass::Busy;
        let said = reassurance(&f).unwrap();
        assert!(!said.contains("partway"), "{said}");
        assert!(
            said.contains("can't tell whether this was charged"),
            "{said}"
        );
        f.billing = None;
        assert_eq!(reassurance(&f), None);
        f.billing = Some(Billing::MaybeBilled);
        f.class = FailureClass::Refused;
        assert_eq!(
            reassurance(&f),
            None,
            "a refusal is billed and says nothing of the kind"
        );
    }

    #[test]
    fn the_reason_lines_name_the_failure_in_plain_words() {
        let line = |p, c| reason_line(&failed(Some(p), None, c), true);
        assert_eq!(
            line(CloudProvider::Anthropic, FailureClass::NoCredit),
            "Your Anthropic key has no credit left or is at a spend limit."
        );
        assert_eq!(
            line(CloudProvider::Anthropic, FailureClass::Busy),
            "Anthropic is busy."
        );
        assert_eq!(
            line(CloudProvider::Anthropic, FailureClass::Refused),
            "Anthropic's safety system stopped this reply."
        );
        assert_eq!(
            line(CloudProvider::Anthropic, FailureClass::LimitReached),
            "Anthropic is paused by your daily limit."
        );
        // "PM couldn't read today's usage" is also a LimitReached: without the limit's figures the
        // reason is the error's own sentence, never "your daily limit" or a $0.00 spent.
        let unreadable = failed(
            Some(CloudProvider::Anthropic),
            None,
            FailureClass::LimitReached,
        );
        assert_eq!(
            reason_line(&unreadable, false),
            "The sentence the error carried."
        );
        assert_eq!(
            line(CloudProvider::Gemini, FailureClass::Paused),
            "Gemini is paused where you are: free keys can't be used in the UK, EEA or Switzerland."
        );
        // A class with no wording of its own says the sentence the error carried.
        assert_eq!(
            line(CloudProvider::OpenAi, FailureClass::Other),
            "The sentence the error carried."
        );
    }

    /// The card appears when there is an offer, or when the class is one the table lets walk; a
    /// failure with no class worth a card and nothing to offer keeps today's plain error. The limit
    /// card carries the limit, and no other card does.
    #[test]
    fn the_card_is_for_an_offer_or_a_walk_class_and_the_limit_rides_only_on_its_own() {
        let none = Facts::default();
        let other = failed(Some(CloudProvider::OpenAi), None, FailureClass::Other);
        assert!(card_for(7, &other, &none, None).is_none(), "plain error");
        let refused = failed(Some(CloudProvider::OpenAi), None, FailureClass::Refused);
        assert!(card_for(7, &refused, &none, None).is_none());
        let busy = failed(Some(CloudProvider::OpenAi), None, FailureClass::Busy);
        let card = card_for(7, &busy, &none, None).expect("a walk class gets a card");
        assert!(card.offers.is_empty());
        assert_eq!(card.turn_id, 7);
        assert_eq!(card.class, "busy");
        assert!(card.limit.is_none());
        // An offer alone is enough, even for a class that never walks.
        let card = card_for(7, &refused, &everything(), None).expect("offers");
        assert_eq!(card.offers.len(), MAX_OFFERS);
        assert!(card.reassurance.is_none());

        let lim = LimitInfo {
            provider: "anthropic".into(),
            limit_usd: 5.0,
            spent_today_usd: 5.12,
            resumes_at: "2026-11-28T00:00:00+00:00".into(),
        };
        let limited = failed(
            Some(CloudProvider::Anthropic),
            None,
            FailureClass::LimitReached,
        );
        let card = card_for(1, &limited, &everything(), Some(lim.clone())).unwrap();
        assert_eq!(card.limit, Some(lim.clone()));
        let card = card_for(1, &busy, &everything(), Some(lim)).unwrap();
        assert!(card.limit.is_none());
    }

    #[test]
    fn the_headline_names_the_model_by_its_catalogue_name() {
        let f = failed(
            Some(CloudProvider::Anthropic),
            Some("anthropic/claude-sonnet-5-5"),
            FailureClass::Busy,
        );
        assert_eq!(subject(&f), "Claude Sonnet 5.5");
        let f = failed(
            Some(CloudProvider::OpenRouter),
            Some("some/model-x"),
            FailureClass::Busy,
        );
        assert_eq!(subject(&f), "model-x");
        let f = failed(None, None, FailureClass::Other);
        assert_eq!(subject(&f), "Your model on this computer");
        let f = failed(Some(CloudProvider::Gemini), None, FailureClass::Busy);
        assert_eq!(subject(&f), "Gemini");
    }

    /// A chain of failures never comes back round: Claude fails, the person clicks Gemini, Gemini
    /// fails, and the new card does not offer Claude again (nor this computer, if it was tried).
    #[test]
    fn an_entry_already_tried_this_turn_is_never_offered_again() {
        let claude = failed(
            Some(CloudProvider::Anthropic),
            Some("anthropic/claude-sonnet-5-5"),
            FailureClass::Busy,
        );
        let first = card_for(3, &claude, &everything(), None).unwrap();
        assert_eq!(first.tried.len(), 1);
        assert!(first.offers.iter().any(|o| o.provider == "gemini"));

        // The person clicked Gemini; the page hands back what the card said was tried.
        let gem = failed(
            Some(CloudProvider::Gemini),
            Some("gemini/gemini-3.5-flash"),
            FailureClass::Busy,
        );
        let second = super::card_for(3, &gem, &everything(), None, &first.tried).unwrap();
        assert_eq!(second.tried.len(), 2);
        for o in &second.offers {
            assert!(
                o.provider != "anthropic" && o.provider != "gemini",
                "{:?}",
                o.key
            );
        }
        assert_eq!(
            keys(&second.offers),
            [
                "openai:openai/gpt-6-luna",
                "openrouter:anthropic/claude-sonnet-5.5",
                "local:llama3"
            ]
        );

        // This computer tried first (a Local-then-cloud turn that fell to the cloud and failed).
        let local_first = [EntryRef {
            provider: "local".into(),
            model: "llama3".into(),
        }];
        let o = super::offers_for(&claude, &everything(), &tried_with(&claude, &local_first));
        assert!(o.iter().all(|o| o.provider != "local"), "{:?}", keys(&o));

        // The list is untrusted and bounded.
        let many: Vec<EntryRef> = (0..50)
            .map(|i| EntryRef {
                provider: "openai".into(),
                model: format!("m{i}"),
            })
            .collect();
        assert!(tried_with(&claude, &many).len() <= MAX_TRIED + 1);
    }

    /// A refusal's headline does not say the model is down: it answered, and said no.
    #[test]
    fn a_refusals_headline_says_it_did_not_answer_not_that_it_is_down() {
        let refused = failed(
            Some(CloudProvider::Anthropic),
            Some("anthropic/claude-sonnet-5-5"),
            FailureClass::Refused,
        );
        let card = card_for(1, &refused, &everything(), None).unwrap();
        assert_eq!(card.headline, "Claude Sonnet 5.5 didn't answer this one");
        let busy = failed(
            Some(CloudProvider::Anthropic),
            Some("anthropic/claude-sonnet-5-5"),
            FailureClass::Busy,
        );
        let card = card_for(1, &busy, &everything(), None).unwrap();
        assert_eq!(card.headline, "Claude Sonnet 5.5 isn't working right now");
    }

    /// A limit pause PM could not read has no figures: the card says the sentence the error carried,
    /// and carries no limit.
    #[test]
    fn an_unreadable_limit_card_says_the_errors_own_sentence_and_carries_no_figures() {
        let unreadable = failed(
            Some(CloudProvider::Anthropic),
            None,
            FailureClass::LimitReached,
        );
        let card = card_for(1, &unreadable, &everything(), None).unwrap();
        assert!(card.limit.is_none());
        assert_eq!(card.reason, "The sentence the error carried.");
    }

    /// An entry the webview sends is an allow-listed provider token, never a guess.
    #[test]
    fn an_entry_ref_parses_only_known_providers() {
        let e = |p: &str| EntryRef {
            provider: p.into(),
            model: "m".into(),
        };
        assert_eq!(e("local").target().unwrap(), Target::Local);
        assert_eq!(
            e("gemini").target().unwrap(),
            Target::Cloud(CloudProvider::Gemini)
        );
        assert!(e("deepseek").target().is_err());
        assert!(e("").target().is_err());
    }

    /// The real gate decides `gate_ok`: a free Gemini key in the UK (blocked) gets NO offer, even when
    /// every other fact is fine, and a paid key outside the region does.
    #[test]
    fn a_free_gemini_key_in_the_uk_builds_no_offer() {
        use crate::region::{region_of_on, Basis};
        let d = direct_providers::provider(CloudProvider::Gemini).unwrap();
        let uk = region_of_on(Some("Europe/London"), false);
        let blocked = direct_providers::refuses(
            CloudProvider::Gemini,
            Some(direct_providers::KeyTier::Free),
            &uk,
        );
        assert!(blocked.is_some(), "the fixture must really be a refusal");
        let _ = Basis::Free;
        let world = direct_keys::EntryWorld {
            opted_in: true,
            key: KeyPresence::Present,
            blocked,
            tier: Some(direct_providers::KeyTier::Free),
        };
        let facts = direct_facts(d, Role::Chat, &world, false);
        assert!(!facts.gate_ok);
        let f = failed(Some(CloudProvider::Anthropic), None, FailureClass::NoCredit);
        let offers = offers_for(
            &f,
            &Facts {
                directs: vec![facts],
                ..Facts::default()
            },
        );
        assert!(offers.is_empty(), "{:?}", keys(&offers));

        // Outside the region the same key is offered, with its free-tier note.
        let nyc = region_of_on(Some("America/New_York"), false);
        let world = direct_keys::EntryWorld {
            opted_in: true,
            key: KeyPresence::Present,
            blocked: direct_providers::refuses(
                CloudProvider::Gemini,
                Some(direct_providers::KeyTier::Free),
                &nyc,
            ),
            tier: Some(direct_providers::KeyTier::Free),
        };
        let facts = direct_facts(d, Role::Chat, &world, false);
        assert!(facts.gate_ok && facts.free_key);
        let offers = offers_for(
            &f,
            &Facts {
                directs: vec![facts],
                ..Facts::default()
            },
        );
        assert_eq!(offers.len(), 1);
        assert!(offers[0].data_note.contains("free tier may train"));
    }

    /// Opted out, no key or an unreadable keychain: each fails the same gate the role setter uses.
    #[test]
    fn a_provider_that_is_off_or_keyless_builds_no_offer() {
        for (opted_in, key) in [
            (false, KeyPresence::Present),
            (true, KeyPresence::Absent),
            (true, KeyPresence::Unreadable),
        ] {
            let d = direct_providers::provider(CloudProvider::OpenAi).unwrap();
            let world = direct_keys::EntryWorld {
                opted_in,
                key,
                blocked: None,
                tier: None,
            };
            assert!(!direct_facts(d, Role::Chat, &world, false).gate_ok);
        }
        let d = direct_providers::provider(CloudProvider::OpenAi).unwrap();
        let ok = direct_keys::EntryWorld {
            opted_in: true,
            key: KeyPresence::Present,
            blocked: None,
            tier: None,
        };
        let f = direct_facts(d, Role::Chat, &ok, false);
        assert!(f.gate_ok && !f.free_key);
        assert!(!f.models.is_empty());
        // Fable is never a default, so the catalogue's Fable note never rides on a default offer.
        let a = direct_providers::provider(CloudProvider::Anthropic).unwrap();
        let f = direct_facts(a, Role::Chat, &ok, false);
        assert!(f.models.iter().all(|m| m.note.is_none()));
    }

    /// Sparse facts, so the cap of three is never what hides a guard: this computer was tried first,
    /// so it is not offered even when it is the only candidate left.
    #[test]
    fn this_computer_is_not_offered_again_when_it_was_tried_first() {
        let facts = Facts {
            directs: vec![direct(CloudProvider::Gemini, true, false, &GEM)],
            local: Some(LocalFacts {
                model: "llama3".into(),
                on_this_computer: true,
            }),
            ..Facts::default()
        };
        let claude = failed(
            Some(CloudProvider::Anthropic),
            Some("anthropic/claude-sonnet-5-5"),
            FailureClass::Busy,
        );
        let fresh = super::offers_for(&claude, &facts, &tried_with(&claude, &[]));
        assert_eq!(
            keys(&fresh),
            ["gemini:gemini/gemini-3.5-flash", "local:llama3"]
        );
        let local_first = [EntryRef {
            provider: "local".into(),
            model: "llama3".into(),
        }];
        let o = super::offers_for(&claude, &facts, &tried_with(&claude, &local_first));
        assert_eq!(keys(&o), ["gemini:gemini/gemini-3.5-flash"]);
    }

    /// The chain Claude, then OpenRouter, then Gemini: when Gemini fails OpenRouter must not come back
    /// (nor Claude). Sparse facts again, so the cap hides nothing.
    #[test]
    fn a_chain_through_openrouter_never_comes_back_to_it() {
        let facts = Facts {
            directs: vec![
                direct(CloudProvider::Anthropic, true, false, &CLAUDE),
                direct(CloudProvider::Gemini, true, false, &GEM),
            ],
            openrouter: Some(OpenRouterFacts {
                model_id: "anthropic/claude-sonnet-5.5".into(),
            }),
            ..Facts::default()
        };
        let claude = failed(
            Some(CloudProvider::Anthropic),
            Some("anthropic/claude-sonnet-5-5"),
            FailureClass::KeyBad,
        );
        let first = super::card_for(1, &claude, &facts, None, &[]).unwrap();
        assert_eq!(
            keys(&first.offers),
            [
                "gemini:gemini/gemini-3.5-flash",
                "openrouter:anthropic/claude-sonnet-5.5"
            ]
        );
        // The person clicked OpenRouter and it failed.
        let via_or = failed(
            Some(CloudProvider::OpenRouter),
            Some("anthropic/claude-sonnet-5.5"),
            FailureClass::Busy,
        );
        let second = super::card_for(1, &via_or, &facts, None, &first.tried).unwrap();
        assert_eq!(keys(&second.offers), ["gemini:gemini/gemini-3.5-flash"]);
        // The person clicked Gemini and it failed: nothing is left, and OpenRouter is not back.
        let gem = failed(
            Some(CloudProvider::Gemini),
            Some("gemini/gemini-3.5-flash"),
            FailureClass::Busy,
        );
        let third = super::card_for(1, &gem, &facts, None, &second.tried).unwrap();
        assert!(third.offers.is_empty(), "{:?}", keys(&third.offers));
        // OpenRouter tried and a different provider failing: the provider-level exclusion alone.
        let tried_or = [EntryRef {
            provider: "openrouter".into(),
            model: "whatever".into(),
        }];
        let o = super::offers_for(&gem, &facts, &tried_with(&gem, &tried_or));
        assert!(
            o.iter().all(|o| o.provider != "openrouter"),
            "{:?}",
            keys(&o)
        );
    }

    /// Sonnet failed with a model-level error and the person clicked Haiku on the same key; when Haiku
    /// fails too, Sonnet is not offered back, and the key has nothing else to offer.
    #[test]
    fn the_same_key_does_not_come_back_to_a_model_already_tried() {
        let facts = Facts {
            directs: vec![direct(CloudProvider::Anthropic, true, false, &CLAUDE)],
            ..Facts::default()
        };
        let sonnet = failed(
            Some(CloudProvider::Anthropic),
            Some("anthropic/claude-sonnet-5-5"),
            FailureClass::Busy,
        );
        let first = super::card_for(1, &sonnet, &facts, None, &[]).unwrap();
        assert_eq!(
            keys(&first.offers),
            ["anthropic:anthropic/claude-haiku-5-5"]
        );
        let haiku = failed(
            Some(CloudProvider::Anthropic),
            Some("anthropic/claude-haiku-5-5"),
            FailureClass::Busy,
        );
        let second = super::card_for(1, &haiku, &facts, None, &first.tried).unwrap();
        assert!(second.offers.is_empty(), "{:?}", keys(&second.offers));
    }

    /// A Local AI server on another machine is not "this computer": the offer says where the reply
    /// goes, in both the button's company and the data note.
    #[test]
    fn a_local_ai_server_elsewhere_is_not_said_to_stay_on_this_computer() {
        let cloud = failed(Some(CloudProvider::Gemini), None, FailureClass::Busy);
        let at = |on_this_computer| {
            let facts = Facts {
                local: Some(LocalFacts {
                    model: "llama3".into(),
                    on_this_computer,
                }),
                ..Facts::default()
            };
            offers_for(&cloud, &facts).remove(0)
        };
        let here = at(true);
        assert_eq!(
            (here.company.as_str(), here.data_note.as_str()),
            ("this computer", "Stays on this computer.")
        );
        let there = at(false);
        assert_eq!(
            (there.company.as_str(), there.data_note.as_str()),
            ("your Local AI server", "Goes to your Local AI server.")
        );
        assert!(!there.data_note.contains("Stays on this computer"));
    }

    /// The reason is a class line; the sentence the error carried (what a person can act on) rides
    /// beside it as the detail, for every class that has wording of its own. Not repeated when the
    /// reason already is that sentence, nor under a limit card's own figures.
    #[test]
    fn the_errors_own_sentence_survives_as_the_detail() {
        let sentence = "Google has blocked this key because it was found in public. Make a new key at aistudio.google.com.";
        for class in [
            FailureClass::KeyBad,
            FailureClass::NoCredit,
            FailureClass::Busy,
            FailureClass::RateLimited,
            FailureClass::NeedsRetention,
            FailureClass::Paused,
            FailureClass::ModelGone,
            FailureClass::SetupStale,
            FailureClass::ContextTooLong,
            FailureClass::Refused,
        ] {
            let mut f = failed(Some(CloudProvider::Gemini), None, class);
            f.message = sentence.into();
            let card = super::card_for(1, &f, &Facts::default(), None, &[]).unwrap_or_else(|| {
                // Refused with nothing to offer is the plain error: give it an offer.
                super::card_for(1, &f, &everything(), None, &[]).unwrap()
            });
            assert_eq!(card.detail.as_deref(), Some(sentence), "{class:?}");
            assert_ne!(card.reason, sentence, "{class:?}");
        }
        // The reason that already is the sentence is not said twice.
        let mut f = failed(Some(CloudProvider::OpenAi), None, FailureClass::Other);
        f.message = "Something the provider said.".into();
        let card = super::card_for(1, &f, &everything(), None, &[]).unwrap();
        assert_eq!(card.reason, "Something the provider said.");
        assert_eq!(card.detail, None);
        // An unreadable-usage pause says the sentence as the reason; nothing repeats it.
        let mut f = failed(
            Some(CloudProvider::Anthropic),
            None,
            FailureClass::LimitReached,
        );
        f.message = "PM couldn't read today's usage.".into();
        let card = super::card_for(1, &f, &everything(), None, &[]).unwrap();
        assert_eq!(card.detail, None);
        // The limit's figures line carries a real limit stop: its sentence is not repeated either.
        let lim = LimitInfo {
            provider: "anthropic".into(),
            limit_usd: 5.0,
            spent_today_usd: 5.12,
            resumes_at: "2026-11-28T00:00:00+00:00".into(),
        };
        let card = super::card_for(1, &f, &everything(), Some(lim), &[]).unwrap();
        assert_eq!(card.detail, None);
        // An empty sentence adds nothing.
        let mut f = failed(Some(CloudProvider::OpenAi), None, FailureClass::Busy);
        f.message = "  ".into();
        assert_eq!(
            super::card_for(1, &f, &Facts::default(), None, &[])
                .unwrap()
                .detail,
            None
        );
    }

    /// The REAL adapters' failure sentences reach the card's detail intact (Gemini's leaked key,
    /// OpenAI's out-of-credit), not only the class line; Anthropic's text rides the same field.
    #[test]
    fn each_direct_providers_sentences_reach_the_card() {
        use crate::region::region_of_on;
        let status = |c: u16| reqwest::StatusCode::from_u16(c).unwrap();
        let from_error = |e: Error| {
            let f = e.provider_failure().expect("typed").clone();
            let failed = Failed {
                provider: Some(f.provider),
                model: f.model.clone(),
                class: f.class,
                billing: Some(f.billing),
                message: f.message.clone(),
                earlier_asked: 0,
                stopped_at_local: None,
            };
            let card = super::card_for(1, &failed, &Facts::default(), None, &[])
                .expect("a walk class has a card");
            (f.message, card)
        };

        let nyc = region_of_on(Some("America/New_York"), false);
        let (said, card) = from_error(crate::gemini_direct::request_error_in(
            status(403),
            r#"{"error":{"code":403,"message":"Your API key was reported as leaked.","status":"PERMISSION_DENIED"}}"#,
            &nyc,
        ));
        assert!(said.contains("found in public"), "{said}");
        assert_eq!(card.detail.as_deref(), Some(said.as_str()));
        assert_eq!(card.reason, "Gemini doesn't accept this key.");

        let (said, card) = from_error(crate::openai_direct::request_error(
            status(429),
            r#"{"error":{"message":"You exceeded your current quota.","type":"insufficient_quota","code":"insufficient_quota"}}"#,
        ));
        assert!(said.contains("platform.openai.com"), "{said}");
        assert_eq!(card.detail.as_deref(), Some(said.as_str()));

        // Anthropic's sentences are built from the model's catalogue row, so the shape is pinned here
        // with the typed error every adapter hands back.
        let text =
            "Fable needs 30-day retention on your workspace: turn it on in the Anthropic console.";
        let (said, card) = from_error(Error::provider(
            CloudProvider::Anthropic,
            FailureClass::NeedsRetention,
            text.into(),
        ));
        assert_eq!(card.detail.as_deref(), Some(said.as_str()));
    }
}
