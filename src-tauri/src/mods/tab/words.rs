// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What an installed row says to the person: its standing as a typed kind, the sentence for it and
//! the one thing they can do about it (Mods spec §1.3 "Parking", §1.10, §1.13, D17). The page draws
//! these and invents nothing, so a state added in Rust without words here fails the test that walks
//! every reason.
//!
//! Pure: the row's facts and, for the safety list's own words, the cached index come in; text goes
//! out. The words for a stop the safety list made come from the index, not from the row (the row
//! holds only that it paused), so a corrected reason reaches the strip.

use serde::Serialize;

use crate::mods::index::Index;
use crate::mods::package::ApiRange;
use crate::mods::policy::{self, ApiFit, MOD_API_OLDEST_MAJOR, MOD_API_VERSION};
use crate::mods::store;

/// How a row stands, in kinds the page branches on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// Active and turned on.
    Running,
    /// Installed (an auto-installed pilot, D17) but its first-open "Turn on" hasn't happened.
    WaitingForYou,
    PausedByYou,
    /// Paused by PM because the page stopped responding while this mod's frame was the last one
    /// active (`mods::watchdog`). Resumed only by the user.
    PausedFrozen,
    PausedBySafetyList,
    RemovedBySafetyList,
    /// Paused because developer mode is off (an unsigned mod).
    PausedDevMode,
    FilesMissing,
    FilesChanged,
    RecordUnreadable,
    ApiUnsupported,
    /// Parked by another owner (a closed room).
    Parked,
    Installing,
    /// Paused for a reason this PM has no words for (a newer PM wrote it).
    PausedOther,
}

/// The one thing the page offers for a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Fix {
    None,
    /// The first-open screen.
    TurnOn,
    Resume,
    /// `repair_mod`: fetch exactly the installed version again from the catalogue.
    Repair,
    /// A file install has no release to come back from: pick the file again.
    InstallFileAgain,
    Uninstall,
    UpdatePm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Standing {
    /// `mods.state`, as stored.
    pub state: String,
    /// `mods.state_reason`, as stored.
    pub reason: Option<String>,
    pub kind: Kind,
    pub text: String,
    pub fix: Fix,
    /// Whether the mod is on: active and turned on. (The registry can still hold it back for the
    /// safety list; that shows as its own kind once the next pass writes it.)
    pub running: bool,
}

/// What the words need to know about a row.
#[derive(Debug, Clone)]
pub struct RowFacts<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub version: &'a str,
    pub package_sha256: Option<&'a str>,
    pub state: &'a str,
    pub reason: Option<&'a str>,
    pub consented: bool,
    /// `mods.installed_from`.
    pub installed_from: &'a str,
    /// Its API range, when the stored manifest still reads.
    pub api: Option<ApiRange>,
}

impl RowFacts<'_> {
    fn is_catalogue(&self) -> bool {
        self.installed_from == store::FROM_CATALOGUE
    }
}

pub fn standing(row: &RowFacts<'_>, index: Option<&Index>) -> Standing {
    let (kind, text, fix) = words(row, index);
    Standing {
        state: row.state.to_owned(),
        reason: row.reason.map(str::to_owned),
        kind,
        text,
        fix,
        running: row.state == store::STATE_ACTIVE && row.consented,
    }
}

fn words(row: &RowFacts<'_>, index: Option<&Index>) -> (Kind, String, Fix) {
    let name = row.name;
    match (row.state, row.reason) {
        (store::STATE_INSTALLING, _) => (
            Kind::Installing,
            "Installing. If it doesn't finish, PM tries again at the next safety list check."
                .to_owned(),
            Fix::None,
        ),
        (store::STATE_ACTIVE, _) if !row.consented => (
            Kind::WaitingForYou,
            format!(
                "{name} is installed but off. Open it to read what it asks for and turn it on."
            ),
            Fix::TurnOn,
        ),
        (store::STATE_ACTIVE, _) => (Kind::Running, "Running.".to_owned(), Fix::None),
        (store::STATE_PAUSED, Some(store::REASON_USER)) => {
            (Kind::PausedByYou, "Paused by you.".to_owned(), Fix::Resume)
        }
        (store::STATE_PAUSED, Some(store::REASON_FROZEN)) => (
            Kind::PausedFrozen,
            format!(
                "{name} stopped responding, and PM paused it so the rest of PM keeps working. \
                 Resume it when you want to try it again."
            ),
            Fix::Resume,
        ),
        (store::STATE_PAUSED, Some(store::REASON_BLACKLIST)) => (
            Kind::PausedBySafetyList,
            paused_by_list(row, index),
            Fix::None,
        ),
        (store::STATE_PAUSED, Some(store::REASON_BLACKLIST_REMOVED)) => (
            Kind::RemovedBySafetyList,
            removed_by_list(row, index),
            Fix::Uninstall,
        ),
        (store::STATE_PAUSED, Some(store::REASON_DEV_MODE)) => (
            Kind::PausedDevMode,
            "Paused because developer mode is off. Turn developer mode on to use it again."
                .to_owned(),
            Fix::None,
        ),
        (store::STATE_PAUSED, _) => (Kind::PausedOther, "Paused.".to_owned(), Fix::None),
        (store::STATE_PARKED, Some(store::REASON_FILES_MISSING)) => {
            if row.is_catalogue() {
                (
                    Kind::FilesMissing,
                    format!("Install {name} to get this back."),
                    Fix::Repair,
                )
            } else {
                (
                    Kind::FilesMissing,
                    "Install the file again to get this back.".to_owned(),
                    Fix::InstallFileAgain,
                )
            }
        }
        (store::STATE_PARKED, Some(store::REASON_FILES_CHANGED)) => {
            if row.is_catalogue() {
                (
                    Kind::FilesChanged,
                    "Its files aren't what PM installed. Repair it to put the right ones back."
                        .to_owned(),
                    Fix::Repair,
                )
            } else {
                (
                    Kind::FilesChanged,
                    "Its files aren't what PM installed. Install the file again to repair it."
                        .to_owned(),
                    Fix::InstallFileAgain,
                )
            }
        }
        (store::STATE_PARKED, Some(store::REASON_RECORD)) => (
            Kind::RecordUnreadable,
            "PM can't read its record any more. Uninstall it, then install it again.".to_owned(),
            Fix::Uninstall,
        ),
        (store::STATE_PARKED, Some(store::REASON_API)) => api_words(row),
        (store::STATE_PARKED, _) => (
            Kind::Parked,
            "It isn't running right now.".to_owned(),
            Fix::None,
        ),
        // `mods.state` is CHECKed to four values; a fifth is a newer PM's.
        _ => (Kind::PausedOther, "It isn't running.".to_owned(), Fix::None),
    }
}

fn paused_by_list(row: &RowFacts<'_>, index: Option<&Index>) -> String {
    let entry = index.and_then(|i| i.blacklisted(row.id, row.version, row.package_sha256));
    match entry {
        Some(e) => {
            let fixed = e
                .fixed_in
                .as_ref()
                .map(|v| format!(" (fixed in version {})", v.as_str()))
                .unwrap_or_default();
            format!(
                "Paused while we fix a problem: {}. Your data is safe. It will turn back on by \
                 itself when the fix arrives{fixed}.",
                e.reason.trim_end_matches('.')
            )
        }
        None => "Paused by PM's safety list. Your data is safe. It turns back on by itself once \
                 the list says it is fixed."
            .to_owned(),
    }
}

fn removed_by_list(row: &RowFacts<'_>, index: Option<&Index>) -> String {
    let entry = index.and_then(|i| i.blacklisted(row.id, row.version, row.package_sha256));
    match entry {
        Some(e) => format!(
            "PM's safety list removed it: {}. Uninstall it to clear what it left, and choose what \
             happens to what it made.",
            e.reason.trim_end_matches('.')
        ),
        None => "PM's safety list removed it. Uninstall it to clear what it left, and choose what \
                 happens to what it made."
            .to_owned(),
    }
}

fn api_words(row: &RowFacts<'_>) -> (Kind, String, Fix) {
    let fit = row
        .api
        .map(|api| policy::api_fit(api, MOD_API_VERSION, MOD_API_OLDEST_MAJOR));
    match fit {
        Some(ApiFit::NeedsNewerPm) => (
            Kind::ApiUnsupported,
            "It needs a newer PM. Update PM to run it.".to_owned(),
            Fix::UpdatePm,
        ),
        _ => (
            Kind::ApiUnsupported,
            "It was built for a mod API this PM no longer supports. It needs an update from its \
             maker."
                .to_owned(),
            Fix::None,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::safety::tests::index_with;
    use serde_json::json;

    fn facts<'a>(state: &'a str, reason: Option<&'a str>) -> RowFacts<'a> {
        RowFacts {
            id: "atlas.map",
            name: "Map",
            version: "1.0.0",
            package_sha256: None,
            state,
            reason,
            consented: true,
            installed_from: store::FROM_CATALOGUE,
            api: None,
        }
    }

    fn of(state: &str, reason: Option<&str>) -> Standing {
        standing(&facts(state, reason), None)
    }

    #[test]
    fn files_missing_says_install_x_for_a_catalogue_mod_and_the_file_again_for_a_file_one() {
        let s = of("parked", Some("files-missing"));
        assert_eq!((s.kind, s.fix), (Kind::FilesMissing, Fix::Repair));
        assert_eq!(s.text, "Install Map to get this back.");
        for from in [store::FROM_FILE, store::FROM_FOLDER] {
            let s = standing(
                &RowFacts {
                    installed_from: from,
                    ..facts("parked", Some("files-missing"))
                },
                None,
            );
            assert_eq!((s.kind, s.fix), (Kind::FilesMissing, Fix::InstallFileAgain));
            assert_eq!(s.text, "Install the file again to get this back.");
        }
    }

    #[test]
    fn changed_files_offer_a_repair_where_a_release_can_put_them_back() {
        let s = of("parked", Some("files-changed"));
        assert_eq!((s.kind, s.fix), (Kind::FilesChanged, Fix::Repair));
        let s = standing(
            &RowFacts {
                installed_from: store::FROM_FILE,
                ..facts("parked", Some("files-changed"))
            },
            None,
        );
        assert_eq!((s.kind, s.fix), (Kind::FilesChanged, Fix::InstallFileAgain));
    }

    #[test]
    fn an_unreadable_record_says_uninstall_it() {
        let s = of("parked", Some("record-unreadable"));
        assert_eq!((s.kind, s.fix), (Kind::RecordUnreadable, Fix::Uninstall));
    }

    #[test]
    fn a_newer_api_says_update_pm_and_a_retired_one_says_ask_its_maker() {
        let needs = |min: (u16, u16), target: (u16, u16)| {
            standing(
                &RowFacts {
                    api: Some(ApiRange { min, target }),
                    ..facts("parked", Some("api-unsupported"))
                },
                None,
            )
        };
        let newer = needs((1, 1), (1, 1));
        assert_eq!(
            (newer.kind, newer.fix),
            (Kind::ApiUnsupported, Fix::UpdatePm)
        );
        assert!(newer.text.contains("Update PM"));
        let retired = needs((0, 9), (0, 9));
        assert_eq!(
            (retired.kind, retired.fix),
            (Kind::ApiUnsupported, Fix::None)
        );
        assert!(!retired.text.contains("Update PM"), "{}", retired.text);
        // A manifest that no longer reads can't be told apart: the safe words.
        assert_eq!(
            standing(&facts("parked", Some("api-unsupported")), None).fix,
            Fix::None
        );
    }

    #[test]
    fn the_users_pause_offers_resume_and_dev_modes_does_not() {
        let s = of("paused", Some("user"));
        assert_eq!(
            (s.kind, s.fix, s.text.as_str()),
            (Kind::PausedByYou, Fix::Resume, "Paused by you.")
        );
        let s = of("paused", Some("dev-mode"));
        assert_eq!((s.kind, s.fix), (Kind::PausedDevMode, Fix::None));
        assert!(s.text.contains("developer mode"));
    }

    #[test]
    fn the_safety_lists_own_words_come_from_the_index_with_the_fix_version() {
        let index = index_with(|v| {
            v["blacklist"] = json!([{"id": "atlas.map", "versions": "=1.0.0", "action": "paused",
                "reason": "It loses notes.", "fixedIn": "1.0.1"}]);
        });
        let s = standing(&facts("paused", Some("blacklist")), Some(&index));
        assert_eq!((s.kind, s.fix), (Kind::PausedBySafetyList, Fix::None));
        assert_eq!(
            s.text,
            "Paused while we fix a problem: It loses notes. Your data is safe. It will turn back \
             on by itself when the fix arrives (fixed in version 1.0.1)."
        );
        // With no matching entry (lifted, not yet applied), still a sentence, still not alarming.
        let s = standing(&facts("paused", Some("blacklist")), None);
        assert!(
            s.text.starts_with("Paused by PM's safety list."),
            "{}",
            s.text
        );
    }

    #[test]
    fn a_removed_mod_quotes_the_reason_and_offers_only_the_uninstall_dialog() {
        let index = index_with(|v| {
            v["blacklist"] = json!([{"id": "atlas.map", "versions": "*", "action": "removed",
                "reason": "It sends your documents elsewhere."}]);
        });
        let s = standing(&facts("paused", Some("blacklist-removed")), Some(&index));
        assert_eq!((s.kind, s.fix), (Kind::RemovedBySafetyList, Fix::Uninstall));
        assert!(
            s.text.contains("It sends your documents elsewhere:")
                || s.text.contains("It sends your documents elsewhere."),
            "{}",
            s.text
        );
        assert!(!s.running);
    }

    #[test]
    fn an_auto_installed_pilot_waits_for_its_first_open_and_is_not_running() {
        let s = standing(
            &RowFacts {
                consented: false,
                ..facts("active", None)
            },
            None,
        );
        assert_eq!(
            (s.kind, s.fix, s.running),
            (Kind::WaitingForYou, Fix::TurnOn, false)
        );
        let s = of("active", None);
        assert_eq!((s.kind, s.fix, s.running), (Kind::Running, Fix::None, true));
    }

    #[test]
    fn every_state_and_reason_the_ledger_can_write_has_words_and_unknown_ones_are_not_alarming() {
        let reasons: Vec<Option<&str>> = std::iter::once(None)
            .chain(store::LEDGER_REASONS.iter().map(|r| Some(*r)))
            .chain([
                Some(store::REASON_USER),
                Some(store::REASON_FROZEN),
                Some(store::REASON_BLACKLIST),
                Some(store::REASON_BLACKLIST_REMOVED),
                Some(store::REASON_DEV_MODE),
                Some("requires:localai"),
                Some("a-reason-from-a-newer-pm"),
            ])
            .collect();
        for state in store::STATES {
            for reason in &reasons {
                let s = of(state, *reason);
                assert!(!s.text.is_empty(), "{state} {reason:?}");
                assert_eq!(s.state, state);
                assert_eq!(s.reason.as_deref(), *reason);
                // A mod is "running" only when it is active and on.
                assert_eq!(s.running, state == "active", "{state} {reason:?}");
            }
        }
        let s = of("paused", Some("a-reason-from-a-newer-pm"));
        assert_eq!((s.kind, s.fix), (Kind::PausedOther, Fix::None));
        let s = of("parked", Some("requires:localai"));
        assert_eq!((s.kind, s.fix), (Kind::Parked, Fix::None));
    }
}
