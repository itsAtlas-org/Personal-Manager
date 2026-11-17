// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Available section (Mods spec §1.13): the signed catalogue as the Mods tab lists it, read
//! from the cached index (slice 5), and the rule for which entry may be installed now. Pure over
//! the index, the installed versions and the clock; the download and the consent screen are the
//! commands' (`commands.rs`, [`super::install`]).
//!
//! **Usable.** Only an index inside its life ([`Index::current_at`]) installs anything: an expired
//! one may still restrict (its blacklist applies), but its catalogue is shown read-only, with each
//! entry saying why it can't be installed. With no index at all the list is empty and says so.
//!
//! **Why an entry can't be installed**, in the order the install itself would refuse: the list is
//! out of date; the entry is on the blacklist (by id and version, or by its sha256); it needs a newer
//! PM than this one (the entry's own `api` range, known without downloading anything); it was built
//! for a mod API PM no longer serves.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::error::{Code, ModsError};
use crate::mods::api::permissions;
use crate::mods::index::{CatalogueEntry, Index};
use crate::mods::policy::{self, ApiFit, MOD_API_OLDEST_MAJOR, MOD_API_VERSION};
use crate::mods::safety;

/// Why an entry can't be installed now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Ok,
    /// The index is past its life: it restricts, it doesn't install.
    Stale,
    /// On PM's safety list; the words are the list's.
    Blocked(String),
    /// Needs a mod API (`M.m`) newer than this PM's.
    NeedsNewerPm(String),
    /// Built for an API major PM no longer serves.
    Retired,
}

impl Availability {
    pub fn words(&self) -> Option<String> {
        match self {
            Self::Ok => None,
            Self::Stale => Some(
                "PM's safety list is out of date, so it won't install from the catalogue until it \
                 is refreshed."
                    .to_owned(),
            ),
            Self::Blocked(words) => Some(words.clone()),
            Self::NeedsNewerPm(api) => Some(format!(
                "It needs mod API {api}, newer than this PM's. Update PM to get it."
            )),
            Self::Retired => Some(
                "It was built for a mod API this PM no longer serves, and needs an update from its \
                 maker."
                    .to_owned(),
            ),
        }
    }

    pub fn code(&self) -> Code {
        match self {
            Self::Ok => Code::Internal,
            Self::Stale | Self::Blocked(_) => Code::SafetyList,
            Self::NeedsNewerPm(_) => Code::NeedsNewerPm,
            Self::Retired => Code::ApiRetired,
        }
    }
}

/// Whether `entry` may be installed from `index` at `now` (pure).
pub fn availability(entry: &CatalogueEntry, index: &Index, now: DateTime<Utc>) -> Availability {
    if !index.current_at(now) {
        return Availability::Stale;
    }
    if let Some(hit) = index.blacklisted(
        entry.id.as_str(),
        entry.version.as_str(),
        Some(&entry.sha256),
    ) {
        return Availability::Blocked(safety::blacklist_words(hit));
    }
    match policy::api_fit(entry.api, MOD_API_VERSION, MOD_API_OLDEST_MAJOR) {
        ApiFit::NeedsNewerPm => {
            Availability::NeedsNewerPm(format!("{}.{}", entry.api.min.0, entry.api.min.1))
        }
        ApiFit::Retired => Availability::Retired,
        ApiFit::Fits | ApiFit::OlderMajor => Availability::Ok,
    }
}

/// The refusal for an entry that can't be installed, `None` when it can.
pub fn refusal(a: &Availability) -> Option<ModsError> {
    a.words().map(|w| ModsError::new(a.code(), w))
}

/// The entry for `id`, or the refusal for there being none (pure). `index`: the cached index, if
/// there is one.
pub fn find<'a>(index: Option<&'a Index>, id: &str) -> Result<&'a CatalogueEntry, ModsError> {
    let index = index.ok_or_else(|| {
        ModsError::new(
            Code::NotInCatalogue,
            "PM hasn't downloaded the catalogue yet. It needs a connection once.",
        )
    })?;
    index
        .entry(id)
        .ok_or_else(|| ModsError::new(Code::NotInCatalogue, "The catalogue doesn't list that mod."))
}

/// Whether `entry` is newer than the installed `version` (pure). A version that doesn't compare
/// counts as not newer: an update is never guessed.
pub fn is_newer(entry: &CatalogueEntry, installed: &str) -> bool {
    semver::Version::parse(installed)
        .ok()
        .zip(semver::Version::parse(entry.version.as_str()).ok())
        .is_some_and(|(have, want)| want > have)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalogue {
    pub entries: Vec<EntryDto>,
    /// Whether the catalogue can install anything now (an index inside its life).
    pub usable: bool,
    /// When it can't, why, as a sentence.
    pub unavailable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub tier: String,
    pub author: String,
    pub repo: String,
    pub commit: String,
    /// The latest version the catalogue lists.
    pub version: String,
    pub permissions: Vec<EntryPermission>,
    pub installed: bool,
    pub installed_version: Option<String>,
    /// A newer version than the installed one is listed.
    pub update_available: bool,
    /// It needs a newer PM ("update PM to get version N").
    pub needs_newer_pm: bool,
    pub api_retired: bool,
    /// On PM's safety list, in the list's words.
    pub blocked: Option<String>,
    /// May be installed (or updated) now.
    pub installable: bool,
    /// Why not, as a sentence, when it isn't.
    pub why_not: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryPermission {
    pub id: String,
    pub class: &'static str,
}

fn class_word(id: &str) -> &'static str {
    use permissions::Class::*;
    match permissions::policy(id).map(|p| p.class) {
        Some(Local) => "local",
        Some(Read) => "read",
        Some(Write) => "write",
        Some(Money) => "money",
        Some(Egress) => "egress",
        None => "unknown",
    }
}

/// The catalogue as the tab lists it (pure). `installed`: installed mods' versions by id.
pub fn list(
    index: Option<&Index>,
    installed: &BTreeMap<String, String>,
    now: DateTime<Utc>,
) -> Catalogue {
    let Some(index) = index else {
        return Catalogue {
            entries: Vec::new(),
            usable: false,
            unavailable: Some(
                "PM hasn't downloaded the catalogue yet. It needs a connection once.".to_owned(),
            ),
        };
    };
    let usable = index.current_at(now);
    let entries = index
        .mods
        .iter()
        .map(|e| {
            let have = installed.get(e.id.as_str());
            let a = availability(e, index, now);
            let blocked = match &a {
                Availability::Blocked(w) => Some(w.clone()),
                _ => None,
            };
            EntryDto {
                id: e.id.as_str().to_owned(),
                name: e.name.clone(),
                description: e.description.clone(),
                tier: e.tier.as_str().to_owned(),
                author: e.author.clone(),
                repo: e.source_repo.clone(),
                commit: e.commit.clone(),
                version: e.version.as_str().to_owned(),
                permissions: e
                    .permissions
                    .iter()
                    .map(|p| EntryPermission {
                        id: p.clone(),
                        class: class_word(p),
                    })
                    .collect(),
                installed: have.is_some(),
                installed_version: have.cloned(),
                update_available: have.is_some_and(|v| is_newer(e, v)),
                needs_newer_pm: matches!(a, Availability::NeedsNewerPm(_)),
                api_retired: a == Availability::Retired,
                blocked,
                installable: a == Availability::Ok,
                why_not: a.words(),
            }
        })
        .collect();
    Catalogue {
        entries,
        usable,
        unavailable: (!usable).then(|| {
            "PM's safety list is out of date. The catalogue is shown, but nothing installs from it \
             until the list is refreshed."
                .to_owned()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::index::tests::good_entry;
    use crate::mods::safety::tests::{index_with, now};
    use serde_json::{json, Value};

    fn index(change: impl FnOnce(&mut Value)) -> Index {
        index_with(|v| {
            v["mods"] = json!([good_entry()]);
            change(v);
        })
    }

    #[test]
    fn an_entry_installs_when_the_list_is_current_unblocked_and_fits_this_pm() {
        let i = index(|_| {});
        let e = &i.mods[0];
        assert_eq!(availability(e, &i, now()), Availability::Ok);
        assert!(refusal(&Availability::Ok).is_none());
    }

    #[test]
    fn a_list_past_its_life_installs_nothing_but_still_lists_everything() {
        let i = index(|_| {});
        let late = i.expires + chrono::Duration::days(3);
        assert_eq!(availability(&i.mods[0], &i, late), Availability::Stale);
        let c = list(Some(&i), &BTreeMap::new(), late);
        assert!(!c.usable);
        assert!(c.unavailable.unwrap().contains("out of date"));
        assert_eq!(c.entries.len(), 1);
        assert!(!c.entries[0].installable);
        assert!(c.entries[0]
            .why_not
            .as_ref()
            .unwrap()
            .contains("out of date"));
        // Inside the two days of skew it is still current.
        let skewed = i.expires + chrono::Duration::days(1);
        assert_eq!(availability(&i.mods[0], &i, skewed), Availability::Ok);
    }

    #[test]
    fn a_blacklisted_entry_is_blocked_with_the_lists_words_by_id_range_or_hash() {
        let by_id = index(|v| {
            v["blacklist"] = json!([{"id": "atlas.map", "versions": "*", "action": "paused",
                "reason": "It loses notes."}]);
        });
        let a = availability(&by_id.mods[0], &by_id, now());
        assert_eq!(
            a,
            Availability::Blocked("PM's safety list: It loses notes.".into())
        );
        assert_eq!(refusal(&a).unwrap().code, Code::SafetyList);
        let sha = by_id.mods[0].sha256.clone();
        let by_hash = index(|v| {
            v["blacklist"] = json!([{"sha256": sha, "action": "removed", "reason": "Known bad."}]);
        });
        assert!(matches!(
            availability(&by_hash.mods[0], &by_hash, now()),
            Availability::Blocked(w) if w.contains("Known bad.")
        ));
        // A range that stops short of the entry's version doesn't block it.
        let other = index(|v| {
            v["blacklist"] = json!([{"id": "atlas.map", "versions": "<0.0.1", "action": "paused",
                "reason": "Old."}]);
        });
        assert_eq!(
            availability(&other.mods[0], &other, now()),
            Availability::Ok
        );
    }

    #[test]
    fn an_entry_needing_a_newer_api_says_update_pm_without_a_download() {
        let newer = index(|v| v["mods"][0]["api"] = json!("1.1-1.1"));
        let a = availability(&newer.mods[0], &newer, now());
        assert_eq!(a, Availability::NeedsNewerPm("1.1".into()));
        assert!(a.words().unwrap().contains("Update PM"));
        assert_eq!(refusal(&a).unwrap().code, Code::NeedsNewerPm);
        let retired = index(|v| v["mods"][0]["api"] = json!("0.9-0.9"));
        let a = availability(&retired.mods[0], &retired, now());
        assert_eq!(a, Availability::Retired);
        assert_eq!(refusal(&a).unwrap().code, Code::ApiRetired);
    }

    #[test]
    fn the_listing_marks_what_is_installed_and_what_has_a_newer_version() {
        let i = index(|_| {});
        let listed_version = i.mods[0].version.as_str().to_owned();
        let none = list(Some(&i), &BTreeMap::new(), now());
        assert!(!none.entries[0].installed && !none.entries[0].update_available);
        assert!(none.usable && none.unavailable.is_none());
        assert!(none.entries[0].installable);
        let same = BTreeMap::from([("atlas.map".to_owned(), listed_version.clone())]);
        let c = list(Some(&i), &same, now());
        assert!(c.entries[0].installed && !c.entries[0].update_available);
        let older = BTreeMap::from([("atlas.map".to_owned(), "0.0.1".to_owned())]);
        let c = list(Some(&i), &older, now());
        assert!(c.entries[0].update_available);
        assert_eq!(c.entries[0].installed_version.as_deref(), Some("0.0.1"));
        // An installed version that isn't a semver is never called outdated.
        let odd = BTreeMap::from([("atlas.map".to_owned(), "weird".to_owned())]);
        assert!(!list(Some(&i), &odd, now()).entries[0].update_available);
        // The entry shows each permission with its class.
        assert!(c.entries[0]
            .permissions
            .iter()
            .all(|p| p.class != "unknown"));
    }

    #[test]
    fn with_no_index_the_list_is_empty_and_says_why_and_find_refuses() {
        let c = list(None, &BTreeMap::new(), now());
        assert!(c.entries.is_empty() && !c.usable);
        assert!(c.unavailable.unwrap().contains("downloaded the catalogue"));
        assert_eq!(
            find(None, "atlas.map").unwrap_err().code,
            Code::NotInCatalogue
        );
        let i = index(|_| {});
        assert!(find(Some(&i), "atlas.map").is_ok());
        assert_eq!(
            find(Some(&i), "acme.other").unwrap_err().code,
            Code::NotInCatalogue
        );
    }

    #[test]
    fn is_newer_compares_semver_including_prereleases() {
        let i = index(|_| {});
        let e = &i.mods[0];
        let v = semver::Version::parse(e.version.as_str()).unwrap();
        let lower = semver::Version::new(v.major, v.minor, 0).to_string();
        assert_eq!(is_newer(e, &lower), v.patch > 0);
        assert!(!is_newer(e, e.version.as_str()));
        assert!(!is_newer(e, "99.0.0"));
        assert!(is_newer(e, "0.0.0-alpha.1"));
    }
}
