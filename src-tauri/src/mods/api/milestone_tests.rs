// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The milestone calls through the real dispatcher over a real store (PR C, K2): the list's DTO under
//! every grant; `milestones.add` (the gift, the quota, and the question PM asks before it makes a
//! project that doesn't exist); `update` and `setStatus` through the one service layer; and `delete`
//! (a milestone the mod made goes at once, any other asks the person first).

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::call_fixtures::*;
use super::confirm::complete;
use super::milestone_calls::{milestone_dto, CREATE_PROJECT_QUESTION, DELETE_MILESTONE_QUESTION};
use super::test_support::TestHost;
use super::{limits, Code, ModError, Point, Reply};
use crate::mods::api::projection::Projection;

const MOD: &str = "atlas.miler";

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().unwrap().keys().cloned().collect()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| (*n).to_owned()).collect()
}

/// A mod that writes and reads milestones.
fn miler() -> (TestHost, String) {
    first(MOD, &["milestones.write", "projects.read"])
}

/// The token of a `MOD-2003` answer.
fn token(e: &ModError) -> String {
    assert_eq!(e.code, Code::NeedsConfirmation, "{e}");
    e.confirm.as_ref().expect("a question").token.clone()
}

/// The person's answer to a question.
async fn answer(host: &TestHost, token: &str, accept: bool) -> Result<Value, ModError> {
    match complete(host, "main", token, accept).await {
        Reply::Value { value } => Ok(value),
        Reply::Error { error } => Err(error),
        other => panic!("{other:?}"),
    }
}

fn milestone_rows(host: &TestHost) -> i64 {
    count(host, "SELECT count(*) FROM project_milestones")
}

// --- milestones.list ----------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_milestone_list_has_exactly_the_fields_the_callers_grants_allow() {
    for calendar in [false, true] {
        let mut perms = vec!["projects.read"];
        if calendar {
            perms.push("calendar.read");
        }
        let (host, s) = first(MOD, &perms);
        host.sql(|c| {
            seed_milestone(c, "Alpha", "Launch");
            c.execute(
                "INSERT INTO calendar_events(id, calendar_id, summary, start, uid) \
                 VALUES ('c:1', 'c', 'Sprint review', '2026-12-05T10:00:00Z', 'uid-1@example.com')",
                [],
            )
            .unwrap();
            c.execute(
                "INSERT INTO project_milestones(project_name, label, event_uid, due_date) \
                 VALUES ('Alpha', 'Review', 'uid-1@example.com', '2026-01-01')",
                [],
            )
            .unwrap();
            // A synced one: its source and external id are not for a mod.
            crate::milestones::upsert_external(
                c,
                "Alpha",
                "sheets",
                "row-77",
                "Synced",
                None,
                Some("in_progress"),
            )
            .unwrap();
        });
        let v = value(call(&host, &s, "milestones.list", json!({})).await);
        let list = v["milestones"].as_array().unwrap();
        assert_eq!(list.len(), 3, "calendar {calendar}");
        let mut expected = set(&[
            "id",
            "project_name",
            "label",
            "due_date",
            "calendar_linked",
            "state",
            "status",
            "sort_order",
        ]);
        if calendar {
            expected.extend(set(&["event_uid", "event_missing"]));
        }
        for m in list {
            assert_eq!(keys(m), expected, "calendar {calendar}");
        }
        // The date a link resolved to is the date, whoever reads it; the link's own flag is always said.
        assert_eq!(list[1]["calendar_linked"], true);
        assert_eq!(list[1]["due_date"], "2026-12-05");
        assert_eq!(list[0]["calendar_linked"], false);
        let text = v.to_string();
        assert!(!text.contains("sheets") && !text.contains("row-77"));
        assert_eq!(
            text.contains("uid-1@example.com"),
            calendar,
            "the event's uid is calendar.read's"
        );
    }
}

/// The DTO names every field of a core milestone, or says it is withheld: a column added to a
/// milestone cannot leak by being added.
#[test]
fn the_milestone_dto_accounts_for_every_field_of_a_core_milestone() {
    let (_dir, conn) = super::test_support::store();
    seed_milestone(&conn, "Alpha", "Launch");
    let m = crate::milestones::list_all(&conn, "2026-11-24")
        .unwrap()
        .remove(0);
    let core = keys(&serde_json::to_value(&m).unwrap());
    let dto = keys(&milestone_dto(&m, Projection::scoped(true, true)));
    // What a mod is never told of a milestone: where a synced one came from, and which mod made it (PM's
    // own label of the ledger, `madeBy`, never another maker's name in a mod's answer).
    let withheld = set(&["source_type", "external_id", "madeBy"]);
    assert_eq!(
        core.difference(&dto).cloned().collect::<BTreeSet<_>>(),
        withheld
    );
    assert!(dto.is_subset(&core));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_milestone_list_comes_in_pages_by_id() {
    let (host, s) = miler();
    host.sql(|c| {
        for i in 0..5 {
            seed_milestone(c, "Alpha", &format!("m{i}"));
        }
    });
    let mut seen = Vec::new();
    let mut after = 0;
    loop {
        let v = value(
            call(
                &host,
                &s,
                "milestones.list",
                json!({"after": after, "limit": 2}),
            )
            .await,
        );
        seen.extend(
            v["milestones"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| m["label"].as_str().unwrap().to_owned()),
        );
        match v["next"].as_i64() {
            Some(next) => after = next,
            None => break,
        }
    }
    assert_eq!(seen, ["m0", "m1", "m2", "m3", "m4"]);
    for bad in [json!({"after": -1}), json!({"limit": 0}), json!({"x": 1})] {
        assert_eq!(
            code(call(&host, &s, "milestones.list", bad).await),
            Code::BadArgs
        );
    }
}

// --- milestones.add -----------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_milestone_in_a_known_project_is_a_gift_made_through_the_service_layer() {
    let (host, s) = miler();
    host.sql(|c| seed_project(c, "Alpha"));
    let v = value(
        call(
            &host,
            &s,
            "milestones.add",
            json!({"project": " alpha ", "label": " Launch ", "dueDate": "2026-12-01"}),
        )
        .await,
    );
    let id = v["id"].as_i64().unwrap();
    assert_eq!(v, json!({"id": id}));
    let row: (String, String, Option<String>) = host.sql(|c| {
        c.query_row(
            "SELECT project_name, label, due_date FROM project_milestones WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    });
    assert_eq!(
        row,
        ("Alpha".into(), "Launch".into(), Some("2026-12-01".into()))
    );
    // The gift's ledger row, written with it; the project engaged with and the observation logged, as
    // the page's own add does; the briefing told to look again.
    assert_eq!(ledger(&host, MOD), [(None, Some(id))]);
    assert_eq!(
        count(
            &host,
            "SELECT count(*) FROM project_activity WHERE project = 'Alpha'"
        ),
        1
    );
    assert!(host.settled().iter().any(|a| a.briefing));
    // A blank label is named "deadline", as the page does.
    let v = value(
        call(
            &host,
            &s,
            "milestones.add",
            json!({"project": "Alpha", "label": "  "}),
        )
        .await,
    );
    let label: String = host.sql(|c| {
        c.query_row(
            "SELECT label FROM project_milestones WHERE id = ?1",
            [v["id"].as_i64().unwrap()],
            |r| r.get(0),
        )
        .unwrap()
    });
    assert_eq!(label, "deadline");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bad_label_or_date_makes_nothing_and_never_asks() {
    let (host, s) = miler();
    host.sql(|c| seed_project(c, "Alpha"));
    for bad in [
        json!({"project": "Alpha", "label": "x".repeat(201)}),
        json!({"project": "Alpha", "label": "line\nbreak"}),
        json!({"project": "Alpha", "label": "ok", "dueDate": "2026-02-30"}),
        json!({"project": "Alpha", "label": "ok", "dueDate": "01/12/2026"}),
        json!({"project": "Alpha", "label": "ok", "dueDate": "2026-12-01T10:00"}),
        json!({"project": "Alpha", "label": "ok", "eventUid": "u"}),
        json!({"project": "Alpha"}),
    ] {
        assert_eq!(
            code(call(&host, &s, "milestones.add", bad.clone()).await),
            Code::BadArgs,
            "{bad}"
        );
    }
    assert_eq!(milestone_rows(&host), 0);
    assert_eq!(count(&host, "SELECT count(*) FROM mod_ledger"), 0);
}

/// Q2 (Bobby, 2026-11-18): a project that doesn't exist is made only after PM asks, for every tier.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_project_asks_first_and_creates_nothing_until_the_person_says_yes() {
    let tiers: [(&str, MakeWorld); 3] = [
        ("first", |p| first(MOD, p)),
        ("second", |p| second("acme.miler", p)),
        ("unsigned", |p| unsigned("acme.miler", "third", p)),
    ];
    for (tier, make) in tiers {
        let (host, s) = make(&["milestones.write", "projects.read"]);
        let mod_id = if tier == "first" { MOD } else { "acme.miler" };
        let e = error(
            call(
                &host,
                &s,
                "milestones.add",
                json!({"project": "  Brand New ", "label": "Kick-off", "dueDate": "2026-12-01"}),
            )
            .await,
        );
        let t = token(&e);
        let question = e.confirm.unwrap().summary;
        assert_eq!(
            question,
            format!("{CREATE_PROJECT_QUESTION}\nBrand New"),
            "{tier}"
        );
        // Nothing exists yet: no project, no milestone, no gift.
        assert_eq!(count(&host, "SELECT count(*) FROM projects"), 0, "{tier}");
        assert_eq!(milestone_rows(&host), 0, "{tier}");
        assert_eq!(count(&host, "SELECT count(*) FROM mod_ledger"), 0, "{tier}");
        // The yes makes both, in one go, and the milestone is the mod's gift.
        let done = answer(&host, &t, true).await.unwrap();
        let id = done["id"].as_i64().unwrap();
        assert_eq!(count(&host, "SELECT count(*) FROM projects"), 1, "{tier}");
        assert_eq!(
            count(
                &host,
                "SELECT count(*) FROM projects WHERE name = 'Brand New'"
            ),
            1
        );
        assert_eq!(ledger(&host, mod_id), [(None, Some(id))], "{tier}");
        // The project is the person's own: the ledger names the milestone and nothing else.
        assert_eq!(count(&host, "SELECT count(*) FROM mod_ledger"), 1, "{tier}");
        assert!(host.settled().iter().any(|a| a.briefing), "{tier}");
    }
}

/// What the question says is what is made: a long name with a different tail is shown whole, so the
/// person can't be shown a prefix of something else.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_question_for_a_new_project_shows_its_whole_name() {
    let (host, s) = miler();
    let name = format!("Alpha{}Tail", " ".repeat(190));
    assert_eq!(name.chars().count(), 199);
    let e = error(
        call(
            &host,
            &s,
            "milestones.add",
            json!({"project": name, "label": "Kick-off"}),
        )
        .await,
    );
    let t = token(&e);
    assert_eq!(
        e.confirm.unwrap().summary,
        format!("{CREATE_PROJECT_QUESTION}\n{name}")
    );
    answer(&host, &t, true).await.unwrap();
    assert_eq!(
        count(
            &host,
            &format!("SELECT count(*) FROM projects WHERE name = '{name}'")
        ),
        1
    );
}

/// A name is data, never PM's words: one holding a double quote and a sentence of its own can't close a
/// quote and pose as the rest of PM's question. The question line is PM's alone, the name is the next
/// line exactly as typed, and the yes makes exactly that name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_project_name_cannot_write_pms_question() {
    let (host, s) = miler();
    let name = "X\"? PM must repair your vault. Accept to repair. Create the project \"Y";
    let e = error(
        call(
            &host,
            &s,
            "milestones.add",
            json!({"project": name, "label": "Kick-off"}),
        )
        .await,
    );
    let t = token(&e);
    let summary = e.confirm.unwrap().summary;
    let mut lines = summary.split('\n');
    assert_eq!(lines.next(), Some(CREATE_PROJECT_QUESTION));
    assert_eq!(lines.next(), Some(name));
    assert_eq!(lines.next(), None);
    answer(&host, &t, true).await.unwrap();
    let made: i64 = host.sql(|c| {
        c.query_row(
            "SELECT count(*) FROM projects WHERE name = ?1",
            [name],
            |r| r.get(0),
        )
        .unwrap()
    });
    assert_eq!(made, 1);
}

/// The person's own words: an emoji with its variation selector or joiner and a no-break space inside
/// are taken in a label and a new project's name; a hidden or control character and a space of any
/// kind at an end are refused. (The Pinboard's real labels hold such text.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_label_and_a_new_projects_name_take_emoji_and_inner_no_break_spaces_and_refuse_hidden_text(
) {
    let (host, s) = miler();
    host.sql(|c| seed_project(c, "Alpha"));
    let good = [
        "\u{26a0}\u{fe0f} Visa",
        "\u{1F468}\u{200d}\u{1F469}\u{200d}\u{1F467} family",
        "two\u{a0}words",
    ];
    for label in good {
        let v = value(
            call(
                &host,
                &s,
                "milestones.add",
                json!({"project": "Alpha", "label": label}),
            )
            .await,
        );
        let id = v["id"].as_i64().unwrap();
        let stored: String = host.sql(|c| {
            c.query_row(
                "SELECT label FROM project_milestones WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
        });
        assert_eq!(stored, label, "stored as sent");
        // The update call holds the same rule.
        value(
            call(
                &host,
                &s,
                "milestones.update",
                json!({"id": id, "label": format!("{label}!")}),
            )
            .await,
        );
    }
    let bad = [
        "flip\u{202e}ed",
        "zero\u{200b}width",
        "bell\u{7}",
        "tab\there",
        "\u{200d}",
        // Every other line break is refused too, so none can reach a summary line (the page draws each
        // line after the first as the thing asked about).
        "line\rbreak",
        "line\u{2028}break",
        "line\u{2029}break",
        "line\u{85}break",
        "line\u{b}break",
        "line\u{c}break",
    ];
    let rows_before = milestone_rows(&host);
    let kept = host.sql(|c| seed_milestone(c, "Alpha", "Kept"));
    for label in bad {
        assert_eq!(
            code(
                call(
                    &host,
                    &s,
                    "milestones.add",
                    json!({"project": "Alpha", "label": label})
                )
                .await
            ),
            Code::BadArgs,
            "{label:?}"
        );
        // The update call refuses it too, and the label stays as it was.
        assert_eq!(
            code(
                call(
                    &host,
                    &s,
                    "milestones.update",
                    json!({"id": kept, "label": label})
                )
                .await
            ),
            Code::BadArgs,
            "{label:?}"
        );
        // A refused name for a new project asks nothing and makes nothing. (A lone joiner after
        // a letter is part of an emoji sequence, so it is the label that has nothing to show.)
        if label != "\u{200d}" {
            assert_eq!(
                code(
                    call(
                        &host,
                        &s,
                        "milestones.add",
                        json!({"project": format!("New {label}"), "label": "x"})
                    )
                    .await
                ),
                Code::BadArgs,
                "{label:?}"
            );
        }
    }
    assert_eq!(
        milestone_rows(&host),
        rows_before + 1,
        "only the seeded one"
    );
    assert_eq!(
        host.sql(|c| c
            .query_row(
                "SELECT label FROM project_milestones WHERE id = ?1",
                [kept],
                |r| r.get::<_, String>(0)
            )
            .unwrap()),
        "Kept"
    );
    // Spaces of any kind at the ends are trimmed from a label as the call has always trimmed them.
    let v = value(
        call(
            &host,
            &s,
            "milestones.add",
            json!({"project": "Alpha", "label": "\u{a0} edges \u{a0}"}),
        )
        .await,
    );
    let id = v["id"].as_i64().unwrap();
    assert_eq!(
        host.sql(|c| c
            .query_row(
                "SELECT label FROM project_milestones WHERE id = ?1",
                [id],
                |r| r.get::<_, String>(0)
            )
            .unwrap()),
        "edges"
    );
    // A new project named with an emoji is asked about whole, and made as asked.
    let name = "Launch \u{1F680}\u{fe0f}\u{a0}2027";
    let e = error(
        call(
            &host,
            &s,
            "milestones.add",
            json!({"project": name, "label": "Go"}),
        )
        .await,
    );
    assert_eq!(
        e.clone().confirm.unwrap().summary,
        format!("{CREATE_PROJECT_QUESTION}\n{name}")
    );
    answer(&host, &token(&e), true).await.unwrap();
    assert_eq!(
        count(
            &host,
            &format!("SELECT count(*) FROM projects WHERE name = '{name}'")
        ),
        1
    );
}

/// F-PROJ: a project the person said yes to is a project like any other: in the tag registry PM's own
/// pickers and `projects.list` read, and an entity that can be renamed or merged. All in the same
/// transaction as the milestone, with no ledger row of its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_project_the_person_agreed_to_is_listed_and_known_like_a_filed_one() {
    let (host, s) = miler();
    let e = error(
        call(
            &host,
            &s,
            "milestones.add",
            json!({"project": "Fresh Start", "label": "Kick-off"}),
        )
        .await,
    );
    assert_eq!(
        value(call(&host, &s, "projects.list", json!({})).await)["projects"],
        json!([]),
        "nothing before the yes"
    );
    answer(&host, &token(&e), true).await.unwrap();
    let listed = value(call(&host, &s, "projects.list", json!({})).await);
    assert_eq!(listed["projects"], json!(["Fresh Start"]));
    let (names, entity): (Vec<String>, Option<i64>) = host.sql(|c| {
        (
            crate::tags::project_names(c).unwrap(),
            crate::entities::resolve_project(c, "Fresh Start", false).unwrap(),
        )
    });
    assert_eq!(names, ["Fresh Start"], "PM's own pickers list it");
    assert!(entity.is_some(), "and it is an entity that can be renamed");
    assert_eq!(count(&host, "SELECT count(*) FROM mod_ledger"), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_no_to_a_new_project_makes_nothing_and_a_token_is_used_once() {
    let (host, s) = miler();
    let args = json!({"project": "Brand New", "label": "Kick-off"});
    let t = token(&error(
        call(&host, &s, "milestones.add", args.clone()).await,
    ));
    assert_eq!(
        answer(&host, &t, false).await.unwrap_err().code,
        Code::Declined
    );
    assert_eq!(count(&host, "SELECT count(*) FROM projects"), 0);
    assert_eq!(milestone_rows(&host), 0);
    // The no is final: the same token can't be turned into a yes.
    assert_eq!(
        answer(&host, &t, true).await.unwrap_err().code,
        Code::Declined
    );
    assert_eq!(milestone_rows(&host), 0);
    // Asking again is a new question.
    let t2 = token(&error(call(&host, &s, "milestones.add", args).await));
    assert_ne!(t, t2);
    answer(&host, &t2, true).await.unwrap();
    assert_eq!(milestone_rows(&host), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_project_question_is_void_if_the_project_appears_or_the_quota_fills_meanwhile() {
    let (host, s) = miler();
    let args = json!({"project": "Brand New", "label": "Kick-off"});
    // The project appears (the person made it) while the question is open: it no longer describes what
    // would happen, so nothing is done.
    let t = token(&error(
        call(&host, &s, "milestones.add", args.clone()).await,
    ));
    host.sql(|c| seed_project(c, "Brand New"));
    assert_eq!(
        answer(&host, &t, true).await.unwrap_err().code,
        Code::StaleRevision
    );
    assert_eq!(milestone_rows(&host), 0);
    // The mod's gifts fill while the question is open: refused at the answer.
    let (host, s) = miler();
    let t = token(&error(call(&host, &s, "milestones.add", args).await));
    host.sql(|c| {
        c.execute(
            "INSERT INTO tasks(title) \
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?1) \
             SELECT 'gift ' || i FROM n",
            [limits::MAX_GIFTS_PER_MOD as i64],
        )
        .unwrap();
        c.execute(
            "INSERT INTO mod_ledger(mod_id, mod_name, tier, author, source_repo, signed, \
                                    install_id, task_id) \
             SELECT m.id, 'Test', m.tier, m.author, m.source_repo, m.signed, m.install_id, t.id \
             FROM mods m, tasks t WHERE m.id = ?1",
            [MOD],
        )
        .unwrap();
    });
    assert_eq!(
        answer(&host, &t, true).await.unwrap_err().code,
        Code::QuotaExceeded
    );
    assert_eq!(count(&host, "SELECT count(*) FROM projects"), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_project_needs_projects_read_a_plain_name_and_is_made_only_by_the_question() {
    // Without projects.read the answer to "does this project exist" is not the mod's.
    let (host, s) = first(MOD, &["milestones.write"]);
    let e = error(
        call(
            &host,
            &s,
            "milestones.add",
            json!({"project": "Brand New", "label": "x"}),
        )
        .await,
    );
    assert_eq!(
        (e.code, e.permission),
        (Code::NotGranted, Some("projects.read"))
    );
    // A name PM could not show safely is refused, not asked about.
    let (host, s) = miler();
    for bad in ["", "   ", "two\nlines", &"p".repeat(201)] {
        let e = error(
            call(
                &host,
                &s,
                "milestones.add",
                json!({"project": bad, "label": "x"}),
            )
            .await,
        );
        assert_eq!(e.code, Code::BadArgs, "{bad:?}");
        assert!(e.confirm.is_none());
    }
    // A second ask while one is open is refused, not queued.
    let args = json!({"project": "Brand New", "label": "x"});
    let t = token(&error(
        call(&host, &s, "milestones.add", args.clone()).await,
    ));
    assert_eq!(
        code(call(&host, &s, "milestones.add", args).await),
        Code::RateLimited
    );
    assert!(answer(&host, &t, true).await.is_ok());
}

// --- milestones.update / setStatus --------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn any_milestone_may_be_edited_and_its_status_set_through_the_service_layer() {
    let (host, s) = miler();
    // Not the mod's gift: the user's own.
    let id = host.sql(|c| seed_milestone(c, "Alpha", "Launch"));
    let v = value(
        call(
            &host,
            &s,
            "milestones.update",
            json!({"id": id, "label": "Go live", "dueDate": "2026-12-09"}),
        )
        .await,
    );
    assert_eq!(v, json!({"id": id}));
    let row: (String, Option<String>) = host.sql(|c| {
        c.query_row(
            "SELECT label, due_date FROM project_milestones WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    });
    assert_eq!(row, ("Go live".into(), Some("2026-12-09".into())));
    assert!(host.settled().iter().any(|a| a.tasks && a.briefing));
    // A blank label is "deadline".
    value(
        call(
            &host,
            &s,
            "milestones.update",
            json!({"id": id, "label": ""}),
        )
        .await,
    );
    let label: String = host.sql(|c| {
        c.query_row(
            "SELECT label FROM project_milestones WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap()
    });
    assert_eq!(label, "deadline");
    // Status: done is met, and leaving done re-opens the flag the person closed on it, exactly as the
    // page's own control does (one service layer, one table of cases in `ops`).
    value(
        call(
            &host,
            &s,
            "milestones.setStatus",
            json!({"id": id, "status": "done"}),
        )
        .await,
    );
    host.sql(|c| {
        c.execute(
            "INSERT INTO flags(anchor_kind, anchor, type, state, source) \
             VALUES ('milestone', ?1, 'deadline-approaching', 'resolved', 'assertion')",
            [id.to_string()],
        )
        .unwrap();
    });
    let state: (String, String) = host.sql(|c| {
        c.query_row(
            "SELECT state, status FROM project_milestones WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    });
    assert_eq!(state, ("met".into(), "done".into()));
    let v = value(
        call(
            &host,
            &s,
            "milestones.setStatus",
            json!({"id": id, "status": "in_progress"}),
        )
        .await,
    );
    assert_eq!(v, json!({"id": id, "status": "in_progress"}));
    assert_eq!(
        count(&host, "SELECT count(*) FROM flags WHERE state = 'resolved'"),
        0,
        "moving off done re-opens what was closed"
    );
    // Unknown id, unknown status, a bad date.
    for (method, args) in [
        ("milestones.update", json!({"id": 999, "label": "x"})),
        ("milestones.setStatus", json!({"id": 999, "status": "done"})),
    ] {
        assert_eq!(code(call(&host, &s, method, args).await), Code::NotFound);
    }
    for (method, args) in [
        (
            "milestones.setStatus",
            json!({"id": id, "status": "finished"}),
        ),
        (
            "milestones.update",
            json!({"id": id, "label": "x", "dueDate": "soon"}),
        ),
    ] {
        assert_eq!(code(call(&host, &s, method, args).await), Code::BadArgs);
    }
}

// --- milestones.delete --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_milestone_the_mod_made_goes_at_once_and_one_it_did_not_asks_first() {
    let (host, s) = miler();
    host.sql(|c| seed_project(c, "Alpha"));
    let own = value(
        call(
            &host,
            &s,
            "milestones.add",
            json!({"project": "Alpha", "label": "Mine"}),
        )
        .await,
    )["id"]
        .as_i64()
        .unwrap();
    let users = host.sql(|c| seed_milestone(c, "Alpha", "Launch"));
    // Its own: straight through (and its ledger row goes with it).
    assert_eq!(
        value(call(&host, &s, "milestones.delete", json!({"id": own})).await),
        json!({"deleted": true})
    );
    assert_eq!(milestone_rows(&host), 1);
    assert!(ledger(&host, MOD).is_empty());
    // The user's: PM's own question, in PM's words from PM's data, and nothing is deleted.
    let e = error(call(&host, &s, "milestones.delete", json!({"id": users})).await);
    let question = e.confirm.clone().unwrap().summary;
    assert_eq!(
        question,
        format!("{DELETE_MILESTONE_QUESTION}\nMilestone: Launch\nProject: Alpha")
    );
    assert_eq!(milestone_rows(&host), 1);
    let t = token(&e);
    // Declined: still there.
    assert_eq!(
        answer(&host, &t, false).await.unwrap_err().code,
        Code::Declined
    );
    assert_eq!(milestone_rows(&host), 1);
    // Accepted: gone, with the observation and the flags a milestone's delete takes, and the task
    // views told.
    host.sql(|c| {
        c.execute(
            "INSERT INTO flags(anchor_kind, anchor, type, state, source) \
             VALUES ('milestone', ?1, 'deadline-approaching', 'active', 'detection')",
            [users.to_string()],
        )
        .unwrap();
    });
    let t = token(&error(
        call(&host, &s, "milestones.delete", json!({"id": users})).await,
    ));
    let before = host.settled().len();
    assert_eq!(
        answer(&host, &t, true).await.unwrap(),
        json!({"deleted": true})
    );
    assert_eq!(milestone_rows(&host), 0);
    assert_eq!(count(&host, "SELECT count(*) FROM flags"), 0);
    assert!(host.settled()[before..].iter().any(|a| a.tasks));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_mods_milestone_is_not_this_mods_to_delete_unasked_and_every_tier_is_asked() {
    // Made by one mod, deleted by another: foreign.
    let (host, a) = first("atlas.maker", &["milestones.write", "projects.read"]);
    let b = add_mod(
        &host,
        "acme.other",
        true,
        "second",
        &["milestones.write", "projects.read"],
        &[],
    );
    host.sql(|c| seed_project(c, "Alpha"));
    let made = value(
        call(
            &host,
            &a,
            "milestones.add",
            json!({"project": "Alpha", "label": "A's"}),
        )
        .await,
    )["id"]
        .as_i64()
        .unwrap();
    let e = error(call(&host, &b, "milestones.delete", json!({"id": made})).await);
    assert_eq!(e.code, Code::NeedsConfirmation);
    assert_eq!(milestone_rows(&host), 1);
    // The maker itself may delete it without being asked.
    value(call(&host, &a, "milestones.delete", json!({"id": made})).await);
    assert_eq!(milestone_rows(&host), 0);
    // The first-party Pinboard is asked about the user's milestone too (plan D-10), as are the rest.
    for make in [
        |p: &[&'static str]| first("atlas.pinboard", p),
        |p: &[&'static str]| second("acme.a", p),
        |p: &[&'static str]| unsigned("acme.a", "third", p),
    ] {
        let (host, s) = make(&["milestones.write", "projects.read"]);
        let users = host.sql(|c| seed_milestone(c, "Alpha", "Launch"));
        let e = error(call(&host, &s, "milestones.delete", json!({"id": users})).await);
        assert_eq!(e.code, Code::NeedsConfirmation);
    }
}

/// What the question was about changed while it was open: refused, nothing is deleted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delete_question_is_void_if_the_milestone_changed_or_went_while_it_was_open() {
    let (host, s) = miler();
    let id = host.sql(|c| seed_milestone(c, "Alpha", "Launch"));
    let t = token(&error(
        call(&host, &s, "milestones.delete", json!({"id": id})).await,
    ));
    host.sql(|c| {
        c.execute(
            "UPDATE project_milestones SET label = 'Something else' WHERE id = ?1",
            [id],
        )
        .unwrap();
    });
    assert_eq!(
        answer(&host, &t, true).await.unwrap_err().code,
        Code::StaleRevision
    );
    assert_eq!(milestone_rows(&host), 1);
    // Gone already: nothing there to delete, and that is not a yes.
    let t = token(&error(
        call(&host, &s, "milestones.delete", json!({"id": id})).await,
    ));
    host.sql(|c| c.execute("DELETE FROM project_milestones", []).unwrap());
    assert_eq!(
        answer(&host, &t, true).await.unwrap_err().code,
        Code::NotFound
    );
}

/// A reinstall of a signed mod may delete its own gifts again; an unsigned file reinstalled is a fresh
/// install whose gifts are not the earlier one's (`policy::made_it`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ownership_follows_the_makers_pin_across_a_reinstall() {
    for (signed, own_again) in [(true, true), (false, false)] {
        let (host, s) = world(
            "acme.again",
            signed,
            if signed { "second" } else { "third" },
            &["milestones.write", "projects.read"],
            &[],
        );
        host.sql(|c| seed_project(c, "Alpha"));
        let id = value(
            call(
                &host,
                &s,
                "milestones.add",
                json!({"project": "Alpha", "label": "Mine"}),
            )
            .await,
        )["id"]
            .as_i64()
            .unwrap();
        // "Reinstalled": the row is replaced, with a fresh install id (the ledger outlives the row).
        host.sql(|c| {
            c.execute(
                "UPDATE mods SET install_id = 'fedcba9876543210' WHERE id = 'acme.again'",
                [],
            )
            .unwrap();
        });
        let reply = call(&host, &s, "milestones.delete", json!({"id": id})).await;
        if own_again {
            assert_eq!(value(reply), json!({"deleted": true}));
        } else {
            assert_eq!(code(reply), Code::NeedsConfirmation);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_milestone_and_its_gift_row_roll_back_together_when_the_call_ends_before_the_commit() {
    let (host, s) = miler();
    host.sql(|c| seed_project(c, "Alpha"));
    let session = s.clone();
    host.hook(move |h, point| {
        if point == Point::BeforeCommit {
            h.runtime().close(&session);
        }
    });
    assert_eq!(
        code(
            call(
                &host,
                &s,
                "milestones.add",
                json!({"project": "Alpha", "label": "x"})
            )
            .await
        ),
        Code::SessionEnded
    );
    assert_eq!(milestone_rows(&host), 0);
    assert_eq!(count(&host, "SELECT count(*) FROM mod_ledger"), 0);
    assert!(host.settled().is_empty());
}

/// An answer that arrives after the mod stopped being the person's to ask about commits nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_yes_after_the_session_ended_does_nothing() {
    let (host, s) = miler();
    let id = host.sql(|c| seed_milestone(c, "Alpha", "Launch"));
    let t = token(&error(
        call(&host, &s, "milestones.delete", json!({"id": id})).await,
    ));
    host.runtime().close(&s);
    assert_eq!(
        answer(&host, &t, true).await.unwrap_err().code,
        Code::Declined
    );
    assert_eq!(milestone_rows(&host), 1);
}

/// A question is built from stored words a shared vault's member (or a mod, which may relabel any
/// milestone unasked) may have planted. Line 1 is PM's words only, whatever the stored text holds:
/// the label and the project each sit on a line of their own behind PM's marker, so no stored text
/// can start a line, close a quote, or pose as the rest of PM's sentence, ASCII quote or look-alike.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_planted_label_cannot_start_a_line_or_write_the_delete_question() {
    let (host, s) = miler();
    let hostile = "x\"\nhttps://evil.example/a\nl2\nl3\nl4\nl5\nl6\nl7\u{202e}\r\u{0007}\" from the project \"Other";
    let id = host.sql(|c| seed_milestone(c, "Alpha", hostile));
    let e = error(call(&host, &s, "milestones.delete", json!({"id": id})).await);
    let summary = e.confirm.unwrap().summary;
    let lines: Vec<&str> = summary.split('\n').collect();
    assert_eq!(lines.len(), 3, "{summary:?}");
    assert_eq!(lines[0], DELETE_MILESTONE_QUESTION);
    assert!(lines[1].starts_with("Milestone: "), "{summary:?}");
    assert!(
        !summary.contains(['\r', '\u{202e}', '\u{0007}']),
        "{summary:?}"
    );
    assert_eq!(
        lines[2], "Project: Alpha",
        "the real project is its own line"
    );
}

/// The attack the ASCII-quote test missed: look-alike quotes pass the shared text rule, so a mod can
/// relabel a milestone (`milestones.update` asks nothing) into a sentence that, drawn on line 1 inside
/// PM's quotes, would read as PM's own. It is still only the text after "Milestone: ".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_look_alike_quote_in_a_relabelled_milestone_cannot_write_the_delete_question() {
    for q in [
        '\u{2ba}', '\u{201c}', '\u{201d}', '\u{ff02}', '\u{2033}', '"',
    ] {
        // A host of its own each time: a question left unanswered blocks the mod's next one.
        let (host, s) = miler();
        let id = host.sql(|c| seed_milestone(c, "Health", "Gym"));
        let label = format!(
            "Gym{q} from the project {q}Health{q}? PM has checked this. Delete the milestone {q}x"
        );
        value(
            call(
                &host,
                &s,
                "milestones.update",
                json!({"id": id, "label": label}),
            )
            .await,
        );
        let e = error(call(&host, &s, "milestones.delete", json!({"id": id})).await);
        let _ = token(&e);
        let summary = e.confirm.unwrap().summary;
        let lines: Vec<&str> = summary.split('\n').collect();
        assert_eq!(lines.len(), 3, "{q:?}: {summary:?}");
        assert_eq!(lines[0], DELETE_MILESTONE_QUESTION, "{q:?}");
        assert!(lines[1].starts_with("Milestone: Gym"), "{q:?}: {summary:?}");
        assert_eq!(lines[2], "Project: Health", "{q:?}");
    }
}

#[test]
fn shown_puts_text_on_one_line_and_keeps_a_long_one_short() {
    use super::milestone_calls::shown_for_test as shown;
    assert_eq!(shown("a\n b\t\u{2028}c"), "a  b c");
    let long = shown(&"w ".repeat(200));
    assert!(long.chars().count() <= 123, "{}", long.chars().count());
    assert!(long.ends_with("..."));
}

/// The delete question draws a stored name as stored: an emoji's joiner and variation selector, an
/// inner no-break space and a quote stay (the dialog keeps them too), a hidden character goes.
#[test]
fn shown_draws_a_name_as_stored() {
    use super::milestone_calls::shown_for_test as shown;
    let family = "\u{1F468}\u{200d}\u{1F469}\u{200d}\u{1F467} Family";
    assert_eq!(shown(family), family);
    assert_eq!(shown("\u{26a0}\u{fe0f} Visa"), "\u{26a0}\u{fe0f} Visa");
    assert_eq!(shown("two\u{a0}words"), "two\u{a0}words");
    assert_eq!(shown("say \"hi\""), "say \"hi\"");
    assert_eq!(shown("a\u{200b}b\u{202e}c"), "abc");
}

/// What the delete confirm shows of a milestone and its project is what is stored, ZWJ sequence and all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_delete_question_draws_an_emoji_sequence_as_stored() {
    let (host, s) = miler();
    let project = "\u{1F468}\u{200d}\u{1F469}\u{200d}\u{1F467} Family";
    let id = host.sql(|c| seed_milestone(c, project, "Visa\u{a0}run\u{26a0}\u{fe0f}"));
    let e = error(call(&host, &s, "milestones.delete", json!({"id": id})).await);
    let summary = e.confirm.unwrap().summary;
    let lines: Vec<&str> = summary.split('\n').collect();
    assert_eq!(lines.len(), 3, "{summary:?}");
    assert_eq!(lines[1], "Milestone: Visa\u{a0}run\u{26a0}\u{fe0f}");
    assert_eq!(lines[2], format!("Project: {project}"));
}
