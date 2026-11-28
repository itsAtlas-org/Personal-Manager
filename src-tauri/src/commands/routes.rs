// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The commands that edit a role's list of models, and the read model the Settings page draws it
//! from.
//!
//! A list is the one truth for "which models, in what order, with what effort" (`route.rs`). It is
//! written ONLY here and in `route/edit.rs`, never by the webview's own settings door (the row is
//! not on `WEBVIEW_PREFS`), and every row passes the gate its own kind has BEFORE it is stored:
//!
//! - a direct provider's row: [`direct_keys::gate_entry`] (turned on, a readable key, not paused
//!   where the person is), the rule the role setter and the not-working card already ask;
//! - this computer's row: Local AI is on and a model is chosen for the role;
//! - an OpenRouter row needs no key to be WRITTEN (it says "no key" when read, as it always did).
//!
//! A daily limit does not stop a row being added (it lifts at midnight). The same checks run again,
//! per row, every time a request is planned, because a restored backup can carry anything: this gate
//! is for the person editing, that one is for the money.
//!
//! The lock rules are the neighbours': the keychain and the places those gates read come BEFORE the
//! guard, and no guard is held across anything slow. Commands here are not named `set_*`: a refusal
//! must never show the Settings "Saved" tick, and the editor announces its own results.

use serde::Serialize;
use tauri::{AppHandle, Manager};

use super::direct_keys::{self, parse_role};
use crate::direct_models;
use crate::direct_providers;
use crate::entry_health;
use crate::error::{Error, Result};
use crate::llm_gateway::{self, CloudProvider, KeyPresence, Role};
use crate::offers::{EntryRef, Target};
use crate::route::edit::{self, Edited};
use crate::route::{self, Entry};
use crate::AppState;

// ---- the read model ------------------------------------------------------------------------------

/// Whether a row can answer right now, and why not, in a token the page words (`lib/roleRoute.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RowStatus {
    /// `ready`, `not_on` (the provider is not turned on), `no_key`, `key_unreadable`,
    /// `paused_region`, `paused_limit`, `not_working`, `local_off`, `no_local_model`, and for a row
    /// AFTER a this-computer row that cannot run: `after_local_off`, `after_local_pick` (never tried).
    pub kind: &'static str,
    /// For `not_working`: why, as the short fragment the walk's own notices use.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// For `not_working`: when it failed (RFC 3339), for "Not working since 14:05".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
}

impl RowStatus {
    fn of(kind: &'static str) -> Self {
        Self {
            kind,
            reason: None,
            since: None,
        }
    }
}

/// One row of a list, as the page draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RowView {
    /// Stable within a list: `local`, or `provider:model`.
    pub key: String,
    /// `local` or `cloud`.
    pub kind: &'static str,
    /// The provider token (`openrouter`, `anthropic`, `gemini`, `openai`) or `local`.
    pub provider: String,
    /// The model id; for this computer, the model chosen for the role (empty when none is).
    pub model: String,
    /// The model as a person names it.
    pub name: String,
    pub effort: Option<String>,
    /// The levels a direct model offers; empty for every other row.
    pub effort_options: Vec<String>,
    /// The catalogue's note for the model (Fable carries one), shown with the row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    /// This computer's row, when Local AI points at a server on ANOTHER machine: the page says "Your
    /// Local AI server", never "This computer", because the text leaves the machine. Absent when
    /// false (every cloud row, and a loopback server).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub local_elsewhere: bool,
    pub status: RowStatus,
}

/// A role's list for the page. `derived` is true until the first edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RouteView {
    pub entries: Vec<RowView>,
    pub derived: bool,
}

/// What an edit did: the list now, and the two things the first edit says aloud.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RouteEdited {
    pub route: RouteView,
    /// This edit stored the list for the first time.
    pub materialised: bool,
    /// On that first edit: saved OpenRouter models the list leaves out because they were not in
    /// use. "N other saved models weren't in use and aren't in this list."
    pub unused_saved: usize,
}

/// Both lists, for `route_status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RouteStatus {
    pub chat: RouteView,
    pub background: RouteView,
}

/// The facts one row's status is judged from. Read with no guard held across the keychain.
struct World {
    direct: Vec<(CloudProvider, direct_keys::EntryWorld)>,
    openrouter_key: KeyPresence,
    limit_paused: Vec<(CloudProvider, bool)>,
    local_on: bool,
    local_model: Option<String>,
    /// Local AI points at a server that is not this computer.
    local_elsewhere: bool,
}

/// Whether a saved Local AI address is a server on another machine, by the loopback test
/// `llm_gateway::local_offer_from` makes. A blank address is nothing to be elsewhere.
fn local_is_elsewhere_url(base: &str) -> bool {
    let base = base.trim();
    !base.is_empty() && !crate::openai_compat::host_is_loopback_literal(base)
}

fn read_world(app: &AppHandle, role: Role, entries: &[Entry]) -> Result<World> {
    let mut direct = Vec::new();
    let mut limit_paused = Vec::new();
    let mut wants_openrouter = false;
    for e in entries {
        let Entry::Cloud { provider, .. } = e else {
            continue;
        };
        if !limit_paused.iter().any(|(p, _)| p == provider) {
            limit_paused.push((*provider, llm_gateway::limit_paused(app, *provider)));
        }
        if provider.is_direct() {
            if !direct.iter().any(|(p, _)| p == provider) {
                if let Some(d) = direct_providers::provider(*provider) {
                    direct.push((*provider, direct_keys::read_entry_world(app, d)?));
                }
            }
        } else {
            wants_openrouter = true;
        }
    }
    let openrouter_key = if wants_openrouter {
        llm_gateway::provider_key_state(CloudProvider::OpenRouter, role)
    } else {
        KeyPresence::Absent
    };
    let (local_on, local_model, local_elsewhere) = if entries.contains(&Entry::Local) {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let on = crate::local_ai_switch::read(&conn)?;
        let (model_key, _) = llm_gateway::local_model_keys(role);
        let model = crate::db::get_setting(&conn, model_key)?
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty());
        // The same loopback test as the card and the strip, so one server is called one thing.
        let elsewhere = crate::db::get_setting(&conn, llm_gateway::LOCAL_BASE_URL_KEY)?
            .is_some_and(|b| local_is_elsewhere_url(&b));
        (on, model, elsewhere)
    } else {
        (true, None, false)
    };
    Ok(World {
        direct,
        openrouter_key,
        limit_paused,
        local_on,
        local_model,
        local_elsewhere,
    })
}

/// One row's status from the world. PURE, so every state is a plain test. The order is the order a
/// person would fix things in: turn the provider on, give it a key, wait out a pause, then trust
/// what PM remembers about it failing.
fn status_of(entry: &Entry, world: &World) -> RowStatus {
    match entry {
        Entry::Local => {
            if !world.local_on {
                RowStatus::of("local_off")
            } else if world.local_model.is_none() {
                RowStatus::of("no_local_model")
            } else {
                RowStatus::of("ready")
            }
        }
        Entry::Cloud {
            provider, model, ..
        } => {
            if provider.is_direct() {
                let Some((_, w)) = world.direct.iter().find(|(p, _)| p == provider) else {
                    return RowStatus::of("not_on");
                };
                if !w.opted_in {
                    return RowStatus::of("not_on");
                }
                match w.key {
                    KeyPresence::Absent => return RowStatus::of("no_key"),
                    KeyPresence::Unreadable => return RowStatus::of("key_unreadable"),
                    KeyPresence::Present => {}
                }
                if w.blocked.is_some() {
                    return RowStatus::of("paused_region");
                }
            } else {
                match world.openrouter_key {
                    KeyPresence::Absent => return RowStatus::of("no_key"),
                    KeyPresence::Unreadable => return RowStatus::of("key_unreadable"),
                    KeyPresence::Present => {}
                }
            }
            if world
                .limit_paused
                .iter()
                .any(|(p, paused)| p == provider && *paused)
            {
                return RowStatus::of("paused_limit");
            }
            if let Some(down) = entry_health::global().is_down(*provider, model) {
                let since: chrono::DateTime<chrono::Utc> = down.since.into();
                return RowStatus {
                    kind: "not_working",
                    reason: Some(down.class.why().to_string()),
                    since: Some(since.to_rfc3339()),
                };
            }
            RowStatus::of("ready")
        }
    }
}

/// The row's stable key within a list: `local`, or `provider:model`. The page names the row it means
/// by this on every edit that takes a position ([`expect_row`]).
fn entry_key(entry: &Entry) -> String {
    match entry {
        Entry::Local => "local".into(),
        Entry::Cloud {
            provider, model, ..
        } => format!("{}:{}", provider.as_str(), model),
    }
}

/// What a position-taking edit says when the row at that position is no longer the row the page drew
/// there (the list changed under it: another window, an edit from the card, a restore).
pub(crate) const LIST_CHANGED: &str = "Your list changed. Here it is again.";

/// Refuse an edit that names a position unless the row there is the row the person meant. Called on
/// the SAME guard as the write, so the check and the write see one list. Writes nothing.
fn expect_row(conn: &rusqlite::Connection, role: Role, index: usize, key: &str) -> Result<()> {
    let list = route::entries_for(conn, role)?;
    match list.entries.get(index) {
        Some(e) if entry_key(e) == key => Ok(()),
        _ => Err(Error::Other(LIST_CHANGED.into())),
    }
}

fn row_view(entry: &Entry, world: &World) -> RowView {
    let status = status_of(entry, world);
    match entry {
        Entry::Local => RowView {
            key: entry_key(entry),
            kind: "local",
            provider: "local".into(),
            model: world.local_model.clone().unwrap_or_default(),
            name: world
                .local_model
                .clone()
                .unwrap_or_else(|| "This computer".into()),
            effort: None,
            effort_options: Vec::new(),
            warning: None,
            local_elsewhere: world.local_elsewhere,
            status,
        },
        Entry::Cloud {
            provider,
            model,
            effort,
        } => {
            let row = provider
                .is_direct()
                .then(|| {
                    direct_models::models(*provider)
                        .iter()
                        .find(|m| m.id == *model)
                })
                .flatten();
            RowView {
                key: entry_key(entry),
                kind: "cloud",
                provider: provider.as_str().to_string(),
                model: model.clone(),
                name: row.map_or_else(|| model.clone(), |m| m.name.clone()),
                effort: effort.clone(),
                effort_options: row.map(|m| m.effort_levels.clone()).unwrap_or_default(),
                warning: row.and_then(|m| m.note.clone()),
                local_elsewhere: false,
                status,
            }
        }
    }
}

/// What a fresh install's list looks like (one OpenRouter row, not yet edited): the default the
/// Settings reset diffs against. No store or keychain is read, so its status is "ready".
pub fn default_route_view() -> RouteView {
    let world = World {
        direct: Vec::new(),
        openrouter_key: KeyPresence::Present,
        limit_paused: Vec::new(),
        local_on: true,
        local_model: None,
        local_elsewhere: false,
    };
    RouteView {
        entries: edit::default_entries()
            .iter()
            .map(|e| row_view(e, &world))
            .collect(),
        derived: true,
    }
}

/// A role's list for the SETTINGS payload (`get_settings`, read by nine call sites). Only the
/// sidebar footer reads it, and only for a list the person has edited (a list nobody edited says what
/// it always said). So an unedited role is built from the store alone, every row "ready", with no
/// keychain, limit or health read: a person who never edits a list pays nothing here that they did
/// not pay before the lists existed (R4). The list editor and the chip read `route_status`, which
/// is the full view.
pub(crate) fn route_view_for_settings(app: &AppHandle, role: Role) -> Result<RouteView> {
    let current = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        route::entries_for(&conn, role)?
    };
    if !current.derived {
        return route_view(app, role);
    }
    Ok(light_view(&current.entries))
}

/// An unedited list's rows with no store or keychain read: statuses are all "ready" (see
/// [`route_view_for_settings`]). PURE.
fn light_view(entries: &[Entry]) -> RouteView {
    let world = World {
        direct: Vec::new(),
        openrouter_key: KeyPresence::Present,
        limit_paused: Vec::new(),
        local_on: true,
        local_model: None,
        local_elsewhere: false,
    };
    RouteView {
        entries: entries
            .iter()
            .map(|e| {
                let mut row = row_view(e, &world);
                row.status = RowStatus::of("ready");
                row
            })
            .collect(),
        derived: true,
    }
}

/// A role's list as the page draws it, from the store and the keychain.
pub(crate) fn route_view(app: &AppHandle, role: Role) -> Result<RouteView> {
    let (current, local_on, waiting) = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let (_, emptied_key) = llm_gateway::local_model_keys(role);
        (
            route::entries_for(&conn, role)?,
            crate::local_ai_switch::read(&conn)?,
            crate::db::get_bool(&conn, emptied_key, false)?,
        )
    };
    let world = read_world(app, role, &current.entries)?;
    let mut entries: Vec<RowView> = current
        .entries
        .iter()
        .map(|e| row_view(e, &world))
        .collect();
    // A list nobody edited keeps the old resolver's statuses (R4); a person's own list says where its
    // walk stops.
    if !current.derived {
        mark_after_stop(&mut entries, local_on, waiting);
    }
    Ok(RouteView {
        entries,
        derived: current.derived,
    })
}

/// While this computer's row cannot run (Local AI off, or the role waiting for a model), the walk
/// STOPS at it: no row after it is ever tried. Say so on those rows, in place of a "Ready" that would
/// be false (`after_local_off`, `after_local_pick`). PURE.
fn mark_after_stop(rows: &mut [RowView], local_on: bool, waiting: bool) {
    let kind = if !local_on {
        "after_local_off"
    } else if waiting {
        "after_local_pick"
    } else {
        return;
    };
    let Some(at) = rows.iter().position(|r| r.kind == "local") else {
        return;
    };
    for row in &mut rows[at + 1..] {
        row.status = RowStatus::of(kind);
    }
}

/// Both roles' lists and their rows' status. Read-only; the page asks again on `route://changed`.
#[tauri::command]
pub fn route_status(app: AppHandle) -> Result<RouteStatus> {
    Ok(RouteStatus {
        chat: route_view(&app, Role::Chat)?,
        background: route_view(&app, Role::Background)?,
    })
}

// ---- the writes -----------------------------------------------------------------------------------

/// A row the webview names, as a list row. An allow-list on the provider; nothing else it sends is
/// trusted (the model is checked against the catalogue by `normalise` when the row is stored).
fn entry_from(provider: &str, model: &str, effort: Option<String>) -> Result<Entry> {
    let target = EntryRef {
        provider: provider.to_string(),
        model: model.to_string(),
    }
    .target()?;
    Ok(match target {
        Target::Local => Entry::Local,
        Target::Cloud(p) => Entry::Cloud {
            provider: p,
            model: model.trim().to_string(),
            effort: effort.filter(|_| p.is_direct()),
        },
    })
}

/// THE gate a row passes before it can be written: the provider's own for a direct row, Local AI's
/// for this computer's, nothing for OpenRouter. A daily limit is not a refusal here.
pub(crate) fn gate_for_write(app: &AppHandle, role: Role, entry: &Entry) -> Result<()> {
    match entry {
        Entry::Local => {
            let on = {
                let state = app.state::<AppState>();
                let conn = state.conn()?;
                crate::local_ai_switch::read(&conn)?
            };
            if !on {
                return Err(Error::Other(
                    crate::local_ai_switch::OFF_REFUSAL.to_string(),
                ));
            }
            if llm_gateway::local_offer_model(app, role).is_none() {
                return Err(Error::Other(format!(
                    "Pick a local model for {} in Local AI first.",
                    match role {
                        Role::Chat => "Chat",
                        Role::Background => "Background",
                    }
                )));
            }
            Ok(())
        }
        Entry::Cloud { provider, .. } if provider.is_direct() => {
            let d = direct_providers::provider(*provider)
                .ok_or_else(|| Error::Other("PM doesn't know that provider.".into()))?;
            direct_keys::gate_entry_now(app, role, d)
        }
        Entry::Cloud { .. } => Ok(()),
    }
}

/// What every write ends with: tell the status readers, and answer with the list as it is now.
fn finish(app: &AppHandle, role: Role, edited: Edited) -> Result<RouteEdited> {
    llm_gateway::ping_status(app);
    llm_gateway::route_changed(app);
    Ok(RouteEdited {
        route: route_view(app, role)?,
        materialised: edited.materialised,
        unused_saved: edited.unused_saved,
    })
}

/// Add a model to a role's list, at `at` (the end by default). Refused with the gate's sentence when
/// the provider is not turned on, has no key, or is paused where the person is; refused for a model
/// PM does not list, a duplicate, a second this-computer row, or past 50 rows.
#[tauri::command]
pub fn route_add(
    app: AppHandle,
    role: String,
    provider: String,
    model: String,
    effort: Option<String>,
    at: Option<usize>,
) -> Result<RouteEdited> {
    let role = parse_role(&role)?;
    let entry = entry_from(&provider, &model, effort)?;
    gate_for_write(&app, role, &entry)?;
    let edited = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        edit::add(&conn, role, entry, at)?
    };
    finish(&app, role, edited)
}

/// Move the row at `from` to position `to`. `key` is the row the page means to move (`local`, or
/// `provider:model`): when the row at `from` is not that row the list changed under the page, and
/// the edit is refused with [`LIST_CHANGED`] and writes nothing.
#[tauri::command]
pub fn route_move(
    app: AppHandle,
    role: String,
    from: usize,
    to: usize,
    key: String,
) -> Result<RouteEdited> {
    let role = parse_role(&role)?;
    let edited = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        expect_row(&conn, role, from, &key)?;
        edit::move_entry(&conn, role, from, to)?
    };
    finish(&app, role, edited)
}

/// Take the row at `index` out, if it is the row `key` names. The last row cannot go.
#[tauri::command]
pub fn route_remove(
    app: AppHandle,
    role: String,
    index: usize,
    key: String,
) -> Result<RouteEdited> {
    let role = parse_role(&role)?;
    let edited = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        expect_row(&conn, role, index, &key)?;
        edit::remove(&conn, role, index)?
    };
    finish(&app, role, edited)
}

/// Set the effort of the direct row at `index`, if it is the row `key` names; `None` is the role's
/// default.
#[tauri::command]
pub fn route_set_effort(
    app: AppHandle,
    role: String,
    index: usize,
    key: String,
    effort: Option<String>,
) -> Result<RouteEdited> {
    let role = parse_role(&role)?;
    let edited = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        expect_row(&conn, role, index, &key)?;
        edit::set_effort(&conn, role, index, effort)?
    };
    finish(&app, role, edited)
}

/// Write the default list (what a fresh install uses) as the role's own. Reset can only lose a
/// choice.
#[tauri::command]
pub fn route_reset(app: AppHandle, role: String) -> Result<RouteEdited> {
    let role = parse_role(&role)?;
    let edited = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        edit::reset(&conn, role)?
    };
    finish(&app, role, edited)
}

/// "Use {model} from now on": put the row first in the role's list (insert it, or move it there).
/// The webview's words are checked here like any other write: the same gate, the same limits. The
/// not-working card sends this and then re-sends the failed message.
#[tauri::command]
pub fn promote_entry(
    app: AppHandle,
    role: String,
    provider: String,
    model: String,
) -> Result<RouteEdited> {
    let role = parse_role(&role)?;
    let entry = entry_from(&provider, &model, None)?;
    gate_for_write(&app, role, &entry)?;
    let edited = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        edit::promote(&conn, role, entry)?
    };
    finish(&app, role, edited)
}

/// "Try again now": forget what PM remembered about this provider failing, so the next request
/// tries its rows again.
#[tauri::command]
pub fn route_try_again(app: AppHandle, provider: String) -> Result<()> {
    let target = EntryRef {
        provider,
        model: String::new(),
    }
    .target()?;
    match target {
        Target::Cloud(p) => entry_health::global().clear_provider(p),
        Target::Local => {}
    }
    llm_gateway::route_changed(&app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> World {
        World {
            direct: Vec::new(),
            openrouter_key: KeyPresence::Present,
            limit_paused: Vec::new(),
            local_on: true,
            local_model: Some("gemma3:4b".into()),
            local_elsewhere: false,
        }
    }

    /// The Settings payload is read on every settings call, so an unedited list is built with no
    /// keychain, limit or health read (delivery audit S6): its rows are all "ready", whatever the
    /// provider, because nothing reads a derived list's statuses from there. The full view is still
    /// used for an edited list, and `get_settings` asks for the light one.
    #[test]
    fn the_settings_payload_reads_nothing_for_a_list_nobody_edited() {
        let claude = Entry::Cloud {
            provider: CloudProvider::Anthropic,
            model: "claude-haiku-5-5".into(),
            effort: None,
        };
        let v = light_view(&[or("a/1"), claude.clone(), Entry::Local]);
        assert!(v.derived);
        assert_eq!(v.entries.len(), 3);
        assert!(v.entries.iter().all(|r| r.status.kind == "ready"));
        // The same rows through the full path with no world would say the provider is not on.
        assert_eq!(status_of(&claude, &world()).kind, "not_on");

        let live = include_str!("routes.rs")
            .split("pub(crate) fn route_view_for_settings(")
            .nth(1)
            .and_then(|s| s.split("\n}\n").next())
            .expect("route_view_for_settings");
        assert!(live.contains("if !current.derived {\n        return route_view(app, role);"));
        assert!(live.contains("light_view(&current.entries)"));
        let settings = include_str!("../settings.rs");
        assert!(settings.contains("route_view_for_settings(&app, Role::Chat)"));
        assert!(settings.contains("route_view_for_settings(&app, Role::Background)"));
        assert!(!settings.contains("commands::route_view(&app"));
    }

    /// A Local row on another machine says so on the page (the same loopback test as the card and
    /// the strip), and a loopback row, a cloud row and a blank address say nothing.
    #[test]
    fn a_local_row_on_another_machine_is_not_called_this_computer() {
        assert!(!local_is_elsewhere_url("http://127.0.0.1:11434"));
        assert!(!local_is_elsewhere_url("http://localhost:11434"));
        assert!(local_is_elsewhere_url("http://192.168.1.20:11434"));
        assert!(local_is_elsewhere_url("https://my-box.example.com"));
        assert!(!local_is_elsewhere_url("  "));
        let mut w = world();
        w.local_elsewhere = true;
        let row = row_view(&Entry::Local, &w);
        assert!(row.local_elsewhere);
        assert_eq!(serde_json::to_value(&row).unwrap()["local_elsewhere"], true);
        assert!(!row_view(&Entry::Local, &world()).local_elsewhere);
        assert!(serde_json::to_value(row_view(&or("a/1"), &w))
            .unwrap()
            .get("local_elsewhere")
            .is_none());
    }

    fn or(model: &str) -> Entry {
        Entry::Cloud {
            provider: CloudProvider::OpenRouter,
            model: model.into(),
            effort: None,
        }
    }

    fn claude() -> Entry {
        let m = direct_models::default_for(CloudProvider::Anthropic, Role::Chat).unwrap();
        Entry::Cloud {
            provider: CloudProvider::Anthropic,
            model: m.id.clone(),
            effort: None,
        }
    }

    fn direct_world(opted_in: bool, key: KeyPresence, blocked: bool) -> direct_keys::EntryWorld {
        direct_keys::EntryWorld {
            opted_in,
            key,
            blocked: blocked.then(|| {
                let uk = crate::region::region_of_on(Some("Europe/London"), false);
                direct_providers::refuses(
                    CloudProvider::Gemini,
                    Some(direct_providers::KeyTier::Free),
                    &uk,
                )
                .expect("a free Gemini key in the UK is refused")
            }),
            tier: None,
        }
    }

    /// An edit that names a position carries the row it means: when the list moved under the page
    /// (another window, the card's "from now on", a restore) the row at that position is another row,
    /// and the edit is refused with one plain sentence and writes nothing. Position and key must both
    /// agree; a position past the end is refused the same way.
    #[test]
    fn a_position_edit_is_refused_when_the_row_there_is_not_the_row_the_page_meant() {
        let dir = tempfile::tempdir().unwrap();
        let pass = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let conn = crate::db::open(&dir.path().join("t.sqlite"), pass).unwrap();
        edit::add(&conn, Role::Chat, or("v/second"), None).unwrap();
        let first = entry_key(&edit::default_entries()[0]);
        let second = "openrouter:v/second";
        assert!(expect_row(&conn, Role::Chat, 0, &first).is_ok());
        assert!(expect_row(&conn, Role::Chat, 1, second).is_ok());
        // The list changes under the page: the second row now leads.
        edit::move_entry(&conn, Role::Chat, 1, 0).unwrap();
        let after = route::entries_for(&conn, Role::Chat).unwrap().entries;
        for (index, key) in [(0, first.as_str()), (1, second), (0, "local"), (7, second)] {
            let said = expect_row(&conn, Role::Chat, index, key).unwrap_err();
            assert_eq!(said.to_string(), LIST_CHANGED, "{index} {key}");
        }
        assert_eq!(LIST_CHANGED, "Your list changed. Here it is again.");
        assert!(expect_row(&conn, Role::Chat, 0, second).is_ok());
        assert_eq!(
            route::entries_for(&conn, Role::Chat).unwrap().entries,
            after,
            "a refusal writes nothing"
        );
        // The three position-taking commands check on the same guard BEFORE they write, and `key`
        // is a parameter of each, so no caller can skip it.
        let live = include_str!("routes.rs")
            .split("#[cfg(test)]\nmod tests {")
            .next()
            .unwrap();
        for (sig, write) in [
            ("pub fn route_move(", "edit::move_entry("),
            ("pub fn route_remove(", "edit::remove("),
            ("pub fn route_set_effort(", "edit::set_effort("),
        ] {
            let body = live.split(sig).nth(1).expect(sig);
            let head = body.split("\n}\n").next().unwrap();
            assert!(head.contains("key: String"), "{sig}");
            let check = head.find("expect_row(&conn, role,").expect(sig);
            assert!(check < head.find(write).expect(sig), "{sig}: check first");
        }
    }

    #[test]
    fn a_row_reports_the_first_thing_a_person_would_fix() {
        let mut w = world();
        // OpenRouter: only its key matters.
        assert_eq!(status_of(&or("a/b"), &w).kind, "ready");
        w.openrouter_key = KeyPresence::Absent;
        assert_eq!(status_of(&or("a/b"), &w).kind, "no_key");
        w.openrouter_key = KeyPresence::Unreadable;
        assert_eq!(status_of(&or("a/b"), &w).kind, "key_unreadable");

        // A direct row: on, then key, then region, then the limit.
        let c = claude();
        assert_eq!(status_of(&c, &w).kind, "not_on", "no world read = not on");
        w.direct = vec![(
            CloudProvider::Anthropic,
            direct_world(false, KeyPresence::Present, false),
        )];
        assert_eq!(status_of(&c, &w).kind, "not_on");
        w.direct = vec![(
            CloudProvider::Anthropic,
            direct_world(true, KeyPresence::Absent, false),
        )];
        assert_eq!(status_of(&c, &w).kind, "no_key");
        w.direct = vec![(
            CloudProvider::Anthropic,
            direct_world(true, KeyPresence::Unreadable, false),
        )];
        assert_eq!(status_of(&c, &w).kind, "key_unreadable");
        w.direct = vec![(
            CloudProvider::Anthropic,
            direct_world(true, KeyPresence::Present, true),
        )];
        assert_eq!(status_of(&c, &w).kind, "paused_region");
        w.direct = vec![(
            CloudProvider::Anthropic,
            direct_world(true, KeyPresence::Present, false),
        )];
        assert_eq!(status_of(&c, &w).kind, "ready");
        w.limit_paused = vec![(CloudProvider::Anthropic, true)];
        assert_eq!(status_of(&c, &w).kind, "paused_limit");
    }

    /// The rows after a this-computer row that cannot run say they are never tried; the rows above it
    /// and the Local row itself keep their own statuses, and a Local row that can run changes nothing.
    #[test]
    fn rows_after_a_local_row_that_cannot_run_say_they_are_never_tried() {
        let w = world();
        let rows = |w: &World| {
            [or("a/0"), Entry::Local, or("a/1")]
                .iter()
                .map(|e| row_view(e, w))
                .collect::<Vec<_>>()
        };
        let kinds = |rows: &[RowView]| rows.iter().map(|r| r.status.kind).collect::<Vec<_>>();
        let mut w_off = world();
        w_off.local_on = false;
        let mut off = rows(&w_off);
        mark_after_stop(&mut off, false, false);
        assert_eq!(
            kinds(&off),
            ["ready", "local_off", "after_local_off"],
            "the row above and the Local row keep their own statuses"
        );
        let mut waiting = rows(&w);
        mark_after_stop(&mut waiting, true, true);
        assert_eq!(kinds(&waiting)[2], "after_local_pick");
        let mut running = rows(&w);
        mark_after_stop(&mut running, true, false);
        assert_eq!(
            kinds(&running)[2],
            "ready",
            "a Local row that can run stops nothing"
        );
        // No Local row: nothing to stop at.
        let mut cloud = vec![row_view(&or("a/1"), &w), row_view(&or("a/2"), &w)];
        mark_after_stop(&mut cloud, false, false);
        assert_eq!(kinds(&cloud), ["ready", "ready"]);
    }

    #[test]
    fn this_computer_is_ready_only_with_local_ai_on_and_a_model() {
        let mut w = world();
        assert_eq!(status_of(&Entry::Local, &w).kind, "ready");
        w.local_model = None;
        assert_eq!(status_of(&Entry::Local, &w).kind, "no_local_model");
        w.local_on = false;
        assert_eq!(status_of(&Entry::Local, &w).kind, "local_off");
    }

    /// THE WRITE GATE: every command that puts a row INTO a list asks `gate_for_write` before it
    /// calls the writer, and the gate for a direct row is the same `gate_entry` the role setter and
    /// the not-working card ask. A scan, because the behaviour needs a keychain.
    #[test]
    fn a_row_is_gated_before_it_is_written() {
        let live = include_str!("routes.rs")
            .split("#[cfg(test)]\nmod tests {")
            .next()
            .unwrap();
        for (command, writer) in [
            ("pub fn route_add(", "edit::add("),
            ("pub fn promote_entry(", "edit::promote("),
        ] {
            let body = live
                .split(command)
                .nth(1)
                .unwrap_or_else(|| panic!("{command}"))
                .split("\n}\n")
                .next()
                .unwrap();
            let gate = body
                .find("gate_for_write(")
                .unwrap_or_else(|| panic!("{command} never asks the gate"));
            let write = body
                .find(writer)
                .unwrap_or_else(|| panic!("{command} never writes"));
            assert!(gate < write, "{command} writes before it asks the gate");
        }
        let gate = live
            .split("pub(crate) fn gate_for_write(")
            .nth(1)
            .unwrap()
            .split("\n}\n")
            .next()
            .unwrap();
        assert!(
            gate.contains("gate_entry_now("),
            "a direct row passes gate_entry"
        );
        assert!(
            gate.contains("local_ai_switch::read("),
            "this computer needs Local AI on"
        );
        assert!(
            gate.contains("local_offer_model("),
            "and a model chosen for the role"
        );
        // Only these two commands add a row (the others move, remove or retune ones already there).
        assert_eq!(live.matches("edit::add(").count(), 1);
        assert_eq!(live.matches("edit::promote(").count(), 1);
    }

    /// The write gate and the status share their words with the planner: a row the gate would
    /// refuse is never `ready`.
    #[test]
    fn an_entry_from_the_page_is_an_allow_list_of_providers() {
        assert_eq!(entry_from("local", "", None).unwrap(), Entry::Local);
        let e = entry_from("anthropic", " m ", Some("high".into())).unwrap();
        assert_eq!(
            e,
            Entry::Cloud {
                provider: CloudProvider::Anthropic,
                model: "m".into(),
                effort: Some("high".into()),
            }
        );
        // OpenRouter has no effort of its own.
        let e = entry_from("openrouter", "a/b", Some("high".into())).unwrap();
        assert_eq!(e, or("a/b"));
        for bad in ["", "Anthropic", "mythos", "google", "openrouter "] {
            let r = entry_from(bad, "m", None);
            if bad.trim() == "openrouter" {
                assert!(r.is_ok());
            } else {
                assert!(r.is_err(), "{bad:?}");
            }
        }
    }
}
