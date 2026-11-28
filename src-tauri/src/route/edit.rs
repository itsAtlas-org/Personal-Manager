// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The WRITER half of a role's list: every way a list changes, on a plain connection so each rule
//! is a test and none of them can reach the keychain or the network.
//!
//! The commands (`commands/routes.rs`) do what needs the world first (is this provider turned on,
//! keyed and not paused where the person is; is Local AI on with a model chosen) and then call in
//! here. Everything here is a function of the stored list and its arguments:
//!
//! - **One gate.** Every write goes through [`save`], which runs [`super::normalise`] and REFUSES a
//!   list that loses a row to it or ends up empty, so a stored list always satisfies every limit and
//!   a write never quietly repairs one row into another company's model.
//! - **The first edit materialises.** A role nobody has edited reads from the older rows; the first
//!   write stores the WHOLE list that read gave, with the edit applied, and says how many saved
//!   OpenRouter models that list leaves out (they were never in use, owner Q7).
//! - **This module and `route.rs` are the only writers of the older routing rows.** They are touched
//!   only to retire a provider from a list nobody has edited (the older "move back to OpenRouter"),
//!   and a scan test pins it.
//!
//! A write also forgets what PM remembered about rows not working (entry health): the person has just
//! said what they want, so a stale mark must never outlive the list it was about.

use rusqlite::Connection;

use super::{
    derive_cloud, entries_for, normalise, route_key, stored_entries, to_json, DropReason, Entry,
    RoleRoute,
};
use crate::direct_models;
use crate::direct_providers;
use crate::error::{Error, Result};
use crate::llm_gateway::{self, CloudProvider, ProviderPref, Role};
use crate::{db, settings};

/// What a write did, for the command to report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Edited {
    /// The list as stored now.
    pub entries: Vec<Entry>,
    /// This write stored the list for the first time.
    pub materialised: bool,
    /// On a first write only: how many OpenRouter models the person had saved that the list does not
    /// hold, because they were not in use (auto-switch was off). Zero otherwise.
    pub unused_saved: usize,
}

/// What a fresh install's list is, and what an emptied list gets back: OpenRouter's default model.
pub(crate) fn default_entries() -> Vec<Entry> {
    vec![Entry::Cloud {
        provider: CloudProvider::OpenRouter,
        model: settings::DEFAULT_MODEL.to_string(),
        effort: None,
    }]
}

fn role_word(role: Role) -> &'static str {
    match role {
        Role::Chat => "Chat",
        Role::Background => "Background",
    }
}

/// Why a row was refused, in a sentence a person can act on.
pub(crate) fn drop_sentence(why: DropReason) -> &'static str {
    match why {
        DropReason::Empty => "That model has no name.",
        DropReason::TooLong => "That model name is too long.",
        DropReason::NotListed => "PM doesn't list that model.",
        DropReason::Retired => "That model has been retired.",
        DropReason::Mythos => "PM doesn't offer that model.",
        DropReason::SecondLocal => "This computer is already in the list.",
        DropReason::Duplicate => "That model is already in the list.",
        DropReason::TooMany => "A list holds at most 50 models.",
    }
}

/// The list as it stands, and whether it is only derived so far.
fn load(conn: &Connection, role: Role) -> Result<RoleRoute> {
    entries_for(conn, role)
}

/// Saved OpenRouter models a derived list leaves out. Only when the role is on OpenRouter with
/// auto-switch off: then the old code used the first id alone, so the others never cost anything and
/// a list showing only the first adds no spend. The first edit tells the person once (Q7).
fn unused_saved(conn: &Connection, role: Role, derived: &[Entry]) -> Result<usize> {
    let (models_key, auto_key) = llm_gateway::openrouter_keys(role);
    if db::get_bool(conn, auto_key, false)? {
        return Ok(0);
    }
    let used = derived
        .iter()
        .filter(|e| {
            matches!(
                e,
                Entry::Cloud {
                    provider: CloudProvider::OpenRouter,
                    ..
                }
            )
        })
        .count();
    if used == 0 {
        return Ok(0);
    }
    Ok(settings::models_for(conn, models_key)?
        .len()
        .saturating_sub(used))
}

/// The one write. Normalises, refuses a list that loses a row or has none, stores it, and forgets
/// what was remembered about rows not working. Returns the stored list.
fn save(conn: &Connection, role: Role, entries: Vec<Entry>) -> Result<Vec<Entry>> {
    let clean = normalise(role, entries);
    if let Some(first) = clean.dropped.first() {
        return Err(Error::Other(drop_sentence(first.why).into()));
    }
    if clean.entries.is_empty() {
        return Err(Error::Other(format!(
            "{} needs at least one model. Add another before taking this one out.",
            role_word(role)
        )));
    }
    db::set_setting(conn, route_key(role), &to_json(&clean.entries)?)?;
    crate::entry_health::global().clear_all();
    Ok(clean.entries)
}

/// Apply `change` to the role's list and store the result. `change` returns the new list, or `None`
/// when it has nothing to change; then nothing is written for a list nobody has edited, so a
/// control that changes nothing never turns a derived list into a stored one.
fn edit_with<F>(conn: &Connection, role: Role, always_write: bool, change: F) -> Result<Edited>
where
    F: FnOnce(Vec<Entry>) -> Result<Option<Vec<Entry>>>,
{
    let current = load(conn, role)?;
    let derived = current.derived;
    let unused = if derived {
        unused_saved(conn, role, &current.entries)?
    } else {
        0
    };
    let before = current.entries.clone();
    let next = change(current.entries)?;
    let unchanged = match &next {
        None => true,
        Some(n) => *n == before,
    };
    if unchanged && derived && !always_write {
        return Ok(Edited {
            entries: before,
            materialised: false,
            unused_saved: 0,
        });
    }
    let entries = save(conn, role, next.unwrap_or(before))?;
    Ok(Edited {
        entries,
        materialised: derived,
        unused_saved: unused,
    })
}

fn out_of_range() -> Error {
    Error::Other("That row isn't in the list any more.".into())
}

/// Add a row at `at` (the end when `None` or past the end). The caller has already asked the
/// entry's own gate.
pub(crate) fn add(
    conn: &Connection,
    role: Role,
    entry: Entry,
    at: Option<usize>,
) -> Result<Edited> {
    edit_with(conn, role, true, |mut list| {
        let at = at.unwrap_or(list.len()).min(list.len());
        list.insert(at, entry);
        Ok(Some(list))
    })
}

/// Move the row at `from` so it sits at `to`.
pub(crate) fn move_entry(conn: &Connection, role: Role, from: usize, to: usize) -> Result<Edited> {
    edit_with(conn, role, true, |mut list| {
        if from >= list.len() || to >= list.len() {
            return Err(out_of_range());
        }
        let row = list.remove(from);
        list.insert(to, row);
        Ok(Some(list))
    })
}

/// Take the row at `index` out. The last row cannot go ([`save`] refuses an empty list).
pub(crate) fn remove(conn: &Connection, role: Role, index: usize) -> Result<Edited> {
    edit_with(conn, role, true, |mut list| {
        if index >= list.len() {
            return Err(out_of_range());
        }
        list.remove(index);
        Ok(Some(list))
    })
}

/// Set a direct row's effort: `None` is the role's default; a level must be one the model offers
/// (the word "default" is not a level). An OpenRouter or this-computer row has no effort to set.
pub(crate) fn set_effort(
    conn: &Connection,
    role: Role,
    index: usize,
    effort: Option<String>,
) -> Result<Edited> {
    edit_with(conn, role, true, |mut list| {
        let row = list.get_mut(index).ok_or_else(out_of_range)?;
        let Entry::Cloud {
            provider,
            model,
            effort: slot,
        } = row
        else {
            return Err(Error::Other("This row has no effort to set.".into()));
        };
        if !provider.is_direct() {
            return Err(Error::Other("This row has no effort to set.".into()));
        }
        let catalogue = direct_models::models(*provider)
            .iter()
            .find(|m| m.id == *model)
            .ok_or_else(|| Error::Other("PM doesn't list that model.".into()))?;
        *slot = match effort {
            Some(level) => {
                if !catalogue.offers(&level) {
                    return Err(Error::Other(format!(
                        "{} doesn't take effort '{level}'.",
                        catalogue.name
                    )));
                }
                Some(level)
            }
            None => direct_providers::default_effort(role, catalogue),
        };
        Ok(Some(list))
    })
}

/// Put a row first: insert it at the head, or move it there when the list already holds it. The row
/// keeps its own effort when the list had one and the caller gave none. This is "Use X from now on".
pub(crate) fn promote(conn: &Connection, role: Role, entry: Entry) -> Result<Edited> {
    edit_with(conn, role, true, |mut list| {
        let mut entry = entry;
        if let Some(at) = list.iter().position(|e| e.same_slot(&entry)) {
            let old = list.remove(at);
            if let (
                Entry::Cloud { effort: new, .. },
                Entry::Cloud {
                    effort: Some(kept), ..
                },
            ) = (&mut entry, old)
            {
                if new.is_none() {
                    *new = Some(kept);
                }
            }
        }
        list.insert(0, entry);
        Ok(Some(list))
    })
}

/// Write the default list (what a fresh install derives) as an explicit stored row. Reset can only
/// lose a choice, and never one that keeps data nearer: a "This computer" row stays where it stands
/// (it is the person's Local AI choice, not one of the cloud rows being reset), so a Reset can never
/// turn a local-first role into a cloud-only one. A list with no cloud row at all ("Local only")
/// has nothing to reset: the default's cloud row would be a fallback the person never chose, so
/// Reset leaves it exactly as it is and writes nothing (review round 2, N1).
pub(crate) fn reset(conn: &Connection, role: Role) -> Result<Edited> {
    let current = load(conn, role)?;
    if current.entries.iter().all(|e| *e == Entry::Local) {
        return Ok(Edited {
            entries: current.entries,
            materialised: false,
            unused_saved: 0,
        });
    }
    edit_with(conn, role, true, |list| {
        let mut out = default_entries();
        if let Some(at) = list.iter().position(|e| *e == Entry::Local) {
            out.insert(at.min(out.len()), Entry::Local);
        }
        Ok(Some(out))
    })
}

/// Replace the rows `matching` selects with `new`, which takes the place of the first row removed
/// (the end when none was). What the older one-provider controls do in terms of the list. Writes
/// nothing when the result is the list as it stands.
pub(crate) fn replace_rows<M>(
    conn: &Connection,
    role: Role,
    matching: M,
    new: Vec<Entry>,
) -> Result<Edited>
where
    M: Fn(&Entry) -> bool,
{
    edit_with(conn, role, false, |list| {
        let at = list.iter().position(&matching).unwrap_or(list.len());
        let mut out: Vec<Entry> = Vec::new();
        let mut placed = false;
        for (i, e) in list.into_iter().enumerate() {
            if i == at && !placed {
                out.extend(new.iter().cloned());
                placed = true;
            }
            if !matching(&e) {
                out.push(e);
            }
        }
        if !placed {
            out.extend(new);
        }
        Ok(Some(out))
    })
}

/// The context meter's "use a larger model": `model` goes where the list's FIRST OpenRouter row
/// stands (inserted, or moved up there when the list already holds it) and every other row stays
/// where the person put it. Writing the whole OpenRouter set instead would pull a later OpenRouter
/// row (below a Claude row, say) up into the first request, ahead of what the person wrote (audit
/// S1). A list that has no OpenRouter row is left alone: the switch never adds a company. A list
/// nobody has edited keeps the older meaning (the model first, the saved models behind it, only
/// the first in use while auto-switch is off), so the first edit says how many it left out.
pub(crate) fn promote_openrouter(conn: &Connection, role: Role, model: &str) -> Result<Edited> {
    let model = model.trim();
    let current = load(conn, role)?;
    let first = current.entries.iter().position(is_openrouter);
    let (true, Some(first)) = (
        !model.is_empty() && model.chars().count() <= super::MAX_MODEL_ID_CHARS,
        first,
    ) else {
        return Ok(Edited {
            entries: current.entries,
            materialised: false,
            unused_saved: 0,
        });
    };
    let row = Entry::Cloud {
        provider: CloudProvider::OpenRouter,
        model: model.to_string(),
        effort: None,
    };
    if current.derived {
        let mut ids = vec![model.to_string()];
        for e in &current.entries {
            if let Entry::Cloud {
                provider: CloudProvider::OpenRouter,
                model: m,
                ..
            } = e
            {
                ids.push(m.clone());
            }
        }
        return set_openrouter_models(conn, role, &ids);
    }
    edit_with(conn, role, false, |mut list| {
        match list.iter().position(|e| e.same_slot(&row)) {
            Some(at) if at > first => {
                list.remove(at);
                list.insert(first, row);
            }
            Some(_) => {}
            None => list.insert(first, row),
        }
        Ok(Some(list))
    })
}

/// The older "which models does OpenRouter try" control: the OpenRouter rows become exactly `ids`,
/// in that order, where the first OpenRouter row was.
pub(crate) fn set_openrouter_models(
    conn: &Connection,
    role: Role,
    ids: &[String],
) -> Result<Edited> {
    let mut seen: Vec<String> = Vec::new();
    for id in ids {
        let id = id.trim();
        if !id.is_empty()
            && id.chars().count() <= super::MAX_MODEL_ID_CHARS
            && !seen.iter().any(|s| s == id)
            && seen.len() < super::MAX_ENTRIES
        {
            seen.push(id.to_string());
        }
    }
    let rows: Vec<Entry> = seen
        .into_iter()
        .map(|model| Entry::Cloud {
            provider: CloudProvider::OpenRouter,
            model,
            effort: None,
        })
        .collect();
    // Saved models of a role that is not on OpenRouter were inert before, and stay inert: this never
    // adds an OpenRouter row to a list that has none (that would be a fallback the person did not
    // write, and a bill on a key they did not choose for this role). The list editor adds rows.
    let current = load(conn, role)?;
    if rows.is_empty() || !current.entries.iter().any(is_openrouter) {
        return Ok(Edited {
            entries: current.entries,
            materialised: false,
            unused_saved: 0,
        });
    }
    // A list nobody has edited, with auto-switch off, used the FIRST saved model only: writing the
    // whole saved list would make PM try models it never tried, so the first edit keeps the one that
    // was in use (and says how many it left out: `Edited::unused_saved`).
    let (_, auto_key) = llm_gateway::openrouter_keys(role);
    let mut rows = rows;
    if current.derived && !db::get_bool(conn, auto_key, false)? {
        rows.truncate(1);
    }
    replace_rows(conn, role, is_openrouter, rows)
}

/// The older "auto-switch" toggle, in terms of the list. On means every saved OpenRouter model is
/// tried in order, which for a list nobody has edited is the whole saved list; a stored list already
/// tries every OpenRouter row it holds, so there is nothing to turn on. Off means the first
/// OpenRouter row alone. (The toggle itself goes in the new editor; this keeps the older Settings
/// page truthful until it is replaced.)
pub(crate) fn set_openrouter_auto(conn: &Connection, role: Role, on: bool) -> Result<Edited> {
    let current = load(conn, role)?;
    // A list nobody has edited reads this very row to decide how many saved models are in use, so
    // the switch is remembered for it FIRST: the first-run wizard turns auto-switch on and saves its
    // models in either order, and a save that found the switch still off would keep only the first
    // model (and the switch, turned on after, would then have nothing left to switch between).
    if current.derived {
        let (_, auto_key) = llm_gateway::openrouter_keys(role);
        if db::get_bool(conn, auto_key, false)? != on {
            db::set_bool(conn, auto_key, on)?;
        }
    }
    let rows: Vec<Entry> = if on {
        // A role that is not on OpenRouter has no OpenRouter row to widen: turning the switch on
        // there must not ADD one (a fallback the person never wrote, on a key they did not choose
        // for this role), the same rule `set_openrouter_models` keeps.
        if !current.derived || !current.entries.iter().any(is_openrouter) {
            return Ok(Edited {
                entries: current.entries,
                materialised: false,
                unused_saved: 0,
            });
        }
        let (models_key, _) = llm_gateway::openrouter_keys(role);
        settings::models_for(conn, models_key)?
            .into_iter()
            .map(|model| Entry::Cloud {
                provider: CloudProvider::OpenRouter,
                model,
                effort: None,
            })
            .collect()
    } else {
        current
            .entries
            .iter()
            .find(|e| is_openrouter(e))
            .cloned()
            .into_iter()
            .collect()
    };
    if rows.is_empty() {
        return Ok(Edited {
            entries: current.entries,
            materialised: false,
            unused_saved: 0,
        });
    }
    replace_rows(conn, role, is_openrouter, rows)
}

fn is_openrouter(e: &Entry) -> bool {
    matches!(
        e,
        Entry::Cloud {
            provider: CloudProvider::OpenRouter,
            ..
        }
    )
}

fn is_cloud(e: &Entry) -> bool {
    matches!(e, Entry::Cloud { .. })
}

/// The older "which provider answers this role" control: every cloud row becomes that provider's.
/// OpenRouter gets the models the person last saved for the role (else the default); a direct
/// provider gets its default model for the role, with that role's default effort. This computer's
/// row stays where it is.
pub(crate) fn use_provider(conn: &Connection, role: Role, p: CloudProvider) -> Result<Edited> {
    let rows = if p.is_direct() {
        let model = direct_models::default_for(p, role)
            .ok_or_else(|| Error::Other("PM has no model for that provider.".into()))?;
        vec![Entry::Cloud {
            provider: p,
            model: model.id.clone(),
            effort: direct_providers::default_effort(role, model),
        }]
    } else {
        let (models_key, auto_key) = llm_gateway::openrouter_keys(role);
        let mut rows: Vec<Entry> = settings::models_for(conn, models_key)?
            .into_iter()
            .map(|model| Entry::Cloud {
                provider: CloudProvider::OpenRouter,
                model,
                effort: None,
            })
            .collect();
        // A role nobody has edited, with auto-switch off, used the FIRST saved model only: moving
        // it onto OpenRouter must not start trying the rest (the same rule as
        // `set_openrouter_models`).
        if load(conn, role)?.derived && !db::get_bool(conn, auto_key, false)? {
            rows.truncate(1);
        }
        rows
    };
    replace_rows(conn, role, is_cloud, rows)
}

/// The older routing control, in terms of the list: `cloud` takes this computer out; `local` leaves
/// it alone; `local-then-cloud` puts it first and keeps (or restores) the cloud rows behind it.
pub(crate) fn set_routing(conn: &Connection, role: Role, pref: ProviderPref) -> Result<Edited> {
    edit_with(conn, role, false, |list| {
        let cloud: Vec<Entry> = list.iter().filter(|e| is_cloud(e)).cloned().collect();
        let cloud_back = |cloud: Vec<Entry>| -> Result<Vec<Entry>> {
            if cloud.is_empty() {
                let rows = derive_cloud(conn, role)?;
                Ok(if rows.is_empty() {
                    default_entries()
                } else {
                    rows
                })
            } else {
                Ok(cloud)
            }
        };
        Ok(Some(match pref {
            ProviderPref::Cloud => cloud_back(cloud)?,
            ProviderPref::Local => vec![Entry::Local],
            ProviderPref::LocalThenCloud => {
                let mut out = vec![Entry::Local];
                out.extend(cloud_back(cloud)?);
                out
            }
        }))
    })
}

// ---- the retire and off hooks -------------------------------------------------------------------

/// What turning a provider off (or removing its key) did to one role's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Retired {
    pub role: Role,
    /// How many rows of the provider left the list (one for a list nobody has edited: the role's
    /// provider setting).
    pub removed: usize,
    /// The list would have been empty, so OpenRouter's default came back ("moved back to
    /// OpenRouter", as the older controls said).
    pub restored_default: bool,
}

/// Remove a provider's rows from both roles' lists, putting OpenRouter's default back where that
/// would leave a role with nothing. A role nobody has edited is moved back to OpenRouter the way it
/// always was (its provider row goes). Runs inside the caller's transaction.
pub(crate) fn retire_provider(conn: &Connection, p: CloudProvider) -> Result<Vec<Retired>> {
    let mut out = Vec::new();
    for role in [Role::Chat, Role::Background] {
        match stored_entries(conn, role)? {
            Some(list) => {
                let kept: Vec<Entry> = list
                    .iter()
                    .filter(|e| !matches!(e, Entry::Cloud { provider, .. } if *provider == p))
                    .cloned()
                    .collect();
                let removed = list.len() - kept.len();
                if removed == 0 {
                    continue;
                }
                let restored_default = kept.is_empty();
                let next = if restored_default {
                    default_entries()
                } else {
                    kept
                };
                save(conn, role, next)?;
                out.push(Retired {
                    role,
                    removed,
                    restored_default,
                });
            }
            None => {
                if llm_gateway::legacy_stored_role_provider(conn, role)? == p {
                    db::delete_setting(conn, llm_gateway::cloud_provider_key(role))?;
                    out.push(Retired {
                        role,
                        removed: 1,
                        restored_default: true,
                    });
                }
            }
        }
    }
    Ok(out)
}

/// The Local AI tab was reset to its defaults: its model and routing rows are gone, and the reset
/// puts routing back to the cloud. A role with a list of its own loses this computer's row the same
/// way (and gets the cloud default if that was its only row), so no list names a model that no
/// longer exists. A role nobody has edited already reads cloud from the deleted rows.
pub(crate) fn local_reset(conn: &Connection) -> Result<()> {
    for role in [Role::Chat, Role::Background] {
        if stored_entries(conn, role)?.is_some_and(|l| l.contains(&Entry::Local)) {
            set_routing(conn, role, ProviderPref::Cloud)?;
        }
    }
    Ok(())
}

/// The roles a retire touched, in order. A test's way of asking "which roles moved".
#[cfg(test)]
pub(crate) fn roles_of(retired: &[Retired]) -> Vec<Role> {
    retired.iter().map(|r| r.role).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::parse;

    fn temp_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open(
            &dir.path().join("t.sqlite"),
            "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff",
        )
        .unwrap();
        (dir, conn)
    }

    fn or(model: &str) -> Entry {
        Entry::Cloud {
            provider: CloudProvider::OpenRouter,
            model: model.into(),
            effort: None,
        }
    }

    fn catalogue(p: CloudProvider) -> &'static direct_models::DirectModel {
        direct_models::default_for(p, Role::Chat).expect("a default row")
    }

    fn direct(p: CloudProvider) -> Entry {
        let m = catalogue(p);
        Entry::Cloud {
            provider: p,
            model: m.id.clone(),
            effort: direct_providers::default_effort(Role::Chat, m),
        }
    }

    fn stored(conn: &Connection, role: Role) -> Option<Vec<Entry>> {
        db::get_setting(conn, route_key(role))
            .unwrap()
            .and_then(|raw| parse(&raw))
    }

    #[test]
    fn the_first_edit_stores_the_whole_list_and_says_what_it_left_out() {
        let (_d, conn) = temp_db();
        db::set_setting(&conn, settings::CHAT_MODELS_KEY, r#"["a/1","b/2","c/3"]"#).unwrap();
        // Auto-switch off: only the first was ever used, so the list shows it and nothing new is spent.
        let first = add(&conn, Role::Chat, or("d/4"), None).unwrap();
        assert!(first.materialised);
        assert_eq!(first.unused_saved, 2);
        assert_eq!(first.entries, vec![or("a/1"), or("d/4")]);
        assert_eq!(stored(&conn, Role::Chat), Some(first.entries.clone()));
        // Said once: the second edit is an ordinary one.
        let second = add(&conn, Role::Chat, or("e/5"), None).unwrap();
        assert!(!second.materialised);
        assert_eq!(second.unused_saved, 0);
        // The older rows are untouched by any of it.
        assert_eq!(
            db::get_setting(&conn, settings::CHAT_MODELS_KEY)
                .unwrap()
                .as_deref(),
            Some(r#"["a/1","b/2","c/3"]"#)
        );
        // Background was never edited.
        assert_eq!(stored(&conn, Role::Background), None);
    }

    #[test]
    fn with_auto_switch_on_every_saved_model_was_in_use_so_nothing_is_left_out() {
        let (_d, conn) = temp_db();
        db::set_setting(&conn, settings::CHAT_MODELS_KEY, r#"["a/1","b/2","c/3"]"#).unwrap();
        db::set_bool(&conn, settings::CHAT_AUTO_SWITCH_KEY, true).unwrap();
        let first = add(&conn, Role::Chat, or("d/4"), None).unwrap();
        assert_eq!(first.unused_saved, 0);
        assert_eq!(
            first.entries,
            vec![or("a/1"), or("b/2"), or("c/3"), or("d/4")]
        );
    }

    /// The first-run wizard saves its models and turns auto-switch on; in EITHER order every model
    /// ends up in the list (review round 1: models first kept only the first, and the switch turned
    /// on after had nothing left to switch between).
    #[test]
    fn auto_switch_and_the_model_list_agree_in_either_order() {
        let three = ["a/1".to_string(), "b/2".to_string(), "c/3".to_string()];
        let want = vec![or("a/1"), or("b/2"), or("c/3")];
        for auto_first in [true, false] {
            let (_d, conn) = temp_db();
            if auto_first {
                set_openrouter_auto(&conn, Role::Chat, true).unwrap();
                set_openrouter_models(&conn, Role::Chat, &three).unwrap();
            } else {
                // The wizard's own old order, kept working: models, then the switch.
                set_openrouter_models(&conn, Role::Chat, &three).unwrap();
                set_openrouter_auto(&conn, Role::Chat, true).unwrap();
            }
            let got = entries_for(&conn, Role::Chat).unwrap().entries;
            if auto_first {
                assert_eq!(got, want, "switch first");
            } else {
                // Models first still keeps the one that was in use (nothing says more were wanted),
                // which is why the wizard now saves the switch first.
                assert_eq!(got, vec![or("a/1")], "models first");
            }
        }
    }

    #[test]
    fn a_control_that_changes_nothing_never_turns_a_derived_list_into_a_stored_one() {
        let (_d, conn) = temp_db();
        // Cloud routing on a role that has no this-computer row: nothing to change.
        set_routing(&conn, Role::Chat, ProviderPref::Cloud).unwrap();
        // The same single OpenRouter model again.
        set_openrouter_models(&conn, Role::Chat, &[settings::DEFAULT_MODEL.to_string()]).unwrap();
        // Auto-switch off when the list already has one row.
        set_openrouter_auto(&conn, Role::Chat, false).unwrap();
        assert_eq!(stored(&conn, Role::Chat), None, "no row was written");
        assert!(entries_for(&conn, Role::Chat).unwrap().derived);
    }

    #[test]
    fn move_remove_effort_promote_and_reset_each_edit_the_one_list() {
        let (_d, conn) = temp_db();
        let claude = direct(CloudProvider::Anthropic);
        add(&conn, Role::Chat, or("a/1"), None).unwrap();
        add(&conn, Role::Chat, or("b/2"), None).unwrap();
        add(&conn, Role::Chat, claude.clone(), None).unwrap();
        // The derived default row was the first of four; take it out so the list is these three.
        let e = remove(&conn, Role::Chat, 0).unwrap();
        assert_eq!(e.entries.len(), 3);
        // Move the last row to the top.
        let e = move_entry(&conn, Role::Chat, 2, 0).unwrap();
        assert_eq!(e.entries, vec![claude.clone(), or("a/1"), or("b/2")]);
        // Effort: a level the model offers, then back to the default.
        let model = catalogue(CloudProvider::Anthropic);
        let level = model.effort_levels.first().cloned().expect("a level");
        let e = set_effort(&conn, Role::Chat, 0, Some(level.clone())).unwrap();
        assert!(matches!(&e.entries[0], Entry::Cloud { effort: Some(l), .. } if *l == level));
        let e = set_effort(&conn, Role::Chat, 0, None).unwrap();
        assert!(matches!(&e.entries[0], Entry::Cloud { effort, .. }
            if *effort == direct_providers::default_effort(Role::Chat, model)));
        // A level the model does not offer, and the word "default", are refused; so is an effort on
        // a row that has none.
        for bad in ["warp", "default", ""] {
            assert!(
                set_effort(&conn, Role::Chat, 0, Some(bad.into())).is_err(),
                "{bad:?}"
            );
        }
        assert!(set_effort(&conn, Role::Chat, 1, Some(level.clone())).is_err());
        // Promote an existing row: it moves to the head and keeps the effort the list had for it.
        set_effort(&conn, Role::Chat, 0, Some(level.clone())).unwrap();
        move_entry(&conn, Role::Chat, 0, 2).unwrap();
        let e = promote(&conn, Role::Chat, direct(CloudProvider::Anthropic)).unwrap();
        assert_eq!(e.entries.len(), 3, "moved, not duplicated");
        assert!(
            matches!(&e.entries[0], Entry::Cloud { provider: CloudProvider::Anthropic, effort: Some(l), .. } if *l == level)
        );
        // Promote a row the list lacks: inserted at the head.
        let e = promote(&conn, Role::Chat, or("z/9")).unwrap();
        assert_eq!(e.entries[0], or("z/9"));
        assert_eq!(e.entries.len(), 4);
        // Remove, then reset to the default as an explicit stored row.
        let e = remove(&conn, Role::Chat, 0).unwrap();
        assert_eq!(e.entries.len(), 3);
        let e = reset(&conn, Role::Chat).unwrap();
        assert_eq!(e.entries, default_entries());
        assert_eq!(stored(&conn, Role::Chat), Some(default_entries()));
    }

    /// Reset puts the CLOUD rows back to the default and leaves this computer where it stands: it is
    /// the person's Local AI choice, and dropping it would send to the cloud what was going to the
    /// local model first (review round 1).
    #[test]
    fn reset_keeps_this_computer_where_it_stands() {
        let (_d, conn) = temp_db();
        add(&conn, Role::Chat, or("a/1"), None).unwrap();
        add(&conn, Role::Chat, Entry::Local, Some(0)).unwrap();
        let e = reset(&conn, Role::Chat).unwrap();
        let mut want = default_entries();
        want.insert(0, Entry::Local);
        assert_eq!(e.entries, want);
        // Below the cloud row it stays below.
        add(&conn, Role::Background, or("a/1"), None).unwrap();
        add(&conn, Role::Background, Entry::Local, None).unwrap();
        let e = reset(&conn, Role::Background).unwrap();
        let mut want = default_entries();
        want.push(Entry::Local);
        assert_eq!(e.entries, want);
    }

    /// A "Local only" list has no cloud row to reset, and Reset must not give it one: the text
    /// would go to OpenRouter the next time the local model failed (review round 2, N1).
    #[test]
    fn reset_on_a_local_only_list_adds_no_cloud_row_and_writes_nothing() {
        // A stored list that is [This computer] alone.
        let (_d, conn) = temp_db();
        add(&conn, Role::Chat, Entry::Local, None).unwrap();
        let e = remove(&conn, Role::Chat, 0).unwrap();
        assert_eq!(e.entries, vec![Entry::Local]);
        let before = db::get_setting(&conn, route_key(Role::Chat)).unwrap();
        let e = reset(&conn, Role::Chat).unwrap();
        assert_eq!(e.entries, vec![Entry::Local]);
        assert_eq!(
            db::get_setting(&conn, route_key(Role::Chat)).unwrap(),
            before
        );
        // A role that is Local only and was never edited stays unwritten (derived).
        let (_d2, conn) = temp_db();
        db::set_setting(&conn, llm_gateway::CHAT_ROUTING_KEY, "local").unwrap();
        let e = reset(&conn, Role::Chat).unwrap();
        assert_eq!(e.entries, vec![Entry::Local]);
        assert!(!e.materialised);
        assert_eq!(stored(&conn, Role::Chat), None);
    }

    /// Every write goes through the one gate: nothing a refused write asked for is stored, and the
    /// list stays exactly as it was.
    #[test]
    fn a_refused_write_leaves_the_list_untouched() {
        let (_d, conn) = temp_db();
        add(&conn, Role::Chat, or("a/1"), None).unwrap();
        add(&conn, Role::Chat, Entry::Local, None).unwrap();
        let before = stored(&conn, Role::Chat);
        let refused: Vec<(&str, Result<Edited>)> = vec![
            ("duplicate", add(&conn, Role::Chat, or("a/1"), None)),
            (
                "a second this-computer row",
                add(&conn, Role::Chat, Entry::Local, None),
            ),
            (
                "a direct model PM does not list",
                add(
                    &conn,
                    Role::Chat,
                    Entry::Cloud {
                        provider: CloudProvider::Anthropic,
                        model: "anthropic/not-a-model".into(),
                        effort: None,
                    },
                    None,
                ),
            ),
            ("an empty id", add(&conn, Role::Chat, or("  "), None)),
            ("past the end", move_entry(&conn, Role::Chat, 0, 9)),
            ("a missing row", remove(&conn, Role::Chat, 9)),
        ];
        for (what, r) in refused {
            assert!(r.is_err(), "{what} should be refused");
        }
        assert_eq!(stored(&conn, Role::Chat), before);
        // The last row cannot be taken out: a role never ends up with nothing.
        remove(&conn, Role::Chat, 2).unwrap();
        remove(&conn, Role::Chat, 0).unwrap();
        let e = remove(&conn, Role::Chat, 0).unwrap_err();
        assert!(e.to_string().contains("needs at least one"), "{e}");
        assert_eq!(stored(&conn, Role::Chat), Some(vec![or("a/1")]));
    }

    #[test]
    fn this_computer_can_be_added_once_and_sit_anywhere() {
        let (_d, conn) = temp_db();
        let e = add(&conn, Role::Chat, Entry::Local, Some(0)).unwrap();
        assert_eq!(e.entries[0], Entry::Local);
        let e = move_entry(&conn, Role::Chat, 0, 1).unwrap();
        assert_eq!(e.entries[1], Entry::Local);
    }

    #[test]
    fn turning_a_provider_off_takes_its_rows_out_of_both_lists() {
        let (_d, conn) = temp_db();
        let claude = direct(CloudProvider::Anthropic);
        add(&conn, Role::Chat, claude.clone(), Some(0)).unwrap();
        add(&conn, Role::Background, claude.clone(), Some(0)).unwrap();
        let r = retire_provider(&conn, CloudProvider::Anthropic).unwrap();
        assert_eq!(roles_of(&r), vec![Role::Chat, Role::Background]);
        assert!(r.iter().all(|x| x.removed == 1 && !x.restored_default));
        // The list had OpenRouter's default behind Claude, so it is that now.
        assert_eq!(stored(&conn, Role::Chat), Some(default_entries()));
        // Another provider's rows are not touched.
        let r = retire_provider(&conn, CloudProvider::Gemini).unwrap();
        assert!(r.is_empty());
    }

    #[test]
    fn a_list_that_would_be_empty_gets_openrouters_default_back() {
        let (_d, conn) = temp_db();
        let claude = direct(CloudProvider::Anthropic);
        // A list of only Claude.
        add(&conn, Role::Chat, claude, Some(0)).unwrap();
        remove(&conn, Role::Chat, 1).unwrap();
        assert_eq!(stored(&conn, Role::Chat).map(|l| l.len()), Some(1));
        let r = retire_provider(&conn, CloudProvider::Anthropic).unwrap();
        assert_eq!(r.len(), 1);
        assert!(r[0].restored_default);
        assert_eq!(stored(&conn, Role::Chat), Some(default_entries()));
    }

    #[test]
    fn a_role_nobody_edited_moves_back_to_openrouter_the_way_it_always_did() {
        let (_d, conn) = temp_db();
        direct_providers::set_opt_in(&conn, CloudProvider::OpenAi, true).unwrap();
        direct_providers::set_role_provider(&conn, Role::Chat, CloudProvider::OpenAi).unwrap();
        let r = direct_providers::set_opt_in(&conn, CloudProvider::OpenAi, false).unwrap();
        assert_eq!(roles_of(&r), vec![Role::Chat]);
        assert!(r[0].restored_default);
        assert_eq!(
            stored(&conn, Role::Chat),
            None,
            "no list was written for it"
        );
        assert_eq!(
            llm_gateway::role_cloud_provider(&conn, Role::Chat).unwrap(),
            CloudProvider::OpenRouter
        );
    }

    /// The Local AI tab's reset puts routing back to the cloud, for a role with a list too.
    #[test]
    fn the_local_ai_reset_takes_this_computer_out_of_a_stored_list() {
        let (_d, conn) = temp_db();
        add(&conn, Role::Chat, Entry::Local, Some(0)).unwrap();
        remove(&conn, Role::Chat, 1).unwrap();
        assert_eq!(stored(&conn, Role::Chat), Some(vec![Entry::Local]));
        local_reset(&conn).unwrap();
        assert_eq!(stored(&conn, Role::Chat), Some(default_entries()));
    }

    #[test]
    fn the_older_openrouter_list_control_never_adds_a_row_to_a_list_without_one() {
        let (_d, conn) = temp_db();
        let claude = direct(CloudProvider::Anthropic);
        add(&conn, Role::Chat, claude.clone(), Some(0)).unwrap();
        remove(&conn, Role::Chat, 1).unwrap();
        set_openrouter_models(&conn, Role::Chat, &["x/1".into(), "y/2".into()]).unwrap();
        assert_eq!(
            stored(&conn, Role::Chat),
            Some(vec![claude]),
            "saved OpenRouter models are inert while the list has no OpenRouter row"
        );
    }

    /// The larger-model switch moves ONE OpenRouter row to where the first OpenRouter row stands and
    /// leaves every other row where the person put it (delivery audit S1: writing the whole
    /// OpenRouter set pulled B up into the first request, ahead of Claude).
    #[test]
    fn the_larger_model_switch_leaves_every_other_row_where_it_was() {
        let (_d, conn) = temp_db();
        let claude = direct(CloudProvider::Anthropic);
        save(
            &conn,
            Role::Chat,
            vec![or("a/1"), claude.clone(), or("b/2")],
        )
        .unwrap();
        let e = promote_openrouter(&conn, Role::Chat, "big/9").unwrap();
        assert_eq!(
            e.entries,
            vec![or("big/9"), or("a/1"), claude.clone(), or("b/2")]
        );
        // A model the list already holds, further down, moves up to the first OpenRouter row.
        let e = promote_openrouter(&conn, Role::Chat, "b/2").unwrap();
        assert_eq!(e.entries, vec![or("b/2"), or("big/9"), or("a/1"), claude]);
        // Already first, or blank: nothing changes.
        let again = promote_openrouter(&conn, Role::Chat, "b/2").unwrap();
        assert_eq!(again.entries, e.entries);
        assert_eq!(
            promote_openrouter(&conn, Role::Chat, "  ").unwrap().entries,
            e.entries
        );
    }

    /// A list with no OpenRouter row gets none from the switch, and a list nobody edited keeps the
    /// older meaning: the larger model first, only the first in use with auto-switch off.
    #[test]
    fn the_larger_model_switch_never_adds_a_company_and_keeps_the_unedited_meaning() {
        let (_d, conn) = temp_db();
        let claude = direct(CloudProvider::Anthropic);
        save(&conn, Role::Chat, vec![claude.clone()]).unwrap();
        let e = promote_openrouter(&conn, Role::Chat, "big/9").unwrap();
        assert_eq!(e.entries, vec![claude]);

        let (_d, conn) = temp_db();
        db::set_setting(&conn, settings::CHAT_MODELS_KEY, r#"["a/1","b/2","c/3"]"#).unwrap();
        let e = promote_openrouter(&conn, Role::Chat, "big/9").unwrap();
        assert!(e.materialised);
        assert_eq!(e.entries, vec![or("big/9")]);
        assert_eq!(e.unused_saved, 2);
    }

    /// The first-run save turns auto-switch on for a role whose only cloud row is a direct one: it
    /// must not add OpenRouter rows (delivery audit N1), the sibling of the models control's rule.
    #[test]
    fn the_auto_switch_never_adds_openrouter_rows_to_a_role_that_is_not_on_openrouter() {
        let (_d, conn) = temp_db();
        direct_providers::set_opt_in(&conn, CloudProvider::Anthropic, true).unwrap();
        direct_providers::set_role_provider(&conn, Role::Chat, CloudProvider::Anthropic).unwrap();
        db::set_setting(&conn, settings::CHAT_MODELS_KEY, r#"["a/1","b/2"]"#).unwrap();
        let before = entries_for(&conn, Role::Chat).unwrap().entries;
        let e = set_openrouter_auto(&conn, Role::Chat, true).unwrap();
        assert!(!e.materialised);
        assert_eq!(e.entries, before);
        assert_eq!(
            stored(&conn, Role::Chat),
            None,
            "the role is still the list nobody edited"
        );
    }

    /// Moving a role nobody has edited onto OpenRouter used the first saved model only while
    /// auto-switch was off (delivery audit N2); with it on, every saved model was in use.
    #[test]
    fn the_older_provider_control_moves_a_derived_role_to_openrouter_with_the_models_that_were_in_use(
    ) {
        for (auto, want) in [(false, vec![or("a/1")]), (true, vec![or("a/1"), or("b/2")])] {
            let (_d, conn) = temp_db();
            direct_providers::set_opt_in(&conn, CloudProvider::Anthropic, true).unwrap();
            direct_providers::set_role_provider(&conn, Role::Chat, CloudProvider::Anthropic)
                .unwrap();
            db::set_setting(&conn, settings::CHAT_MODELS_KEY, r#"["a/1","b/2"]"#).unwrap();
            db::set_bool(&conn, settings::CHAT_AUTO_SWITCH_KEY, auto).unwrap();
            let e = use_provider(&conn, Role::Chat, CloudProvider::OpenRouter).unwrap();
            assert_eq!(e.entries, want, "auto-switch {auto}");
        }
    }

    #[test]
    fn the_older_provider_control_swaps_the_cloud_rows_and_keeps_this_computer_in_place() {
        let (_d, conn) = temp_db();
        add(&conn, Role::Chat, Entry::Local, Some(0)).unwrap();
        let e = use_provider(&conn, Role::Chat, CloudProvider::Anthropic).unwrap();
        assert_eq!(e.entries.len(), 2);
        assert_eq!(e.entries[0], Entry::Local);
        assert!(matches!(
            &e.entries[1],
            Entry::Cloud {
                provider: CloudProvider::Anthropic,
                ..
            }
        ));
        // And back to OpenRouter: the models the person last saved (the default here).
        let e = use_provider(&conn, Role::Chat, CloudProvider::OpenRouter).unwrap();
        assert_eq!(e.entries, vec![Entry::Local, or(settings::DEFAULT_MODEL)]);
        // The older routing words, read from the list.
        assert_eq!(
            llm_gateway::pref_of_list(&e.entries),
            ProviderPref::LocalThenCloud
        );
        let e = set_routing(&conn, Role::Chat, ProviderPref::Local).unwrap();
        assert_eq!(e.entries, vec![Entry::Local]);
        let e = set_routing(&conn, Role::Chat, ProviderPref::LocalThenCloud).unwrap();
        assert_eq!(e.entries[0], Entry::Local);
        assert!(e.entries.len() >= 2, "the cloud rows come back behind it");
        let e = set_routing(&conn, Role::Chat, ProviderPref::Cloud).unwrap();
        assert!(!e.entries.contains(&Entry::Local));
    }

    #[test]
    fn a_role_with_a_list_reads_its_routing_and_provider_from_it() {
        let (_d, conn) = temp_db();
        // The older rows say local only on Anthropic...
        db::set_setting(&conn, llm_gateway::CHAT_ROUTING_KEY, "local").unwrap();
        db::set_setting(&conn, llm_gateway::CHAT_CLOUD_PROVIDER_KEY, "anthropic").unwrap();
        assert_eq!(
            llm_gateway::routing_prefs(&conn)
                .unwrap()
                .for_role(Role::Chat),
            ProviderPref::Local
        );
        // ...until the person writes a list: then the list is the truth and the rows are not read.
        add(&conn, Role::Chat, or("a/1"), Some(0)).unwrap();
        assert_eq!(
            llm_gateway::routing_prefs(&conn)
                .unwrap()
                .for_role(Role::Chat),
            ProviderPref::LocalThenCloud
        );
        assert_eq!(
            llm_gateway::role_cloud_provider(&conn, Role::Chat).unwrap(),
            CloudProvider::OpenRouter
        );
        assert_eq!(
            llm_gateway::routing_word(&conn, Role::Background).unwrap(),
            "cloud",
            "the other role is still read from the older rows"
        );
    }

    /// A list edit forgets what PM remembered about rows not working: the one line that does it is
    /// in the one write.
    #[test]
    fn every_write_forgets_what_was_remembered_about_rows_failing() {
        let live = include_str!("edit.rs")
            .split("#[cfg(test)]\nmod tests {")
            .next()
            .unwrap();
        let body = live
            .split("fn save(conn: &Connection")
            .nth(1)
            .unwrap()
            .split("\n}\n")
            .next()
            .unwrap();
        assert!(body.contains("entry_health::global().clear_all()"));
        assert!(body.contains("normalise(role, entries)"));
        // And no other function here stores a list: `save` is the only writer of the row.
        assert_eq!(live.matches("set_setting(conn, route_key(").count(), 1);
    }

    /// THE SCAN: the older routing rows (the OpenRouter lists, auto-switch, a role's provider, a
    /// direct provider's models and effort, the Local AI routing) are written by `route.rs` and
    /// `route/edit.rs` and nothing else in the live code. A command that wrote one would be a second
    /// truth beside the list. Test code is exempt (it sets up a role nobody has edited).
    #[test]
    fn the_older_routing_rows_are_written_only_by_the_list_module() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk(&src, &mut files);
        let tokens = [
            "CHAT_MODELS_KEY",
            "BACKGROUND_MODELS_KEY",
            "AUTO_SWITCH_KEY",
            "cloud_provider_key",
            "CLOUD_PROVIDER_KEY",
            "CHAT_ROUTING_KEY",
            "BACKGROUND_ROUTING_KEY",
            "routing_key",
            "keys.models",
            "keys.effort",
            "\"chat_models\"",
            "\"background_models\"",
            "_routing\"",
        ];
        let writes = ["set_setting(", "delete_setting(", "set_bool("];
        let mut offenders = Vec::new();
        for path in files {
            let rel = path
                .strip_prefix(&src)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            // The list module itself, and test files, are the exemptions.
            if rel == "route.rs" || rel == "route/edit.rs" || rel.ends_with("_tests.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let live = text.split("#[cfg(test)]\nmod tests {").next().unwrap();
            for w in writes {
                for (at, _) in live.match_indices(w) {
                    let window: String = live[at..].chars().take(160).collect();
                    if !tokens.iter().any(|t| window.contains(t)) {
                        continue;
                    }
                    // A helper that exists only for tests (`#[cfg(test)] fn ...`): its item, back
                    // to the previous blank line, carries the attribute.
                    let item_start = live[..at].rfind("\n\n").unwrap_or(0);
                    if live[item_start..at].contains("#[cfg(test)]") {
                        continue;
                    }
                    offenders.push(format!("{rel}: {}", window.lines().next().unwrap_or("")));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "an older routing row is written outside the list module:\n{}",
            offenders.join("\n")
        );
    }
}
