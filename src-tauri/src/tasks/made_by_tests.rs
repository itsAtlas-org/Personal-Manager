// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! "Made by X" on the lists the views read (K5): a task or milestone with a ledger row carries its
//! maker, one without carries none, a deleted thing's row goes with it, a planted name is cut down, and
//! the join reaches the ledger by its unique index, so it cannot make a long list scan the ledger.

use rusqlite::{params, Connection};

use super::*;
use crate::made_by::MadeBy;

const TODAY: &str = "2026-11-03";
const INSTALL: &str = "0123456789abcdef";

fn store() -> (tempfile::TempDir, Connection) {
    const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
    (dir, conn)
}

fn fields(title: &str) -> TaskFields {
    TaskFields {
        title: title.into(),
        why: None,
        project: None,
        milestone_id: None,
        event_uid: None,
        do_start: None,
        do_end: None,
        deadline: None,
        deadline_kind: "hard".into(),
        deadline_from: FROM_DATE.into(),
        lane: "foreground".into(),
        stack_id: None,
        parked: false,
    }
}

/// Write a ledger row for a task or a milestone, as the gift path does beside the thing itself.
fn gift(
    conn: &Connection,
    name: &str,
    tier: &str,
    signed: bool,
    task: Option<i64>,
    ms: Option<i64>,
) {
    conn.execute(
        "INSERT INTO mod_ledger(mod_id, mod_name, tier, author, source_repo, signed, install_id, \
                                task_id, milestone_id) \
         VALUES ('acme.tool', ?1, ?2, 'Acme', 'acme/tool', ?3, ?4, ?5, ?6)",
        params![name, tier, signed, INSTALL, task, ms],
    )
    .unwrap();
}

fn maker(name: &str, tier: &str, signed: bool) -> Option<MadeBy> {
    Some(MadeBy {
        name: name.into(),
        tier: tier.into(),
        signed,
    })
}

fn by_title(conn: &Connection) -> Vec<(String, Option<MadeBy>)> {
    list(conn, None, TODAY)
        .unwrap()
        .into_iter()
        .map(|t| (t.title, t.made_by))
        .collect()
}

#[test]
fn a_task_a_mod_made_says_who_and_the_users_own_say_nothing() {
    let (_dir, conn) = store();
    let mine = add(&conn, &fields("mine"), "user").unwrap();
    let gifted = add(&conn, &fields("gifted"), "user").unwrap();
    let unsigned = add(&conn, &fields("from a test"), "user").unwrap();
    gift(&conn, "Pinboard", "first", true, Some(gifted), None);
    gift(&conn, "Atlas", "third", false, Some(unsigned), None);
    assert_eq!(
        by_title(&conn),
        [
            ("mine".to_string(), None),
            ("gifted".to_string(), maker("Pinboard", "first", true)),
            // Named "Atlas" and claiming nothing it is not: the ledger says unsigned, third party.
            ("from a test".to_string(), maker("Atlas", "third", false)),
        ]
    );
    let _ = mine;
}

#[test]
fn a_milestone_a_mod_made_says_who_and_the_users_own_say_nothing() {
    let (_dir, conn) = store();
    let mine = crate::milestones::add(&conn, "P", "mine", Some("2026-12-01".into()), None).unwrap();
    let gifted =
        crate::milestones::add(&conn, "P", "gifted", Some("2026-12-02".into()), None).unwrap();
    gift(&conn, "Pinboard", "first", true, None, Some(gifted));
    for list in [
        crate::milestones::list_for_project(&conn, "P", TODAY).unwrap(),
        crate::milestones::list_all(&conn, TODAY).unwrap(),
    ] {
        let made: Vec<_> = list.iter().map(|m| (m.id, m.made_by.clone())).collect();
        assert_eq!(
            made,
            [(mine, None), (gifted, maker("Pinboard", "first", true))]
        );
    }
}

#[test]
fn a_ledger_row_for_a_deleted_task_leaves_no_label_on_anything() {
    let (_dir, conn) = store();
    let gone = add(&conn, &fields("gone"), "user").unwrap();
    gift(&conn, "Pinboard", "first", true, Some(gone), None);
    remove(&conn, gone).unwrap();
    // The row went with the task (the FK cascades), and a later task is a different id.
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM mod_ledger", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0);
    let later = add(&conn, &fields("later"), "user").unwrap();
    assert_ne!(
        later, gone,
        "ids are never reused, so no label can land on a newer task"
    );
    assert_eq!(by_title(&conn), [("later".to_string(), None)]);
}

#[test]
fn a_milestone_a_mod_made_keeps_its_label_after_the_mod_is_gone_but_not_after_the_milestone() {
    let (_dir, conn) = store();
    let m = crate::milestones::add(&conn, "P", "gifted", None, None).unwrap();
    gift(&conn, "Pinboard", "first", true, None, Some(m));
    // The mod's own row is not in the table at all: the ledger outlives it.
    assert_eq!(
        crate::milestones::list_all(&conn, TODAY).unwrap()[0].made_by,
        maker("Pinboard", "first", true)
    );
    crate::milestones::remove(&conn, m).unwrap();
    assert!(crate::milestones::list_all(&conn, TODAY)
        .unwrap()
        .is_empty());
}

#[test]
fn a_planted_name_is_cut_down_before_it_leaves_core() {
    let (_dir, conn) = store();
    let t = add(&conn, &fields("t"), "user").unwrap();
    let hostile = format!("{}\u{202e}\u{0007}{}", "x".repeat(200), "y".repeat(50));
    gift(&conn, &hostile, "third", false, Some(t), None);
    let made = list(&conn, None, TODAY).unwrap().remove(0).made_by.unwrap();
    assert!(made.name.chars().count() <= crate::made_by::NAME_MAX_CHARS);
    assert!(!made.name.contains('\u{202e}') && !made.name.contains('\u{0007}'));
    assert_eq!(made.tier, "third");
}

#[test]
fn the_list_has_one_row_per_task_whatever_the_ledger_holds() {
    let (_dir, conn) = store();
    let a = add(&conn, &fields("a"), "user").unwrap();
    let b = add(&conn, &fields("b"), "user").unwrap();
    gift(&conn, "One", "first", true, Some(a), None);
    gift(&conn, "Two", "second", true, Some(b), None);
    // A milestone's row for the same numeric id must not label a task.
    let m = crate::milestones::add(&conn, "P", "m", None, None).unwrap();
    gift(&conn, "Three", "third", false, None, Some(m));
    assert_eq!(list(&conn, None, TODAY).unwrap().len(), 2);
    assert_eq!(by_title(&conn)[0].1, maker("One", "first", true));
    assert_eq!(by_title(&conn)[1].1, maker("Two", "second", true));
}

/// The rows a query plan names, one line each.
fn plan(conn: &Connection, sql: &str, bind: &[&dyn rusqlite::ToSql]) -> Vec<String> {
    let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
    let rows: Vec<String> = stmt
        .query_map(bind, |r| r.get::<_, String>(3))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    rows
}

/// The made-by join, the ledger table `l`, must be reached by a SEARCH on the named unique index and
/// never by a SCAN (or a throwaway automatic index), so a list cannot walk the ledger once per row.
/// Pinned as a plan, not a duration: a wall-clock ratio is noise on a shared runner, and the plan is
/// the property itself.
fn assert_ledger_is_searched_by(plan: &[String], index: &str, column: &str) {
    let ledger: Vec<&String> = plan
        .iter()
        .filter(|d| d.starts_with("SEARCH l ") || d.starts_with("SCAN l"))
        .collect();
    assert_eq!(ledger.len(), 1, "the ledger is read once; plan: {plan:#?}");
    let want = format!("SEARCH l USING INDEX {index} ({column}=?)");
    // SQLite's newer versions add " LEFT-JOIN" after the search; the search itself is the claim.
    assert!(ledger[0].starts_with(&want), "plan: {plan:#?}");
    assert!(
        !plan.iter().any(|d| d.starts_with("SCAN mod_ledger")),
        "plan: {plan:#?}"
    );
}

#[test]
fn the_task_lists_made_by_join_searches_the_ledgers_unique_index_and_every_task_is_listed() {
    let (_dir, conn) = store();
    {
        let tx = conn.unchecked_transaction().unwrap();
        for i in 0..200 {
            tx.execute(
                "INSERT INTO tasks(title, ordinal, deadline_kind, deadline_from, lane, source) \
                 VALUES (?1, ?2, 'hard', 'date', 'foreground', 'user')",
                params![format!("task {i}"), i],
            )
            .unwrap();
        }
        // A tenth of them made by mods.
        tx.execute(
            "INSERT INTO mod_ledger(mod_id, mod_name, tier, author, source_repo, signed, install_id, task_id) \
             SELECT 'acme.tool', 'Acme', 'second', 'Acme', 'a/b', 1, ?1, id FROM tasks WHERE id % 10 = 0",
            params![INSTALL],
        )
        .unwrap();
        tx.commit().unwrap();
    }
    // The list's own query, for the whole list and for one project.
    assert_ledger_is_searched_by(
        &plan(&conn, LOAD_SQL, params![Option::<String>::None]),
        "idx_mod_ledger_task",
        "task_id",
    );
    assert_ledger_is_searched_by(
        &plan(&conn, LOAD_SQL, params![Some("P")]),
        "idx_mod_ledger_task",
        "task_id",
    );
    // And the real function still reads them all, a tenth labelled.
    let tasks = list(&conn, None, TODAY).unwrap();
    assert_eq!(tasks.len(), 200);
    assert_eq!(tasks.iter().filter(|t| t.made_by.is_some()).count(), 20);
}

#[test]
fn the_milestone_lists_made_by_join_searches_the_ledgers_unique_index() {
    let (_dir, conn) = store();
    for (sql, bind) in [
        (
            crate::milestones::LOAD_ALL_SQL,
            params![] as &[&dyn rusqlite::ToSql],
        ),
        (crate::milestones::LOAD_ONE_SQL, params!["P"]),
    ] {
        assert_ledger_is_searched_by(
            &plan(&conn, sql, bind),
            "idx_mod_ledger_milestone",
            "milestone_id",
        );
    }
}
