// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the accepted index decides about a mod (Mods spec §1.2, §1.10, plan decisions 10 and 11):
//! the [`CatalogueCheck`] an install, an update or a repair asks, the [`ServeCheck`] the startup
//! load asks of every row before a frame can mount, the check for an unsigned file, and the pure
//! plan for applying the blacklist to an installed row. All of it is decision logic over an
//! [`Index`]; reading the cached index, writing rows and telling the page are the callers'
//! ([`super::refresh`], `ledger/blacklist.rs`).
//!
//! **What the index may do to an installed row** ([`plan_row`]). It only ever *restricts*, with one
//! exception, a resume, and that one needs a current index:
//! - A pause entry matching the row's id and version (or its package sha256) pauses it, data kept.
//! - A removal entry removes it: switched off, grants revoked, files and cache-class rows deleted;
//!   the row stays, typed `blacklist-removed`, until the user answers the uninstall dialog (I-16).
//! - A row the blacklist paused resumes when no entry matches it any more *and* no pause entry for
//!   its id names a `fixedIn` above its version. So an update to `fixedIn` or later resumes it, an
//!   index that lifts the entry resumes it, and a pre-release of the fix (below `fixedIn`, outside
//!   the range) stays paused. An expired index never resumes anything: it is only trusted for what
//!   it restricts.
//! - A row another owner parked (a closed room) or paused (the user, dev mode) is left alone: it
//!   serves nothing, the owner's reason stays so its stop outlives the entry, and the startup
//!   [`ServeCheck`] holds it should its owner lift the stop while the entry stands. (A removal
//!   still takes any row.)
//!
//! **What an install must agree with.** Catalogue installs (decision 10): the index must be current
//! (an expired one restricts, it doesn't install), list the id at exactly the package's version,
//! and agree with the signed package on sha256, tier, author, source repo, commit, API range and
//! permissions; a *repair* instead re-installs exactly the pinned version and file, whatever the
//! index lists now. A signed `.pmmod` installed from a file (decision 11) must match the index's
//! entry when the index lists that id *and* version; otherwise only the blacklist and the
//! publisher pin (the ledger's) apply, so an offline file install works. Every kind checks the
//! blacklist by id and version range and by sha256.

// The refresher, the updater and the ledger's startup are the callers.
#![cfg_attr(not(test), allow(dead_code))]

use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::index::{BlacklistAction, BlacklistEntry, BlacklistTarget, Index};
use super::index_state::IndexState;
use super::keys::{KeyRing, TrustedKeys};
use super::ledger::{CatalogueCheck, ServeCheck, Source};
use super::package::VerifiedPackage;
use super::store;

/// The keys this machine holds: the two Atlas keys and, in a dev build with `PM_DATA_DIR`, its test
/// key. Built once, so the dev key's log line prints once.
pub fn ring() -> &'static KeyRing {
    static RING: OnceLock<KeyRing> = OnceLock::new();
    RING.get_or_init(KeyRing::machine)
}

/// The keys a signature may be checked against right now: the held keys less what this machine has
/// seen revoked. What every `install_package` is handed (the key-id gate).
pub fn trusted_keys(state: &IndexState) -> TrustedKeys {
    ring().trusted(&state.revoked)
}

/// The words a refusal or a strip gives for an entry.
pub fn blacklist_words(entry: &BlacklistEntry) -> String {
    format!("PM's safety list: {}", entry.reason)
}

// ---- The checks an install and a startup ask -----------------------------------------------------

/// What the catalogue check ties a package to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// The version the index lists as the latest.
    Latest,
    /// Exactly the installed version and file, whatever the index lists now (spec §1.3, "repair").
    Repair { version: String, sha256: String },
}

/// The accepted index, as the ledger's two hooks.
pub struct SafetyCheck<'a> {
    index: Option<&'a Index>,
    now: DateTime<Utc>,
    mode: Mode,
}

impl<'a> SafetyCheck<'a> {
    /// For the startup load and recovery, which only hold blacklisted rows back.
    pub fn serve(index: Option<&'a Index>) -> Self {
        Self::catalogue(index, Utc::now(), Mode::Latest)
    }

    /// For an install, update or repair at `now`.
    pub fn catalogue(index: Option<&'a Index>, now: DateTime<Utc>, mode: Mode) -> Self {
        Self { index, now, mode }
    }
}

impl ServeCheck for SafetyCheck<'_> {
    fn hold(&self, id: &str, version: &str, package_sha256: Option<&str>) -> Option<String> {
        self.index?
            .blacklisted(id, version, package_sha256)
            .map(blacklist_words)
    }
}

impl CatalogueCheck for SafetyCheck<'_> {
    fn check(
        &self,
        pkg: &VerifiedPackage,
        package_sha256: &str,
        source: Source,
    ) -> Result<(), String> {
        let id = pkg.manifest.id.as_str();
        let version = pkg.manifest.version.to_string();
        if let Some(entry) = self
            .index
            .and_then(|i| i.blacklisted(id, &version, Some(package_sha256)))
        {
            return Err(blacklist_words(entry));
        }
        match (source, &self.mode) {
            (Source::Catalogue, Mode::Repair { version: v, sha256 }) => {
                if *v != version || sha256 != package_sha256 {
                    return Err(
                        "a repair puts back exactly the installed version and file".to_owned()
                    );
                }
                Ok(())
            }
            (Source::Catalogue, Mode::Latest) => {
                let index = self.index.ok_or_else(|| {
                    "PM has no safety list yet, so it can't install from the catalogue".to_owned()
                })?;
                if !index.current_at(self.now) {
                    return Err(
                        "PM's safety list is out of date, so it won't install from the catalogue \
                         until it is refreshed"
                            .to_owned(),
                    );
                }
                let entry = index
                    .entry(id)
                    .ok_or_else(|| "it isn't in the catalogue".to_owned())?;
                agree(entry, pkg, package_sha256)
            }
            (Source::File, _) => {
                // Decision 11: where the index lists this id at this version, the file must be
                // that file. Anything else it lists (another version) says nothing about it.
                match self.index.and_then(|i| i.entry(id)) {
                    Some(entry) if entry.version.as_str() == version => {
                        agree(entry, pkg, package_sha256)
                    }
                    _ => Ok(()),
                }
            }
        }
    }
}

/// The signed package against its catalogue entry: the same file and the same claims.
fn agree(
    entry: &super::index::CatalogueEntry,
    pkg: &VerifiedPackage,
    package_sha256: &str,
) -> Result<(), String> {
    let m = &pkg.manifest;
    let version = m.version.to_string();
    let differs = |what: &str| Err(format!("it isn't the catalogue's file: its {what} differs"));
    if entry.version.as_str() != version {
        return differs("version");
    }
    if entry.sha256 != package_sha256 {
        return differs("sha256");
    }
    if entry.tier != m.publisher.tier {
        return differs("tier");
    }
    if entry.author != m.publisher.author {
        return differs("author");
    }
    if entry.source_repo != m.publisher.source_repo {
        return differs("source repo");
    }
    if entry.commit != m.publisher.commit {
        return differs("source commit");
    }
    if entry.api != m.api {
        return differs("API range");
    }
    let mut permissions: Vec<&str> = m.permissions.iter().map(|p| p.id.as_str()).collect();
    permissions.sort_unstable();
    if permissions
        != entry
            .permissions
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    {
        return differs("permissions");
    }
    Ok(())
}

/// The check for an unsigned file (spec §1.1, §1.10), for slice 6's install path: its id may not be
/// one the catalogue lists (the catalogue's mod is the signed one), and nothing on the blacklist
/// may match it, by id and version or by its sha256, so a known-bad unsigned file is refused too.
pub fn unsigned_check(
    index: Option<&Index>,
    id: &str,
    version: &str,
    sha256: &str,
) -> Result<(), String> {
    let Some(index) = index else {
        return Ok(());
    };
    if let Some(entry) = index.blacklisted(id, version, Some(sha256)) {
        return Err(blacklist_words(entry));
    }
    if index.entry(id).is_some() {
        return Err(
            "its id belongs to a mod in the catalogue; only that signed mod may use it".to_owned(),
        );
    }
    Ok(())
}

// ---- Applying the blacklist to an installed row (pure) -------------------------------------------

/// What the plan reads of a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowView {
    pub id: String,
    pub name: String,
    pub version: String,
    pub package_sha256: Option<String>,
    pub state: String,
    pub reason: Option<String>,
}

impl RowView {
    fn removed(&self) -> bool {
        self.reason.as_deref() == Some(store::REASON_BLACKLIST_REMOVED)
    }

    fn blacklist_paused(&self) -> bool {
        self.state == store::STATE_PAUSED && self.reason.as_deref() == Some(store::REASON_BLACKLIST)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Keep,
    Pause {
        reason: String,
        fixed_in: Option<String>,
    },
    Remove {
        reason: String,
    },
    Resume,
    /// A row already removed: make sure its files are gone (a crash may have left them).
    FinishRemoval,
}

/// Whether a pause entry for `id` names a fix above `version`: the mod is outside every range but
/// the fix hasn't arrived (a pre-release between the range's top and `fixedIn`).
fn awaiting_fix(index: &Index, id: &str, version: &str) -> bool {
    let Ok(version) = semver::Version::parse(version) else {
        return true;
    };
    index.blacklist.iter().any(|e| {
        matches!(&e.target, BlacklistTarget::Versions { id: eid, .. } if eid.as_str() == id)
            && e.action == BlacklistAction::Paused
            && e.fixed_in
                .as_ref()
                .and_then(|f| semver::Version::parse(f.as_str()).ok())
                .is_some_and(|f| f > version)
    })
}

/// What `index` does to `row` at `now` (module doc).
pub fn plan_row(row: &RowView, index: &Index, now: DateTime<Utc>) -> Action {
    if row.state == store::STATE_INSTALLING {
        return Action::Keep;
    }
    if row.removed() {
        return Action::FinishRemoval;
    }
    let hit = index.blacklisted(&row.id, &row.version, row.package_sha256.as_deref());
    match hit {
        Some(e) if e.action == BlacklistAction::Removed => Action::Remove {
            reason: e.reason.clone(),
        },
        Some(e) => {
            // Another owner's stop is theirs to lift. Taking it over would overwrite its reason, and
            // when the entry lifted the row would resume to `active`, switching back on a mod the
            // user (or dev mode) had paused. It serves nothing meanwhile, and the startup
            // [`ServeCheck`] holds it should its owner lift the stop while the entry stands.
            // (Slice 6 seam: that covers a restart, not a live resume. The resume path must ask
            // the check itself; see the note on `ledger::ServeCheck`.)
            let other_owners_park = row.state == store::STATE_PARKED
                && !row.reason.as_deref().is_some_and(store::is_ledger_reason);
            let other_owners_pause = row.state == store::STATE_PAUSED && !row.blacklist_paused();
            if row.blacklist_paused() || other_owners_park || other_owners_pause {
                Action::Keep
            } else {
                Action::Pause {
                    reason: e.reason.clone(),
                    fixed_in: e.fixed_in.as_ref().map(|v| v.as_str().to_owned()),
                }
            }
        }
        None if row.blacklist_paused()
            && index.current_at(now)
            && !awaiting_fix(index, &row.id, &row.version) =>
        {
            Action::Resume
        }
        None => Action::Keep,
    }
}

// ---- What the page is told -----------------------------------------------------------------------

/// The event the page listens to (`mods://notice`), carrying what slice 6's strips say.
pub const NOTICE_EVENT: &str = "mods://notice";

/// One thing the safety list did, for a strip. The words (`reason`) are the index's, plain text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ModNotice {
    /// "X is paused while we fix a problem: <reason>. Your data is safe."
    Paused {
        id: String,
        name: String,
        version: String,
        reason: String,
        fixed_in: Option<String>,
    },
    /// "X is back on, fixed in version N."
    Resumed {
        id: String,
        name: String,
        version: String,
    },
    /// "X was removed: <reason>", with the uninstall dialog for what it made and stored.
    Removed {
        id: String,
        name: String,
        version: String,
        reason: String,
    },
    /// An update asks for more than was granted and waits for the user.
    UpdateWaiting {
        id: String,
        name: String,
        version: String,
    },
    /// "Update PM to get version N": the newest version needs a mod API this PM doesn't have.
    UpdateNeedsNewerPm {
        id: String,
        name: String,
        version: String,
        /// The API (`M.m`) it needs at least.
        needs_api: String,
    },
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::mods::index::tests::{good_entry, good_index, verified_by, Keys, SHA};
    use crate::mods::index::{package_url, Index};
    use crate::mods::package::tests::signed_package;
    use crate::mods::package::{self, VerifiedPackage};
    use serde_json::{json, Value};

    pub(crate) const NOW: &str = "2027-01-15T00:00:00Z";

    pub(crate) fn now() -> DateTime<Utc> {
        crate::mods::index::parse_time(NOW).unwrap()
    }

    /// An index (seq 5) with `change` applied to its JSON.
    pub(crate) fn index_with(change: impl FnOnce(&mut Value)) -> Index {
        let keys = Keys::new();
        let mut v = good_index(5);
        change(&mut v);
        verified_by(&v, &keys.main, &keys).index
    }

    /// The fixture package and its signer, parsed.
    pub(crate) fn verified(version: &str) -> (VerifiedPackage, String) {
        let (bytes, key) = signed_package(|m, _| m["version"] = Value::String(version.into()));
        let pkg = package::verify(&bytes, &[key]).unwrap();
        let sha = hex::encode(sha2::Sha256::digest(&bytes));
        (pkg, sha)
    }
    use sha2::Digest;

    /// A catalogue entry that agrees with `pkg`.
    pub(crate) fn entry_for(pkg: &VerifiedPackage, sha: &str) -> Value {
        let m = &pkg.manifest;
        let mut permissions: Vec<&str> = m.permissions.iter().map(|p| p.id.as_str()).collect();
        permissions.sort_unstable();
        let version = crate::mods::disk::VersionName::parse(&m.version.to_string()).unwrap();
        let mut e = good_entry();
        e["version"] = json!(m.version.to_string());
        e["sha256"] = json!(sha);
        e["permissions"] = json!(permissions);
        e["api"] = json!(m.api.comment_form());
        e["url"] = json!(package_url(&m.id, &version));
        e["author"] = json!(m.publisher.author);
        e["sourceRepo"] = json!(m.publisher.source_repo);
        e["commit"] = json!(m.publisher.commit);
        e
    }

    /// An index whose catalogue is exactly `entry` and whose blacklist is `blacklist`.
    pub(crate) fn index_listing(entry: Value, blacklist: Value) -> Index {
        index_with(|v| {
            v["mods"] = json!([entry]);
            v["blacklist"] = blacklist;
        })
    }

    fn check(
        index: Option<&Index>,
        pkg: &VerifiedPackage,
        sha: &str,
        source: Source,
        mode: Mode,
    ) -> Result<(), String> {
        SafetyCheck::catalogue(index, now(), mode).check(pkg, sha, source)
    }

    // ---- CatalogueCheck ------------------------------------------------------------------------

    #[test]
    fn a_catalogue_install_must_agree_with_the_current_indexs_entry() {
        let (pkg, sha) = verified("1.2.0");
        let index = index_listing(entry_for(&pkg, &sha), json!([]));
        assert!(check(Some(&index), &pkg, &sha, Source::Catalogue, Mode::Latest).is_ok());

        // Each claim that differs refuses, and says which.
        for (field, value, word) in [
            ("sha256", json!(SHA), "sha256"),
            ("author", json!("Someone Else"), "author"),
            ("sourceRepo", json!("evil/Personal-Manager"), "source repo"),
            (
                "commit",
                json!("ffffffffffffffffffffffffffffffffffffffff"),
                "source commit",
            ),
            ("permissions", json!(["documents.meta"]), "permissions"),
        ] {
            let mut e = entry_for(&pkg, &sha);
            e[field] = value;
            let index = index_listing(e, json!([]));
            let err = check(Some(&index), &pkg, &sha, Source::Catalogue, Mode::Latest).unwrap_err();
            assert!(err.contains(word), "{field}: {err}");
        }
        // A different tier (and so a different id family) is the one the index can't even express
        // for this id; the version and the api range are covered here.
        let mut e = entry_for(&pkg, &sha);
        e["api"] = json!("1.0-1.1");
        let index = index_listing(e, json!([]));
        let err = check(Some(&index), &pkg, &sha, Source::Catalogue, Mode::Latest).unwrap_err();
        assert!(err.contains("API range"), "{err}");
        // The index lists another version as the latest: this one is not what it vouches for.
        let (older, older_sha) = verified("1.1.0");
        let index = index_listing(entry_for(&pkg, &sha), json!([]));
        let err = check(
            Some(&index),
            &older,
            &older_sha,
            Source::Catalogue,
            Mode::Latest,
        )
        .unwrap_err();
        assert!(err.contains("version"), "{err}");
    }

    #[test]
    fn a_catalogue_install_needs_a_current_index_that_lists_the_id() {
        let (pkg, sha) = verified("1.2.0");
        let index = index_listing(entry_for(&pkg, &sha), json!([]));
        // No index at all.
        let err = check(None, &pkg, &sha, Source::Catalogue, Mode::Latest).unwrap_err();
        assert!(err.contains("no safety list"), "{err}");
        // An expired one (decision 10): it restricts, it doesn't install.
        let late = crate::mods::index::parse_time("2027-02-01T00:00:00Z").unwrap();
        let err = SafetyCheck::catalogue(Some(&index), late, Mode::Latest)
            .check(&pkg, &sha, Source::Catalogue)
            .unwrap_err();
        assert!(err.contains("out of date"), "{err}");
        // Two days of skew past `expires` is still current.
        let skew = crate::mods::index::parse_time("2027-02-01T00:00:00Z").unwrap()
            - chrono::Duration::days(1);
        assert!(SafetyCheck::catalogue(Some(&index), skew, Mode::Latest)
            .check(&pkg, &sha, Source::Catalogue)
            .is_ok());
        // An id the catalogue doesn't list.
        let other = index_with(|v| v["mods"] = json!([]));
        let err = check(Some(&other), &pkg, &sha, Source::Catalogue, Mode::Latest).unwrap_err();
        assert!(err.contains("isn't in the catalogue"), "{err}");
    }

    #[test]
    fn a_repair_is_exactly_the_pinned_version_and_file_whatever_the_index_lists() {
        let (pkg, sha) = verified("1.0.0");
        // The index has moved on to 1.2.0 (not this package's entry at all).
        let (newer, newer_sha) = verified("1.2.0");
        let index = index_listing(entry_for(&newer, &newer_sha), json!([]));
        let repair = Mode::Repair {
            version: "1.0.0".into(),
            sha256: sha.clone(),
        };
        assert!(check(Some(&index), &pkg, &sha, Source::Catalogue, repair.clone()).is_ok());
        // Offline-first: a repair needs no index.
        assert!(check(None, &pkg, &sha, Source::Catalogue, repair.clone()).is_ok());
        // Not the pinned file, or not the pinned version.
        let err = check(Some(&index), &pkg, SHA, Source::Catalogue, repair.clone()).unwrap_err();
        assert!(err.contains("exactly the installed"), "{err}");
        let err = check(Some(&index), &newer, &newer_sha, Source::Catalogue, repair).unwrap_err();
        assert!(err.contains("exactly the installed"), "{err}");
    }

    #[test]
    fn a_repair_re_checks_the_blacklist() {
        let (pkg, sha) = verified("1.0.0");
        let index = index_with(|v| {
            v["blacklist"] = json!([{"id": "atlas.map", "versions": "=1.0.0", "action": "paused",
                "reason": "It loses notes.", "fixedIn": "1.0.1"}]);
        });
        let repair = Mode::Repair {
            version: "1.0.0".into(),
            sha256: sha.clone(),
        };
        let err = check(Some(&index), &pkg, &sha, Source::Catalogue, repair).unwrap_err();
        assert!(err.contains("It loses notes."), "{err}");
    }

    #[test]
    fn every_kind_of_install_refuses_a_blacklisted_mod_by_range_and_by_sha() {
        let (pkg, sha) = verified("1.2.0");
        let by_range = index_with(|v| {
            v["mods"] = json!([entry_for_json(&pkg, &sha)]);
            v["blacklist"] = json!([{"id": "atlas.map", "versions": "<2.0.0", "action": "removed",
                "reason": "Sends your documents elsewhere."}]);
        });
        let by_sha = index_with(|v| {
            v["mods"] = json!([entry_for_json(&pkg, &sha)]);
            v["blacklist"] = json!([{"sha256": sha, "action": "paused", "reason": "Bad file."}]);
        });
        for index in [&by_range, &by_sha] {
            for source in [Source::Catalogue, Source::File] {
                let err = check(Some(index), &pkg, &sha, source, Mode::Latest).unwrap_err();
                assert!(err.starts_with("PM's safety list: "), "{err}");
            }
        }
        // Past the range, and not that sha, it passes.
        let (fixed, fixed_sha) = verified("2.0.0");
        let ok = index_with(|v| {
            v["mods"] = json!([entry_for_json(&fixed, &fixed_sha)]);
            v["blacklist"] = json!([{"id": "atlas.map", "versions": "<2.0.0", "action": "removed",
                "reason": "x"}]);
        });
        assert!(check(
            Some(&ok),
            &fixed,
            &fixed_sha,
            Source::Catalogue,
            Mode::Latest
        )
        .is_ok());
    }

    fn entry_for_json(pkg: &VerifiedPackage, sha: &str) -> Value {
        entry_for(pkg, sha)
    }

    #[test]
    fn a_signed_file_must_match_the_index_only_where_the_index_lists_that_version() {
        let (pkg, sha) = verified("1.2.0");
        let index = index_listing(entry_for(&pkg, &sha), json!([]));
        // Listed id and version, the very file: fine. Another file with that version: refused.
        assert!(check(Some(&index), &pkg, &sha, Source::File, Mode::Latest).is_ok());
        let (tampered, tampered_sha) = signed_other_file("1.2.0");
        let err = check(
            Some(&index),
            &tampered,
            &tampered_sha,
            Source::File,
            Mode::Latest,
        )
        .unwrap_err();
        assert!(err.contains("sha256"), "{err}");
        // The index lists another version of the id: only the blacklist and the pin apply.
        let (other_version, other_sha) = signed_other_file("1.1.0");
        assert!(check(
            Some(&index),
            &other_version,
            &other_sha,
            Source::File,
            Mode::Latest
        )
        .is_ok());
        // No index at all (a fresh install, offline): a file install still works.
        assert!(check(None, &tampered, &tampered_sha, Source::File, Mode::Latest).is_ok());
        // An expired index still vouches for what it lists.
        let late = crate::mods::index::parse_time("2028-01-01T00:00:00Z").unwrap();
        assert!(SafetyCheck::catalogue(Some(&index), late, Mode::Latest)
            .check(&tampered, &tampered_sha, Source::File)
            .is_err());
    }

    /// The fixture at `version` with a different file in it: another sha256, same claims.
    fn signed_other_file(version: &str) -> (VerifiedPackage, String) {
        let (bytes, key) = signed_package(|m, files| {
            m["version"] = Value::String(version.into());
            files.insert("app/main.js".into(), b"console.log('other');".to_vec());
        });
        let pkg = package::verify(&bytes, &[key]).unwrap();
        (pkg, hex::encode(sha2::Sha256::digest(&bytes)))
    }

    // ---- The unsigned check --------------------------------------------------------------------

    #[test]
    fn an_unsigned_file_may_not_borrow_a_catalogue_id_or_match_the_blacklist() {
        let (pkg, sha) = verified("1.2.0");
        let index = index_listing(
            entry_for(&pkg, &sha),
            json!([
                {"id": "acme.notes", "versions": "<1.2.0", "action": "paused", "reason": "Loses notes.", "fixedIn": "1.2.0"},
                {"sha256": SHA, "action": "removed", "reason": "Sends your documents elsewhere."}
            ]),
        );
        // A catalogue id: refused, whatever the file.
        let err = unsigned_check(Some(&index), "atlas.map", "9.9.9", &"c".repeat(64)).unwrap_err();
        assert!(err.contains("catalogue"), "{err}");
        // A known-bad file by sha, under any id and version.
        let err = unsigned_check(Some(&index), "my.thing", "1.0.0", SHA).unwrap_err();
        assert!(err.contains("Sends your documents elsewhere."), "{err}");
        // A blacklisted id and version, with a different file.
        let err = unsigned_check(Some(&index), "acme.notes", "1.0.0", &"d".repeat(64)).unwrap_err();
        assert!(err.contains("Loses notes."), "{err}");
        // An ordinary file; and no index at all.
        assert!(unsigned_check(Some(&index), "acme.notes", "1.2.0", &"d".repeat(64)).is_ok());
        assert!(unsigned_check(Some(&index), "my.thing", "1.0.0", &"d".repeat(64)).is_ok());
        assert!(unsigned_check(None, "atlas.map", "1.0.0", SHA).is_ok());
    }

    // ---- ServeCheck ----------------------------------------------------------------------------

    #[test]
    fn the_serve_check_holds_a_blacklisted_row_by_range_or_sha_and_nothing_else() {
        let index = index_with(|v| {
            v["blacklist"] = json!([
                {"id": "acme.notes", "versions": ">=1.0.0, <1.2.0", "action": "paused", "reason": "Loses notes.", "fixedIn": "1.2.0"},
                {"sha256": SHA, "action": "removed", "reason": "Bad file."}
            ]);
        });
        let check = SafetyCheck::serve(Some(&index));
        let held = check.hold("acme.notes", "1.1.0", Some(&"e".repeat(64)));
        assert_eq!(held.as_deref(), Some("PM's safety list: Loses notes."));
        assert_eq!(
            check.hold("any.mod", "9.0.0", Some(SHA)).as_deref(),
            Some("PM's safety list: Bad file.")
        );
        assert_eq!(
            check.hold("acme.notes", "1.2.0", Some(&"e".repeat(64))),
            None
        );
        assert!(
            check.hold("acme.notes", "1.1.0", None).is_some(),
            "no sha at all: the range still holds it"
        );
        assert_eq!(check.hold("any.mod", "1.0.0", None), None);
        // No cached index: nothing is held.
        assert_eq!(
            SafetyCheck::serve(None).hold("acme.notes", "1.1.0", None),
            None
        );
    }

    // ---- The plan for an installed row ---------------------------------------------------------

    fn row(state: &str, reason: Option<&str>, version: &str) -> RowView {
        RowView {
            id: "acme.notes".into(),
            name: "Notes".into(),
            version: version.into(),
            package_sha256: Some("e".repeat(64)),
            state: state.into(),
            reason: reason.map(str::to_owned),
        }
    }

    fn pause_entry(range: &str, fixed: Option<&str>) -> Value {
        let mut e = json!({"id": "acme.notes", "versions": range, "action": "paused",
            "reason": "Loses notes."});
        if let Some(f) = fixed {
            e["fixedIn"] = json!(f);
        }
        e
    }

    fn removal_entry(range: &str) -> Value {
        json!({"id": "acme.notes", "versions": range, "action": "removed",
            "reason": "Sends your documents elsewhere."})
    }

    fn with_blacklist(list: Vec<Value>) -> Index {
        index_with(|v| v["blacklist"] = json!(list))
    }

    #[test]
    fn a_matching_pause_pauses_an_active_row_with_the_indexs_words_and_fix() {
        let index = with_blacklist(vec![pause_entry(">=1.0.0, <1.2.0", Some("1.2.0"))]);
        assert_eq!(
            plan_row(&row("active", None, "1.1.0"), &index, now()),
            Action::Pause {
                reason: "Loses notes.".into(),
                fixed_in: Some("1.2.0".into())
            }
        );
        // A row the user (or dev mode) paused stays theirs: the blacklist must not overwrite the
        // reason, or lifting the entry would resume a mod they switched off.
        for reason in [Some("user"), Some("dev-mode"), None] {
            assert_eq!(
                plan_row(&row("paused", reason, "1.1.0"), &index, now()),
                Action::Keep,
                "{reason:?}"
            );
        }
        // A row the ledger parked for its files is paused too.
        assert!(matches!(
            plan_row(
                &row("parked", Some(store::REASON_FILES_MISSING), "1.1.0"),
                &index,
                now()
            ),
            Action::Pause { .. }
        ));
        // Already paused by it, or parked by another owner (a closed room): left as it is.
        assert_eq!(
            plan_row(&row("paused", Some("blacklist"), "1.1.0"), &index, now()),
            Action::Keep
        );
        assert_eq!(
            plan_row(&row("parked", Some("room-closed"), "1.1.0"), &index, now()),
            Action::Keep
        );
        // An install in flight is never touched; a version outside the range isn't matched.
        assert_eq!(
            plan_row(&row("installing", None, "1.1.0"), &index, now()),
            Action::Keep
        );
        assert_eq!(
            plan_row(&row("active", None, "1.2.0"), &index, now()),
            Action::Keep
        );
    }

    #[test]
    fn a_removal_removes_whatever_state_the_row_is_in_and_wins_over_a_pause() {
        let index = with_blacklist(vec![pause_entry("*", None), removal_entry("<2.0.0")]);
        for (state, reason) in [
            ("active", None),
            ("paused", Some("user")),
            ("paused", Some("blacklist")),
            ("parked", Some("room-closed")),
        ] {
            assert_eq!(
                plan_row(&row(state, reason, "1.0.0"), &index, now()),
                Action::Remove {
                    reason: "Sends your documents elsewhere.".into()
                },
                "{state} {reason:?}"
            );
        }
        // By sha, too.
        let by_sha = with_blacklist(vec![
            json!({"sha256": "e".repeat(64), "action": "removed", "reason": "Bad file."}),
        ]);
        assert!(matches!(
            plan_row(&row("active", None, "7.0.0"), &by_sha, now()),
            Action::Remove { .. }
        ));
        // A removed row stays removed: its files are only made sure of.
        assert_eq!(
            plan_row(
                &row("paused", Some("blacklist-removed"), "1.0.0"),
                &with_blacklist(vec![]),
                now()
            ),
            Action::FinishRemoval
        );
    }

    #[test]
    fn a_paused_row_resumes_when_no_entry_matches_and_the_fix_has_arrived() {
        let paused = |v| row("paused", Some("blacklist"), v);
        // Updated to fixedIn: no entry matches.
        let index = with_blacklist(vec![pause_entry(">=1.0.0, <1.2.0", Some("1.2.0"))]);
        assert_eq!(plan_row(&paused("1.1.0"), &index, now()), Action::Keep);
        assert_eq!(plan_row(&paused("1.2.0"), &index, now()), Action::Resume);
        assert_eq!(plan_row(&paused("1.3.0"), &index, now()), Action::Resume);
        // A pre-release of the fix sits above the range and below fixedIn: not fixed yet.
        assert_eq!(
            plan_row(&paused("1.2.0-beta.1"), &index, now()),
            Action::Keep
        );
        // The entry lifted altogether (a false positive): it resumes.
        assert_eq!(
            plan_row(&paused("1.1.0"), &with_blacklist(vec![]), now()),
            Action::Resume
        );
        // A pause with no fix named: it resumes only once nothing matches.
        let open = with_blacklist(vec![pause_entry("<2.0.0", None)]);
        assert_eq!(plan_row(&paused("1.9.0"), &open, now()), Action::Keep);
        assert_eq!(plan_row(&paused("2.0.0"), &open, now()), Action::Resume);
        // Not the blacklist's pause: never resumed here.
        assert_eq!(
            plan_row(&row("paused", Some("user"), "1.2.0"), &index, now()),
            Action::Keep
        );
        assert_eq!(
            plan_row(&row("paused", None, "1.2.0"), &index, now()),
            Action::Keep
        );
    }

    #[test]
    fn an_expired_index_restricts_but_never_resumes() {
        let index = with_blacklist(vec![pause_entry(">=1.0.0, <1.2.0", Some("1.2.0"))]);
        let late = crate::mods::index::parse_time("2027-03-01T00:00:00Z").unwrap();
        assert!(!index.current_at(late));
        // It still pauses...
        assert!(matches!(
            plan_row(&row("active", None, "1.1.0"), &index, late),
            Action::Pause { .. }
        ));
        // ...but a row that would resume stays paused until a current index says so.
        assert_eq!(
            plan_row(&row("paused", Some("blacklist"), "1.2.0"), &index, late),
            Action::Keep
        );
    }

    #[test]
    fn the_notice_serialises_with_camel_case_fields_and_a_kind() {
        let n = ModNotice::Paused {
            id: "a.b".into(),
            name: "B".into(),
            version: "1.0.0".into(),
            reason: "r".into(),
            fixed_in: Some("1.0.1".into()),
        };
        assert_eq!(
            serde_json::to_value(&n).unwrap(),
            json!({"kind": "paused", "id": "a.b", "name": "B", "version": "1.0.0",
                   "reason": "r", "fixedIn": "1.0.1"})
        );
        let n = ModNotice::UpdateWaiting {
            id: "a.b".into(),
            name: "B".into(),
            version: "2.0.0".into(),
        };
        assert_eq!(serde_json::to_value(&n).unwrap()["kind"], "updateWaiting");
        assert_eq!(NOTICE_EVENT, "mods://notice");
    }
}
