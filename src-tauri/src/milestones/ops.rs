// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The milestone operations, with every side effect a milestone change has: the ONE service layer
//! behind the IPC commands (`commands/assistant.rs`) and, later, the mod handlers.
//!
//! The row write itself is `milestones::*`. What the user's own edit also does is spread over the
//! command bodies: touch the project (editing a milestone is engaging with its project), log a
//! `milestone` observation in the project's activity, tell the briefing to look again, re-open the
//! flags a "done" had closed when a milestone leaves done, and tell the open task views to re-read
//! (a task's deadline can follow a milestone). A second copy of that list on the mod path would
//! silently skip one the day somebody adds a step to the first (the status/state pair once
//! diverged exactly that way: `flags::reopen_milestone`). So there is no second copy: both paths
//! call these.
//!
//! **What happens after the DB guard drops is returned, not done.** The task views' announcement
//! must be sent with no lock held, and the briefing nudge is a flag set on [`AppState`]; both ride
//! in an [`Aftermath`] the caller settles with [`Aftermath::finish`] once its guard is gone. That
//! keeps this module a plain `&Connection` layer, testable without a window, and `#[must_use]` on
//! the type means a caller that forgets to settle it is told so at compile time.
//!
//! `add` and `add_in` open or join a transaction; the others (like the `milestones::*` writes under
//! them) take a plain connection.

use rusqlite::Connection;

use crate::error::{Error, Result};
use crate::{briefing, flags, milestones, project_activity, projects, tasks, AppState};

/// What a milestone change leaves to do after the caller's DB guard is dropped.
#[must_use = "settle it with `finish` once the DB guard is dropped, or the briefing and the task views are not told"]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Aftermath {
    /// The facts the briefing is written from moved: [`briefing::nudge`] it.
    pub briefing: bool,
    /// A task's deadline can follow this milestone: [`tasks::announce_changed`].
    pub tasks: bool,
}

impl Aftermath {
    /// Settle it. Call with no DB guard held (the announcement reaches every window).
    pub fn finish(self, state: &AppState, app: &tauri::AppHandle) {
        if self.briefing {
            briefing::nudge(state);
        }
        if self.tasks {
            tasks::announce_changed(app);
        }
    }
}

/// A change to a milestone that exists: the briefing looks again and the task views re-read.
const CHANGED: Aftermath = Aftermath {
    briefing: true,
    tasks: true,
};

/// Bump the activity date of the project owning milestone `id` and log the observation: editing a
/// milestone counts as engaging with its project. Best-effort: an unknown id is a no-op.
pub(crate) fn touch_milestone_project(conn: &Connection, id: i64) -> Result<()> {
    if let Some(project) = milestones::project_of(conn, id)? {
        projects::touch(conn, &project)?;
        project_activity::record(conn, &project, project_activity::Kind::Milestone, Some(id));
    }
    Ok(())
}

/// The aftermath of a new milestone: a new milestone has no task following it yet, so the task views
/// have nothing to re-read.
const ADDED: Aftermath = Aftermath {
    briefing: true,
    tasks: false,
};

/// Add a milestone to a project (creating the project's metadata row if needed). A non-empty
/// `event_uid` makes it calendar-linked. Returns the new stable id.
///
/// The milestone commits in its own transaction and the project's touch and the activity row follow
/// it, as they did when the command did these itself: a failure of the touch is reported and the
/// milestone is kept, not rolled back. A caller with a row of its own to commit beside the milestone
/// wants all of it or none, and uses [`add_in`].
pub fn add(
    conn: &Connection,
    project: &str,
    label: &str,
    due_date: Option<String>,
    event_uid: Option<String>,
) -> Result<(i64, Aftermath)> {
    let project = project_name(project)?;
    let id = milestones::add(conn, project, label, due_date, event_uid)?;
    after_add(conn, project, id)?;
    Ok((id, ADDED))
}

/// [`add`] inside the caller's transaction, for a caller with a row of its own to commit beside the
/// milestone (a mod's gift ledger row): the touch and the activity row are inside it too, so a gift
/// is all there or not there at all. Refused outside a transaction. The `milestones.add` handler
/// (PR C1, K2) is its caller; tests cover it now.
#[cfg_attr(not(test), allow(dead_code))]
pub fn add_in(
    conn: &Connection,
    project: &str,
    label: &str,
    due_date: Option<String>,
    event_uid: Option<String>,
) -> Result<(i64, Aftermath)> {
    let project = project_name(project)?;
    let id = milestones::add_in(conn, project, label, due_date, event_uid)?;
    after_add(conn, project, id)?;
    Ok((id, ADDED))
}

fn project_name(project: &str) -> Result<&str> {
    let project = project.trim();
    if project.is_empty() {
        return Err(Error::Other("project name is empty".into()));
    }
    Ok(project)
}

/// Engaging with the project: its activity date moves and the milestone is logged.
fn after_add(conn: &Connection, project: &str, id: i64) -> Result<()> {
    projects::touch(conn, project)?;
    project_activity::record(conn, project, project_activity::Kind::Milestone, Some(id));
    Ok(())
}

/// Edit a milestone's label and (for a PM-native milestone) its date.
pub fn update(
    conn: &Connection,
    id: i64,
    label: &str,
    due_date: Option<String>,
) -> Result<Aftermath> {
    milestones::update(conn, id, label, due_date)?;
    touch_milestone_project(conn, id)?;
    Ok(CHANGED)
}

/// Link a milestone to a calendar event or unlink it (see [`milestones::set_event`]). A refusal is
/// in the words the user sees, and changes nothing.
pub fn set_event(
    conn: &Connection,
    id: i64,
    event_uid: Option<String>,
    cached_date: Option<String>,
    only_if_linked_to: Option<&str>,
) -> Result<Aftermath> {
    relink(conn, id, event_uid, cached_date, only_if_linked_to)?;
    Ok(CHANGED)
}

/// [`set_event`] without the aftermath.
pub(crate) fn relink(
    conn: &Connection,
    id: i64,
    event_uid: Option<String>,
    cached_date: Option<String>,
    only_if_linked_to: Option<&str>,
) -> Result<()> {
    if !milestones::set_event(conn, id, event_uid, cached_date, only_if_linked_to)? {
        let words = match milestones::project_of(conn, id)? {
            None => "That milestone no longer exists.",
            Some(_) => "That milestone changed since; nothing was moved.",
        };
        return Err(Error::Other(words.into()));
    }
    touch_milestone_project(conn, id)
}

/// Mark a milestone met or unmet.
pub fn set_state(conn: &Connection, id: i64, met: bool) -> Result<Aftermath> {
    milestones::set_state(conn, id, met)?;
    // Un-marking a milestone done is the "I made a mistake" undo: clear any flag the user asserted done
    // on it, so the next briefing refresh's detection can surface the deadline again. A completion vouched
    // done is otherwise a protected record the re-scan can't re-open. Ticking it done needs no such step —
    // detection prunes the now-met milestone's active flag on its own.
    if !met {
        flags::reopen_milestone(conn, id)?;
    }
    touch_milestone_project(conn, id)?;
    Ok(CHANGED)
}

/// Set a milestone's progress status (v42), the four-level counterpart to the met/unmet tick-box.
/// `milestones::set_status` carries `state` along, so this goes through exactly the same
/// flag-reopening step [`set_state`] does: moving OFF `done` is the same "I made a mistake" undo as
/// un-ticking the box, and must clear a user-asserted completion so detection can surface the
/// deadline again. Skipping that here would make the two controls behave differently on the same
/// transition.
pub fn set_status(conn: &Connection, id: i64, status: &str) -> Result<Aftermath> {
    milestones::set_status(conn, id, status)?;
    if status != milestones::STATUS_DONE {
        flags::reopen_milestone(conn, id)?;
    }
    touch_milestone_project(conn, id)?;
    Ok(CHANGED)
}

/// Delete a milestone by id. A task linked to it loses the link (the FK nulls it), so the task views
/// re-read.
pub fn remove(conn: &Connection, id: i64) -> Result<Aftermath> {
    // Resolve the owning project before the row is gone, then bump its activity.
    let project = milestones::project_of(conn, id)?;
    milestones::remove(conn, id)?;
    if let Some(project) = project {
        projects::touch(conn, &project)?;
        // The row is gone, but `source_ref` is a plain pointer (not an FK), so the deleted
        // milestone's id is still a valid historical reference for the observation.
        project_activity::record(conn, &project, project_activity::Kind::Milestone, Some(id));
    }
    Ok(CHANGED)
}

/// Persist a new ordering of a project's milestones (ids in display order).
pub fn reorder(conn: &Connection, project: &str, ordered_ids: &[i64]) -> Result<Aftermath> {
    let project = project.trim();
    milestones::reorder(conn, project, ordered_ids)?;
    projects::touch(conn, project)?;
    // A bulk reorder has no single milestone id, so the observation is project-level (source_ref None).
    project_activity::record(conn, project, project_activity::Kind::Milestone, None);
    Ok(Aftermath::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn store() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        (dir, conn)
    }

    fn activity_rows(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM project_activity", [], |r| r.get(0))
            .unwrap()
    }

    /// A milestone `Atlas`/"Launch" that is done, with the flag the user asserted done on it.
    fn done_with_a_resolved_flag(conn: &Connection) -> i64 {
        let (id, _) = add(conn, "Atlas", "Launch", Some("2026-12-01".into()), None).unwrap();
        let _ = set_status(conn, id, milestones::STATUS_DONE).unwrap();
        conn.execute(
            "INSERT INTO flags(anchor_kind, anchor, type, state, source) \
             VALUES ('milestone', ?1, 'deadline-approaching', 'resolved', 'assertion')",
            [id.to_string()],
        )
        .unwrap();
        id
    }

    fn resolved_flags(conn: &Connection, id: i64) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM flags WHERE anchor_kind = 'milestone' AND anchor = ?1 \
             AND state = 'resolved'",
            [id.to_string()],
            |r| r.get(0),
        )
        .unwrap()
    }

    type Entry = (
        &'static str,
        fn(&Connection, i64) -> Result<Aftermath>,
        bool,
    );

    /// ONE table for the two ways in (the IPC command and the mod handler both call these): every
    /// way a milestone can leave `done` clears the flag the user asserted done on it, and every way
    /// that keeps it done, or doesn't touch status, leaves the flag alone. The tick-box and the
    /// four-level status once diverged on exactly this.
    #[test]
    fn every_way_off_done_reopens_its_flags_and_no_other_way_does() {
        let table: [Entry; 9] = [
            (
                "status in progress",
                |c, id| set_status(c, id, "in_progress"),
                true,
            ),
            (
                "status not started",
                |c, id| set_status(c, id, "not_started"),
                true,
            ),
            (
                "status almost done",
                |c, id| set_status(c, id, "almost_done"),
                true,
            ),
            ("untick", |c, id| set_state(c, id, false), true),
            (
                "status done again",
                |c, id| set_status(c, id, "done"),
                false,
            ),
            ("tick", |c, id| set_state(c, id, true), false),
            (
                "edit",
                |c, id| update(c, id, "Renamed", Some("2026-12-02".into())),
                false,
            ),
            (
                "relink to an event",
                |c, id| set_event(c, id, Some("uid@x".into()), None, None),
                false,
            ),
            (
                "reorder",
                |c, id| {
                    let project = milestones::project_of(c, id)?.unwrap();
                    reorder(c, &project, &[id])
                },
                false,
            ),
        ];
        for (name, op, reopens) in table {
            let (_dir, conn) = store();
            let id = done_with_a_resolved_flag(&conn);
            assert_eq!(resolved_flags(&conn, id), 1, "{name}: set up");
            let _ = op(&conn, id).unwrap();
            assert_eq!(
                resolved_flags(&conn, id),
                if reopens { 0 } else { 1 },
                "{name}: the flag the user asserted done"
            );
        }
    }

    /// What each operation tells the caller to do once its guard is dropped.
    #[test]
    fn each_operation_reports_what_is_left_to_do() {
        let (_dir, conn) = store();
        let changed = Aftermath {
            briefing: true,
            tasks: true,
        };
        let (id, added) = add(&conn, "Atlas", "Launch", None, None).unwrap();
        assert_eq!(
            added,
            Aftermath {
                briefing: true,
                tasks: false
            },
            "a new milestone moves the briefing, but no task follows it yet"
        );
        assert_eq!(update(&conn, id, "L2", None).unwrap(), changed);
        assert_eq!(
            set_event(&conn, id, Some("u@x".into()), None, None).unwrap(),
            changed
        );
        assert_eq!(set_state(&conn, id, true).unwrap(), changed);
        assert_eq!(set_status(&conn, id, "in_progress").unwrap(), changed);
        assert_eq!(
            reorder(&conn, "Atlas", &[id]).unwrap(),
            Aftermath::default()
        );
        assert_eq!(remove(&conn, id).unwrap(), changed);
    }

    /// Every change is engaging with the project: its activity date moves and one `milestone`
    /// observation is logged, whichever door the change came through.
    #[test]
    fn each_operation_touches_the_project_and_logs_it() {
        let (_dir, conn) = store();
        let (id, _) = add(&conn, "Atlas", "Launch", None, None).unwrap();
        assert_eq!(activity_rows(&conn), 1, "add logs one");
        type Op<'a> = Box<dyn Fn() -> Result<Aftermath> + 'a>;
        let ops: [(&str, Op); 6] = [
            ("update", Box::new(|| update(&conn, id, "L2", None))),
            (
                "set_event",
                Box::new(|| set_event(&conn, id, Some("u@x".into()), None, None)),
            ),
            ("set_state", Box::new(|| set_state(&conn, id, true))),
            (
                "set_status",
                Box::new(|| set_status(&conn, id, "almost_done")),
            ),
            ("reorder", Box::new(|| reorder(&conn, "Atlas", &[id]))),
            ("remove", Box::new(|| remove(&conn, id))),
        ];
        for (n, (name, op)) in ops.into_iter().enumerate() {
            conn.execute("UPDATE projects SET last_touched = NULL", [])
                .unwrap();
            let _ = op().unwrap();
            assert_eq!(
                activity_rows(&conn),
                2 + n as i64,
                "{name}: one observation"
            );
            let touched: Option<String> = conn
                .query_row(
                    "SELECT last_touched FROM projects WHERE name = 'Atlas'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(
                touched.is_some(),
                "{name}: the project's activity date moved"
            );
        }
    }

    /// A refusal does nothing and promises nothing: no aftermath, no activity.
    #[test]
    fn a_refused_change_leaves_no_trace() {
        let (_dir, conn) = store();
        let (id, _) = add(&conn, "Atlas", "Launch", None, None).unwrap();
        let seen = activity_rows(&conn);
        assert!(set_status(&conn, id, "nonsense").is_err());
        assert!(set_event(&conn, 9999, Some("u@x".into()), None, None).is_err());
        assert!(add(&conn, "  ", "x", None, None).is_err());
        assert_eq!(activity_rows(&conn), seen);
    }

    /// A touch that fails after the milestone committed leaves the milestone, as the command did before
    /// the operations moved here (the milestone was written first and the touch followed, outside any
    /// transaction); only `add_in`, inside its caller's transaction, takes the milestone back with it.
    #[test]
    fn a_failed_touch_keeps_the_milestone_the_ipc_add_made() {
        let (_dir, conn) = store();
        conn.execute_batch(
            "CREATE TRIGGER no_touch BEFORE UPDATE ON projects \
             BEGIN SELECT RAISE(ABORT, 'no'); END;",
        )
        .unwrap();
        // The project must exist for the touch to be an UPDATE.
        conn.execute("INSERT INTO projects(name) VALUES ('Atlas')", [])
            .unwrap();
        let rows = |c: &Connection| -> i64 {
            c.query_row("SELECT COUNT(*) FROM project_milestones", [], |r| r.get(0))
                .unwrap()
        };
        assert!(add(&conn, "Atlas", "Launch", None, None).is_err());
        assert_eq!(
            rows(&conn),
            1,
            "the milestone was kept, and the error shown"
        );
        // Inside a caller's transaction the same failure takes the gift with it.
        let tx = conn.unchecked_transaction().unwrap();
        assert!(add_in(&tx, "Atlas", "Gift", None, None).is_err());
        tx.rollback().unwrap();
        assert_eq!(rows(&conn), 1);
    }

    /// `add_in` is `add` for a caller with a row of its own to commit beside the milestone.
    #[test]
    fn add_in_joins_its_callers_transaction_and_still_logs_the_project() {
        let (_dir, conn) = store();
        let tx = conn.unchecked_transaction().unwrap();
        let _ = add_in(&tx, "Atlas", "Launch", None, None).unwrap();
        assert_eq!(
            activity_rows(&tx),
            1,
            "logged inside the caller's transaction"
        );
        tx.rollback().unwrap();
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM project_milestones", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            (rows, activity_rows(&conn)),
            (0, 0),
            "all of it rolled back"
        );
        assert!(
            add_in(&conn, "Atlas", "x", None, None).is_err(),
            "not outside one"
        );
    }

    /// No test can build the commands' `State`, so this reads the file's own text (the precedent in
    /// `commands/assistant.rs`): the milestone commands reach the database ONLY through this module.
    /// A second copy of a side effect in a command body is the drift this layer exists to stop.
    #[test]
    fn the_milestone_commands_go_through_the_service_layer() {
        let source = include_str!("../commands/assistant.rs");
        let production = &source[..source.find("#[cfg(test)]\nmod tests").unwrap()];
        for command in [
            "add_milestone",
            "update_milestone",
            "set_milestone_event",
            "set_milestone_state",
            "set_milestone_status",
            "delete_milestone",
            "reorder_milestones",
        ] {
            let at = production
                .find(&format!("pub fn {command}("))
                .unwrap_or_else(|| panic!("{command} is defined"));
            let body = &production[at..];
            let body = &body[..body.find("\n}\n").unwrap()];
            assert!(body.contains("ops::"), "{command} calls the service layer");
            assert!(
                body.contains(".finish(&state, &app)"),
                "{command} settles its aftermath"
            );
            for direct in [
                "milestones::add",
                "milestones::update",
                "milestones::set_",
                "milestones::remove",
                "milestones::reorder",
                "flags::reopen_milestone",
                "briefing::nudge",
                "project_activity::",
                "projects::touch",
                "announce_changed",
            ] {
                assert!(!body.contains(direct), "{command} does {direct} itself");
            }
        }
    }
}
