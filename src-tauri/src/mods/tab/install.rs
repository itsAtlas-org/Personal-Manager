// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Installing a signed mod from the Mods tab, in two steps with the user's answer between them
//! (Mods spec §1.13, plan decisions 2 and 11): [`prepare`] reads nothing the user hasn't picked,
//! verifies a `.pmmod`, runs every check an install would, and returns the consent screen *without
//! installing*; [`finish`] installs the verified bytes the screen was about and writes the grants
//! the user's answers made.
//!
//! **What `prepare` checks, so a refusal comes before the user reads a screen:** the verifier (a
//! trusted Atlas key not revoked, the checksums, the tier rule), the signed index (the blacklist by
//! id, version and sha256; decision 11's sha256 match where the index lists this id and version; a
//! catalogue install must agree with its entry), the API range, and the ledger's own rules (the
//! publisher pin, a downgrade, a removed mod's row), all through [`Ledger::preview`], which is the
//! install's own check run without the write. `finish` runs them all again.
//!
//! **What `finish` writes, in order:** the answers are turned into a grant plan *first*
//! ([`consent::plan`]), so an answer that doesn't fit the manifest installs nothing; then the
//! ledger's install (files, row, registry), consented, since the user is looking at it; then the
//! grants, in one transaction of their own. A failure of the grants leaves the mod installed with
//! nothing granted (every install-time permission refuses until the user grants it in the tab),
//! which [`Finished::grants_problem`] reports; it never leaves a grant without its mod.
//!
//! Generic over the ledger's store, so the tests run it over a real store and real files.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::consent::{self, Answers, GrantPlan};
use super::error::{Code, ModsError, Result};
use super::listing::{self, PermissionDto};
use super::stage::{self, Staged, Trust};
use super::unsigned::UnsignedScreen;
use crate::mods::api::grants::Decision;
use crate::mods::api::permissions::{self, PolicyTier};
use crate::mods::fetch::{self, Endpoints, Transport};
use crate::mods::index::Index;
use crate::mods::keys::TrustedKeys;
use crate::mods::ledger::{
    Approval, InstallOptions, Installed, Ledger, LedgerError, LedgerStore, Preview, Source,
};
use crate::mods::package::{self, Limits, Manifest, VerifiedPackage};
use crate::mods::policy::{InstallKind, Widening};
use crate::mods::safety::{Mode, SafetyCheck};
use crate::mods::store;

/// What an install is checked against: the clock, the keys this machine still trusts, the cached
/// safety list and whether the engine is sealed.
pub struct Env<'a> {
    pub now: DateTime<Utc>,
    pub keys: &'a TrustedKeys,
    pub index: Option<&'a Index>,
    pub engine_sealed: bool,
}

/// Read a picked `.pmmod` file: a regular file, no larger than the verifier's cap, read through the
/// handle it was opened as (no second look at the path), and at most one byte past the cap is ever
/// read, so a file that grows or a device that never ends can't fill memory. Blocking.
pub fn read_file(path: &Path) -> Result<Vec<u8>> {
    let cap = Limits::DEFAULT.file;
    let unreadable = || ModsError::new(Code::FileUnreadable, "PM couldn't read that file.");
    let file = File::open(path).map_err(|_| unreadable())?;
    let meta = file.metadata().map_err(|_| unreadable())?;
    if !meta.is_file() {
        return Err(ModsError::new(Code::FileUnreadable, "That isn't a file."));
    }
    let too_large = || {
        ModsError::new(
            Code::FileTooLarge,
            format!(
                "That file is larger than the {} MB a mod may be.",
                cap >> 20
            ),
        )
    };
    if meta.len() > cap as u64 {
        return Err(too_large());
    }
    match read_at_most(file, cap, meta.len() as usize).map_err(|_| unreadable())? {
        Some(bytes) => Ok(bytes),
        None => Err(too_large()),
    }
}

/// Everything `reader` holds, or `None` when it holds more than `cap` bytes: at most `cap + 1` bytes
/// are ever taken from it, so a stream that never ends costs `cap + 1` bytes of memory, not all of it.
/// `hint` is the size it was said to have (a reservation, never trusted).
fn read_at_most(reader: impl Read, cap: usize, hint: usize) -> std::io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::with_capacity(hint.min(cap));
    reader.take(cap as u64 + 1).read_to_end(&mut bytes)?;
    Ok((bytes.len() <= cap).then_some(bytes))
}

/// The consent screen: what the user is shown before anything is installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsentScreen {
    /// Hand this back to `install_mod_consented`. Empty until the staging layer sets it.
    pub token: String,
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    /// The publisher: the name the maker's pin and signature speak of.
    pub author: String,
    /// The people the manifest credits (`authors`), in its order; empty when it names none. Shown
    /// after the publisher, under a label that marks them as the mod's own claim.
    pub authors: Vec<String>,
    /// The source repo, `owner/repo`, and the commit the maker's signed build came from.
    pub repo: String,
    pub commit: String,
    /// `first` or `second` for a signed mod; `third` for an unsigned file, whose `unsigned` part
    /// makes it the stark screen.
    pub tier: String,
    /// `fresh`, `update` or `repair`.
    pub kind: &'static str,
    /// The version installed now, for an update or a repair.
    pub installed_version: Option<String>,
    /// `file` or `catalogue`.
    pub from: &'static str,
    pub sha256: String,
    pub permissions: Vec<ConsentPermission>,
    pub internet_hosts: Vec<String>,
    pub secrets: Vec<ConsentSecret>,
    pub nav_tab: Option<String>,
    pub calendar_overlay: Option<String>,
    /// What an update asks for that the installed version didn't (`permission:ai`,
    /// `host:example.com`, `secret:key`, `contribution:nav.tab`).
    pub widening: Vec<String>,
    /// Set for an unsigned file: nobody has checked it, it is judged with the other unsigned mods
    /// (D3), and installing it asks the user to type its id (`install_mod_consented`'s
    /// `confirm_id`). `None` for a signed mod.
    pub unsigned: Option<UnsignedScreen>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsentPermission {
    #[serde(flatten)]
    pub permission: PermissionDto,
    /// An update asks for this and the installed version didn't.
    pub is_new: bool,
    /// The screen must send an answer for it (it is asked at install).
    pub answer_required: bool,
    /// What the screen should start on: an earlier decision, else yes.
    pub default_answer: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConsentSecret {
    pub name: String,
    /// The one host it is sent to.
    pub host: String,
}

fn widening_text(w: &Widening) -> String {
    match w {
        Widening::Permission(p) => format!("permission:{p}"),
        Widening::Host(h) => format!("host:{h}"),
        Widening::Secret(s) => format!("secret:{s}"),
        Widening::Contribution(c) => format!("contribution:{c}"),
    }
}

fn kind_word(k: InstallKind) -> &'static str {
    match k {
        InstallKind::Fresh => "fresh",
        InstallKind::Update => "update",
        InstallKind::Repair => "repair",
    }
}

fn source_word(s: Source) -> &'static str {
    match s {
        Source::File => "file",
        Source::Catalogue => "catalogue",
    }
}

/// The screen for a verified manifest (pure). `decisions` are the installed row's earlier answers,
/// for an update or a repair.
pub fn screen(
    m: &Manifest,
    preview: &Preview,
    decisions: &BTreeMap<String, Decision>,
    sha256: &str,
    source: Source,
) -> ConsentScreen {
    let tier = PolicyTier::of(true, m.publisher.tier.as_str());
    let new: Vec<&str> = preview
        .widening
        .iter()
        .filter_map(|w| match w {
            Widening::Permission(p) => Some(p.as_str()),
            _ => None,
        })
        .collect();
    let permissions = m
        .permissions
        .iter()
        .filter_map(|p| {
            let shown = listing::permission(
                &p.id,
                &p.why,
                tier,
                decisions.get(&p.id).copied(),
                &[],
                true,
            )?;
            let when = permissions::policy(&p.id)?.when(tier);
            Some(ConsentPermission {
                is_new: new.contains(&p.id.as_str()),
                answer_required: when == permissions::When::Install,
                default_answer: decisions.get(&p.id).is_none_or(|d| *d == Decision::Granted),
                permission: shown,
            })
        })
        .collect();
    ConsentScreen {
        token: String::new(),
        id: m.id.as_str().to_owned(),
        name: m.name.clone(),
        description: m.description.clone(),
        version: m.version.to_string(),
        author: m.publisher.author.clone(),
        authors: m.authors.clone(),
        repo: m.publisher.source_repo.clone(),
        commit: m.publisher.commit.clone(),
        tier: m.publisher.tier.as_str().to_owned(),
        kind: kind_word(preview.kind),
        installed_version: preview.installed_version.clone(),
        from: source_word(source),
        sha256: sha256.to_owned(),
        permissions,
        internet_hosts: m.internet_hosts.clone(),
        secrets: m
            .secrets
            .iter()
            .map(|s| ConsentSecret {
                name: s.name.clone(),
                host: s.host.clone(),
            })
            .collect(),
        nav_tab: m.contributions.nav_tab.clone(),
        calendar_overlay: m.contributions.calendar_overlay.clone(),
        widening: preview.widening.iter().map(widening_text).collect(),
        unsigned: None,
    }
}

/// Verify `bytes`, run every check an install would, and build the consent screen and the staged
/// bytes behind it, installing nothing. CPU and file work: call it from a blocking thread.
pub fn prepare<S: LedgerStore>(
    ledger: &Ledger<'_, S>,
    bytes: Vec<u8>,
    source: Source,
    env: &Env<'_>,
) -> Result<(ConsentScreen, Staged)> {
    let pkg = package::verify(&bytes, env.keys.public_keys())?;
    let sha256 = stage::sha256_of(&bytes);
    let check = SafetyCheck::catalogue(env.index, env.now, Mode::Latest);
    let opts = InstallOptions {
        source,
        consented: true,
        approval: Approval::Approved,
        catalogue: &check,
    };
    let preview = ledger.preview(&pkg, &sha256, &opts)?;
    let id = &pkg.manifest.id;
    let decisions = match preview.kind {
        InstallKind::Fresh => BTreeMap::new(),
        InstallKind::Update | InstallKind::Repair => ledger
            .with_conn(|c| Ok(read_decisions(c, id.as_str())?))
            .map_err(ModsError::from)?,
    };
    let screen = screen(&pkg.manifest, &preview, &decisions, &sha256, source);
    let staged = Staged {
        id: id.clone(),
        version: pkg.manifest.version.to_string(),
        sha256,
        source,
        trust: Trust::Signed,
        bytes,
        folder: None,
    };
    Ok((screen, staged))
}

fn read_decisions(
    conn: &rusqlite::Connection,
    id: &str,
) -> rusqlite::Result<BTreeMap<String, Decision>> {
    let mut stmt = conn.prepare("SELECT permission, decision FROM mod_grants WHERE mod_id = ?1")?;
    let mut out = BTreeMap::new();
    for row in stmt.query_map([id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })? {
        let (permission, decision) = row?;
        match decision.as_str() {
            store::GRANTED => out.insert(permission, Decision::Granted),
            store::DENIED => out.insert(permission, Decision::Denied),
            _ => None,
        };
    }
    Ok(out)
}

/// Download a catalogue package: the pinned prefix, the redirect allow-list, the size and time
/// caps, and the signed index's sha256 checked before anything reads the bytes
/// ([`fetch::fetch_package`]), each refusal worded for the page. Not under any lock: it is the one
/// step that waits on the network.
pub async fn download<T: Transport>(
    transport: &T,
    endpoints: &Endpoints,
    url: &str,
    sha256: &str,
) -> Result<Vec<u8>> {
    fetch::fetch_package(transport, endpoints, url, sha256)
        .await
        .map_err(ModsError::from)
}

/// What an install did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallResult {
    pub id: String,
    pub version: String,
    /// The row's state after the install (`active`, or another owner's pause it kept).
    pub state: String,
    /// `fresh`, `update` or `repair`.
    pub kind: &'static str,
    /// Old version folders an update removed.
    pub pruned: Vec<String>,
    /// The user's answers were written as grants.
    pub grants_saved: bool,
    /// When they weren't: the mod is installed with nothing granted, and the page says so.
    pub grants_problem: Option<ModsError>,
}

/// An install that finished, with what the caller announces afterwards.
#[derive(Debug)]
pub struct Finished {
    pub result: InstallResult,
    /// The mods to tell `grants.changed`, once the grants committed.
    pub told: Vec<crate::mods::ids::ModId>,
}

/// Install the staged bytes and write the grants `answers` make. See the module doc for the order.
/// Blocking.
pub fn finish<S: LedgerStore>(
    ledger: &Ledger<'_, S>,
    staged: Staged,
    answers: &Answers,
    env: &Env<'_>,
) -> Result<Finished> {
    // A token made for an unsigned file never finishes as a signed install (its own finish has the
    // id confirm and developer mode).
    if staged.trust != Trust::Signed {
        return Err(ModsError::consent_expired());
    }
    // Verified again, with the keys this machine trusts now: a key revoked while the screen waited
    // is refused here.
    let pkg: VerifiedPackage = package::verify(&staged.bytes, env.keys.public_keys())?;
    let m = &pkg.manifest;
    if m.id != staged.id || m.version.to_string() != staged.version {
        // The bytes are the ones hashed at the screen, so this is a bug, not an attack.
        return Err(ModsError::consent_expired());
    }
    let tier = PolicyTier::of(true, m.publisher.tier.as_str());
    let declared = m.permissions.iter().map(|p| p.id.clone()).collect();
    // The plan first: an answer that doesn't fit the manifest installs nothing.
    let plan: GrantPlan = consent::plan(&declared, tier, answers)
        .map_err(|e| ModsError::bad_request(e.to_string()))?;
    let check = SafetyCheck::catalogue(env.index, env.now, Mode::Latest);
    let opts = InstallOptions {
        source: staged.source,
        consented: true,
        approval: Approval::Approved,
        catalogue: &check,
    };
    let installed: Installed = ledger.install_verified(&pkg, &staged.sha256, &opts, env.now)?;
    let id = m.id.clone();
    let wrote = ledger.with_conn(|c| {
        consent::apply_atomically(c, &id, &plan, env.engine_sealed).map_err(LedgerError::Grants)?;
        Ok(consent::announce(c, &id, plan.touched())?)
    });
    let (grants_saved, grants_problem, told) = match wrote {
        Ok(told) => (true, None, told),
        Err(e) => (false, Some(ModsError::from(e)), Vec::new()),
    };
    Ok(Finished {
        result: InstallResult {
            id: id.as_str().to_owned(),
            version: installed.version,
            state: installed.state,
            kind: kind_word(installed.kind),
            pruned: installed.pruned,
            grants_saved,
            grants_problem,
        },
        told,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::ids::Tier;
    use crate::mods::package::{Contributions, Permission, Publisher, SecretSlot};
    use semver::Version;
    use std::io::Write;

    #[test]
    fn a_picked_file_is_read_whole_when_it_is_a_file_within_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.pmmod");
        File::create(&file).unwrap().write_all(b"bytes").unwrap();
        assert_eq!(read_file(&file).unwrap(), b"bytes");
    }

    #[test]
    fn a_folder_a_missing_path_and_an_oversized_file_are_refused_with_their_own_code() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            read_file(dir.path()).unwrap_err().code,
            Code::FileUnreadable
        );
        assert_eq!(
            read_file(&dir.path().join("nope.pmmod")).unwrap_err().code,
            Code::FileUnreadable
        );
        // One byte past the cap: a sparse file, so the test writes no 32 MB.
        let cap = Limits::DEFAULT.file as u64;
        let big = dir.path().join("big.pmmod");
        let f = File::create(&big).unwrap();
        f.set_len(cap + 1).unwrap();
        assert_eq!(read_file(&big).unwrap_err().code, Code::FileTooLarge);
        // Exactly at the cap is read (and left to the verifier).
        f.set_len(cap).unwrap();
        assert_eq!(read_file(&big).unwrap().len(), cap as usize);
    }

    /// A reader that never ends, counting what is taken from it.
    struct Endless(std::rc::Rc<std::cell::Cell<usize>>);

    impl Read for Endless {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            buf.fill(7);
            self.0.set(self.0.get() + buf.len());
            Ok(buf.len())
        }
    }

    #[test]
    fn a_stream_that_never_ends_costs_one_byte_past_the_cap_not_all_of_it() {
        let taken = std::rc::Rc::new(std::cell::Cell::new(0));
        assert_eq!(read_at_most(Endless(taken.clone()), 1000, 0).unwrap(), None);
        // The reader is asked in blocks, so a block's worth of slack at most, never unbounded.
        assert!(
            taken.get() >= 1001 && taken.get() < 1001 + 8192,
            "{}",
            taken.get()
        );
        // At the cap exactly, whole; one past, refused.
        assert_eq!(
            read_at_most(&[1u8; 10][..], 10, 10).unwrap().unwrap().len(),
            10
        );
        assert_eq!(read_at_most(&[1u8; 11][..], 10, 11).unwrap(), None);
        // A hint larger than the cap is never reserved past it.
        assert!(read_at_most(&[][..], 10, usize::MAX)
            .unwrap()
            .unwrap()
            .is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_device_that_never_ends_is_refused_without_reading_it() {
        // /dev/zero reports a length of 0 and is not a regular file.
        assert_eq!(
            read_file(Path::new("/dev/zero")).unwrap_err().code,
            Code::FileUnreadable
        );
    }

    fn manifest() -> Manifest {
        Manifest {
            id: crate::mods::ids::ModId::parse("acme.notes").unwrap(),
            name: "Notes".into(),
            description: "Notes for projects.".into(),
            icon: None,
            version: Version::parse("1.2.0").unwrap(),
            api: package::ApiRange {
                min: (1, 0),
                target: (1, 0),
            },
            publisher: Publisher {
                tier: Tier::Second,
                author: "Acme".into(),
                source_repo: "acme/notes".into(),
                commit: "d".repeat(40),
            },
            authors: vec!["Ann Lee".into(), "Bo Chen".into()],
            permissions: ["documents.meta", "documents.text", "storage", "ai"]
                .iter()
                .map(|p| Permission {
                    id: (*p).into(),
                    why: format!("why {p}"),
                })
                .collect(),
            internet_hosts: vec!["api.acme.test".into()],
            secrets: vec![SecretSlot {
                name: "key".into(),
                host: "api.acme.test".into(),
                header: "x-key".into(),
            }],
            contributions: Contributions {
                nav_tab: Some("Notes".into()),
                calendar_overlay: None,
                help: vec![],
            },
            data: Default::default(),
            entry: package::Entry {
                page: "app/index.html".into(),
                workers: vec![],
            },
            requires: vec![],
        }
    }

    fn fresh() -> Preview {
        Preview {
            kind: InstallKind::Fresh,
            installed_version: None,
            widening: vec![],
        }
    }

    #[test]
    fn the_screen_shows_who_made_it_what_it_asks_for_and_which_answers_it_needs() {
        let s = screen(
            &manifest(),
            &fresh(),
            &BTreeMap::new(),
            &"a".repeat(64),
            Source::File,
        );
        assert_eq!(
            (
                s.id.as_str(),
                s.name.as_str(),
                s.tier.as_str(),
                s.kind,
                s.from
            ),
            ("acme.notes", "Notes", "second", "fresh", "file")
        );
        assert_eq!((s.author.as_str(), s.repo.as_str()), ("Acme", "acme/notes"));
        assert_eq!(s.authors, ["Ann Lee", "Bo Chen"]);
        assert_eq!(
            s.commit,
            "d".repeat(40),
            "second party shows the reviewed commit"
        );
        assert_eq!(s.nav_tab.as_deref(), Some("Notes"));
        assert_eq!(s.internet_hosts, ["api.acme.test"]);
        assert_eq!(
            s.secrets,
            [ConsentSecret {
                name: "key".into(),
                host: "api.acme.test".into()
            }]
        );
        let by: BTreeMap<&str, &ConsentPermission> = s
            .permissions
            .iter()
            .map(|p| (p.permission.id.as_str(), p))
            .collect();
        // Second party: documents.meta is asked at install; documents.text again at first use;
        // storage is automatic; ai is first use.
        assert!(by["documents.meta"].answer_required);
        assert!(!by["documents.text"].answer_required);
        assert!(!by["storage"].answer_required && !by["ai"].answer_required);
        assert_eq!(by["documents.meta"].permission.why, "why documents.meta");
        assert!(by.values().all(|p| p.default_answer && !p.is_new));
        assert!(s.token.is_empty(), "the staging layer sets the token");
    }

    #[test]
    fn an_update_marks_what_is_new_and_starts_each_answer_on_the_earlier_decision() {
        let preview = Preview {
            kind: InstallKind::Update,
            installed_version: Some("1.1.0".into()),
            widening: vec![
                Widening::Permission("documents.text".into()),
                Widening::Host("api.acme.test".into()),
            ],
        };
        let decisions = BTreeMap::from([
            ("documents.meta".to_owned(), Decision::Denied),
            ("storage".to_owned(), Decision::Granted),
        ]);
        let s = screen(
            &manifest(),
            &preview,
            &decisions,
            &"a".repeat(64),
            Source::Catalogue,
        );
        assert_eq!((s.kind, s.from), ("update", "catalogue"));
        assert_eq!(s.installed_version.as_deref(), Some("1.1.0"));
        assert_eq!(
            s.widening,
            ["permission:documents.text", "host:api.acme.test"]
        );
        let by: BTreeMap<&str, &ConsentPermission> = s
            .permissions
            .iter()
            .map(|p| (p.permission.id.as_str(), p))
            .collect();
        assert!(by["documents.text"].is_new && !by["documents.meta"].is_new);
        assert!(
            !by["documents.meta"].default_answer,
            "an earlier refusal stays refused"
        );
        assert!(by["storage"].default_answer && by["documents.text"].default_answer);
    }

    // ---- the download ------------------------------------------------------------------------

    use crate::mods::fetch::{FetchError, Hop};
    use std::collections::VecDeque;
    use std::future::Future;
    use std::sync::Mutex;

    /// Answers in order, and the URLs asked.
    struct Script(
        Mutex<VecDeque<std::result::Result<Hop, FetchError>>>,
        Mutex<Vec<String>>,
    );

    impl Script {
        fn new(answers: Vec<std::result::Result<Hop, FetchError>>) -> Self {
            Self(Mutex::new(answers.into()), Mutex::default())
        }
    }

    impl Transport for Script {
        fn get(
            &self,
            url: &reqwest::Url,
            _cap: usize,
            _timeout: std::time::Duration,
        ) -> impl Future<Output = std::result::Result<Hop, FetchError>> + Send {
            self.1.lock().unwrap().push(url.to_string());
            let next = self.0.lock().unwrap().pop_front().expect("no more answers");
            async move { next }
        }
    }

    fn ok(body: &[u8]) -> std::result::Result<Hop, FetchError> {
        Ok(Hop {
            status: 200,
            location: None,
            body: body.to_vec(),
        })
    }

    fn package_url() -> String {
        crate::mods::index::package_url(
            &crate::mods::ids::ModId::parse("atlas.map").unwrap(),
            &crate::mods::disk::VersionName::parse("1.2.0").unwrap(),
        )
    }

    fn sha(bytes: &[u8]) -> String {
        stage::sha256_of(bytes)
    }

    #[tokio::test]
    async fn a_download_is_the_listed_file_or_a_worded_refusal() {
        let e = Endpoints::production();
        let file = b"the package".to_vec();
        // The listed file.
        let t = Script::new(vec![ok(&file)]);
        assert_eq!(
            download(&t, &e, &package_url(), &sha(&file)).await.unwrap(),
            file
        );
        // Another file: refused before anything reads it.
        let t = Script::new(vec![ok(b"something else")]);
        let err = download(&t, &e, &package_url(), &sha(&file))
            .await
            .unwrap_err();
        assert_eq!(err.code, Code::DownloadRefused);
        // Offline, and not published yet: the same advice, the connection.
        for answer in [
            Err(FetchError::Offline("no route".into())),
            Ok(Hop {
                status: 404,
                location: None,
                body: vec![],
            }),
        ] {
            let t = Script::new(vec![answer]);
            let err = download(&t, &e, &package_url(), &sha(&file))
                .await
                .unwrap_err();
            assert_eq!(err.code, Code::Offline);
        }
        // A URL outside the pinned prefix is never requested.
        let t = Script::new(vec![]);
        let err = download(&t, &e, "https://evil.example/x.pmmod", &sha(&file))
            .await
            .unwrap_err();
        assert_eq!(err.code, Code::DownloadRefused);
        assert!(t.1.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_download_over_the_cap_is_refused() {
        let e = Endpoints::production();
        let t = Script::new(vec![Err(FetchError::TooLarge)]);
        let err = download(&t, &e, &package_url(), &"0".repeat(64))
            .await
            .unwrap_err();
        assert_eq!(err.code, Code::DownloadRefused);
    }

    #[test]
    fn the_screen_serialises_with_the_permission_fields_beside_its_own() {
        let s = screen(
            &manifest(),
            &fresh(),
            &BTreeMap::new(),
            &"a".repeat(64),
            Source::File,
        );
        let v = serde_json::to_value(&s).unwrap();
        let p = &v["permissions"][0];
        for key in [
            "id",
            "why",
            "class",
            "when",
            "access",
            "isNew",
            "answerRequired",
            "defaultAnswer",
        ] {
            assert!(p.get(key).is_some(), "{key} in {p}");
        }
        assert!(v["token"].is_string() && v["commit"].is_string() && v["widening"].is_array());
    }
}
