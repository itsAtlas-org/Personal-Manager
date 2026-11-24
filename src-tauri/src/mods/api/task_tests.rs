// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The task calls through the real dispatcher over a real store (PR C, K2): `tasks.list`'s DTO under
//! every grant, its paging and its project filter; `tasks.add`'s gift (the task and its ledger row
//! together or not at all, the quota, the family's rate); `tasks.setStatus`, `tasks.reorder`,
//! `tasks.importChecklist`; and `ui.openTaskEditor`.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::call_fixtures::*;
use super::task_calls::{list_page, task_dto};
use super::test_support::TestHost;
use super::{limits, Code, PageAction, Point};
use crate::mods::api::projection::Projection;

const TASK_MOD: &str = "atlas.tasker";

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().unwrap().keys().cloned().collect()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| (*n).to_owned()).collect()
}

/// What every `tasks.list` task carries, whatever the caller holds.
const ALWAYS: &[&str] = &[
    "id",
    "title",
    "why",
    "status",
    "completed_at",
    "lane",
    "stack_id",
    "parked",
    "ordinal",
    "source",
    "created_at",
    "updated_at",
    "do_start",
    "do_end",
    "deadline",
    "deadline_kind",
    "deadline_from",
    "effective_deadline",
    "deadline_missing",
    "milestone_linked",
    "event_linked",
];
/// What `projects.read` adds.
const PROJECTS: &[&str] = &[
    "project_name",
    "project_archived",
    "milestone_id",
    "milestone_label",
];
/// What `calendar.read` adds.
const CALENDAR: &[&str] = &["event_uid", "event_title"];

/// A project "Alpha" with a milestone and a task linked to both that and an event, plus a task of no
/// project at all. Returns the ids `(linked, plain)`.
fn seed_linked(host: &TestHost) -> (i64, i64) {
    host.sql(|c| {
        let milestone = seed_milestone(c, "Alpha", "Launch");
        c.execute(
            "INSERT INTO calendar_events(id, calendar_id, summary, start, uid) \
             VALUES ('c:1', 'c', 'Sprint review', '2026-12-05T10:00:00Z', 'uid-1@example.com')",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO tasks(title, project_name, milestone_id, event_uid, deadline_from, \
                               deadline, ordinal) \
             VALUES ('Linked', 'Alpha', ?1, 'uid-1@example.com', 'event', '2026-12-05', 0)",
            [milestone],
        )
        .unwrap();
        let linked = c.last_insert_rowid();
        (linked, seed_task(c, "Plain", None))
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_task_list_has_exactly_the_fields_the_callers_grants_allow() {
    // The grid: DTO fields x grants. Each cell is a mod holding `tasks.read` and some of the two.
    for (projects, calendar) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut perms = vec!["tasks.read"];
        if projects {
            perms.push("projects.read");
        }
        if calendar {
            perms.push("calendar.read");
        }
        let (host, s) = first(TASK_MOD, &perms);
        let (linked, plain) = seed_linked(&host);
        let v = value(call(&host, &s, "tasks.list", json!({})).await);
        let tasks = v["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 2, "{perms:?}");
        let mut expected = set(ALWAYS);
        if projects {
            expected.extend(set(PROJECTS));
        }
        if calendar {
            expected.extend(set(CALENDAR));
        }
        for task in tasks {
            assert_eq!(keys(task), expected, "{perms:?}");
        }
        let (a, b) = (&tasks[0], &tasks[1]);
        assert_eq!(
            (a["id"].as_i64(), b["id"].as_i64()),
            (Some(linked), Some(plain))
        );
        // A dropped link is never taken for a removed one: the flags are always said.
        assert_eq!(
            (a["milestone_linked"].as_bool(), a["event_linked"].as_bool()),
            (Some(true), Some(true)),
            "{perms:?}"
        );
        assert_eq!(
            (b["milestone_linked"].as_bool(), b["event_linked"].as_bool()),
            (Some(false), Some(false)),
            "{perms:?}"
        );
        // A date alone is never withheld (it names no event), nor is the date the link resolved to.
        assert_eq!(a["deadline_from"], "event", "{perms:?}");
        assert_eq!(a["effective_deadline"], "2026-12-05", "{perms:?}");
        assert_eq!(a["deadline_missing"], false, "{perms:?}");
        if projects {
            assert_eq!(
                (a["project_name"].as_str(), a["milestone_label"].as_str()),
                (Some("Alpha"), Some("Launch"))
            );
            assert!(a["milestone_id"].is_i64());
            assert_eq!(b["project_name"], Value::Null);
        }
        if calendar {
            assert_eq!(
                (a["event_uid"].as_str(), a["event_title"].as_str()),
                (Some("uid-1@example.com"), Some("Sprint review"))
            );
        }
        // Nothing of the event or the project leaks by another name.
        let text = v.to_string();
        if !calendar {
            assert!(
                !text.contains("Sprint review") && !text.contains("uid-1"),
                "{perms:?}"
            );
        }
        if !projects {
            assert!(
                !text.contains("Alpha") && !text.contains("Launch"),
                "{perms:?}"
            );
        }
    }
}

/// The DTO is built from named fields, so a column added to a task can never leak by being added: this
/// fails until somebody decides whether a mod may know it.
#[test]
fn the_dto_names_every_field_of_a_core_task_and_nothing_else() {
    use crate::tasks::{self, Task};
    let (_dir, conn) = super::test_support::store();
    conn.execute("INSERT INTO tasks(title, ordinal) VALUES ('a', 0)", [])
        .unwrap();
    let task: Task = tasks::list(&conn, None, "2026-12-01").unwrap().remove(0);
    let core = keys(&serde_json::to_value(&task).unwrap());
    let dto = keys(&task_dto(&task, Projection::scoped(true, true)));
    let flags = set(&["milestone_linked", "event_linked"]);
    assert_eq!(
        core.difference(&dto).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "a core task field a mod isn't told about, and isn't named as withheld"
    );
    assert_eq!(
        dto.difference(&core).cloned().collect::<BTreeSet<_>>(),
        flags,
        "a DTO field that is no column of a task"
    );
    // And what a caller with neither grant gets is the grid above's always-set.
    let none = keys(&task_dto(&task, Projection::NONE));
    assert_eq!(none, set(ALWAYS));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_list_comes_in_pages_that_follow_the_global_order_and_stacks_only_once() {
    let (host, s) = first(TASK_MOD, &["tasks.read"]);
    host.sql(|c| {
        for i in 0..7 {
            seed_task(c, &format!("t{i}"), None);
        }
        c.execute(
            "INSERT INTO task_stacks(title, kind) VALUES ('Errands', 'sequential')",
            [],
        )
        .unwrap();
    });
    let mut seen = Vec::new();
    let mut after: Option<String> = None;
    let mut pages = 0;
    loop {
        let args = match &after {
            Some(a) => json!({"limit": 3, "after": a}),
            None => json!({"limit": 3}),
        };
        let v = value(call(&host, &s, "tasks.list", args).await);
        pages += 1;
        // The stacks arrive with the first page and are not repeated.
        assert_eq!(
            v["stacks"].as_array().unwrap().len(),
            usize::from(after.is_none())
        );
        seen.extend(
            v["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["title"].as_str().unwrap().to_owned()),
        );
        match v["next"].as_str() {
            Some(next) => after = Some(next.to_owned()),
            None => break,
        }
    }
    assert_eq!(
        (pages, seen),
        (3, (0..7).map(|i| format!("t{i}")).collect::<Vec<_>>())
    );
    // A stack is its fields and nothing more.
    let v = value(call(&host, &s, "tasks.list", json!({})).await);
    assert_eq!(
        keys(&v["stacks"][0]),
        set(&["id", "title", "kind", "note", "created_at", "updated_at"])
    );
    // A bad cursor or limit is a bad argument.
    for args in [
        json!({"after": "x"}),
        json!({"after": "1:x"}),
        json!({"limit": 0}),
        json!({"limit": 5001}),
        json!({"where": "1=1"}),
    ] {
        assert_eq!(
            code(call(&host, &s, "tasks.list", args.clone()).await),
            Code::BadArgs,
            "{args}"
        );
    }
}

#[test]
fn a_page_ends_early_under_its_byte_budget_and_always_advances() {
    use crate::tasks;
    let (_dir, conn) = super::test_support::store();
    for i in 0..5 {
        seed_task(&conn, &format!("task {i}"), None);
    }
    let all = tasks::list(&conn, None, "2026-12-01").unwrap();
    let one = task_dto(&all[0], Projection::NONE).to_string().len() + 1;
    let (page, next) = list_page(&all, None, 5, 2 * one + 1, Projection::NONE);
    assert_eq!(page.len(), 2);
    assert_eq!(
        next.as_deref(),
        Some(format!("{}:{}", all[1].ordinal, all[1].id).as_str())
    );
    // A budget smaller than one task still lets one through.
    let (page, next) = list_page(&all, None, 5, 1, Projection::NONE);
    assert_eq!((page.len(), next.is_some()), (1, true));
    // The last page has no cursor.
    let after = Some((all[3].ordinal, all[3].id));
    let (page, next) = list_page(&all, after, 5, usize::MAX, Projection::NONE);
    assert_eq!((page.len(), next), (1, None));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_project_filter_reads_one_projects_tasks_and_needs_projects_read() {
    let (host, s) = first(TASK_MOD, &["tasks.read", "projects.read"]);
    host.sql(|c| {
        seed_project(c, "Alpha");
        seed_project(c, "Beta");
        seed_task(c, "a1", Some("Alpha"));
        seed_task(c, "b1", Some("Beta"));
        seed_task(c, "a2", Some("Alpha"));
    });
    let titles = |v: Value| -> Vec<String> {
        v["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["title"].as_str().unwrap().to_owned())
            .collect()
    };
    let v = value(call(&host, &s, "tasks.list", json!({"project": "Alpha"})).await);
    assert_eq!(titles(v), ["a1", "a2"]);
    // A different spelling of the same project is that project.
    let v = value(call(&host, &s, "tasks.list", json!({"project": " alpha "})).await);
    assert_eq!(titles(v), ["a1", "a2"]);
    // A project nobody has is empty, never an error that says whether it exists.
    let v = value(call(&host, &s, "tasks.list", json!({"project": "Nope"})).await);
    assert!(titles(v).is_empty());
    // Without projects.read, naming one is refused: otherwise the filter tells which names have tasks.
    let (host, s) = first(TASK_MOD, &["tasks.read"]);
    host.sql(|c| {
        seed_project(c, "Alpha");
        seed_task(c, "a1", Some("Alpha"))
    });
    let e = error(call(&host, &s, "tasks.list", json!({"project": "Alpha"})).await);
    assert_eq!(
        (e.code, e.permission),
        (Code::NotGranted, Some("projects.read"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_added_task_is_the_mods_gift_and_its_ledger_row_exists_with_it() {
    let (host, s) = first(TASK_MOD, &["tasks.write", "tasks.read", "projects.read"]);
    host.sql(|c| seed_project(c, "Alpha"));
    let v = value(
        call(
            &host,
            &s,
            "tasks.add",
            json!({"title": "  Send the deck ", "why": "Because", "project": "alpha",
                   "deadline": "2026-12-03", "deadlineKind": "soft"}),
        )
        .await,
    );
    let id = v["id"].as_i64().unwrap();
    assert_eq!(v, json!({"id": id, "status": "todo"}));
    // The row is the user's task in PM's own spelling of the project, with the source the page's own
    // add gives, and a ledger row for it, written with it.
    let row: (
        String,
        Option<String>,
        Option<String>,
        String,
        String,
        String,
    ) = host.sql(|c| {
        c.query_row(
            "SELECT title, why, project_name, deadline_kind, deadline, source FROM tasks \
             WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .unwrap()
    });
    assert_eq!(
        row,
        (
            "Send the deck".into(),
            Some("Because".into()),
            Some("Alpha".into()),
            "soft".into(),
            "2026-12-03".into(),
            "user".into()
        )
    );
    assert_eq!(ledger(&host, TASK_MOD), [(Some(id), None)]);
    let maker: (String, String, i64) = host.sql(|c| {
        c.query_row(
            "SELECT mod_name, tier, signed FROM mod_ledger WHERE task_id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    });
    assert_eq!(maker, ("Test".into(), "first".into(), 1));
    // The task views are told to re-read, once the guard is gone.
    assert!(host.settled().iter().any(|a| a.tasks));
    // The project counts as engaged with.
    let touched: Option<String> = host.sql(|c| {
        c.query_row(
            "SELECT last_touched FROM projects WHERE name = 'Alpha'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    });
    assert!(touched.is_some());
    // A task with no project and nothing else is fine, and needs no projects.read.
    let (host, s) = first(TASK_MOD, &["tasks.write", "tasks.read"]);
    let v = value(call(&host, &s, "tasks.add", json!({"title": "x"})).await);
    assert!(v["id"].is_i64());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_added_task_that_names_what_does_not_exist_or_breaks_a_rule_makes_nothing() {
    let (host, s) = first(TASK_MOD, &["tasks.write", "tasks.read", "projects.read"]);
    // An unknown project is NotFound, and no project, task or ledger row is made.
    let e = error(
        call(
            &host,
            &s,
            "tasks.add",
            json!({"title": "x", "project": "Nowhere"}),
        )
        .await,
    );
    assert_eq!(e.code, Code::NotFound);
    assert!(!e.message.contains("Nowhere"));
    for bad in [
        json!({"title": ""}),
        json!({"title": "   "}),
        json!({"title": "x".repeat(501)}),
        json!({"title": "x", "why": "y".repeat(10_001)}),
        json!({"title": "x", "deadline": "2026-02-30"}),
        json!({"title": "x", "deadline": "tomorrow"}),
        json!({"title": "x", "deadlineKind": "firm"}),
        json!({"title": "x", "lane": "background"}),
        json!({"title": "x", "stackId": 1}),
        json!({"title": "x", "milestoneId": 1}),
        json!({"title": "x", "source": "assistant"}),
        json!({}),
    ] {
        let e = error(call(&host, &s, "tasks.add", bad.clone()).await);
        assert_eq!(e.code, Code::BadArgs, "{bad}");
        // The refusal says the rule, never what was sent.
        assert!(
            !e.message.contains("firm") && !e.message.contains("tomorrow"),
            "{}",
            e.message
        );
    }
    assert_eq!(count(&host, "SELECT count(*) FROM tasks"), 0);
    assert_eq!(count(&host, "SELECT count(*) FROM projects"), 0);
    assert_eq!(count(&host, "SELECT count(*) FROM mod_ledger"), 0);
}

/// "Ledger row exists iff the task exists": a call that stops being the mod's between the write and the
/// commit leaves neither.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_task_and_its_ledger_row_roll_back_together_when_the_call_ends_before_the_commit() {
    let (host, s) = first(TASK_MOD, &["tasks.write", "tasks.read"]);
    let session = s.clone();
    host.hook(move |h, point| {
        if point == Point::BeforeCommit {
            // The frame goes away between the write and the commit (the guard is held: only the
            // runtime's own lock is taken).
            h.runtime().close(&session);
        }
    });
    assert_eq!(
        code(call(&host, &s, "tasks.add", json!({"title": "x"})).await),
        Code::SessionEnded
    );
    assert_eq!(count(&host, "SELECT count(*) FROM tasks"), 0);
    assert_eq!(count(&host, "SELECT count(*) FROM mod_ledger"), 0);
    assert!(
        host.settled().is_empty(),
        "nothing happened, so nobody is told"
    );
}

/// `n` tasks the mod made, with their ledger rows.
fn make_gifts(host: &TestHost, mod_id: &str, n: i64) {
    host.sql(|c| {
        c.execute(
            "INSERT INTO tasks(title) \
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?1) \
             SELECT 'gift ' || i FROM n",
            [n],
        )
        .unwrap();
        c.execute(
            "INSERT INTO mod_ledger(mod_id, mod_name, tier, author, source_repo, signed, \
                                    install_id, task_id) \
             SELECT m.id, 'Test', m.tier, m.author, m.source_repo, m.signed, m.install_id, t.id \
             FROM mods m, tasks t WHERE m.id = ?1",
            [mod_id],
        )
        .unwrap();
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mod_past_its_gift_quota_is_refused_and_makes_nothing() {
    let (host, s) = first(TASK_MOD, &["tasks.write", "tasks.read"]);
    let max = limits::MAX_GIFTS_PER_MOD as i64;
    make_gifts(&host, TASK_MOD, max);
    assert_eq!(count(&host, "SELECT count(*) FROM mod_ledger"), max);
    let e = error(call(&host, &s, "tasks.add", json!({"title": "one more"})).await);
    assert_eq!(e.code, Code::QuotaExceeded);
    assert_eq!(count(&host, "SELECT count(*) FROM tasks"), max);
    let e = error(
        call(
            &host,
            &s,
            "tasks.importChecklist",
            json!({"items": [{"title": "a", "done": false}]}),
        )
        .await,
    );
    assert_eq!(e.code, Code::QuotaExceeded);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gift_calls_are_rate_limited_by_their_family() {
    let (host, s) = first(TASK_MOD, &["tasks.write", "tasks.read"]);
    // The family's burst is 50, refilling 10 a second: a quiet mod gets its burst at once, then waits.
    let mut refused = None;
    for n in 0..(limits::GIFT_BURST as usize + 5) {
        let reply = call(&host, &s, "tasks.add", json!({"title": format!("t{n}")})).await;
        if let super::Reply::Error { error } = reply {
            refused = Some((n, error.code));
            break;
        }
    }
    let (at, code) = refused.expect("the family's bucket ran dry");
    assert_eq!(code, Code::RateLimited);
    assert!(
        (limits::GIFT_BURST as usize..limits::GIFT_BURST as usize + 5).contains(&at),
        "refused at call {at}"
    );
    // Other calls of the mod are still served: the family's bucket is its own.
    value(call(&host, &s, "tasks.list", json!({})).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn any_tasks_status_may_be_set_and_done_is_stamped() {
    let (host, s) = first(TASK_MOD, &["tasks.write", "tasks.read"]);
    // A task of the user's own, not the mod's.
    let id = host.sql(|c| seed_task(c, "mine", None));
    let v = value(
        call(
            &host,
            &s,
            "tasks.setStatus",
            json!({"id": id, "status": "done"}),
        )
        .await,
    );
    assert_eq!(v, json!({"id": id, "status": "done"}));
    let (status, done): (String, Option<String>) = host.sql(|c| {
        c.query_row(
            "SELECT status, completed_at FROM tasks WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    });
    assert_eq!(status, "done");
    assert!(done.is_some());
    value(
        call(
            &host,
            &s,
            "tasks.setStatus",
            json!({"id": id, "status": "doing"}),
        )
        .await,
    );
    let done: Option<String> = host.sql(|c| {
        c.query_row("SELECT completed_at FROM tasks WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap()
    });
    assert!(done.is_none(), "leaving done clears when it was finished");
    assert!(host.settled().iter().any(|a| a.tasks));
    // Unknown task, unknown status, extra fields.
    assert_eq!(
        code(
            call(
                &host,
                &s,
                "tasks.setStatus",
                json!({"id": 999, "status": "done"})
            )
            .await
        ),
        Code::NotFound
    );
    for bad in [
        json!({"id": id, "status": "finished"}),
        json!({"id": id}),
        json!({"id": id, "status": "done", "x": 1}),
    ] {
        assert_eq!(
            code(call(&host, &s, "tasks.setStatus", bad).await),
            Code::BadArgs
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reorder_puts_the_named_tasks_in_each_others_places_or_changes_nothing() {
    let (host, s) = first(TASK_MOD, &["tasks.write", "tasks.read"]);
    let ids: Vec<i64> = host.sql(|c| {
        (0..4)
            .map(|i| seed_task(c, &format!("t{i}"), None))
            .collect()
    });
    let order = |host: &TestHost| -> Vec<i64> {
        host.sql(|c| {
            let mut stmt = c
                .prepare("SELECT id FROM tasks ORDER BY ordinal, id")
                .unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        })
    };
    // t3 and t1 swap places; the others keep theirs.
    let v = value(call(&host, &s, "tasks.reorder", json!({"ids": [ids[3], ids[1]]})).await);
    assert_eq!(v, json!({"reordered": 2}));
    assert_eq!(order(&host), [ids[0], ids[3], ids[2], ids[1]]);
    // One unknown id and nothing changes.
    let before = order(&host);
    let e = error(call(&host, &s, "tasks.reorder", json!({"ids": [ids[0], 999]})).await);
    assert_eq!(e.code, Code::NotFound);
    assert_eq!(order(&host), before);
    // A task named twice, too many, and none at all.
    assert_eq!(
        code(call(&host, &s, "tasks.reorder", json!({"ids": [ids[0], ids[0]]})).await),
        Code::BadArgs
    );
    let many: Vec<i64> = (1..=limits::MAX_TASKS_PER_CALL as i64 + 1).collect();
    assert_eq!(
        code(call(&host, &s, "tasks.reorder", json!({"ids": many})).await),
        Code::BadArgs
    );
    assert_eq!(
        value(call(&host, &s, "tasks.reorder", json!({"ids": []})).await),
        json!({"reordered": 0})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_imported_checklist_makes_a_gift_of_every_line_it_keeps() {
    let (host, s) = first(TASK_MOD, &["tasks.write", "tasks.read", "projects.read"]);
    host.sql(|c| {
        seed_project(c, "Alpha");
        seed_task(c, "Already there", Some("Alpha"));
    });
    let v = value(
        call(
            &host,
            &s,
            "tasks.importChecklist",
            json!({"project": "Alpha", "items": [
                {"title": "Book room", "done": false},
                {"title": "  already THERE ", "done": false},
                {"title": "Send invites", "done": true},
                {"title": "  ", "done": false},
                {"title": "book room", "done": true},
            ]}),
        )
        .await,
    );
    let created: Vec<i64> = v["created"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i.as_i64().unwrap())
        .collect();
    assert_eq!((created.len(), v["skipped"].as_u64()), (2, Some(3)));
    // Each kept line is the mod's gift, written with its task.
    assert_eq!(
        ledger(&host, TASK_MOD),
        created
            .iter()
            .map(|id| (Some(*id), None))
            .collect::<Vec<_>>()
    );
    let done: Vec<String> = host.sql(|c| {
        let mut stmt = c
            .prepare("SELECT status FROM tasks WHERE title <> 'Already there' ORDER BY id")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    });
    assert_eq!(done, ["todo", "done"]);
    assert!(host.settled().iter().any(|a| a.tasks));
    // An unknown project, and too many lines.
    assert_eq!(
        code(
            call(
                &host,
                &s,
                "tasks.importChecklist",
                json!({"project": "Nope", "items": [{"title": "x", "done": false}]})
            )
            .await
        ),
        Code::NotFound
    );
    let items: Vec<Value> = (0..=limits::MAX_CHECKLIST_ITEMS)
        .map(|i| json!({"title": format!("l{i}"), "done": false}))
        .collect();
    assert_eq!(
        code(call(&host, &s, "tasks.importChecklist", json!({"items": items})).await),
        Code::BadArgs
    );
    // Nothing imported, nothing announced: a list of blanks tells nobody to re-read.
    let before = host.settled().len();
    let v = value(
        call(
            &host,
            &s,
            "tasks.importChecklist",
            json!({"items": [{"title": " ", "done": false}]}),
        )
        .await,
    );
    assert_eq!(v, json!({"created": [], "skipped": 1}));
    assert_eq!(host.settled().len(), before + 1);
    assert!(!host.settled().last().unwrap().tasks);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_import_over_the_quota_as_a_whole_is_refused_before_any_line_is_made() {
    let (host, s) = first(TASK_MOD, &["tasks.write", "tasks.read"]);
    let max = limits::MAX_GIFTS_PER_MOD as i64;
    make_gifts(&host, TASK_MOD, max - 1);
    let two = json!({"items": [{"title": "a", "done": false}, {"title": "b", "done": false}]});
    assert_eq!(
        code(call(&host, &s, "tasks.importChecklist", two).await),
        Code::QuotaExceeded
    );
    assert_eq!(count(&host, "SELECT count(*) FROM tasks"), max - 1);
    // One fits.
    let one = json!({"items": [{"title": "a", "done": false}]});
    assert_eq!(
        value(call(&host, &s, "tasks.importChecklist", one).await)["skipped"],
        0
    );
}

// --- ui.openTaskEditor --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_editor_opens_on_a_task_that_exists_and_on_a_new_one_with_a_prefill() {
    let (host, s) = first(TASK_MOD, &["tasks.write", "tasks.read", "projects.read"]);
    let id = host.sql(|c| seed_task(c, "mine", None));
    let (action, args) = page(call(&host, &s, "ui.openTaskEditor", json!({"id": id})).await);
    assert_eq!(
        (action, args),
        (PageAction::UiOpenTaskEditor, json!({"id": id}))
    );
    // Nothing was made or changed by opening it: a task made in the editor is the person's own.
    assert_eq!(count(&host, "SELECT count(*) FROM mod_ledger"), 0);
    let (_, args) = page(
        call(
            &host,
            &s,
            "ui.openTaskEditor",
            json!({"new": {"title": "  Call Sam ", "project": " Atlas ", "deadline": "2026-12-01"}}),
        )
        .await,
    );
    assert_eq!(
        args,
        json!({"new": {"title": "Call Sam", "project": "Atlas", "deadline": "2026-12-01"}})
    );
    // A prefill names no lookup: a project that doesn't exist is only text in a field.
    let (_, args) = page(call(&host, &s, "ui.openTaskEditor", json!({"new": {}})).await);
    assert_eq!(
        args,
        json!({"new": {"title": null, "project": null, "deadline": null}})
    );
    assert_eq!(count(&host, "SELECT count(*) FROM projects"), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_editor_refuses_an_unknown_task_a_bad_prefill_and_an_unreadable_project() {
    let (host, s) = first(TASK_MOD, &["tasks.write", "tasks.read", "projects.read"]);
    assert_eq!(
        code(call(&host, &s, "ui.openTaskEditor", json!({"id": 999})).await),
        Code::NotFound
    );
    for bad in [
        json!({}),
        json!({"id": 1, "new": {}}),
        json!({"new": {"title": "x".repeat(501)}}),
        json!({"new": {"deadline": "2026-02-30"}}),
        json!({"new": {"project": "line\nbreak"}}),
        json!({"new": {"project": "p".repeat(201)}}),
        json!({"new": {"extra": 1}}),
    ] {
        assert_eq!(
            code(call(&host, &s, "ui.openTaskEditor", bad.clone()).await),
            Code::BadArgs,
            "{bad}"
        );
    }
    // The person's own words: an emoji and an inner no-break space are fine in a project name, a
    // hidden character or a no-break space at an end is not.
    for good in ["\u{26a0}\u{fe0f} Visa", "Team\u{a0}A"] {
        let (_, args) = page(
            call(
                &host,
                &s,
                "ui.openTaskEditor",
                json!({"new": {"project": good}}),
            )
            .await,
        );
        assert_eq!(args["new"]["project"], good);
    }
    for bad in ["flip\u{202e}ed", "zero\u{200b}width", "tab\there"] {
        assert_eq!(
            code(
                call(
                    &host,
                    &s,
                    "ui.openTaskEditor",
                    json!({"new": {"project": bad}})
                )
                .await
            ),
            Code::BadArgs,
            "{bad:?}"
        );
    }
    // A prefilled project is a name the mod was shown: without projects.read it is refused.
    let (host, s) = first(TASK_MOD, &["tasks.write", "tasks.read"]);
    let e = error(
        call(
            &host,
            &s,
            "ui.openTaskEditor",
            json!({"new": {"project": "Atlas"}}),
        )
        .await,
    );
    assert_eq!(
        (e.code, e.permission),
        (Code::NotGranted, Some("projects.read"))
    );
    // A new task with no project needs none.
    page(
        call(
            &host,
            &s,
            "ui.openTaskEditor",
            json!({"new": {"title": "x"}}),
        )
        .await,
    );
}
