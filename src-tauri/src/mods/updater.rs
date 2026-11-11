// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The background updater and the pinned-version repair download (Mods spec §1.3, plan decision
//! 10). The decisions are pure and sit here; the ledger's half (reading the rows, installing,
//! recording an update that waits) is `ledger/updates.rs`, and [`super::refresh`] runs them after
//! each accepted fetch.
//!
//! **What is updated.** A mod installed from the catalogue, whose accepted index is *current*
//! (an expired index restricts but never installs, decision 10) and lists a newer version of the
//! same publisher, that the blacklist doesn't match, that this PM's API serves, and that isn't
//! already waiting. The download comes from the entry's own URL (which the index format pins to
//! `<prefix><id>-v<version>/<id>-<version>.pmmod`, then [`super::fetch`] pins again), is capped at
//! the package limit, and must have the index's sha256 *before* anything reads it; the verifier,
//! the key-id gate, the publisher pin and the catalogue check (which agrees the file with the
//! entry) follow, in the ledger. Only then is it installed, as an unattended update: one that asks
//! for more than was granted (`Refusal::NeedsApproval`) is not applied, and is recorded as waiting.
//!
//! **What isn't.** A user-paused mod is updated (the pause survives, `state_after_install`), and so
//! is a blacklist-paused one: that is how it gets its fix, and the blacklist then resumes it. A mod
//! the list removed is not (nothing installs onto it), and neither is one installed from a file or
//! a folder.
//!
//! **A repair** puts back exactly the pinned version and file ([`repair_url`]) for a row whose
//! files are missing: the same download path, with the row's own sha256 as the one the file must
//! have. It is the user's click (slice 6), never automatic.

// The refresher and the Mods tab (slice 6) are the callers.
#![cfg_attr(not(test), allow(dead_code))]

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use super::disk::VersionName;
use super::fetch::{fetch_package, Endpoints, FetchError, Transport};
use super::ids::ModId;
use super::index::{package_url, Index};
use super::ledger::Waiting;
use super::policy::{self, ApiFit, MOD_API_OLDEST_MAJOR, MOD_API_VERSION};
use super::store;

/// What the updater reads of an installed row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateRow {
    pub id: String,
    pub version: String,
    pub tier: String,
    pub author: String,
    pub source_repo: String,
    pub installed_from: String,
    pub reason: Option<String>,
}

/// Why a row isn't updated this time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// The index is past its life: it may restrict, it may not install.
    IndexNotCurrent,
    /// Installed from a file or a folder: the catalogue doesn't own it.
    NotFromCatalogue,
    /// The safety list removed it.
    Removed,
    /// The index doesn't list it any more.
    NotListed,
    UpToDate,
    /// The newer version is itself on the blacklist.
    Blacklisted,
    /// The newer version is built for an API major this PM has retired.
    ApiRetired,
    /// The entry's publisher isn't the installed mod's (the ledger would refuse it).
    PublisherChanged,
    /// This very version already asked for approval and is waiting for it.
    AlreadyWaiting,
    /// A version that isn't a semver (a hand-edited row).
    Unreadable,
}

/// A download the updater will make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Download {
    pub id: String,
    pub version: String,
    pub url: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Skip(Skip),
    /// Needs a newer PM ("update PM to get version N"): nothing is downloaded.
    NeedsNewerPm {
        version: String,
        needs_api: String,
        sha256: String,
    },
    Download(Download),
}

/// What to do for each row (`id`, step), against `index` at `now`. `waiting` is the updates that
/// already asked for approval, so each is asked once, not every six hours.
pub fn plan(
    rows: &[UpdateRow],
    waiting: &BTreeMap<String, Waiting>,
    index: &Index,
    now: DateTime<Utc>,
) -> Vec<(String, Step)> {
    rows.iter()
        .map(|row| (row.id.clone(), step(row, waiting.get(&row.id), index, now)))
        .collect()
}

fn step(row: &UpdateRow, waiting: Option<&Waiting>, index: &Index, now: DateTime<Utc>) -> Step {
    if !index.current_at(now) {
        return Step::Skip(Skip::IndexNotCurrent);
    }
    if row.reason.as_deref() == Some(store::REASON_BLACKLIST_REMOVED) {
        return Step::Skip(Skip::Removed);
    }
    if row.installed_from != store::FROM_CATALOGUE {
        return Step::Skip(Skip::NotFromCatalogue);
    }
    let Some(entry) = index.entry(&row.id) else {
        return Step::Skip(Skip::NotListed);
    };
    let (Ok(have), Ok(latest)) = (
        semver::Version::parse(&row.version),
        semver::Version::parse(entry.version.as_str()),
    ) else {
        return Step::Skip(Skip::Unreadable);
    };
    if latest <= have {
        return Step::Skip(Skip::UpToDate);
    }
    if entry.tier.as_str() != row.tier
        || entry.author != row.author
        || entry.source_repo != row.source_repo
    {
        return Step::Skip(Skip::PublisherChanged);
    }
    if index
        .blacklisted(&row.id, entry.version.as_str(), Some(&entry.sha256))
        .is_some()
    {
        return Step::Skip(Skip::Blacklisted);
    }
    match policy::api_fit(entry.api, MOD_API_VERSION, MOD_API_OLDEST_MAJOR) {
        ApiFit::NeedsNewerPm => {
            return Step::NeedsNewerPm {
                version: entry.version.as_str().to_owned(),
                needs_api: format!("{}.{}", entry.api.min.0, entry.api.min.1),
                sha256: entry.sha256.clone(),
            }
        }
        ApiFit::Retired => return Step::Skip(Skip::ApiRetired),
        ApiFit::Fits | ApiFit::OlderMajor => {}
    }
    if waiting.is_some_and(|w| {
        w.kind == super::ledger::WaitingKind::Approval
            && w.version == entry.version.as_str()
            && w.sha256 == entry.sha256
    }) {
        return Step::Skip(Skip::AlreadyWaiting);
    }
    Step::Download(Download {
        id: row.id.clone(),
        version: entry.version.as_str().to_owned(),
        url: entry.url.clone(),
        sha256: entry.sha256.clone(),
    })
}

/// A download's outcome: the bytes (their sha256 already the listed one), or why not.
#[derive(Debug)]
pub struct Downloaded {
    pub download: Download,
    pub bytes: Result<Vec<u8>, FetchError>,
}

/// Fetch every planned download, one at a time (a mod is a few hundred KB, and one slow line
/// shouldn't open six connections), each capped, timed and checked against the index's sha256.
pub async fn download_all<T: Transport>(
    transport: &T,
    endpoints: &Endpoints,
    downloads: Vec<Download>,
) -> Vec<Downloaded> {
    let mut out = Vec::with_capacity(downloads.len());
    for download in downloads {
        let bytes = fetch_package(transport, endpoints, &download.url, &download.sha256).await;
        out.push(Downloaded { download, bytes });
    }
    out
}

/// The URL of the pinned version's package, which is where its repair comes from: derived from the
/// id and version exactly as the index derives an entry's, so a version the index no longer lists
/// can still be put back.
pub fn repair_url(id: &ModId, version: &str) -> Result<String, String> {
    let version = VersionName::parse(version).map_err(|e| e.to_string())?;
    Ok(package_url(id, &version))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::fetch::Hop;
    use crate::mods::index::tests::{good_entry, good_index, verified_by, Keys};
    use crate::mods::ledger::WaitingKind;
    use serde_json::{json, Value};
    use std::collections::VecDeque;
    use std::future::Future;
    use std::sync::Mutex;

    fn t(s: &str) -> DateTime<Utc> {
        crate::mods::index::parse_time(s).unwrap()
    }

    const NOW: &str = "2027-01-15T00:00:00Z";

    /// An index whose one entry, `atlas.map`, is at `version`, with `change` applied to the entry.
    fn index_at(version: &str, change: impl FnOnce(&mut Value)) -> Index {
        let keys = Keys::new();
        let mut v = good_index(5);
        let mut e = good_entry();
        e["version"] = json!(version);
        e["url"] = json!(format!(
            "https://github.com/itsAtlas-org/PM-Mods/releases/download/atlas.map-v{version}/atlas.map-{version}.pmmod"
        ));
        change(&mut e);
        v["mods"] = json!([e]);
        verified_by(&v, &keys.main, &keys).index
    }

    fn row(version: &str) -> UpdateRow {
        let e = good_entry();
        UpdateRow {
            id: "atlas.map".into(),
            version: version.into(),
            tier: e["tier"].as_str().unwrap().into(),
            author: e["author"].as_str().unwrap().into(),
            source_repo: e["sourceRepo"].as_str().unwrap().into(),
            installed_from: "catalogue".into(),
            reason: None,
        }
    }

    fn step_for(row: &UpdateRow, index: &Index) -> Step {
        plan(std::slice::from_ref(row), &BTreeMap::new(), index, t(NOW))
            .remove(0)
            .1
    }

    #[test]
    fn a_newer_listed_version_of_a_catalogue_mod_is_downloaded_from_its_entry() {
        let index = index_at("1.2.0", |_| {});
        match step_for(&row("1.0.0"), &index) {
            Step::Download(d) => {
                assert_eq!((d.id.as_str(), d.version.as_str()), ("atlas.map", "1.2.0"));
                assert!(d.url.ends_with("/atlas.map-v1.2.0/atlas.map-1.2.0.pmmod"));
                assert_eq!(d.sha256, good_entry()["sha256"].as_str().unwrap());
            }
            other => panic!("{other:?}"),
        }
        // A pre-release of the installed version is older; the same version is up to date.
        assert_eq!(step_for(&row("1.2.0"), &index), Step::Skip(Skip::UpToDate));
        assert_eq!(
            step_for(&row("1.3.0"), &index),
            Step::Skip(Skip::UpToDate),
            "an update never downgrades"
        );
    }

    #[test]
    fn nothing_installs_from_an_expired_index_or_onto_a_removed_or_foreign_row() {
        let index = index_at("1.2.0", |_| {});
        let late = t("2027-03-01T00:00:00Z");
        assert!(!index.current_at(late));
        let rows = [row("1.0.0")];
        assert_eq!(
            plan(&rows, &BTreeMap::new(), &index, late)[0].1,
            Step::Skip(Skip::IndexNotCurrent)
        );
        let mut removed = row("1.0.0");
        removed.reason = Some(store::REASON_BLACKLIST_REMOVED.into());
        assert_eq!(step_for(&removed, &index), Step::Skip(Skip::Removed));
        for from in ["file", "folder"] {
            let mut r = row("1.0.0");
            r.installed_from = from.into();
            assert_eq!(step_for(&r, &index), Step::Skip(Skip::NotFromCatalogue));
        }
        let mut other = row("1.0.0");
        other.id = "acme.other".into();
        assert_eq!(step_for(&other, &index), Step::Skip(Skip::NotListed));
        let mut garbled = row("one");
        garbled.id = "atlas.map".into();
        assert_eq!(step_for(&garbled, &index), Step::Skip(Skip::Unreadable));
    }

    #[test]
    fn a_changed_publisher_a_blacklisted_version_and_a_retired_api_are_not_downloaded() {
        let index = index_at("1.2.0", |_| {});
        let mut moved = row("1.0.0");
        moved.author = "Someone Else".into();
        assert_eq!(step_for(&moved, &index), Step::Skip(Skip::PublisherChanged));
        let mut moved = row("1.0.0");
        moved.source_repo = "evil/repo".into();
        assert_eq!(step_for(&moved, &index), Step::Skip(Skip::PublisherChanged));

        // The latest version is itself blacklisted, by range and by file.
        let keys = Keys::new();
        let mut v = good_index(5);
        let mut e = good_entry();
        e["version"] = json!("1.2.0");
        e["url"] = json!("https://github.com/itsAtlas-org/PM-Mods/releases/download/atlas.map-v1.2.0/atlas.map-1.2.0.pmmod");
        v["mods"] = json!([e.clone()]);
        v["blacklist"] = json!([{"id": "atlas.map", "versions": "=1.2.0", "action": "paused",
            "reason": "Bad.", "fixedIn": "1.2.1"}]);
        let by_range = verified_by(&v, &keys.main, &keys).index;
        assert_eq!(
            step_for(&row("1.0.0"), &by_range),
            Step::Skip(Skip::Blacklisted)
        );
        v["blacklist"] = json!([{"sha256": e["sha256"], "action": "removed", "reason": "Bad."}]);
        let by_sha = verified_by(&v, &keys.main, &keys).index;
        assert_eq!(
            step_for(&row("1.0.0"), &by_sha),
            Step::Skip(Skip::Blacklisted)
        );

        // An API major this PM retired is skipped for good.
        let old = index_at("1.2.0", |e| e["api"] = json!("0.1-0.1"));
        assert_eq!(step_for(&row("1.0.0"), &old), Step::Skip(Skip::ApiRetired));
    }

    #[test]
    fn a_newer_api_than_this_pm_has_is_update_pm_not_a_download() {
        let index = index_at("1.2.0", |e| e["api"] = json!("1.1-1.1"));
        match step_for(&row("1.0.0"), &index) {
            Step::NeedsNewerPm {
                version,
                needs_api,
                sha256,
            } => {
                assert_eq!((version.as_str(), needs_api.as_str()), ("1.2.0", "1.1"));
                assert_eq!(sha256, good_entry()["sha256"].as_str().unwrap());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_update_that_already_asked_for_approval_is_not_downloaded_again() {
        let index = index_at("1.2.0", |_| {});
        let sha = good_entry()["sha256"].as_str().unwrap().to_owned();
        let waiting = |version: &str, sha: &str, kind| {
            BTreeMap::from([(
                "atlas.map".to_owned(),
                Waiting {
                    version: version.into(),
                    sha256: sha.into(),
                    kind,
                    widenings: vec![],
                    needs_api: None,
                },
            )])
        };
        let rows = [row("1.0.0")];
        let go = |w: &BTreeMap<String, Waiting>| plan(&rows, w, &index, t(NOW))[0].1.clone();
        assert_eq!(
            go(&waiting("1.2.0", &sha, WaitingKind::Approval)),
            Step::Skip(Skip::AlreadyWaiting)
        );
        // Another version or another file than the one that waits: ask again.
        assert!(matches!(
            go(&waiting("1.1.0", &sha, WaitingKind::Approval)),
            Step::Download(_)
        ));
        assert!(matches!(
            go(&waiting("1.2.0", &"0".repeat(64), WaitingKind::Approval)),
            Step::Download(_)
        ));
        // A note that it needed a newer PM says nothing about whether this PM now fits.
        assert!(matches!(
            go(&waiting("1.2.0", &sha, WaitingKind::NewerPm)),
            Step::Download(_)
        ));
    }

    #[test]
    fn the_repair_url_is_the_one_the_index_derives_and_a_bad_version_has_none() {
        let id = ModId::parse("atlas.map").unwrap();
        assert_eq!(
            repair_url(&id, "1.0.0").unwrap(),
            "https://github.com/itsAtlas-org/PM-Mods/releases/download/atlas.map-v1.0.0/atlas.map-1.0.0.pmmod"
        );
        for bad in ["", "1.0", "../1.0.0", "1.0.0+build", "1.0.0-RC1"] {
            assert!(repair_url(&id, bad).is_err(), "{bad}");
        }
    }

    // ---- The download ---------------------------------------------------------------------------

    struct Script(Mutex<VecDeque<Result<Hop, FetchError>>>);

    impl Transport for Script {
        fn get(
            &self,
            _url: &reqwest::Url,
            _cap: usize,
            _timeout: std::time::Duration,
        ) -> impl Future<Output = Result<Hop, FetchError>> + Send {
            let next = self.0.lock().unwrap().pop_front().expect("no more answers");
            async move { next }
        }
    }

    fn body(bytes: &[u8]) -> Result<Hop, FetchError> {
        Ok(Hop {
            status: 200,
            location: None,
            body: bytes.to_vec(),
        })
    }

    #[tokio::test]
    async fn a_download_must_have_the_listed_sha256_and_failures_are_kept_apart() {
        use sha2::{Digest, Sha256};
        let e = Endpoints::production();
        let good = b"the package";
        let sha = hex::encode(Sha256::digest(good));
        let dl = |id: &str, sha: &str| {
            Download {
            id: id.into(),
            version: "1.2.0".into(),
            url: format!(
                "https://github.com/itsAtlas-org/PM-Mods/releases/download/{id}-v1.2.0/{id}-1.2.0.pmmod"
            ),
            sha256: sha.into(),
        }
        };
        let script = Script(Mutex::new(
            vec![
                body(good),
                body(b"swapped"),
                Err(FetchError::Offline("down".into())),
            ]
            .into(),
        ));
        let done = download_all(
            &script,
            &e,
            vec![
                dl("a.one", &sha),
                dl("a.two", &sha),
                dl("a.three", &sha),
                // Not under the pinned prefix: refused with no request (the script has no answer).
                Download {
                    url: "https://evil.example/x.pmmod".into(),
                    ..dl("a.four", &sha)
                },
            ],
        )
        .await;
        assert_eq!(done[0].bytes.as_ref().unwrap(), good);
        assert!(matches!(done[1].bytes, Err(FetchError::ShaMismatch { .. })));
        assert!(matches!(done[2].bytes, Err(FetchError::Offline(_))));
        assert!(matches!(done[3].bytes, Err(FetchError::NotPinned(_))));
    }
}
