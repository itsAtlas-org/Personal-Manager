// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Project milestones (board card 7) — the many-to-one replacement for the single
//! `projects.deadline` scalar. A project carries zero or more dated milestones, each
//! its own row with a STABLE id (the anchor a later flag-layer card hangs
//! deadline-derived flags on). A milestone is either **PM-native** (a user-set,
//! editable `due_date`) or **calendar-linked** (`event_uid` set → its date syncs FROM
//! the canonical, read-only `calendar_events` mirror by iCal UID).
//!
//! The focus view keeps exactly **one** honest status per project, now derived over
//! the *set*: the nearest UNMET milestone is the "governing" one and drives the
//! status. The reduction-to-one-delta happens here, upstream of `projects::derive_status`,
//! which stays a pure `Option<f64> -> ProjectStatus` function. `governing` and
//! `days_until` are themselves pure (deterministically unit-testable), mirroring the
//! `derive_status` / `retrieval::decay_factor` pattern.

use std::collections::HashMap;

use chrono::NaiveDate;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::error::{Error, Result};

pub mod ops;

/// The progress values `project_milestones.status` admits (v42), coarsest-first. Deliberately a
/// small closed set mirrored by the migration's CHECK constraint — a value not in here is rejected
/// at the setter so the DB constraint is a backstop, not the error surface the user meets.
pub const STATUSES: [&str; 4] = ["not_started", "in_progress", "almost_done", "done"];

/// The one `status` value that means the milestone is finished, and therefore the only one that
/// maps to `state = "met"`. Named because three places key off it and a typo in any of them would
/// silently desynchronise `status` from `state`.
pub const STATUS_DONE: &str = "done";

/// The `status` a milestone falls back to when a met one is un-ticked — see `set_state`. Un-ticking
/// means "not finished after all", and "in progress" is the only non-terminal value that stays true
/// whether the user had it at `almost_done` or never set one at all.
const STATUS_REOPENED: &str = "in_progress";

/// One milestone as the frontend sees it: a stored row with its date *resolved*
/// (calendar-linked dates taken from the live calendar mirror) and a couple of derived
/// flags. `due_date` is the EFFECTIVE date — the stored value for a PM-native
/// milestone, the resolved calendar date for a linked one (falling back to the cached
/// value, with `event_missing = true`, when the event is gone/unsynced).
#[derive(Clone, Debug, Serialize)]
pub struct Milestone {
    pub id: i64,
    pub project_name: String,
    pub label: String,
    pub due_date: Option<String>,
    pub event_uid: Option<String>,
    /// `true` when this milestone's date comes from the calendar (read-only) rather
    /// than being user-editable. Equals `event_uid.is_some()`.
    pub calendar_linked: bool,
    /// `true` only for a calendar-linked milestone whose UID isn't in the current
    /// mirror (deleted event, deselected calendar, or not yet synced) — the UI shows
    /// "event not found" and the date falls back to the last cached value.
    pub event_missing: bool,
    /// `"met"`, `"unmet"`, or `None` (untracked — treated as unmet in derivation).
    pub state: Option<String>,
    /// Richer progress (v42): one of [`STATUSES`], or `None` when the user has never set one (a
    /// pre-v42 row). Additive beside `state`, which remains what `is_met` — and so `governing` and
    /// every deadline flag — actually reads; the two setters write both so they cannot contradict.
    pub status: Option<String>,
    /// Where an externally-owned milestone came from (`"sheets"`, `"notion"`, …); `None` = PM-native
    /// (v42). Paired with `external_id` as the durable anchor, the same pattern `event_uid` uses for
    /// calendar-linked rows.
    pub source_type: Option<String>,
    /// The source's own stable row id for an externally-owned milestone; `None` = PM-native (v42).
    pub external_id: Option<String>,
    pub sort_order: i64,
    /// The mod that made it (the `mod_ledger` row written with it), or `None` for the user's own and
    /// for everything made before mods. Decoration only: see [`crate::made_by`].
    #[serde(rename = "madeBy")]
    pub made_by: Option<crate::made_by::MadeBy>,
}

impl Milestone {
    /// A milestone counts as met only when explicitly marked so; NULL/`"unmet"` are unmet.
    /// Deliberately still reads `state`, not `status` — v42 added `status` beside it without moving
    /// any shipped derivation, and the setters keep the pair consistent.
    pub fn is_met(&self) -> bool {
        self.state.as_deref() == Some("met")
    }
}

/// The `state` that must accompany a given `status` so the pair can never disagree: `done` is the
/// only finished value, everything else is explicitly unmet.
fn state_for_status(status: &str) -> &'static str {
    if status == STATUS_DONE {
        "met"
    } else {
        "unmet"
    }
}

/// Reject a `status` outside [`STATUSES`] before it reaches SQL, so a bad value surfaces as a plain
/// message instead of a CHECK-constraint failure.
fn validate_status(status: &str) -> Result<()> {
    if STATUSES.contains(&status) {
        Ok(())
    } else {
        Err(Error::Other(format!(
            "unknown milestone status {status:?} (expected one of {})",
            STATUSES.join(", ")
        )))
    }
}

/// What drives a project's focus-view status + card line — a thin projection of the
/// governing milestone (the full set rides on `ProjectOverview.milestones`).
#[derive(Clone, Debug, Serialize)]
pub struct GoverningMilestone {
    pub id: i64,
    pub label: String,
    pub due_date: Option<String>,
}

/// A raw `project_milestones` row, before calendar resolution.
struct Row {
    id: i64,
    project_name: String,
    label: String,
    due_date: Option<String>,
    event_uid: Option<String>,
    state: Option<String>,
    status: Option<String>,
    source_type: Option<String>,
    external_id: Option<String>,
    sort_order: i64,
    made_by: Option<crate::made_by::MadeBy>,
}

/// Whole days from `today` to `date` (negative = overdue). Both are read as civil
/// dates (the leading `YYYY-MM-DD`); `None` if either is unparseable, so a milestone
/// with no usable date is simply excluded from derivation — never silently "met".
pub fn days_until(today: &str, date: &str) -> Option<f64> {
    let t = parse_date(today)?;
    let d = parse_date(date)?;
    Some((d - t).num_days() as f64)
}

fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.get(0..10)?, "%Y-%m-%d").ok()
}

/// The governing milestone: the nearest UNMET milestone with a resolved date. Overdue
/// (most negative `days_until`) governs over a future one; met and date-less
/// milestones are excluded; `None` when a project has no unmet dated milestone (it then
/// falls through to the other status signals). Pure — given a fixed `today`, the result
/// is a deterministic function of the slice. On a tie the earliest in `sort_order`
/// wins (the input is loaded ordered), so the choice is stable.
pub fn governing<'a>(milestones: &'a [Milestone], today: &str) -> Option<&'a Milestone> {
    milestones
        .iter()
        .filter(|m| !m.is_met())
        .filter_map(|m| {
            m.due_date
                .as_deref()
                .and_then(|d| days_until(today, d))
                .map(|du| (m, du))
        })
        .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(m, _)| m)
}

/// The day-delta of the governing milestone — the single `Option<f64>` fed into
/// `derive_status`, reproducing the old scalar-deadline signal over the milestone set.
pub fn governing_days(milestones: &[Milestone], today: &str) -> Option<f64> {
    governing(milestones, today)
        .and_then(|m| m.due_date.as_deref())
        .and_then(|d| days_until(today, d))
}

/// `GoverningMilestone` projection for the overview (the card line + briefing).
pub fn governing_info(milestones: &[Milestone], today: &str) -> Option<GoverningMilestone> {
    governing(milestones, today).map(|m| GoverningMilestone {
        id: m.id,
        label: m.label.clone(),
        due_date: m.due_date.clone(),
    })
}

/// uid → effective civil date, built from the calendar mirror so a calendar-linked
/// milestone resolves to the soonest not-yet-past instance of its event (recurrences
/// expand to many rows sharing a UID), falling back to the latest past instance so an
/// overdue calendar milestone still governs. Computed once per `list_overviews`.
pub fn calendar_dates_by_uid(conn: &Connection, today: &str) -> Result<HashMap<String, String>> {
    Ok(calendar_occurrences_by_uid(conn)?
        .into_iter()
        .filter_map(|(uid, dates)| {
            // Ascending, so the first on or after today is the soonest upcoming one, and the last
            // is the latest past one when nothing is upcoming. Lexicographic compare is
            // chronological for ISO dates.
            let date = dates
                .iter()
                .find(|d| d.as_str() >= today)
                .or_else(|| dates.last())
                .cloned()?;
            Some((uid, date))
        })
        .collect())
}

/// uid → every civil date its event occurs on, ascending with no repeats. The one bucketing every
/// calendar-linked date reads: [`calendar_dates_by_uid`] picks the soonest upcoming one from it, and
/// a task pinned to one occurrence (PA-1 R1) picks the one nearest its pin.
pub fn calendar_occurrences_by_uid(conn: &Connection) -> Result<HashMap<String, Vec<String>>> {
    // Bucket each event by the civil date the USER sees it on (its zone-local date), not its raw UTC
    // date — otherwise a timed event near midnight resolves a calendar-linked milestone to the wrong
    // day west/east of UTC. Callers compare these with a zone-local `today`, so the two agree.
    let zone = crate::commands::resolve_zone(conn);
    // NB: this deliberately does NOT filter out quiet calendars. A milestone links to an event by its
    // iCal UID only when the user EXPLICITLY attached it — a deliberate project deadline, distinct
    // from the calendar's "quiet" (don't-surface-its-events) preference. Silencing that link off the
    // back of quieting the calendar would leave the milestone tracking a stale cached date while its
    // flags still fired (inconsistent), so quiet governs the event stream (via `agenda_query`) and
    // leaves explicit milestone links alone. (Extending quiet to linked milestones is a separate opt.)
    let mut stmt = conn.prepare(
        "SELECT uid, start FROM calendar_events WHERE uid IS NOT NULL AND start IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;

    let mut by_uid: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        let (uid, start) = row?;
        by_uid
            .entry(uid)
            .or_default()
            .push(crate::clock::zone_date_of(&start, zone));
    }
    for dates in by_uid.values_mut() {
        dates.sort_unstable();
        dates.dedup();
    }
    Ok(by_uid)
}

/// Resolve a raw row against the uid→date map into the frontend `Milestone`.
fn resolve(row: Row, uid_dates: &HashMap<String, String>) -> Milestone {
    let calendar_linked = row.event_uid.is_some();
    let (due_date, event_missing) = match &row.event_uid {
        Some(uid) => match uid_dates.get(uid) {
            Some(d) => (Some(d.clone()), false),
            // Event gone/unsynced: keep the last cached date but flag it missing.
            None => (row.due_date.clone(), true),
        },
        None => (row.due_date.clone(), false),
    };
    Milestone {
        id: row.id,
        project_name: row.project_name,
        label: row.label,
        due_date,
        event_uid: row.event_uid,
        calendar_linked,
        event_missing,
        state: row.state,
        status: row.status,
        source_type: row.source_type,
        external_id: row.external_id,
        sort_order: row.sort_order,
        made_by: row.made_by,
    }
}

// The maker rides on a LEFT JOIN (`idx_mod_ledger_milestone` is a unique index, so a milestone
// matches at most one row and the list is the same length as without it). A test reads the query
// plan of both statements, so the join stays a search of that index.
macro_rules! load_select {
    () => {
        "SELECT m.id, m.project_name, m.label, m.due_date, m.event_uid, m.state, \
                m.status, m.source_type, m.external_id, m.sort_order, \
                l.mod_name, l.tier, l.signed \
           FROM project_milestones m LEFT JOIN mod_ledger l ON l.milestone_id = m.id"
    };
}
/// One project's milestones, in their own order.
pub(crate) const LOAD_ONE_SQL: &str = concat!(
    load_select!(),
    " WHERE m.project_name = ?1 ORDER BY m.sort_order, m.id"
);
/// Every project's milestones.
pub(crate) const LOAD_ALL_SQL: &str = concat!(
    load_select!(),
    " ORDER BY m.project_name, m.sort_order, m.id"
);

fn load_rows(conn: &Connection, project: Option<&str>) -> Result<Vec<Row>> {
    let map = |r: &rusqlite::Row| -> rusqlite::Result<Row> {
        Ok(Row {
            id: r.get(0)?,
            project_name: r.get(1)?,
            label: r.get(2)?,
            due_date: r.get(3)?,
            event_uid: r.get(4)?,
            state: r.get(5)?,
            status: r.get(6)?,
            source_type: r.get(7)?,
            external_id: r.get(8)?,
            sort_order: r.get(9)?,
            made_by: crate::made_by::read(r, 10)?,
        })
    };
    let rows = match project {
        Some(name) => {
            let mut stmt = conn.prepare(LOAD_ONE_SQL)?;
            let out: Vec<Row> = stmt
                .query_map(params![name], map)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            out
        }
        None => {
            let mut stmt = conn.prepare(LOAD_ALL_SQL)?;
            let out: Vec<Row> = stmt
                .query_map([], map)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            out
        }
    };
    Ok(rows)
}

/// Resolved milestones for one project (the project-detail surface), date-ordered.
/// `today` is the user's zone-local civil date (from `clock::today_sql_in`), so the
/// calendar-date resolution agrees with the focus view's boundaries.
pub fn list_for_project(conn: &Connection, project: &str, today: &str) -> Result<Vec<Milestone>> {
    let uid_dates = calendar_dates_by_uid(conn, today)?;
    Ok(load_rows(conn, Some(project))?
        .into_iter()
        .map(|r| resolve(r, &uid_dates))
        .collect())
}

/// Every resolved milestone across all projects, for the calendar overlay (each carries its
/// `project_name`, so the frontend can navigate on click). `today` is the user's zone-local civil
/// date, so calendar-linked dates resolve the same way the project surface sees them.
pub fn list_all(conn: &Connection, today: &str) -> Result<Vec<Milestone>> {
    let uid_dates = calendar_dates_by_uid(conn, today)?;
    Ok(load_rows(conn, None)?
        .into_iter()
        .map(|r| resolve(r, &uid_dates))
        .collect())
}

/// The unmet milestones linked to the calendar event `ical`, resolved as [`list_all`] resolves them
/// (the soonest occurrence on or after `today`, else the latest past one, in the user's zone), whose
/// date is on or after `day` (`YYYY-MM-DD`): what a "this and following" change offers to move to
/// its new part (C9b-2). A ticked-off one isn't offered: moving it would change nothing the user
/// sees.
pub fn linked_on_or_after(
    conn: &Connection,
    ical: &str,
    day: &str,
    today: &str,
) -> Result<Vec<Milestone>> {
    let dates = calendar_dates_by_uid(conn, today)?;
    Ok(load_rows(conn, None)?
        .into_iter()
        .filter(|r| r.event_uid.as_deref() == Some(ical))
        .map(|r| resolve(r, &dates))
        .filter(|m| !m.is_met())
        .filter(|m| {
            m.due_date
                .as_deref()
                .is_some_and(|d| d.get(..10).unwrap_or(d) >= day)
        })
        .collect())
}

/// Whether any milestone is linked to the calendar event `ical`, met or not: what an event put on
/// another calendar under a new identity leaves behind (#884, C18d).
pub fn any_linked_to(conn: &Connection, ical: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM project_milestones WHERE event_uid = ?1)",
        [ical],
        |r| r.get(0),
    )?)
}

/// Every milestone, resolved and grouped by project name — one pass for `list_overviews`.
pub fn all_by_project(conn: &Connection, today: &str) -> Result<HashMap<String, Vec<Milestone>>> {
    let uid_dates = calendar_dates_by_uid(conn, today)?;
    let mut out: HashMap<String, Vec<Milestone>> = HashMap::new();
    for row in load_rows(conn, None)? {
        let m = resolve(row, &uid_dates);
        out.entry(m.project_name.clone()).or_default().push(m);
    }
    Ok(out)
}

/// Trim a value and treat blank as absent (mirrors `projects::clean`).
fn clean(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Add a milestone, making its project exist first ([`crate::projects::ensure_named`]) so the FK holds for
/// a lazy (never-triaged) project and the pickers offer it. `sort_order` defaults to the end of the list. A non-empty
/// `event_uid` makes it calendar-linked; `due_date` is then a cache for offline display.
/// Returns the new stable id. Its own transaction: [`add_in`] is the same write for a caller that
/// has one open.
pub fn add(
    conn: &Connection,
    project: &str,
    label: &str,
    due_date: Option<String>,
    event_uid: Option<String>,
) -> Result<i64> {
    let tx = conn.unchecked_transaction()?;
    let id = add_in(&tx, project, label, due_date, event_uid)?;
    tx.commit()?;
    Ok(id)
}

/// [`add`] inside the caller's transaction: it opens none and commits none, so a caller can write
/// a gift ledger row beside the milestone and have both commit, or neither (see `tasks::add_in`).
/// Refused outside a transaction.
pub fn add_in(
    conn: &Connection,
    project: &str,
    label: &str,
    due_date: Option<String>,
    event_uid: Option<String>,
) -> Result<i64> {
    crate::tasks::require_transaction(conn)?;
    let label = clean(Some(label.to_string())).unwrap_or_else(|| "deadline".to_string());
    let due_date = clean(due_date);
    let event_uid = clean(event_uid);
    crate::projects::ensure_named(conn, project)?;
    conn.execute(
        "INSERT INTO project_milestones(project_name, label, due_date, event_uid, sort_order) \
         VALUES (?1, ?2, ?3, ?4, \
                 (SELECT COALESCE(MAX(sort_order) + 1, 0) FROM project_milestones WHERE project_name = ?1))",
        params![project, label, due_date, event_uid],
    )?;
    Ok(conn.last_insert_rowid())
}

/// The longest milestone label a mod may write, in characters.
pub const MOD_LABEL_MAX: usize = 200;

/// A mod-supplied label and date, checked: [`add`] stores both as given and the IPC path leans on
/// the date field in the page. A mod has no such field, so the date must be a real `YYYY-MM-DD`
/// (month and day in range for the year, exactly ten characters, nothing after), and the label
/// one line of the person's words of at most [`MOD_LABEL_MAX`] characters (`mods::package::user_line_fault`:
/// no control or hidden character but an emoji's joiner and variation selector, no tab or line break). Both are trimmed first; a blank label stays blank (`add` names it
/// "deadline", as it does for the page) and a blank date is no date. Returns the pair to store. The
/// words are fixed and never echo what was sent.
pub fn validate_label_and_date(
    label: &str,
    due_date: Option<&str>,
) -> Result<(String, Option<String>)> {
    let label = label.trim();
    if crate::mods::package::user_line_fault(label, MOD_LABEL_MAX, true).is_err() {
        return Err(Error::Other(format!(
            "A milestone label must be plain text of at most {MOD_LABEL_MAX} characters."
        )));
    }
    let date = match due_date.map(str::trim).filter(|d| !d.is_empty()) {
        None => None,
        Some(d) => {
            let real = d.len() == 10
                && NaiveDate::parse_from_str(d, "%Y-%m-%d")
                    .is_ok_and(|parsed| parsed.format("%Y-%m-%d").to_string() == d);
            if !real {
                return Err(Error::Other(
                    "A milestone date must be a real date written YYYY-MM-DD.".into(),
                ));
            }
            Some(d.to_string())
        }
    };
    Ok((label.to_string(), date))
}

/// Update a milestone's editable fields (label + PM-native date). A calendar-linked
/// milestone keeps its calendar-owned date — the `due_date` here is ignored unless the
/// milestone is PM-native (`event_uid IS NULL`), so the canonical calendar value can
/// never be clobbered from the UI.
pub fn update(conn: &Connection, id: i64, label: &str, due_date: Option<String>) -> Result<()> {
    let label = clean(Some(label.to_string())).unwrap_or_else(|| "deadline".to_string());
    let due_date = clean(due_date);
    conn.execute(
        "UPDATE project_milestones \
         SET label = ?2, \
             due_date = CASE WHEN event_uid IS NULL THEN ?3 ELSE due_date END, \
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') \
         WHERE id = ?1",
        params![id, label, due_date],
    )?;
    Ok(())
}

/// Link a milestone to a calendar event (Some uid; `cached_date` seeds the offline
/// cache) or unlink it (None — the date becomes PM-native/editable again, keeping its
/// last value). The two provenances share one row, distinguished only by `event_uid`.
/// `only_if_linked_to`: change it only while it is still linked to that event (a move between the
/// two parts of a repeating event, offered a while ago, never undoes a change made since; C9b-2).
/// Whether a milestone was changed: false for an id no milestone has, or one linked elsewhere now.
pub fn set_event(
    conn: &Connection,
    id: i64,
    event_uid: Option<String>,
    cached_date: Option<String>,
    only_if_linked_to: Option<&str>,
) -> Result<bool> {
    let event_uid = clean(event_uid);
    let cached_date = clean(cached_date);
    let changed = match event_uid {
        // Link: stamp the uid and seed the cache (keep the old date if none supplied).
        Some(uid) => conn.execute(
            "UPDATE project_milestones \
             SET event_uid = ?2, due_date = COALESCE(?3, due_date), \
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') \
             WHERE id = ?1 AND (?4 IS NULL OR event_uid = ?4)",
            params![id, uid, cached_date, only_if_linked_to],
        )?,
        // Unlink: clear the uid; the last resolved date stays as the editable value.
        None => conn.execute(
            "UPDATE project_milestones \
             SET event_uid = NULL, due_date = COALESCE(?2, due_date), \
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') \
             WHERE id = ?1 AND (?3 IS NULL OR event_uid = ?3)",
            params![id, cached_date, only_if_linked_to],
        )?,
    };
    Ok(changed > 0)
}

/// Mark a milestone met or unmet (the row's tick-box). Also carries `status` along so the pair
/// stays consistent: ticking means `done`; un-ticking a `done` milestone reopens it at
/// `in_progress`, while a milestone that was already unfinished keeps whatever progress value the
/// user had chosen (un-ticking an already-unticked row must not wipe `almost_done`).
pub fn set_state(conn: &Connection, id: i64, met: bool) -> Result<()> {
    let state = if met { "met" } else { "unmet" };
    conn.execute(
        "UPDATE project_milestones \
         SET state  = ?2, \
             status = CASE \
                        WHEN ?2 = 'met'      THEN ?3 \
                        WHEN status = ?3     THEN ?4 \
                        ELSE status \
                      END, \
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') \
         WHERE id = ?1",
        params![id, state, STATUS_DONE, STATUS_REOPENED],
    )?;
    Ok(())
}

/// Set a milestone's progress `status` (v42), carrying `state` with it so the pair can never
/// disagree — `done` is met, every other value is unmet. This is the richer counterpart to
/// `set_state`; both write both columns, which is what lets `is_met` keep reading `state` alone
/// while the UI shows four levels of progress.
pub fn set_status(conn: &Connection, id: i64, status: &str) -> Result<()> {
    validate_status(status)?;
    conn.execute(
        "UPDATE project_milestones \
         SET status = ?2, state = ?3, \
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') \
         WHERE id = ?1",
        params![id, status, state_for_status(status)],
    )?;
    Ok(())
}

/// Create-or-update the milestone owned by an external source, keyed on its own durable
/// `(source_type, external_id)` anchor.
///
/// **This is the only supported way for an external sync to write milestones, and the reason the
/// v42 anchor exists.** Flags anchor on `project_milestones.id`, so a sync that cleared its rows
/// and re-inserted them would mint fresh ids and silently orphan every flag pointing at the old
/// ones — the failure is invisible until a flag that should have fired doesn't. Upserting keeps the
/// id stable for the life of the external row; the partial UNIQUE index behind the `ON CONFLICT`
/// makes the delete-and-recreate alternative fail loudly instead of corrupting the anchor space.
///
/// Returns the milestone's stable id — the same value on every later call for that external row.
///
/// No caller yet: the first is the Tracked Spreadsheet & Database Sync card this one unblocks. It
/// ships now, with the columns and the index, because the invariant is only cheap to guarantee
/// while there is still nothing writing external milestones — the same reason `Source::LocalPath`
/// is a forward seam in `registry.rs`. Exercised by the tests below.
#[allow(dead_code)]
pub fn upsert_external(
    conn: &Connection,
    project: &str,
    source_type: &str,
    external_id: &str,
    label: &str,
    due_date: Option<String>,
    status: Option<&str>,
) -> Result<i64> {
    if let Some(s) = status {
        validate_status(s)?;
    }
    let label = clean(Some(label.to_string())).unwrap_or_else(|| "deadline".to_string());
    let due_date = clean(due_date);
    let state = status.map(state_for_status);

    let tx = conn.unchecked_transaction()?;
    // The project exists the way a filed one does (row, entity, registry tag), so the pickers offer it.
    crate::projects::ensure_named(&tx, project)?;
    // The `ON CONFLICT` target repeats the partial index's own predicate — SQLite requires the two
    // to match before it will use a partial index to resolve an upsert.
    tx.execute(
        "INSERT INTO project_milestones \
             (project_name, label, due_date, source_type, external_id, status, state, sort_order) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, \
                 (SELECT COALESCE(MAX(sort_order) + 1, 0) FROM project_milestones WHERE project_name = ?1)) \
         ON CONFLICT(source_type, external_id) \
                  WHERE source_type IS NOT NULL AND external_id IS NOT NULL \
         DO UPDATE SET \
             project_name = excluded.project_name, \
             label        = excluded.label, \
             due_date     = excluded.due_date, \
             status       = COALESCE(excluded.status, status), \
             state        = COALESCE(excluded.state,  state), \
             updated_at   = strftime('%Y-%m-%dT%H:%M:%fZ','now')",
        params![project, label, due_date, source_type, external_id, status, state],
    )?;
    let id: i64 = tx.query_row(
        "SELECT id FROM project_milestones WHERE source_type = ?1 AND external_id = ?2",
        params![source_type, external_id],
        |r| r.get(0),
    )?;
    tx.commit()?;
    Ok(id)
}

/// Delete a milestone by id.
pub fn remove(conn: &Connection, id: i64) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    remove_in(&tx, id)?;
    tx.commit()?;
    Ok(())
}

/// [`remove`] inside the caller's transaction (see [`add_in`]). Refused outside a transaction.
pub fn remove_in(tx: &Connection, id: i64) -> Result<()> {
    crate::tasks::require_transaction(tx)?;
    tx.execute("DELETE FROM project_milestones WHERE id = ?1", params![id])?;
    // The milestone id IS the flag anchor, and `flags` has no FK to `project_milestones` (the anchor
    // is a text column spanning two identity spaces — iCal UIDs and milestone ids — so no single FK
    // could express it). So deleting the milestone left its flags pointing at a row that no longer
    // exists: an ACTIVE flag lingered until the next detection pass happened to prune it, and a
    // RESOLVED one — a user-asserted tombstone — was removed by nothing, ever. Both states go: the
    // anchor is gone, so there is nothing left for any flag on it to mean. Same transaction, because
    // a half-done delete is what leaves the orphan this fixes.
    tx.execute(
        "DELETE FROM flags WHERE anchor_kind = ?1 AND anchor = ?2",
        params![crate::flags::ANCHOR_MILESTONE, id.to_string()],
    )?;
    Ok(())
}

/// The project a milestone belongs to, or `None` if the id is unknown. Used by the command
/// layer to bump that project's activity date (`projects::touch`) after an id-only edit.
pub fn project_of(conn: &Connection, id: i64) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT project_name FROM project_milestones WHERE id = ?1",
            params![id],
            |r| r.get::<_, String>(0),
        )
        .optional()?)
}

/// Rewrite the ordering of a project's milestones from a caller-supplied id list
/// (sort_order = position). Idempotent and scoped to the project: ids not belonging to
/// it are ignored. The frontend reorders locally and persists the whole order at once.
pub fn reorder(conn: &Connection, project: &str, ordered_ids: &[i64]) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    for (pos, id) in ordered_ids.iter().enumerate() {
        tx.execute(
            "UPDATE project_milestones SET sort_order = ?3, \
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') \
             WHERE id = ?1 AND project_name = ?2",
            params![id, project, pos as i64],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Route a legacy single `deadline` (from `set_project_metadata` or an AI proposal)
/// into the canonical PM-native milestone labelled `"deadline"`: update it if present,
/// else create it. Calendar-linked milestones are never touched. Keeps the old
/// single-field edit path working while milestones are the source of truth.
pub fn set_primary_deadline(conn: &Connection, project: &str, deadline: &str) -> Result<()> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM project_milestones \
             WHERE project_name = ?1 AND label = 'deadline' AND event_uid IS NULL \
             ORDER BY sort_order, id LIMIT 1",
            params![project],
            |r| r.get(0),
        )
        .ok();
    match existing {
        Some(id) => update(conn, id, "deadline", Some(deadline.to_string())),
        None => add(conn, project, "deadline", Some(deadline.to_string()), None).map(|_| ()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(id: i64, due: Option<&str>, met: bool) -> Milestone {
        Milestone {
            id,
            project_name: "P".into(),
            label: format!("m{id}"),
            due_date: due.map(|s| s.into()),
            event_uid: None,
            calendar_linked: false,
            event_missing: false,
            state: Some(if met { "met" } else { "unmet" }.into()),
            status: Some(if met { "done" } else { "in_progress" }.into()),
            source_type: None,
            external_id: None,
            sort_order: id,
            made_by: None,
        }
    }

    const TODAY: &str = "2026-06-28";

    #[test]
    fn nearest_unmet_governs() {
        let set = vec![
            ms(1, Some("2026-07-28"), false), // +30
            ms(2, Some("2026-07-01"), false), // +3  <- nearest unmet
            ms(3, Some("2026-06-30"), true),  // +2 but MET
        ];
        let g = governing(&set, TODAY).unwrap();
        assert_eq!(g.id, 2);
        assert_eq!(governing_days(&set, TODAY), Some(3.0));
    }

    #[test]
    fn overdue_unmet_outranks_future() {
        let set = vec![
            ms(1, Some("2026-07-01"), false), // +3
            ms(2, Some("2026-06-25"), false), // -3 overdue, most negative
        ];
        assert_eq!(governing(&set, TODAY).unwrap().id, 2);
        assert_eq!(governing_days(&set, TODAY), Some(-3.0));
    }

    #[test]
    fn all_met_or_dateless_has_no_governing() {
        let all_met = vec![ms(1, Some("2026-07-01"), true)];
        assert!(governing(&all_met, TODAY).is_none());
        // A date-less (unresolved calendar-linked) milestone is excluded, never "met".
        let dateless = vec![ms(2, None, false)];
        assert!(governing(&dateless, TODAY).is_none());
        // Empty set.
        assert!(governing(&[], TODAY).is_none());
    }

    #[test]
    fn ties_resolve_by_sort_order_deterministically() {
        let set = vec![
            ms(5, Some("2026-07-01"), false),
            ms(3, Some("2026-07-01"), false),
        ];
        // Both +3; the first in the (sort-ordered) slice wins — id 5 here.
        assert_eq!(governing(&set, TODAY).unwrap().id, 5);
    }

    #[test]
    fn days_until_handles_datetime_and_garbage() {
        assert_eq!(days_until(TODAY, "2026-06-28T09:00:00Z"), Some(0.0));
        assert_eq!(days_until(TODAY, "not-a-date"), None);
    }

    // --- v42: status / external anchor ------------------------------------------------------

    /// A store with the full migration ladder applied, for the v42 column tests.
    fn store() -> (tempfile::TempDir, Connection) {
        const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        (dir, conn)
    }

    fn row(conn: &Connection, id: i64) -> (Option<String>, Option<String>) {
        conn.query_row(
            "SELECT state, status FROM project_milestones WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    }

    /// The whole point of writing both columns at each setter: no reachable sequence of calls
    /// leaves `status` saying one thing and `state` (what `is_met`/`governing` read) another.
    #[test]
    fn status_and_state_never_contradict() {
        let (_dir, conn) = store();
        let id = add(&conn, "P", "pitch", Some("2026-08-01".into()), None).unwrap();

        set_status(&conn, id, "almost_done").unwrap();
        assert_eq!(
            row(&conn, id),
            (Some("unmet".into()), Some("almost_done".into())),
            "an unfinished status must leave the milestone unmet"
        );

        set_status(&conn, id, "done").unwrap();
        assert_eq!(
            row(&conn, id),
            (Some("met".into()), Some("done".into())),
            "done is the one status that marks the milestone met"
        );

        // Back down from done via the status control.
        set_status(&conn, id, "in_progress").unwrap();
        assert_eq!(
            row(&conn, id),
            (Some("unmet".into()), Some("in_progress".into()))
        );

        // ...and via the tick-box, from both directions.
        set_state(&conn, id, true).unwrap();
        assert_eq!(
            row(&conn, id),
            (Some("met".into()), Some("done".into())),
            "ticking the box carries status to done"
        );
        set_state(&conn, id, false).unwrap();
        assert_eq!(
            row(&conn, id),
            (Some("unmet".into()), Some("in_progress".into())),
            "un-ticking a done milestone reopens it at in_progress"
        );
    }

    /// Un-ticking an already-unfinished milestone must not clobber a deliberate `almost_done`.
    #[test]
    fn unticking_an_unfinished_milestone_keeps_its_progress() {
        let (_dir, conn) = store();
        let id = add(&conn, "P", "pitch", Some("2026-08-01".into()), None).unwrap();
        set_status(&conn, id, "almost_done").unwrap();
        set_state(&conn, id, false).unwrap();
        assert_eq!(
            row(&conn, id),
            (Some("unmet".into()), Some("almost_done".into()))
        );
    }

    #[test]
    fn unknown_status_is_rejected_before_sql() {
        let (_dir, conn) = store();
        let id = add(&conn, "P", "pitch", None, None).unwrap();
        let err = set_status(&conn, id, "nearly").unwrap_err().to_string();
        assert!(err.contains("nearly"), "message names the bad value: {err}");
        assert_eq!(
            row(&conn, id).1,
            None,
            "a rejected status leaves the row untouched"
        );
    }

    /// The card's load-bearing invariant: re-syncing an external row UPDATES it in place, so the
    /// milestone id — which is the flag anchor — survives. A regression here orphans flags silently.
    #[test]
    fn external_upsert_keeps_the_id_stable_across_resyncs() {
        let (_dir, conn) = store();
        let first = upsert_external(
            &conn,
            "Atlas",
            "sheets",
            "row-7",
            "pitch",
            Some("2026-08-01".into()),
            Some("in_progress"),
        )
        .unwrap();
        let second = upsert_external(
            &conn,
            "Atlas",
            "sheets",
            "row-7",
            "pitch (final)",
            Some("2026-08-09".into()),
            Some("done"),
        )
        .unwrap();
        assert_eq!(
            first, second,
            "the id is the flag anchor and must not be re-minted"
        );

        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM project_milestones WHERE external_id = 'row-7'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "upsert updated in place rather than inserting a second row"
        );

        let ms = list_for_project(&conn, "Atlas", TODAY).unwrap();
        let m = ms.iter().find(|m| m.id == first).unwrap();
        assert_eq!(m.label, "pitch (final)");
        assert_eq!(m.due_date.as_deref(), Some("2026-08-09"));
        assert_eq!(m.source_type.as_deref(), Some("sheets"));
        assert_eq!(m.external_id.as_deref(), Some("row-7"));
        assert!(
            m.is_met(),
            "status=done carried state to met through the upsert"
        );
    }

    /// A project first named by an external milestone is one every picker offers, as for `add`.
    #[test]
    fn a_project_first_named_by_an_external_milestone_is_offered_by_the_pickers() {
        let (_dir, conn) = store();
        upsert_external(&conn, "Brand New", "sheets", "r1", "pitch", None, None).unwrap();
        assert_eq!(crate::tags::project_names(&conn).unwrap(), ["Brand New"]);
        // A re-sync, and the project under another casing, add no second registry row.
        upsert_external(&conn, "Brand New", "sheets", "r1", "pitch 2", None, None).unwrap();
        upsert_external(&conn, "brand new", "sheets", "r2", "wrap", None, None).unwrap();
        let tags: i64 = conn
            .query_row(
                "SELECT count(*) FROM tags WHERE kind = 'project'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tags, 1);
        assert_eq!(crate::tags::project_names(&conn).unwrap(), ["Brand New"]);
    }

    /// Two different sources may legitimately use the same row id; the anchor is the PAIR.
    #[test]
    fn external_anchor_is_scoped_by_source_type() {
        let (_dir, conn) = store();
        let a = upsert_external(&conn, "Atlas", "sheets", "1", "a", None, None).unwrap();
        let b = upsert_external(&conn, "Atlas", "notion", "1", "b", None, None).unwrap();
        assert_ne!(
            a, b,
            "same external_id under a different source is a different milestone"
        );
    }

    /// PM-native rows carry NULL for both anchor columns; the index is partial so any number of
    /// them coexist (a plain UNIQUE index over the pair would still allow this in SQLite, but the
    /// predicate states the intent — this pins that the many-NULL case really is unconstrained).
    #[test]
    fn native_milestones_are_unconstrained_by_the_external_index() {
        let (_dir, conn) = store();
        for i in 0..3 {
            add(
                &conn,
                "P",
                &format!("m{i}"),
                Some("2026-08-01".into()),
                None,
            )
            .unwrap();
        }
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM project_milestones WHERE project_name = 'P' AND source_type IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 3);
    }

    /// Mirror rows of the calendar event `uid`, one per start.
    fn occurrences(conn: &Connection, uid: &str, starts: &[&str]) {
        for (i, start) in starts.iter().enumerate() {
            conn.execute(
                "INSERT INTO calendar_events (id, calendar_id, summary, start, all_day, uid) \
                 VALUES (?1, 'cal', 'Gym', ?2, 0, ?3)",
                params![format!("{uid}#{i}"), start, uid],
            )
            .unwrap();
        }
    }

    /// C9b-2: a split offers the milestones whose date, resolved as the list resolves it (the
    /// soonest occurrence from today), is on or after the day it starts; a ticked-off one, or one
    /// on another event, never.
    #[test]
    fn linked_on_or_after_resolves_as_the_list_does() {
        let (_dir, conn) = store();
        let today = "2026-10-12";
        occurrences(
            &conn,
            "gym@x",
            &[
                "2026-10-05T09:00:00Z",
                "2026-10-19T09:00:00Z",
                "2026-10-26T09:00:00Z",
            ],
        );
        occurrences(
            &conn,
            "past@x",
            &["2026-09-28T09:00:00Z", "2026-10-05T09:00:00Z"],
        );
        let launch = add(&conn, "Atlas", "Launch", None, Some("gym@x".into())).unwrap();
        let done = add(&conn, "Atlas", "Done", None, Some("gym@x".into())).unwrap();
        set_state(&conn, done, true).unwrap();
        add(&conn, "Atlas", "Other", None, Some("past@x".into())).unwrap();
        let ids = |day: &str, uid: &str| -> Vec<(i64, Option<String>)> {
            linked_on_or_after(&conn, uid, day, today)
                .unwrap()
                .into_iter()
                .map(|m| (m.id, m.due_date))
                .collect()
        };
        assert_eq!(
            ids("2026-10-19", "gym@x"),
            vec![(launch, Some("2026-10-19".into()))]
        );
        assert!(ids("2026-10-26", "gym@x").is_empty());
        // Only past occurrences: the latest past one, which is before the day.
        assert!(ids("2026-10-19", "past@x").is_empty());
        let other = linked_on_or_after(&conn, "past@x", "2026-10-05", today).unwrap();
        assert_eq!(other.len(), 1);
        assert_eq!(other[0].due_date.as_deref(), Some("2026-10-05"));
    }

    /// C18d: whether a milestone names a calendar event, met or not, and only that event.
    #[test]
    fn any_linked_to_finds_a_link_to_that_event_only() {
        let (_dir, conn) = store();
        assert!(!any_linked_to(&conn, "abc@x").unwrap());
        let id = add(&conn, "Atlas", "Launch", None, Some("abc@x".into())).unwrap();
        add(&conn, "Atlas", "Kick-off", Some("2026-11-02".into()), None).unwrap();
        assert!(any_linked_to(&conn, "abc@x").unwrap());
        assert!(!any_linked_to(&conn, "abd@x").unwrap());
        set_state(&conn, id, true).unwrap();
        assert!(any_linked_to(&conn, "abc@x").unwrap(), "met still links");
    }

    /// C9b-2: linking says whether a milestone had that id, so "Moved" is never said of nothing;
    /// with `only_if_linked_to`, only while it's still linked there (a stale offer moves nothing).
    #[test]
    fn set_event_says_when_no_milestone_has_the_id() {
        let (_dir, conn) = store();
        let id = add(&conn, "Atlas", "Launch", None, Some("abc@x".into())).unwrap();
        let link = |m: i64, uid: Option<&str>, only: Option<&str>| {
            set_event(&conn, m, uid.map(str::to_string), None, only).unwrap()
        };
        let now = || -> Option<String> {
            conn.query_row(
                "SELECT event_uid FROM project_milestones WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert!(!link(id + 99, Some("tail@x"), None));
        assert!(!link(id + 99, None, None));
        assert!(link(id, Some("tail@x"), Some("abc@x")));
        assert_eq!(now().as_deref(), Some("tail@x"));
        // An offer made before the move: it's no longer linked there, so nothing changes.
        assert!(!link(id, Some("tail@x"), Some("abc@x")));
        assert!(link(id, Some("abc@x"), None));
        // Unlinked meanwhile: a Move pressed now moves nothing, and the link stays as the user set it.
        assert!(link(id, None, None));
        assert!(!link(id, Some("tail@x"), Some("abc@x")));
        assert_eq!(now(), None);
    }

    /// The migration stamps `done` on rows already marked met, so no pre-v42 row starts life in the
    /// contradictory state the setters are built to prevent.
    #[test]
    fn migration_backfills_status_for_already_met_milestones() {
        let (_dir, conn) = store();
        let id = add(&conn, "P", "pitch", Some("2026-08-01".into()), None).unwrap();
        // Simulate a pre-v42 row: met, with no status yet.
        conn.execute(
            "UPDATE project_milestones SET state = 'met', status = NULL WHERE id = ?1",
            params![id],
        )
        .unwrap();
        conn.execute(
            "UPDATE project_milestones SET status = 'done' WHERE state = 'met'",
            [],
        )
        .unwrap();
        assert_eq!(row(&conn, id), (Some("met".into()), Some("done".into())));
    }

    // --- K0: the seams a mod path needs (PR C) -----------------------------------------------

    fn milestone_rows(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM project_milestones", [], |r| r.get(0))
            .unwrap()
    }

    /// See `tasks::add_in`: a gift's ledger row commits with the milestone or not at all.
    #[test]
    fn add_in_rolls_back_with_its_callers_transaction() {
        let (_dir, conn) = store();
        let tx = conn.unchecked_transaction().unwrap();
        let id = add_in(&tx, "Brand New", "Pitch", Some("2026-12-01".into()), None).unwrap();
        assert!(id > 0);
        assert_eq!(milestone_rows(&tx), 1);
        tx.rollback().unwrap();
        assert_eq!(milestone_rows(&conn), 0);
        let projects: i64 = conn
            .query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0))
            .unwrap();
        assert_eq!(projects, 0, "the project row it minted rolled back too");

        let tx = conn.unchecked_transaction().unwrap();
        add_in(&tx, "Atlas", "Kept", None, None).unwrap();
        tx.commit().unwrap();
        assert_eq!(milestone_rows(&conn), 1);
        // The wrapper is the same write in a transaction of its own.
        add(&conn, "Atlas", "Second", None, None).unwrap();
        assert_eq!(milestone_rows(&conn), 2);
    }

    /// A project first named by a milestone is one every picker offers (`tags::project_names`).
    #[test]
    fn a_project_first_named_by_a_milestone_is_offered_by_the_pickers() {
        let (_dir, conn) = store();
        add(&conn, "Brand New", "Pitch", None, None).unwrap();
        assert_eq!(crate::tags::project_names(&conn).unwrap(), ["Brand New"]);
        // A second milestone, and the same project under another casing, add no second registry
        // row or entity: the milestone keeps the name it was given, as it always has.
        add(&conn, "Brand New", "Launch", None, None).unwrap();
        add(&conn, "brand new", "Wrap", None, None).unwrap();
        let rows = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(rows("SELECT count(*) FROM tags WHERE kind = 'project'"), 1);
        assert_eq!(
            rows("SELECT count(*) FROM entities WHERE type = 'project' AND lower(canonical_name) = 'brand new'"),
            1
        );
        assert_eq!(rows("SELECT count(*) FROM projects"), 2);
        assert_eq!(crate::tags::project_names(&conn).unwrap(), ["Brand New"]);
    }

    /// A project named by an alias is the entity's project: the pickers keep offering the canonical
    /// name, never the alias.
    #[test]
    fn a_milestone_naming_a_project_by_an_alias_offers_the_canonical_name() {
        let (_dir, conn) = store();
        let id = crate::entities::resolve_project(&conn, "Atlas", true)
            .unwrap()
            .unwrap();
        conn.execute(
            "INSERT INTO entity_aliases(entity_id, alias) VALUES (?1, 'The Atlas')",
            [id],
        )
        .unwrap();
        add(&conn, "The Atlas", "Pitch", None, None).unwrap();
        assert_eq!(crate::tags::project_names(&conn).unwrap(), ["Atlas"]);
        let entities: i64 = conn
            .query_row(
                "SELECT count(*) FROM entities WHERE type = 'project' AND lower(canonical_name) LIKE '%atlas'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(entities, 1);
    }

    #[test]
    fn add_in_refuses_to_run_outside_a_transaction() {
        let (_dir, conn) = store();
        assert!(add_in(&conn, "Atlas", "Pitch", None, None).is_err());
        assert_eq!(milestone_rows(&conn), 0);
    }

    #[test]
    fn a_mod_milestone_date_must_be_a_real_ymd() {
        let ok = |d: &str| validate_label_and_date("Pitch", Some(d)).map(|(_, date)| date);
        assert_eq!(ok("2026-12-01").unwrap().as_deref(), Some("2026-12-01"));
        assert_eq!(
            ok(" 2028-02-29 ").unwrap().as_deref(),
            Some("2028-02-29"),
            "a leap day, trimmed"
        );
        for bad in [
            "2026-02-30",       // not a day
            "2027-02-29",       // not a leap year
            "2026-13-01",       // not a month
            "2026-1-5",         // not zero-padded
            "26-12-01",         // not four digits
            "2026-12-01T10:00", // a time is not a date
            "01-12-2026",
            "2026/12/01",
            "tomorrow",
            "２０２６-12-01",
            "+2026-12-01",
        ] {
            let err = validate_label_and_date("Pitch", Some(bad)).unwrap_err();
            assert_eq!(
                err.to_string(),
                "A milestone date must be a real date written YYYY-MM-DD.",
                "{bad:?}"
            );
        }
        // No date is allowed (a blank is no date); the label is trimmed and a blank one is left for
        // `add` to name.
        assert_eq!(
            validate_label_and_date("  Pitch ", None).unwrap(),
            ("Pitch".into(), None)
        );
        assert_eq!(
            validate_label_and_date("Pitch", Some("  ")).unwrap().1,
            None
        );
        assert_eq!(validate_label_and_date("  ", None).unwrap().0, "");
    }

    #[test]
    fn a_mod_milestone_label_is_short_plain_text() {
        let longest = "x".repeat(MOD_LABEL_MAX);
        assert_eq!(validate_label_and_date(&longest, None).unwrap().0, longest);
        let too_long = "x".repeat(MOD_LABEL_MAX + 1);
        for bad in [
            too_long.as_str(),
            "two\nlines",
            "tab\there",
            "bell\u{7}",
            "hidden\u{200b}space",
            "flip\u{202e}text",
        ] {
            let err = validate_label_and_date(bad, None).unwrap_err();
            assert_eq!(
                err.to_string(),
                "A milestone label must be plain text of at most 200 characters.",
                "{bad:?}"
            );
        }
        // Counted in characters, not bytes.
        assert!(validate_label_and_date(&"é".repeat(MOD_LABEL_MAX), None).is_ok());
    }
}
