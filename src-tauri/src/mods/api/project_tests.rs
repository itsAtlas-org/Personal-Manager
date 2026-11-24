// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `projects.list` and the rule that every write call of K2 commits nothing for a mod that is gone
//! (PR C, K2).

use serde_json::json;

use super::call_fixtures::*;
use super::{limits, Code, Point};

const MOD: &str = "atlas.lister";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_project_list_is_names_only_in_the_pickers_order_and_needs_projects_read() {
    let (host, s) = first(MOD, &["projects.read"]);
    host.sql(|c| {
        // The registry PM's own pickers read (`tags::project_names`).
        for name in ["beta", "Alpha", "alpha two"] {
            c.execute(
                "INSERT INTO tags(kind, name, norm) VALUES ('project', ?1, lower(?1))",
                [name],
            )
            .unwrap();
        }
        // Counts, an archive state and aliases exist on a project, and none is for a mod.
        seed_project(c, "beta");
        c.execute(
            "UPDATE projects SET archived_at = '2026-01-01T00:00:00Z' WHERE name = 'beta'",
            [],
        )
        .unwrap();
        seed_task(c, "t", Some("beta"));
    });
    let v = value(call(&host, &s, "projects.list", json!({})).await);
    assert_eq!(
        v,
        json!({"projects": ["Alpha", "alpha two", "beta"], "truncated": false})
    );
    // No arguments.
    assert_eq!(
        code(call(&host, &s, "projects.list", json!({"all": true})).await),
        Code::BadArgs
    );
    // Without projects.read: refused.
    let (_host, s) = first(MOD, &["tasks.read"]);
    let e = error(call(&_host, &s, "projects.list", json!({})).await);
    assert_eq!(
        (e.code, e.permission),
        (Code::NotGranted, Some("projects.read"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_list_past_the_cap_is_cut_and_says_so() {
    let (host, s) = first(MOD, &["projects.read"]);
    host.sql(|c| {
        c.execute(
            "INSERT INTO tags(kind, name, norm) \
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?1) \
             SELECT 'project', printf('p%06d', i), printf('p%06d', i) FROM n",
            [limits::MAX_PROJECTS_LISTED as i64 + 1],
        )
        .unwrap();
    });
    let v = value(call(&host, &s, "projects.list", json!({})).await);
    assert_eq!(
        v["projects"].as_array().unwrap().len(),
        limits::MAX_PROJECTS_LISTED
    );
    assert_eq!(v["truncated"], true);
}

/// A call authorised and then overtaken by an uninstall (the mod's row gone before its data step)
/// commits nothing, for each of the calls that write.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_in_flight_across_an_uninstall_commits_nothing() {
    let calls = [
        ("tasks.add", json!({"title": "x"})),
        (
            "tasks.importChecklist",
            json!({"items": [{"title": "x", "done": false}]}),
        ),
        ("milestones.add", json!({"project": "Alpha", "label": "x"})),
        ("tasks.setStatus", json!({"id": 1, "status": "done"})),
        (
            "calendar.overlay.set",
            json!({"entries": [{"id": "a", "date": "2026-12-01", "label": "x"}]}),
        ),
    ];
    for (method, args) in calls {
        let (host, s) = world(
            MOD,
            true,
            "first",
            &[
                "tasks.write",
                "tasks.read",
                "milestones.write",
                "projects.read",
                "storage",
            ],
            &["calendar.overlay"],
        );
        host.sql(|c| {
            seed_project(c, "Alpha");
            seed_task(c, "mine", Some("Alpha"));
        });
        host.hook(|h, point| {
            if point == Point::BeforeData {
                // The mod is uninstalled after the call was authorised and before its data step.
                h.sql(|c| c.execute("DELETE FROM mods", []).unwrap());
            }
        });
        assert_eq!(
            code(call(&host, &s, method, args).await),
            Code::SessionEnded,
            "{method}"
        );
        assert_eq!(count(&host, "SELECT count(*) FROM tasks"), 1, "{method}");
        assert_eq!(
            count(&host, "SELECT count(*) FROM project_milestones"),
            0,
            "{method}"
        );
        assert_eq!(
            count(&host, "SELECT count(*) FROM mod_ledger"),
            0,
            "{method}"
        );
        assert_eq!(
            count(&host, "SELECT count(*) FROM mod_overlay"),
            0,
            "{method}"
        );
        let status: String = host.sql(|c| {
            c.query_row("SELECT status FROM tasks", [], |r| r.get(0))
                .unwrap()
        });
        assert_eq!(status, "todo", "{method}");
    }
}
