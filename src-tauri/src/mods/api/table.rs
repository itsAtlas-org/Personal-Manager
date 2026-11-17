// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The dispatch table (Mods spec §1.5): one row for every method and every event the API has ever
//! had, with the permission it needs, its class, and when it was added, deprecated or removed. It
//! drives the dispatcher (an unknown or removed name is refused here, and the permission checked is
//! the row's) and the Developer tab (slice 6), and a test fails on a handler or event without a
//! row, or a row without a handler.
//!
//! A row is never deleted: a removed method keeps its row with `removed` set, so a mod built for an
//! older API gets [`super::Code::RemovedMethod`] rather than "unknown".
//!
//! `experimental` rows (D13) are outside the API freeze and may change in a minor: the four calls no
//! pilot uses yet (`ai.complete`, `net.fetch`, `secrets.set`, `notify`, PR B) land marked so.

use super::limits::Family;
use super::permissions::Class;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A frame calls it and gets an answer.
    Call,
    /// PM posts it to a frame that holds the permission (or to any session, without one).
    Event,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub name: &'static str,
    pub kind: Kind,
    /// The permission it needs; `None` for what any session has (a session's own lifecycle).
    pub permission: Option<&'static str>,
    /// The permission's class (`Local` for a session-only row). Held equal to the policy table's by
    /// a test, so the Developer tab can show it without a second lookup.
    pub class: Class,
    /// The API version (major, minor) it arrived in.
    pub added: (u16, u16),
    pub deprecated: Option<(u16, u16)>,
    pub removed: Option<(u16, u16)>,
    pub experimental: bool,
    /// The bucket and concurrency cap, beyond the generic ones, the dispatcher admits it under
    /// (`limits::Family`). `General` for everything that waits only on PM's own database.
    pub family: Family,
}

const fn call(name: &'static str, permission: &'static str, class: Class) -> Row {
    Row {
        name,
        kind: Kind::Call,
        permission: Some(permission),
        class,
        added: (1, 0),
        deprecated: None,
        removed: None,
        experimental: false,
        family: Family::General,
    }
}

/// One of PR B's four experimental calls (D13: outside the API freeze, `ai.complete`, `net.fetch`,
/// `secrets.set`, `notify`): a call row marked so, which the Developer tab and ERROR_CODES show as
/// experimental. It needs a `family` because each of them waits on something outside PM (or, for
/// `secrets.set` and `notify`, on the person), and a handler slice must say which limits it lives
/// under rather than inherit "none" by default.
// Used by the rows each handler slice adds (X5, X6); the table has none until then.
#[cfg_attr(not(test), allow(dead_code))]
const fn experimental_call(
    name: &'static str,
    permission: &'static str,
    class: Class,
    family: Family,
) -> Row {
    Row {
        experimental: true,
        family,
        ..call(name, permission, class)
    }
}

/// An event a frame needs `permission` to be told (or any session, with `None`: PM's word about the
/// session itself, which carries no data).
const fn event(name: &'static str, permission: Option<&'static str>, class: Class) -> Row {
    Row {
        name,
        kind: Kind::Event,
        permission,
        class,
        added: (1, 0),
        deprecated: None,
        removed: None,
        experimental: false,
        family: Family::General,
    }
}

/// Every row. PR B adds the experimental calls; PR C the Pinboard's calls and events.
pub const ROWS: &[Row] = &[
    // The spec's table gives `theme.get` to `storage` and `ui.tab` alike, and its events table
    // gives `theme.changed` to `storage`; the call takes the event's permission, so a mod that may
    // be told the theme changed may also ask for it.
    call("theme.get", "storage", Class::Local),
    call("storage.get", "storage", Class::Local),
    call("storage.set", "storage", Class::Local),
    // Per-document storage is `documents.meta`'s (spec §1.5's table), not `storage`'s: it is keyed to
    // the user's documents, and a mod that may not know they exist has no use for rows on them.
    call("storage.doc.get", "documents.meta", Class::Read),
    call("storage.doc.set", "documents.meta", Class::Read),
    call("storage.doc.list", "documents.meta", Class::Read),
    call("storage.doc.delete", "documents.meta", Class::Read),
    call("documents.list", "documents.meta", Class::Read),
    call("ui.openDocument", "documents.meta", Class::Read),
    // A vector can be inverted toward its text, so these have a permission of their own.
    call(
        "embeddings.documentVectors",
        "documents.vectors",
        Class::Read,
    ),
    call("embeddings.signature", "documents.vectors", Class::Read),
    // Events. Each carries the same narrow shape as the call it matches, or ids only (events.rs).
    // A document that landed, as `documents.list`'s DTO with every field.
    event("documents.added", Some("documents.meta"), Class::Read),
    // Ids only, after the delete committed.
    event("documents.deleted", Some("documents.meta"), Class::Read),
    // The page's snapshot of PM's theme, the shape `theme.get` answers with.
    event("theme.changed", Some("storage"), Class::Local),
    // Lifecycle: no permission, no data. The mod's answers to "may I?" may now differ (re-ask)...
    event("grants.changed", None, Class::Local),
    // ...it was paused, and its session ended. These two are PM's goodbye (outbox.rs).
    event("mod.paused", None, Class::Local),
    event("session.closed", None, Class::Local),
];

/// The live event row named `name` in `rows` at API `version`.
pub fn event_row(rows: &'static [Row], name: &str, version: (u16, u16)) -> Option<&'static Row> {
    rows.iter().find(|r| {
        r.kind == Kind::Event
            && r.name == name
            && r.added <= version
            && !r.removed.is_some_and(|v| v <= version)
    })
}

/// What a name resolves to for a caller of this PM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lookup {
    Found(&'static Row),
    Removed(&'static Row),
    Unknown,
}

/// The call named `name` in `rows` at API `version`: unknown when there is no such call row (or it
/// arrives in a later API, which no row in this PM's own table can), removed once `version` reaches
/// its `removed`.
pub fn lookup_call(rows: &'static [Row], name: &str, version: (u16, u16)) -> Lookup {
    match rows.iter().find(|r| r.kind == Kind::Call && r.name == name) {
        Some(row) if row.added > version => Lookup::Unknown,
        Some(row) if row.removed.is_some_and(|v| v <= version) => Lookup::Removed(row),
        Some(row) => Lookup::Found(row),
        None => Lookup::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::api::handlers::Method;
    use crate::mods::api::permissions;
    use crate::mods::policy::MOD_API_VERSION;
    use std::collections::BTreeSet;

    /// [`lookup_call`] in this PM's table, at its API.
    fn lookup(name: &str) -> Lookup {
        lookup_call(ROWS, name, MOD_API_VERSION)
    }

    /// The class a row should carry: its permission's, or `Local` without one.
    fn class_of(permission: Option<&str>) -> Option<Class> {
        match permission {
            None => Some(Class::Local),
            Some(p) => permissions::policy(p).map(|p| p.class),
        }
    }

    #[test]
    fn every_row_is_well_formed() {
        let mut names = BTreeSet::new();
        for row in ROWS {
            assert!(names.insert(row.name), "{} has two rows", row.name);
            let shaped = !row.name.is_empty()
                && row.name.len() <= 128
                && row.name.split('.').all(|part| {
                    part.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
                        && part.bytes().all(|b| b.is_ascii_alphanumeric())
                });
            assert!(shaped, "{} isn't a dotted camelCase name", row.name);
            assert_eq!(
                class_of(row.permission),
                Some(row.class),
                "{}: its class isn't its permission's",
                row.name
            );
            assert!(
                row.added <= MOD_API_VERSION,
                "{} arrives after this PM",
                row.name
            );
            // A family's limits are for the calls that wait on the outside, which are the
            // experimental ones; an event is never admitted at all.
            if row.family != Family::General {
                assert!(
                    row.kind == Kind::Call && row.experimental,
                    "{}: a family is for an experimental call",
                    row.name
                );
            }
            if let Some(d) = row.deprecated {
                assert!(d >= row.added, "{}", row.name);
            }
            if let Some(r) = row.removed {
                assert!(r > row.added, "{}", row.name);
                assert!(
                    row.deprecated.is_some_and(|d| d < r),
                    "{}: removed unannounced",
                    row.name
                );
            }
        }
    }

    /// The coverage gate (spec §1.5): every handler has a call row, every call row a handler (but a
    /// removed one, which is answered by its row), and every event row a registered event and the
    /// reverse.
    #[test]
    fn every_handler_and_event_has_a_row_and_every_row_its_handler() {
        for method in Method::ALL {
            assert!(
                matches!(lookup(method.name()), Lookup::Found(_)),
                "the handler {} has no live call row",
                method.name()
            );
        }
        for row in ROWS {
            match row.kind {
                Kind::Call if row.removed.is_none() => assert!(
                    Method::from_name(row.name).is_some(),
                    "the call row {} has no handler",
                    row.name
                ),
                Kind::Call => assert!(Method::from_name(row.name).is_none(), "{}", row.name),
                Kind::Event => assert!(
                    crate::mods::api::EVENTS.contains(&row.name),
                    "the event row {} isn't a registered event",
                    row.name
                ),
            }
        }
        for event in crate::mods::api::EVENTS {
            assert!(
                ROWS.iter()
                    .any(|r| r.kind == Kind::Event && r.name == *event),
                "the event {event} has no row"
            );
        }
    }

    /// A table made for the test: one live call, one removed in 2.0, one added in 1.2, one event.
    const TEST_ROWS: &[Row] = &[
        call("a.live", "storage", Class::Local),
        Row {
            deprecated: Some((1, 3)),
            removed: Some((2, 0)),
            ..call("a.gone", "storage", Class::Local)
        },
        Row {
            added: (1, 2),
            ..call("a.later", "storage", Class::Local)
        },
        Row {
            kind: Kind::Event,
            ..call("a.event", "storage", Class::Local)
        },
    ];

    /// The helper PR B's rows are made with: a call row, in API 1.0, marked experimental, under the
    /// family it is given; and it differs from a plain `call` in exactly those two fields.
    #[test]
    fn an_experimental_call_is_a_call_row_marked_so_under_its_family() {
        let row = experimental_call("a.b", "ai", Class::Money, Family::Ai);
        assert_eq!(
            row,
            Row {
                experimental: true,
                family: Family::Ai,
                ..call("a.b", "ai", Class::Money)
            }
        );
        assert!(row.experimental && row.kind == Kind::Call);
        assert_eq!((row.permission, row.class), (Some("ai"), Class::Money));
        assert_eq!(
            (row.added, row.deprecated, row.removed),
            ((1, 0), None, None)
        );
        // A plain call and an event are neither experimental nor limited beyond the generic.
        for plain in [
            call("a.c", "storage", Class::Local),
            event("a.d", None, Class::Local),
        ] {
            assert!(!plain.experimental && plain.family == Family::General);
        }
        // It is found like any call row, and `kit.rs` lists it by `experimental`.
        const NET_ROWS: &[Row] = &[experimental_call(
            "net.x",
            "internet",
            Class::Egress,
            Family::Net,
        )];
        assert!(matches!(
            lookup_call(NET_ROWS, "net.x", MOD_API_VERSION),
            Lookup::Found(r) if r.experimental && r.family == Family::Net
        ));
    }

    /// The only experimental rows the live table may ever carry are PR B's four, each under the
    /// family its limits are written for; every other row is general.
    #[test]
    fn the_live_tables_experimental_rows_are_held_to_their_families() {
        for row in ROWS {
            if row.experimental {
                let want = match row.name {
                    "ai.complete" => Family::Ai,
                    "net.fetch" => Family::Net,
                    "secrets.set" | "notify" => Family::General,
                    other => panic!("{other} is experimental but isn't one of PR B's four"),
                };
                assert_eq!(row.family, want, "{}", row.name);
                assert_eq!(row.kind, Kind::Call, "{}", row.name);
            } else {
                assert_eq!(row.family, Family::General, "{}", row.name);
            }
        }
    }

    #[test]
    fn a_name_is_found_removed_or_unknown_by_the_callers_api() {
        assert!(matches!(lookup("storage.get"), Lookup::Found(r) if r.name == "storage.get"));
        for name in [
            "",
            "storage",
            "storage.get.x",
            "Storage.get",
            "nope.never",
            "storage.get ",
        ] {
            assert_eq!(lookup(name), Lookup::Unknown, "{name:?}");
        }
        let at = |name, version| lookup_call(TEST_ROWS, name, version);
        assert!(matches!(at("a.live", (1, 0)), Lookup::Found(_)));
        assert!(
            matches!(at("a.gone", (1, 9)), Lookup::Found(_)),
            "deprecated, still served"
        );
        assert!(matches!(at("a.gone", (2, 0)), Lookup::Removed(r) if r.name == "a.gone"));
        assert!(matches!(at("a.gone", (3, 1)), Lookup::Removed(_)));
        assert_eq!(at("a.later", (1, 1)), Lookup::Unknown);
        assert!(matches!(at("a.later", (1, 2)), Lookup::Found(_)));
        // An event's name is no call.
        assert_eq!(at("a.event", (1, 0)), Lookup::Unknown);
    }
}
