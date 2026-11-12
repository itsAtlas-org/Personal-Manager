// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The grant model (Mods spec §1.5, D3; plan decision 2): whether a mod may use a permission now,
//! and the two writers of `mod_grants`.
//!
//! - **Rows.** `mod_grants` holds each decision the user made: granted or denied. A permission whose
//!   policy is `Auto` needs no row; `Install` needs a granted row, written when the user consents to
//!   the install ([`record_install_consent`]); one asked at first use, or one by one (unsigned), with
//!   no row yet is [`Outcome::NeedsPrompt`], which the page turns into a PM-drawn prompt (slice 6)
//!   whose answer is recorded by [`set_decision`]. A denied row refuses whatever the policy.
//! - **Declared first.** A mod can only ever use what its manifest declares: a row for a permission
//!   the installed manifest doesn't list (an update dropped it) grants nothing.
//! - **D3 and the engine.** Unsigned mods are judged together ([`permissions::EXCLUSIONS`]), on every
//!   call and on every grant change; and none gets a read-class grant while the engine is unsealed
//!   ([`permissions::NEEDS_SEALED_ENGINE`]). Both refuse a stored grant too, so a store restored
//!   from elsewhere, or edited by hand, can't carry a forbidden pair into use.
//!
//! [`decide`] is the whole decision, pure; the rest reads its facts and writes rows.

use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use super::error::{Code, ModError};
use super::permissions::{self, Class, Policy, PolicyTier, When};
use crate::mods::ids::ModId;
use crate::mods::package::DataClass;
use crate::mods::store;

/// A stored decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Granted,
    Denied,
}

impl Decision {
    pub fn stored(self) -> &'static str {
        match self {
            Self::Granted => store::GRANTED,
            Self::Denied => store::DENIED,
        }
    }

    fn from_stored(s: &str) -> Option<Self> {
        match s {
            store::GRANTED => Some(Self::Granted),
            store::DENIED => Some(Self::Denied),
            _ => None,
        }
    }
}

/// Why a permission isn't usable, when asking wouldn't change it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The installed manifest doesn't declare it.
    Undeclared,
    /// The user said no.
    Denied,
    /// An install-time permission the user never consented to.
    NotConsented,
    /// An unsigned mod's read while the engine isn't sealed.
    UnsealedEngine,
    /// D3: another unsigned mod (or this one) holds the other side.
    Exclusion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Granted,
    NeedsPrompt,
    Refused(Refused),
}

/// What [`decide`] needs to know about one mod and one permission.
#[derive(Debug, Clone)]
pub struct Facts<'a> {
    pub tier: PolicyTier,
    pub declared: bool,
    pub decision: Option<Decision>,
    /// Every grant installed mods hold, as `(tier, class)`, this mod's included. Only read when an
    /// exclusion could apply ([`permissions::exclusion_applies`]); empty otherwise.
    pub held: &'a [(PolicyTier, Class)],
    pub engine_sealed: bool,
}

/// The grant decision. Order: what the mod declared, what the user said no to, the rules that hold
/// whatever the user said (the sealed engine, D3), then a yes on record, then the policy.
pub fn decide(policy: &Policy, f: &Facts<'_>) -> Outcome {
    if !f.declared {
        return Outcome::Refused(Refused::Undeclared);
    }
    if f.decision == Some(Decision::Denied) {
        return Outcome::Refused(Refused::Denied);
    }
    if !permissions::allowed_by_engine(f.tier, policy.class, f.engine_sealed) {
        return Outcome::Refused(Refused::UnsealedEngine);
    }
    if permissions::excluded(f.tier, policy.class, f.held.iter().copied()) {
        return Outcome::Refused(Refused::Exclusion);
    }
    if f.decision == Some(Decision::Granted) {
        return Outcome::Granted;
    }
    match policy.when(f.tier) {
        When::Auto => Outcome::Granted,
        When::Install => Outcome::Refused(Refused::NotConsented),
        When::InstallThenFirstUse | When::FirstUse | When::Ask => Outcome::NeedsPrompt,
    }
}

/// An installed mod as the grant check and the handlers need it, read from its `mods` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModFacts {
    pub id: ModId,
    pub state: String,
    pub consented: bool,
    pub tier: PolicyTier,
    /// The permission ids its installed manifest declares.
    pub declared: BTreeSet<String>,
    /// Its manifest's storage keys and their classes (`data.keys`).
    pub data_classes: BTreeMap<String, DataClass>,
}

impl ModFacts {
    /// Whether the mod may run: active, and turned on by the user (D17).
    pub fn running(&self) -> Result<(), ModError> {
        if self.state == store::STATE_ACTIVE && self.consented {
            return Ok(());
        }
        let message = match self.state.as_str() {
            store::STATE_PAUSED => "This mod is paused.",
            store::STATE_PARKED => "This mod is parked.",
            store::STATE_INSTALLING => "This mod is still installing.",
            store::STATE_ACTIVE => "This mod hasn't been turned on yet.",
            _ => "This mod isn't running.",
        };
        Err(ModError::new(Code::Paused, message))
    }
}

/// What a stored manifest declares: its permission ids and its storage keys' classes. A manifest
/// that can't be read (the ledger parks such a row at startup) declares nothing.
pub fn manifest_declarations(manifest: &str) -> (BTreeSet<String>, BTreeMap<String, DataClass>) {
    let Ok(value) = serde_json::from_str::<Value>(manifest) else {
        return Default::default();
    };
    let declared = value["permissions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["id"].as_str().map(str::to_owned))
        .collect();
    let classes = value["data"]["keys"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|k| {
            let key = k["key"].as_str()?.to_owned();
            let class = serde_json::from_value::<DataClass>(k["class"].clone()).ok()?;
            Some((key, class))
        })
        .collect();
    (declared, classes)
}

/// The installed mod `id`, or `None` when it has no row (uninstalled).
pub fn read_mod(conn: &Connection, id: &ModId) -> rusqlite::Result<Option<ModFacts>> {
    conn.query_row(
        "SELECT state, consented_at IS NOT NULL, signed, tier, manifest FROM mods WHERE id = ?1",
        [id.as_str()],
        |r| {
            let manifest: String = r.get(4)?;
            let (declared, data_classes) = manifest_declarations(&manifest);
            Ok(ModFacts {
                id: id.clone(),
                state: r.get(0)?,
                consented: r.get(1)?,
                tier: PolicyTier::of(r.get(2)?, &r.get::<_, String>(3)?),
                declared,
                data_classes,
            })
        },
    )
    .optional()
}

fn read_decision(
    conn: &Connection,
    id: &ModId,
    permission: &str,
) -> rusqlite::Result<Option<Decision>> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT decision FROM mod_grants WHERE mod_id = ?1 AND permission = ?2",
            params![id.as_str(), permission],
            |r| r.get(0),
        )
        .optional()?;
    // The column's CHECK allows only the two words; anything else reads as no decision.
    Ok(stored.as_deref().and_then(Decision::from_stored))
}

/// Every permission installed mods of `tier` hold now, as `(tier, class)`: a declared permission
/// with a granted row, or with no row and an `Auto` policy. What D3 judges, across the tier's
/// whole set, paused and parked mods included (a pause keeps its grants).
pub fn holdings(conn: &Connection, tier: PolicyTier) -> rusqlite::Result<Vec<(PolicyTier, Class)>> {
    holdings_except(conn, tier, None)
}

/// [`holdings`] without `except`'s own: what the rest of the tier holds. An unsigned file installed
/// over an earlier install of the same id starts with no grants (its old ones are reset), so the
/// screen and the plan judge it against everyone else's.
pub fn holdings_except(
    conn: &Connection,
    tier: PolicyTier,
    except: Option<&ModId>,
) -> rusqlite::Result<Vec<(PolicyTier, Class)>> {
    let mut mods = conn.prepare("SELECT id, signed, tier, manifest FROM mods")?;
    let rows = mods.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, bool>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    let mut decisions: BTreeMap<(String, String), Option<Decision>> = BTreeMap::new();
    let mut grants = conn.prepare("SELECT mod_id, permission, decision FROM mod_grants")?;
    for row in grants.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (mod_id, permission, decision) = row?;
        decisions.insert((mod_id, permission), Decision::from_stored(&decision));
    }
    let mut held = Vec::new();
    for row in rows {
        let (id, signed, stored_tier, manifest) = row?;
        let this_tier = PolicyTier::of(signed, &stored_tier);
        if this_tier != tier || except.is_some_and(|e| e.as_str() == id) {
            continue;
        }
        let (declared, _) = manifest_declarations(&manifest);
        for permission in declared {
            let Some(policy) = permissions::policy(&permission) else {
                continue;
            };
            let holds = match decisions.get(&(id.clone(), permission.clone())) {
                Some(Some(Decision::Granted)) => true,
                Some(_) => false,
                None => policy.when(tier) == When::Auto,
            };
            if holds {
                held.push((tier, policy.class));
            }
        }
    }
    Ok(held)
}

/// May `facts`' mod use `permission` now? Reads the decision, and the tier's holdings only when an
/// exclusion could apply. The caller holds the DB guard (or a transaction) and passes it here.
pub fn check(
    conn: &Connection,
    facts: &ModFacts,
    permission: &'static str,
    engine_sealed: bool,
) -> Result<(), ModError> {
    let Some(policy) = permissions::policy(permission) else {
        // A dispatch row naming a permission the policy table lacks: the table tests forbid it.
        return Err(ModError::internal(format!("no policy for {permission}")));
    };
    let decision = read_decision(conn, &facts.id, permission).map_err(ModError::internal)?;
    let held = if permissions::exclusion_applies(facts.tier, policy.class) {
        holdings(conn, facts.tier).map_err(ModError::internal)?
    } else {
        Vec::new()
    };
    let outcome = decide(
        policy,
        &Facts {
            tier: facts.tier,
            declared: facts.declared.contains(permission),
            decision,
            held: &held,
            engine_sealed,
        },
    );
    match outcome {
        Outcome::Granted => Ok(()),
        Outcome::NeedsPrompt => Err(ModError::new(
            Code::NeedsPrompt,
            format!("This needs your permission first ({permission})."),
        )
        .with_permission(policy.id)),
        // D3's exclusion is told to the frame as a plain "off": its own words name the other side
        // ("another unsigned mod already holds the opposite"), which would show a mod something about
        // the mods beside it. The user gets the explanation on PM's own surface (`GrantError`).
        Outcome::Refused(Refused::Exclusion) => Err(refusal(policy.id, Refused::Denied)),
        Outcome::Refused(why) => Err(refusal(policy.id, why)),
    }
}

fn refusal(permission: &'static str, why: Refused) -> ModError {
    let message = match why {
        Refused::Undeclared => "This mod didn't ask for that permission.",
        Refused::Denied => "That permission is off for this mod.",
        Refused::NotConsented => "That permission wasn't granted when this mod was installed.",
        Refused::UnsealedEngine => {
            "Unsigned mods can't read your data until PM's web engine is sealed."
        }
        Refused::Exclusion => {
            "Unsigned mods may read your data or reach the internet, never both, and another \
             unsigned mod (or this one) already holds the other."
        }
    };
    ModError::new(Code::NotGranted, message).with_permission(permission)
}

/// Why a grant couldn't be recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantError {
    NotInstalled,
    /// Not a permission API 1.0 has.
    Unknown(String),
    /// Not one the installed manifest declares.
    Undeclared(String),
    /// A rule refuses this grant whatever the user says (D3, the sealed engine).
    Refused(String, Refused),
    Db(String),
}

impl std::fmt::Display for GrantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInstalled => f.write_str("That mod isn't installed."),
            Self::Unknown(p) => write!(f, "{p:?} isn't a permission PM has."),
            Self::Undeclared(p) => write!(f, "This mod didn't ask for {p:?}."),
            Self::Refused(p, why) => write!(f, "{} ({p})", refusal("", *why).message),
            Self::Db(e) => write!(f, "PM couldn't record the permission ({e})."),
        }
    }
}

impl std::error::Error for GrantError {}

impl From<rusqlite::Error> for GrantError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e.to_string())
    }
}

/// Run `f` inside a savepoint: whole or not at all, whether or not the caller is already inside a
/// transaction (the install's own commit may be).
fn atomically<T>(
    conn: &Connection,
    f: impl FnOnce(&Connection) -> Result<T, GrantError>,
) -> Result<T, GrantError> {
    conn.execute_batch("SAVEPOINT mod_grants_write")?;
    match f(conn) {
        Ok(v) => {
            conn.execute_batch("RELEASE mod_grants_write")?;
            Ok(v)
        }
        Err(e) => {
            // Undo, then leave the savepoint; the first error is the one worth reporting.
            let _ = conn.execute_batch("ROLLBACK TO mod_grants_write; RELEASE mod_grants_write");
            Err(e)
        }
    }
}

fn write_row(
    conn: &Connection,
    id: &ModId,
    permission: &str,
    decision: Decision,
) -> rusqlite::Result<()> {
    let now = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    conn.execute(
        "INSERT INTO mod_grants(mod_id, permission, decision, decided_at) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(mod_id, permission) DO UPDATE SET decision = ?3, decided_at = ?4",
        params![id.as_str(), permission, decision.stored(), now],
    )?;
    Ok(())
}

/// The installed mod and the policy of a permission it declares, or why not.
fn declared_policy(
    conn: &Connection,
    id: &ModId,
    permission: &str,
) -> Result<(ModFacts, &'static Policy), GrantError> {
    let facts = read_mod(conn, id)?.ok_or(GrantError::NotInstalled)?;
    let policy = permissions::policy(permission)
        .ok_or_else(|| GrantError::Unknown(permission.to_owned()))?;
    if !facts.declared.contains(permission) {
        return Err(GrantError::Undeclared(permission.to_owned()));
    }
    Ok((facts, policy))
}

/// Whether the rules that hold whatever the user says allow granting `policy` to `facts`' mod.
fn grantable(
    conn: &Connection,
    facts: &ModFacts,
    policy: &Policy,
    engine_sealed: bool,
) -> Result<(), GrantError> {
    if !permissions::allowed_by_engine(facts.tier, policy.class, engine_sealed) {
        return Err(GrantError::Refused(
            policy.id.to_owned(),
            Refused::UnsealedEngine,
        ));
    }
    if permissions::exclusion_applies(facts.tier, policy.class) {
        let held = holdings(conn, facts.tier)?;
        if permissions::excluded(facts.tier, policy.class, held) {
            return Err(GrantError::Refused(
                policy.id.to_owned(),
                Refused::Exclusion,
            ));
        }
    }
    Ok(())
}

/// Record the user's consent on the install screen: a granted row for every permission in
/// `permissions` whose policy for this mod's tier is `Install` (plan decision 2: explicit,
/// auditable, revocable one by one). `Auto` ones need no row, and those asked at first use are left
/// for their prompt, even when the install screen listed them. Every id must be one the installed
/// manifest declares. All or nothing. Returns the permissions recorded.
///
/// The Mods tab's install (`tab::install::finish`) calls it right after the ledger's commit, and the
/// first-open "Turn on" (`consent_mod`) inside its transaction, both through `tab::consent::apply`.
pub fn record_install_consent(
    conn: &Connection,
    id: &ModId,
    permissions: &[&str],
    engine_sealed: bool,
) -> Result<Vec<&'static str>, GrantError> {
    atomically(conn, |conn| {
        let mut recorded = Vec::new();
        for permission in permissions {
            let (facts, policy) = declared_policy(conn, id, permission)?;
            if policy.when(facts.tier) != When::Install {
                continue;
            }
            grantable(conn, &facts, policy, engine_sealed)?;
            write_row(conn, id, policy.id, Decision::Granted)?;
            recorded.push(policy.id);
        }
        Ok(recorded)
    })
}

/// Record one decision: the user's answer to a first-use or unsigned prompt, or a toggle in the
/// mod's row of the Mods tab (`mod_permission_decide`). A grant D3 or the unsealed engine forbids is
/// refused, and nothing is written; a denial always is (taking a permission away is always allowed).
pub fn set_decision(
    conn: &Connection,
    id: &ModId,
    permission: &str,
    decision: Decision,
    engine_sealed: bool,
) -> Result<(), GrantError> {
    atomically(conn, |conn| {
        let (facts, policy) = declared_policy(conn, id, permission)?;
        if decision == Decision::Granted {
            grantable(conn, &facts, policy, engine_sealed)?;
        }
        write_row(conn, id, policy.id, decision)?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::api::permissions::POLICIES;
    use crate::mods::api::test_support::{insert_mod, manifest, store, ModRow};
    use PolicyTier::{First, Second, Unsigned};

    fn facts(tier: PolicyTier) -> Facts<'static> {
        Facts {
            tier,
            declared: true,
            decision: None,
            held: &[],
            engine_sealed: true,
        }
    }

    /// Every permission against every tier with no row: the policy table's answer, cell by cell.
    #[test]
    fn with_no_row_each_tier_gets_its_policy() {
        for p in POLICIES {
            for tier in [First, Second, Unsigned] {
                let want = match p.when(tier) {
                    When::Auto => Outcome::Granted,
                    When::Install => Outcome::Refused(Refused::NotConsented),
                    _ => Outcome::NeedsPrompt,
                };
                assert_eq!(decide(p, &facts(tier)), want, "{} {tier:?}", p.id);
            }
        }
    }

    #[test]
    fn a_row_decides_but_never_past_what_was_declared_or_a_rule() {
        let read = permissions::policy("documents.meta").unwrap();
        let egress = permissions::policy("internet").unwrap();
        let storage = permissions::policy("storage").unwrap();
        for tier in [First, Second, Unsigned] {
            let granted = Facts {
                decision: Some(Decision::Granted),
                ..facts(tier)
            };
            assert_eq!(decide(read, &granted), Outcome::Granted, "{tier:?}");
            let denied = Facts {
                decision: Some(Decision::Denied),
                ..facts(tier)
            };
            // A denial wins even over Auto.
            assert_eq!(decide(storage, &denied), Outcome::Refused(Refused::Denied));
            assert_eq!(decide(read, &denied), Outcome::Refused(Refused::Denied));
            let undeclared = Facts {
                declared: false,
                ..granted.clone()
            };
            assert_eq!(
                decide(read, &undeclared),
                Outcome::Refused(Refused::Undeclared)
            );
            assert_eq!(
                decide(
                    storage,
                    &Facts {
                        declared: false,
                        ..facts(tier)
                    }
                ),
                Outcome::Refused(Refused::Undeclared)
            );
        }
        // D3 refuses a stored grant too, on either side, and before any prompt.
        let held_egress = [(Unsigned, Class::Egress)];
        let held_read = [(Unsigned, Class::Read)];
        let granted = |held| Facts {
            decision: Some(Decision::Granted),
            held,
            ..facts(Unsigned)
        };
        assert_eq!(
            decide(read, &granted(&held_egress)),
            Outcome::Refused(Refused::Exclusion)
        );
        assert_eq!(
            decide(egress, &granted(&held_read)),
            Outcome::Refused(Refused::Exclusion)
        );
        let unasked = Facts {
            held: &held_egress,
            ..facts(Unsigned)
        };
        assert_eq!(decide(read, &unasked), Outcome::Refused(Refused::Exclusion));
        // Signed tiers aren't judged by unsigned holdings.
        for tier in [First, Second] {
            let f = Facts {
                decision: Some(Decision::Granted),
                held: &held_egress,
                ..facts(tier)
            };
            assert_eq!(decide(read, &f), Outcome::Granted, "{tier:?}");
        }
        // The unsealed engine: unsigned reads only, granted row or not.
        let unsealed = |tier, decision| Facts {
            decision,
            engine_sealed: false,
            ..facts(tier)
        };
        for decision in [None, Some(Decision::Granted)] {
            assert_eq!(
                decide(read, &unsealed(Unsigned, decision)),
                Outcome::Refused(Refused::UnsealedEngine)
            );
            assert_ne!(
                decide(egress, &unsealed(Unsigned, decision)),
                Outcome::Refused(Refused::UnsealedEngine)
            );
        }
        assert_eq!(
            decide(read, &unsealed(First, Some(Decision::Granted))),
            Outcome::Granted
        );
        assert_eq!(decide(storage, &unsealed(Unsigned, None)), Outcome::Granted);
    }

    #[test]
    fn a_stored_manifest_declares_its_permissions_and_storage_classes() {
        let (declared, classes) = manifest_declarations(&manifest(
            &["storage", "documents.meta"],
            &[("settings", "content"), ("layout", "cache")],
        ));
        assert_eq!(
            declared,
            BTreeSet::from(["storage".to_owned(), "documents.meta".to_owned()])
        );
        assert_eq!(classes.get("settings"), Some(&DataClass::Content));
        assert_eq!(classes.get("layout"), Some(&DataClass::Cache));
        assert_eq!(manifest_declarations("not json"), Default::default());
        assert_eq!(manifest_declarations("{}"), Default::default());
    }

    #[test]
    fn running_means_active_and_turned_on() {
        let mut f = ModFacts {
            id: ModId::parse("acme.tool").unwrap(),
            state: store::STATE_ACTIVE.into(),
            consented: true,
            tier: First,
            declared: BTreeSet::new(),
            data_classes: BTreeMap::new(),
        };
        assert!(f.running().is_ok());
        f.consented = false;
        assert_eq!(f.running().unwrap_err().code, Code::Paused);
        f.consented = true;
        for state in [
            store::STATE_PAUSED,
            store::STATE_PARKED,
            store::STATE_INSTALLING,
            "anything",
        ] {
            f.state = state.into();
            assert_eq!(f.running().unwrap_err().code, Code::Paused, "{state}");
        }
    }

    fn id(s: &str) -> ModId {
        ModId::parse(s).unwrap()
    }

    fn decision_of(conn: &Connection, m: &str, p: &str) -> Option<String> {
        conn.query_row(
            "SELECT decision FROM mod_grants WHERE mod_id = ?1 AND permission = ?2",
            params![m, p],
            |r| r.get(0),
        )
        .optional()
        .unwrap()
    }

    #[test]
    fn install_consent_writes_exactly_the_install_time_grants() {
        let (_dir, conn) = store();
        let perms = [
            "storage",
            "documents.meta",
            "documents.text",
            "calendar.details",
            "ai",
            "internet",
        ];
        insert_mod(&conn, &ModRow::signed("acme.second", "second", &perms));
        let recorded = record_install_consent(&conn, &id("acme.second"), &perms, true).unwrap();
        // Second party: meta and internet at install; text and calendar.details wait for their
        // first use; storage is auto; ai is first use.
        assert_eq!(recorded, ["documents.meta", "internet"]);
        for p in ["documents.meta", "internet"] {
            assert_eq!(
                decision_of(&conn, "acme.second", p).as_deref(),
                Some("granted")
            );
        }
        for p in ["storage", "documents.text", "calendar.details", "ai"] {
            assert_eq!(decision_of(&conn, "acme.second", p), None, "{p}");
        }
        // First party: text is install-time there.
        insert_mod(&conn, &ModRow::signed("atlas.tool", "first", &perms));
        let recorded = record_install_consent(&conn, &id("atlas.tool"), &perms, true).unwrap();
        assert_eq!(
            recorded,
            [
                "documents.meta",
                "documents.text",
                "calendar.details",
                "internet"
            ]
        );
        // Unsigned: nothing is install-time.
        insert_mod(&conn, &ModRow::unsigned("acme.free", &perms));
        assert!(
            record_install_consent(&conn, &id("acme.free"), &perms, true)
                .unwrap()
                .is_empty()
        );
        // Consent overrides an earlier denial (an update the user re-approved).
        set_decision(
            &conn,
            &id("acme.second"),
            "documents.meta",
            Decision::Denied,
            true,
        )
        .unwrap();
        record_install_consent(&conn, &id("acme.second"), &["documents.meta"], true).unwrap();
        assert_eq!(
            decision_of(&conn, "acme.second", "documents.meta").as_deref(),
            Some("granted")
        );
    }

    #[test]
    fn install_consent_refuses_what_isnt_declared_or_known_and_writes_nothing() {
        let (_dir, conn) = store();
        insert_mod(
            &conn,
            &ModRow::signed("acme.second", "second", &["documents.meta"]),
        );
        let m = id("acme.second");
        assert_eq!(
            record_install_consent(&conn, &m, &["documents.meta", "tasks.read"], true),
            Err(GrantError::Undeclared("tasks.read".into()))
        );
        assert_eq!(
            record_install_consent(&conn, &m, &["documents.meta", "root"], true),
            Err(GrantError::Unknown("root".into()))
        );
        // All or nothing: the first, valid one wasn't kept.
        assert_eq!(decision_of(&conn, "acme.second", "documents.meta"), None);
        assert_eq!(
            record_install_consent(&conn, &id("acme.gone"), &["storage"], true),
            Err(GrantError::NotInstalled)
        );
    }

    /// D3 at grant time, across two unsigned mods: a read for one refuses internet for the other,
    /// and the reverse; a denial always goes through; signed mods are free of it; and nothing is
    /// written by a refusal.
    #[test]
    fn grants_to_unsigned_mods_are_judged_across_the_set() {
        let (_dir, conn) = store();
        insert_mod(
            &conn,
            &ModRow::unsigned("acme.reader", &["documents.meta", "internet"]),
        );
        insert_mod(
            &conn,
            &ModRow::unsigned("acme.poster", &["documents.meta", "internet"]),
        );
        insert_mod(
            &conn,
            &ModRow::signed("acme.signed", "second", &["documents.meta", "internet"]),
        );
        let reader = id("acme.reader");
        let poster = id("acme.poster");
        set_decision(&conn, &reader, "documents.meta", Decision::Granted, true).unwrap();
        assert_eq!(
            set_decision(&conn, &poster, "internet", Decision::Granted, true),
            Err(GrantError::Refused("internet".into(), Refused::Exclusion))
        );
        assert_eq!(decision_of(&conn, "acme.poster", "internet"), None);
        // The reader itself can't take internet either.
        assert_eq!(
            set_decision(&conn, &reader, "internet", Decision::Granted, true),
            Err(GrantError::Refused("internet".into(), Refused::Exclusion))
        );
        // Another read is fine; a signed mod's internet is fine.
        set_decision(&conn, &poster, "documents.meta", Decision::Granted, true).unwrap();
        set_decision(
            &conn,
            &id("acme.signed"),
            "internet",
            Decision::Granted,
            true,
        )
        .unwrap();
        // Once every unsigned read is denied, internet may be granted, and then reads are refused.
        set_decision(&conn, &reader, "documents.meta", Decision::Denied, true).unwrap();
        set_decision(&conn, &poster, "documents.meta", Decision::Denied, true).unwrap();
        set_decision(&conn, &poster, "internet", Decision::Granted, true).unwrap();
        assert_eq!(
            set_decision(&conn, &reader, "documents.meta", Decision::Granted, true),
            Err(GrantError::Refused(
                "documents.meta".into(),
                Refused::Exclusion
            ))
        );
        assert_eq!(
            decision_of(&conn, "acme.reader", "documents.meta").as_deref(),
            Some("denied")
        );
        // A paused unsigned mod's grant still counts.
        conn.execute(
            "UPDATE mods SET state = 'paused' WHERE id = 'acme.poster'",
            [],
        )
        .unwrap();
        assert!(set_decision(&conn, &reader, "documents.meta", Decision::Granted, true).is_err());
    }

    /// What an unsigned file's screen and plan judge a fresh install of an id against: everyone
    /// else's grants. Its own old ones are about to be reset, so they must not count.
    #[test]
    fn holdings_except_leaves_out_one_mods_own_grants_and_nothing_else() {
        let (_dir, conn) = store();
        insert_mod(
            &conn,
            &ModRow::unsigned("acme.reader", &["documents.meta", "internet"]),
        );
        insert_mod(&conn, &ModRow::unsigned("acme.poster", &["internet"]));
        set_decision(
            &conn,
            &id("acme.reader"),
            "documents.meta",
            Decision::Granted,
            true,
        )
        .unwrap();
        set_decision(
            &conn,
            &id("acme.poster"),
            "internet",
            Decision::Denied,
            true,
        )
        .unwrap();
        let all = holdings(&conn, Unsigned).unwrap();
        assert!(all.contains(&(Unsigned, Class::Read)));
        // Without the reader, nothing reads: the rest holds no read.
        let rest = holdings_except(&conn, Unsigned, Some(&id("acme.reader"))).unwrap();
        assert!(!rest.contains(&(Unsigned, Class::Read)));
        // Excluding nobody is `holdings`; excluding another id changes nothing.
        assert_eq!(holdings_except(&conn, Unsigned, None).unwrap(), all);
        assert_eq!(
            holdings_except(&conn, Unsigned, Some(&id("acme.poster"))).unwrap(),
            all
        );
    }

    #[test]
    fn an_unsealed_engine_refuses_unsigned_read_grants_only() {
        let (_dir, conn) = store();
        insert_mod(
            &conn,
            &ModRow::unsigned("acme.free", &["documents.meta", "internet"]),
        );
        insert_mod(
            &conn,
            &ModRow::signed("acme.signed", "second", &["documents.meta"]),
        );
        assert_eq!(
            set_decision(
                &conn,
                &id("acme.free"),
                "documents.meta",
                Decision::Granted,
                false
            ),
            Err(GrantError::Refused(
                "documents.meta".into(),
                Refused::UnsealedEngine
            ))
        );
        assert_eq!(decision_of(&conn, "acme.free", "documents.meta"), None);
        set_decision(
            &conn,
            &id("acme.free"),
            "internet",
            Decision::Granted,
            false,
        )
        .unwrap();
        set_decision(
            &conn,
            &id("acme.signed"),
            "documents.meta",
            Decision::Granted,
            false,
        )
        .unwrap();
        record_install_consent(&conn, &id("acme.signed"), &["documents.meta"], false).unwrap();
    }

    #[test]
    fn a_decision_needs_an_installed_mod_and_a_declared_known_permission() {
        let (_dir, conn) = store();
        insert_mod(&conn, &ModRow::signed("acme.tool", "second", &["storage"]));
        let m = id("acme.tool");
        assert_eq!(
            set_decision(&conn, &m, "tasks.read", Decision::Granted, true),
            Err(GrantError::Undeclared("tasks.read".into()))
        );
        assert_eq!(
            set_decision(&conn, &m, "everything", Decision::Denied, true),
            Err(GrantError::Unknown("everything".into()))
        );
        assert_eq!(
            set_decision(&conn, &id("acme.none"), "storage", Decision::Denied, true),
            Err(GrantError::NotInstalled)
        );
        set_decision(&conn, &m, "storage", Decision::Denied, true).unwrap();
        assert_eq!(
            decision_of(&conn, "acme.tool", "storage").as_deref(),
            Some("denied")
        );
        // A decision written inside the caller's own transaction rolls back with it.
        conn.execute_batch("BEGIN").unwrap();
        set_decision(&conn, &m, "storage", Decision::Granted, true).unwrap();
        conn.execute_batch("ROLLBACK").unwrap();
        assert_eq!(
            decision_of(&conn, "acme.tool", "storage").as_deref(),
            Some("denied")
        );
    }

    #[test]
    fn holdings_count_declared_grants_and_auto_ones_of_the_tier_only() {
        let (_dir, conn) = store();
        insert_mod(
            &conn,
            &ModRow::unsigned("acme.a", &["storage", "documents.meta", "internet"]),
        );
        insert_mod(
            &conn,
            &ModRow::signed("acme.b", "second", &["documents.meta"]),
        );
        // A granted row for something the manifest no longer declares grants nothing.
        conn.execute_batch(
            "INSERT INTO mod_grants(mod_id, permission, decision) VALUES
               ('acme.a', 'documents.meta', 'granted'),
               ('acme.a', 'tasks.read', 'granted'),
               ('acme.b', 'documents.meta', 'granted');",
        )
        .unwrap();
        let mut held = holdings(&conn, Unsigned).unwrap();
        held.sort_by_key(|(_, c)| *c);
        assert_eq!(held, [(Unsigned, Class::Local), (Unsigned, Class::Read)]);
        assert_eq!(holdings(&conn, Second).unwrap(), [(Second, Class::Read)]);
        assert!(holdings(&conn, First).unwrap().is_empty());
    }
}
