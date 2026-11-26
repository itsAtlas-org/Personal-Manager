// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! People: PM's own address book (Contacts, PR A), and the single owner of the `people`,
//! `person_details`, `contact_labels`, `person_labels` and `person_sources` tables and of the
//! `person_projects` links (v63).
//!
//! A person is a record with a permanent `id` in this store and a random `uid` across stores;
//! **people are not `entities` rows** and never reference `entities` or `documents` (E2), so two
//! people called Sam coexist and a Rebuild can't orphan anyone. Every function here is a plain
//! `&Connection` function, free of `AppHandle`, so a later reader needs no IPC. Writes that must
//! commit together with another table's rows have an `*_in` form that refuses to run outside a
//! transaction ([`require_transaction`], the shape of `tasks.rs`); the others open their own.
//!
//! The rules every write follows (spec 2.2) live here and in the three submodules: [`fold`] (how
//! text is cleaned and keyed), [`partial_date`] (birthdays with no year) and [`sources`] (links to
//! a vCard file or a Google contact, and the tombstone a purge leaves behind). In short:
//!
//! - **Keys are written with the row.** `sort_key`, `search_key` and each detail's `value_key` are
//!   recomputed in the same transaction as any change ([`rekey_person`]); a query never folds.
//! - **`updated_at` moves on every change** to a person's row, details, labels, star, project links
//!   or mapped sources ([`touch_people`]), strictly forward even inside one millisecond: the
//!   editor's "changed elsewhere" check relies on it. Timeline entries alone do not move it.
//! - **A typed display name is yours.** It is recomposed from the parts only when it was never
//!   typed (it equalled the composition of the old parts).
//! - **No contact value reaches a log, a panic or an error string.** Errors name the field and a
//!   count. Types holding values do not derive `Debug`; their manual `Debug` prints ids and counts.
//! - **Trash, then purge.** Delete moves a person to Trash for [`TRASH_DAYS`] days; [`purge_expired`]
//!   and [`delete_people_now`] then remove them in one transaction, keeping only a Google tombstone
//!   holding ids (spec 2.2).
//!
//! Not model-path code: nothing here may be named from `chat`, `briefing`, `mods` or the MCP tools
//! (E13, gated in a later slice).

pub mod card;
pub mod dupes;
pub mod export;
pub mod fold;
pub mod import;
pub mod meet;
pub mod merge;
pub mod partial_date;
pub mod sources;
pub mod timeline;
pub mod vcard;

#[cfg(test)]
mod import_tests;
#[cfg(test)]
mod meet_tests;
#[cfg(test)]
mod merge_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod timeline_tests;
#[cfg(test)]
mod vcard_tests;

use std::collections::{HashMap, HashSet};

use rusqlite::types::Value as SqlValue;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};
use fold::{clean_multiline, clean_text, fold_key, value_key};
use sources::SourceInfo;

// --- the closed value sets (I-12: one owner per vocabulary; v63 has no CHECK on them) ----------

/// `person_details.kind`. `web` covers websites and socials (the label carries "Instagram"); `date`
/// is anniversaries and other dates (a birthday is a column); `location` is "where they are now".
pub const DETAIL_KINDS: [&str; 12] = [
    "email",
    "phone",
    "address",
    "web",
    "work",
    "school",
    "nickname",
    "former_name",
    "date",
    "relation",
    "custom",
    "location",
];
/// `person_sources.provider` (and `label_sources.provider`). `icloud` comes later.
pub const PROVIDERS: [&str; 2] = [PROVIDER_VCARD, PROVIDER_GOOGLE];
/// `person_sources.state`.
pub const SOURCE_STATES: [&str; 6] = [
    STATE_LINKED,
    STATE_PENDING_CREATE,
    STATE_PENDING_DELETE,
    STATE_GONE,
    STATE_IGNORED,
    STATE_LOCAL_ONLY,
];
/// `interactions.kind`. `email` arrives with PA-4.
pub const INTERACTION_KINDS: [&str; 4] = ["met", "call", "message", "note"];
/// `meet_prompt_answers.answer`.
pub const PROMPT_ANSWERS: [&str; 3] = ["logged", "skipped", "series_skipped"];

/// A card imported from a vCard file.
pub const PROVIDER_VCARD: &str = "vcard";
/// A Google contact (the Google PR).
pub const PROVIDER_GOOGLE: &str = "google";
pub const STATE_LINKED: &str = "linked";
pub const STATE_PENDING_CREATE: &str = "pending_create";
/// The user ticked "Also delete them from Google" and Google's delete is not confirmed yet.
pub const STATE_PENDING_DELETE: &str = "pending_delete";
pub const STATE_GONE: &str = "gone";
/// A tombstone holding ids only.
pub const STATE_IGNORED: &str = "ignored";
pub const STATE_LOCAL_ONLY: &str = "local_only";

const KIND_EMAIL: &str = "email";
const KIND_PHONE: &str = "phone";
const KIND_ADDRESS: &str = "address";
const KIND_WORK: &str = "work";
const KIND_NICKNAME: &str = "nickname";
const KIND_DATE: &str = "date";

// --- the caps, matching Google's so a sync can never fail on size -------------------------------

/// Characters in one value (a name part, a label, a detail's value or parts string).
pub const VALUE_MAX: usize = 1_024;
/// Bytes in a person's notes.
pub const NOTES_MAX: usize = 64 * 1_024;
/// Details one person may hold.
pub const DETAILS_MAX: usize = 500;
/// Labels one person may carry.
pub const LABELS_PER_PERSON_MAX: usize = 64;
/// People one store may hold (a soft cap with a clear message: Google's per-account limit).
pub const PEOPLE_MAX: i64 = 25_000;
/// Days a person stays in Trash before the purge removes them.
pub const TRASH_DAYS: i64 = 30;
/// Words a search query uses (the rest are ignored).
const SEARCH_WORDS_MAX: usize = 8;
/// The most a `parts` JSON may weigh once written.
const PARTS_MAX_BYTES: usize = 16 * 1_024;

/// What an editor save says when the record moved under it (the Google sync can edit a person
/// while the dialog is open). A constant so the frontend can recognise it.
pub const CHANGED_ELSEWHERE: &str =
    "This person was changed somewhere else since you opened them. Close and reopen them to see the latest.";

// --- the one emit ---------------------------------------------------------------------------------

/// Emitted (with `()`) after every people write, once the DB guard is dropped. The People tab
/// debounces its refetch by 250 ms, so an import or a merge emits once, at the end.
pub const PEOPLE_CHANGED: &str = "people://changed";

/// Tell every open view the address book changed. **The one place [`PEOPLE_CHANGED`] is emitted**
/// (a gate in a later slice fails the build on any other emit of it). Call it once the DB guard is
/// dropped. Nothing is told to mods: people never reach them (E13).
pub fn announce_changed(app: &tauri::AppHandle) {
    use tauri::Emitter;
    let _ = app.emit(PEOPLE_CHANGED, ());
}

// --- the merge Undo shelf (AppState holds the mutex; the rules live in `merge`) -----------------------

/// Put a merge's before-images on the shelf, for the store open now. Returns the Undo token, or
/// none if there was nothing to hold (or the shelf is unusable: the merge then simply has no Undo).
/// Call it while still holding the DB guard, so the epoch is the store the merge wrote to.
pub fn shelve(
    shelf: &std::sync::Mutex<merge::UndoShelf>,
    epoch: u64,
    steps: Vec<merge::UndoStep>,
) -> Option<String> {
    shelf
        .lock()
        .ok()?
        .put(epoch, steps, std::time::Instant::now())
}

/// Take a merge's steps off the shelf, or refuse in the words of [`merge::NO_LONGER`].
pub fn unshelve(
    shelf: &std::sync::Mutex<merge::UndoShelf>,
    token: &str,
    epoch: u64,
    at: std::time::Instant,
) -> Result<Vec<merge::UndoStep>> {
    shelf
        .lock()
        .map_err(|_| refuse(merge::NO_LONGER))?
        .take(token, epoch, at)
}

/// Put steps back under their token after an Undo that wrote nothing failed for a reason other than
/// a refusal (the database was busy, say): the offer is still good and a retry can use it.
pub fn reshelve(
    shelf: &std::sync::Mutex<merge::UndoShelf>,
    token: String,
    epoch: u64,
    steps: Vec<merge::UndoStep>,
) {
    if let Ok(mut shelf) = shelf.lock() {
        shelf.put_back(token, epoch, steps, std::time::Instant::now());
    }
}

// --- errors: fixed words, never a value -------------------------------------------------------------

fn refuse(message: impl Into<String>) -> Error {
    Error::Other(message.into())
}

/// The refusal for a missing person. Fixed words: never the id's neighbours or any name.
fn no_such_person() -> Error {
    refuse("That person isn't in PM any more.")
}

fn too_long(field: &str, len: usize, max: usize, unit: &str) -> Error {
    refuse(format!(
        "The {field} is too long ({len} of {max} {unit} allowed)."
    ))
}

// --- types -------------------------------------------------------------------------------------------

/// One person to create or save. The editor sends the whole record; `details` replaces the old
/// list (a detail that carries its `id` is updated in place, so the sync's bookkeeping on it
/// survives). Holds contact values, so no `Debug` derive: its manual one prints counts only.
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub struct PersonDraft {
    pub display_name: String,
    pub given_name: String,
    pub middle_name: String,
    pub family_name: String,
    pub name_prefix: String,
    pub name_suffix: String,
    /// `YYYY-MM-DD`, or `--MM-DD` when the year is unknown.
    #[serde(rename = "birthday")]
    pub born_on: Option<String>,
    pub met_where: String,
    /// `YYYY`, `YYYY-MM` or `YYYY-MM-DD`.
    pub met_on: Option<String>,
    pub notes: String,
    pub starred: bool,
    pub details: Vec<DetailDraft>,
}

impl std::fmt::Debug for PersonDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersonDraft")
            .field("details", &self.details.len())
            .finish_non_exhaustive()
    }
}

/// One detail to save. See [`PersonDraft`].
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub struct DetailDraft {
    /// The stored row this edits; `None` for a new detail.
    pub id: Option<i64>,
    pub kind: String,
    pub label: String,
    pub value: String,
    pub parts: Option<Value>,
    pub from_date: Option<String>,
    pub to_date: Option<String>,
    pub is_current: bool,
}

impl std::fmt::Debug for DetailDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DetailDraft")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// One stored detail.
#[derive(Clone, Serialize)]
pub struct Detail {
    pub id: i64,
    pub kind: String,
    pub label: String,
    pub value: String,
    pub parts: Option<Value>,
    pub from_date: Option<String>,
    pub to_date: Option<String>,
    pub is_current: bool,
    pub position: i64,
}

impl std::fmt::Debug for Detail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Detail")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// A label as a person carries it.
#[derive(Clone, Serialize)]
pub struct LabelRef {
    pub id: i64,
    pub name: String,
}

impl std::fmt::Debug for LabelRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LabelRef")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// A label and how many people (outside Trash) carry it.
#[derive(Clone, Serialize)]
pub struct Label {
    pub id: i64,
    #[serde(rename = "uid")]
    pub ident: String,
    pub name: String,
    pub people: i64,
}

impl std::fmt::Debug for Label {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Label")
            .field("id", &self.id)
            .field("people", &self.people)
            .finish_non_exhaustive()
    }
}

/// One person, everything (spec 2.3 `get_person`), with the first page of their timeline.
#[derive(Clone, Serialize)]
pub struct Person {
    pub id: i64,
    #[serde(rename = "uid")]
    pub ident: String,
    pub display_name: String,
    pub given_name: String,
    pub middle_name: String,
    pub family_name: String,
    pub name_prefix: String,
    pub name_suffix: String,
    #[serde(rename = "birthday")]
    pub born_on: Option<String>,
    pub met_where: String,
    pub met_on: Option<String>,
    pub notes: String,
    pub starred: bool,
    /// The uids folded into this person by merges. Internal: the wire never carries them.
    #[serde(skip)]
    pub folded_idents: Vec<String>,
    pub trashed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub details: Vec<Detail>,
    pub labels: Vec<LabelRef>,
    /// The newest `met` entry's date, or none.
    pub last_met: Option<String>,
    /// The first page of their timeline, newest first; the rest comes from `list_timeline`.
    pub timeline: timeline::TimelinePage,
    pub sources: Vec<SourceInfo>,
    pub projects: Vec<String>,
}

impl std::fmt::Debug for Person {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Person")
            .field("id", &self.id)
            .field("details", &self.details.len())
            .field("labels", &self.labels.len())
            .field("sources", &self.sources.len())
            .field("projects", &self.projects.len())
            .finish_non_exhaustive()
    }
}

/// One row of the list (spec 2.3 `list_people`).
#[derive(Clone, Serialize)]
pub struct PersonRow {
    pub id: i64,
    #[serde(rename = "uid")]
    pub ident: String,
    pub display_name: String,
    /// The primary email, else the primary phone.
    pub primary: Option<String>,
    /// The first work organisation.
    pub company: Option<String>,
    pub labels: Vec<LabelRef>,
    pub starred: bool,
    pub last_met: Option<String>,
    /// The state of the person's Google link (the first one), if any.
    pub google_state: Option<String>,
    pub trashed_at: Option<String>,
    /// `YYYY-MM-DD` or `--MM-DD`: the list's upcoming-birthdays strip reads it without a second
    /// round trip per person.
    #[serde(rename = "birthday")]
    pub born_on: Option<String>,
}

impl std::fmt::Debug for PersonRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersonRow")
            .field("id", &self.id)
            .field("labels", &self.labels.len())
            .finish_non_exhaustive()
    }
}

/// What a save answers: enough for the editor to carry on without a second read.
#[derive(Clone, Serialize)]
pub struct PersonSaved {
    pub id: i64,
    pub display_name: String,
    pub updated_at: String,
}

impl std::fmt::Debug for PersonSaved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersonSaved")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// Which people a list shows. Everything but `Trash` hides people in Trash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum ListFilter {
    All,
    Starred,
    Label(i64),
    Trash,
    Unlabelled,
}

/// How a list is ordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum ListSort {
    First,
    Last,
    RecentlyMet,
}

// --- small shared helpers ------------------------------------------------------------------------------

/// The guard behind every `*_in` function: the caller opened a transaction. A programming error,
/// never a user's.
pub(crate) fn require_transaction(conn: &Connection) -> Result<()> {
    if conn.is_autocommit() {
        return Err(refuse(
            "internal error: this write must run inside a transaction",
        ));
    }
    Ok(())
}

/// Run one of the `*_in` writes in a transaction of its own: the thin wrapper shape every public
/// write without a caller's transaction uses. An error drops the transaction, which rolls back.
fn in_tx<T>(conn: &Connection, body: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
    let tx = conn.unchecked_transaction()?;
    let out = body(&tx)?;
    tx.commit()?;
    Ok(out)
}

/// 32 random lowercase hex characters: the cross-store identity a person is minted with.
fn new_ident() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| refuse("rng failure"))?;
    Ok(hex::encode(bytes))
}

fn now(conn: &Connection) -> Result<String> {
    crate::ingest::iso_now(conn)
}

fn char_count(s: &str) -> usize {
    s.chars().count()
}

fn check_len(field: &str, s: &str) -> Result<()> {
    let n = char_count(s);
    if n > VALUE_MAX {
        return Err(too_long(field, n, VALUE_MAX, "characters"));
    }
    Ok(())
}

/// Set `updated_at` on each person, strictly forward: a write inside the same millisecond as the
/// last one still produces a new value, so the editor's "changed elsewhere" check can't miss it.
pub(crate) fn touch_people(tx: &Connection, ids: &[i64]) -> Result<()> {
    let stamp = now(tx)?;
    let mut stmt = tx.prepare(
        "UPDATE people SET updated_at = CASE WHEN ?1 > updated_at THEN ?1 \
             ELSE strftime('%Y-%m-%dT%H:%M:%fZ', updated_at, '+0.001 seconds') END \
          WHERE id = ?2",
    )?;
    for id in ids {
        stmt.execute(params![stamp, id])?;
    }
    Ok(())
}

fn person_exists(conn: &Connection, id: i64) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM people WHERE id = ?1)",
        params![id],
        |r| r.get(0),
    )?)
}

/// The ids that exist, deduplicated, in the order given.
fn existing_ids(conn: &Connection, ids: &[i64]) -> Result<Vec<i64>> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for &id in ids {
        if seen.insert(id) && person_exists(conn, id)? {
            out.push(id);
        }
    }
    Ok(out)
}

// --- names ----------------------------------------------------------------------------------------------

/// The five name parts of a person, cleaned.
#[derive(Default)]
struct NameParts {
    prefix: String,
    given: String,
    middle: String,
    family: String,
    suffix: String,
}

/// What a person is called when nobody typed a name (spec 2.2): prefix given middle family suffix,
/// else the first nickname, else the first work organisation, else the first email, else the first
/// phone. `details` is `(kind, value)` in display order (position within kind). Empty when there is
/// nothing to go on.
fn compose_display_name(name: &NameParts, details: &[(String, String)]) -> String {
    let full = [
        &name.prefix,
        &name.given,
        &name.middle,
        &name.family,
        &name.suffix,
    ]
    .iter()
    .filter(|s| !s.is_empty())
    .map(|s| s.as_str())
    .collect::<Vec<_>>()
    .join(" ");
    if !full.is_empty() {
        return full;
    }
    for kind in [KIND_NICKNAME, KIND_WORK, KIND_EMAIL, KIND_PHONE] {
        if let Some((_, value)) = details.iter().find(|(k, v)| k == kind && !v.is_empty()) {
            return value.clone();
        }
    }
    String::new()
}

// --- a draft, cleaned and checked -------------------------------------------------------------------------

struct CleanDetail {
    id: Option<i64>,
    kind: String,
    label: String,
    value: String,
    parts: Option<Value>,
    from_date: Option<String>,
    to_date: Option<String>,
    is_current: bool,
}

struct CleanDraft {
    display_name: String,
    name: NameParts,
    born_on: Option<String>,
    met_where: String,
    met_on: Option<String>,
    notes: String,
    starred: bool,
    details: Vec<CleanDetail>,
}

/// Clean every string inside a `parts` JSON and check its size. Unknown keys are kept verbatim
/// (forward compatibility); only the text inside is cleaned.
fn clean_parts(value: &Value, multiline: bool, depth: usize) -> Result<Value> {
    if depth > 4 {
        return Err(refuse("A detail's extra fields are nested too deeply."));
    }
    Ok(match value {
        Value::String(s) => {
            let cleaned = if multiline {
                clean_multiline(s)
            } else {
                clean_text(s)
            };
            check_len("detail", &cleaned)?;
            Value::String(cleaned)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|v| clean_parts(v, multiline, depth + 1))
                .collect::<Result<Vec<_>>>()?,
        ),
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                out.insert(clean_text(k), clean_parts(v, multiline, depth + 1)?);
            }
            Value::Object(out)
        }
        other => other.clone(),
    })
}

/// A date field: blank means none; otherwise it must be a real date of the shape `normalise` takes.
fn clean_date(
    field: &str,
    raw: &Option<String>,
    normalise: fn(&str) -> Option<String>,
) -> Result<Option<String>> {
    let Some(text) = raw else { return Ok(None) };
    let text = clean_text(text);
    if text.is_empty() {
        return Ok(None);
    }
    normalise(&text)
        .map(Some)
        .ok_or_else(|| refuse(format!("The {field} isn't a real date.")))
}

/// Validate and clean a whole draft before anything is written (spec 2.2: caps are checked before
/// the write). No refusal quotes a value.
fn clean_draft(draft: &PersonDraft) -> Result<CleanDraft> {
    if draft.details.len() > DETAILS_MAX {
        return Err(refuse(format!(
            "A person can hold up to {DETAILS_MAX} details ({} given).",
            draft.details.len()
        )));
    }
    let single = |field: &str, s: &str| -> Result<String> {
        let c = clean_text(s);
        check_len(field, &c)?;
        Ok(c)
    };
    let name = NameParts {
        prefix: single("name prefix", &draft.name_prefix)?,
        given: single("given name", &draft.given_name)?,
        middle: single("middle name", &draft.middle_name)?,
        family: single("family name", &draft.family_name)?,
        suffix: single("name suffix", &draft.name_suffix)?,
    };
    let display_name = single("name", &draft.display_name)?;
    let met_where = single("place you met", &draft.met_where)?;
    let notes = clean_multiline(&draft.notes);
    if notes.len() > NOTES_MAX {
        return Err(too_long("notes", notes.len(), NOTES_MAX, "bytes"));
    }
    let born_on = clean_date("birthday", &draft.born_on, partial_date::normalise_born_on)?;
    // No "not in the future" rule: the spec has none, and the device clock and the user's zone can
    // both put a true date ahead of "today", which would then lock every later save of the person.
    let met_on = clean_date("date you met", &draft.met_on, partial_date::normalise_dated)?;

    let mut details = Vec::with_capacity(draft.details.len());
    let mut seen_ids = HashSet::new();
    for d in &draft.details {
        if !DETAIL_KINDS.contains(&d.kind.as_str()) {
            return Err(refuse("A detail has a kind PM doesn't know."));
        }
        if let Some(id) = d.id {
            if !seen_ids.insert(id) {
                return Err(refuse("A detail appears twice in one save."));
            }
        }
        let multiline = d.kind == KIND_ADDRESS;
        let value = if multiline {
            clean_multiline(&d.value)
        } else {
            clean_text(&d.value)
        };
        check_len(&d.kind, &value)?;
        // A blank row (an editor line left empty) holds nothing; it is dropped, not saved.
        if value.is_empty() {
            continue;
        }
        let value = if d.kind == KIND_DATE {
            partial_date::normalise_any(&value)
                .ok_or_else(|| refuse("A date detail isn't a real date."))?
        } else {
            value
        };
        let parts = match &d.parts {
            None | Some(Value::Null) => None,
            Some(v @ Value::Object(_)) => {
                let cleaned = clean_parts(v, multiline, 0)?;
                if cleaned.to_string().len() > PARTS_MAX_BYTES {
                    return Err(too_long(
                        "details' extra fields",
                        cleaned.to_string().len(),
                        PARTS_MAX_BYTES,
                        "bytes",
                    ));
                }
                Some(cleaned)
            }
            Some(_) => return Err(refuse("A detail's extra fields must be an object.")),
        };
        details.push(CleanDetail {
            id: d.id,
            kind: d.kind.clone(),
            label: single("label", &d.label)?,
            value,
            parts,
            from_date: clean_date("from date", &d.from_date, partial_date::normalise_dated)?,
            to_date: clean_date("to date", &d.to_date, partial_date::normalise_dated)?,
            is_current: d.is_current,
        });
    }
    Ok(CleanDraft {
        display_name,
        name,
        born_on,
        met_where,
        met_on,
        notes,
        starred: draft.starred,
        details,
    })
}

/// `(kind, value)` in display order: details grouped by kind in the order given.
fn ordered_pairs(details: &[CleanDetail]) -> Vec<(String, String)> {
    details
        .iter()
        .map(|d| (d.kind.clone(), d.value.clone()))
        .collect()
}

fn empty_to_none(s: &str) -> Option<&str> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

// --- keys -------------------------------------------------------------------------------------------------

/// Recompute a person's `sort_key`, `search_key` and every detail's `value_key` from what is
/// stored. Called inside the transaction of every write that changes a person or their details,
/// and by the merge and the sync's apply path. A no-op for an unknown id.
pub(crate) fn rekey_person(tx: &Connection, person_id: i64) -> Result<()> {
    type NameRow = (String, Option<String>, Option<String>, Option<String>);
    let row: Option<NameRow> = tx
        .query_row(
            "SELECT display_name, given_name, middle_name, family_name FROM people WHERE id = ?1",
            params![person_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((display, given, middle, family)) = row else {
        return Ok(());
    };
    struct Row {
        id: i64,
        kind: String,
        value: String,
        parts: Option<String>,
        key: String,
    }
    let mut stmt = tx.prepare(
        "SELECT id, kind, value, parts, value_key FROM person_details \
          WHERE person_id = ?1 ORDER BY kind, position, id",
    )?;
    let rows = stmt
        .query_map(params![person_id], |r| {
            Ok(Row {
                id: r.get(0)?,
                kind: r.get(1)?,
                value: r.get(2)?,
                parts: r.get(3)?,
                key: r.get(4)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(stmt);
    for row in &rows {
        let parts: Option<Value> = row
            .parts
            .as_deref()
            .and_then(|t| serde_json::from_str(t).ok());
        let key = value_key(&row.kind, &row.value, parts.as_ref());
        if key != row.key {
            tx.execute(
                "UPDATE person_details SET value_key = ?2 WHERE id = ?1",
                params![row.id, key],
            )?;
        }
    }
    let pairs: Vec<(&str, &str)> = rows
        .iter()
        .map(|r| (r.kind.as_str(), r.value.as_str()))
        .collect();
    let search = fold::search_key(&fold::SearchParts {
        display_name: &display,
        given_name: given.as_deref().unwrap_or(""),
        middle_name: middle.as_deref().unwrap_or(""),
        family_name: family.as_deref().unwrap_or(""),
        details: &pairs,
    });
    tx.execute(
        "UPDATE people SET sort_key = ?2, search_key = ?3 WHERE id = ?1",
        params![person_id, fold::sort_key(&display), search],
    )?;
    Ok(())
}

// --- create and update ----------------------------------------------------------------------------------

fn insert_detail(
    tx: &Connection,
    person_id: i64,
    d: &CleanDetail,
    position: i64,
    stamp: &str,
) -> Result<()> {
    let key = value_key(&d.kind, &d.value, d.parts.as_ref());
    tx.execute(
        "INSERT INTO person_details(person_id, kind, label, value, parts, from_date, to_date, \
                                    is_current, position, value_key, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
        params![
            person_id,
            d.kind,
            d.label,
            d.value,
            d.parts.as_ref().map(Value::to_string),
            d.from_date,
            d.to_date,
            d.is_current,
            position,
            key,
            stamp
        ],
    )?;
    Ok(())
}

/// The position of each detail among those of its kind, in the order given.
fn positions(details: &[CleanDetail]) -> Vec<i64> {
    let mut next: HashMap<&str, i64> = HashMap::new();
    details
        .iter()
        .map(|d| {
            let slot = next.entry(d.kind.as_str()).or_insert(0);
            let at = *slot;
            *slot += 1;
            at
        })
        .collect()
}

/// Create a person in the caller's transaction. Returns the new id. Refuses a blank record (no
/// name, email, phone, nickname or company to call them by) and a store already holding
/// [`PEOPLE_MAX`] people.
pub fn create_person_in(tx: &Connection, draft: &PersonDraft) -> Result<i64> {
    require_transaction(tx)?;
    let held: i64 = tx.query_row("SELECT COUNT(*) FROM people", [], |r| r.get(0))?;
    if held >= PEOPLE_MAX {
        return Err(refuse(format!(
            "PM holds up to {PEOPLE_MAX} people. Delete some before adding more."
        )));
    }
    let clean = clean_draft(draft)?;
    if clean.details.iter().any(|d| d.id.is_some()) {
        return Err(refuse("A new person can't reuse another person's details."));
    }
    let display = if clean.display_name.is_empty() {
        compose_display_name(&clean.name, &ordered_pairs(&clean.details))
    } else {
        clean.display_name.clone()
    };
    if display.is_empty() {
        return Err(refuse("Give them a name, an email or a phone number."));
    }
    check_len("name", &display)?;
    let stamp = now(tx)?;
    tx.execute(
        "INSERT INTO people(uid, display_name, given_name, middle_name, family_name, name_prefix, \
                            name_suffix, birthday, met_where, met_on, notes, starred, \
                            created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)",
        params![
            new_ident()?,
            display,
            empty_to_none(&clean.name.given),
            empty_to_none(&clean.name.middle),
            empty_to_none(&clean.name.family),
            empty_to_none(&clean.name.prefix),
            empty_to_none(&clean.name.suffix),
            clean.born_on,
            clean.met_where,
            clean.met_on,
            clean.notes,
            clean.starred,
            stamp
        ],
    )?;
    let id = tx.last_insert_rowid();
    for (d, position) in clean.details.iter().zip(positions(&clean.details)) {
        insert_detail(tx, id, d, position, &stamp)?;
    }
    rekey_person(tx, id)?;
    Ok(id)
}

/// Create a person. Returns the new id.
pub fn create_person(conn: &Connection, draft: &PersonDraft) -> Result<i64> {
    let tx = conn.unchecked_transaction()?;
    let id = create_person_in(&tx, draft)?;
    tx.commit()?;
    Ok(id)
}

/// Save a person in the caller's transaction: refuses when `updated_at` is no longer
/// `base_updated_at` (the record moved under the editor), when the person is in Trash, and when a
/// detail's `id` isn't one of theirs. `details` replaces the list: an id keeps its row (and the
/// sync's `src_key` on it), a missing id deletes, no id inserts.
///
/// The display name follows spec 2.2: recomposed from the new parts when the old one was never
/// typed (it equalled the composition of the old parts) and the draft didn't change it; a typed
/// name is left alone.
pub fn update_person_in(
    tx: &Connection,
    id: i64,
    draft: &PersonDraft,
    base_updated_at: &str,
) -> Result<PersonSaved> {
    require_transaction(tx)?;
    type Stored = (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        Option<String>,
    );
    let stored: Option<Stored> = tx
        .query_row(
            "SELECT display_name, given_name, middle_name, family_name, name_prefix, name_suffix, \
                    updated_at, trashed_at FROM people WHERE id = ?1",
            params![id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ))
            },
        )
        .optional()?;
    let Some((old_display, og, om, of, op, os, updated_at, trashed_at)) = stored else {
        return Err(no_such_person());
    };
    if updated_at != base_updated_at {
        return Err(refuse(CHANGED_ELSEWHERE));
    }
    if trashed_at.is_some() {
        return Err(refuse("That person is in Trash. Restore them to edit."));
    }
    let clean = clean_draft(draft)?;

    // The old details, in display order, to learn whether the old name was ever typed.
    let old_details: Vec<(i64, String, String)> = {
        let mut stmt = tx.prepare(
            "SELECT id, kind, value FROM person_details WHERE person_id = ?1 \
              ORDER BY kind, position, id",
        )?;
        let rows = stmt
            .query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    let old_ids: HashSet<i64> = old_details.iter().map(|(i, _, _)| *i).collect();
    if clean
        .details
        .iter()
        .filter_map(|d| d.id)
        .any(|i| !old_ids.contains(&i))
    {
        return Err(refuse("A detail doesn't belong to this person."));
    }
    let old_name = NameParts {
        prefix: op.unwrap_or_default(),
        given: og.unwrap_or_default(),
        middle: om.unwrap_or_default(),
        family: of.unwrap_or_default(),
        suffix: os.unwrap_or_default(),
    };
    let old_pairs: Vec<(String, String)> = old_details
        .iter()
        .map(|(_, k, v)| (k.clone(), v.clone()))
        .collect();
    let was_typed = old_display != compose_display_name(&old_name, &old_pairs);
    let new_composed = compose_display_name(&clean.name, &ordered_pairs(&clean.details));
    let display = if clean.display_name.is_empty() {
        new_composed
    } else if !was_typed && clean.display_name == old_display {
        // Never typed, and the editor left it alone: follow the parts.
        new_composed
    } else {
        clean.display_name.clone()
    };
    if display.is_empty() {
        return Err(refuse("Give them a name, an email or a phone number."));
    }
    check_len("name", &display)?;

    tx.execute(
        "UPDATE people SET display_name = ?2, given_name = ?3, middle_name = ?4, family_name = ?5, \
                           name_prefix = ?6, name_suffix = ?7, birthday = ?8, met_where = ?9, \
                           met_on = ?10, notes = ?11, starred = ?12 \
          WHERE id = ?1",
        params![
            id,
            display,
            empty_to_none(&clean.name.given),
            empty_to_none(&clean.name.middle),
            empty_to_none(&clean.name.family),
            empty_to_none(&clean.name.prefix),
            empty_to_none(&clean.name.suffix),
            clean.born_on,
            clean.met_where,
            clean.met_on,
            clean.notes,
            clean.starred
        ],
    )?;

    let stamp = now(tx)?;
    let kept: HashSet<i64> = clean.details.iter().filter_map(|d| d.id).collect();
    for old in &old_ids {
        if !kept.contains(old) {
            tx.execute("DELETE FROM person_details WHERE id = ?1", params![old])?;
        }
    }
    for (d, position) in clean.details.iter().zip(positions(&clean.details)) {
        match d.id {
            Some(detail_id) => {
                tx.execute(
                    "UPDATE person_details SET kind = ?2, label = ?3, value = ?4, parts = ?5, \
                            from_date = ?6, to_date = ?7, is_current = ?8, position = ?9, \
                            updated_at = ?10 \
                      WHERE id = ?1 AND person_id = ?11",
                    params![
                        detail_id,
                        d.kind,
                        d.label,
                        d.value,
                        d.parts.as_ref().map(Value::to_string),
                        d.from_date,
                        d.to_date,
                        d.is_current,
                        position,
                        stamp,
                        id
                    ],
                )?;
            }
            None => insert_detail(tx, id, d, position, &stamp)?,
        }
    }
    rekey_person(tx, id)?;
    touch_people(tx, &[id])?;
    let updated_at: String = tx.query_row(
        "SELECT updated_at FROM people WHERE id = ?1",
        params![id],
        |r| r.get(0),
    )?;
    Ok(PersonSaved {
        id,
        display_name: display,
        updated_at,
    })
}

/// Save a person. See [`update_person_in`].
pub fn update_person(
    conn: &Connection,
    id: i64,
    draft: &PersonDraft,
    base_updated_at: &str,
) -> Result<PersonSaved> {
    let tx = conn.unchecked_transaction()?;
    let saved = update_person_in(&tx, id, draft, base_updated_at)?;
    tx.commit()?;
    Ok(saved)
}

// --- reading one person -----------------------------------------------------------------------------------

fn labels_of(conn: &Connection, id: i64) -> Result<Vec<LabelRef>> {
    let mut stmt = conn.prepare(
        "SELECT l.id, l.name FROM person_labels pl JOIN contact_labels l ON l.id = pl.label_id \
          WHERE pl.person_id = ?1 ORDER BY l.name_key",
    )?;
    let rows = stmt
        .query_map(params![id], |r| {
            Ok(LabelRef {
                id: r.get(0)?,
                name: r.get(1)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// A person with everything: details, labels, last met, sources and projects, and the first page
/// of their timeline. Works for a person in Trash too (the Trash list opens their record).
pub fn get_person(conn: &Connection, id: i64) -> Result<Person> {
    let mut person = get_person_core(conn, id)?;
    person.timeline = timeline::first_page(conn, id)?;
    Ok(person)
}

/// [`get_person`] without the timeline page (it comes back empty). The export and import loops
/// read a whole address book through this: a page of notes per person would be loaded and dropped.
pub(crate) fn get_person_core(conn: &Connection, id: i64) -> Result<Person> {
    type Row = (
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        Option<String>,
        String,
        bool,
        Option<String>,
        Option<String>,
        String,
        String,
    );
    let row: Option<Row> = conn
        .query_row(
            "SELECT uid, display_name, given_name, middle_name, family_name, name_prefix, \
                    name_suffix, birthday, met_where, met_on, notes, starred, merged_uids, \
                    trashed_at, created_at, updated_at FROM people WHERE id = ?1",
            params![id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                    r.get(10)?,
                    r.get(11)?,
                    r.get(12)?,
                    r.get(13)?,
                    r.get(14)?,
                    r.get(15)?,
                ))
            },
        )
        .optional()?;
    let Some((
        ident,
        display_name,
        given,
        middle,
        family,
        prefix,
        suffix,
        born_on,
        met_where,
        met_on,
        notes,
        starred,
        merged,
        trashed_at,
        created_at,
        updated_at,
    )) = row
    else {
        return Err(no_such_person());
    };
    let mut stmt = conn.prepare(
        "SELECT id, kind, label, value, parts, from_date, to_date, is_current, position \
           FROM person_details WHERE person_id = ?1 ORDER BY kind, position, id",
    )?;
    let details = stmt
        .query_map(params![id], |r| {
            let parts: Option<String> = r.get(4)?;
            Ok(Detail {
                id: r.get(0)?,
                kind: r.get(1)?,
                label: r.get(2)?,
                value: r.get(3)?,
                parts: parts.and_then(|t| serde_json::from_str(&t).ok()),
                from_date: r.get(5)?,
                to_date: r.get(6)?,
                is_current: r.get(7)?,
                position: r.get(8)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(stmt);
    let mut stmt = conn.prepare(
        "SELECT project_name FROM person_projects WHERE person_id = ?1 ORDER BY project_name",
    )?;
    let projects = stmt
        .query_map(params![id], |r| r.get(0))?
        .collect::<std::result::Result<Vec<String>, _>>()?;
    drop(stmt);
    Ok(Person {
        id,
        ident,
        display_name,
        given_name: given.unwrap_or_default(),
        middle_name: middle.unwrap_or_default(),
        family_name: family.unwrap_or_default(),
        name_prefix: prefix.unwrap_or_default(),
        name_suffix: suffix.unwrap_or_default(),
        born_on,
        met_where,
        met_on,
        notes,
        starred,
        folded_idents: sources::parse_ident_list(merged.as_deref()),
        trashed_at,
        created_at,
        updated_at,
        details,
        labels: labels_of(conn, id)?,
        last_met: timeline::last_met(conn, id)?,
        timeline: timeline::TimelinePage {
            entries: Vec::new(),
            next: None,
        },
        sources: sources::sources_of(conn, id)?,
        projects,
    })
}

// --- lists and search ---------------------------------------------------------------------------------

/// The digit key of a query (or one word of it) that asks for a phone number: only digits and phone
/// punctuation, at least three digits in all. A shorter one ("12") matches names and emails only,
/// so it can't flood the list with every number that happens to hold it.
fn phone_shaped(text: &str) -> Option<String> {
    if !text
        .chars()
        .all(|c| fold::ascii_digit(c).is_some() || matches!(c, '+' | '-' | '(' | ')' | '.' | ' '))
    {
        return None;
    }
    let key = fold::digits_key(text);
    if key.chars().filter(char::is_ascii_digit).count() >= 3 {
        Some(key)
    } else {
        None
    }
}

/// The SQL that ANDs the query's folded words over `people.search_key` (aliased `p`), with its
/// parameters pushed in order. Empty when the query holds no word.
fn search_clause(query: &str, params: &mut Vec<SqlValue>) -> String {
    const TEXT: &str = "substr(p.search_key, 1, instr(p.search_key || char(31), char(31)) - 1)";
    const PHONES: &str = "substr(p.search_key, instr(p.search_key || char(31), char(31)) + 1)";
    let folded = fold_key(query);
    let words: Vec<&str> = folded
        .split(' ')
        .filter(|w| !w.is_empty())
        .take(SEARCH_WORDS_MAX)
        .collect();
    let mut clauses = Vec::new();
    for word in &words {
        params.push(SqlValue::Text(word.to_string()));
        match phone_shaped(word) {
            Some(digits) => {
                params.push(SqlValue::Text(digits));
                clauses.push(format!("(instr({TEXT}, ?) > 0 OR instr({PHONES}, ?) > 0)"));
            }
            None => clauses.push(format!("instr({TEXT}, ?) > 0")),
        }
    }
    let joined = clauses.join(" AND ");
    // A number typed with spaces ("+44 20 7946 0123") has words too short to be phone words on
    // their own, so the query is also judged as a whole: its digits, run together, may match a
    // phone. The per-word match stays as the other branch, so a name and a number still combine.
    if words.len() > 1 {
        if let Some(digits) = phone_shaped(&words.join(" ")) {
            params.push(SqlValue::Text(digits));
            return format!("(({joined}) OR instr({PHONES}, ?) > 0)");
        }
    }
    joined
}

fn query_rows(
    conn: &Connection,
    filter: ListFilter,
    query: Option<&str>,
    sort: ListSort,
    limit: Option<usize>,
) -> Result<Vec<PersonRow>> {
    let mut values: Vec<SqlValue> = Vec::new();
    let mut wheres: Vec<String> = Vec::new();
    match filter {
        ListFilter::All => wheres.push("p.trashed_at IS NULL".into()),
        ListFilter::Starred => wheres.push("p.trashed_at IS NULL AND p.starred = 1".into()),
        ListFilter::Label(label_id) => {
            wheres.push(
                "p.trashed_at IS NULL AND EXISTS \
                 (SELECT 1 FROM person_labels pl WHERE pl.person_id = p.id AND pl.label_id = ?)"
                    .into(),
            );
            values.push(SqlValue::Integer(label_id));
        }
        ListFilter::Unlabelled => wheres.push(
            "p.trashed_at IS NULL AND NOT EXISTS \
             (SELECT 1 FROM person_labels pl WHERE pl.person_id = p.id)"
                .into(),
        ),
        ListFilter::Trash => wheres.push("p.trashed_at IS NOT NULL".into()),
    }
    if let Some(q) = query {
        let clause = search_clause(q, &mut values);
        if !clause.is_empty() {
            wheres.push(clause);
        }
    }
    let order = match sort {
        ListSort::RecentlyMet => "lm.last_met IS NULL, lm.last_met DESC, p.sort_key, p.id",
        // `Last` is ordered below, in Rust, by family name; this is its tiebreak.
        ListSort::First | ListSort::Last => "p.sort_key, p.id",
    };
    let first_of = |kind: &str| {
        format!(
            "(SELECT d.value FROM person_details d WHERE d.person_id = p.id AND d.kind = '{kind}' \
               ORDER BY d.position, d.id LIMIT 1)"
        )
    };
    let sql = format!(
        "SELECT p.id, p.uid, p.display_name, p.starred, p.trashed_at, p.family_name, p.sort_key, \
                {email}, {phone}, {company}, lm.last_met, \
                (SELECT s.state FROM person_sources s \
                  WHERE s.person_id = p.id AND s.provider = 'google' ORDER BY s.id LIMIT 1), \
                p.birthday \
           FROM people p \
           LEFT JOIN (SELECT ip.person_id AS pid, MAX(i.on_date) AS last_met \
                        FROM interaction_people ip JOIN interactions i ON i.id = ip.interaction_id \
                       WHERE i.kind = 'met' GROUP BY ip.person_id) lm ON lm.pid = p.id \
          WHERE {wheres} ORDER BY {order}{limit}",
        email = first_of(KIND_EMAIL),
        phone = first_of(KIND_PHONE),
        company = first_of(KIND_WORK),
        wheres = wheres.join(" AND "),
        limit = match (sort, limit) {
            // `Last` re-sorts after the read, so it can only be cut after.
            (ListSort::Last, _) | (_, None) => String::new(),
            (_, Some(n)) => format!(" LIMIT {n}"),
        },
    );
    struct Raw {
        row: PersonRow,
        family_key: String,
    }
    let mut stmt = conn.prepare(&sql)?;
    let mut raws = stmt
        .query_map(params_from_iter(values.iter()), |r| {
            let family: Option<String> = r.get(5)?;
            let sort_key: String = r.get(6)?;
            let email: Option<String> = r.get(7)?;
            let phone: Option<String> = r.get(8)?;
            Ok(Raw {
                family_key: match family.as_deref().map(fold_key) {
                    Some(k) if !k.is_empty() => k,
                    _ => sort_key,
                },
                row: PersonRow {
                    id: r.get(0)?,
                    ident: r.get(1)?,
                    display_name: r.get(2)?,
                    primary: email.or(phone),
                    company: r.get(9)?,
                    labels: Vec::new(),
                    starred: r.get(3)?,
                    last_met: r.get(10)?,
                    google_state: r.get(11)?,
                    trashed_at: r.get(4)?,
                    born_on: r.get(12)?,
                },
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(stmt);
    if sort == ListSort::Last {
        // The one sort that folds at read time: the v63 schema stores only `sort_key` (the folded
        // display name), no folded family name. A change to `fold_key` therefore also changes this
        // order until the next write of each person; a stored family key is a schema change.
        // Stable: people with the same family name keep their first-name order.
        raws.sort_by(|a, b| a.family_key.cmp(&b.family_key));
        if let Some(n) = limit {
            raws.truncate(n);
        }
    }
    let mut by_person: HashMap<i64, Vec<LabelRef>> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT pl.person_id, l.id, l.name FROM person_labels pl \
           JOIN contact_labels l ON l.id = pl.label_id ORDER BY l.name_key",
    )?;
    let pairs = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            LabelRef {
                id: r.get(1)?,
                name: r.get(2)?,
            },
        ))
    })?;
    for pair in pairs {
        let (person, label) = pair?;
        by_person.entry(person).or_default().push(label);
    }
    drop(stmt);
    Ok(raws
        .into_iter()
        .map(|mut raw| {
            raw.row.labels = by_person.remove(&raw.row.id).unwrap_or_default();
            raw.row
        })
        .collect())
}

/// The list the People tab shows: one filter, an optional search, one sort. People in Trash appear
/// under `Trash` and nowhere else. `last_met` comes from one grouped subquery over the timeline.
pub fn list_people(
    conn: &Connection,
    filter: ListFilter,
    query: Option<&str>,
    sort: ListSort,
) -> Result<Vec<PersonRow>> {
    query_rows(conn, filter, query, sort, None)
}

/// Find people for Ctrl K and the pickers: the folded words of `query` ANDed over names, nicknames,
/// emails and companies; a word of three or more digits also matches phone numbers. Never returns
/// a person in Trash. A blank query returns the first `limit` people alphabetically.
pub fn search_people(conn: &Connection, query: &str, limit: usize) -> Result<Vec<PersonRow>> {
    query_rows(
        conn,
        ListFilter::All,
        Some(query),
        ListSort::First,
        Some(limit.clamp(1, 200)),
    )
}

// --- star ---------------------------------------------------------------------------------------------------

/// Star or unstar people. Returns how many changed.
pub fn star_people(conn: &Connection, ids: &[i64], starred: bool) -> Result<usize> {
    in_tx(conn, |tx| star_people_in(tx, ids, starred))
}

/// [`star_people`] in the caller's transaction.
pub fn star_people_in(tx: &Connection, ids: &[i64], starred: bool) -> Result<usize> {
    require_transaction(tx)?;
    let mut changed = Vec::new();
    for id in existing_ids(tx, ids)? {
        let n = tx.execute(
            "UPDATE people SET starred = ?2 WHERE id = ?1 AND starred <> ?2",
            params![id, starred],
        )?;
        if n > 0 {
            changed.push(id);
        }
    }
    touch_people(tx, &changed)?;
    Ok(changed.len())
}

// --- labels ---------------------------------------------------------------------------------------------------

fn clean_label_name(name: &str) -> Result<String> {
    let cleaned = clean_text(name);
    if cleaned.is_empty() {
        return Err(refuse("A label needs a name."));
    }
    check_len("label name", &cleaned)?;
    Ok(cleaned)
}

/// Every label with how many people (outside Trash) carry it, by name.
pub fn list_labels(conn: &Connection) -> Result<Vec<Label>> {
    let mut stmt = conn.prepare(
        "SELECT l.id, l.uid, l.name, \
                (SELECT COUNT(*) FROM person_labels pl JOIN people p ON p.id = pl.person_id \
                  WHERE pl.label_id = l.id AND p.trashed_at IS NULL) \
           FROM contact_labels l ORDER BY l.name_key",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Label {
                id: r.get(0)?,
                ident: r.get(1)?,
                name: r.get(2)?,
                people: r.get(3)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Make a label. Names are unique by their folded form (Google rejects two groups with one name),
/// so "Family" and "family" are one label: this refuses the second.
pub fn create_label(conn: &Connection, name: &str) -> Result<i64> {
    in_tx(conn, |tx| create_label_in(tx, name))
}

/// [`create_label`] in the caller's transaction.
pub fn create_label_in(tx: &Connection, name: &str) -> Result<i64> {
    require_transaction(tx)?;
    let name = clean_label_name(name)?;
    let key = fold_key(&name);
    let taken: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM contact_labels WHERE name_key = ?1)",
        params![key],
        |r| r.get(0),
    )?;
    if taken {
        return Err(refuse("A label with that name already exists."));
    }
    tx.execute(
        "INSERT INTO contact_labels(uid, name, name_key, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![new_ident()?, name, key, now(tx)?],
    )?;
    Ok(tx.last_insert_rowid())
}

/// The label a name means, made if it doesn't exist yet (a typed new label).
pub fn ensure_label(conn: &Connection, name: &str) -> Result<i64> {
    in_tx(conn, |tx| ensure_label_in(tx, name))
}

/// [`ensure_label`] in the caller's transaction (an import's labels).
pub fn ensure_label_in(tx: &Connection, name: &str) -> Result<i64> {
    require_transaction(tx)?;
    let key = fold_key(&clean_label_name(name)?);
    let existing: Option<i64> = tx
        .query_row(
            "SELECT id FROM contact_labels WHERE name_key = ?1",
            params![key],
            |r| r.get(0),
        )
        .optional()?;
    match existing {
        Some(id) => Ok(id),
        None => create_label_in(tx, name),
    }
}

/// Rename a label. Refuses a name another label already holds; changing only the case of its own
/// name is fine. The people who carry it change too (their label name is part of the record), so
/// their `updated_at` moves like any other label write.
pub fn rename_label(conn: &Connection, label_id: i64, name: &str) -> Result<()> {
    in_tx(conn, |tx| rename_label_in(tx, label_id, name))
}

/// [`rename_label`] in the caller's transaction.
pub fn rename_label_in(tx: &Connection, label_id: i64, name: &str) -> Result<()> {
    require_transaction(tx)?;
    let name = clean_label_name(name)?;
    let key = fold_key(&name);
    let clash: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM contact_labels WHERE name_key = ?1 AND id <> ?2)",
        params![key, label_id],
        |r| r.get(0),
    )?;
    if clash {
        return Err(refuse("A label with that name already exists."));
    }
    let n = tx.execute(
        "UPDATE contact_labels SET name = ?2, name_key = ?3 WHERE id = ?1",
        params![label_id, name, key],
    )?;
    if n == 0 {
        return Err(refuse("That label isn't in PM any more."));
    }
    touch_people(tx, &carriers_of(tx, label_id)?)?;
    Ok(())
}

/// Everyone who carries a label (in Trash or not).
fn carriers_of(tx: &Connection, label_id: i64) -> Result<Vec<i64>> {
    let mut stmt = tx.prepare("SELECT person_id FROM person_labels WHERE label_id = ?1")?;
    let rows = stmt
        .query_map(params![label_id], |r| r.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Delete a label. Never deletes a person: the people who carried it just lose it. Returns how
/// many people carried it.
pub fn delete_label(conn: &Connection, label_id: i64) -> Result<usize> {
    in_tx(conn, |tx| delete_label_in(tx, label_id))
}

/// [`delete_label`] in the caller's transaction.
pub fn delete_label_in(tx: &Connection, label_id: i64) -> Result<usize> {
    require_transaction(tx)?;
    let carriers = carriers_of(tx, label_id)?;
    tx.execute(
        "DELETE FROM contact_labels WHERE id = ?1",
        params![label_id],
    )?;
    touch_people(tx, &carriers)?;
    Ok(carriers.len())
}

fn label_exists(conn: &Connection, label_id: i64) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM contact_labels WHERE id = ?1)",
        params![label_id],
        |r| r.get(0),
    )?)
}

/// Put a label on people. A person who already carries it is skipped; one already at
/// [`LABELS_PER_PERSON_MAX`] labels refuses the whole call (nothing is written). Returns how many
/// people gained the label.
pub fn label_people(conn: &Connection, ids: &[i64], label_id: i64) -> Result<usize> {
    in_tx(conn, |tx| label_people_in(tx, ids, label_id))
}

/// [`label_people`] in the caller's transaction.
pub fn label_people_in(tx: &Connection, ids: &[i64], label_id: i64) -> Result<usize> {
    require_transaction(tx)?;
    if !label_exists(tx, label_id)? {
        return Err(refuse("That label isn't in PM any more."));
    }
    let mut added = Vec::new();
    for id in existing_ids(tx, ids)? {
        let has: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM person_labels WHERE person_id = ?1 AND label_id = ?2)",
            params![id, label_id],
            |r| r.get(0),
        )?;
        if has {
            continue;
        }
        let held: i64 = tx.query_row(
            "SELECT COUNT(*) FROM person_labels WHERE person_id = ?1",
            params![id],
            |r| r.get(0),
        )?;
        if held >= LABELS_PER_PERSON_MAX as i64 {
            return Err(refuse(format!(
                "A person can carry up to {LABELS_PER_PERSON_MAX} labels."
            )));
        }
        tx.execute(
            "INSERT INTO person_labels(person_id, label_id) VALUES (?1, ?2)",
            params![id, label_id],
        )?;
        added.push(id);
    }
    touch_people(tx, &added)?;
    Ok(added.len())
}

/// Take a label off people. Returns how many carried it.
pub fn unlabel_people(conn: &Connection, ids: &[i64], label_id: i64) -> Result<usize> {
    in_tx(conn, |tx| unlabel_people_in(tx, ids, label_id))
}

/// [`unlabel_people`] in the caller's transaction.
pub fn unlabel_people_in(tx: &Connection, ids: &[i64], label_id: i64) -> Result<usize> {
    require_transaction(tx)?;
    let mut removed = Vec::new();
    for id in existing_ids(tx, ids)? {
        let n = tx.execute(
            "DELETE FROM person_labels WHERE person_id = ?1 AND label_id = ?2",
            params![id, label_id],
        )?;
        if n > 0 {
            removed.push(id);
        }
    }
    touch_people(tx, &removed)?;
    Ok(removed.len())
}

// --- project links --------------------------------------------------------------------------------------------

/// Link a person to a project by the name typed. The name is first resolved to the project it
/// means (`tasks::canonical_project`: "atlas" finds "Atlas"), and a project that doesn't exist yet
/// gets its row (`projects::ensure_named`), so the foreign key holds and a case variant never makes
/// a second project. Returns the canonical name that was stored.
pub fn link_person_project(conn: &Connection, id: i64, typed: &str) -> Result<String> {
    in_tx(conn, |tx| link_person_project_in(tx, id, typed))
}

/// [`link_person_project`] in the caller's transaction.
pub fn link_person_project_in(tx: &Connection, id: i64, typed: &str) -> Result<String> {
    require_transaction(tx)?;
    let typed = clean_text(typed);
    if typed.is_empty() {
        return Err(refuse("Name the project to link."));
    }
    check_len("project name", &typed)?;
    if !person_exists(tx, id)? {
        return Err(no_such_person());
    }
    let canonical = crate::tasks::canonical_project(tx, &typed)?;
    crate::projects::ensure_named(tx, &canonical)?;
    let n = tx.execute(
        "INSERT OR IGNORE INTO person_projects(person_id, project_name, created_at) \
         VALUES (?1, ?2, ?3)",
        params![id, canonical, now(tx)?],
    )?;
    if n > 0 {
        touch_people(tx, &[id])?;
    }
    Ok(canonical)
}

/// Unlink a person from a project, by the name the record shows. A link that isn't there is not an
/// error. The project itself is untouched.
pub fn unlink_person_project(conn: &Connection, id: i64, project: &str) -> Result<()> {
    in_tx(conn, |tx| unlink_person_project_in(tx, id, project))
}

/// [`unlink_person_project`] in the caller's transaction.
pub fn unlink_person_project_in(tx: &Connection, id: i64, project: &str) -> Result<()> {
    require_transaction(tx)?;
    if !person_exists(tx, id)? {
        return Err(no_such_person());
    }
    let n = tx.execute(
        "DELETE FROM person_projects WHERE person_id = ?1 AND project_name = ?2",
        params![id, project],
    )?;
    if n > 0 {
        touch_people(tx, &[id])?;
    }
    Ok(())
}

// --- Trash, purge --------------------------------------------------------------------------------------------

/// Move people to Trash. `google_delete` names Google sources (by `person_sources.id`) whose
/// contact the user also asked to delete at Google (P2's tick): each linked source with a Google
/// id becomes `pending_delete`. It is always empty in PR A. A source that doesn't belong to one of
/// these people is refused (nothing is written). A person already in Trash keeps the date they went
/// in, so trashing twice never extends the 30 days. Returns how many people moved.
pub fn trash_people(conn: &Connection, ids: &[i64], google_delete: &[i64]) -> Result<usize> {
    in_tx(conn, |tx| trash_people_in(tx, ids, google_delete))
}

/// [`trash_people`] in the caller's transaction.
pub fn trash_people_in(tx: &Connection, ids: &[i64], google_delete: &[i64]) -> Result<usize> {
    require_transaction(tx)?;
    let people = existing_ids(tx, ids)?;
    let stamp = now(tx)?;
    let mut moved = Vec::new();
    for &id in &people {
        let n = tx.execute(
            "UPDATE people SET trashed_at = ?2 WHERE id = ?1 AND trashed_at IS NULL",
            params![id, stamp],
        )?;
        if n > 0 {
            moved.push(id);
        }
    }
    for &source_id in google_delete {
        let row: Option<(Option<i64>, String, String, bool)> = tx
            .query_row(
                "SELECT person_id, provider, state, external_id IS NOT NULL \
                   FROM person_sources WHERE id = ?1",
                params![source_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((owner, provider, state, has_external)) = row else {
            return Err(refuse("That Google link isn't in PM any more."));
        };
        if !owner.is_some_and(|o| people.contains(&o)) {
            return Err(refuse("That Google link doesn't belong to these people."));
        }
        // Nothing to delete at Google for a link that never reached it (or is already handled).
        if provider == PROVIDER_GOOGLE && state == STATE_LINKED && has_external {
            tx.execute(
                "UPDATE person_sources SET state = ?2 WHERE id = ?1",
                params![source_id, STATE_PENDING_DELETE],
            )?;
        }
    }
    touch_people(tx, &moved)?;
    Ok(moved.len())
}

/// Bring people back from Trash. A `pending_delete` source that hasn't been sent goes back to
/// `linked` (the delete is cancelled); a delete Google already confirmed left a tombstone, which
/// stays, so the person returns with no Google link. Returns how many people came back.
pub fn restore_people(conn: &Connection, ids: &[i64]) -> Result<usize> {
    in_tx(conn, |tx| restore_people_in(tx, ids))
}

/// [`restore_people`] in the caller's transaction (an import that brings someone back from Trash).
pub fn restore_people_in(tx: &Connection, ids: &[i64]) -> Result<usize> {
    require_transaction(tx)?;
    let mut back = Vec::new();
    for id in existing_ids(tx, ids)? {
        let n = tx.execute(
            "UPDATE people SET trashed_at = NULL WHERE id = ?1 AND trashed_at IS NOT NULL",
            params![id],
        )?;
        if n > 0 {
            back.push(id);
            tx.execute(
                "UPDATE person_sources SET state = ?2 WHERE person_id = ?1 AND state = ?3",
                params![id, STATE_LINKED, STATE_PENDING_DELETE],
            )?;
        }
    }
    touch_people(tx, &back)?;
    Ok(back.len())
}

/// Delete people for good, in the caller's transaction (spec 2.2, steps 1 to 5):
///
/// 1. collect the interactions they took part in (through a temp table, not one huge `IN`), since
///    the cascade in step 3 removes those links;
/// 2. [`sources::retire`]: file sources go, Google sources become tombstones of ids;
/// 3. delete the people (the FK nulls `person_id` on the tombstones and cascades details, labels,
///    project links and `interaction_people`);
/// 4. delete, of the interactions from step 1, only those left with nobody;
/// 5. delete the "not the same person" pairs naming a removed uid.
///
/// Returns how many people were deleted. Unknown ids are skipped.
pub fn delete_people_in(tx: &Connection, ids: &[i64]) -> Result<usize> {
    require_transaction(tx)?;
    tx.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS people_doomed(id INTEGER PRIMARY KEY); \
         CREATE TEMP TABLE IF NOT EXISTS people_doomed_ix(id INTEGER PRIMARY KEY); \
         DELETE FROM people_doomed; DELETE FROM people_doomed_ix;",
    )?;
    {
        let mut stmt =
            tx.prepare("INSERT OR IGNORE INTO people_doomed SELECT id FROM people WHERE id = ?1")?;
        for id in ids {
            stmt.execute(params![id])?;
        }
    }
    // Step 1.
    tx.execute(
        "INSERT OR IGNORE INTO people_doomed_ix \
         SELECT DISTINCT interaction_id FROM interaction_people \
          WHERE person_id IN (SELECT id FROM people_doomed)",
        [],
    )?;
    let doomed: Vec<i64> = {
        let mut stmt = tx.prepare("SELECT id FROM people_doomed")?;
        let rows = stmt
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    let mut idents: Vec<String> = Vec::new();
    for id in &doomed {
        if let Some(all) = sources::idents_of(tx, *id)? {
            idents.extend(all);
        }
    }
    // Step 2.
    sources::retire(tx, &doomed)?;
    // Step 3.
    let removed = tx.execute(
        "DELETE FROM people WHERE id IN (SELECT id FROM people_doomed)",
        [],
    )?;
    // Step 4.
    tx.execute(
        "DELETE FROM interactions WHERE id IN (SELECT id FROM people_doomed_ix) \
           AND NOT EXISTS (SELECT 1 FROM interaction_people ip WHERE ip.interaction_id = interactions.id)",
        [],
    )?;
    // Step 5.
    {
        let mut stmt =
            tx.prepare("DELETE FROM person_distinct_pairs WHERE low_uid = ?1 OR high_uid = ?1")?;
        for ident in &idents {
            stmt.execute(params![ident])?;
        }
    }
    tx.execute_batch("DELETE FROM people_doomed; DELETE FROM people_doomed_ix;")?;
    Ok(removed)
}

/// Delete people for good ("Delete now" in Trash), in one transaction. See [`delete_people_in`].
///
/// Only people already in Trash are deleted: this is irreversible and the command takes ids from
/// the webview, so a stale selection can never destroy a live record. A live or unknown id is
/// skipped, and the count is of people actually deleted. (A merge removes its losers through
/// [`delete_people_in`], which has no such limit.)
pub fn delete_people_now(conn: &Connection, ids: &[i64]) -> Result<usize> {
    in_tx(conn, |tx| {
        let mut trashed = Vec::new();
        for id in existing_ids(tx, ids)? {
            let in_trash: bool = tx.query_row(
                "SELECT trashed_at IS NOT NULL FROM people WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )?;
            if in_trash {
                trashed.push(id);
            }
        }
        delete_people_in(tx, &trashed)
    })
}

/// Purge everyone who went into Trash more than [`TRASH_DAYS`] days before `now` (an ISO UTC
/// instant), in one transaction. Returns how many were removed.
pub fn purge_expired(conn: &Connection, now: &str) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    let expired: Vec<i64> = {
        let mut stmt = tx.prepare(
            "SELECT id FROM people WHERE trashed_at IS NOT NULL \
               AND trashed_at < strftime('%Y-%m-%dT%H:%M:%fZ', ?1, ?2)",
        )?;
        let rows = stmt
            .query_map(params![now, format!("-{TRASH_DAYS} days")], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    if expired.is_empty() {
        return Ok(0);
    }
    let n = delete_people_in(&tx, &expired)?;
    tx.commit()?;
    Ok(n)
}

/// The purge a store runs when it opens: [`purge_expired`] as of the store's own clock.
pub fn purge_on_open(conn: &Connection) -> Result<usize> {
    purge_expired(conn, &now(conn)?)
}
