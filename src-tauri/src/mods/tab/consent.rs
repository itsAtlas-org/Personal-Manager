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

use crate::mods::ai as money;
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
    /// An unsigned mod: yes to reading the user's data and to reaching outside PM (D3: one or the
    /// other, never both).
    BothSides,
    /// An unsigned mod: D3 refuses this yes, because another unsigned mod holds the other side.
    Exclusion(String),
    /// An unsigned mod: a read, while the web engine isn't sealed.
    UnsealedEngine(String),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown(p) => write!(f, "{p:?} isn't a permission PM has."),
            Self::Undeclared(p) => write!(f, "This mod didn't ask for {p:?}."),
            Self::Missing(p) => write!(f, "The screen left out an answer for {p:?}."),
            Self::BothSides => f.write_str(
                "An unsigned mod may read your data or reach the internet, never both. Choose one.",
            ),
            Self::Exclusion(p) => write!(
                f,
                "Unsigned mods may read your data or reach the internet, never both, and another \
                 unsigned mod already holds the other ({p})."
            ),
            Self::UnsealedEngine(p) => write!(
                f,
                "Unsigned mods can't read your data until PM's web engine is sealed ({p})."
            ),
        }
    }
}

/// The rows to write: install-time grants, an unsigned mod's yes answers, and denials.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GrantPlan {
    pub grant: Vec<String>,
    /// An unsigned mod's yes answers: asked one by one on its own install screen
    /// ([`When::Ask`]), written as decisions rather than install consent.
    pub grant_asked: Vec<String>,
    pub deny: Vec<String>,
}

impl GrantPlan {
    /// Every permission the plan touches.
    pub fn touched(&self) -> impl Iterator<Item = &str> {
        self.grant
            .iter()
            .chain(&self.grant_asked)
            .chain(&self.deny)
            .map(String::as_str)
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

/// The plan for an unsigned mod's answers (pure), judged BEFORE anything is installed: the stark
/// screen asks every permission that is asked one by one ([`When::Ask`]), strictly as [`plan`] does
/// (no answer for what the mod didn't declare, one for each that is asked), and then D3's rules are
/// applied to the yes answers: never a read and an egress together, never one against what the other
/// unsigned mods hold (`held_others`: every grant of the other unsigned mods), and no read while the
/// engine isn't sealed. A forbidden pair is refused here, so it is never installed; the grant
/// writers check again when the rows are written.
pub fn plan_unsigned(
    declared: &BTreeSet<String>,
    answers: &Answers,
    held_others: &[(PolicyTier, Class)],
    engine_sealed: bool,
) -> Result<GrantPlan, PlanError> {
    let tier = PolicyTier::Unsigned;
    for asked in answers.keys() {
        if permissions::policy(asked).is_none() {
            return Err(PlanError::Unknown(asked.clone()));
        }
        if !declared.contains(asked) {
            return Err(PlanError::Undeclared(asked.clone()));
        }
    }
    let asked_here = |when: When| matches!(when, When::Install | When::Ask);
    for id in declared {
        let Some(policy) = permissions::policy(id) else {
            continue;
        };
        if asked_here(policy.when(tier)) && !answers.contains_key(id) {
            return Err(PlanError::Missing(id.clone()));
        }
    }
    let mut out = GrantPlan::default();
    let (mut reads, mut egress) = (false, false);
    for (id, yes) in answers {
        let Some(policy) = permissions::policy(id) else {
            continue;
        };
        if !yes {
            out.deny.push(id.clone());
            continue;
        }
        if !asked_here(policy.when(tier)) {
            continue; // first use: waits for its prompt
        }
        if !permissions::allowed_by_engine(tier, policy.class, engine_sealed) {
            return Err(PlanError::UnsealedEngine(id.clone()));
        }
        if permissions::excluded(tier, policy.class, held_others.iter().copied()) {
            return Err(PlanError::Exclusion(id.clone()));
        }
        reads |= policy.class == Class::Read;
        egress |= policy.class == Class::Egress;
        out.grant_asked.push(id.clone());
    }
    if reads && egress {
        return Err(PlanError::BothSides);
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
    for p in &plan.grant_asked {
        grants::set_decision(conn, id, p, Decision::Granted, engine_sealed)?;
    }
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
    announce_with(conn, id, touched.into_iter().any(touches_d3))
}

/// [`announce`] for a change that may have moved D3's line without naming a permission: an unsigned
/// install over an earlier one reset the mod's grants, which may free the other side for every
/// other unsigned mod.
pub fn announce_unsigned_set(conn: &Connection, id: &ModId) -> rusqlite::Result<Vec<ModId>> {
    announce_with(conn, id, true)
}

fn announce_with(
    conn: &Connection,
    id: &ModId,
    judged_together: bool,
) -> rusqlite::Result<Vec<ModId>> {
    let mut out = vec![id.clone()];
    if !judged_together {
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
///
/// **A monthly limit rides with the yes to `ai`.** `cap_usd` is the limit the money prompt's field
/// held. It is written in the same transaction as the grant, so a yes that is refused (D3, the
/// sealed engine) leaves no limit behind and a limit that cannot be written leaves no yes: the
/// prompt can never half-answer. It is accepted only with a yes to `ai`, and only a number the
/// ledger can use ([`money::valid_cap`]); anything else is refused before the store is read. With
/// no limit the grant is written and the limit stays as it was (none, or the one already set), and
/// a paid call still asks for one: the handler's state rule, not this write, is what stops spending.
pub fn decide(
    conn: &mut Connection,
    id: &ModId,
    permission: &str,
    granted: bool,
    cap_usd: Option<f64>,
    engine_sealed: bool,
) -> Result<Vec<ModId>, DecideError> {
    if let Some(cap) = cap_usd {
        if permission != "ai" || !granted {
            return Err(DecideError::Limit(LimitRefusal::NotWithThis));
        }
        if !money::valid_cap(cap) {
            return Err(DecideError::Limit(LimitRefusal::NotAnAmount));
        }
    }
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
    if let Some(cap) = cap_usd {
        money::set_cap(&tx, id.as_str(), Some(cap)).map_err(|e| match e {
            money::CapError::BadAmount => DecideError::Limit(LimitRefusal::NotAnAmount),
            money::CapError::ModGone => DecideError::Grant(GrantError::NotInstalled),
            money::CapError::Db(e) => DecideError::Grant(e.into()),
        })?;
    }
    let tell = announce(&tx, id, [permission])?;
    tx.commit()?;
    Ok(tell)
}

/// Why a limit sent with a decision was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitRefusal {
    /// A limit goes only with a yes to `ai`.
    NotWithThis,
    /// Not a number from 0 to the ceiling.
    NotAnAmount,
}

#[derive(Debug)]
pub enum DecideError {
    Grant(GrantError),
    /// The safety list removed the mod: its permissions stay revoked.
    Removed,
    /// The limit sent with the decision can't be used; nothing was written.
    Limit(LimitRefusal),
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
            ..GrantPlan::default()
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
            ..GrantPlan::default()
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
        decide(
            &mut c,
            &id("zed.reader"),
            "documents.meta",
            true,
            None,
            true,
        )
        .unwrap();
        let refused = decide(&mut c, &id("acme.web"), "internet", true, None, true);
        assert!(
            matches!(refused, Err(DecideError::Grant(GrantError::Refused(_, _)))),
            "{refused:?}"
        );
        assert!(grants_of(&c, "acme.web").is_empty());
        // Taking one away is always allowed, and frees the other.
        decide(
            &mut c,
            &id("zed.reader"),
            "documents.meta",
            false,
            None,
            true,
        )
        .unwrap();
        decide(&mut c, &id("acme.web"), "internet", true, None, true).unwrap();
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
            decide(&mut c, &id("atlas.map"), "storage", true, None, true),
            Err(DecideError::Removed)
        ));
        assert!(matches!(
            decide(&mut c, &id("nobody.here"), "storage", true, None, true),
            Err(DecideError::Grant(GrantError::NotInstalled))
        ));
        assert!(grants_of(&c, "atlas.map").is_empty());
    }

    fn chat(c: &Connection) {
        insert(
            c,
            &Row {
                id: "acme.chat",
                signed: true,
                tier: "second",
                manifest: manifest(&["ai", "storage"]),
                ..Row::default()
            },
        );
    }

    fn cap_of(c: &Connection, id: &str) -> Option<f64> {
        c.query_row("SELECT ai_cap_usd FROM mods WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap()
    }

    /// The money prompt's yes: the grant and the limit are one write.
    #[test]
    fn a_yes_to_ai_carries_the_limit_and_both_are_written() {
        let (_dir, mut c) = test_store();
        chat(&c);
        decide(&mut c, &id("acme.chat"), "ai", true, Some(2.5), true).unwrap();
        assert_eq!(
            grants_of(&c, "acme.chat"),
            BTreeMap::from([("ai".into(), "granted".into())])
        );
        assert_eq!(cap_of(&c, "acme.chat"), Some(2.5));
        // Zero is a limit: it allows the AI and no cloud spend.
        decide(&mut c, &id("acme.chat"), "ai", true, Some(0.0), true).unwrap();
        assert_eq!(cap_of(&c, "acme.chat"), Some(0.0));
    }

    /// A limit that cannot be used, or sent with the wrong answer, is refused before anything is
    /// written: no grant, no limit.
    #[test]
    fn a_limit_that_does_not_belong_is_refused_and_writes_nothing() {
        let (_dir, mut c) = test_store();
        chat(&c);
        let refuse = |c: &mut Connection, permission: &str, granted, cap| {
            decide(c, &id("acme.chat"), permission, granted, Some(cap), true)
        };
        for bad in [
            -0.01,
            -1.0,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            1e300,
            crate::mods::ai::MAX_CAP_USD + 1.0,
        ] {
            assert!(
                matches!(
                    refuse(&mut c, "ai", true, bad),
                    Err(DecideError::Limit(LimitRefusal::NotAnAmount))
                ),
                "{bad}"
            );
        }
        // A limit goes only with a yes to `ai`.
        assert!(matches!(
            refuse(&mut c, "ai", false, 1.0),
            Err(DecideError::Limit(LimitRefusal::NotWithThis))
        ));
        assert!(matches!(
            refuse(&mut c, "storage", true, 1.0),
            Err(DecideError::Limit(LimitRefusal::NotWithThis))
        ));
        assert!(grants_of(&c, "acme.chat").is_empty());
        assert_eq!(cap_of(&c, "acme.chat"), None);
        // Neither does a mod that didn't ask for `ai` get a limit with a refused grant.
        insert(
            &c,
            &Row {
                id: "acme.noai",
                signed: true,
                tier: "second",
                manifest: manifest(&["storage"]),
                ..Row::default()
            },
        );
        let e = decide(&mut c, &id("acme.noai"), "ai", true, Some(1.0), true);
        assert!(matches!(
            e,
            Err(DecideError::Grant(GrantError::Undeclared(_)))
        ));
        assert_eq!(cap_of(&c, "acme.noai"), None);
        assert!(grants_of(&c, "acme.noai").is_empty());
    }

    /// One transaction: when the limit cannot be written, the yes is not either.
    #[test]
    fn a_limit_that_fails_to_write_takes_the_grant_with_it() {
        let (_dir, mut c) = test_store();
        chat(&c);
        c.execute_batch(
            "CREATE TRIGGER refuse_cap BEFORE UPDATE OF ai_cap_usd ON mods \
             BEGIN SELECT RAISE(ABORT, 'no'); END;",
        )
        .unwrap();
        let failed = decide(&mut c, &id("acme.chat"), "ai", true, Some(1.0), true);
        assert!(matches!(failed, Err(DecideError::Grant(GrantError::Db(_)))));
        assert!(grants_of(&c, "acme.chat").is_empty(), "the yes was undone");
        assert_eq!(cap_of(&c, "acme.chat"), None);
    }

    /// A yes with no limit writes the grant and leaves the limit as it was; a no never touches it.
    /// What stops a paid call with no limit is the call's own state check, not this write.
    #[test]
    fn a_decision_with_no_limit_leaves_the_limit_alone() {
        let (_dir, mut c) = test_store();
        chat(&c);
        decide(&mut c, &id("acme.chat"), "ai", true, None, true).unwrap();
        assert_eq!(cap_of(&c, "acme.chat"), None);
        c.execute("UPDATE mods SET ai_cap_usd = 4.0", []).unwrap();
        decide(&mut c, &id("acme.chat"), "ai", false, None, true).unwrap();
        assert_eq!(cap_of(&c, "acme.chat"), Some(4.0));
        decide(&mut c, &id("acme.chat"), "ai", true, None, true).unwrap();
        assert_eq!(cap_of(&c, "acme.chat"), Some(4.0));
        // A new limit replaces it.
        decide(&mut c, &id("acme.chat"), "ai", true, Some(0.5), true).unwrap();
        assert_eq!(cap_of(&c, "acme.chat"), Some(0.5));
    }

    /// A mod the safety list removed takes no limit either.
    #[test]
    fn a_removed_mod_takes_no_limit() {
        let (_dir, mut c) = test_store();
        insert(
            &c,
            &Row {
                id: "acme.chat",
                signed: true,
                tier: "second",
                manifest: manifest(&["ai"]),
                state: "paused",
                reason: Some("blacklist-removed"),
                ..Row::default()
            },
        );
        assert!(matches!(
            decide(&mut c, &id("acme.chat"), "ai", true, Some(1.0), true),
            Err(DecideError::Removed)
        ));
        assert_eq!(cap_of(&c, "acme.chat"), None);
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
        let tell = decide(&mut c, &id("acme.two"), "internet", true, None, true).unwrap();
        let mut tell: Vec<&str> = tell.iter().map(ModId::as_str).collect();
        tell.sort();
        assert_eq!(tell, ["acme.one", "acme.two"]);
    }

    // ---- an unsigned mod's plan ----------------------------------------------------------------

    const READ: (PolicyTier, Class) = (Unsigned, Class::Read);
    const NET: (PolicyTier, Class) = (Unsigned, Class::Egress);

    fn unsigned_plan(
        declared: &[&str],
        said: &[(&str, bool)],
        held: &[(PolicyTier, Class)],
        sealed: bool,
    ) -> Result<GrantPlan, PlanError> {
        plan_unsigned(&set(declared), &answers(said), held, sealed)
    }

    #[test]
    fn an_unsigned_mods_answers_are_strict_like_a_signed_ones() {
        let declared = ["documents.meta", "storage"];
        assert_eq!(
            unsigned_plan(&declared, &[], &[], true),
            Err(PlanError::Missing("documents.meta".into())),
            "asked one by one, so each needs its answer"
        );
        assert_eq!(
            unsigned_plan(
                &declared,
                &[("documents.meta", true), ("internet", true)],
                &[],
                true
            ),
            Err(PlanError::Undeclared("internet".into()))
        );
        assert_eq!(
            unsigned_plan(
                &declared,
                &[("documents.meta", true), ("made.up", true)],
                &[],
                true
            ),
            Err(PlanError::Unknown("made.up".into()))
        );
        // Automatic permissions need no answer.
        assert!(unsigned_plan(&["storage"], &[], &[], true)
            .unwrap()
            .touched()
            .next()
            .is_none());
    }

    #[test]
    fn an_unsigned_yes_is_a_decision_a_no_is_a_denial_and_first_use_waits() {
        let declared = ["documents.meta", "tasks.write", "ai", "storage"];
        let p = unsigned_plan(
            &declared,
            &[
                ("documents.meta", true),
                ("tasks.write", false),
                ("ai", true),
                ("storage", true),
            ],
            &[],
            true,
        )
        .unwrap();
        assert_eq!(p.grant_asked, ["documents.meta"]);
        assert!(
            p.grant.is_empty(),
            "nothing is install consent for an unsigned mod"
        );
        assert_eq!(p.deny, ["tasks.write"]);
        assert_eq!(
            p.touched().collect::<Vec<_>>(),
            ["documents.meta", "tasks.write"],
            "ai (first use) and storage (automatic) write nothing"
        );
        // A no to everything is the plan that installs nothing granted.
        let none = unsigned_plan(
            &declared,
            &[("documents.meta", false), ("tasks.write", false)],
            &[],
            true,
        )
        .unwrap();
        assert!(none.grant_asked.is_empty());
    }

    #[test]
    fn both_sides_of_one_unsigned_manifest_are_refused_before_anything_is_written() {
        let both = ["documents.meta", "internet"];
        assert_eq!(
            unsigned_plan(
                &both,
                &[("documents.meta", true), ("internet", true)],
                &[],
                true
            ),
            Err(PlanError::BothSides)
        );
        // `links` is egress too: read and links together are as refused as read and internet.
        assert_eq!(
            unsigned_plan(
                &["documents.text", "links"],
                &[("documents.text", true), ("links", true)],
                &[],
                true
            ),
            Err(PlanError::BothSides)
        );
        // One side is the user's choice, and a no to both is always fine.
        assert!(unsigned_plan(
            &both,
            &[("documents.meta", true), ("internet", false)],
            &[],
            true
        )
        .is_ok());
        assert!(unsigned_plan(
            &both,
            &[("documents.meta", false), ("internet", false)],
            &[],
            true
        )
        .is_ok());
    }

    #[test]
    fn the_other_unsigned_mods_holdings_refuse_the_opposite_side_before_anything_is_written() {
        let both = ["documents.meta", "internet"];
        // One side, with the other side held by another unsigned mod.
        assert_eq!(
            unsigned_plan(
                &both,
                &[("documents.meta", true), ("internet", false)],
                &[NET],
                true
            ),
            Err(PlanError::Exclusion("documents.meta".into()))
        );
        assert_eq!(
            unsigned_plan(
                &both,
                &[("documents.meta", false), ("internet", true)],
                &[READ],
                true
            ),
            Err(PlanError::Exclusion("internet".into()))
        );
        // Another mod holding the SAME side, or holding nothing, is fine.
        assert!(unsigned_plan(
            &both,
            &[("documents.meta", true), ("internet", false)],
            &[READ],
            true
        )
        .is_ok());
        assert!(unsigned_plan(
            &both,
            &[("documents.meta", false), ("internet", true)],
            &[NET],
            true
        )
        .is_ok());
        // A no is always allowed, under any holdings.
        assert!(unsigned_plan(
            &both,
            &[("documents.meta", false), ("internet", false)],
            &[READ, NET],
            true
        )
        .is_ok());
    }

    #[test]
    fn an_unsealed_engine_refuses_an_unsigned_read_but_not_the_internet() {
        let both = ["documents.meta", "internet"];
        assert_eq!(
            unsigned_plan(
                &both,
                &[("documents.meta", true), ("internet", false)],
                &[],
                false
            ),
            Err(PlanError::UnsealedEngine("documents.meta".into()))
        );
        assert!(unsigned_plan(
            &both,
            &[("documents.meta", false), ("internet", true)],
            &[],
            false
        )
        .is_ok());
    }

    #[test]
    fn an_unsigned_plan_is_written_as_decisions_and_the_writers_check_d3_again() {
        let (_d, mut c) = test_store();
        insert(
            &c,
            &Row {
                id: "acme.one",
                signed: false,
                tier: "third",
                installed_from: "file",
                manifest: manifest(&["documents.meta", "internet", "storage"]),
                ..Row::default()
            },
        );
        let plan = unsigned_plan(
            &["documents.meta", "internet", "storage"],
            &[("documents.meta", true), ("internet", false)],
            &[],
            true,
        )
        .unwrap();
        apply_atomically(&mut c, &id("acme.one"), &plan, true).unwrap();
        assert_eq!(
            grants_of(&c, "acme.one"),
            BTreeMap::from([
                ("documents.meta".into(), "granted".into()),
                ("internet".into(), "denied".into()),
            ])
        );
        // A plan built against stale holdings is still refused when written: whole or not at all.
        c.execute("DELETE FROM mod_grants", []).unwrap();
        let stale = GrantPlan {
            grant_asked: vec!["documents.meta".into(), "internet".into()],
            ..GrantPlan::default()
        };
        assert!(apply_atomically(&mut c, &id("acme.one"), &stale, true).is_err());
        assert!(grants_of(&c, "acme.one").is_empty(), "nothing written");
    }

    #[test]
    fn every_refusal_of_an_unsigned_plan_has_its_words() {
        for e in [
            PlanError::BothSides,
            PlanError::Exclusion("documents.meta".into()),
            PlanError::UnsealedEngine("documents.meta".into()),
        ] {
            let said = e.to_string();
            assert!(
                said.contains("never both") || said.contains("sealed"),
                "{said}"
            );
        }
    }
}
