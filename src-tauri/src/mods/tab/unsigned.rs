// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Installing an unsigned `.pmmod` from the Mods tab (Mods spec §1.1, §1.13, §1.15, D3; slice 6,
//! part 3): the signed install's two steps with the stark screen between them, for a file nobody
//! has checked. Developer mode must be on for this device (the ledger reads the store's record
//! itself, `ledger::unsigned`), and installing asks the user to type the mod's id.
//!
//! - [`prepare`] verifies the archive as an unsigned one (`package::verify_unsigned`: everything but
//!   the signature), runs every check the install would (`Ledger::preview_unsigned`: developer mode,
//!   third party, no `atlas.` id, no catalogue id, the blacklist by sha256, the API range, the id
//!   not held by anything but an earlier unsigned install), and judges D3 across the other unsigned
//!   mods BEFORE the screen is offered: each permission comes back `allowed`, `asks` or `off` (with
//!   why) as the dispatcher would answer, and when the manifest asks for both reading and the
//!   internet the screen carries the choice ([`ReadOrInternet`]). It installs nothing; the verified
//!   bytes wait under the same single-use token as a signed install's (`tab::stage`,
//!   [`Trust::Unsigned`]), which only an unsigned finish can use.
//! - [`finish`] checks the typed id (equal to the mod's, in Rust, before anything else), verifies the
//!   staged bytes again, turns the answers into a plan ([`consent::plan_unsigned`]: every permission
//!   asked one by one needs an answer, and D3 and the sealed engine are applied to the yes answers
//!   across the other unsigned mods) and only then installs; the grants are written in a transaction
//!   of their own, as for a signed install.
//!
//! The screen shows what the file claims about itself (author, repository, commit): with no
//! signature none of it is vouched for, and the page says so.

use serde::Serialize;

use super::consent::{self, Answers, GrantPlan, PlanError};
use super::error::{Code, ModsError, Result};
use super::install::{ConsentPermission, ConsentScreen, ConsentSecret, Finished, InstallResult};
use super::listing;
use super::stage::{self, Staged, Trust};
use crate::mods::api::grants;
use crate::mods::api::permissions::{self, Class, PolicyTier, When};
use crate::mods::ids::ModId;
use crate::mods::index::Index;
use crate::mods::ledger::{
    Ledger, LedgerError, LedgerStore, Source, UnsignedInstall, UnsignedOptions,
};
use crate::mods::package::{self, Manifest, VerifiedPackage};
use crate::mods::policy::InstallKind;
use chrono::{DateTime, Utc};

/// What an unsigned install is checked against: the clock, the cached safety list, whether the
/// engine is sealed, and this device (developer mode is per device).
pub struct UnsignedEnv<'a> {
    pub now: DateTime<Utc>,
    pub index: Option<&'a Index>,
    pub engine_sealed: bool,
    pub device: &'a str,
}

impl<'a> UnsignedEnv<'a> {
    pub(super) fn options(&self) -> UnsignedOptions<'a> {
        UnsignedOptions {
            device: self.device,
            index: self.index,
        }
    }
}

/// What makes a screen the stark one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsignedScreen {
    /// What the user types to confirm: the mod's id, exactly.
    pub confirm_id: String,
    /// It replaces an earlier unsigned install of this id: a fresh install, its grants and secrets
    /// reset unless the user approves them again, its stored data kept.
    pub reinstall: bool,
    /// The web engine is sealed (an unsigned mod can't be granted a read otherwise).
    pub engine_sealed: bool,
    /// Present when the manifest asks for both reading the user's data and reaching outside PM:
    /// the user may allow one side, never both (D3).
    pub read_or_internet: Option<ReadOrInternet>,
    /// Present when this is a developer's test folder (`tab::folder`) and not a file: the folder,
    /// as a path to show. The screen then says the mod and what it stores last until PM restarts.
    pub test_folder: Option<String>,
}

/// D3's choice on the stark screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadOrInternet {
    /// The declared permissions that read the user's data.
    pub read: Vec<String>,
    /// The declared permissions that reach outside PM (`internet`, `links`).
    pub internet: Vec<String>,
    /// Which side another unsigned mod already holds: `read` or `internet`. The other side is off
    /// for this one, whatever it asks; each permission's own `access` says so.
    pub held_by_others: Option<&'static str>,
}

/// Which side the other unsigned mods hold (pure).
fn held_side(held_others: &[(PolicyTier, Class)]) -> Option<&'static str> {
    let has = |class| {
        held_others
            .iter()
            .any(|(t, c)| *t == PolicyTier::Unsigned && *c == class)
    };
    if has(Class::Read) {
        Some("read")
    } else if has(Class::Egress) {
        Some("internet")
    } else {
        None
    }
}

/// The stark screen for a verified unsigned manifest (pure). `held_others`: every grant the other
/// unsigned mods hold, which D3 judges this one against.
pub fn screen(
    m: &Manifest,
    reinstall: bool,
    installed_version: Option<String>,
    held_others: &[(PolicyTier, Class)],
    sha256: &str,
    engine_sealed: bool,
) -> ConsentScreen {
    let tier = PolicyTier::Unsigned;
    let permissions: Vec<ConsentPermission> = m
        .permissions
        .iter()
        .filter_map(|p| {
            // No earlier decision: a reinstall's grants are reset, so every answer starts again, and
            // starts at no.
            let shown = listing::permission(&p.id, &p.why, tier, None, held_others, engine_sealed)?;
            let when = permissions::policy(&p.id)?.when(tier);
            Some(ConsentPermission {
                is_new: false,
                answer_required: matches!(when, When::Install | When::Ask),
                default_answer: false,
                permission: shown,
            })
        })
        .collect();
    let ids_of = |class: Class| -> Vec<String> {
        m.permissions
            .iter()
            .filter(|p| permissions::policy(&p.id).is_some_and(|x| x.class == class))
            .map(|p| p.id.clone())
            .collect()
    };
    let (read, internet) = (ids_of(Class::Read), ids_of(Class::Egress));
    let read_or_internet = (!read.is_empty() && !internet.is_empty()).then(|| ReadOrInternet {
        read,
        internet,
        held_by_others: held_side(held_others),
    });
    ConsentScreen {
        token: String::new(),
        id: m.id.as_str().to_owned(),
        name: m.name.clone(),
        description: m.description.clone(),
        version: m.version.to_string(),
        author: m.publisher.author.clone(),
        repo: m.publisher.source_repo.clone(),
        commit: m.publisher.commit.clone(),
        tier: m.publisher.tier.as_str().to_owned(),
        kind: "fresh",
        installed_version,
        from: "file",
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
        widening: Vec::new(),
        unsigned: Some(UnsignedScreen {
            confirm_id: m.id.as_str().to_owned(),
            reinstall,
            engine_sealed,
            read_or_internet,
            test_folder: None,
        }),
    }
}

/// The user typed what the screen asked for: the mod's id, exactly (pure).
pub fn check_confirm(id: &ModId, typed: Option<&str>) -> Result<()> {
    if typed == Some(id.as_str()) {
        Ok(())
    } else {
        Err(ModsError::new(
            Code::ConfirmMismatch,
            "To install an unsigned mod, type its id exactly as shown.",
        ))
    }
}

/// How a plan's refusal reaches the page: D3 and the sealed engine are grant refusals the page
/// words; the rest are a request that doesn't fit the manifest.
pub fn plan_error(e: PlanError) -> ModsError {
    match e {
        PlanError::BothSides | PlanError::Exclusion(_) | PlanError::UnsealedEngine(_) => {
            ModsError::new(Code::GrantRefused, e.to_string())
        }
        PlanError::Unknown(_) | PlanError::Undeclared(_) | PlanError::Missing(_) => {
            ModsError::bad_request(e.to_string())
        }
    }
}

pub(super) fn declared_of(m: &Manifest) -> std::collections::BTreeSet<String> {
    m.permissions.iter().map(|p| p.id.clone()).collect()
}

/// What the other unsigned mods hold, as D3 judges this one against it: every grant but this id's.
pub(super) fn held_by_others<S: LedgerStore>(
    ledger: &Ledger<'_, S>,
    id: &ModId,
) -> Result<Vec<(PolicyTier, Class)>> {
    ledger
        .with_conn(|c| Ok(grants::holdings_except(c, PolicyTier::Unsigned, Some(id))?))
        .map_err(ModsError::from)
}

/// Verify an unsigned `.pmmod`, run every check an install would, judge D3 across the other unsigned
/// mods, and build the stark screen and the staged bytes behind it, installing nothing. CPU and file
/// work: call it from a blocking thread.
pub fn prepare<S: LedgerStore>(
    ledger: &Ledger<'_, S>,
    bytes: Vec<u8>,
    env: &UnsignedEnv<'_>,
) -> Result<(ConsentScreen, Staged)> {
    let pkg = package::verify_unsigned(&bytes)?;
    let sha256 = stage::sha256_of(&bytes);
    let preview = ledger.preview_unsigned(&pkg, &sha256, &env.options())?;
    let id = &pkg.manifest.id;
    let held = held_by_others(ledger, id)?;
    let screen = screen(
        &pkg.manifest,
        preview.reinstall,
        preview.installed_version,
        &held,
        &sha256,
        env.engine_sealed,
    );
    let staged = Staged {
        id: id.clone(),
        version: pkg.manifest.version.to_string(),
        sha256,
        source: Source::File,
        trust: Trust::Unsigned,
        bytes,
        folder: None,
    };
    Ok((screen, staged))
}

/// Install the staged unsigned bytes after the user typed the id, and write the grants `answers`
/// make. See the module doc for the order. Blocking.
pub fn finish<S: LedgerStore>(
    ledger: &Ledger<'_, S>,
    staged: Staged,
    answers: &Answers,
    confirm_id: Option<&str>,
    env: &UnsignedEnv<'_>,
) -> Result<Finished> {
    // A token made for a signed file never finishes here.
    if staged.trust != Trust::Unsigned {
        return Err(ModsError::consent_expired());
    }
    // The typed id first, before the bytes are looked at again.
    check_confirm(&staged.id, confirm_id)?;
    let pkg: VerifiedPackage = package::verify_unsigned(&staged.bytes)?;
    let m = &pkg.manifest;
    if m.id != staged.id || m.version.to_string() != staged.version {
        // The bytes are the ones hashed at the screen, so this is a bug, not an attack.
        return Err(ModsError::consent_expired());
    }
    // The plan before anything is written, against what the other unsigned mods hold now.
    let held = held_by_others(ledger, &m.id)?;
    let plan: GrantPlan =
        consent::plan_unsigned(&declared_of(m), answers, &held, env.engine_sealed)
            .map_err(plan_error)?;
    let UnsignedInstall {
        installed,
        reinstall,
    } = ledger.install_unsigned(&pkg, &staged.sha256, &env.options(), env.now)?;
    let id = m.id.clone();
    let wrote = ledger.with_conn(|c| {
        consent::apply_atomically(c, &id, &plan, env.engine_sealed).map_err(LedgerError::Grants)?;
        // A reinstall reset this mod's grants, which may free the other side for every other
        // unsigned mod: they are all told.
        Ok(if reinstall {
            consent::announce_unsigned_set(c, &id)?
        } else {
            consent::announce(c, &id, plan.touched())?
        })
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
            kind: match installed.kind {
                InstallKind::Fresh => "fresh",
                InstallKind::Update => "update",
                InstallKind::Repair => "repair",
            },
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
    use crate::mods::package::tests::unsigned_package;

    fn manifest_of(change: impl FnOnce(&mut serde_json::Value)) -> Manifest {
        let bytes = unsigned_package(|m, _| change(m));
        package::verify_unsigned(&bytes).unwrap().manifest
    }

    #[test]
    fn the_typed_id_must_be_the_mods_exactly() {
        let id = ModId::parse("acme.tool").unwrap();
        assert!(check_confirm(&id, Some("acme.tool")).is_ok());
        for typed in [
            None,
            Some(""),
            Some("acme"),
            Some("acme.tool "),
            Some(" acme.tool"),
            Some("Acme.tool"),
            Some("acme.tool\n"),
            Some("acme.tool.x"),
        ] {
            assert_eq!(
                check_confirm(&id, typed).unwrap_err().code,
                Code::ConfirmMismatch,
                "{typed:?}"
            );
        }
    }

    #[test]
    fn d3_and_the_sealed_engine_are_grant_refusals_and_the_rest_a_bad_request() {
        for e in [
            PlanError::BothSides,
            PlanError::Exclusion("documents.meta".into()),
            PlanError::UnsealedEngine("documents.meta".into()),
        ] {
            assert_eq!(plan_error(e).code, Code::GrantRefused);
        }
        for e in [
            PlanError::Unknown("x".into()),
            PlanError::Undeclared("x".into()),
            PlanError::Missing("x".into()),
        ] {
            assert_eq!(plan_error(e).code, Code::BadRequest);
        }
    }

    #[test]
    fn the_screen_is_the_stark_one_and_says_what_the_file_claims() {
        let m = manifest_of(|_| {});
        let s = screen(&m, false, None, &[], &"a".repeat(64), true);
        assert_eq!(
            (s.tier.as_str(), s.kind, s.from),
            ("third", "fresh", "file")
        );
        assert_eq!((s.author.as_str(), s.repo.as_str()), ("Acme", "acme/tool"));
        let u = s.unsigned.as_ref().unwrap();
        assert_eq!(u.confirm_id, "acme.tool");
        assert!(s.widening.is_empty() && s.token.is_empty());
        // Every answer starts at no, and nothing is "new".
        assert!(s.permissions.iter().all(|p| !p.default_answer && !p.is_new));
    }

    #[test]
    fn a_reinstall_says_so_and_names_the_version_it_replaces() {
        let m = manifest_of(|_| {});
        let s = screen(&m, true, Some("0.9.0".into()), &[], &"a".repeat(64), true);
        assert!(s.unsigned.unwrap().reinstall);
        assert_eq!(s.installed_version.as_deref(), Some("0.9.0"));
    }

    #[test]
    fn the_choice_appears_only_when_the_file_asks_for_both_sides() {
        let reads_only = manifest_of(|_| {});
        let s = screen(&reads_only, false, None, &[], &"a".repeat(64), true);
        assert!(s.unsigned.unwrap().read_or_internet.is_none());
        let net_only = manifest_of(|m| {
            m["contributions"] = serde_json::json!({});
            m["permissions"] = serde_json::json!([
                {"id": "storage", "why": "x"}, {"id": "internet", "why": "y"}
            ]);
            m["internetHosts"] = serde_json::json!(["rates.example.com"]);
        });
        let s = screen(&net_only, false, None, &[], &"a".repeat(64), true);
        assert!(s.unsigned.unwrap().read_or_internet.is_none());
        let both = manifest_of(|m| {
            m["contributions"] = serde_json::json!({});
            m["permissions"] = serde_json::json!([
                {"id": "storage", "why": "x"},
                {"id": "documents.text", "why": "y"},
                {"id": "links", "why": "z"}
            ]);
        });
        let s = screen(&both, false, None, &[], &"a".repeat(64), true);
        let c = s.unsigned.unwrap().read_or_internet.unwrap();
        assert_eq!(
            (c.read, c.internet),
            (vec!["documents.text".to_owned()], vec!["links".to_owned()])
        );
    }

    #[test]
    fn which_side_the_others_hold_is_read_from_their_grants() {
        assert_eq!(held_side(&[]), None);
        assert_eq!(
            held_side(&[(PolicyTier::Unsigned, Class::Read)]),
            Some("read")
        );
        assert_eq!(
            held_side(&[(PolicyTier::Unsigned, Class::Egress)]),
            Some("internet")
        );
        // A signed mod's holdings are not this judgement's business.
        assert_eq!(held_side(&[(PolicyTier::Second, Class::Egress)]), None);
        assert_eq!(held_side(&[(PolicyTier::Unsigned, Class::Local)]), None);
    }
}
