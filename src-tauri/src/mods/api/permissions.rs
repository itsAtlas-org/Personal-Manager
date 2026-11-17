// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The permission policy table (Mods spec §1.5), as data: each permission's class, and for each
//! trust tier when PM asks for it. Plus D3's exclusion and the sealed-engine rule, also as data, so
//! opening either up later is an edit to a table, never to the code that reads it.
//!
//! The ids here are API 1.0's whole set; `package::PERMISSIONS` (what a manifest may declare) is
//! pinned to them by a test in both directions.

/// What a permission reaches, which decides how PM treats it (spec §1.5's class column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Class {
    /// The mod's own things: its storage, its tab, PM's theme.
    Local,
    /// The user's data.
    Read,
    /// Changes to the user's data (gifts).
    Write,
    /// Spends the user's money.
    Money,
    /// Reaches outside PM: the internet, the browser.
    Egress,
}

/// Who a mod is, for the policy: its tier when an Atlas signature vouched for it, and unsigned
/// otherwise, whatever tier its own manifest claims (a dev build's unsigned `atlas.` folder mod
/// included).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PolicyTier {
    First,
    Second,
    Unsigned,
}

impl PolicyTier {
    /// From a `mods` row's `signed` and `tier`. A signed row whose tier PM doesn't know (a hand
    /// edit) is treated as unsigned, the least trusted.
    pub fn of(signed: bool, tier: &str) -> Self {
        match (signed, tier) {
            (true, "first") => Self::First,
            (true, "second") => Self::Second,
            _ => Self::Unsigned,
        }
    }
}

/// When PM asks before a permission is usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    /// Never: declaring it is enough. No `mod_grants` row is needed.
    Auto,
    /// On the install screen: a granted row is written at consent
    /// (`grants::record_install_consent`).
    Install,
    /// Shown on the install screen, and asked again the first time it is used: the row is written
    /// by the first-use prompt, not at install.
    InstallThenFirstUse,
    /// The first time it is used.
    FirstUse,
    /// Asked one by one (unsigned mods): on the stark install screen or at first use, never in a
    /// batch.
    Ask,
}

/// One permission's row of the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    pub id: &'static str,
    pub class: Class,
    pub first: When,
    pub second: When,
    pub unsigned: When,
}

impl Policy {
    /// When a `tier` mod is asked for this permission.
    pub fn when(&self, tier: PolicyTier) -> When {
        match tier {
            PolicyTier::First => self.first,
            PolicyTier::Second => self.second,
            PolicyTier::Unsigned => self.unsigned,
        }
    }
}

const fn row(id: &'static str, class: Class, first: When, second: When, unsigned: When) -> Policy {
    Policy {
        id,
        class,
        first,
        second,
        unsigned,
    }
}

use Class::{Egress, Local, Money, Read, Write};
use When::{Ask as AskEach, Auto, FirstUse, Install, InstallThenFirstUse};

/// Spec §1.5's table. Where a cell says more than when to ask, the rest belongs to the call that
/// uses it: `ai`'s money prompt only for a paid provider (PR B's `ai.complete`), and `links`' PM-drawn
/// confirmation on every `shell.openLink` (PR C), which the second-party and unsigned rows need on
/// top of their grant.
pub const POLICIES: &[Policy] = &[
    row("storage", Local, Auto, Auto, Auto),
    row("ui.tab", Local, Auto, Auto, Auto),
    row("notify", Local, Auto, Install, AskEach),
    row("projects.read", Read, Install, Install, AskEach),
    row("tasks.read", Read, Install, Install, AskEach),
    row("documents.meta", Read, Install, Install, AskEach),
    row("documents.vectors", Read, Install, Install, AskEach),
    row(
        "documents.text",
        Read,
        Install,
        InstallThenFirstUse,
        AskEach,
    ),
    row("calendar.read", Read, Install, InstallThenFirstUse, AskEach),
    row("calendar.details", Read, Install, FirstUse, AskEach),
    row("tasks.write", Write, Install, Install, AskEach),
    row("milestones.write", Write, Install, Install, AskEach),
    row("documents.write", Write, Install, FirstUse, AskEach),
    row("ai", Money, FirstUse, FirstUse, FirstUse),
    row("internet", Egress, Install, Install, AskEach),
    row("links", Egress, Install, Install, AskEach),
];

pub fn policy(id: &str) -> Option<&'static Policy> {
    POLICIES.iter().find(|p| p.id == id)
}

/// Two sets of classes no mod of `tier` may hold one each of while any mod of that tier holds the
/// other, judged across every installed mod of the tier together (D3: two unsigned files from one
/// author could otherwise split the roles and talk through PM's own database).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exclusion {
    pub tier: PolicyTier,
    pub one: &'static [Class],
    pub other: &'static [Class],
}

/// D3: unsigned mods may read the user's data or reach outside PM, never both. `ai` is money, not
/// egress: its provider is PM's own ZDR route and the answer comes back only to the mod.
pub const EXCLUSIONS: &[Exclusion] = &[Exclusion {
    tier: PolicyTier::Unsigned,
    one: &[Read],
    other: &[Egress],
}];

/// What the engine must have sealed before a tier may use a class (spec §1.4, Step 0 item 3): an
/// unsealed engine may leak through WebRTC or a preconnect, so unsigned mods get no read grants
/// until it is sealed.
pub const NEEDS_SEALED_ENGINE: &[(PolicyTier, Class)] = &[(PolicyTier::Unsigned, Read)];

/// Whether a `tier` mod using `class` is refused by an exclusion, given `held`: every grant held by
/// installed mods, as `(tier, class)` (the caller's own included).
pub fn excluded(
    tier: PolicyTier,
    class: Class,
    held: impl IntoIterator<Item = (PolicyTier, Class)> + Clone,
) -> bool {
    EXCLUSIONS.iter().any(|ex| {
        if ex.tier != tier {
            return false;
        }
        let opposite = if ex.one.contains(&class) {
            ex.other
        } else if ex.other.contains(&class) {
            ex.one
        } else {
            return false;
        };
        held.clone()
            .into_iter()
            .any(|(t, c)| t == ex.tier && opposite.contains(&c))
    })
}

/// Whether judging `excluded` for a `tier` mod using `class` needs the holdings at all (so the
/// common call reads nothing more).
pub fn exclusion_applies(tier: PolicyTier, class: Class) -> bool {
    EXCLUSIONS
        .iter()
        .any(|ex| ex.tier == tier && (ex.one.contains(&class) || ex.other.contains(&class)))
}

/// Whether a `tier` mod may use `class` while the engine is (or isn't) sealed.
pub fn allowed_by_engine(tier: PolicyTier, class: Class, engine_sealed: bool) -> bool {
    engine_sealed || !NEEDS_SEALED_ENGINE.contains(&(tier, class))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::package::PERMISSIONS;
    use std::collections::BTreeSet;

    #[test]
    fn the_policy_table_and_the_manifests_permissions_are_the_same_set() {
        let table: BTreeSet<&str> = POLICIES.iter().map(|p| p.id).collect();
        assert_eq!(table.len(), POLICIES.len(), "a permission is listed twice");
        let manifest: BTreeSet<&str> = PERMISSIONS.iter().copied().collect();
        assert_eq!(manifest.len(), PERMISSIONS.len());
        assert_eq!(
            table.difference(&manifest).collect::<Vec<_>>(),
            Vec::<&&str>::new(),
            "in the policy table, not declarable"
        );
        assert_eq!(
            manifest.difference(&table).collect::<Vec<_>>(),
            Vec::<&&str>::new(),
            "declarable, with no policy"
        );
    }

    /// Spec §1.5's table, cell by cell, so an edit to it is a deliberate one.
    #[test]
    fn the_table_is_the_specs() {
        let cell = |id: &str| {
            let p = policy(id).unwrap();
            (p.class, p.first, p.second, p.unsigned)
        };
        assert_eq!(cell("storage"), (Local, Auto, Auto, Auto));
        assert_eq!(cell("ui.tab"), (Local, Auto, Auto, Auto));
        assert_eq!(cell("notify"), (Local, Auto, Install, AskEach));
        for id in [
            "projects.read",
            "tasks.read",
            "documents.meta",
            "documents.vectors",
        ] {
            assert_eq!(cell(id), (Read, Install, Install, AskEach), "{id}");
        }
        for id in ["documents.text", "calendar.read"] {
            assert_eq!(
                cell(id),
                (Read, Install, InstallThenFirstUse, AskEach),
                "{id}"
            );
        }
        assert_eq!(cell("calendar.details"), (Read, Install, FirstUse, AskEach));
        assert_eq!(cell("tasks.write"), (Write, Install, Install, AskEach));
        assert_eq!(cell("milestones.write"), (Write, Install, Install, AskEach));
        assert_eq!(cell("documents.write"), (Write, Install, FirstUse, AskEach));
        assert_eq!(cell("ai"), (Money, FirstUse, FirstUse, FirstUse));
        assert_eq!(cell("internet"), (Egress, Install, Install, AskEach));
        assert_eq!(cell("links"), (Egress, Install, Install, AskEach));
        assert_eq!(policy("nope"), None);
        // Every unsigned cell asks, but for what is only the mod's own.
        for p in POLICIES {
            match p.class {
                Local => {}
                Money => assert_eq!(p.unsigned, FirstUse, "{}", p.id),
                _ => assert_eq!(p.unsigned, AskEach, "{}", p.id),
            }
        }
    }

    #[test]
    fn only_an_atlas_signature_makes_a_tier() {
        assert_eq!(PolicyTier::of(true, "first"), PolicyTier::First);
        assert_eq!(PolicyTier::of(true, "second"), PolicyTier::Second);
        for (signed, tier) in [
            (false, "first"),
            (false, "second"),
            (false, "third"),
            (true, "third"),
            (true, "First"),
            (true, ""),
        ] {
            assert_eq!(
                PolicyTier::of(signed, tier),
                PolicyTier::Unsigned,
                "{signed} {tier}"
            );
        }
    }

    /// D3 across the unsigned set: read refuses egress and the reverse, whichever mod holds which;
    /// a held grant of the same class, or of a class D3 doesn't name, never does; signed tiers are
    /// never judged by it, nor are an unsigned mod's holdings judged for a signed caller.
    #[test]
    fn unsigned_mods_may_read_or_reach_out_never_both() {
        use PolicyTier::{First, Second, Unsigned};
        let none: [(PolicyTier, Class); 0] = [];
        for class in [Local, Read, Write, Money, Egress] {
            assert!(!excluded(Unsigned, class, none), "{class:?} alone");
        }
        assert!(excluded(Unsigned, Read, [(Unsigned, Egress)]));
        assert!(excluded(Unsigned, Egress, [(Unsigned, Read)]));
        assert!(excluded(
            Unsigned,
            Read,
            [(Unsigned, Local), (Unsigned, Egress)]
        ));
        assert!(!excluded(
            Unsigned,
            Read,
            [(Unsigned, Read), (Unsigned, Write)]
        ));
        assert!(!excluded(
            Unsigned,
            Egress,
            [(Unsigned, Egress), (Unsigned, Money)]
        ));
        // Money and writes are outside D3: `ai` is PM's own route.
        for class in [Local, Write, Money] {
            assert!(!excluded(
                Unsigned,
                class,
                [(Unsigned, Read), (Unsigned, Egress)]
            ));
        }
        // Signed mods' holdings never count, and signed callers are never judged.
        assert!(!excluded(
            Unsigned,
            Read,
            [(First, Egress), (Second, Egress)]
        ));
        for tier in [First, Second] {
            assert!(!excluded(tier, Read, [(Unsigned, Egress), (tier, Egress)]));
            assert!(!excluded(tier, Egress, [(Unsigned, Read), (tier, Read)]));
        }
        // A set already holding both (a hand-edited store, a restore) refuses both sides.
        let both = [(Unsigned, Read), (Unsigned, Egress)];
        assert!(excluded(Unsigned, Read, both) && excluded(Unsigned, Egress, both));
        // Only D3's classes need the holdings read.
        assert!(exclusion_applies(Unsigned, Read) && exclusion_applies(Unsigned, Egress));
        for class in [Local, Write, Money] {
            assert!(!exclusion_applies(Unsigned, class));
        }
        for tier in [First, Second] {
            assert!(!exclusion_applies(tier, Read) && !exclusion_applies(tier, Egress));
        }
    }

    #[test]
    fn an_unsealed_engine_takes_only_unsigned_reads() {
        use PolicyTier::{First, Second, Unsigned};
        for tier in [First, Second, Unsigned] {
            for class in [Local, Read, Write, Money, Egress] {
                assert!(allowed_by_engine(tier, class, true), "{tier:?} {class:?}");
                let unsealed = allowed_by_engine(tier, class, false);
                assert_eq!(
                    unsealed,
                    !(tier == Unsigned && class == Read),
                    "{tier:?} {class:?}"
                );
            }
        }
    }
}
