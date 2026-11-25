// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The milestone calls (PR C, K2): `milestones.list`, `milestones.add`, `milestones.update`,
//! `milestones.setStatus` and `milestones.delete`.
//!
//! **One service layer.** Every write goes through `milestones::ops`, the layer the page's own
//! commands use, so a mod's change has every side effect the user's has (the project is touched, the
//! activity logged, the briefing nudged, the flags a "done" had closed re-opened when a milestone
//! leaves done, the task views told to re-read). A copy here would silently skip the next step
//! somebody adds there. What the write leaves for after the guard rides back as an `Aftermath`.
//!
//! **Milestones are read with `projects.read`** (they belong to projects, spec §1.5), and every call
//! that names one also needs it (the second-READ rule), so a write-only mod cannot probe ids. Which
//! milestones a mod may touch: it may add its own (gifts, each with its ledger row in the SAME
//! transaction), edit or set the status of any, and delete its own straight away; a milestone it did
//! not make goes through PM's own question first (all tiers, the first-party Pinboard too).
//!
//! **A project is never made by a write that merely names it.** A mod that names a project that does
//! not exist asks to make one: PM asks the person first (Bobby 2026-11-18: "Create project X?"), and on a
//! yes the project and the milestone are made in one transaction. The project is then the person's
//! own: it carries no maker label (the ledger names tasks and milestones only) and an uninstall never
//! offers to remove it.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use super::calls::{self, internal, refusal, Wrote};
use super::confirm::{self, Target};
use super::gifts::{self, Gift};
use super::handlers::parse;
use super::permissions::{confirm_each_time, ConfirmKind};
use super::projection::{Dto, Needs, Projection};
use super::second_read;
use super::{limits, Authorised, Code, Host, ModError, Reply};
use crate::milestones::{self, ops, Milestone};
use crate::mods::confirm::Action;
use crate::mods::ids::ModId;
use crate::mods::package::{tidy_user_line, user_line_fault};

/// The longest a project name a mod asks to create is, in characters: PM's question shows all of it.
const NEW_PROJECT_MAX: usize = 200;
/// How much of a stored label or project name PM's question shows.
const SHOWN_MAX: usize = 120;

fn no_such_milestone() -> ModError {
    ModError::new(Code::NotFound, "There is no such milestone.")
}

// --- milestones.list ----------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    #[serde(default)]
    after: Option<i64>,
    #[serde(default)]
    limit: Option<usize>,
}

/// A milestone as a mod with this `projection` gets it. The date is the resolved one (a linked
/// milestone's date follows its event), which names no event; whether a link exists is always said
/// (`calendar_linked`) and the event's uid, and whether the event is still in the calendar, are
/// `calendar.read`'s. Never the source or external id of a synced one.
pub(super) fn milestone_dto(m: &Milestone, projection: Projection) -> Value {
    Dto::new(projection)
        .field(Needs::Nothing, "id", m.id)
        .field(Needs::Nothing, "project_name", m.project_name.as_str())
        .field(Needs::Nothing, "label", m.label.as_str())
        .field(Needs::Nothing, "due_date", m.due_date.clone())
        .field(Needs::Nothing, "calendar_linked", m.calendar_linked)
        .field(Needs::Nothing, "state", m.state.clone())
        .field(Needs::Nothing, "status", m.status.clone())
        .field(Needs::Nothing, "sort_order", m.sort_order)
        .field(Needs::Calendar, "event_uid", m.event_uid.clone())
        .field(Needs::Calendar, "event_missing", m.event_missing)
        .build()
}

/// A page of `all` (by id) after `after`, under `limit` and `budget`; the cursor is the last id when
/// more follow.
fn list_page(
    all: &[Milestone],
    after: i64,
    limit: usize,
    budget: usize,
    projection: Projection,
) -> (Vec<Value>, Option<i64>) {
    let mut page: Vec<Value> = Vec::new();
    let mut used = 0usize;
    let mut last = None;
    for m in all.iter().filter(|m| m.id > after) {
        let value = milestone_dto(m, projection);
        let size = value.to_string().len() + 1;
        if page.len() == limit || (!page.is_empty() && used + size > budget) {
            return (page, last);
        }
        used += size;
        last = Some(m.id);
        page.push(value);
    }
    (page, None)
}

/// `milestones.list({after?, limit?})` → `{milestones, next}`.
pub(super) async fn list<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let ListArgs { after, limit } = parse(args)?;
    let after = after.unwrap_or(0);
    if after < 0 {
        return Err(ModError::bad_args("after is a milestone id: 0 or more."));
    }
    let limit = limit.unwrap_or(limits::MAX_TASKS_PER_CALL);
    if limit == 0 || limit > limits::MAX_TASKS_PER_CALL {
        return Err(ModError::bad_args(format!(
            "limit is 1 to {}.",
            limits::MAX_TASKS_PER_CALL
        )));
    }
    let value = calls::read(host, auth, move |cx| {
        let projection = Projection::of_grants(cx.conn, cx.facts, cx.host.engine_sealed())?;
        let today = crate::clock::today_sql_in(crate::commands::resolve_zone(cx.conn));
        let mut all = milestones::list_all(cx.conn, &today).map_err(internal)?;
        all.sort_by_key(|m| m.id);
        let (page, next) = list_page(
            &all,
            after,
            limit,
            limits::LIST_PAGE_BUDGET_BYTES,
            projection,
        );
        Ok(json!({"milestones": page, "next": next}))
    })
    .await?;
    Ok(Reply::value(value))
}

// --- milestones.add -----------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct AddArgs {
    project: String,
    label: String,
    #[serde(default)]
    due_date: Option<String>,
}

/// The name of a project a mod asks to create: trimmed, one line of the person's words (an emoji is
/// fine), at most [`NEW_PROJECT_MAX`] characters. PM's question shows all of it.
fn new_project_name(typed: &str) -> Result<String, ModError> {
    let name = typed.trim();
    user_line_fault(name, NEW_PROJECT_MAX, false).map_err(|_| {
        ModError::bad_args(format!(
            "A new project's name is one plain line of at most {NEW_PROJECT_MAX} characters."
        ))
    })?;
    Ok(name.to_owned())
}

/// Make `project` and one milestone in it, as the mod's gift: the project's row, the milestone, the
/// project's touch and activity, and the ledger row, in the caller's transaction.
pub(super) fn create_with_milestone(
    conn: &Connection,
    mod_id: &ModId,
    project: &str,
    label: &str,
    due_date: Option<String>,
) -> Result<Wrote<Value>, ModError> {
    gifts::ensure_room(conn, mod_id, 1)?;
    let (id, aftermath) = ops::add_in(conn, project, label, due_date, None).map_err(internal)?;
    // `ops::add_in` made the project the way a typed one comes to exist (`projects::ensure_named`:
    // row, entity and registry row, in the milestone's transaction), so it is in PM's pickers and
    // `projects.list` and can be renamed or merged. No ledger row for it.
    gifts::record(conn, mod_id, Gift::Milestone(id))?;
    Ok(Wrote {
        value: json!({"id": id}),
        aftermath,
    })
}

/// `milestones.add({project, label, dueDate?})` → `{id}`; or, for a project that doesn't exist yet,
/// `MOD-2003` carrying PM's question.
pub(super) async fn add<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let AddArgs {
        project,
        label,
        due_date,
    } = parse(args)?;
    let (label, due_date) =
        milestones::validate_label_and_date(&label, due_date.as_deref()).map_err(refusal)?;
    let value = calls::write(host, auth, move |cx| {
        // `projects.read`, before any lookup: the answer to "does this project exist" is a read.
        second_read::require(cx.host, cx.conn, cx.facts, cx.auth)?;
        let mod_id = &cx.auth.ticket.mod_id;
        gifts::ensure_room(cx.conn, mod_id, 1)?;
        let Some(name) =
            crate::tasks::find_existing_project(cx.conn, &project).map_err(internal)?
        else {
            let name = new_project_name(&project)?;
            if confirm_each_time(cx.facts.tier, ConfirmKind::CreateProject) {
                return Err(confirm::request(
                    cx.host,
                    cx.conn,
                    cx.auth,
                    Action::CreateProject {
                        project: name,
                        label,
                        due_date,
                    },
                ));
            }
            return create_with_milestone(cx.conn, mod_id, &name, &label, due_date);
        };
        let (id, aftermath) =
            ops::add_in(cx.conn, &name, &label, due_date, None).map_err(internal)?;
        gifts::record(cx.conn, mod_id, Gift::Milestone(id))?;
        Ok(Wrote {
            value: json!({"id": id}),
            aftermath,
        })
    })
    .await?;
    Ok(Reply::value(value))
}

// --- milestones.update / setStatus --------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct UpdateArgs {
    id: i64,
    label: String,
    #[serde(default)]
    due_date: Option<String>,
}

/// Whether milestone `id` exists, answered `NotFound` when not (after the second reads).
fn require_milestone(conn: &Connection, id: i64) -> Result<(), ModError> {
    milestones::project_of(conn, id)
        .map_err(internal)?
        .map(|_| ())
        .ok_or_else(no_such_milestone)
}

/// `milestones.update({id, label, dueDate?})` → `{id}`: any milestone, as the page's own edit does
/// (a calendar-linked one keeps its calendar's date). A blank label is named "deadline".
pub(super) async fn update<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let UpdateArgs {
        id,
        label,
        due_date,
    } = parse(args)?;
    let (label, due_date) =
        milestones::validate_label_and_date(&label, due_date.as_deref()).map_err(refusal)?;
    let value = calls::write(host, auth, move |cx| {
        second_read::require(cx.host, cx.conn, cx.facts, cx.auth)?;
        require_milestone(cx.conn, id)?;
        let aftermath = ops::update(cx.conn, id, &label, due_date).map_err(internal)?;
        Ok(Wrote {
            value: json!({"id": id}),
            aftermath,
        })
    })
    .await?;
    Ok(Reply::value(value))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusArgs {
    id: i64,
    status: String,
}

/// `milestones.setStatus({id, status})` → `{id, status}`, status one of `not_started`, `in_progress`,
/// `almost_done`, `done`: the page's own four levels, through the same service (moving off `done`
/// re-opens the flags the person closed on it).
pub(super) async fn set_status<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let StatusArgs { id, status } = parse(args)?;
    let status: &'static str = milestones::STATUSES
        .into_iter()
        .find(|s| *s == status)
        .ok_or_else(|| {
            ModError::bad_args(
                "A milestone's status is not_started, in_progress, almost_done or done.",
            )
        })?;
    let value = calls::write(host, auth, move |cx| {
        second_read::require(cx.host, cx.conn, cx.facts, cx.auth)?;
        require_milestone(cx.conn, id)?;
        let aftermath = ops::set_status(cx.conn, id, status).map_err(internal)?;
        Ok(Wrote {
            value: json!({"id": id, "status": status}),
            aftermath,
        })
    })
    .await?;
    Ok(Reply::value(value))
}

// --- milestones.delete --------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteArgs {
    id: i64,
}

/// `milestones.delete({id})` → `{deleted: true}`. One the mod made goes at once; any other asks the
/// person first (`MOD-2003`), because a mod may edit anything of the user's but not remove it
/// unasked.
pub(super) async fn delete<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let DeleteArgs { id } = parse(args)?;
    let value = calls::write(host, auth, move |cx| {
        second_read::require(cx.host, cx.conn, cx.facts, cx.auth)?;
        require_milestone(cx.conn, id)?;
        let own = gifts::made_milestone(cx.conn, &cx.auth.ticket.mod_id, id)?;
        if !own && confirm_each_time(cx.facts.tier, ConfirmKind::ForeignMilestoneDelete) {
            return Err(confirm::request(
                cx.host,
                cx.conn,
                cx.auth,
                Action::DeleteMilestone { id },
            ));
        }
        let aftermath = ops::remove_in(cx.conn, id).map_err(internal)?;
        Ok(Wrote {
            value: json!({"deleted": true}),
            aftermath,
        })
    })
    .await?;
    Ok(Reply::value(value))
}

// --- the person's yes ---------------------------------------------------------------------------

/// `text` as the delete question shows it: the shared rule for a person's own words
/// ([`tidy_user_line`]), so it is drawn as stored (an emoji's joiner and variation selectors, a
/// no-break space and a double quote stay) except that it is on ONE line (a tab or line break becomes
/// a space, a control or hidden character goes), and at most [`SHOWN_MAX`] characters, then an
/// ellipsis. Stored text is the user's own (or a shared vault member's): it must not start a new line
/// (the page draws each line after the first as the thing asked about, behind PM's own marker) or
/// reorder what follows it. No quote is rewritten: the text is never inside PM's own sentence.
fn shown(text: &str) -> String {
    let flat = tidy_user_line(text);
    if flat.chars().count() <= SHOWN_MAX {
        flat
    } else {
        let cut: String = flat.chars().take(SHOWN_MAX).collect();
        format!("{cut}...")
    }
}

#[cfg(test)]
pub(super) fn shown_for_test(text: &str) -> String {
    shown(text)
}

/// The question line PM asks before a mod's call deletes a milestone it did not make. Constant: the
/// milestone and its project follow on lines of their own ([`delete_question`]).
pub(super) const DELETE_MILESTONE_QUESTION: &str = "Delete this milestone?";

/// What PM asks about a milestone: its question, then the milestone's label and its project's name,
/// each on its own line behind a marker (the page draws the lines after the first as the thing asked
/// about). Both go through [`shown`], so each is one line and clamped, and the marker keeps a line
/// whose text is empty from being dropped; no stored text is on the question line.
fn delete_question(label: &str, project: &str) -> String {
    format!(
        "{DELETE_MILESTONE_QUESTION}\nMilestone: {}\nProject: {}",
        shown(label),
        shown(project)
    )
}

/// The milestone a delete question is about, as PM reads it now: everything its words and the
/// snapshot depend on. `NotFound` when it is gone.
pub(super) fn read_delete(conn: &Connection, id: i64) -> Result<Target, ModError> {
    type Stored = (
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let row: Option<Stored> = conn
        .query_row(
            "SELECT project_name, label, due_date, status, event_uid \
             FROM project_milestones WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .map_err(ModError::internal)?;
    let (project, label, due, status, event) = row.ok_or_else(no_such_milestone)?;
    Ok(Target {
        facts: format!("{id}|{project}|{label}|{due:?}|{status:?}|{event:?}"),
        // Line 1 is PM's words only. The stored label and project (which a mod may have just relabelled,
        // or a vault member planted) each sit on a line of their own after it, marked as which is which
        // by a prefix PM writes: `shown` keeps each to one line, so neither can start a line that
        // imitates PM's question or the other's marker, and a look-alike quote can't pose as PM's own.
        summary: delete_question(&label, &project),
    })
}

/// Delete the milestone the person agreed to delete, in the caller's transaction.
pub(super) fn do_delete(conn: &Connection, id: i64) -> Result<Wrote<Value>, ModError> {
    let aftermath = ops::remove_in(conn, id).map_err(internal)?;
    Ok(Wrote {
        value: json!({"deleted": true}),
        aftermath,
    })
}

/// The question line PM asks before a mod's call makes a new project; the name follows on its own line.
pub(super) const CREATE_PROJECT_QUESTION: &str = "Create a new project with this name?";

/// The project a create question is about: it must still not exist, or the question no longer
/// describes what would happen.
pub(super) fn read_create(
    conn: &Connection,
    project: &str,
    label: &str,
    due_date: &Option<String>,
) -> Result<Target, ModError> {
    let exists = crate::tasks::find_existing_project(conn, project)
        .map_err(internal)?
        .is_some();
    Ok(Target {
        facts: format!(
            "{project}|{label}|{due_date:?}|{}",
            if exists { "exists" } else { "absent" }
        ),
        // The whole name, never a cut one: what the person is shown is what is made. It is at most
        // NEW_PROJECT_MAX characters and one plain line (`new_project_name`). It sits on its own line,
        // which the dialog draws apart from the question as the thing asked about, so the question line
        // is PM's words only: a name holding a double quote can't close a quote around it and pose as
        // the rest of PM's sentence, and the name is never rewritten to stop it.
        summary: format!("{CREATE_PROJECT_QUESTION}\n{project}"),
    })
}
