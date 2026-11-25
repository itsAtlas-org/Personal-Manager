// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The task calls (PR C, K2): `tasks.list`, `tasks.add`, `tasks.setStatus`, `tasks.reorder`,
//! `tasks.importChecklist` and `ui.openTaskEditor`.
//!
//! **What a task is to a mod.** The same rows the Tasks panel and the Runway read (`crate::tasks`,
//! the one owner of the tables), through the same writers (`add_in`, `change_status`, `reorder_in`,
//! `import_checklist_in`), so a mod's task is indistinguishable from the user's except that it has a
//! `mod_ledger` row naming its maker (written in the SAME transaction as the task, so the two exist
//! together or not at all) and counts against the mod's quota (`gifts`).
//!
//! **What a task tells a mod depends on its grants** ([`projection`]): without `projects.read` the
//! project's name, the linked milestone and the project's archive flag are not in the DTO; without
//! `calendar.read`, the linked event's uid and title are not. A date alone (`effective_deadline`,
//! `deadline_from`, `deadline_missing`) is never withheld: it names no event. **A field that is
//! dropped must never read as a link that was removed:** core's own readers take a null
//! `milestone_id` or `event_uid` for "the link was removed" (`lib/tasks.ts`), so the DTO always says
//! whether a link exists (`milestone_linked`, `event_linked`) and the kit shows "Event's date", not
//! "removed", for a link it may not look through.
//!
//! **Paged** by cursor under the same byte budget as `documents.list`: the cursor is the last task's
//! place in the one global order (`<ordinal>:<id>`), so a task landing or going between two pages
//! cannot repeat or skip another. The stacks come with the first page only.
//!
//! **The editor** (`ui.openTaskEditor`) is PM's own, drawn by the page (core's `TaskEditor`): Rust
//! checks the task exists (or that a new task's prefill is plain text and a real date) and the page
//! opens it, answering at once; the editor then stays open on its own, and the frame re-reads on
//! `tasks.changed`. One editor at a time is the page's to keep, since only it knows when one is open.
//! A task made in that editor is the person's own, never the mod's gift.

use std::collections::HashSet;

use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

use super::calls::{self, existing_project, internal, refusal, tasks_changed, Ctx};
use super::gifts::{self, Gift};
use super::handlers::parse;
use super::projection::{Dto, Needs, Projection};
use super::second_read;
use super::{grants, limits, Authorised, Code, Host, ModError, PageAction, Reply};
use crate::mods::package::user_line_fault;
use crate::tasks::{self, ChecklistItem, Task, TaskFields, TaskStack};

fn no_such_task() -> ModError {
    ModError::new(Code::NotFound, "There is no such task.")
}

/// The longest a prefilled project name is: it is text for a field the person edits.
const PREFILL_PROJECT_MAX: usize = 200;

// --- tasks.list ---------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    after: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

/// A task as a mod with this `projection` gets it, field by field from the named columns.
pub(super) fn task_dto(t: &Task, projection: Projection) -> Value {
    Dto::new(projection)
        .field(Needs::Nothing, "id", t.id)
        .field(Needs::Nothing, "title", t.title.as_str())
        .field(Needs::Nothing, "why", t.why.clone())
        .field(Needs::Nothing, "status", t.status.as_str())
        .field(Needs::Nothing, "completed_at", t.completed_at.clone())
        .field(Needs::Nothing, "lane", t.lane.as_str())
        .field(Needs::Nothing, "stack_id", t.stack_id)
        .field(Needs::Nothing, "parked", t.parked)
        .field(Needs::Nothing, "ordinal", t.ordinal)
        .field(Needs::Nothing, "source", t.source.as_str())
        .field(Needs::Nothing, "created_at", t.created_at.as_str())
        .field(Needs::Nothing, "updated_at", t.updated_at.as_str())
        .field(Needs::Nothing, "do_start", t.do_start.clone())
        .field(Needs::Nothing, "do_end", t.do_end.clone())
        // The task's own date and the date its link resolved to: dates alone, never what the event is.
        .field(Needs::Nothing, "deadline", t.deadline.clone())
        .field(Needs::Nothing, "deadline_kind", t.deadline_kind.as_str())
        .field(Needs::Nothing, "deadline_from", t.deadline_from.as_str())
        .field(
            Needs::Nothing,
            "effective_deadline",
            t.effective_deadline.clone(),
        )
        .field(Needs::Nothing, "deadline_missing", t.deadline_missing)
        // Whether a link exists is always said, so a link a mod may not look through never reads as
        // one that was removed.
        .field(Needs::Nothing, "milestone_linked", t.milestone_id.is_some())
        .field(Needs::Nothing, "event_linked", t.event_uid.is_some())
        .field(Needs::Projects, "project_name", t.project_name.clone())
        .field(Needs::Projects, "project_archived", t.project_archived)
        .field(Needs::Projects, "milestone_id", t.milestone_id)
        .field(
            Needs::Projects,
            "milestone_label",
            t.milestone_label.clone(),
        )
        .field(Needs::Calendar, "event_uid", t.event_uid.clone())
        .field(Needs::Calendar, "event_title", t.event_title.clone())
        .build()
}

fn stack_dto(s: &TaskStack) -> Value {
    Dto::new(Projection::NONE)
        .field(Needs::Nothing, "id", s.id)
        .field(Needs::Nothing, "title", s.title.as_str())
        .field(Needs::Nothing, "kind", s.kind.as_str())
        .field(Needs::Nothing, "note", s.note.clone())
        .field(Needs::Nothing, "created_at", s.created_at.as_str())
        .field(Needs::Nothing, "updated_at", s.updated_at.as_str())
        .build()
}

/// A task's place in the global order, as a cursor.
fn cursor_of(t: &Task) -> String {
    format!("{}:{}", t.ordinal, t.id)
}

fn parse_cursor(raw: &str) -> Result<(i64, i64), ModError> {
    let bad = || ModError::bad_args("after is the cursor a previous page gave.");
    let (ordinal, id) = raw.split_once(':').ok_or_else(bad)?;
    Ok((
        ordinal.parse().map_err(|_| bad())?,
        id.parse().map_err(|_| bad())?,
    ))
}

/// A page of `tasks` (already in the global order) after `after`: at most `limit`, ending early
/// under `budget` bytes of JSON (a page's first task always goes in, so the list always advances).
/// The cursor is the last task returned when more follow, else `None`.
pub(super) fn list_page(
    tasks: &[Task],
    after: Option<(i64, i64)>,
    limit: usize,
    budget: usize,
    projection: Projection,
) -> (Vec<Value>, Option<String>) {
    let rest = tasks
        .iter()
        .filter(|t| after.is_none_or(|a| (t.ordinal, t.id) > a));
    let mut page: Vec<Value> = Vec::new();
    let mut used = 0usize;
    let mut last: Option<&Task> = None;
    for task in rest {
        let value = task_dto(task, projection);
        let size = value.to_string().len() + 1;
        if page.len() == limit || (!page.is_empty() && used + size > budget) {
            return (page, last.map(cursor_of));
        }
        used += size;
        last = Some(task);
        page.push(value);
    }
    (page, None)
}

/// `tasks.list({project?, after?, limit?})` → `{tasks, stacks, next}`.
pub(super) async fn list<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let ListArgs {
        project,
        after,
        limit,
    } = parse(args)?;
    let after = after.as_deref().map(parse_cursor).transpose()?;
    let limit = limit.unwrap_or(limits::MAX_TASKS_PER_CALL);
    if limit == 0 || limit > limits::MAX_TASKS_PER_CALL {
        return Err(ModError::bad_args(format!(
            "limit is 1 to {}.",
            limits::MAX_TASKS_PER_CALL
        )));
    }
    let project = project
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty());
    let value = calls::read(host, auth, move |cx| {
        let sealed = cx.host.engine_sealed();
        // Naming a project is reading about projects: a mod without `projects.read` could otherwise
        // ask which names have tasks.
        if project.is_some() {
            grants::check(cx.conn, cx.facts, "projects.read", sealed)?;
        }
        let projection = Projection::of_grants(cx.conn, cx.facts, sealed)?;
        let today = crate::clock::today_sql_in(crate::commands::resolve_zone(cx.conn));
        let name = match &project {
            Some(typed) => Some(tasks::canonical_project(cx.conn, typed).map_err(internal)?),
            None => None,
        };
        let all = tasks::list(cx.conn, name.as_deref(), &today).map_err(internal)?;
        let stacks = if after.is_none() {
            tasks::list_stacks(cx.conn).map_err(internal)?
        } else {
            Vec::new()
        };
        let (tasks, next) = list_page(
            &all,
            after,
            limit,
            limits::LIST_PAGE_BUDGET_BYTES,
            projection,
        );
        Ok(json!({
            "tasks": tasks,
            "stacks": stacks.iter().map(stack_dto).collect::<Vec<_>>(),
            "next": next,
        }))
    })
    .await?;
    Ok(Reply::value(value))
}

// --- tasks.add ----------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct AddArgs {
    title: String,
    #[serde(default)]
    why: Option<String>,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    deadline: Option<String>,
    #[serde(default)]
    deadline_kind: Option<String>,
}

fn named(project: Option<String>) -> Option<String> {
    project
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
}

/// `tasks.add({title, why?, project?, deadline?, deadlineKind?})` → `{id, status}`.
pub(super) async fn add<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let AddArgs {
        title,
        why,
        project,
        deadline,
        deadline_kind,
    } = parse(args)?;
    // The closed set is checked here, so the core's own refusal (which would name the value it was
    // sent) is never what the mod reads.
    let deadline_kind = match deadline_kind.as_deref() {
        None => "hard",
        Some(kind) => tasks::DEADLINE_KINDS
            .into_iter()
            .find(|k| *k == kind)
            .ok_or_else(|| ModError::bad_args("A deadline kind is hard or soft."))?,
    };
    let fields = TaskFields {
        title,
        why,
        project: None,
        milestone_id: None,
        event_uid: None,
        do_start: None,
        do_end: None,
        deadline,
        deadline_kind: deadline_kind.to_owned(),
        deadline_from: tasks::FROM_DATE.to_owned(),
        lane: "foreground".to_owned(),
        stack_id: None,
        parked: false,
    };
    // Everything that needs no database, before the guard: the sentences name the rule, never the
    // text.
    let mut checked = tasks::validate_fields(&fields).map_err(refusal)?;
    let project = named(project);
    let value = calls::write(host, auth, move |cx| {
        second_read::require(cx.host, cx.conn, cx.facts, cx.auth)?;
        if let Some(typed) = &project {
            second_read::require_named(cx.host, cx.conn, cx.facts, cx.auth)?;
            checked.project = Some(existing_project(cx.conn, typed)?);
        }
        let mod_id = &cx.auth.ticket.mod_id;
        gifts::ensure_room(cx.conn, mod_id, 1)?;
        let id = tasks::add_in(cx.conn, &checked, tasks::SOURCE_USER).map_err(internal)?;
        gifts::record(cx.conn, mod_id, Gift::Task(id))?;
        // Engaging with a project counts as activity, as the page's own add does.
        if let Some(name) = &checked.project {
            crate::projects::touch(cx.conn, name).map_err(internal)?;
        }
        Ok(tasks_changed(
            json!({"id": id, "status": tasks::STATUS_TODO}),
        ))
    })
    .await?;
    Ok(Reply::value(value))
}

// --- tasks.setStatus ----------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusArgs {
    id: i64,
    status: String,
}

/// `tasks.setStatus({id, status})` → `{id, status}`: any task, not only the mod's own (spec §1.5).
pub(super) async fn set_status<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let StatusArgs { id, status } = parse(args)?;
    let status: &'static str = tasks::STATUSES
        .into_iter()
        .find(|s| *s == status)
        .ok_or_else(|| ModError::bad_args("A task's status is todo, doing or done."))?;
    let value = calls::write(host, auth, move |cx| {
        second_read::require(cx.host, cx.conn, cx.facts, cx.auth)?;
        if !tasks::exists(cx.conn, id).map_err(internal)? {
            return Err(no_such_task());
        }
        tasks::change_status(cx.conn, id, status).map_err(internal)?;
        if let Some(project) = tasks::project_of(cx.conn, id).map_err(internal)? {
            crate::projects::touch(cx.conn, &project).map_err(internal)?;
        }
        Ok(tasks_changed(json!({"id": id, "status": status})))
    })
    .await?;
    Ok(Reply::value(value))
}

// --- tasks.reorder ------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReorderArgs {
    ids: Vec<i64>,
}

/// `tasks.reorder({ids})` → `{reordered}`: the named tasks take each other's places, in that order;
/// every other task keeps its own. All must exist or nothing changes.
pub(super) async fn reorder<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let ReorderArgs { ids } = parse(args)?;
    if ids.len() > limits::MAX_TASKS_PER_CALL {
        return Err(ModError::bad_args(format!(
            "A reorder names at most {} tasks.",
            limits::MAX_TASKS_PER_CALL
        )));
    }
    let mut seen = HashSet::new();
    if !ids.iter().all(|id| seen.insert(*id)) {
        return Err(ModError::bad_args("A task is named twice."));
    }
    let value = calls::write(host, auth, move |cx| {
        second_read::require(cx.host, cx.conn, cx.facts, cx.auth)?;
        if ids.is_empty() {
            return Ok(calls::quiet(json!({"reordered": 0})));
        }
        let wanted = serde_json::to_string(&ids).map_err(ModError::internal)?;
        let found: i64 = cx
            .conn
            .query_row(
                "SELECT count(*) FROM tasks WHERE id IN (SELECT value FROM json_each(?1))",
                params![wanted],
                |r| r.get(0),
            )
            .map_err(ModError::internal)?;
        if usize::try_from(found).ok() != Some(ids.len()) {
            return Err(no_such_task());
        }
        tasks::reorder_in(cx.conn, &ids).map_err(internal)?;
        Ok(tasks_changed(json!({"reordered": ids.len()})))
    })
    .await?;
    Ok(Reply::value(value))
}

// --- tasks.importChecklist ----------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemArgs {
    title: String,
    done: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportArgs {
    #[serde(default)]
    project: Option<String>,
    items: Vec<ItemArgs>,
}

/// `tasks.importChecklist({project?, items})` → `{created, skipped}`: the lines of a note's checklist
/// as tasks, each one the mod's gift. Blank lines, repeats and tasks already there are skipped.
pub(super) async fn import_checklist<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let ImportArgs { project, items } = parse(args)?;
    if items.len() > limits::MAX_CHECKLIST_ITEMS {
        return Err(ModError::bad_args(format!(
            "A checklist import takes at most {} lines.",
            limits::MAX_CHECKLIST_ITEMS
        )));
    }
    let items: Vec<ChecklistItem> = items
        .into_iter()
        .map(|i| ChecklistItem {
            title: i.title,
            done: i.done,
        })
        .collect();
    let project = named(project);
    let value = calls::write(host, auth, move |cx| {
        second_read::require(cx.host, cx.conn, cx.facts, cx.auth)?;
        let project = match &project {
            Some(typed) => {
                second_read::require_named(cx.host, cx.conn, cx.facts, cx.auth)?;
                Some(existing_project(cx.conn, typed)?)
            }
            None => None,
        };
        let mod_id = &cx.auth.ticket.mod_id;
        // Checked for the whole batch: it cannot step over the quota by being counted as one.
        gifts::ensure_room(cx.conn, mod_id, items.len() as u64)?;
        let done =
            tasks::import_checklist_in(cx.conn, project.as_deref(), &items).map_err(internal)?;
        for id in &done.created {
            gifts::record(cx.conn, mod_id, Gift::Task(*id))?;
        }
        if let Some(project) = &project {
            if !done.created.is_empty() {
                crate::projects::touch(cx.conn, project).map_err(internal)?;
            }
        }
        let value = json!({"created": done.created, "skipped": done.skipped});
        Ok(if done.created.is_empty() {
            calls::quiet(value)
        } else {
            tasks_changed(value)
        })
    })
    .await?;
    Ok(Reply::value(value))
}

// --- ui.openTaskEditor --------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditorArgs {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    new: Option<NewTaskArgs>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewTaskArgs {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    deadline: Option<String>,
}

/// What the editor starts a new task from: text for its fields, never a lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Prefill {
    title: Option<String>,
    project: Option<String>,
    deadline: Option<String>,
}

impl Prefill {
    fn checked(raw: NewTaskArgs) -> Result<Self, ModError> {
        let title = match raw
            .title
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
        {
            Some(title) => Some(tasks::validate_title(&title).map_err(refusal)?),
            None => None,
        };
        let project = named(raw.project);
        if let Some(project) = &project {
            user_line_fault(project, PREFILL_PROJECT_MAX, false).map_err(|_| {
                ModError::bad_args(format!(
                    "A project name is one plain line of at most {PREFILL_PROJECT_MAX} characters."
                ))
            })?;
        }
        let deadline = tasks::validate_date("deadline", raw.deadline).map_err(refusal)?;
        Ok(Self {
            title,
            project,
            deadline,
        })
    }

    fn page_args(&self) -> Value {
        json!({"new": {"title": self.title, "project": self.project, "deadline": self.deadline}})
    }
}

/// `ui.openTaskEditor({id})` or `({new: {title?, project?, deadline?}})`: PM's page opens its own task
/// editor and the call answers at once with `{opened: true}`.
pub(super) async fn open_editor<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let EditorArgs { id, new } = parse(args)?;
    let page_args = match (id, new) {
        (Some(id), None) => {
            calls::read(host, auth, move |cx| {
                second_read::require(cx.host, cx.conn, cx.facts, cx.auth)?;
                if !tasks::exists(cx.conn, id).map_err(internal)? {
                    return Err(no_such_task());
                }
                Ok(json!({"id": id}))
            })
            .await?
        }
        (None, Some(new)) => {
            let prefill = Prefill::checked(new)?;
            calls::read(host, auth, move |cx: &Ctx<'_, H>| {
                second_read::require(cx.host, cx.conn, cx.facts, cx.auth)?;
                // A prefilled project is a name the mod was shown.
                if prefill.project.is_some() {
                    grants::check(cx.conn, cx.facts, "projects.read", cx.host.engine_sealed())?;
                }
                Ok(prefill.page_args())
            })
            .await?
        }
        _ => {
            return Err(ModError::bad_args(
                "Name a task by id, or give the new task to start from, not both.",
            ))
        }
    };
    Ok(Reply::Page {
        action: PageAction::UiOpenTaskEditor,
        args: page_args,
    })
}
