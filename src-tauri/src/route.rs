// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The ordered list of models a role tries, top to bottom: one list per role, across every provider
//! and this computer (Bobby, 2026-11-27: "that option list of preference and fallback isnt just for
//! open router but all the models including local").
//!
//! This module is the TYPE and the READ. A list is a row in `settings` (`role_route_chat`,
//! `role_route_background`), device class, written only by Rust commands (a later slice owns the
//! writers). A role with no row, or a row PM cannot make sense of, is DERIVED from the rows PM has
//! always kept (its routing, its provider, its model list, its auto-switch, a direct provider's one
//! model and its effort), so a person who never edits a list resolves, byte for byte, as before and
//! nothing is written by a read (rule R4). The first edit writes the whole list.
//!
//! Ids are `{provider, model}` pairs, never a bare string whose company is guessed from a prefix: a
//! direct id and an OpenRouter id can look alike, and calling that luck is how a reply goes to the
//! wrong company. [`Entry::Local`] is a POINTER, "this role's local model": the Local AI tab stays
//! the one home of the server and the model, and the list owns only where local sits.
//!
//! [`normalise`] is the one gate on every list, run on every write AND every read, because a restored
//! backup can carry anything. It never trusts a stored row: a row that fails a limit is dropped from
//! the list (and reported), never repaired into some other company's model.
//!
//! Nothing routes from this yet; the readers arrive with the ladder.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::direct_models::{self, names_mythos, DirectModel};
use crate::direct_providers;
use crate::error::{Error, Result};
use crate::llm_gateway::{self, CloudProvider, ProviderPref, Role};
use crate::{db, settings};

/// Every way a list is written (add, move, remove, effort, promote, reset, and the retire and off
/// hooks). This file is the type and the read; that one is the writer.
pub(crate) mod edit;

/// A list never holds more than this many rows: the old OpenRouter cap, so deriving from that list
/// loses nothing.
pub(crate) const MAX_ENTRIES: usize = 50;

/// The longest model id a list keeps (the OpenRouter cap, `settings::save_models`).
pub(crate) const MAX_MODEL_ID_CHARS: usize = 200;

/// The stored shape's version. A row with any other number is not read (it falls back to derive), so
/// a later shape can never be half-understood by this one.
const STORED_VERSION: u32 = 1;

pub(crate) const CHAT_ROUTE_KEY: &str = "role_route_chat";
pub(crate) const BACKGROUND_ROUTE_KEY: &str = "role_route_background";

/// The role's list row. Exhaustive, so a third role fails to compile here.
pub(crate) fn route_key(role: Role) -> &'static str {
    match role {
        Role::Chat => CHAT_ROUTE_KEY,
        Role::Background => BACKGROUND_ROUTE_KEY,
    }
}

/// One row of a role's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Entry {
    /// This role's model on this computer (a pointer: the model itself lives in the Local AI rows).
    Local,
    /// One model of one provider. `effort` is a direct provider's level for this row (`None` omits the
    /// field); it is always `None` for OpenRouter, whose effort is the model's own.
    Cloud {
        provider: CloudProvider,
        model: String,
        effort: Option<String>,
    },
}

impl Entry {
    /// Two rows name the same thing when they are both Local, or the same `(provider, model)`: the
    /// effort is a setting of the row, not part of what it is.
    fn same_slot(&self, other: &Entry) -> bool {
        match (self, other) {
            (Entry::Local, Entry::Local) => true,
            (
                Entry::Cloud {
                    provider: p,
                    model: m,
                    ..
                },
                Entry::Cloud {
                    provider: q,
                    model: n,
                    ..
                },
            ) => p == q && m == n,
            _ => false,
        }
    }
}

/// A role's list as PM reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RoleRoute {
    pub entries: Vec<Entry>,
    /// True until the first edit: the list was worked out from the older rows and nothing is stored.
    pub derived: bool,
}

// ---- the stored shape ----------------------------------------------------------------------------

/// `{"v":1,"entries":[{"k":"local"},{"k":"cloud","p":"anthropic","m":"anthropic/claude-sonnet-5-5","e":"medium"}]}`.
/// Strict on purpose: any field, kind or provider PM does not know makes the WHOLE row unreadable
/// (and so derived), because a list read with some rows quietly missing is a different order of
/// preference than the person wrote.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    v: u32,
    entries: Vec<StoredEntry>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "lowercase", deny_unknown_fields)]
enum StoredEntry {
    // A struct variant with no fields, not a unit variant: serde does not apply
    // `deny_unknown_fields` to a unit variant, and `{"k":"local","m":"x"}` must stay unreadable.
    Local {},
    Cloud {
        p: String,
        m: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        e: Option<String>,
    },
}

/// The provider a stored token names, by an ALLOW-LIST with no default: unlike
/// [`CloudProvider::from_setting`], which turns junk into OpenRouter for the old single-provider row,
/// a list row that names no provider PM knows is unreadable, never "OpenRouter".
fn provider_from_token(token: &str) -> Option<CloudProvider> {
    [
        CloudProvider::OpenRouter,
        CloudProvider::OpenAi,
        CloudProvider::Gemini,
        CloudProvider::Anthropic,
    ]
    .into_iter()
    .find(|p| p.as_str() == token)
}

/// Read a stored row. `None` for anything that is not exactly the version-1 shape.
pub(crate) fn parse(raw: &str) -> Option<Vec<Entry>> {
    let stored: Stored = serde_json::from_str(raw).ok()?;
    if stored.v != STORED_VERSION {
        return None;
    }
    stored
        .entries
        .into_iter()
        .map(|e| match e {
            StoredEntry::Local {} => Some(Entry::Local),
            StoredEntry::Cloud { p, m, e } => Some(Entry::Cloud {
                provider: provider_from_token(&p)?,
                model: m,
                effort: e,
            }),
        })
        .collect()
}

/// The row to store for a list. Does not normalise: the writer does that first and stores what it
/// returns.
pub(crate) fn to_json(entries: &[Entry]) -> Result<String> {
    let stored = Stored {
        v: STORED_VERSION,
        entries: entries
            .iter()
            .map(|e| match e {
                Entry::Local => StoredEntry::Local {},
                Entry::Cloud {
                    provider,
                    model,
                    effort,
                } => StoredEntry::Cloud {
                    p: provider.as_str().to_string(),
                    m: model.clone(),
                    e: effort.clone(),
                },
            })
            .collect(),
    };
    serde_json::to_string(&stored).map_err(|e| Error::Other(e.to_string()))
}

// ---- normalise ------------------------------------------------------------------------------------

/// Why a row did not make it into the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DropReason {
    /// The model id is empty once trimmed.
    Empty,
    /// The model id is longer than [`MAX_MODEL_ID_CHARS`].
    TooLong,
    /// A direct provider's model that is not a row of PM's catalogue for that provider.
    NotListed,
    /// A catalogue row whose announced shutdown date has come.
    Retired,
    /// Anthropic's Mythos line, which PM never offers, saves or calls.
    Mythos,
    /// A second row for this computer: it has one server and one model per role.
    SecondLocal,
    /// The same `(provider, model)` as an earlier row.
    Duplicate,
    /// Past [`MAX_ENTRIES`].
    TooMany,
}

/// A row [`normalise`] removed, with where it stood in what it was given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Dropped {
    pub index: usize,
    pub entry: Entry,
    pub why: DropReason,
}

/// A list that has been through every limit, and what did not survive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Normalised {
    pub entries: Vec<Entry>,
    pub dropped: Vec<Dropped>,
}

/// Whether a catalogue row names Mythos in any field a person or a request could see: the id, the
/// id the provider's API knows it by, and its display name. The catalogue has no such row today; this
/// is the layer that still refuses one if a later edit adds it (the same three fields
/// `anthropic_direct::requested_model` checks before anything is sent).
fn row_names_mythos(m: &DirectModel) -> bool {
    names_mythos(&m.id) || names_mythos(&m.wire) || names_mythos(&m.name)
}

/// Why a catalogue row cannot be a list entry on `today`, whatever the entry says: it names Mythos,
/// or its announced shutdown has come (the same rule `direct_providers::models_for` applies when it
/// drops a stored id). A function of the row alone, so a test can hand it a row the committed
/// catalogue does not have.
fn row_refusal(row: &DirectModel, today: &str) -> std::result::Result<(), DropReason> {
    if row_names_mythos(row) {
        return Err(DropReason::Mythos);
    }
    if row.retired(today) {
        return Err(DropReason::Retired);
    }
    Ok(())
}

/// Apply every limit to a list, in order, keeping the first of anything repeated:
///
/// - at most [`MAX_ENTRIES`] rows, at most one [`Entry::Local`] ANYWHERE, no repeated
///   `(provider, model)`;
/// - a direct provider's model must be a row of its catalogue that has not retired, and never names
///   Mythos (checked on the id BEFORE the catalogue, so even a future row is refused);
/// - a direct row's effort is kept only when its model offers that level, else it becomes the role's
///   default ([`direct_providers::default_effort`]); an OpenRouter row has none;
/// - an id is trimmed, non-empty and at most [`MAX_MODEL_ID_CHARS`].
///
/// PURE (the catalogue is a constant). Idempotent: normalising its own output changes nothing.
pub(crate) fn normalise(role: Role, entries: Vec<Entry>) -> Normalised {
    normalise_on(role, entries, &direct_models::utc_today())
}

/// [`normalise`] on a given day (a UTC date), so a test never reads the clock.
pub(crate) fn normalise_on(role: Role, entries: Vec<Entry>, today: &str) -> Normalised {
    let mut kept: Vec<Entry> = Vec::new();
    let mut dropped: Vec<Dropped> = Vec::new();
    for (index, entry) in entries.into_iter().enumerate() {
        match check(role, &entry, today) {
            Err(why) => dropped.push(Dropped { index, entry, why }),
            Ok(clean) => {
                let why = if kept.iter().any(|k| k.same_slot(&clean)) {
                    Some(if clean == Entry::Local {
                        DropReason::SecondLocal
                    } else {
                        DropReason::Duplicate
                    })
                } else if kept.len() >= MAX_ENTRIES {
                    Some(DropReason::TooMany)
                } else {
                    None
                };
                match why {
                    Some(why) => dropped.push(Dropped { index, entry, why }),
                    None => kept.push(clean),
                }
            }
        }
    }
    Normalised {
        entries: kept,
        dropped,
    }
}

/// One row against the limits that need no other row: the cleaned row, or why it is refused.
fn check(role: Role, entry: &Entry, today: &str) -> std::result::Result<Entry, DropReason> {
    let Entry::Cloud {
        provider,
        model,
        effort,
    } = entry
    else {
        return Ok(Entry::Local);
    };
    let id = model.trim();
    if id.is_empty() {
        return Err(DropReason::Empty);
    }
    if id.chars().count() > MAX_MODEL_ID_CHARS {
        return Err(DropReason::TooLong);
    }
    if !provider.is_direct() {
        return Ok(Entry::Cloud {
            provider: *provider,
            model: id.to_string(),
            effort: None,
        });
    }
    // The id's own name first: Mythos is refused even if some catalogue row were to carry it.
    if names_mythos(id) {
        return Err(DropReason::Mythos);
    }
    let row = direct_models::models(*provider)
        .iter()
        .find(|m| m.id == id)
        .ok_or(DropReason::NotListed)?;
    row_refusal(row, today)?;
    let level = effort
        .as_deref()
        .map(str::trim)
        .filter(|l| row.offers(l))
        .map(str::to_string)
        .or_else(|| direct_providers::default_effort(role, row));
    Ok(Entry::Cloud {
        provider: *provider,
        model: row.id.clone(),
        effort: level,
    })
}

// ---- the read: stored, else derived --------------------------------------------------------------

/// The role's list: the stored one when it parses and keeps at least one row after [`normalise`],
/// else the one DERIVED from today's rows. A stored list that normalises to nothing is treated like
/// an unreadable one (a role is never empty by accident, and the derived rows are the configuration
/// the person last set with the older controls). Never writes.
pub(crate) fn entries_for(conn: &Connection, role: Role) -> Result<RoleRoute> {
    if let Some(entries) = stored_entries(conn, role)? {
        return Ok(RoleRoute {
            entries,
            derived: false,
        });
    }
    Ok(RoleRoute {
        entries: derive(conn, role)?,
        derived: true,
    })
}

/// The person's own list, when there is one PM can read: the stored row, parsed and put through
/// [`normalise`], with at least one row left. `None` means "nobody has edited this role's list" (no
/// row, an unreadable row, or one that normalises to nothing). This is the only place that decides
/// derived-or-stored, so the readers that must not derive (the provider shown for a role, the
/// routing the Local AI tab reads) agree with [`entries_for`] by construction. Never writes.
pub(crate) fn stored_entries(conn: &Connection, role: Role) -> Result<Option<Vec<Entry>>> {
    let Some(raw) = db::get_setting(conn, route_key(role))? else {
        return Ok(None);
    };
    let Some(parsed) = parse(&raw) else {
        return Ok(None);
    };
    let clean = normalise(role, parsed);
    Ok((!clean.entries.is_empty()).then_some(clean.entries))
}

/// The list the older rows describe, which is exactly what the old code used, in the same order:
///
/// - routing `cloud` (or absent, or unknown): the cloud rows only;
/// - `local`: this computer only;
/// - `local-then-cloud`: this computer, then the cloud rows, except in the "pick a model" state
///   switching Local AI off left (`Emptied`), where the old code answered as `local` and so does this;
/// - the cloud rows are the role's EFFECTIVE provider (a direct provider that is not opted in is
///   demoted to OpenRouter, as [`llm_gateway::role_cloud_provider`] does): for OpenRouter, the whole
///   saved model list when auto-switch is on and only its first id when it is off; for a direct
///   provider, its one model and its effort.
///
/// Local AI being OFF changes nothing here: the old code stopped such a role at the gate, not in its
/// plan, and the list's own rule for a Local row while Local AI is off is the same stop. Passed
/// through [`normalise`] so every list, derived or stored, keeps the same limits.
pub(crate) fn derive(conn: &Connection, role: Role) -> Result<Vec<Entry>> {
    // The OLD rows, never the list-aware readers: this is what a list nobody has edited is made of.
    let prefs = llm_gateway::legacy_routing_prefs(conn)?;
    let served = prefs.served_pref(role);
    let mut entries = Vec::new();
    if matches!(served, ProviderPref::Local | ProviderPref::LocalThenCloud) {
        entries.push(Entry::Local);
    }
    if served != ProviderPref::Local {
        entries.extend(derive_cloud(conn, role)?);
    }
    Ok(normalise(role, entries).entries)
}

/// The cloud rows the OLD rows describe for a role, whatever its routing says. `derive` uses it, and
/// so does the writer that has to put cloud rows back after a role was local only.
pub(crate) fn derive_cloud(conn: &Connection, role: Role) -> Result<Vec<Entry>> {
    let provider = llm_gateway::legacy_role_cloud_provider(conn, role)?;
    if provider.is_direct() {
        // A direct arm holds one model; `models_for` is never empty.
        let model = direct_providers::models_for(conn, provider, role)?
            .into_iter()
            .next()
            .ok_or_else(|| Error::Other("PM has no model for that provider.".into()))?;
        let effort = direct_providers::effort_for(conn, provider, role)?;
        return Ok(vec![Entry::Cloud {
            provider,
            model,
            effort,
        }]);
    }
    let (models_key, auto_key) = llm_gateway::openrouter_keys(role);
    Ok(settings::effective_models(conn, models_key, auto_key)?
        .into_iter()
        .map(|model| Entry::Cloud {
            provider: CloudProvider::OpenRouter,
            model,
            effort: None,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_gateway::{ProviderChoice, RuntimeContext};

    const TODAY: &str = "2026-11-28";

    fn temp_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sqlite");
        let key = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let conn = db::open(&path, key).unwrap();
        (dir, conn)
    }

    fn or(model: &str) -> Entry {
        Entry::Cloud {
            provider: CloudProvider::OpenRouter,
            model: model.into(),
            effort: None,
        }
    }

    fn direct(p: CloudProvider, model: &str, effort: Option<&str>) -> Entry {
        Entry::Cloud {
            provider: p,
            model: model.into(),
            effort: effort.map(str::to_string),
        }
    }

    /// A catalogue row of `p` that is not retired, optionally not the role's default.
    fn row_of(p: CloudProvider) -> &'static DirectModel {
        direct_models::models(p)
            .iter()
            .find(|m| !m.retired(TODAY) && !m.effort_levels.is_empty())
            .expect("a live row with levels")
    }

    // ---- the stored shape ----

    #[test]
    fn the_stored_shape_is_the_one_the_brief_names_and_round_trips() {
        let m = row_of(CloudProvider::Anthropic);
        let list = vec![
            Entry::Local,
            direct(CloudProvider::Anthropic, &m.id, Some(&m.effort_levels[0])),
            or("vendor/model-a"),
        ];
        let json = to_json(&list).unwrap();
        assert_eq!(
            json,
            format!(
                r#"{{"v":1,"entries":[{{"k":"local"}},{{"k":"cloud","p":"anthropic","m":"{}","e":"{}"}},{{"k":"cloud","p":"openrouter","m":"vendor/model-a"}}]}}"#,
                m.id, m.effort_levels[0]
            )
        );
        assert_eq!(parse(&json), Some(list));
    }

    #[test]
    fn a_row_that_is_not_exactly_the_version_one_shape_is_unreadable() {
        for bad in [
            "",
            "null",
            "[]",
            "junk",
            r#"{"entries":[]}"#,
            r#"{"v":2,"entries":[]}"#,
            r#"{"v":0,"entries":[]}"#,
            r#"{"v":1}"#,
            r#"{"v":1,"entries":"x"}"#,
            r#"{"v":1,"entries":[{"k":"remote"}]}"#,
            r#"{"v":1,"entries":[{"k":"cloud","p":"openrouter"}]}"#,
            // An unknown provider is NOT OpenRouter, whatever the old single-provider row would say.
            r#"{"v":1,"entries":[{"k":"cloud","p":"mistral","m":"a/b"}]}"#,
            r#"{"v":1,"entries":[{"k":"cloud","p":"OpenAI","m":"a/b"}]}"#,
            r#"{"v":1,"entries":[{"k":"cloud","p":"","m":"a/b"}]}"#,
            r#"{"v":1,"entries":[{"k":"cloud","p":"openrouter","m":"a/b","x":1}]}"#,
            r#"{"v":1,"entries":[{"k":"local","m":"a/b"}]}"#,
            r#"{"v":1,"entries":[],"extra":true}"#,
            // One bad row spoils the row: a list read with a row missing is another order.
            r#"{"v":1,"entries":[{"k":"local"},{"k":"cloud","p":"nope","m":"a/b"}]}"#,
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
        assert_eq!(parse(r#"{"v":1,"entries":[]}"#), Some(vec![]));
    }

    // ---- normalise ----

    #[test]
    fn at_most_fifty_rows_and_the_extra_ones_are_said() {
        let list: Vec<Entry> = (0..60).map(|i| or(&format!("v/m-{i}"))).collect();
        let n = normalise_on(Role::Chat, list, TODAY);
        assert_eq!(n.entries.len(), MAX_ENTRIES);
        assert_eq!(
            n.entries[49],
            or("v/m-49"),
            "the FIRST fifty stay, in order"
        );
        assert_eq!(n.dropped.len(), 10);
        assert!(n.dropped.iter().all(|d| d.why == DropReason::TooMany));
        assert_eq!(n.dropped[0].index, 50);
        // Exactly fifty is untouched.
        let fifty: Vec<Entry> = (0..50).map(|i| or(&format!("v/m-{i}"))).collect();
        let n = normalise_on(Role::Chat, fifty.clone(), TODAY);
        assert_eq!((n.entries, n.dropped), (fifty, vec![]));
    }

    #[test]
    fn a_dropped_row_does_not_use_up_one_of_the_fifty() {
        let mut list: Vec<Entry> = vec![or("   "); 5];
        list.extend((0..50).map(|i| or(&format!("v/m-{i}"))));
        let n = normalise_on(Role::Chat, list, TODAY);
        assert_eq!(n.entries.len(), 50);
        assert_eq!(n.dropped.len(), 5);
        assert!(n.dropped.iter().all(|d| d.why == DropReason::Empty));
    }

    #[test]
    fn this_computer_can_sit_anywhere_but_only_once() {
        for at in [0, 1, 2] {
            let mut list = vec![or("v/a"), or("v/b")];
            list.insert(at, Entry::Local);
            let n = normalise_on(Role::Chat, list.clone(), TODAY);
            assert_eq!(n.entries, list, "Local at {at} is kept where it stands");
            assert!(n.dropped.is_empty());
        }
        let n = normalise_on(
            Role::Chat,
            vec![or("v/a"), Entry::Local, or("v/b"), Entry::Local],
            TODAY,
        );
        assert_eq!(n.entries, vec![or("v/a"), Entry::Local, or("v/b")]);
        assert_eq!(
            n.dropped,
            vec![Dropped {
                index: 3,
                entry: Entry::Local,
                why: DropReason::SecondLocal
            }]
        );
    }

    #[test]
    fn a_repeated_provider_and_model_is_dropped_but_the_same_id_on_two_companies_is_not() {
        let m = row_of(CloudProvider::OpenAi);
        let list = vec![
            or("openai/x"),
            or("openai/x"),
            or(" openai/x "),
            // The same-looking id on a DIFFERENT company is a different row.
            direct(CloudProvider::OpenAi, &m.id, None),
            direct(CloudProvider::OpenAi, &m.id, Some(&m.effort_levels[0])),
            or(&m.id),
        ];
        let n = normalise_on(Role::Chat, list, TODAY);
        assert_eq!(n.entries.len(), 3, "{:?}", n.entries);
        assert_eq!(n.entries[0], or("openai/x"));
        assert_eq!(n.entries[2], or(&m.id));
        assert_eq!(
            n.dropped
                .iter()
                .map(|d| (d.index, d.why))
                .collect::<Vec<_>>(),
            vec![
                (1, DropReason::Duplicate),
                (2, DropReason::Duplicate),
                (4, DropReason::Duplicate)
            ]
        );
    }

    #[test]
    fn an_id_is_trimmed_not_empty_and_not_over_two_hundred_characters() {
        let long = "x".repeat(MAX_MODEL_ID_CHARS + 1);
        let just = "y".repeat(MAX_MODEL_ID_CHARS);
        let n = normalise_on(
            Role::Chat,
            vec![or("  v/a  "), or(""), or("  "), or(&long), or(&just)],
            TODAY,
        );
        assert_eq!(n.entries, vec![or("v/a"), or(&just)]);
        assert_eq!(
            n.dropped.iter().map(|d| d.why).collect::<Vec<_>>(),
            vec![DropReason::Empty, DropReason::Empty, DropReason::TooLong]
        );
    }

    #[test]
    fn a_direct_model_must_be_a_catalogue_row_of_that_provider() {
        let a = row_of(CloudProvider::Anthropic);
        let g = row_of(CloudProvider::Gemini);
        let n = normalise_on(
            Role::Chat,
            vec![
                direct(CloudProvider::Anthropic, &a.id, None),
                // Made up, another company's row, and an OpenRouter-style id that is no row at all.
                direct(CloudProvider::Anthropic, "anthropic/not-a-model", None),
                direct(CloudProvider::Anthropic, &g.id, None),
                direct(CloudProvider::Gemini, "", None),
            ],
            TODAY,
        );
        assert_eq!(n.entries.len(), 1);
        assert_eq!(
            n.dropped.iter().map(|d| d.why).collect::<Vec<_>>(),
            vec![
                DropReason::NotListed,
                DropReason::NotListed,
                DropReason::Empty
            ]
        );
        // OpenRouter takes any id (it has a live catalogue PM does not bundle).
        let n = normalise_on(Role::Chat, vec![or("anything/at-all")], TODAY);
        assert_eq!(n.entries, vec![or("anything/at-all")]);
    }

    #[test]
    fn a_retired_catalogue_row_is_dropped_on_its_day() {
        // No committed row announces a shutdown today, so the rule is proved on a row that does.
        let mut row = row_of(CloudProvider::Gemini).clone();
        assert_eq!(row_refusal(&row, TODAY), Ok(()));
        row.retires_on = Some("2026-12-01".into());
        assert_eq!(row_refusal(&row, "2026-11-30"), Ok(()));
        assert_eq!(row_refusal(&row, "2026-12-01"), Err(DropReason::Retired));
        assert_eq!(row_refusal(&row, "2027-01-01"), Err(DropReason::Retired));
        // The rule the list shares with `models_for` is the catalogue's own `retired`.
        assert!(row.retired("2026-12-01"));
        // And no committed row is refused by `normalise`, so a list of the whole catalogue survives.
        for s in &direct_models::catalogue().providers {
            let p = provider_from_token(&s.provider).unwrap();
            let list: Vec<Entry> = s.models.iter().map(|m| direct(p, &m.id, None)).collect();
            let n = normalise_on(Role::Chat, list, TODAY);
            assert_eq!(n.dropped, vec![], "{}", s.provider);
        }
    }

    #[test]
    fn mythos_is_refused_by_the_id_the_row_and_the_catalogue() {
        // Layer 1, the id's own name: refused before the catalogue is asked, so it holds even if a
        // row were ever added. (Case-insensitive, on any provider's direct entry.)
        for id in [
            "anthropic/claude-mythos-5-2",
            "Anthropic/Claude-MYTHOS-5",
            "openai/mythos",
        ] {
            for p in [
                CloudProvider::Anthropic,
                CloudProvider::OpenAi,
                CloudProvider::Gemini,
            ] {
                let n = normalise_on(Role::Chat, vec![direct(p, id, None)], TODAY);
                assert!(n.entries.is_empty(), "{id} on {p:?}");
                assert_eq!(n.dropped[0].why, DropReason::Mythos);
            }
        }
        // Layer 2, the row: a catalogue row whose wire id or display name says Mythos is refused
        // even when its id does not.
        let mut row = row_of(CloudProvider::Anthropic).clone();
        assert_eq!(row_refusal(&row, TODAY), Ok(()));
        row.wire = "claude-mythos-9".into();
        assert_eq!(row_refusal(&row, TODAY), Err(DropReason::Mythos));
        let mut row = row_of(CloudProvider::Anthropic).clone();
        row.name = "Claude Mythos 9".into();
        assert_eq!(row_refusal(&row, TODAY), Err(DropReason::Mythos));
        let mut row = row_of(CloudProvider::Anthropic).clone();
        row.id = "anthropic/claude-mythos-9".into();
        assert_eq!(row_refusal(&row, TODAY), Err(DropReason::Mythos));
        // Layer 3, the catalogue: it holds no such row, and no row of it trips the check.
        for s in &direct_models::catalogue().providers {
            for m in &s.models {
                assert!(!row_names_mythos(m), "{} names Mythos", m.id);
            }
        }
    }

    #[test]
    fn a_direct_rows_effort_is_kept_only_if_its_model_offers_it_else_the_roles_default() {
        let m = row_of(CloudProvider::Anthropic);
        let p = CloudProvider::Anthropic;
        let offered = m.effort_levels.last().unwrap().as_str();
        let chat_default = None;
        let bg_default = direct_providers::default_effort(Role::Background, m);
        let padded = format!(" {offered} ");
        assert!(bg_default.is_some());
        for (role, given, want) in [
            (Role::Chat, Some(offered), Some(offered.to_string())),
            (Role::Background, Some(offered), Some(offered.to_string())),
            // Not a level of that model, empty, "default" (never a level) and no level at all.
            (Role::Chat, Some("ludicrous"), chat_default.clone()),
            (Role::Chat, Some(""), chat_default.clone()),
            (Role::Chat, Some("default"), chat_default.clone()),
            (Role::Chat, None, chat_default.clone()),
            (Role::Background, Some("ludicrous"), bg_default.clone()),
            (Role::Background, None, bg_default.clone()),
            // Padding around a real level is the level.
            (Role::Chat, Some(padded.as_str()), Some(offered.to_string())),
        ] {
            let n = normalise_on(role, vec![direct(p, &m.id, given)], TODAY);
            assert_eq!(
                n.entries,
                vec![direct(p, &m.id, want.as_deref())],
                "{role:?} {given:?}"
            );
        }
        // An OpenRouter row has no effort whatever it was handed.
        let n = normalise_on(
            Role::Background,
            vec![Entry::Cloud {
                provider: CloudProvider::OpenRouter,
                model: "v/a".into(),
                effort: Some("high".into()),
            }],
            TODAY,
        );
        assert_eq!(n.entries, vec![or("v/a")]);
    }

    #[test]
    fn normalising_a_normalised_list_changes_nothing() {
        let m = row_of(CloudProvider::Gemini);
        let messy = vec![
            or(" v/a "),
            Entry::Local,
            direct(CloudProvider::Gemini, &m.id, Some("nope")),
            or("v/a"),
            Entry::Local,
            or(""),
        ];
        for role in [Role::Chat, Role::Background] {
            let once = normalise_on(role, messy.clone(), TODAY);
            let twice = normalise_on(role, once.entries.clone(), TODAY);
            assert_eq!(twice.entries, once.entries);
            assert!(twice.dropped.is_empty());
        }
    }

    // ---- entries_for: stored, else derived ----

    #[test]
    fn a_fresh_store_derives_the_default_openrouter_list_and_writes_nothing() {
        let (_d, conn) = temp_db();
        let before = settings_snapshot(&conn);
        for role in [Role::Chat, Role::Background] {
            let r = entries_for(&conn, role).unwrap();
            assert!(r.derived);
            assert_eq!(r.entries, vec![or(settings::DEFAULT_MODEL)]);
        }
        assert_eq!(settings_snapshot(&conn), before, "a read writes nothing");
        assert_eq!(db::get_setting(&conn, CHAT_ROUTE_KEY).unwrap(), None);
    }

    #[test]
    fn a_stored_list_is_read_in_its_own_order_and_is_not_derived() {
        let (_d, conn) = temp_db();
        let m = row_of(CloudProvider::Anthropic);
        let list = vec![
            or("v/a"),
            Entry::Local,
            direct(CloudProvider::Anthropic, &m.id, None),
        ];
        db::set_setting(&conn, CHAT_ROUTE_KEY, &to_json(&list).unwrap()).unwrap();
        let r = entries_for(&conn, Role::Chat).unwrap();
        assert!(!r.derived);
        assert_eq!(
            r.entries,
            vec![
                or("v/a"),
                Entry::Local,
                direct(
                    CloudProvider::Anthropic,
                    &m.id,
                    direct_providers::default_effort(Role::Chat, m).as_deref()
                )
            ]
        );
        // The other role has no row of its own and is untouched by this one.
        assert!(entries_for(&conn, Role::Background).unwrap().derived);
        // Reads write nothing, stored or not.
        let before = settings_snapshot(&conn);
        entries_for(&conn, Role::Chat).unwrap();
        assert_eq!(settings_snapshot(&conn), before);
    }

    #[test]
    fn an_unreadable_stored_row_falls_back_to_the_derived_list() {
        let (_d, conn) = temp_db();
        db::set_setting(&conn, settings::CHAT_MODELS_KEY, r#"["v/old"]"#).unwrap();
        for bad in [
            "junk",
            "",
            r#"{"v":9,"entries":[{"k":"local"}]}"#,
            r#"{"v":1,"entries":[{"k":"cloud","p":"mistral","m":"a/b"}]}"#,
            // Readable but empty, and readable but every row refused: a role is never empty.
            r#"{"v":1,"entries":[]}"#,
            r#"{"v":1,"entries":[{"k":"cloud","p":"anthropic","m":"anthropic/claude-mythos-5-2"}]}"#,
            r#"{"v":1,"entries":[{"k":"cloud","p":"openrouter","m":"  "}]}"#,
        ] {
            db::set_setting(&conn, CHAT_ROUTE_KEY, bad).unwrap();
            let r = entries_for(&conn, Role::Chat).unwrap();
            assert!(r.derived, "{bad}");
            assert_eq!(r.entries, vec![or("v/old")], "{bad}");
        }
    }

    #[test]
    fn a_stored_list_loses_only_the_rows_that_fail_a_limit() {
        let (_d, conn) = temp_db();
        let raw = r#"{"v":1,"entries":[
            {"k":"cloud","p":"openrouter","m":"v/a"},
            {"k":"cloud","p":"anthropic","m":"anthropic/claude-mythos-5-2"},
            {"k":"local"},
            {"k":"cloud","p":"openrouter","m":"v/a"},
            {"k":"local"},
            {"k":"cloud","p":"openrouter","m":"v/b"}]}"#;
        db::set_setting(&conn, BACKGROUND_ROUTE_KEY, raw).unwrap();
        let r = entries_for(&conn, Role::Background).unwrap();
        assert!(!r.derived);
        assert_eq!(r.entries, vec![or("v/a"), Entry::Local, or("v/b")]);
    }

    // ---- the golden derive ----

    fn settings_snapshot(conn: &Connection) -> Vec<(String, String)> {
        let mut stmt = conn
            .prepare("SELECT key, value FROM settings ORDER BY key")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    /// Every settings row the old routing read, so a configuration can be cleared between cases.
    fn clear_routing(conn: &Connection) {
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
            CHAT_ROUTE_KEY.into(),
            BACKGROUND_ROUTE_KEY.into(),
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

    /// How OpenRouter's side is configured in one case.
    #[derive(Clone, Copy, Debug)]
    enum OrList {
        Absent,
        One,
        Three,
        /// Only the old single `default_model`, no list.
        LegacyOnly,
        /// A stored list that is not an array of ids.
        Junk,
    }

    const OR_ONE: &[&str] = &["vendor/one"];
    const OR_THREE: &[&str] = &["vendor/first", "vendor/second", "vendor/third"];

    fn or_models(list: OrList) -> Vec<String> {
        let ids: &[&str] = match list {
            OrList::Absent | OrList::Junk => &[settings::DEFAULT_MODEL],
            OrList::LegacyOnly => &["vendor/legacy"],
            OrList::One => OR_ONE,
            OrList::Three => OR_THREE,
        };
        ids.iter().map(|s| s.to_string()).collect()
    }

    /// The model PM would put in a direct arm for `p` in a given configuration, worked out BY HAND
    /// from the catalogue (not through `models_for`), and the effort it would send.
    fn direct_expected(
        p: CloudProvider,
        role: Role,
        stored_model: Option<&str>,
        stored_effort: Option<&str>,
    ) -> (String, Option<String>) {
        let default = direct_models::default_for(p, role).unwrap();
        let model = match stored_model {
            Some(id) => direct_models::models(p)
                .iter()
                .find(|m| m.id == id && !m.retired(&direct_models::utc_today()))
                .unwrap_or(default),
            None => default,
        };
        let effort = match stored_effort {
            Some(l) if model.offers(l) => Some(l.to_string()),
            _ => match role {
                Role::Chat => None,
                Role::Background => {
                    if model.offers("low") {
                        Some("low".into())
                    } else {
                        model.effort_levels.first().cloned()
                    }
                }
            },
        };
        (model.id.clone(), effort)
    }

    /// The whole configuration space the old code branched on, derived and compared with what the old
    /// code would have used: the cloud models and effort in the same order, and where this computer
    /// sits. The expectation is written out by hand AND checked against the old functions' own
    /// answers (`resolve_provider`, `role_cloud_provider`, `effective_models`, `arm_inputs`), so a
    /// mistake in either shows.
    #[test]
    fn the_golden_derive_holds_exactly_what_the_old_code_would_use() {
        let (_d, conn) = temp_db();
        let direct_ps = [
            CloudProvider::OpenAi,
            CloudProvider::Gemini,
            CloudProvider::Anthropic,
        ];
        // A model of each provider that is not its default, to prove the STORED one is the one used.
        let second = |p: CloudProvider, role: Role| -> &'static DirectModel {
            let default = direct_models::default_for(p, role).unwrap();
            direct_models::models(p)
                .iter()
                .find(|m| m.id != default.id && !m.retired(&direct_models::utc_today()))
                .expect("a second live row")
        };
        let mut cases = 0usize;
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
            // (stored provider token, opted in)
            let providers: Vec<(Option<&str>, bool)> = [None, Some("junk"), Some("openrouter")]
                .into_iter()
                .flat_map(|t| [(t, false), (t, true)])
                .chain(
                    ["openai", "gemini", "anthropic"]
                        .into_iter()
                        .flat_map(|t| [(Some(t), false), (Some(t), true)]),
                )
                .collect();
            for (token, opted) in providers {
                let stored_provider = token
                    .map(|t| CloudProvider::from_setting(Some(t)))
                    .unwrap_or(CloudProvider::OpenRouter);
                let effective = if stored_provider.is_direct() && !opted {
                    CloudProvider::OpenRouter
                } else {
                    stored_provider
                };
                // Vary only what the effective provider reads.
                let or_axis: Vec<(OrList, Option<bool>)> = if effective.is_direct() {
                    vec![(OrList::Absent, None)]
                } else {
                    [
                        OrList::Absent,
                        OrList::One,
                        OrList::Three,
                        OrList::LegacyOnly,
                        OrList::Junk,
                    ]
                    .into_iter()
                    .flat_map(|l| [None, Some(false), Some(true)].map(|a| (l, a)))
                    .collect()
                };
                let direct_axis: Vec<(Option<&str>, Option<&str>)> = if effective.is_direct() {
                    let s = second(effective, role);
                    let level_of_second = s.effort_levels.last().map(String::as_str);
                    vec![
                        (None, None),
                        (Some(s.id.as_str()), None),
                        (Some(s.id.as_str()), level_of_second),
                        (None, Some("no-such-level")),
                        (Some("bogus/not-a-row"), None),
                        (Some("bogus/not-a-row"), Some("no-such-level")),
                    ]
                } else {
                    vec![(None, None)]
                };
                for (or_list, auto) in &or_axis {
                    for (dm, de) in &direct_axis {
                        for routing in [
                            None,
                            Some("cloud"),
                            Some("local"),
                            Some("local-then-cloud"),
                            Some("junk"),
                        ] {
                            for local_on in [true, false] {
                                for emptied in [false, true] {
                                    clear_routing(&conn);
                                    if let Some(t) = token {
                                        db::set_setting(
                                            &conn,
                                            llm_gateway::cloud_provider_key(role),
                                            t,
                                        )
                                        .unwrap();
                                    }
                                    for p in direct_ps {
                                        if opted {
                                            direct_providers::set_opt_in(&conn, p, true).unwrap();
                                        }
                                    }
                                    match or_list {
                                        OrList::Absent => {}
                                        OrList::One => db::set_setting(
                                            &conn,
                                            models_key,
                                            &serde_json::to_string(OR_ONE).unwrap(),
                                        )
                                        .unwrap(),
                                        OrList::Three => db::set_setting(
                                            &conn,
                                            models_key,
                                            &serde_json::to_string(OR_THREE).unwrap(),
                                        )
                                        .unwrap(),
                                        OrList::LegacyOnly => {
                                            db::set_setting(&conn, "default_model", "vendor/legacy")
                                                .unwrap()
                                        }
                                        OrList::Junk => {
                                            db::set_setting(&conn, models_key, "{not an array")
                                                .unwrap()
                                        }
                                    }
                                    if let Some(a) = auto {
                                        db::set_bool(&conn, auto_key, *a).unwrap();
                                    }
                                    if effective.is_direct() {
                                        let keys = direct_providers::provider(effective)
                                            .unwrap()
                                            .role(role);
                                        if let Some(m) = dm {
                                            db::set_setting(
                                                &conn,
                                                keys.models,
                                                &serde_json::to_string(&[m]).unwrap(),
                                            )
                                            .unwrap();
                                        }
                                        if let Some(e) = de {
                                            db::set_setting(&conn, keys.effort, e).unwrap();
                                        }
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

                                    // ---- what the old code would use, by hand ----
                                    let cloud_expected: Vec<Entry> = if effective.is_direct() {
                                        let (model, effort) = direct_expected(
                                            effective,
                                            role,
                                            dm.as_deref(),
                                            de.as_deref(),
                                        );
                                        vec![Entry::Cloud {
                                            provider: effective,
                                            model,
                                            effort,
                                        }]
                                    } else {
                                        let all = or_models(*or_list);
                                        let used = if *auto == Some(true) {
                                            all
                                        } else {
                                            all.into_iter().take(1).collect()
                                        };
                                        used.into_iter().map(|m| or(&m)).collect()
                                    };
                                    let served = match routing {
                                        Some("local") => ProviderPref::Local,
                                        Some("local-then-cloud") if emptied => ProviderPref::Local,
                                        Some("local-then-cloud") => ProviderPref::LocalThenCloud,
                                        _ => ProviderPref::Cloud,
                                    };
                                    let expected: Vec<Entry> = match served {
                                        ProviderPref::Cloud => cloud_expected.clone(),
                                        ProviderPref::Local => vec![Entry::Local],
                                        ProviderPref::LocalThenCloud => {
                                            let mut v = vec![Entry::Local];
                                            v.extend(cloud_expected.clone());
                                            v
                                        }
                                    };

                                    let got = entries_for(&conn, role).unwrap();
                                    let ctx = format!(
                                        "{role:?} routing={routing:?} local_on={local_on} emptied={emptied} \
                                         provider={token:?} opted={opted} or={or_list:?} auto={auto:?} \
                                         dmodel={dm:?} deffort={de:?}"
                                    );
                                    assert!(got.derived, "{ctx}");
                                    assert_eq!(got.entries, expected, "{ctx}");

                                    // ---- and what the old code's own functions say ----
                                    let prefs = llm_gateway::routing_prefs(&conn).unwrap();
                                    let choice = llm_gateway::resolve_provider(
                                        role,
                                        &prefs,
                                        &RuntimeContext::default(),
                                    );
                                    let old_cloud: Vec<Entry> = {
                                        let p =
                                            llm_gateway::role_cloud_provider(&conn, role).unwrap();
                                        assert_eq!(p, effective, "{ctx}");
                                        if p.is_direct() {
                                            let a = direct_providers::arm_inputs(&conn, p, role)
                                                .unwrap();
                                            assert_eq!(a.models.len(), 1, "{ctx}");
                                            vec![Entry::Cloud {
                                                provider: p,
                                                model: a.models[0].clone(),
                                                effort: a.effort,
                                            }]
                                        } else {
                                            settings::effective_models(&conn, models_key, auto_key)
                                                .unwrap()
                                                .into_iter()
                                                .map(|m| or(&m))
                                                .collect()
                                        }
                                    };
                                    let from_old = match (choice, prefs.for_role(role)) {
                                        (ProviderChoice::Cloud, _) => old_cloud,
                                        (ProviderChoice::Local, _) => vec![Entry::Local],
                                        (ProviderChoice::LocalThenCloud, _) => {
                                            let mut v = vec![Entry::Local];
                                            v.extend(old_cloud);
                                            v
                                        }
                                        // The old code stopped here, at the gate; what it WOULD have
                                        // planned is the preference's own shape.
                                        (ProviderChoice::LocalAiOff, ProviderPref::Local) => {
                                            vec![Entry::Local]
                                        }
                                        (ProviderChoice::LocalAiOff, _) => {
                                            let mut v = vec![Entry::Local];
                                            v.extend(old_cloud);
                                            v
                                        }
                                        (ProviderChoice::CloudForPower, _) => {
                                            unreachable!("an inert context never moves a role")
                                        }
                                    };
                                    // With Local AI off the old code stopped at the gate whatever
                                    // the "pick a model" record said, so the emptied-and-off case
                                    // has no plan of the old code's to compare; the hand-written
                                    // expectation above (the same stop, `Local` alone) covers it.
                                    if !(emptied
                                        && !local_on
                                        && prefs.for_role(role) == ProviderPref::LocalThenCloud)
                                    {
                                        assert_eq!(got.entries, from_old, "old code: {ctx}");
                                    }
                                    cases += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(cases > 4000, "the sweep ran {cases} cases");
    }
}
