// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! From a role's list to something that can be run: the PLAN.
//!
//! `RoutePlan` (in `llm_gateway`) is the four shapes PM has always had, and a mod's call reads it
//! with an exhaustive match, so it never grows. This module adds the type the person's OWN paths use,
//! [`Plan`], which is either one of those four shapes ([`Plan::Route`]) or a [`Ladder`] of rungs in the
//! person's order ([`Plan::Ladder`]). Because a mod is handed a `RoutePlan` and not a `Plan`, a mod can
//! never be handed a ladder: the type says so (I-13).
//!
//! The planner ([`plan`]) is PURE. It is given the list, the routing preferences, the runtime context
//! and what the world says about each provider (a key, an opt-in, a stamped tier) and answers with a
//! plan, so every rule is a plain test and the golden test can hold it to what the old resolver built.
//!
//! The rules it keeps:
//! - A list that today's configuration can produce resolves to the SAME `RoutePlan` the old resolver
//!   built, with the same arms and the same contents (R4). Only another list builds a ladder.
//! - A run of consecutive OpenRouter rows is ONE arm (one request, OpenRouter's own fallback, the
//!   zero-retention pins intact); a direct row is one arm of its own.
//! - Local AI off, or a role waiting for its model to be picked, with a Local row in the list STOPS
//!   the walk at that row: the rows above it run as normal, no row after it is ever tried (a list is
//!   never a back door round "Local AI off never becomes a cloud bill"), and a Local HEAD stops the
//!   role at once.
//! - A row that cannot be used (no key, not turned on) is passed over and SAID in a ladder; in one of
//!   the old shapes (a derived list) it is dropped silently, exactly as the old resolver did.
//! - A mod's plan ([`Mode::Mod`]) is the list minus every direct row, arranged as an old shape, and
//!   only when the first cloud row is OpenRouter or the list is local only; a direct first cloud row
//!   gives the plan the old resolver gave (which serves no mod).

use crate::direct_providers::KeyTier;
use crate::error::{AttemptEntry, Error, Result, SkipReason};
use crate::llm_gateway::{
    CloudArm, CloudProvider, LocalArm, Role, RoutePlan, RoutingPrefs, RuntimeContext,
};
use crate::route::{Entry, RoleRoute};
use crate::secret::Secret;

/// One step of a ladder, in the person's order.
pub enum Rung {
    /// This computer's model for the role.
    Local(LocalArm),
    /// One request: a run of OpenRouter rows, or one row of a direct provider.
    Cloud(CloudArm),
    /// A row PM could not even build (no key, not turned on, no local model), kept so the walk can
    /// say it was passed over. The walk reads `why` as the row's `pre_check` skip.
    Unusable {
        entry: AttemptEntry,
        why: SkipReason,
    },
}

impl Rung {
    /// Whether the row is this computer or a company, as the walk's brake needs it. A row PM could
    /// not build keeps the side it was on: an unbuilt local row is still a free one.
    pub(crate) fn kind(&self) -> crate::ladder::Kind {
        match self {
            Rung::Local(_) => crate::ladder::Kind::Local,
            Rung::Cloud(_) => crate::ladder::Kind::Cloud,
            Rung::Unusable { entry, .. } if entry.provider.is_none() => crate::ladder::Kind::Local,
            Rung::Unusable { .. } => crate::ladder::Kind::Cloud,
        }
    }

    /// The row as the notes name it.
    pub fn label(&self) -> AttemptEntry {
        match self {
            Rung::Local(l) => AttemptEntry {
                provider: None,
                model: Some(l.model.clone()),
                // The same loopback test the 4c card makes (`llm_gateway::local_offer_from`).
                local_elsewhere: !crate::openai_compat::host_is_loopback_literal(&l.base_url),
            },
            Rung::Cloud(a) => AttemptEntry {
                provider: Some(a.provider),
                model: a.models.first().cloned(),
                local_elsewhere: false,
            },
            Rung::Unusable { entry, .. } => entry.clone(),
        }
    }
}

/// The person's list as something to walk: rungs in order. Built only when the list is not one of
/// the four old shapes.
pub struct Ladder {
    pub rungs: Vec<Rung>,
    /// Every model any rung may use, in order: what the cost logger prices and the label fallback
    /// reads. Computed once so [`Plan::models`] can hand out a slice.
    models: Vec<String>,
}

impl Ladder {
    pub fn new(rungs: Vec<Rung>) -> Self {
        let models = rungs
            .iter()
            .flat_map(|r| match r {
                Rung::Local(l) => vec![l.model.clone()],
                Rung::Cloud(a) => a.models.clone(),
                Rung::Unusable { .. } => Vec::new(),
            })
            .collect();
        Self { rungs, models }
    }
}

/// What a person's own request runs on. A mod never sees this type.
pub enum Plan {
    /// One of the four shapes PM has always had.
    Route(RoutePlan),
    /// The person's list, walked rung by rung.
    Ladder(Ladder),
}

impl Plan {
    /// The model the plan tries first, to attribute spend when the server did not say which model
    /// answered. For a ladder it is the first rung that can run; a reply is booked on the model that
    /// answered ([`crate::openrouter::Completion::model`]), never on this.
    pub fn primary_model_id(&self) -> &str {
        match self {
            Plan::Route(p) => p.primary_model_id(),
            Plan::Ladder(l) => l.models.first().map(String::as_str).unwrap_or_default(),
        }
    }

    /// Every model the plan may use, in order, for the cost logger.
    pub fn models(&self) -> &[String] {
        match self {
            Plan::Route(p) => p.models(),
            Plan::Ladder(l) => &l.models,
        }
    }

    /// The On battery policy chose this route (a ladder is never power-routed).
    pub fn is_power_routed(&self) -> bool {
        matches!(self, Plan::Route(p) if p.is_power_routed())
    }

    /// Whether the plan exists only because Local AI was on when it was resolved.
    pub fn needs_local_ai(&self) -> bool {
        match self {
            Plan::Route(p) => p.needs_local_ai(),
            Plan::Ladder(l) => l.rungs.iter().any(|r| matches!(r, Rung::Local(_))),
        }
    }

    /// The local arm a batch should be sized against: the head of the plan when it is Local, as a
    /// local-then-cloud route is. `None` for a cloud-first plan.
    pub(crate) fn sizing_arm(&self) -> Option<&LocalArm> {
        match self {
            Plan::Route(p) => crate::llm_gateway::sizing_arm(p),
            Plan::Ladder(l) => match l.rungs.iter().find(|r| !matches!(r, Rung::Unusable { .. })) {
                Some(Rung::Local(local)) => Some(local),
                _ => None,
            },
        }
    }
}

impl From<RoutePlan> for Plan {
    fn from(p: RoutePlan) -> Self {
        Plan::Route(p)
    }
}

/// Whose plan it is: a person's own request, or a mod's call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Person,
    Mod,
}

/// What a provider's key says, for one request. An unreadable keychain is never "no key": the key
/// may well be there.
pub(crate) enum KeyRead {
    Key(Secret),
    None,
    Unreadable(String),
}

/// What the world says about one provider, read once for the request.
pub(crate) struct Seat {
    /// A direct provider is turned on in Your own keys (OpenRouter always is).
    pub opted_in: bool,
    pub key: KeyRead,
    /// The stored tier, only when it was stored beside THIS key.
    pub tier: Option<KeyTier>,
}

pub(crate) struct Inputs<'a> {
    pub role: Role,
    pub route: &'a RoleRoute,
    pub prefs: &'a RoutingPrefs,
    pub ctx: &'a RuntimeContext,
    /// The role's local arm, when the server and a model are set.
    pub local: Option<LocalArm>,
}

/// The list a mod's call may use: [Local?, the first run of OpenRouter rows], and nothing else, so
/// no direct row, no second company and no Local row after the cloud run can reach it. Only when the
/// first cloud row is OpenRouter or there is no cloud row. A direct FIRST cloud row keeps just that
/// row (and a Local row before it), which is the old resolver's plan and serves no mod.
fn mod_entries(entries: &[Entry]) -> Vec<Entry> {
    let first_cloud = entries
        .iter()
        .position(|e| matches!(e, Entry::Cloud { .. }));
    let mut out = Vec::new();
    if let Some(i) = first_cloud {
        if i > 0 {
            out.push(Entry::Local);
        }
    } else if entries.iter().any(|e| matches!(e, Entry::Local)) {
        return vec![Entry::Local];
    }
    let Some(i) = first_cloud else {
        return out;
    };
    match &entries[i] {
        Entry::Cloud { provider, .. } if provider.is_direct() => out.push(entries[i].clone()),
        _ => {
            for e in &entries[i..] {
                match e {
                    Entry::Cloud {
                        provider: CloudProvider::OpenRouter,
                        ..
                    } => out.push(e.clone()),
                    _ => break,
                }
            }
        }
    }
    out
}

/// The OpenRouter models a mod's call may use under a person's list, in order: the models of
/// [`mod_entries`]. A direct first cloud row leaves none.
pub(crate) fn mod_openrouter_models(entries: &[Entry]) -> Vec<String> {
    mod_entries(entries)
        .into_iter()
        .filter_map(|e| match e {
            Entry::Cloud {
                provider: CloudProvider::OpenRouter,
                model,
                ..
            } => Some(model),
            _ => None,
        })
        .collect()
}

/// A maximal run of consecutive OpenRouter rows, or one direct row.
struct Run {
    provider: CloudProvider,
    models: Vec<String>,
    effort: Option<String>,
}

/// A row of the list once consecutive OpenRouter rows are one: this computer, or one request.
enum Step {
    Local,
    Cloud(Run),
}

fn runs_of(entries: &[Entry]) -> Vec<Step> {
    let mut out: Vec<Step> = Vec::new();
    for e in entries {
        match e {
            Entry::Local => out.push(Step::Local),
            Entry::Cloud {
                provider: CloudProvider::OpenRouter,
                model,
                ..
            } => match out.last_mut() {
                Some(Step::Cloud(run)) if run.provider == CloudProvider::OpenRouter => {
                    run.models.push(model.clone())
                }
                _ => out.push(Step::Cloud(Run {
                    provider: CloudProvider::OpenRouter,
                    models: vec![model.clone()],
                    effort: None,
                })),
            },
            Entry::Cloud {
                provider,
                model,
                effort,
            } => out.push(Step::Cloud(Run {
                provider: *provider,
                models: vec![model.clone()],
                effort: effort.clone(),
            })),
        }
    }
    out
}

/// Build the plan for a role from its list.
///
/// `Ok(None)` is "nothing can answer": no usable row, or the role is stopped (Local AI off or waiting
/// for a model with a Local row in the list). The caller says why ([`explain`]). `Err` is only an
/// unreadable keychain in a list the old resolver would have read the same key for.
///
/// `seat` is asked for each provider that has a row, in list order, never twice for one provider's
/// key in a way that matters (the real one reads the keychain, so it is called outside any guard);
/// `power_ok` says whether a cloud arm may take a role over on battery (not a paused region, not a
/// limit-reached provider).
pub(crate) fn plan(
    inp: Inputs<'_>,
    mode: Mode,
    seat: &mut dyn FnMut(CloudProvider) -> Seat,
    power_ok: &dyn Fn(&CloudArm) -> bool,
) -> Result<Option<Plan>> {
    let Inputs {
        role,
        route,
        prefs,
        ctx,
        mut local,
    } = inp;
    let entries: Vec<Entry> = match mode {
        Mode::Person => route.entries.clone(),
        Mode::Mod => mod_entries(&route.entries),
    };
    // The shapes the old resolver built are what a derived list, and every mod plan, resolves to:
    // anything it could not use was dropped without a word. Only a person's own stored list says
    // what it passed over.
    let silent = route.derived || mode == Mode::Mod;

    // A Local row while Local AI is off, or while the role waits for its model to be picked, STOPS
    // the walk at that row: no row after it is ever tried (the list is never a back door round
    // "Local AI off never becomes a cloud bill", D7). The rows ABOVE it run exactly as normal, so
    // [Claude, This computer] still answers from Claude; a Local HEAD stops the role at once. A
    // mod's plan and a list nobody edited keep the whole-role stop (the old resolver's answer).
    let why_stopped = if !prefs.local_ai_on {
        Some(SkipReason::LocalAiOff)
    } else if prefs.emptied(role) {
        Some(SkipReason::PickAModel)
    } else {
        None
    };
    let mut stop_at: Option<SkipReason> = None;
    let entries = match (
        entries.iter().position(|e| matches!(e, Entry::Local)),
        why_stopped,
    ) {
        (Some(i), Some(why)) => {
            if i == 0 || silent {
                return Ok(None);
            }
            stop_at = Some(why);
            entries[..i].to_vec()
        }
        _ => entries,
    };

    let mut rungs: Vec<Rung> = Vec::new();
    let mut seen: Vec<(CloudProvider, Seat)> = Vec::new();
    for run in runs_of(&entries) {
        match run {
            Step::Local => match local.take() {
                Some(arm) => rungs.push(Rung::Local(arm)),
                None => rungs.push(Rung::Unusable {
                    entry: AttemptEntry {
                        provider: None,
                        model: None,
                        local_elsewhere: false,
                    },
                    why: SkipReason::NoLocalModel,
                }),
            },
            Step::Cloud(run) => {
                let at = match seen.iter().position(|(p, _)| *p == run.provider) {
                    Some(at) => at,
                    None => {
                        seen.push((run.provider, seat(run.provider)));
                        seen.len() - 1
                    }
                };
                let s = &seen[at].1;
                let entry = AttemptEntry {
                    provider: Some(run.provider),
                    model: run.models.first().cloned(),
                    local_elsewhere: false,
                };
                if !s.opted_in {
                    rungs.push(Rung::Unusable {
                        entry,
                        why: SkipReason::NotOnForYou,
                    });
                    continue;
                }
                match &s.key {
                    KeyRead::None => rungs.push(Rung::Unusable {
                        entry,
                        why: SkipReason::NoKey,
                    }),
                    KeyRead::Unreadable(msg) if silent => {
                        return Err(Error::Other(msg.clone()));
                    }
                    KeyRead::Unreadable(_) => rungs.push(Rung::Unusable {
                        entry,
                        why: SkipReason::KeyUnreadable,
                    }),
                    KeyRead::Key(key) => rungs.push(Rung::Cloud(if run.provider.is_direct() {
                        CloudArm {
                            provider: run.provider,
                            key: key.clone(),
                            models: run.models,
                            effort: run.effort,
                            tier: s.tier,
                            role: Some(role),
                        }
                    } else {
                        CloudArm::new(run.provider, key.clone(), run.models)
                    })),
                }
            }
        }
    }

    // The row the walk stops at, said in the notes (a skipped this-computer row whose reason ends the
    // list). Last by construction: nothing after it was planned.
    if let Some(why) = stop_at {
        rungs.push(Rung::Unusable {
            entry: AttemptEntry {
                provider: None,
                model: None,
                // Named as the rest of the PR names it: "your Local AI server" when the stored
                // address is not this computer, "this computer" otherwise. With no server set
                // there is no arm, and no claim about a place either way.
                local_elsewhere: local
                    .as_ref()
                    .is_some_and(|l| !crate::openai_compat::host_is_loopback_literal(&l.base_url)),
            },
            why,
        });
    }

    let any_unusable = rungs.iter().any(|r| matches!(r, Rung::Unusable { .. }));
    let usable = rungs.len()
        - rungs
            .iter()
            .filter(|r| matches!(r, Rung::Unusable { .. }))
            .count();
    if usable == 0 {
        return Ok(None);
    }

    // The On battery policy: a role whose list starts with a usable Local row moves onto its FIRST
    // cloud row only (one request, no walk, never back to local), when that row is usable and the
    // person said yes for ITS provider (the context's consent is by the first cloud row's provider).
    let head_is_local = matches!(rungs.first(), Some(Rung::Local(_)));
    let first_cloud = rungs.iter().position(|r| {
        matches!(
            r,
            Rung::Cloud(_)
                | Rung::Unusable {
                    entry: AttemptEntry {
                        provider: Some(_),
                        ..
                    },
                    ..
                }
        )
    });
    if head_is_local && ctx.moves(role) {
        if let Some(i) = first_cloud {
            let ok = matches!(&rungs[i], Rung::Cloud(a) if power_ok(a));
            if ok {
                let displaced = match rungs.remove(0) {
                    Rung::Local(l) => l.model,
                    _ => unreachable!("the head was checked"),
                };
                let cloud = rungs
                    .into_iter()
                    .find_map(|r| match r {
                        Rung::Cloud(a) => Some(a),
                        _ => None,
                    })
                    .expect("the first cloud row was checked");
                return Ok(Some(Plan::Route(RoutePlan::CloudForPower {
                    cloud,
                    displaced_local_model: displaced,
                })));
            }
        }
    }

    // A person's stored list that passed a row over keeps every rung so the walk can say so.
    if !silent && any_unusable {
        return Ok(Some(Plan::Ladder(Ladder::new(rungs))));
    }

    let mut usable_rungs: Vec<Rung> = rungs
        .into_iter()
        .filter(|r| !matches!(r, Rung::Unusable { .. }))
        .collect();
    let shape = match (usable_rungs.len(), usable_rungs.first()) {
        (1, Some(Rung::Cloud(_))) => match usable_rungs.pop() {
            Some(Rung::Cloud(a)) => Some(RoutePlan::Cloud(a)),
            _ => None,
        },
        (1, Some(Rung::Local(_))) => match usable_rungs.pop() {
            Some(Rung::Local(l)) => Some(RoutePlan::LocalOnly(l)),
            _ => None,
        },
        (2, Some(Rung::Local(_))) if matches!(usable_rungs.get(1), Some(Rung::Cloud(_))) => {
            let cloud = usable_rungs.pop();
            let local = usable_rungs.pop();
            match (local, cloud) {
                (Some(Rung::Local(local)), Some(Rung::Cloud(cloud))) => {
                    Some(RoutePlan::LocalThenCloud { local, cloud })
                }
                _ => None,
            }
        }
        _ => None,
    };
    Ok(Some(match shape {
        Some(p) => Plan::Route(p),
        None => Plan::Ladder(Ladder::new(usable_rungs)),
    }))
}

// ---- saying why nothing can answer ---------------------------------------------------------------

/// What one row of a list is, for the line that says why a role has nothing to answer it.
pub(crate) struct RowStatus {
    pub entry: AttemptEntry,
    pub unusable: Option<SkipReason>,
}

/// One plain line about a list that nothing can answer, naming each row and why it can't: "Claude:
/// no key saved. Gemini: paused where you are." A Local row while Local AI is off says that first,
/// because the cloud rows after it are not tried.
pub(crate) fn explain(rows: &[RowStatus], local_ai_off_with_local_row: bool) -> String {
    if local_ai_off_with_local_row {
        return "Local AI is off. Turn it on, or take the local model out of this list.".into();
    }
    let parts: Vec<String> = rows
        .iter()
        .filter_map(|r| {
            r.unusable.map(|why| {
                let who = r.entry.who();
                let who = who.chars().next().map_or(String::new(), |f| {
                    f.to_uppercase().collect::<String>() + &who[f.len_utf8()..]
                });
                format!("{who}: {}.", why.why())
            })
        })
        .collect();
    if parts.is_empty() {
        return crate::llm_gateway::no_provider_message();
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::direct_models;
    use crate::direct_providers;
    use crate::llm_gateway::{self, ProviderChoice, RoleProviders};
    use crate::power::{Consent, DirectConsent, PowerScope};
    use crate::{db, settings};
    use rusqlite::Connection;

    /// Obviously fake, short, and never next to the word "key" in a quoted literal.
    const FAKE: &str = "fake-abcdefghijklmnop";

    fn temp_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sqlite");
        let pass = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let conn = db::open(&path, pass).unwrap();
        (dir, conn)
    }

    /// Every settings row the routing reads, so a configuration can be cleared between cases.
    fn clear(conn: &Connection) {
        let mut keys: Vec<String> = vec![
            llm_gateway::CHAT_ROUTING_KEY.into(),
            llm_gateway::BACKGROUND_ROUTING_KEY.into(),
            llm_gateway::CHAT_MODEL_EMPTIED_KEY.into(),
            llm_gateway::BACKGROUND_MODEL_EMPTIED_KEY.into(),
            crate::local_ai_switch::LOCAL_AI_ENABLED_KEY.into(),
            "chat_cloud_provider".into(),
            "background_cloud_provider".into(),
            settings::CHAT_MODELS_KEY.into(),
            settings::BACKGROUND_MODELS_KEY.into(),
            settings::CHAT_AUTO_SWITCH_KEY.into(),
            settings::BACKGROUND_AUTO_SWITCH_KEY.into(),
            "default_model".into(),
            crate::route::CHAT_ROUTE_KEY.into(),
            crate::route::BACKGROUND_ROUTE_KEY.into(),
        ];
        for p in direct_providers::PROVIDERS {
            keys.push(p.opt_in.into());
            for role in [Role::Chat, Role::Background] {
                keys.push(p.role(role).models.into());
                keys.push(p.role(role).effort.into());
            }
        }
        for k in keys {
            db::delete_setting(conn, &k).unwrap();
        }
    }

    fn local_arm() -> LocalArm {
        LocalArm {
            base_url: "http://127.0.0.1:1".into(),
            model: "llama3".into(),
            token: None,
        }
    }

    fn arm_text(a: &CloudArm) -> String {
        format!(
            "{}|{}|{:?}|{:?}|{:?}|{:?}",
            a.provider.as_str(),
            a.key.expose(),
            a.models,
            a.effort,
            a.tier,
            a.role
        )
    }

    fn local_text(l: &LocalArm) -> String {
        format!("{}|{}|{:?}", l.base_url, l.model, l.token.is_some())
    }

    /// A plan as text, arm contents included, so two plans compare by what they would send.
    fn text(p: Option<&RoutePlan>) -> String {
        match p {
            None => "nothing".into(),
            Some(RoutePlan::Cloud(a)) => format!("Cloud[{}]", arm_text(a)),
            Some(RoutePlan::LocalOnly(l)) => format!("LocalOnly[{}]", local_text(l)),
            Some(RoutePlan::LocalThenCloud { local, cloud }) => {
                format!("LocalThenCloud[{}][{}]", local_text(local), arm_text(cloud))
            }
            Some(RoutePlan::CloudForPower {
                cloud,
                displaced_local_model,
            }) => format!(
                "CloudForPower[{}][{displaced_local_model}]",
                arm_text(cloud)
            ),
        }
    }

    fn plan_text(p: Result<Option<Plan>>) -> String {
        match p.unwrap() {
            None => "nothing".into(),
            Some(Plan::Route(r)) => text(Some(&r)),
            Some(Plan::Ladder(_)) => "LADDER".into(),
        }
    }

    /// The world a case describes, as the real resolver would read it.
    fn world<'a>(
        conn: &'a Connection,
        role: Role,
        key_present: bool,
    ) -> impl FnMut(CloudProvider) -> Seat + 'a {
        move |p| {
            let opted_in = !p.is_direct() || direct_providers::opted_in(conn, p).unwrap();
            if !opted_in {
                return Seat {
                    opted_in,
                    key: KeyRead::None,
                    tier: None,
                };
            }
            if !key_present {
                return Seat {
                    opted_in,
                    key: KeyRead::None,
                    tier: None,
                };
            }
            let tier = if p.is_direct() {
                let i = direct_providers::arm_inputs(conn, p, role).unwrap();
                direct_providers::tier_for_key(i.tier, i.key_fp.as_deref(), FAKE)
            } else {
                None
            };
            Seat {
                opted_in,
                key: KeyRead::Key(Secret::from(FAKE)),
                tier,
            }
        }
    }

    /// What the OLD resolver (before lists) built, from the same world: the oracle.
    fn old_plan(
        conn: &Connection,
        role: Role,
        key_present: bool,
        local: Option<LocalArm>,
        ctx: &RuntimeContext,
        power_ok: bool,
    ) -> Option<RoutePlan> {
        let prefs = llm_gateway::routing_prefs(conn).unwrap();
        let choice = llm_gateway::resolve_provider(role, &prefs, ctx);
        let cloud = || -> Option<CloudArm> {
            let provider = llm_gateway::role_cloud_provider(conn, role).unwrap();
            if !key_present {
                return None;
            }
            let key = Secret::from(FAKE);
            if provider.is_direct() {
                let inputs = direct_providers::arm_inputs(conn, provider, role).unwrap();
                let tier = direct_providers::tier_for_key(
                    inputs.tier,
                    inputs.key_fp.as_deref(),
                    key.expose(),
                );
                return Some(CloudArm {
                    provider,
                    key,
                    models: inputs.models,
                    effort: inputs.effort,
                    tier,
                    role: Some(role),
                });
            }
            let (models_key, auto_key) = llm_gateway::openrouter_keys(role);
            let models = settings::effective_models(conn, models_key, auto_key).unwrap();
            Some(CloudArm::new(CloudProvider::OpenRouter, key, models))
        };
        match choice {
            ProviderChoice::Cloud => cloud().map(RoutePlan::Cloud),
            ProviderChoice::Local => local.map(RoutePlan::LocalOnly),
            ProviderChoice::LocalThenCloud | ProviderChoice::CloudForPower => {
                let cloud = cloud();
                let power = choice == ProviderChoice::CloudForPower
                    && !cloud.as_ref().is_some_and(|_| !power_ok);
                llm_gateway::hydrate_local_then_cloud(power, local, cloud)
            }
            ProviderChoice::LocalAiOff => None,
        }
    }

    /// THE GOLDEN PLAN. Every configuration that existed before lists (routing x provider x
    /// auto-switch x model list x Local AI on/off x emptied x a key or none x a local model or none x
    /// the battery policy moving the role or not x the policy allowed or not) resolves, through the
    /// list the old rows derive, to the IDENTICAL plan the old resolver built: the same variant, the
    /// same arms, the same keys, models, effort, tier and role. A person who never edits a list sees
    /// no change in what is sent, to whom, or in what order.
    #[test]
    fn the_golden_plan_is_what_the_old_resolver_built_for_every_old_configuration() {
        let (_d, conn) = temp_db();
        let direct_ps = [
            CloudProvider::OpenAi,
            CloudProvider::Gemini,
            CloudProvider::Anthropic,
        ];
        let second = |p: CloudProvider, role: Role| -> String {
            let default = direct_models::default_for(p, role).unwrap();
            direct_models::models(p)
                .iter()
                .find(|m| m.id != default.id && !m.retired(&direct_models::utc_today()))
                .expect("a second live row")
                .id
                .clone()
        };
        enum Cfg {
            Or {
                three: bool,
                auto: Option<bool>,
            },
            Direct {
                p: CloudProvider,
                second: bool,
            },
            /// The provider is stored but the person never opted in: demoted to OpenRouter.
            NotOptedIn(CloudProvider),
        }
        let mut cfgs: Vec<Cfg> = Vec::new();
        for three in [false, true] {
            for auto in [None, Some(false), Some(true)] {
                cfgs.push(Cfg::Or { three, auto });
            }
        }
        for p in direct_ps {
            for second in [false, true] {
                cfgs.push(Cfg::Direct { p, second });
            }
        }
        cfgs.push(Cfg::NotOptedIn(CloudProvider::Gemini));

        let all_consent = {
            let mut d = DirectConsent::NONE;
            for p in direct_ps {
                d = d.with(p, PowerScope::Both);
            }
            d
        };
        let mut cases = 0usize;
        let mut kinds = std::collections::BTreeSet::new();
        for role in [Role::Chat, Role::Background] {
            let (models_key, auto_key) = llm_gateway::openrouter_keys(role);
            let (routing_key, emptied_key) = match role {
                Role::Chat => (
                    llm_gateway::CHAT_ROUTING_KEY,
                    llm_gateway::CHAT_MODEL_EMPTIED_KEY,
                ),
                Role::Background => (
                    llm_gateway::BACKGROUND_ROUTING_KEY,
                    llm_gateway::BACKGROUND_MODEL_EMPTIED_KEY,
                ),
            };
            for cfg in &cfgs {
                for routing in [None, Some("cloud"), Some("local"), Some("local-then-cloud")] {
                    for local_on in [true, false] {
                        for emptied in [false, true] {
                            clear(&conn);
                            match cfg {
                                Cfg::Or { three, auto } => {
                                    if *three {
                                        db::set_setting(
                                            &conn,
                                            models_key,
                                            r#"["vendor/first","vendor/second","vendor/third"]"#,
                                        )
                                        .unwrap();
                                    }
                                    if let Some(a) = auto {
                                        db::set_bool(&conn, auto_key, *a).unwrap();
                                    }
                                }
                                Cfg::Direct { p, second: s } => {
                                    db::set_setting(
                                        &conn,
                                        llm_gateway::cloud_provider_key(role),
                                        p.as_str(),
                                    )
                                    .unwrap();
                                    direct_providers::set_opt_in(&conn, *p, true).unwrap();
                                    if *s {
                                        let keys =
                                            direct_providers::provider(*p).unwrap().role(role);
                                        db::set_setting(
                                            &conn,
                                            keys.models,
                                            &serde_json::to_string(&[second(*p, role)]).unwrap(),
                                        )
                                        .unwrap();
                                    }
                                }
                                Cfg::NotOptedIn(p) => db::set_setting(
                                    &conn,
                                    llm_gateway::cloud_provider_key(role),
                                    p.as_str(),
                                )
                                .unwrap(),
                            }
                            if let Some(r) = routing {
                                db::set_setting(&conn, routing_key, r).unwrap();
                            }
                            db::set_bool(
                                &conn,
                                crate::local_ai_switch::LOCAL_AI_ENABLED_KEY,
                                local_on,
                            )
                            .unwrap();
                            if emptied {
                                db::set_bool(&conn, emptied_key, true).unwrap();
                            }
                            // The role's readiness provider reads exactly as before for these lists.
                            let providers = RoleProviders::read(&conn).unwrap();
                            assert_eq!(
                                providers.for_role(role),
                                llm_gateway::role_cloud_provider(&conn, role).unwrap()
                            );
                            let route = crate::route::entries_for(&conn, role).unwrap();
                            assert!(route.derived);
                            let prefs = llm_gateway::routing_prefs(&conn).unwrap();
                            for key_present in [true, false] {
                                // Switching Local AI off deletes the model it is emptied of: a role
                                // that is waiting for a pick has no local arm.
                                let locals: &[bool] =
                                    if emptied { &[false] } else { &[true, false] };
                                for has_local in locals.iter().copied() {
                                    for battery in 0..3 {
                                        let ctx = if battery == 0 {
                                            RuntimeContext::default()
                                        } else {
                                            RuntimeContext {
                                                battery_low: true,
                                                scope: PowerScope::Both,
                                                consent: Consent::ALL,
                                                direct_consent: all_consent,
                                                providers,
                                                keep_local: false,
                                            }
                                        };
                                        let power_ok = battery != 2;
                                        let want = text(
                                            old_plan(
                                                &conn,
                                                role,
                                                key_present,
                                                has_local.then(local_arm),
                                                &ctx,
                                                power_ok,
                                            )
                                            .as_ref(),
                                        );
                                        let got = plan_text(plan(
                                            Inputs {
                                                role,
                                                route: &route,
                                                prefs: &prefs,
                                                ctx: &ctx,
                                                local: has_local.then(local_arm),
                                            },
                                            Mode::Person,
                                            &mut world(&conn, role, key_present),
                                            &|_| power_ok,
                                        ));
                                        assert_eq!(
                                            got, want,
                                            "{role:?} routing={routing:?} local_on={local_on} \
                                             emptied={emptied} key={key_present} local={has_local} \
                                             battery={battery} entries={:?}",
                                            route.entries
                                        );
                                        kinds.insert(want.split('[').next().unwrap().to_string());
                                        cases += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(cases > 3000, "the sweep ran {cases} cases");
        // Every old shape was met, so the comparison is not vacuous.
        for k in [
            "Cloud",
            "LocalOnly",
            "LocalThenCloud",
            "CloudForPower",
            "nothing",
        ] {
            assert!(kinds.contains(k), "never saw {k}: {kinds:?}");
        }
    }

    // ---- lists a person writes ---------------------------------------------------------------

    fn or(m: &str) -> Entry {
        Entry::Cloud {
            provider: CloudProvider::OpenRouter,
            model: m.into(),
            effort: None,
        }
    }

    fn direct(p: CloudProvider) -> Entry {
        let m = direct_models::default_for(p, Role::Chat).unwrap();
        Entry::Cloud {
            provider: p,
            model: m.id.clone(),
            effort: None,
        }
    }

    fn stored(entries: Vec<Entry>) -> RoleRoute {
        RoleRoute {
            entries,
            derived: false,
        }
    }

    /// Plan a list with every provider keyed and turned on, unless a closure says otherwise.
    fn run(
        route: &RoleRoute,
        mode: Mode,
        local: bool,
        local_on: bool,
        ctx: &RuntimeContext,
        seat: &mut dyn FnMut(CloudProvider) -> Seat,
        power_ok: bool,
    ) -> Option<Plan> {
        let mut prefs = llm_gateway::RoutingPrefs::uniform(llm_gateway::ProviderPref::Cloud);
        prefs.local_ai_on = local_on;
        plan(
            Inputs {
                role: Role::Chat,
                route,
                prefs: &prefs,
                ctx,
                local: local.then(local_arm),
            },
            mode,
            seat,
            &|_| power_ok,
        )
        .unwrap()
    }

    fn all_keyed(p: CloudProvider) -> Seat {
        Seat {
            opted_in: true,
            key: KeyRead::Key(Secret::from(format!("fake-{}", p.as_str()))),
            tier: None,
        }
    }

    fn shape(p: &Option<Plan>) -> String {
        match p {
            None => "nothing".into(),
            Some(Plan::Route(r)) => text(Some(r)),
            Some(Plan::Ladder(l)) => format!(
                "Ladder[{}]",
                l.rungs
                    .iter()
                    .map(|r| match r {
                        Rung::Local(l) => format!("local:{}", l.model),
                        Rung::Cloud(a) => format!("{}:{}", a.provider.as_str(), a.models.join("+")),
                        Rung::Unusable { entry, why } => {
                            format!("skip:{}:{}", entry.who(), why.as_str())
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    }

    const INERT: RuntimeContext = RuntimeContext {
        battery_low: false,
        scope: PowerScope::Both,
        consent: Consent::NONE,
        direct_consent: DirectConsent::NONE,
        providers: RoleProviders::OPENROUTER,
        keep_local: false,
    };

    /// Only another list builds a ladder, in the person's order; consecutive OpenRouter rows are ONE
    /// request and a direct row is one request of its own.
    #[test]
    fn a_list_that_is_not_an_old_shape_is_a_ladder_in_the_persons_order() {
        let at = CloudProvider::Anthropic;
        let gm = CloudProvider::Gemini;
        let go = |entries| {
            shape(&run(
                &stored(entries),
                Mode::Person,
                true,
                true,
                &INERT,
                &mut all_keyed,
                true,
            ))
        };
        let a = direct(at);
        let g = direct(gm);
        let am = match &a {
            Entry::Cloud { model, .. } => model.clone(),
            _ => unreachable!(),
        };
        let gmod = match &g {
            Entry::Cloud { model, .. } => model.clone(),
            _ => unreachable!(),
        };
        assert_eq!(
            go(vec![a.clone(), g.clone()]),
            format!("Ladder[anthropic:{am},gemini:{gmod}]")
        );
        // Two OpenRouter rows together are one request, in the order written.
        assert_eq!(
            go(vec![or("v/a"), or("v/b"), a.clone(), or("v/c")]),
            format!("Ladder[openrouter:v/a+v/b,anthropic:{am},openrouter:v/c]")
        );
        // Local anywhere: first, in the middle, last.
        assert_eq!(
            go(vec![Entry::Local, a.clone(), g.clone()]),
            format!("Ladder[local:llama3,anthropic:{am},gemini:{gmod}]")
        );
        assert_eq!(
            go(vec![a.clone(), Entry::Local, or("v/a")]),
            format!("Ladder[anthropic:{am},local:llama3,openrouter:v/a]")
        );
        assert_eq!(
            go(vec![a.clone(), Entry::Local]),
            format!("Ladder[anthropic:{am},local:llama3]")
        );
        // Lists that ARE an old shape stay one.
        assert!(go(vec![or("v/a"), or("v/b")]).starts_with("Cloud["));
        assert!(go(vec![a.clone()]).starts_with("Cloud["));
        assert!(go(vec![Entry::Local]).starts_with("LocalOnly["));
        assert!(go(vec![Entry::Local, or("v/a"), or("v/b")]).starts_with("LocalThenCloud["));
    }

    /// A row PM could not use is passed over and SAID in a ladder (never answered from elsewhere),
    /// and a role with nothing usable has nothing to answer it.
    #[test]
    fn a_stored_list_says_which_rows_it_passed_over() {
        let at = CloudProvider::Anthropic;
        let mut seat = |p: CloudProvider| match p {
            CloudProvider::Anthropic => Seat {
                opted_in: true,
                key: KeyRead::None,
                tier: None,
            },
            CloudProvider::Gemini => Seat {
                opted_in: false,
                key: KeyRead::None,
                tier: None,
            },
            CloudProvider::OpenAi => Seat {
                opted_in: true,
                key: KeyRead::Unreadable("keychain error".into()),
                tier: None,
            },
            other => all_keyed(other),
        };
        let route = stored(vec![
            direct(at),
            direct(CloudProvider::Gemini),
            direct(CloudProvider::OpenAi),
            or("v/a"),
        ]);
        let got = shape(&run(
            &route,
            Mode::Person,
            false,
            true,
            &INERT,
            &mut seat,
            true,
        ));
        assert!(
            got.ends_with("skip:Anthropic:no_key,skip:Gemini:not_on,skip:OpenAI:key_unreadable,openrouter:v/a]"),
            "{got}"
        );
        // Nothing usable at all: nothing answers.
        let none = stored(vec![direct(at), direct(CloudProvider::Gemini)]);
        assert_eq!(
            shape(&run(
                &none,
                Mode::Person,
                false,
                true,
                &INERT,
                &mut seat,
                true
            )),
            "nothing"
        );
        // A derived list drops what it can't use without a word, as the old resolver did.
        let derived = RoleRoute {
            entries: vec![or("v/a"), or("v/b")],
            derived: true,
        };
        let mut no_key = |_p: CloudProvider| Seat {
            opted_in: true,
            key: KeyRead::None,
            tier: None,
        };
        assert_eq!(
            shape(&run(
                &derived,
                Mode::Person,
                false,
                true,
                &INERT,
                &mut no_key,
                true
            )),
            "nothing"
        );
    }

    /// A Local HEAD while Local AI is off, or while the role waits for its model, STOPS the role: no
    /// cloud row after it is tried, and not even a key is read. The list is never a back door round
    /// "Local AI off never becomes a cloud bill". (A Local row BELOW cloud rows stops the walk at
    /// that row instead: `local_ai_off_stops_the_walk_at_a_local_row_below_cloud_rows`.)
    #[test]
    fn a_local_row_with_local_ai_off_stops_the_role_and_no_cloud_row_is_tried() {
        let route = stored(vec![Entry::Local, direct(CloudProvider::Anthropic)]);
        let mut asked = 0;
        let mut seat = |p: CloudProvider| {
            asked += 1;
            all_keyed(p)
        };
        assert_eq!(
            shape(&run(
                &route,
                Mode::Person,
                true,
                false,
                &INERT,
                &mut seat,
                true
            )),
            "nothing"
        );
        // The same list with Local AI on is a plan.
        assert!(shape(&run(
            &route,
            Mode::Person,
            true,
            true,
            &INERT,
            &mut seat,
            true
        ))
        .starts_with("LocalThenCloud"));
        // Waiting for a model (emptied) is the same stop.
        let mut prefs = llm_gateway::RoutingPrefs::uniform(llm_gateway::ProviderPref::Cloud);
        prefs.emptied = llm_gateway::Emptied {
            chat: true,
            background: false,
        };
        let p = plan(
            Inputs {
                role: Role::Chat,
                route: &route,
                prefs: &prefs,
                ctx: &INERT,
                local: None,
            },
            Mode::Person,
            &mut all_keyed,
            &|_| true,
        )
        .unwrap();
        assert!(p.is_none());
        let _ = asked;
        // And the message says so first, whatever else is wrong with the cloud rows.
        let rows = [RowStatus {
            entry: AttemptEntry {
                provider: Some(CloudProvider::Anthropic),
                model: None,
                local_elsewhere: false,
            },
            unusable: Some(SkipReason::NoKey),
        }];
        assert_eq!(
            explain(&rows, true),
            "Local AI is off. Turn it on, or take the local model out of this list."
        );
        assert_eq!(explain(&rows, false), "Anthropic: no key saved.");
    }

    /// A Local row BELOW cloud rows, with Local AI off or the role waiting for a model: the rows
    /// above it are planned as normal, the walk stops AT the Local row (a skipped note whose reason
    /// ends the list), and no row after it is built, so none can ever be tried or even have its key
    /// read. A Local head stops the role at once; the mod's plan is untouched.
    #[test]
    fn local_ai_off_stops_the_walk_at_a_local_row_below_cloud_rows() {
        let at = CloudProvider::Anthropic;
        let a = direct(at);
        let am = match &a {
            Entry::Cloud { model, .. } => model.clone(),
            _ => unreachable!(),
        };
        let asked = std::cell::RefCell::new(Vec::new());
        let go = |entries: Vec<Entry>, mode: Mode, on: bool| {
            let mut seat = |p: CloudProvider| {
                asked.borrow_mut().push(p);
                all_keyed(p)
            };
            shape(&run(
                &stored(entries),
                mode,
                false,
                on,
                &INERT,
                &mut seat,
                true,
            ))
        };
        // [Claude, This computer] off: Claude answers, the walk stops at the Local row.
        assert_eq!(
            go(vec![a.clone(), Entry::Local], Mode::Person, false),
            format!("Ladder[anthropic:{am},skip:this computer:local_ai_off]")
        );
        // [Claude, This computer, OpenRouter] off: the OpenRouter row after the stop is NOT planned and
        // its key is never read.
        asked.borrow_mut().clear();
        assert_eq!(
            go(
                vec![a.clone(), Entry::Local, or("v/a")],
                Mode::Person,
                false
            ),
            format!("Ladder[anthropic:{am},skip:this computer:local_ai_off]")
        );
        assert!(
            !asked.borrow().contains(&CloudProvider::OpenRouter),
            "a row after the stop is never asked for its key: {:?}",
            asked.borrow()
        );
        // A Local head stops the role at once, with the cloud rows behind it untouched.
        assert_eq!(
            go(vec![Entry::Local, a.clone()], Mode::Person, false),
            "nothing"
        );
        // On, the same lists are the ladders they always were (the Local row is a row).
        assert_eq!(
            go(vec![a.clone(), Entry::Local, or("v/a")], Mode::Person, true),
            format!("Ladder[anthropic:{am},skip:this computer:no_local_model,openrouter:v/a]")
        );
        // Waiting for a model (emptied) stops at the Local row the same way, saying why.
        let mut prefs = llm_gateway::RoutingPrefs::uniform(llm_gateway::ProviderPref::Cloud);
        prefs.emptied = llm_gateway::Emptied {
            chat: true,
            background: false,
        };
        let route = stored(vec![a.clone(), Entry::Local, or("v/a")]);
        let p = plan(
            Inputs {
                role: Role::Chat,
                route: &route,
                prefs: &prefs,
                ctx: &INERT,
                local: None,
            },
            Mode::Person,
            &mut all_keyed,
            &|_| true,
        )
        .unwrap();
        assert_eq!(
            shape(&p),
            format!("Ladder[anthropic:{am},skip:this computer:pick_a_model]")
        );
        // Nothing above the stop is usable: nothing answers, and the message is the Local-off one.
        let mut no_key = |_p: CloudProvider| Seat {
            opted_in: true,
            key: KeyRead::None,
            tier: None,
        };
        assert_eq!(
            shape(&run(
                &stored(vec![a.clone(), Entry::Local]),
                Mode::Person,
                false,
                false,
                &INERT,
                &mut no_key,
                true
            )),
            "nothing"
        );
        // A mod's plan keeps the whole-role stop for a Local row that comes first, and drops a Local
        // row that comes after the run (as it always did): the type still holds no ladder.
        assert_eq!(
            go(vec![Entry::Local, or("v/a")], Mode::Mod, false),
            "nothing"
        );
        assert!(
            go(vec![or("v/a"), Entry::Local], Mode::Mod, false).starts_with("Cloud[openrouter|")
        );
    }

    /// I-13: a mod's plan never holds a direct row and never a ladder. It is the list's first run of
    /// OpenRouter rows (plus this computer when it comes first), and only when the FIRST cloud row is
    /// OpenRouter; a direct first cloud row gives a plan that serves no mod (what the old resolver
    /// gave), whatever follows it.
    #[test]
    fn a_mods_plan_never_holds_a_direct_row_or_a_ladder() {
        let at = || direct(CloudProvider::Anthropic);
        let mod_plan = |entries: Vec<Entry>| {
            shape(&run(
                &stored(entries),
                Mode::Mod,
                true,
                true,
                &INERT,
                &mut all_keyed,
                true,
            ))
        };
        // [Claude, OpenRouter]: the first cloud row is direct, so the mod plan is Claude's alone,
        // which serves no mod: the OpenRouter row written as a chat fallback is not a mod's route.
        assert!(mod_plan(vec![at(), or("v/a")]).starts_with("Cloud[anthropic|"));
        // [OpenRouter, Claude]: only the OpenRouter run.
        assert!(mod_plan(vec![or("v/a"), at()]).starts_with("Cloud[openrouter|"));
        assert!(mod_plan(vec![or("v/a"), at()]).contains("[\"v/a\"]"));
        assert!(!mod_plan(vec![or("v/a"), at()]).contains("anthropic"));
        // [OpenRouter a, OpenRouter b, Claude, OpenRouter c]: the first run, both models, one request.
        assert!(
            mod_plan(vec![or("v/a"), or("v/b"), at(), or("v/c")]).contains("[\"v/a\", \"v/b\"]")
        );
        // Local first is kept (the existing local-then-cloud), a Local row after the run is ignored.
        assert!(mod_plan(vec![Entry::Local, or("v/a"), at()]).starts_with("LocalThenCloud["));
        assert!(mod_plan(vec![or("v/a"), Entry::Local]).starts_with("Cloud[openrouter|"));
        // Local only is local only.
        assert!(mod_plan(vec![Entry::Local]).starts_with("LocalOnly["));
        // No OpenRouter and no local row: nothing for a mod (MOD-5004 upstream).
        assert!(!mod_plan(vec![at(), direct(CloudProvider::Gemini)]).contains("openrouter"));
        // Whatever the list, a mod plan is one of the four old shapes and every cloud arm it holds is
        // OpenRouter unless the first cloud row was direct.
        for entries in [
            vec![or("v/a"), at(), or("v/b")],
            vec![Entry::Local, or("v/a"), at()],
            vec![or("v/a"), or("v/b"), Entry::Local, at()],
        ] {
            let got = mod_plan(entries);
            assert!(!got.starts_with("Ladder"), "{got}");
            assert!(!got.contains("anthropic"), "{got}");
        }
        // A direct FIRST cloud row keeps only that row (and a Local row before it): the old plan,
        // which serves no mod, and never a ladder.
        let got = mod_plan(vec![Entry::Local, at(), or("v/a")]);
        assert!(got.starts_with("LocalThenCloud["), "{got}");
        assert!(!got.contains("openrouter"), "{got}");
        // The money prompt names the same OpenRouter rows the plan holds.
        assert_eq!(
            mod_openrouter_models(&[or("v/a"), or("v/b"), at(), or("v/c")]),
            vec!["v/a".to_string(), "v/b".to_string()]
        );
        assert!(mod_openrouter_models(&[at(), or("v/a")]).is_empty());
        assert_eq!(
            mod_openrouter_models(&[Entry::Local, or("v/a")]),
            vec!["v/a".to_string()]
        );
    }

    /// On battery, a list whose head is Local moves onto its FIRST cloud row only (one request, with
    /// that provider's own consent), never onto a row the policy may not use, and never past an
    /// unusable first row to a later one.
    #[test]
    fn on_battery_a_local_head_moves_onto_the_first_cloud_row_only() {
        let at = CloudProvider::Anthropic;
        let ctx = RuntimeContext {
            battery_low: true,
            scope: PowerScope::Both,
            consent: Consent::ALL,
            direct_consent: DirectConsent::NONE.with(at, PowerScope::Both),
            providers: RoleProviders {
                chat: at,
                background: at,
            },
            keep_local: false,
        };
        let route = stored(vec![
            Entry::Local,
            direct(at),
            direct(CloudProvider::Gemini),
        ]);
        let moved = shape(&run(
            &route,
            Mode::Person,
            true,
            true,
            &ctx,
            &mut all_keyed,
            true,
        ));
        assert!(moved.starts_with("CloudForPower[anthropic|"), "{moved}");
        assert!(!moved.contains("gemini"), "one request, no walk: {moved}");
        assert!(moved.ends_with("[llama3]"), "{moved}");
        // The policy refused the arm (paused or at its limit): the role stays local, ladder intact.
        let stays = shape(&run(
            &route,
            Mode::Person,
            true,
            true,
            &ctx,
            &mut all_keyed,
            false,
        ));
        assert!(
            stays.starts_with("Ladder[local:llama3,anthropic:"),
            "{stays}"
        );
        // No consent for the provider: nothing moves.
        let no = RuntimeContext {
            direct_consent: DirectConsent::NONE,
            ..ctx
        };
        assert!(shape(&run(
            &route,
            Mode::Person,
            true,
            true,
            &no,
            &mut all_keyed,
            true
        ))
        .starts_with("Ladder["));
        // The first cloud row has no key: the policy does not skip to the second.
        let mut seat = |p: CloudProvider| {
            if p == at {
                Seat {
                    opted_in: true,
                    key: KeyRead::None,
                    tier: None,
                }
            } else {
                all_keyed(p)
            }
        };
        let blocked = shape(&run(
            &route,
            Mode::Person,
            true,
            true,
            &ctx,
            &mut seat,
            true,
        ));
        assert!(!blocked.starts_with("CloudForPower"), "{blocked}");
        // A list that does not start with Local never moves.
        let cloud_first = stored(vec![direct(at), Entry::Local]);
        assert!(!shape(&run(
            &cloud_first,
            Mode::Person,
            true,
            true,
            &ctx,
            &mut all_keyed,
            true
        ))
        .starts_with("CloudForPower"));
    }

    /// The plan answers for the models it may use (the cost logger) and for whether it needs Local AI.
    #[test]
    fn a_ladder_knows_its_models_and_whether_it_needs_local_ai() {
        let at = CloudProvider::Anthropic;
        let route = stored(vec![or("v/a"), or("v/b"), direct(at), Entry::Local]);
        let p = run(
            &route,
            Mode::Person,
            true,
            true,
            &INERT,
            &mut all_keyed,
            true,
        )
        .unwrap();
        let am = match direct(at) {
            Entry::Cloud { model, .. } => model,
            _ => unreachable!(),
        };
        assert_eq!(
            p.models(),
            ["v/a".to_string(), "v/b".to_string(), am, "llama3".into()]
        );
        assert_eq!(p.primary_model_id(), "v/a");
        assert!(p.needs_local_ai());
        assert!(!p.is_power_routed());
        assert!(
            p.sizing_arm().is_none(),
            "cloud first is sized like a cloud route"
        );
        let cloud_only = stored(vec![direct(at), or("v/a")]);
        let p = run(
            &cloud_only,
            Mode::Person,
            true,
            true,
            &INERT,
            &mut all_keyed,
            true,
        )
        .unwrap();
        assert!(!p.needs_local_ai());
        let local_first = stored(vec![Entry::Local, direct(at), or("v/a")]);
        let p = run(
            &local_first,
            Mode::Person,
            true,
            true,
            &INERT,
            &mut all_keyed,
            true,
        )
        .unwrap();
        assert!(
            p.sizing_arm().is_some(),
            "a local head sizes the batch to its window"
        );
    }
}
