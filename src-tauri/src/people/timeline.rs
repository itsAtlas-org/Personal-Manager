// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The timeline: what happened between you and the people in PM (spec 6.1). An entry is a `met`,
//! `call`, `message` or `note` with a civil date, an optional instant, a note and, when it came
//! from a calendar event, a copy of where it came from (the event's iCal id, the occurrence's
//! start and the title). One entry names several people through `interaction_people`.
//!
//! - **`last_met` is a query**, never a stored column: the newest `met` entry's date, so it can't
//!   go stale ([`last_met`]).
//! - **Entries never move `people.updated_at`.** That column is the editor's "changed elsewhere"
//!   check and the merge Undo guard; logging a coffee must not make an open editor refuse its save.
//! - **A trashed person is invisible on other people's timelines**: a shared entry lists the others
//!   only, and leaves the trashed one out. Their link survives, so a restore puts them back, and an
//!   edit that replaces the people on an entry never drops a link it can't see.
//! - **The event link is a soft link** (I-04): the iCal id plus the occurrence's start, no FK to
//!   `calendar_events`, whose rows a sync deletes and re-creates. The title is copied for the same
//!   reason.
//!
//! The meetings still owed an answer, and answering them, are in [`super::meet`].
//!
//! Not model-path code (E13). No entry text reaches a log, a panic or an error string: refusals are
//! fixed words naming the field, never a value.

use std::collections::HashSet;

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::fold::{clean_multiline, clean_text};
use super::partial_date::normalise_dated;
use super::{
    in_tx, no_such_person, now, refuse, require_transaction, INTERACTION_KINDS, NOTES_MAX,
    VALUE_MAX,
};
use crate::error::Result;

/// Entries on a record's first page (the one `get_person` carries) and the default page.
pub const PAGE: usize = 20;
/// The most one page may hold, whatever the caller asks.
pub const PAGE_MAX: usize = 100;
/// People one entry may name (a conference is still one entry).
pub const ENTRY_PEOPLE_MAX: usize = 500;
/// The longest an event start copied onto an entry may be (an ISO instant or a date).
pub(super) const START_MAX: usize = 64;

/// The `kind` of the entry that makes someone "met": the first of [`INTERACTION_KINDS`], not a second
/// literal, so the set keeps one owner (I-12).
pub const KIND_MET: &str = INTERACTION_KINDS[0];

fn no_such_entry() -> crate::error::Error {
    refuse("That entry isn't in PM any more.")
}

// --- types -------------------------------------------------------------------------------------

/// Someone else on a shared entry: enough to show a name and open them.
#[derive(Clone, Serialize)]
pub struct EntryPerson {
    pub id: i64,
    pub display_name: String,
}

impl std::fmt::Debug for EntryPerson {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EntryPerson")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// One entry on a person's timeline. Holds a note and a title, so no `Debug` derive: the manual one
/// prints ids and counts.
#[derive(Clone, Serialize)]
pub struct TimelineEntry {
    pub id: i64,
    pub kind: String,
    /// The user's civil date, `YYYY-MM-DD`.
    pub on_date: String,
    /// An instant (ISO UTC) when known, e.g. the event's start.
    pub at: Option<String>,
    pub note: String,
    /// The calendar event's iCal id, when the entry came from one.
    #[serde(rename = "event_uid")]
    pub event_ical: Option<String>,
    /// That occurrence's start, as the calendar stored it.
    pub event_start: Option<String>,
    /// The event's title, copied (calendar rows are deleted and re-created by syncs).
    pub event_title: Option<String>,
    /// Everyone else on the entry, leaving out the person whose timeline this is and anyone in Trash.
    pub others: Vec<EntryPerson>,
    pub created_at: String,
}

impl std::fmt::Debug for TimelineEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimelineEntry")
            .field("id", &self.id)
            .field("others", &self.others.len())
            .finish_non_exhaustive()
    }
}

/// Where the next page starts: the last entry of the page before, by the order the timeline is
/// kept in (date, then instant, then id, all newest first). Holds no contact value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineCursor {
    pub on_date: String,
    /// The entry's instant, or empty when it has none.
    pub at: String,
    pub id: i64,
}

/// One page of a timeline, newest first. `next` is `None` on the last page.
#[derive(Clone, Serialize)]
pub struct TimelinePage {
    pub entries: Vec<TimelineEntry>,
    pub next: Option<TimelineCursor>,
}

impl std::fmt::Debug for TimelinePage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimelinePage")
            .field("entries", &self.entries.len())
            .field("more", &self.next.is_some())
            .finish()
    }
}

/// The calendar occurrence an entry came from.
#[derive(Clone, Deserialize)]
pub struct EventLink {
    /// The event's iCal id.
    #[serde(rename = "uid")]
    pub ical: String,
    /// The occurrence's start as the calendar stores it (an instant or a date).
    pub start: String,
    pub title: String,
}

/// One entry to add, or the edit to save over an existing one. Holds a note, so no `Debug` derive.
/// On an edit the `event` link is left as it was: an entry's origin is not editable.
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub struct InteractionDraft {
    pub kind: String,
    /// The user's civil date, `YYYY-MM-DD`.
    pub on_date: String,
    /// An instant (RFC 3339) when the time is known.
    pub at: Option<String>,
    pub note: String,
    /// Everyone the entry names (at least one, none in Trash).
    pub people: Vec<i64>,
    pub event: Option<EventLink>,
}

impl std::fmt::Debug for InteractionDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InteractionDraft")
            .field("people", &self.people.len())
            .finish_non_exhaustive()
    }
}

/// A draft after validation: cleaned, normalised, and ready to write.
pub(super) struct CleanEntry {
    pub kind: String,
    pub on_date: String,
    pub at: Option<String>,
    pub note: String,
    /// The event's iCal id and the occurrence's start: the soft link.
    pub link: Option<(String, String)>,
    /// The event's title, copied. An entry may carry a title with no link (an event with no id).
    pub title: Option<String>,
}

// --- reading -------------------------------------------------------------------------------------

/// The date of the newest `met` entry that names this person, or none. The one definition of "last
/// met": the record asks it, and the People list's grouped subquery says the same thing in SQL.
pub fn last_met(conn: &Connection, person_id: i64) -> Result<Option<String>> {
    Ok(conn.query_row(
        "SELECT MAX(i.on_date) FROM interaction_people ip \
           JOIN interactions i ON i.id = ip.interaction_id \
          WHERE ip.person_id = ?1 AND i.kind = ?2",
        params![person_id, KIND_MET],
        |r| r.get(0),
    )?)
}

/// One page of a person's timeline, newest first (date, then instant, then id). `before` is the
/// `next` cursor of the page before, or `None` for the first page. `limit` is clamped to
/// `1..=`[`PAGE_MAX`]. Refuses an unknown person (one in Trash still opens, as their record does).
pub fn list_timeline(
    conn: &Connection,
    person_id: i64,
    before: Option<&TimelineCursor>,
    limit: usize,
) -> Result<TimelinePage> {
    if !super::person_exists(conn, person_id)? {
        return Err(no_such_person());
    }
    let limit = limit.clamp(1, PAGE_MAX);
    // One more than asked for: the extra row only says whether there is a next page.
    let mut stmt = conn.prepare(
        "SELECT i.id, i.kind, i.on_date, i.at, i.note, i.event_uid, i.event_start, i.event_title, \
                i.created_at \
           FROM interaction_people ip JOIN interactions i ON i.id = ip.interaction_id \
          WHERE ip.person_id = ?1 \
            AND (?2 IS NULL OR (i.on_date, COALESCE(i.at, ''), i.id) < (?2, ?3, ?4)) \
          ORDER BY i.on_date DESC, COALESCE(i.at, '') DESC, i.id DESC \
          LIMIT ?5",
    )?;
    let (cursor_date, cursor_at, cursor_id) = match before {
        Some(c) => (Some(c.on_date.as_str()), c.at.as_str(), c.id),
        None => (None, "", 0),
    };
    let mut entries = stmt
        .query_map(
            params![
                person_id,
                cursor_date,
                cursor_at,
                cursor_id,
                limit as i64 + 1
            ],
            |r| {
                Ok(TimelineEntry {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    on_date: r.get(2)?,
                    at: r.get(3)?,
                    note: r.get(4)?,
                    event_ical: r.get(5)?,
                    event_start: r.get(6)?,
                    event_title: r.get(7)?,
                    others: Vec::new(),
                    created_at: r.get(8)?,
                })
            },
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(stmt);
    let more = entries.len() > limit;
    entries.truncate(limit);
    let mut others = conn.prepare(
        "SELECT p.id, p.display_name FROM interaction_people ip JOIN people p ON p.id = ip.person_id \
          WHERE ip.interaction_id = ?1 AND ip.person_id <> ?2 AND p.trashed_at IS NULL \
          ORDER BY p.sort_key, p.id",
    )?;
    for entry in &mut entries {
        entry.others = others
            .query_map(params![entry.id, person_id], |r| {
                Ok(EntryPerson {
                    id: r.get(0)?,
                    display_name: r.get(1)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
    }
    let next = if more {
        entries.last().map(|e| TimelineCursor {
            on_date: e.on_date.clone(),
            at: e.at.clone().unwrap_or_default(),
            id: e.id,
        })
    } else {
        None
    };
    Ok(TimelinePage { entries, next })
}

/// The first page, as the record carries it.
pub fn first_page(conn: &Connection, person_id: i64) -> Result<TimelinePage> {
    list_timeline(conn, person_id, None, PAGE)
}

// --- validating -----------------------------------------------------------------------------------

/// `YYYY-MM-DD` and a real day, or none.
fn civil_day(text: &str) -> Option<String> {
    let text = clean_text(text);
    normalise_dated(&text).filter(|d| d.len() == 10)
}

/// An RFC 3339 instant as `YYYY-MM-DDTHH:MM:SSZ`, or none.
pub(super) fn instant(text: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(text.trim()).ok().map(|dt| {
        dt.with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Secs, true)
    })
}

/// The first `max` characters, cut on a character boundary.
pub(super) fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// Validate and clean a draft before anything is written. No refusal quotes a value. An event title
/// is clipped, not refused: a meeting with a very long name must still be loggable.
pub(super) fn clean_entry(draft: &InteractionDraft, with_event: bool) -> Result<CleanEntry> {
    if !INTERACTION_KINDS.contains(&draft.kind.as_str()) {
        return Err(refuse(
            "An entry is a meeting, a call, a message or a note.",
        ));
    }
    let on_date = civil_day(&draft.on_date).ok_or_else(|| refuse("The date isn't a real date."))?;
    let at = match draft.at.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        None => None,
        Some(text) => Some(instant(text).ok_or_else(|| refuse("The time isn't a real time."))?),
    };
    let note = clean_multiline(&draft.note);
    if note.len() > NOTES_MAX {
        return Err(refuse(format!(
            "The note is too long ({} of {NOTES_MAX} bytes allowed).",
            note.len()
        )));
    }
    let (link, title) = match (&draft.event, with_event) {
        (Some(link), true) => {
            let ical = clean_text(&link.ical);
            let start = clean_text(&link.start);
            if ical.is_empty() || start.is_empty() {
                return Err(refuse("A meeting link needs both its id and its start."));
            }
            if ical.chars().count() > VALUE_MAX || start.chars().count() > START_MAX {
                return Err(refuse("The meeting's id or start is too long."));
            }
            (
                Some((ical, start)),
                Some(clip(&clean_text(&link.title), VALUE_MAX)),
            )
        }
        _ => (None, None),
    };
    Ok(CleanEntry {
        kind: draft.kind.clone(),
        on_date,
        at,
        note,
        link,
        title,
    })
}

/// The people an entry names: deduplicated, in order, at least one, none missing or in Trash.
fn live_people(tx: &Connection, people: &[i64]) -> Result<Vec<i64>> {
    let mut seen = HashSet::new();
    let ids: Vec<i64> = people
        .iter()
        .copied()
        .filter(|id| seen.insert(*id))
        .collect();
    if ids.is_empty() {
        return Err(refuse("Name at least one person."));
    }
    if ids.len() > ENTRY_PEOPLE_MAX {
        return Err(refuse(format!(
            "An entry can name up to {ENTRY_PEOPLE_MAX} people ({} given).",
            ids.len()
        )));
    }
    for id in &ids {
        let live: Option<bool> = tx
            .query_row(
                "SELECT trashed_at IS NULL FROM people WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        if live != Some(true) {
            return Err(no_such_person());
        }
    }
    Ok(ids)
}

// --- writing --------------------------------------------------------------------------------------

/// Insert one entry and its people. The caller has validated both. Never touches `people`.
pub(super) fn insert_entry(tx: &Connection, entry: &CleanEntry, people: &[i64]) -> Result<i64> {
    let stamp = now(tx)?;
    let (ical, start) = match &entry.link {
        Some((ical, start)) => (Some(ical), Some(start)),
        None => (None, None),
    };
    tx.execute(
        "INSERT INTO interactions(kind, on_date, at, note, event_uid, event_start, event_title, \
                                  created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        params![
            entry.kind,
            entry.on_date,
            entry.at,
            entry.note,
            ical,
            start,
            entry.title,
            stamp
        ],
    )?;
    let id = tx.last_insert_rowid();
    for person in people {
        tx.execute(
            "INSERT INTO interaction_people(interaction_id, person_id) VALUES (?1, ?2)",
            params![id, person],
        )?;
    }
    Ok(id)
}

/// Add an entry in the caller's transaction. Returns its id. People's own records are not touched
/// (`updated_at` stays), so an open editor still saves.
pub fn add_interaction_in(tx: &Connection, draft: &InteractionDraft) -> Result<i64> {
    require_transaction(tx)?;
    let entry = clean_entry(draft, true)?;
    let people = live_people(tx, &draft.people)?;
    insert_entry(tx, &entry, &people)
}

/// Add an entry. See [`add_interaction_in`].
pub fn add_interaction(conn: &Connection, draft: &InteractionDraft) -> Result<i64> {
    in_tx(conn, |tx| add_interaction_in(tx, draft))
}

/// Save an edit over an entry in the caller's transaction: its kind, date, time, note and people.
/// The event link stays as it was. The people replace the ones the entry shows: anyone in Trash
/// keeps their link (the editor can't see them, and a restore brings them back), so an edit never
/// drops what it couldn't show.
pub fn update_interaction_in(tx: &Connection, id: i64, draft: &InteractionDraft) -> Result<()> {
    require_transaction(tx)?;
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM interactions WHERE id = ?1)",
        params![id],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(no_such_entry());
    }
    let entry = clean_entry(draft, false)?;
    let people = live_people(tx, &draft.people)?;
    let stamp = now(tx)?;
    tx.execute(
        "UPDATE interactions SET kind = ?2, on_date = ?3, at = ?4, note = ?5, updated_at = ?6 \
          WHERE id = ?1",
        params![id, entry.kind, entry.on_date, entry.at, entry.note, stamp],
    )?;
    // Drop the live people the edit took off; keep the trashed ones' links.
    let named: HashSet<i64> = people.iter().copied().collect();
    let mut linked = tx.prepare(
        "SELECT ip.person_id FROM interaction_people ip JOIN people p ON p.id = ip.person_id \
          WHERE ip.interaction_id = ?1 AND p.trashed_at IS NULL",
    )?;
    let now_on: Vec<i64> = linked
        .query_map(params![id], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    drop(linked);
    for person in now_on.iter().filter(|p| !named.contains(p)) {
        tx.execute(
            "DELETE FROM interaction_people WHERE interaction_id = ?1 AND person_id = ?2",
            params![id, person],
        )?;
    }
    for person in &people {
        tx.execute(
            "INSERT OR IGNORE INTO interaction_people(interaction_id, person_id) VALUES (?1, ?2)",
            params![id, person],
        )?;
    }
    Ok(())
}

/// Save an edit over an entry. See [`update_interaction_in`].
pub fn update_interaction(conn: &Connection, id: i64, draft: &InteractionDraft) -> Result<()> {
    in_tx(conn, |tx| update_interaction_in(tx, id, draft))
}

/// Delete an entry for everyone it names (its rows in `interaction_people` go with it). A meeting's
/// "logged" answer stays, so deleting the entry doesn't make the meeting owed an answer again.
pub fn delete_interaction(conn: &Connection, id: i64) -> Result<()> {
    let n = conn.execute("DELETE FROM interactions WHERE id = ?1", params![id])?;
    if n == 0 {
        return Err(no_such_entry());
    }
    Ok(())
}
