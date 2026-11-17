// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the Mods tab says about the safety list (Mods spec §1.10): when it was last checked (and
//! "not checked since …" after 30 days), whether this machine's record of it is damaged, and the
//! strips to draw: mods paused, removed, back on, or with an update waiting.
//!
//! The strips are *derived* from the rows, the notes the safety list keeps and the cached index, so
//! a strip is drawn correctly even when the page missed the `mods://notice` event (it may not have
//! been listening yet); the event is only the page's cue to look again. A strip clears itself when
//! the state it describes changes (a paused mod resumed, a removed one uninstalled), except the
//! "back on" strip, which stays until it is dismissed.

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::listing::RawRow;
use crate::mods::index::{self, Index, SafetyList};
use crate::mods::ledger::{Resumed, Waiting, WaitingKind};
use crate::mods::safety::ModNotice;
use crate::mods::store;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SafetyStatus {
    pub checked: Checked,
    /// This machine's record of the safety list is unusable, when it is.
    pub record: Option<RecordFault>,
    /// The index in force, when there is one.
    pub index: Option<IndexInfo>,
    /// The strips to draw, in the shape `mods://notice` sends.
    pub notices: Vec<ModNotice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Checked {
    /// `never` (no good fetch yet), `checked`, or `stale` (none for over 30 days).
    pub kind: &'static str,
    /// The last good fetch, RFC 3339.
    pub at: Option<String>,
    /// "Safety list not checked since 03-11-2026." for `stale`, a plain line for `never`.
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordFault {
    /// `damaged` (only a reset mends it) or `unreadable` (an I/O failure that may mend itself).
    pub kind: &'static str,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexInfo {
    pub seq: u64,
    pub issued: String,
    pub expires: String,
    /// Inside its life: the catalogue installs from it.
    pub current: bool,
}

/// DD-MM-YYYY, the house date.
fn day(t: DateTime<Utc>) -> String {
    t.format("%d-%m-%Y").to_string()
}

/// How fresh the safety list is, in words (pure). The 30-day rule is `index::safety_list`'s.
pub fn checked(last_good_fetch: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Checked {
    match index::safety_list(last_good_fetch, now) {
        SafetyList::NeverChecked => Checked {
            kind: "never",
            at: None,
            text: Some("PM hasn't checked the safety list yet.".to_owned()),
        },
        SafetyList::Checked(t) => Checked {
            kind: "checked",
            at: Some(index::time_text(t)),
            text: None,
        },
        SafetyList::NotCheckedSince(t) => Checked {
            kind: "stale",
            at: Some(index::time_text(t)),
            text: Some(format!("Safety list not checked since {}.", day(t))),
        },
    }
}

pub fn record_fault(fault: Option<(String, bool)>) -> Option<RecordFault> {
    fault.map(|(text, unreadable)| RecordFault {
        kind: if unreadable { "unreadable" } else { "damaged" },
        text,
    })
}

pub fn index_info(index: Option<&Index>, now: DateTime<Utc>) -> Option<IndexInfo> {
    index.map(|i| IndexInfo {
        seq: i.seq,
        issued: index::time_text(i.issued),
        expires: index::time_text(i.expires),
        current: i.current_at(now),
    })
}

/// The strips from the rows and notes (pure), ordered: removed, paused, waiting, back on. Nothing
/// is invented: a paused row the cached index no longer names still gets a strip (with an empty
/// reason, which the page words as "paused by PM's safety list"), because the row says so.
pub fn notices(
    rows: &[RawRow],
    index: Option<&Index>,
    waiting: &std::collections::BTreeMap<String, Waiting>,
    resumed: &[Resumed],
) -> Vec<ModNotice> {
    let named = |row: &RawRow| {
        index.and_then(|i| i.blacklisted(&row.id, &row.version, row.package_sha256.as_deref()))
    };
    let mut out = Vec::new();
    for row in rows {
        if row.state_reason.as_deref() == Some(store::REASON_BLACKLIST_REMOVED) {
            out.push(ModNotice::Removed {
                id: row.id.clone(),
                name: row.name.clone(),
                version: row.version.clone(),
                reason: named(row).map(|e| e.reason.clone()).unwrap_or_default(),
            });
        }
    }
    for row in rows {
        if row.state == store::STATE_PAUSED
            && row.state_reason.as_deref() == Some(store::REASON_BLACKLIST)
        {
            let entry = named(row);
            out.push(ModNotice::Paused {
                id: row.id.clone(),
                name: row.name.clone(),
                version: row.version.clone(),
                reason: entry.map(|e| e.reason.clone()).unwrap_or_default(),
                fixed_in: entry.and_then(|e| e.fixed_in.as_ref().map(|v| v.as_str().to_owned())),
            });
        }
    }
    for row in rows {
        let Some(w) = waiting.get(&row.id) else {
            continue;
        };
        // A note the installed version has already caught up with is stale.
        let stale = semver::Version::parse(&row.version)
            .ok()
            .zip(semver::Version::parse(&w.version).ok())
            .is_some_and(|(have, want)| have >= want);
        if stale {
            continue;
        }
        out.push(match w.kind {
            WaitingKind::Approval => ModNotice::UpdateWaiting {
                id: row.id.clone(),
                name: row.name.clone(),
                version: w.version.clone(),
            },
            WaitingKind::NewerPm => ModNotice::UpdateNeedsNewerPm {
                id: row.id.clone(),
                name: row.name.clone(),
                version: w.version.clone(),
                needs_api: w.needs_api.clone().unwrap_or_default(),
            },
        });
    }
    for r in resumed {
        // Only for a mod that is still installed and on.
        if let Some(row) = rows
            .iter()
            .find(|row| row.id == r.id && row.state == store::STATE_ACTIVE)
        {
            out.push(ModNotice::Resumed {
                id: row.id.clone(),
                name: row.name.clone(),
                version: r.version.clone(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::safety::tests::{index_with, now};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn row(id: &str, state: &str, reason: Option<&str>) -> RawRow {
        RawRow {
            id: id.into(),
            name: id.to_uppercase(),
            version: "1.0.0".into(),
            tier: "first".into(),
            author: "Atlas".into(),
            source_repo: "r".into(),
            source_commit: "0".repeat(40),
            signed: true,
            installed_from: "catalogue".into(),
            package_sha256: None,
            manifest: "{}".into(),
            files: "[]".into(),
            state: state.into(),
            state_reason: reason.map(str::to_owned),
            consented: true,
            installed_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn at(s: &str) -> DateTime<Utc> {
        index::parse_time(s).unwrap()
    }

    #[test]
    fn the_safety_list_is_never_checked_checked_or_stale_after_thirty_days() {
        let n = at("2026-12-31T12:00:00Z");
        let never = checked(None, n);
        assert_eq!((never.kind, never.at.as_deref()), ("never", None));
        assert!(never.text.unwrap().contains("hasn't checked"));
        let fresh = checked(Some(at("2026-12-25T00:00:00Z")), n);
        assert_eq!((fresh.kind, fresh.text), ("checked", None));
        // Exactly thirty days is still fine; past it, it says since when, in the house date.
        let edge = checked(Some(n - chrono::Duration::days(30)), n);
        assert_eq!(edge.kind, "checked");
        let stale = checked(Some(at("2026-11-03T08:00:00Z")), n);
        assert_eq!(stale.kind, "stale");
        assert_eq!(
            stale.text.as_deref(),
            Some("Safety list not checked since 03-11-2026.")
        );
        assert_eq!(stale.at.as_deref(), Some("2026-11-03T08:00:00Z"));
    }

    #[test]
    fn a_record_fault_says_damaged_or_unreadable_and_nothing_when_fine() {
        assert_eq!(record_fault(None), None);
        let damaged = record_fault(Some(("it is damaged".into(), false))).unwrap();
        assert_eq!(
            (damaged.kind, damaged.text.as_str()),
            ("damaged", "it is damaged")
        );
        assert_eq!(
            record_fault(Some(("eio".into(), true))).unwrap().kind,
            "unreadable"
        );
    }

    #[test]
    fn the_index_in_force_says_whether_it_can_still_install() {
        let i = index_with(|_| {});
        let info = index_info(Some(&i), now()).unwrap();
        assert!(info.current && info.seq == i.seq);
        let late = index_info(Some(&i), i.expires + chrono::Duration::days(3)).unwrap();
        assert!(!late.current);
        assert!(index_info(None, now()).is_none());
    }

    #[test]
    fn the_strips_come_from_the_rows_with_the_lists_words_and_clear_with_the_state() {
        let index = index_with(|v| {
            v["blacklist"] = json!([
                {"id": "atlas.map", "versions": "=1.0.0", "action": "paused",
                 "reason": "It loses notes.", "fixedIn": "1.0.1"},
                {"id": "acme.bad", "versions": "*", "action": "removed", "reason": "It phones home."},
            ]);
        });
        let rows = [
            row("atlas.map", "paused", Some("blacklist")),
            row("acme.bad", "paused", Some("blacklist-removed")),
            row("acme.mine", "paused", Some("user")),
            row("acme.fine", "active", None),
            row("acme.back", "active", None),
            row("acme.parked", "parked", Some("files-missing")),
        ];
        let waiting = BTreeMap::from([
            (
                "acme.fine".to_owned(),
                Waiting {
                    version: "1.1.0".into(),
                    sha256: "0".repeat(64),
                    kind: WaitingKind::Approval,
                    widenings: vec![],
                    needs_api: None,
                },
            ),
            (
                "acme.parked".to_owned(),
                Waiting {
                    version: "2.0.0".into(),
                    sha256: "0".repeat(64),
                    kind: WaitingKind::NewerPm,
                    widenings: vec![],
                    needs_api: Some("1.2".into()),
                },
            ),
            // Caught up: no strip.
            (
                "acme.back".to_owned(),
                Waiting {
                    version: "1.0.0".into(),
                    sha256: "0".repeat(64),
                    kind: WaitingKind::Approval,
                    widenings: vec![],
                    needs_api: None,
                },
            ),
        ]);
        let resumed = [
            Resumed {
                id: "acme.back".into(),
                version: "1.0.0".into(),
                at: "t".into(),
            },
            // Its mod was uninstalled or is paused again: no strip.
            Resumed {
                id: "gone.mod".into(),
                version: "1.0.0".into(),
                at: "t".into(),
            },
            Resumed {
                id: "acme.mine".into(),
                version: "1.0.0".into(),
                at: "t".into(),
            },
        ];
        let got = notices(&rows, Some(&index), &waiting, &resumed);
        let kinds: Vec<String> = got
            .iter()
            .map(|n| {
                serde_json::to_value(n).unwrap()["kind"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "removed",
                "paused",
                "updateWaiting",
                "updateNeedsNewerPm",
                "resumed"
            ]
        );
        match &got[0] {
            ModNotice::Removed { id, reason, .. } => {
                assert_eq!(
                    (id.as_str(), reason.as_str()),
                    ("acme.bad", "It phones home.")
                );
            }
            other => panic!("{other:?}"),
        }
        match &got[1] {
            ModNotice::Paused {
                id,
                reason,
                fixed_in,
                ..
            } => {
                assert_eq!(id, "atlas.map");
                assert_eq!(reason, "It loses notes.");
                assert_eq!(fixed_in.as_deref(), Some("1.0.1"));
            }
            other => panic!("{other:?}"),
        }
        assert!(
            matches!(&got[3], ModNotice::UpdateNeedsNewerPm { needs_api, .. } if needs_api == "1.2")
        );
        assert!(matches!(&got[4], ModNotice::Resumed { id, .. } if id == "acme.back"));
    }

    #[test]
    fn a_paused_row_the_index_no_longer_names_still_gets_a_strip_with_no_reason() {
        let rows = [row("atlas.map", "paused", Some("blacklist"))];
        let got = notices(&rows, None, &BTreeMap::new(), &[]);
        assert!(
            matches!(&got[..], [ModNotice::Paused { reason, fixed_in: None, .. }] if reason.is_empty())
        );
    }
}
