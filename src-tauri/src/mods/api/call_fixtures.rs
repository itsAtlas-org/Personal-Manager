// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What PR C's K2 tests share: a mod of a given identity and tier in a live session over the API's
//! test host, the calls made through the real dispatcher, and the answers read back.

use rusqlite::{params, Connection};
use serde_json::{json, Value};

use super::test_support::{insert_mod, live_session, serve, ModRow, TestHost};
pub use super::Host;
use super::{dispatch, Caller, Code, ModError, PageAction, Reply};
use crate::mods::outbox::testing::discarding;
use crate::mods::MAIN_LABEL;

/// A host and the session of the mod a test made.
pub type World = (TestHost, String);

/// A function that makes a world with no arguments (a mod of a fixed identity).
pub type MakeFixed = fn() -> World;

/// A function that makes a world for a mod holding the given permissions.
pub type MakeWorld = fn(&[&'static str]) -> World;

/// A mod that is Atlas's own: signed, tier First.
pub fn first(id: &'static str, perms: &[&'static str]) -> (TestHost, String) {
    world(id, true, "first", perms, &[])
}

/// A signed second-party mod.
pub fn second(id: &'static str, perms: &[&'static str]) -> (TestHost, String) {
    world(id, true, "second", perms, &[])
}

/// An unsigned folder mod. `claims` is the tier its own row says (a developer's folder may claim any).
pub fn unsigned(
    id: &'static str,
    claims: &'static str,
    perms: &[&'static str],
) -> (TestHost, String) {
    world(id, false, claims, perms, &[])
}

/// A mod of this identity in a live session, holding `perms` (declared and granted: every permission
/// a policy would ask for is answered yes), whose manifest declares `contributions`.
pub fn world(
    id: &'static str,
    signed: bool,
    tier: &'static str,
    perms: &[&'static str],
    contributions: &[&str],
) -> (TestHost, String) {
    let host = TestHost::new();
    let session = add_mod(&host, id, signed, tier, perms, contributions);
    (host, session)
}

/// Another mod beside the first, in its own live session over the same store.
pub fn add_mod(
    host: &TestHost,
    id: &'static str,
    signed: bool,
    tier: &'static str,
    perms: &[&'static str],
    contributions: &[&str],
) -> String {
    let row = ModRow {
        signed,
        ..ModRow::signed(id, tier, perms)
    };
    host.sql(|c| {
        insert_mod(c, &row);
        if !contributions.is_empty() {
            let mut manifest: Value = serde_json::from_str(
                &c.query_row("SELECT manifest FROM mods WHERE id = ?1", [id], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap(),
            )
            .unwrap();
            manifest["contributions"] = Value::Object(
                contributions
                    .iter()
                    .map(|name| ((*name).to_owned(), json!({"label": "x"})))
                    .collect(),
            );
            c.execute(
                "UPDATE mods SET manifest = ?2 WHERE id = ?1",
                params![id, manifest.to_string()],
            )
            .unwrap();
        }
        for p in perms {
            c.execute(
                "INSERT INTO mod_grants(mod_id, permission, decision) VALUES (?1, ?2, 'granted')",
                params![id, p],
            )
            .unwrap();
        }
    });
    serve(host, id);
    live_session(host, id, discarding())
}

pub fn frame(session: &str) -> Caller {
    Caller::Frame {
        window: MAIN_LABEL.into(),
        session: session.into(),
    }
}

/// One call through the real dispatcher.
pub async fn call(host: &TestHost, session: &str, method: &str, args: Value) -> Reply {
    dispatch(host, frame(session), method, &args.to_string()).await
}

/// The call's answer, which must be a value.
pub fn value(reply: Reply) -> Value {
    match reply {
        Reply::Value { value } => value,
        other => panic!("not a value: {other:?}"),
    }
}

/// The call's refusal.
pub fn error(reply: Reply) -> ModError {
    match reply {
        Reply::Error { error } => error,
        other => panic!("not a refusal: {other:?}"),
    }
}

pub fn code(reply: Reply) -> Code {
    error(reply).code
}

/// The page action a call authorised.
pub fn page(reply: Reply) -> (PageAction, Value) {
    match reply {
        Reply::Page { action, args } => (action, args),
        other => panic!("not a page action: {other:?}"),
    }
}

/// How many rows `sql` (a `SELECT count(*) ...`) finds.
pub fn count(host: &TestHost, sql: &str) -> i64 {
    host.sql(|c| c.query_row(sql, [], |r| r.get(0)).unwrap())
}

pub fn seed_project(conn: &Connection, name: &str) {
    conn.execute("INSERT INTO projects(name) VALUES (?1)", [name])
        .unwrap();
}

/// A task of the user's own (no maker), in `project`, with a status, at the end of the order.
pub fn seed_task(conn: &Connection, title: &str, project: Option<&str>) -> i64 {
    conn.execute(
        "INSERT INTO tasks(title, project_name, ordinal) \
         VALUES (?1, ?2, (SELECT COALESCE(MAX(ordinal) + 1, 0) FROM tasks))",
        params![title, project],
    )
    .unwrap();
    conn.last_insert_rowid()
}

/// A milestone of the user's own (no maker).
pub fn seed_milestone(conn: &Connection, project: &str, label: &str) -> i64 {
    conn.execute(
        "INSERT INTO projects(name) VALUES (?1) ON CONFLICT(name) DO NOTHING",
        [project],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO project_milestones(project_name, label, due_date, sort_order) \
         VALUES (?1, ?2, '2026-12-01', (SELECT COALESCE(MAX(sort_order) + 1, 0) \
                                        FROM project_milestones WHERE project_name = ?1))",
        params![project, label],
    )
    .unwrap();
    conn.last_insert_rowid()
}

/// The `mod_ledger` rows of `mod_id`: `(task_id, milestone_id)` of each.
pub fn ledger(host: &TestHost, mod_id: &str) -> Vec<(Option<i64>, Option<i64>)> {
    host.sql(|c| {
        let mut stmt = c
            .prepare("SELECT task_id, milestone_id FROM mod_ledger WHERE mod_id = ?1 ORDER BY id")
            .unwrap();
        let rows = stmt
            .query_map([mod_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        rows.map(Result::unwrap).collect()
    })
}
