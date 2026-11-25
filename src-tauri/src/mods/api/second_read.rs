// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The second-READ rule (Mods spec §1.5: "a write can never be used as a read"; PR C, K1).
//!
//! A call that writes to a target it NAMES (a task, a milestone, a project) can answer "ok" or "not
//! found", and so tell its caller whether an id exists. A mod holding only the write permission must
//! not learn that, so such a call also needs the matching READ permissions, checked inside the handler
//! with [`grants::check`] BEFORE any lookup. A mod without them is refused `NotGranted`, naming the
//! read permission it lacks: the check runs before anything is read, so it says nothing about whether
//! the id exists (no probe), and it tells the mod's author which permission to declare. (The spec
//! says the same error as "not found"; this deliberately differs, recorded as D-14.)
//!
//! **One table, one helper, one test.** [`WRITE_RULES`] lists, for each call of a write class that
//! names a target, the reads it needs and arguments that pass its parsing and name nothing. A handler
//! calls [`require`] after [`super::recheck`], and `require` finds its rule by the call's own table row,
//! so a handler cannot carry a copy of the list that drifts. The table test below walks the dispatch
//! table itself: every `Class::Write` call row must have a rule (and an unlisted one is refused by
//! `require` as PM's failure, fail closed), and every row with a rule is called with only its write
//! permission, then with each of its reads missing in turn, and must answer `NotGranted` naming that
//! read: never `NotFound`. So a write row K2 adds is covered the moment it is added.

use rusqlite::Connection;

use super::grants::{self, ModFacts};
#[cfg(test)]
use super::{recheck, Code, Reply};
use super::{Authorised, Host, ModError};
#[cfg(test)]
use serde_json::Value;

/// One write call's second reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteRule {
    /// The call's table name.
    pub call: &'static str,
    /// The read permissions it needs besides its own, in the order they are checked (the first is
    /// the one a mod with none of them is told about).
    pub reads: &'static [&'static str],
    /// The reads it needs only when the call NAMES the optional target they open (`tasks.add`'s
    /// `project`): checked by [`require_named`], after `reads`, by a handler that was given one. A
    /// call that names none needs only `reads`.
    pub if_named: &'static [&'static str],
    /// The call's arguments as JSON, which its handler parses and which name a target that does not
    /// exist (and every optional target a rule lists in `if_named`): the table test sends them, so a
    /// refusal that comes before the lookup is the only way to answer anything but `NotFound`.
    pub probe: &'static str,
}

const fn rule(
    call: &'static str,
    reads: &'static [&'static str],
    if_named: &'static [&'static str],
    probe: &'static str,
) -> WriteRule {
    WriteRule {
        call,
        reads,
        if_named,
        probe,
    }
}

/// The write calls that name nothing which already exists, so a second read would protect nothing:
/// `documents.saveNote` writes a note under a key in the caller's OWN namespace, and what it answers
/// (that it saved, and with `documents.meta` the new document's id) is the same whether the key was
/// new or not. The table test holds this list to real write rows without a rule.
#[cfg(test)]
pub const NAMES_NOTHING: &[&str] = &["documents.saveNote"];

/// The rules of every write call that names an existing target.
pub const WRITE_RULES: &[WriteRule] = &[
    // A write call of the dispatcher's own tests, so the table test has a row to run whatever the
    // live table holds; it is in no dispatch table of PM's.
    #[cfg(test)]
    rule(
        PROBE_CALL,
        &["tasks.read", "projects.read"],
        &[],
        r#"{"id": 1}"#,
    ),
    // A task's project is a name it was shown (`projects.read`), and a new task's own record is read
    // back with `tasks.read`.
    rule(
        "tasks.add",
        &["tasks.read"],
        &["projects.read"],
        r#"{"title": "x", "project": "Nowhere"}"#,
    ),
    rule(
        "tasks.setStatus",
        &["tasks.read"],
        &[],
        r#"{"id": 1, "status": "done"}"#,
    ),
    rule("tasks.reorder", &["tasks.read"], &[], r#"{"ids": [1]}"#),
    rule(
        "tasks.importChecklist",
        &["tasks.read"],
        &["projects.read"],
        r#"{"project": "Nowhere", "items": [{"title": "x", "done": false}]}"#,
    ),
    // The editor opens on a task by id; a new task's prefilled project is checked by its handler
    // (it is text for a field, not a lookup).
    rule("ui.openTaskEditor", &["tasks.read"], &[], r#"{"id": 1}"#),
    // A milestone belongs to a project, and milestones are read with `projects.read`.
    rule(
        "milestones.add",
        &["projects.read"],
        &[],
        r#"{"project": "Nowhere", "label": "x"}"#,
    ),
    rule(
        "milestones.update",
        &["projects.read"],
        &[],
        r#"{"id": 1, "label": "x"}"#,
    ),
    rule(
        "milestones.setStatus",
        &["projects.read"],
        &[],
        r#"{"id": 1, "status": "done"}"#,
    ),
    rule("milestones.delete", &["projects.read"], &[], r#"{"id": 1}"#),
];

/// The rule for the call named `call`.
pub fn rule_for(call: &str) -> Option<&'static WriteRule> {
    WRITE_RULES.iter().find(|r| r.call == call)
}

/// Check the second reads of `auth`'s call: `facts` is the mod as [`super::recheck`] just read it,
/// under the guard `conn` is. Call it before the handler looks anything up. A call with no rule is
/// PM's failure (fail closed), not a pass.
pub(super) fn require<H: Host>(
    host: &H,
    conn: &Connection,
    facts: &ModFacts,
    auth: &Authorised,
) -> Result<(), ModError> {
    let rule = rule_of(auth)?;
    for read in rule.reads {
        grants::check(conn, facts, read, host.engine_sealed())?;
    }
    Ok(())
}

/// The reads a call needs only because it names its optional target ([`WriteRule::if_named`]): the
/// handler calls this when its arguments carry one, right after [`require`].
pub(super) fn require_named<H: Host>(
    host: &H,
    conn: &Connection,
    facts: &ModFacts,
    auth: &Authorised,
) -> Result<(), ModError> {
    let rule = rule_of(auth)?;
    for read in rule.if_named {
        grants::check(conn, facts, read, host.engine_sealed())?;
    }
    Ok(())
}

fn rule_of(auth: &Authorised) -> Result<&'static WriteRule, ModError> {
    rule_for(auth.row.name).ok_or_else(|| {
        ModError::internal(format!(
            "{} names a target but has no second-read rule",
            auth.row.name
        ))
    })
}

/// The test-only call's name (it has a handler in `handlers.rs` under `cfg(test)`).
#[cfg(test)]
pub(super) const PROBE_CALL: &str = "test.secondReadProbe";

/// The test call's handler: parses, checks the second reads, THEN looks (and finds nothing).
#[cfg(test)]
pub(super) async fn probe<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct Args {
        id: i64,
    }
    super::handlers::parse::<Args>(args)?;
    let (host, auth) = (host.clone(), auth.clone());
    super::blocking(move || {
        let conn = host.store().conn().map_err(|_| ModError::locked())?;
        let facts = recheck(&host, &conn, &auth)?;
        require(&host, &conn, &facts, &auth)?;
        Err(ModError::new(Code::NotFound, "Nothing there."))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::api::permissions::{self, Class};
    use crate::mods::api::table::{self, Kind, Row};
    use crate::mods::api::test_support::{insert_mod, live_session, serve, ModRow, TestHost};
    use crate::mods::api::{dispatch_in, limits, Caller, Code, Reply};
    use crate::mods::ids::ModId;
    use crate::mods::outbox::testing::discarding;
    use crate::mods::MAIN_LABEL;
    use serde_json::Value;

    /// A dispatch table of one write call, the test's own.
    const PROBE_ROWS: &[Row] = &[Row {
        name: PROBE_CALL,
        kind: Kind::Call,
        permission: Some("tasks.write"),
        class: Class::Write,
        added: (1, 0),
        deprecated: None,
        removed: None,
        experimental: false,
        family: limits::Family::General,
    }];

    /// A mod declaring `declared` (a first-party one: every Install permission needs its granted
    /// row, written here), in a live session.
    fn mod_with(host: &TestHost, declared: &[&'static str]) -> String {
        let row = ModRow::signed("atlas.writer", "first", declared);
        host.sql(|c| {
            insert_mod(c, &row);
            for p in declared {
                c.execute(
                    "INSERT INTO mod_grants(mod_id, permission, decision) \
                     VALUES ('atlas.writer', ?1, 'granted')",
                    [p],
                )
                .unwrap();
            }
        });
        serve(host, "atlas.writer");
        live_session(host, "atlas.writer", discarding())
    }

    async fn call(host: &TestHost, rows: &'static [Row], session: &str, row: &Row) -> Reply {
        let rule = rule_for(row.name).expect("a write row has a rule");
        let caller = Caller::Frame {
            window: MAIN_LABEL.into(),
            session: session.into(),
        };
        dispatch_in(host, rows, caller, row.name, rule.probe).await
    }

    /// The table test: every write call row of `rows` has a rule (or says it names nothing), and is
    /// refused `NotGranted` naming the read it lacks (never `NotFound`) by a mod holding only its
    /// write permission, and by a mod holding everything but one read, for each read in turn (the
    /// reads a call needs only for its optional target included: the probe names it). With every read
    /// it gets as far as the lookup.
    async fn every_write_row_refuses_before_any_lookup(rows: &'static [Row]) -> usize {
        let mut covered = 0;
        for row in rows
            .iter()
            .filter(|r| r.kind == Kind::Call && r.class == Class::Write)
        {
            if NAMES_NOTHING.contains(&row.name) {
                assert!(rule_for(row.name).is_none(), "{} names nothing", row.name);
                continue;
            }
            let rule = rule_for(row.name)
                .unwrap_or_else(|| panic!("{} is a write call with no second-read rule", row.name));
            let write = row.permission.expect("a write row has a permission");
            serde_json::from_str::<Value>(rule.probe).expect("the probe is JSON");
            let needed: Vec<&'static str> = rule
                .reads
                .iter()
                .chain(rule.if_named.iter())
                .copied()
                .collect();
            for read in &needed {
                assert_eq!(
                    permissions::policy(read).unwrap().class,
                    Class::Read,
                    "{}: {read} isn't a read",
                    row.name
                );
            }
            // Only the write permission: refused, naming the first read.
            let host = TestHost::new();
            let s = mod_with(&host, &[write]);
            let e = match call(&host, rows, &s, row).await {
                Reply::Error { error } => error,
                other => panic!("{}: {other:?}", row.name),
            };
            assert_eq!(e.code, Code::NotGranted, "{}", row.name);
            assert_eq!(e.permission, Some(needed[0]), "{}", row.name);
            // Everything but one read, for each read.
            for missing in &needed {
                let mut held = vec![write];
                held.extend(needed.iter().filter(|r| *r != missing));
                let host = TestHost::new();
                let s = mod_with(&host, &held);
                let e = match call(&host, rows, &s, row).await {
                    Reply::Error { error } => error,
                    other => panic!("{} without {missing}: {other:?}", row.name),
                };
                assert_eq!(e.code, Code::NotGranted, "{} without {missing}", row.name);
                assert_eq!(
                    e.permission,
                    Some(*missing),
                    "{} without {missing}",
                    row.name
                );
            }
            // Every read: the check passes, and the handler's own lookup is what answers.
            let host = TestHost::new();
            let mut all = vec![write];
            all.extend(needed.iter().copied());
            let s = mod_with(&host, &all);
            let e = match call(&host, rows, &s, row).await {
                Reply::Error { error } => error,
                other => panic!("{} with every read: {other:?}", row.name),
            };
            assert_ne!(e.code, Code::NotGranted, "{} with every read", row.name);
            covered += 1;
        }
        covered
    }

    /// PM's own dispatch table: every write call it has is held to the rule.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn every_write_call_of_the_live_table_refuses_a_write_only_mod_before_any_lookup() {
        let covered = every_write_row_refuses_before_any_lookup(table::ROWS).await;
        let writes = table::ROWS
            .iter()
            .filter(|r| r.kind == Kind::Call && r.class == Class::Write)
            .count();
        assert_eq!(covered + NAMES_NOTHING.len(), writes);
        // A call that says it names nothing is a real write row.
        for name in NAMES_NOTHING {
            assert!(
                table::ROWS
                    .iter()
                    .any(|r| r.name == *name && r.class == Class::Write),
                "{name} is listed as naming nothing and is no write row"
            );
        }
        // A rule that names no row of the live table is a stale one (except the test's own).
        for rule in WRITE_RULES.iter().filter(|r| r.call != PROBE_CALL) {
            assert!(
                table::ROWS
                    .iter()
                    .any(|r| r.name == rule.call && r.class == Class::Write),
                "{} has a rule and no write row",
                rule.call
            );
        }
    }

    /// The same driver over a table with a real write call (the test's), so the test is proven to
    /// bite: it ran one row, through the real dispatcher.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_table_test_covers_a_write_row_through_the_real_dispatcher() {
        assert_eq!(
            every_write_row_refuses_before_any_lookup(PROBE_ROWS).await,
            1
        );
    }

    /// A write call with no rule is PM's failure, never a pass: fail closed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_write_call_with_no_rule_fails_closed() {
        const NO_RULE: Row = Row {
            name: "test.noRule",
            ..PROBE_ROWS[0]
        };
        let host = TestHost::new();
        // Holding every permission the probe rule names: only the missing rule can refuse.
        let s = mod_with(&host, &["tasks.write", "tasks.read", "projects.read"]);
        let facts = host
            .sql(|c| grants::read_mod(c, &ModId::parse("atlas.writer").unwrap()))
            .unwrap()
            .unwrap();
        let auth = Authorised {
            ticket: host.runtime().begin_call(&s).unwrap(),
            row: &NO_RULE,
        };
        let e = host.sql(|c| require(&host, c, &facts, &auth)).unwrap_err();
        assert_eq!(e.code, Code::Internal);
    }

    /// A rule's shape: each names a call, reads that are read-class permissions, and no duplicates.
    #[test]
    fn every_rule_is_well_formed() {
        let mut seen = std::collections::BTreeSet::new();
        for rule in WRITE_RULES {
            assert!(seen.insert(rule.call), "{} has two rules", rule.call);
            assert!(!rule.reads.is_empty(), "{}", rule.call);
            let both: Vec<_> = rule.reads.iter().chain(rule.if_named).collect();
            let distinct: std::collections::BTreeSet<_> = both.iter().collect();
            assert_eq!(distinct.len(), both.len(), "{}", rule.call);
            for read in both {
                assert_eq!(permissions::policy(read).unwrap().class, Class::Read);
            }
            let probe: Value = serde_json::from_str(rule.probe).unwrap();
            assert!(probe.is_object() || probe.is_null(), "{}", rule.call);
        }
        assert!(rule_for("nope").is_none());
    }
}
