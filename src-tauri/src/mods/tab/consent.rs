// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the user's answers on a consent screen become (Mods spec §1.5, plan decision 2): explicit
//! `mod_grants` rows, written at consent, so every grant is auditable and revocable one by one.
//!
//! **The answers.** The screen lists every permission the mod declares, and the page sends one
//! yes or no for each it showed ([`Answers`]). [`plan`] turns them into rows, strictly:
//! - an answer for a permission the mod didn't declare, or PM doesn't have, is refused, so a page
//!   can't write a row for something else;
//! - every permission asked *at install* for this tier (`When::Install`) needs an answer: consent
//!   covers what it names, and a screen that left one out is a bug, not a "no";
//! - a yes writes a row only for a permission asked at install. A permission asked at first use
//!   (`ai`, a sensitive read) is left for its prompt, even when the screen listed it, as
//!   `grants::record_install_consent` already says; an automatic one needs no row;
//! - a no is always written, whatever the policy: taking a permission away is always allowed, and
//!   a recorded denial is what stops a later prompt from asking again.
//!
//! **Where it is written.** [`apply`] runs the two writers in the caller's transaction (the first
//! open's "Turn on" writes the grants with `consented_at`, so a refused grant leaves the mod off) or
//! in its own right after an install's commit. Either way D3 and the sealed-engine rule apply at
//! grant time, so a forbidden pair can't be recorded.
//!
//! **Who is told.** `grants.changed` goes to the mod, and, when the change touched a read or an
//! egress permission of an unsigned mod, to every other unsigned mod too (D3 makes one unsigned mod's
//! grant another's refusal): [`announce`] names them.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::Connection;

use crate::mods::api::grants::{self, Decision, GrantError};
use crate::mods::api::permissions::{self, Class, PolicyTier, When};
use crate::mods::ids::ModId;
use crate::mods::store;

/// The user's answers: permission id to granted (true) or refused (false).
pub type Answers = BTreeMap<String, bool>;

/// Why the answers can't be turned into grants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// PM has no such permission.
    Unknown(String),
    /// The mod didn't declare it.
    Undeclared(String),
    /// A permission asked at install, with no answer.
    Missing(String),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown(p) => write!(f, "{p:?} isn't a permission PM has."),
            Self::Undeclared(p) => write!(f, "This mod didn't ask for {p:?}."),
            Self::Missing(p) => write!(f, "The screen left out an answer for {p:?}."),
        }
    }
}

/// The rows to write: install-time grants, and denials.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GrantPlan {
    pub grant: Vec<String>,
    pub deny: Vec<String>,
}

impl GrantPlan {
    /// Every permission the plan touches.
    pub fn touched(&self) -> impl Iterator<Item = &str> {
        self.grant.iter().chain(&self.deny).map(String::as_str)
    }
}

/// The plan for `answers` over the permissions a mod of `tier` `declared` (pure).
pub fn plan(
    declared: &BTreeSet<String>,
    tier: PolicyTier,
    answers: &Answers,
) -> Result<GrantPlan, PlanError> {
    for asked in answers.keys() {
        if permissions::policy(asked).is_none() {
            return Err(PlanError::Unknown(asked.clone()));
        }
        if !declared.contains(asked) {
            return Err(PlanError::Undeclared(asked.clone()));
        }
    }
    for id in declared {
        let Some(policy) = permissions::policy(id) else {
            continue;
        };
        if policy.when(tier) == When::Install && !answers.contains_key(id) {
            return Err(PlanError::Missing(id.clone()));
        }
    }
    let mut out = GrantPlan::default();
    for (id, yes) in answers {
        let Some(policy) = permissions::policy(id) else {
            continue;
        };
        if !yes {
            out.deny.push(id.clone());
        } else if policy.when(tier) == When::Install {
            out.grant.push(id.clone());
        }
    }
    Ok(out)
}

/// Write the plan's rows. The caller owns the transaction: a refusal part way leaves rows written
/// before it, so run this inside one (or use [`apply_atomically`]).
pub fn apply(
    conn: &Connection,
    id: &ModId,
    plan: &GrantPlan,
    engine_sealed: bool,
) -> Result<(), GrantError> {
    let grant: Vec<&str> = plan.grant.iter().map(String::as_str).collect();
    grants::record_install_consent(conn, id, &grant, engine_sealed)?;
    for p in &plan.deny {
        grants::set_decision(conn, id, p, Decision::Denied, engine_sealed)?;
    }
    Ok(())
}

/// [`apply`] in a transaction of its own: whole or not at all.
pub fn apply_atomically(
    conn: &mut Connection,
    id: &ModId,
    plan: &GrantPlan,
    engine_sealed: bool,
) -> Result<(), GrantError> {
    let tx = conn.transaction()?;
    apply(&tx, id, plan, engine_sealed)?;
    tx.commit()?;
    Ok(())
}

/// Whether a change to `permission` could change another unsigned mod's answers (pure): D3 judges
/// reads against egress across the whole unsigned set.
fn touches_d3(permission: &str) -> bool {
    permissions::policy(permission).is_some_and(|p| {
        permissions::exclusion_applies(PolicyTier::Unsigned, p.class)
            && matches!(p.class, Class::Read | Class::Egress)
    })
}

/// The mods to tell `grants.changed` after a change to `touched` permissions of `id` committed:
/// `id` itself, and, when the mod is unsigned and a touched permission is one D3 judges, every
/// other unsigned mod. Read after the commit.
pub fn announce<'a>(
    conn: &Connection,
    id: &ModId,
    touched: impl IntoIterator<Item = &'a str>,
) -> rusqlite::Result<Vec<ModId>> {
    let mut out = vec![id.clone()];
    if !touched.into_iter().any(touches_d3) {
        return Ok(out);
    }
    let mut stmt = conn.prepare("SELECT id, signed, tier FROM mods")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, bool>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    let mut mods = Vec::new();
    for row in rows {
        mods.push(row?);
    }
    let this_unsigned = mods
        .iter()
        .find(|(m, _, _)| m == id.as_str())
        .is_some_and(|(_, signed, tier)| PolicyTier::of(*signed, tier) == PolicyTier::Unsigned);
    if this_unsigned {
        for (m, signed, tier) in mods {
            if PolicyTier::of(signed, &tier) == PolicyTier::Unsigned {
                if let Ok(m) = ModId::parse(&m) {
                    if &m != id {
                        out.push(m);
                    }
                }
            }
        }
    }
    Ok(out)
}

/// One permission decision from the Mods tab's toggle (`mod_permission_decide`): refused for a mod
/// the safety list removed (its grants were revoked and stay so), then
/// [`grants::set_decision`], whole or not at all. Returns the mods to tell afterwards.
pub fn decide(
    conn: &mut Connection,
    id: &ModId,
    permission: &str,
    granted: bool,
    engine_sealed: bool,
) -> Result<Vec<ModId>, DecideError> {
    let tx = conn.transaction()?;
    let reason: Option<Option<String>> = tx
        .query_row(
            "SELECT state_reason FROM mods WHERE id = ?1",
            [id.as_str()],
            |r| r.get(0),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })?;
    match reason {
        None => return Err(DecideError::Grant(GrantError::NotInstalled)),
        Some(r) if r.as_deref() == Some(store::REASON_BLACKLIST_REMOVED) => {
            return Err(DecideError::Removed)
        }
        Some(_) => {}
    }
    let decision = if granted {
        Decision::Granted
    } else {
        Decision::Denied
    };
    grants::set_decision(&tx, id, permission, decision, engine_sealed)?;
    let tell = announce(&tx, id, [permission])?;
    tx.commit()?;
    Ok(tell)
}

#[derive(Debug)]
pub enum DecideError {
    Grant(GrantError),
    /// The safety list removed the mod: its permissions stay revoked.
    Removed,
}

impl From<GrantError> for DecideError {
    fn from(e: GrantError) -> Self {
        Self::Grant(e)
    }
}

impl From<rusqlite::Error> for DecideError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Grant(e.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::tab::testing::{insert, manifest, store as test_store, Row};
    use PolicyTier::{First, Second, Unsigned};

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    fn answers(items: &[(&str, bool)]) -> Answers {
        items.iter().map(|(p, a)| ((*p).to_owned(), *a)).collect()
    }

    #[test]
    fn a_yes_writes_a_row_only_for_what_is_asked_at_install() {
        // documents.meta is asked at install for first party; documents.text for a second-party
        // mod is asked again at first use; storage is automatic; ai is first use.
        let declared = set(&["documents.meta", "documents.text", "storage", "ai"]);
        let said = answers(&[
            ("documents.meta", true),
            ("documents.text", true),
            ("storage", true),
            ("ai", true),
        ]);
        let first = plan(&declared, First, &said).unwrap();
        assert_eq!(first.grant, ["documents.meta", "documents.text"]);
        assert!(first.deny.is_empty());
        let second = plan(&declared, Second, &said).unwrap();
        assert_eq!(
            second.grant,
            ["documents.meta"],
            "documents.text waits for its first use"
        );
    }

    #[test]
    fn a_no_is_always_a_recorded_denial_whatever_the_policy() {
        let declared = set(&["documents.meta", "storage", "ai", "documents.text"]);
        let said = answers(&[
            ("documents.meta", false),
            ("storage", false),
            ("ai", false),
            ("documents.text", false),
        ]);
        let p = plan(&declared, Second, &said).unwrap();
        assert!(p.grant.is_empty());
        assert_eq!(
            p.deny,
            ["ai", "documents.meta", "documents.text", "storage"]
        );
    }

    #[test]
    fn the_screen_must_answer_every_install_time_permission_and_only_declared_ones() {
        let declared = set(&["documents.meta", "tasks.read"]);
        assert_eq!(
            plan(&declared, First, &answers(&[("documents.meta", true)])),
            Err(PlanError::Missing("tasks.read".into()))
        );
        assert_eq!(
            plan(
                &declared,
                First,
                &answers(&[
                    ("documents.meta", true),
                    ("tasks.read", true),
                    ("internet", true)
                ])
            ),
            Err(PlanError::Undeclared("internet".into()))
        );
        assert_eq!(
            plan(
                &declared,
                First,
                &answers(&[
                    ("documents.meta", true),
                    ("tasks.read", true),
                    ("made.up", true)
                ])
            ),
            Err(PlanError::Unknown("made.up".into()))
        );
        // A first-use permission needs no answer; an empty set of answers is fine for a mod that
        // asks for nothing at install.
        assert_eq!(
            plan(&set(&["ai"]), First, &Answers::new()),
            Ok(GrantPlan::default())
        );
        assert_eq!(
            plan(&set(&[]), First, &Answers::new()),
            Ok(GrantPlan::default())
        );
    }

    #[test]
    fn an_unsigned_mod_gets_no_install_time_grant_from_a_yes() {
        // Every unsigned cell is "ask", not "install": the stark screen answers them one by one at
        // first use (part 3), so a yes here writes nothing, and a no is still recorded.
        let declared = set(&["documents.meta", "internet"]);
        let p = plan(
            &declared,
            Unsigned,
            &answers(&[("documents.meta", true), ("internet", false)]),
        )
        .unwrap();
        assert_eq!(
            (p.grant.as_slice(), p.deny.as_slice()),
            (&[][..], &["internet".to_owned()][..])
        );
    }

    fn grants_of(c: &Connection, id: &str) -> BTreeMap<String, String> {
        let mut stmt = c
            .prepare("SELECT permission, decision FROM mod_grants WHERE mod_id = ?1")
            .unwrap();
        stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    fn id(s: &str) -> ModId {
        ModId::parse(s).unwrap()
    }

    #[test]
    fn apply_writes_the_rows_and_a_refusal_writes_none() {
        let (_dir, mut c) = test_store();
        insert(
            &c,
            &Row {
                manifest: manifest(&["documents.meta", "tasks.read", "storage"]),
                ..Row::default()
            },
        );
        let good = GrantPlan {
            grant: vec!["documents.meta".into(), "tasks.read".into()],
            deny: vec!["storage".into()],
        };
        apply_atomically(&mut c, &id("atlas.map"), &good, true).unwrap();
        assert_eq!(
            grants_of(&c, "atlas.map"),
            BTreeMap::from([
                ("documents.meta".into(), "granted".into()),
                ("tasks.read".into(), "granted".into()),
                ("storage".into(), "denied".into()),
            ])
        );
        // A plan naming a permission the manifest lacks is refused whole.
        c.execute("DELETE FROM mod_grants", []).unwrap();
        let bad = GrantPlan {
            grant: vec!["documents.meta".into(), "internet".into()],
            deny: vec![],
        };
        assert_eq!(
            apply_atomically(&mut c, &id("atlas.map"), &bad, true),
            Err(GrantError::Undeclared("internet".into()))
        );
        assert!(grants_of(&c, "atlas.map").is_empty(), "nothing written");
    }

    #[test]
    fn a_grant_d3_forbids_is_refused_at_consent_and_writes_nothing() {
        let (_dir, mut c) = test_store();
        insert(
            &c,
            &Row {
                id: "zed.reader",
                signed: false,
                tier: "third",
                installed_from: "file",
                manifest: manifest(&["documents.meta"]),
                ..Row::default()
            },
        );
        insert(
            &c,
            &Row {
                id: "acme.web",
                signed: false,
                tier: "third",
                installed_from: "file",
                manifest: manifest(&["internet"]),
                ..Row::default()
            },
        );
        // set_decision is the writer an unsigned grant goes through; the unsigned reader holds a
        // read, so the other's internet is refused, and the refusal writes nothing.
        decide(&mut c, &id("zed.reader"), "documents.meta", true, true).unwrap();
        let refused = decide(&mut c, &id("acme.web"), "internet", true, true);
        assert!(
            matches!(refused, Err(DecideError::Grant(GrantError::Refused(_, _)))),
            "{refused:?}"
        );
        assert!(grants_of(&c, "acme.web").is_empty());
        // Taking one away is always allowed, and frees the other.
        decide(&mut c, &id("zed.reader"), "documents.meta", false, true).unwrap();
        decide(&mut c, &id("acme.web"), "internet", true, true).unwrap();
    }

    #[test]
    fn a_permission_decision_for_a_removed_or_missing_mod_is_refused() {
        let (_dir, mut c) = test_store();
        insert(
            &c,
            &Row {
                state: "paused",
                reason: Some("blacklist-removed"),
                ..Row::default()
            },
        );
        assert!(matches!(
            decide(&mut c, &id("atlas.map"), "storage", true, true),
            Err(DecideError::Removed)
        ));
        assert!(matches!(
            decide(&mut c, &id("nobody.here"), "storage", true, true),
            Err(DecideError::Grant(GrantError::NotInstalled))
        ));
        assert!(grants_of(&c, "atlas.map").is_empty());
    }

    #[test]
    fn grants_changed_goes_to_the_mod_and_to_the_whole_unsigned_set_only_when_d3_could_move() {
        let (_dir, mut c) = test_store();
        for (m, signed, tier) in [
            ("atlas.map", true, "first"),
            ("acme.one", false, "third"),
            ("acme.two", false, "third"),
        ] {
            insert(
                &c,
                &Row {
                    id: m,
                    signed,
                    tier,
                    installed_from: if signed { "catalogue" } else { "file" },
                    manifest: manifest(&["documents.meta", "internet", "storage", "tasks.write"]),
                    ..Row::default()
                },
            );
        }
        let told = |c: &Connection, who: &str, what: &[&str]| -> Vec<String> {
            let mut v: Vec<String> = announce(c, &id(who), what.iter().copied())
                .unwrap()
                .iter()
                .map(|m| m.as_str().to_owned())
                .collect();
            v.sort();
            v
        };
        // An unsigned read or egress change moves the other unsigned mods' answers.
        assert_eq!(
            told(&c, "acme.one", &["documents.meta"]),
            ["acme.one", "acme.two"]
        );
        assert_eq!(
            told(&c, "acme.one", &["internet"]),
            ["acme.one", "acme.two"]
        );
        // A write or local permission doesn't.
        assert_eq!(told(&c, "acme.one", &["tasks.write"]), ["acme.one"]);
        assert_eq!(told(&c, "acme.one", &["storage"]), ["acme.one"]);
        // A signed mod's read says nothing to the unsigned ones: no exclusion applies to it.
        assert_eq!(told(&c, "atlas.map", &["documents.meta"]), ["atlas.map"]);
        // And the decide command returns the same set.
        let tell = decide(&mut c, &id("acme.two"), "internet", true, true).unwrap();
        let mut tell: Vec<&str> = tell.iter().map(ModId::as_str).collect();
        tell.sort();
        assert_eq!(tell, ["acme.one", "acme.two"]);
    }
}
