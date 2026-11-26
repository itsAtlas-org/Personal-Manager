// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Merging two people into one, and taking the merge back (spec 4.3).
//!
//! One person (the **survivor**) keeps their `id` and `uid`; the other (the **folded** person) is
//! deleted once everything of theirs has moved: details (de-duplicated by [`plan_details`]), labels,
//! timeline entries, project links, every source link (so the next sync can't re-create the
//! duplicate), the "not the same person" pairs naming them (rewritten onto the survivor) and the
//! folded `uid`s, which the survivor records in `merged_uids`. All of it is one transaction.
//!
//! **A merge never silently drops a value.** Where a single-value field (name, birthday, where and
//! when you met, notes) differs on both sides the caller must choose; the one not chosen is dropped,
//! and the Undo strip is the safety net. [`differences`] and [`plan_details`] are the one rule for
//! "what differs" and "would anything be lost", shared with the duplicate finder's "Nothing
//! differs" group, so the group's promise ("merge with nothing lost") is the merge's own test.
//!
//! **Undo** is held in memory only ([`UndoShelf`], in `AppState`): a before-image of every row the
//! merge touched, an Undo token and the store epoch, for [`UNDO_HOLD`]. It is offered only while the
//! survivor's `updated_at` still equals what the merge wrote and the same store is open; any edit
//! since refuses it ("This merge can no longer be undone."). A restart only loses the offer.
//!
//! No contact value reaches an error string, a log or a `Debug` print: refusals are fixed words and
//! the types that hold rows print counts only.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use rusqlite::types::Value as SqlValue;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::fold::fold_key;
use super::sources::parse_ident_list;
use super::{
    empty_to_none, in_tx, new_ident, no_such_person, now, refuse, rekey_person,
    require_transaction, touch_people, DETAILS_MAX, LABELS_PER_PERSON_MAX, NOTES_MAX,
};
use crate::error::{Error, Result};

/// How long the server keeps a merge's before-image: the strip offers Undo for 30 seconds, and a
/// click in the last of them must still land.
pub const UNDO_HOLD: Duration = Duration::from_secs(35);
/// How many merges' before-images are kept at once (the oldest go first).
pub const UNDO_SHELF_MAX: usize = 20;
/// The words for an Undo that is too late (an edit since, another store, the time up).
pub const NO_LONGER: &str = "This merge can no longer be undone.";

/// A note longer than this is compared by a fingerprint in the duplicate scan, so the scan of a
/// big address book never holds every note in memory. The merge itself reads the real text.
const BIG_NOTE_BYTES: usize = 4_096;

// --- what a person is, for comparing ---------------------------------------------------------------

/// One stored detail, as the merge rules read it.
#[derive(Clone)]
pub(crate) struct DetailFacts {
    pub id: i64,
    pub kind: String,
    pub label: String,
    pub value: String,
    pub parts: Option<Value>,
    pub from_date: Option<String>,
    pub to_date: Option<String>,
    pub is_current: bool,
    pub position: i64,
    pub value_key: String,
}

/// A person as the merge rules and the duplicate finder read them. Holds contact values: no
/// `Debug`.
pub(crate) struct Facts {
    pub id: i64,
    pub ident: String,
    pub display_name: String,
    pub given: String,
    pub middle: String,
    pub family: String,
    pub prefix: String,
    pub suffix: String,
    pub born_on: Option<String>,
    pub met_where: String,
    pub met_on: Option<String>,
    /// The note, or (a scan only, past [`BIG_NOTE_BYTES`]) a fingerprint that starts with a NUL,
    /// which no real note can (notes are cleaned of control characters).
    pub notes: String,
    pub starred: bool,
    pub created_at: String,
    pub trashed: bool,
    pub details: Vec<DetailFacts>,
}

impl Facts {
    /// Whether `notes` is a fingerprint of a long note rather than the text.
    pub fn notes_are_fingerprint(&self) -> bool {
        self.notes.starts_with('\u{0}')
    }
}

fn note_form(text: String, whole: bool) -> String {
    if whole || text.len() <= BIG_NOTE_BYTES {
        return text;
    }
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    format!("\u{0}{}:{:016x}", text.len(), h.finish())
}

fn parts_of(text: Option<String>) -> Option<Value> {
    text.and_then(|t| serde_json::from_str(&t).ok())
}

/// People matching `filter` (a `WHERE` fragment on `people p`, with `args`), with their details.
/// `whole_notes` keeps every note's text; without it a long one is fingerprinted.
fn load_where(
    conn: &Connection,
    filter: &str,
    args: &[SqlValue],
    whole_notes: bool,
) -> Result<Vec<Facts>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT p.id, p.uid, p.display_name, p.given_name, p.middle_name, p.family_name, \
                p.name_prefix, p.name_suffix, p.birthday, p.met_where, p.met_on, p.notes, \
                p.starred, p.created_at, p.trashed_at IS NOT NULL \
           FROM people p WHERE {filter} ORDER BY p.id"
    ))?;
    let mut people: Vec<Facts> = Vec::new();
    let mut rows = stmt.query(params_from_iter(args.iter()))?;
    while let Some(r) = rows.next()? {
        let notes: String = r.get(11)?;
        people.push(Facts {
            id: r.get(0)?,
            ident: r.get(1)?,
            display_name: r.get(2)?,
            given: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
            middle: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
            family: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
            prefix: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
            suffix: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
            born_on: r.get(8)?,
            met_where: r.get(9)?,
            met_on: r.get(10)?,
            notes: note_form(notes, whole_notes),
            starred: r.get(12)?,
            created_at: r.get(13)?,
            trashed: r.get(14)?,
            details: Vec::new(),
        });
    }
    drop(rows);
    drop(stmt);
    let at: HashMap<i64, usize> = people.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
    let mut stmt = conn.prepare(&format!(
        "SELECT d.person_id, d.id, d.kind, d.label, d.value, d.parts, d.from_date, d.to_date, \
                d.is_current, d.position, d.value_key \
           FROM person_details d JOIN people p ON p.id = d.person_id WHERE {filter} \
          ORDER BY d.person_id, d.kind, d.position, d.id"
    ))?;
    let mut rows = stmt.query(params_from_iter(args.iter()))?;
    while let Some(r) = rows.next()? {
        let person: i64 = r.get(0)?;
        let Some(&i) = at.get(&person) else { continue };
        people[i].details.push(DetailFacts {
            id: r.get(1)?,
            kind: r.get(2)?,
            label: r.get(3)?,
            value: r.get(4)?,
            parts: parts_of(r.get(5)?),
            from_date: r.get(6)?,
            to_date: r.get(7)?,
            is_current: r.get(8)?,
            position: r.get(9)?,
            value_key: r.get(10)?,
        });
    }
    Ok(people)
}

/// Everyone outside Trash (the duplicate scan's input).
pub(crate) fn load_live(conn: &Connection) -> Result<Vec<Facts>> {
    load_where(conn, "p.trashed_at IS NULL", &[], false)
}

/// One person, Trash included, with every note in full.
pub(crate) fn load_one(conn: &Connection, id: i64) -> Result<Option<Facts>> {
    Ok(
        load_where(conn, "p.id = ?1", &[SqlValue::Integer(id)], true)?
            .into_iter()
            .next(),
    )
}

// --- what differs, and what would be lost ------------------------------------------------------------

/// Which single-value fields differ between two people: both sides hold a value and they aren't
/// the same. A field one side leaves blank never differs (the other's value just fills it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Differs {
    /// The display name or a name part.
    pub name: bool,
    #[serde(rename = "birthday")]
    pub born_on: bool,
    /// Where you met or when.
    pub met: bool,
    pub notes: bool,
}

impl Differs {
    pub fn any(&self) -> bool {
        self.name || self.born_on || self.met || self.notes
    }
}

fn both_differ(a: &str, b: &str) -> bool {
    !a.is_empty() && !b.is_empty() && a != b
}

fn both_differ_opt(a: &Option<String>, b: &Option<String>) -> bool {
    matches!((a, b), (Some(x), Some(y)) if x != y)
}

/// The single-value fields that differ between `a` and `b` (see [`Differs`]).
pub(crate) fn differences(a: &Facts, b: &Facts) -> Differs {
    Differs {
        name: a.display_name != b.display_name
            || both_differ(&a.given, &b.given)
            || both_differ(&a.middle, &b.middle)
            || both_differ(&a.family, &b.family)
            || both_differ(&a.prefix, &b.prefix)
            || both_differ(&a.suffix, &b.suffix),
        born_on: both_differ_opt(&a.born_on, &b.born_on),
        met: both_differ(&a.met_where, &b.met_where) || both_differ_opt(&a.met_on, &b.met_on),
        notes: both_differ(a.notes.trim(), b.notes.trim()),
    }
}

/// What happens to one of the folded person's details.
#[derive(Clone)]
pub(crate) enum Fate {
    /// It moves to the survivor as a row of its own.
    Move,
    /// It is the same detail as the survivor's `into`th: it is dropped, and `Fill` carries what the
    /// survivor's row gains from it.
    Drop(usize, Fill),
}

/// What a dropped duplicate hands to the row that stays: only blanks are filled.
#[derive(Clone, Default)]
pub(crate) struct Fill {
    pub label: Option<String>,
    pub from_date: Option<String>,
    pub to_date: Option<String>,
    pub is_current: bool,
    pub parts: Option<Value>,
}

impl Fill {
    fn is_empty(&self) -> bool {
        self.label.is_none()
            && self.from_date.is_none()
            && self.to_date.is_none()
            && !self.is_current
            && self.parts.is_none()
    }
}

/// How the folded person's details land on the survivor.
pub(crate) struct DetailPlan {
    /// One per detail of the folded person, in their order.
    pub fates: Vec<Fate>,
    /// How many are dropped as duplicates.
    pub dropped: usize,
    /// Every dropped duplicate says nothing the kept row doesn't (same text, nothing to add or
    /// contradict), so dropping it loses nothing.
    pub lossless: bool,
}

/// The identity two details share to count as the same (spec 4.3): the kind and a **non-empty**
/// `value_key`, and for work, school and address their structured part too (a job's title, a
/// school's field, the address's parts) with the from date for work and school. A row with an empty
/// key has no identity and is never de-duplicated.
fn identity(d: &DetailFacts) -> Option<String> {
    if d.value_key.is_empty() {
        return None;
    }
    let part = |key: &str| -> String {
        d.parts
            .as_ref()
            .and_then(|p| p.get(key))
            .and_then(Value::as_str)
            .map(fold_key)
            .unwrap_or_default()
    };
    let from = d.from_date.as_deref().unwrap_or("");
    let extra = match d.kind.as_str() {
        "work" => format!("{}\u{1f}{from}", part("title")),
        "school" => format!("{}\u{1f}{from}", part("field")),
        "address" => {
            let mut all: Vec<String> = d
                .parts
                .as_ref()
                .and_then(Value::as_object)
                .map(|o| {
                    o.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| format!("{k}={}", fold_key(s))))
                        .filter(|kv| !kv.ends_with('='))
                        .collect()
                })
                .unwrap_or_default();
            all.sort();
            all.join("\u{1f}")
        }
        _ => String::new(),
    };
    Some(format!("{}\u{1f}{}\u{1f}{extra}", d.kind, d.value_key))
}

/// Two non-blank labels that differ keep both rows (spec 4.3): the label is what tells "home" from
/// "work".
fn labels_conflict(a: &str, b: &str) -> bool {
    let (a, b) = (fold_key(a), fold_key(b));
    !a.is_empty() && !b.is_empty() && a != b
}

/// Fill a date: the dropped row's fills a blank; two different ones mean the dropped one is lost.
fn fill_date(kept: &Option<String>, dropped: &Option<String>, lossy: &mut bool) -> Option<String> {
    match (kept, dropped) {
        (None, Some(d)) => Some(d.clone()),
        (Some(k), Some(d)) if k != d => {
            *lossy = true;
            None
        }
        _ => None,
    }
}

fn fill_for(kept: &DetailFacts, dropped: &DetailFacts, lossy: &mut bool) -> Fill {
    let mut fill = Fill::default();
    if kept.value.trim() != dropped.value.trim() {
        // Same key, different text ("A@x.com" and "a@x.com"): the other spelling would be lost.
        *lossy = true;
    }
    if kept.label.trim().is_empty() && !dropped.label.trim().is_empty() {
        fill.label = Some(dropped.label.clone());
    }
    fill.from_date = fill_date(&kept.from_date, &dropped.from_date, lossy);
    fill.to_date = fill_date(&kept.to_date, &dropped.to_date, lossy);
    fill.is_current = dropped.is_current && !kept.is_current;
    if let Some(Value::Object(theirs)) = &dropped.parts {
        let mut ours = match &kept.parts {
            Some(Value::Object(o)) => o.clone(),
            _ => serde_json::Map::new(),
        };
        let mut changed = false;
        for (k, v) in theirs {
            let blank = matches!(v, Value::String(s) if s.trim().is_empty()) || v.is_null();
            if blank {
                continue;
            }
            match ours.get(k) {
                None => {
                    ours.insert(k.clone(), v.clone());
                    changed = true;
                }
                Some(Value::String(s)) if s.trim().is_empty() => {
                    ours.insert(k.clone(), v.clone());
                    changed = true;
                }
                Some(have) if have == v => {}
                Some(_) => *lossy = true,
            }
        }
        if changed {
            fill.parts = Some(Value::Object(ours));
        }
    }
    fill
}

/// How the folded person's details (`from`) land on the survivor's (`into`): which are the same
/// detail (and so dropped, handing over only what fills a blank) and which move as rows of their
/// own. Each of the survivor's rows can absorb one duplicate.
pub(crate) fn plan_details(into: &[DetailFacts], from: &[DetailFacts]) -> DetailPlan {
    let mut taken = vec![false; into.len()];
    let mut fates = Vec::with_capacity(from.len());
    let mut dropped = 0;
    let mut lossless = true;
    for f in from {
        let found = identity(f).and_then(|id| {
            into.iter().enumerate().position(|(i, kept)| {
                !taken[i]
                    && identity(kept).as_deref() == Some(id.as_str())
                    && !labels_conflict(&kept.label, &f.label)
            })
        });
        match found {
            Some(i) => {
                taken[i] = true;
                let mut lossy = false;
                let fill = fill_for(&into[i], f, &mut lossy);
                lossless &= !lossy;
                dropped += 1;
                fates.push(Fate::Drop(i, fill));
            }
            None => fates.push(Fate::Move),
        }
    }
    DetailPlan {
        fates,
        dropped,
        lossless,
    }
}

/// Whether merging `a` and `b` loses nothing at all: no single-value field differs and no dropped
/// duplicate says more than the row that stays. The duplicate finder's "Nothing differs" group.
pub(crate) fn nothing_differs(a: &Facts, b: &Facts) -> bool {
    !differences(a, b).any() && plan_details(&a.details, &b.details).lossless
}

// --- choices and preview ---------------------------------------------------------------------------

/// Which person's value a merge keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum Side {
    /// The one who stays.
    Survivor,
    /// The one being folded in.
    Folded,
}

/// What a merge does with two different notes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum NotesChoice {
    Survivor,
    Folded,
    /// Both, the survivor's first, a blank line between.
    Both,
}

/// The answers a merge needs: one for each single-value field that differs. A missing answer for a
/// field that differs refuses the merge (nothing is dropped unasked).
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(default)]
pub struct MergeChoices {
    /// The display name and every name part, kept together.
    pub name: Option<Side>,
    #[serde(rename = "birthday")]
    pub born_on: Option<Side>,
    /// Where you met and when, kept together.
    pub met: Option<Side>,
    pub notes: Option<NotesChoice>,
}

/// What a merge of two people would do, counted from the rows themselves (spec 4.3). Counts are of
/// the merged record: "everything from both moves to one record".
#[derive(Clone, Copy, Debug, Serialize)]
pub struct MergePreview {
    /// Details on the merged record, after duplicates are folded together.
    pub details: usize,
    /// Details dropped because the survivor already has the same one.
    pub details_dropped: usize,
    pub labels: usize,
    /// Timeline entries (a meeting both were at counts once).
    pub timeline: usize,
    pub projects: usize,
    /// Source links (a vCard file, a Google contact): every one moves, so none is lost.
    pub sources: usize,
    pub differs: Differs,
    /// A merge loses nothing at all (the "Nothing differs" test).
    pub lossless: bool,
}

fn count(conn: &Connection, sql: &str, a: i64, b: i64) -> Result<usize> {
    let n: i64 = conn.query_row(sql, params![a, b], |r| r.get(0))?;
    Ok(usize::try_from(n).unwrap_or(0))
}

fn load_live_pair(conn: &Connection, into: i64, from: i64) -> Result<(Facts, Facts)> {
    if into == from {
        return Err(refuse("Pick two different people to merge."));
    }
    let a = load_one(conn, into)?.ok_or_else(no_such_person)?;
    let b = load_one(conn, from)?.ok_or_else(no_such_person)?;
    if a.trashed || b.trashed {
        return Err(refuse("Restore people from Trash before merging them."));
    }
    Ok((a, b))
}

/// What merging `from` into `into` would do. Read-only, and refuses what the merge would (the same
/// person twice, someone in Trash, someone gone).
pub fn preview(conn: &Connection, into: i64, from: i64) -> Result<MergePreview> {
    let (a, b) = load_live_pair(conn, into, from)?;
    let plan = plan_details(&a.details, &b.details);
    Ok(MergePreview {
        details: a.details.len() + b.details.len() - plan.dropped,
        details_dropped: plan.dropped,
        labels: count(
            conn,
            "SELECT COUNT(DISTINCT label_id) FROM person_labels WHERE person_id IN (?1, ?2)",
            into,
            from,
        )?,
        timeline: count(
            conn,
            "SELECT COUNT(DISTINCT interaction_id) FROM interaction_people \
              WHERE person_id IN (?1, ?2)",
            into,
            from,
        )?,
        projects: count(
            conn,
            "SELECT COUNT(DISTINCT project_name) FROM person_projects WHERE person_id IN (?1, ?2)",
            into,
            from,
        )?,
        sources: count(
            conn,
            "SELECT COUNT(*) FROM person_sources WHERE person_id IN (?1, ?2)",
            into,
            from,
        )?,
        differs: differences(&a, &b),
        lossless: nothing_differs(&a, &b),
    })
}

// --- row images (the before-image) --------------------------------------------------------------------

/// One stored row, column by column, read with `SELECT *` so a column a later change adds (the
/// 29 February answer) is carried without this file naming it.
#[derive(Clone)]
struct RowImage {
    cols: Vec<String>,
    vals: Vec<SqlValue>,
}

fn images(
    conn: &Connection,
    table: &str,
    filter: &str,
    args: &[SqlValue],
) -> Result<Vec<RowImage>> {
    let mut stmt = conn.prepare(&format!("SELECT * FROM {table} WHERE {filter}"))?;
    let cols: Vec<String> = stmt.column_names().map(|c| c.to_string()).collect();
    let mut rows = stmt.query(params_from_iter(args.iter()))?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let mut vals = Vec::with_capacity(cols.len());
        for i in 0..cols.len() {
            vals.push(r.get::<_, SqlValue>(i)?);
        }
        out.push(RowImage {
            cols: cols.clone(),
            vals,
        });
    }
    Ok(out)
}

impl RowImage {
    fn insert(&self, tx: &Connection, table: &str, or_ignore: bool) -> Result<usize> {
        let marks = vec!["?"; self.cols.len()].join(", ");
        let verb = if or_ignore {
            "INSERT OR IGNORE"
        } else {
            "INSERT"
        };
        Ok(tx.execute(
            &format!(
                "{verb} INTO {table}({}) VALUES ({marks})",
                self.cols.join(", ")
            ),
            params_from_iter(self.vals.iter()),
        )?)
    }

    /// Put every column except `id` back on the row with this `id`.
    fn restore_onto(&self, tx: &Connection, table: &str, id: i64) -> Result<()> {
        let sets: Vec<String> = self
            .cols
            .iter()
            .filter(|c| c.as_str() != "id")
            .map(|c| format!("{c} = ?"))
            .collect();
        let mut args: Vec<SqlValue> = self
            .cols
            .iter()
            .zip(&self.vals)
            .filter(|(c, _)| c.as_str() != "id")
            .map(|(_, v)| v.clone())
            .collect();
        args.push(SqlValue::Integer(id));
        tx.execute(
            &format!("UPDATE {table} SET {} WHERE id = ?", sets.join(", ")),
            params_from_iter(args.iter()),
        )?;
        Ok(())
    }
}

/// Everything needed to put one merge back. Holds contact values: no `Debug`, held in memory only.
pub struct UndoStep {
    survivor: i64,
    folded: i64,
    survivor_ident: String,
    folded_ident: String,
    /// The survivor's `updated_at` as the merge wrote it: Undo only while it still holds.
    written_at: String,
    survivor_row: RowImage,
    folded_row: RowImage,
    details: Vec<RowImage>,
    /// Labels the folded person carried, and the ones the survivor gained from them.
    folded_labels: Vec<i64>,
    added_labels: Vec<i64>,
    folded_timeline: Vec<i64>,
    added_timeline: Vec<i64>,
    /// `(project, created_at)`.
    folded_projects: Vec<(String, String)>,
    added_projects: Vec<String>,
    moved_sources: Vec<i64>,
    pairs_before: Vec<RowImage>,
    /// The pair rows the merge wrote, `(low, high)`.
    pairs_created: Vec<(String, String)>,
}

impl std::fmt::Debug for UndoStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UndoStep")
            .field("details", &self.details.len())
            .field("sources", &self.moved_sources.len())
            .finish_non_exhaustive()
    }
}

// --- the merge ----------------------------------------------------------------------------------------

/// What a merge decided, before it writes anything. Every refusal happens while building it, so a
/// batch that skips a pair it can't merge never leaves one half done.
struct Plan {
    into: Facts,
    from: Facts,
    name: [String; 6],
    born_on: Option<String>,
    met_where: String,
    met_on: Option<String>,
    notes: String,
    starred: bool,
    created_at: String,
    folded_idents: Vec<String>,
    details: DetailPlan,
}

fn pick<T: Clone>(side: Side, kept: T, folded: T) -> T {
    match side {
        Side::Survivor => kept,
        Side::Folded => folded,
    }
}

fn need(chosen: bool, what: &str) -> Result<()> {
    if chosen {
        Ok(())
    } else {
        Err(refuse(format!("Choose which {what} to keep.")))
    }
}

/// The non-blank of two texts, the survivor's first.
fn either(a: &str, b: &str) -> String {
    if a.is_empty() {
        b.to_string()
    } else {
        a.to_string()
    }
}

fn plan(conn: &Connection, into: i64, from: i64, choices: &MergeChoices) -> Result<Plan> {
    let (a, b) = load_live_pair(conn, into, from)?;
    let diff = differences(&a, &b);

    // Display name, then prefix, given, middle, family, suffix: one block, chosen whole when the
    // two differ, filled blank by blank when they only complete each other.
    let names = |p: &Facts| {
        [
            p.display_name.clone(),
            p.prefix.clone(),
            p.given.clone(),
            p.middle.clone(),
            p.family.clone(),
            p.suffix.clone(),
        ]
    };
    let (na, nb) = (names(&a), names(&b));
    let name = if diff.name {
        let side = choices.name;
        need(side.is_some(), "name")?;
        pick(side.unwrap_or(Side::Survivor), na, nb)
    } else {
        std::array::from_fn(|i| either(&na[i], &nb[i]))
    };

    let born_on = if diff.born_on {
        need(choices.born_on.is_some(), "birthday")?;
        pick(
            choices.born_on.unwrap_or(Side::Survivor),
            a.born_on.clone(),
            b.born_on.clone(),
        )
    } else {
        a.born_on.clone().or_else(|| b.born_on.clone())
    };

    let (met_where, met_on) = if diff.met {
        need(choices.met.is_some(), "place and date you met")?;
        pick(
            choices.met.unwrap_or(Side::Survivor),
            (a.met_where.clone(), a.met_on.clone()),
            (b.met_where.clone(), b.met_on.clone()),
        )
    } else {
        (
            either(&a.met_where, &b.met_where),
            a.met_on.clone().or_else(|| b.met_on.clone()),
        )
    };

    let notes = if diff.notes {
        match choices.notes {
            None => return Err(refuse("Choose which notes to keep.")),
            Some(NotesChoice::Survivor) => a.notes.clone(),
            Some(NotesChoice::Folded) => b.notes.clone(),
            Some(NotesChoice::Both) => format!("{}\n\n{}", a.notes.trim(), b.notes.trim()),
        }
    } else {
        either(&a.notes, &b.notes)
    };
    if notes.len() > NOTES_MAX {
        return Err(refuse(
            "The two sets of notes together are too long to keep both.",
        ));
    }

    let details = plan_details(&a.details, &b.details);
    if a.details.len() + b.details.len() - details.dropped > DETAILS_MAX {
        return Err(refuse(format!(
            "Together they'd have more than {DETAILS_MAX} details."
        )));
    }
    let labels = count(
        conn,
        "SELECT COUNT(DISTINCT label_id) FROM person_labels WHERE person_id IN (?1, ?2)",
        into,
        from,
    )?;
    if labels > LABELS_PER_PERSON_MAX {
        return Err(refuse(format!(
            "Together they'd carry more than {LABELS_PER_PERSON_MAX} labels."
        )));
    }

    // The uids folded into the survivor so far, then the folded person's own and theirs.
    let stored: Option<String> = conn.query_row(
        "SELECT merged_uids FROM people WHERE id = ?1",
        params![into],
        |r| r.get(0),
    )?;
    let folded_stored: Option<String> = conn.query_row(
        "SELECT merged_uids FROM people WHERE id = ?1",
        params![from],
        |r| r.get(0),
    )?;
    let mut folded_idents = parse_ident_list(stored.as_deref());
    for ident in std::iter::once(b.ident.clone()).chain(parse_ident_list(folded_stored.as_deref()))
    {
        if ident != a.ident && !folded_idents.contains(&ident) {
            folded_idents.push(ident);
        }
    }

    Ok(Plan {
        name,
        born_on,
        met_where,
        met_on,
        notes,
        starred: a.starred || b.starred,
        created_at: if b.created_at < a.created_at {
            b.created_at.clone()
        } else {
            a.created_at.clone()
        },
        folded_idents,
        details,
        into: a,
        from: b,
    })
}

fn ids_of(conn: &Connection, sql: &str, args: &[SqlValue]) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(params_from_iter(args.iter()), |r| r.get(0))?
        .collect::<std::result::Result<Vec<i64>, _>>()?;
    Ok(rows)
}

fn int(v: i64) -> SqlValue {
    SqlValue::Integer(v)
}

fn text(v: &str) -> SqlValue {
    SqlValue::Text(v.to_string())
}

/// Write a plan: one transaction (the caller's), every row re-pointed, the folded person deleted.
fn apply(tx: &Connection, plan: Plan) -> Result<UndoStep> {
    let (into, from) = (plan.into.id, plan.from.id);
    let (into_ident, from_ident) = (plan.into.ident.clone(), plan.from.ident.clone());

    // The before-image, of exactly what this merge is about to change.
    let survivor_row = images(tx, "people", "id = ?1", &[int(into)])?
        .pop()
        .ok_or_else(no_such_person)?;
    let folded_row = images(tx, "people", "id = ?1", &[int(from)])?
        .pop()
        .ok_or_else(no_such_person)?;
    let details = images(
        tx,
        "person_details",
        "person_id IN (?1, ?2) ORDER BY id",
        &[int(into), int(from)],
    )?;
    let pairs_before = images(
        tx,
        "person_distinct_pairs",
        "low_uid IN (?1, ?2) OR high_uid IN (?1, ?2)",
        &[text(&into_ident), text(&from_ident)],
    )?;
    let folded_labels = ids_of(
        tx,
        "SELECT label_id FROM person_labels WHERE person_id = ?1 ORDER BY label_id",
        &[int(from)],
    )?;
    let folded_timeline = ids_of(
        tx,
        "SELECT interaction_id FROM interaction_people WHERE person_id = ?1 ORDER BY interaction_id",
        &[int(from)],
    )?;
    let folded_projects: Vec<(String, String)> = {
        let mut stmt = tx.prepare(
            "SELECT project_name, created_at FROM person_projects WHERE person_id = ?1 \
              ORDER BY project_name",
        )?;
        let rows = stmt
            .query_map(params![from], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    let moved_sources = ids_of(
        tx,
        "SELECT id FROM person_sources WHERE person_id = ?1 ORDER BY id",
        &[int(from)],
    )?;

    // Details: each of the folded person's either moves (after the survivor's own rows of its
    // kind) or is dropped as a duplicate after handing over what fills a blank.
    let stamp = now(tx)?;
    let mut touched_kinds: Vec<String> = Vec::new();
    for (d, fate) in plan.from.details.iter().zip(&plan.details.fates) {
        match fate {
            Fate::Move => {
                tx.execute(
                    "UPDATE person_details SET person_id = ?2, position = ?3 WHERE id = ?1",
                    params![d.id, into, 1_000_000 + d.position],
                )?;
                if !touched_kinds.contains(&d.kind) {
                    touched_kinds.push(d.kind.clone());
                }
            }
            Fate::Drop(i, fill) => {
                if !fill.is_empty() {
                    let kept = &plan.into.details[*i];
                    tx.execute(
                        "UPDATE person_details SET \
                             label = COALESCE(?2, label), from_date = COALESCE(?3, from_date), \
                             to_date = COALESCE(?4, to_date), is_current = (is_current OR ?5), \
                             parts = COALESCE(?6, parts), updated_at = ?7 WHERE id = ?1",
                        params![
                            kept.id,
                            fill.label,
                            fill.from_date,
                            fill.to_date,
                            fill.is_current,
                            fill.parts.as_ref().map(Value::to_string),
                            stamp
                        ],
                    )?;
                }
                tx.execute("DELETE FROM person_details WHERE id = ?1", params![d.id])?;
            }
        }
    }
    for kind in &touched_kinds {
        let ids = ids_of(
            tx,
            "SELECT id FROM person_details WHERE person_id = ?1 AND kind = ?2 \
              ORDER BY position, id",
            &[int(into), text(kind)],
        )?;
        for (position, id) in ids.iter().enumerate() {
            tx.execute(
                "UPDATE person_details SET position = ?2 WHERE id = ?1",
                params![id, position as i64],
            )?;
        }
    }

    // Labels, timeline entries and project links: the union.
    let added_labels = ids_of(
        tx,
        "SELECT label_id FROM person_labels WHERE person_id = ?2 \
            AND label_id NOT IN (SELECT label_id FROM person_labels WHERE person_id = ?1)",
        &[int(into), int(from)],
    )?;
    for label in &added_labels {
        tx.execute(
            "INSERT OR IGNORE INTO person_labels(person_id, label_id) VALUES (?1, ?2)",
            params![into, label],
        )?;
    }
    let added_timeline = ids_of(
        tx,
        "SELECT interaction_id FROM interaction_people WHERE person_id = ?2 \
            AND interaction_id NOT IN (SELECT interaction_id FROM interaction_people WHERE person_id = ?1)",
        &[int(into), int(from)],
    )?;
    for entry in &added_timeline {
        tx.execute(
            "INSERT OR IGNORE INTO interaction_people(interaction_id, person_id) VALUES (?1, ?2)",
            params![entry, into],
        )?;
    }
    let mut added_projects: Vec<String> = Vec::new();
    for (project, created_at) in &folded_projects {
        let n = tx.execute(
            "INSERT OR IGNORE INTO person_projects(person_id, project_name, created_at) \
             VALUES (?1, ?2, ?3)",
            params![into, project, created_at],
        )?;
        if n > 0 {
            added_projects.push(project.clone());
        }
    }

    // Every source link moves: the survivor keeps each provider link, or the next sync re-creates
    // the duplicate.
    tx.execute(
        "UPDATE person_sources SET person_id = ?1 WHERE person_id = ?2",
        params![into, from],
    )?;

    // "Not the same person" pairs naming the folded uid move to the survivor's, ordered low to
    // high, and a pair that would name the survivor twice (the two were marked distinct) is
    // dropped.
    let mut pairs_created: Vec<(String, String)> = Vec::new();
    let naming: Vec<(String, String)> = {
        let mut stmt = tx.prepare(
            "SELECT low_uid, high_uid FROM person_distinct_pairs WHERE low_uid = ?1 OR high_uid = ?1",
        )?;
        let rows = stmt
            .query_map(params![from_ident], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    for (low, high) in naming {
        let created: String = tx.query_row(
            "SELECT created_at FROM person_distinct_pairs WHERE low_uid = ?1 AND high_uid = ?2",
            params![low, high],
            |r| r.get(0),
        )?;
        tx.execute(
            "DELETE FROM person_distinct_pairs WHERE low_uid = ?1 AND high_uid = ?2",
            params![low, high],
        )?;
        let other = if low == from_ident { high } else { low };
        if other == into_ident {
            continue;
        }
        let (l, h) = if into_ident < other {
            (into_ident.clone(), other)
        } else {
            (other, into_ident.clone())
        };
        let n = tx.execute(
            "INSERT OR IGNORE INTO person_distinct_pairs(low_uid, high_uid, created_at) \
             VALUES (?1, ?2, ?3)",
            params![l, h, created],
        )?;
        if n > 0 {
            pairs_created.push((l, h));
        }
    }

    // The survivor's row: the kept values, the folded uids, the earlier start. The 29 February
    // answer (E22, a later slice) travels with the birthday that is kept, so that slice adds its
    // column to this UPDATE beside `birthday`; Undo already carries it (its images are `SELECT *`).
    let [display, prefix, given, middle, family, suffix] = &plan.name;
    tx.execute(
        "UPDATE people SET display_name = ?2, name_prefix = ?3, given_name = ?4, middle_name = ?5, \
                           family_name = ?6, name_suffix = ?7, birthday = ?8, met_where = ?9, \
                           met_on = ?10, notes = ?11, starred = ?12, created_at = ?13, \
                           merged_uids = ?14 \
          WHERE id = ?1",
        params![
            into,
            display,
            empty_to_none(prefix),
            empty_to_none(given),
            empty_to_none(middle),
            empty_to_none(family),
            empty_to_none(suffix),
            plan.born_on,
            plan.met_where,
            plan.met_on,
            plan.notes,
            plan.starred,
            plan.created_at,
            serde_json::to_string(&plan.folded_idents).map_err(|_| refuse("internal error"))?
        ],
    )?;

    // The folded person goes: their remaining label, timeline and project rows cascade, and
    // nothing else points at them any more.
    tx.execute("DELETE FROM people WHERE id = ?1", params![from])?;
    rekey_person(tx, into)?;
    touch_people(tx, &[into])?;
    let written_at: String = tx.query_row(
        "SELECT updated_at FROM people WHERE id = ?1",
        params![into],
        |r| r.get(0),
    )?;

    Ok(UndoStep {
        survivor: into,
        folded: from,
        survivor_ident: into_ident,
        folded_ident: from_ident,
        written_at,
        survivor_row,
        folded_row,
        details,
        folded_labels,
        added_labels,
        folded_timeline,
        added_timeline,
        folded_projects,
        added_projects,
        moved_sources,
        pairs_before,
        pairs_created,
    })
}

/// What a merge answers: who stays, by what name, and how many merges it was.
#[derive(Clone, Serialize)]
pub struct MergeDone {
    /// The survivor's id (of the last merge, for a batch).
    pub survivor: i64,
    pub display_name: String,
    /// How many people were folded in.
    pub merged: usize,
    /// The token that takes it back for [`UNDO_HOLD`], or none if it can't be offered.
    pub undo: Option<String>,
}

impl std::fmt::Debug for MergeDone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MergeDone")
            .field("survivor", &self.survivor)
            .field("merged", &self.merged)
            .finish_non_exhaustive()
    }
}

/// Merge `from` into `into` in the caller's transaction. Returns the Undo step for the shelf.
pub fn merge_in(
    tx: &Connection,
    into: i64,
    from: i64,
    choices: &MergeChoices,
) -> Result<(MergeDone, UndoStep)> {
    require_transaction(tx)?;
    let planned = plan(tx, into, from, choices)?;
    let display_name = planned.name[0].clone();
    let step = apply(tx, planned)?;
    Ok((
        MergeDone {
            survivor: into,
            display_name,
            merged: 1,
            undo: None,
        },
        step,
    ))
}

/// Merge two people, in one transaction. See [`merge_in`].
pub fn merge_people(
    conn: &Connection,
    into: i64,
    from: i64,
    choices: &MergeChoices,
) -> Result<(MergeDone, UndoStep)> {
    in_tx(conn, |tx| merge_in(tx, into, from, choices))
}

/// Merge every pair of `pairs` that loses nothing, in one transaction. The webview's list is not
/// trusted: each pair is re-read and re-judged here, and one that differs, is gone, is in Trash or
/// would break a cap is left alone. The older record (the lower id) stays. A person already folded
/// into another earlier in the batch is followed to their survivor, so three copies of one person
/// become one. Returns how many people were folded in, and every step for one Undo.
pub fn merge_identical_pairs(
    conn: &Connection,
    pairs: &[(i64, i64)],
) -> Result<(MergeDone, Vec<UndoStep>)> {
    in_tx(conn, |tx| {
        let mut gone_into: HashMap<i64, i64> = HashMap::new();
        let mut steps: Vec<UndoStep> = Vec::new();
        let mut last: Option<(i64, String)> = None;
        for &(x, y) in pairs {
            let follow = |mut id: i64| {
                // A chain is at most as long as the batch.
                for _ in 0..=pairs.len() {
                    match gone_into.get(&id) {
                        Some(&next) => id = next,
                        None => break,
                    }
                }
                id
            };
            let (x, y) = (follow(x), follow(y));
            if x == y {
                continue;
            }
            let (into, from) = if x < y { (x, y) } else { (y, x) };
            let ready = match (load_one(tx, into)?, load_one(tx, from)?) {
                (Some(a), Some(b)) => {
                    // "Not the same person" is the user's own answer: a pair they hid is never
                    // merged here, even when an earlier step of this batch (which rewrites the
                    // pair rows) made it reachable through a chain. Read fresh every time.
                    let (low, high) = if a.ident < b.ident {
                        (&a.ident, &b.ident)
                    } else {
                        (&b.ident, &a.ident)
                    };
                    let hidden: bool = tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM person_distinct_pairs \
                          WHERE low_uid = ?1 AND high_uid = ?2)",
                        params![low, high],
                        |r| r.get(0),
                    )?;
                    !hidden && !a.trashed && !b.trashed && nothing_differs(&a, &b)
                }
                _ => false,
            };
            if !ready {
                continue;
            }
            match plan(tx, into, from, &MergeChoices::default()) {
                Ok(p) => {
                    let name = p.name[0].clone();
                    steps.push(apply(tx, p)?);
                    gone_into.insert(from, into);
                    last = Some((into, name));
                }
                // A refusal (a cap, a note too long): this pair stays for the person.
                Err(Error::Other(_)) => continue,
                Err(e) => return Err(e),
            }
        }
        let (survivor, display_name) = last.unwrap_or((0, String::new()));
        Ok((
            MergeDone {
                survivor,
                display_name,
                merged: steps.len(),
                undo: None,
            },
            steps,
        ))
    })
}

// --- Undo ----------------------------------------------------------------------------------------------

/// Put one merge back, in the caller's transaction (spec 4.3). Refuses, writing nothing, unless the
/// survivor is still there with the `updated_at` the merge wrote (no edit, label, star, project
/// change or Trash since) and the folded person's `uid` is still free. The folded person returns
/// with their original `id` and `uid`; the survivor goes back to the row the merge found; details
/// return with their own ids; the timeline entries, labels and project links the merge added to the
/// survivor are taken off again (and no others, so a meeting logged since stays); source links move
/// back by row id without touching their `etag`, `snapshot` or `dirty_at`; the pair rows the merge
/// wrote go and the ones it removed return.
fn undo_step(tx: &Connection, s: &UndoStep) -> Result<()> {
    let still: Option<String> = tx
        .query_row(
            "SELECT updated_at FROM people WHERE id = ?1 AND uid = ?2",
            params![s.survivor, s.survivor_ident],
            |r| r.get(0),
        )
        .optional()?;
    if still.as_deref() != Some(s.written_at.as_str()) {
        return Err(refuse(NO_LONGER));
    }
    let taken: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM people WHERE id = ?1 OR uid = ?2)",
        params![s.folded, s.folded_ident],
        |r| r.get(0),
    )?;
    if taken {
        return Err(refuse(NO_LONGER));
    }

    s.survivor_row.restore_onto(tx, "people", s.survivor)?;
    s.folded_row.insert(tx, "people", false)?;
    tx.execute(
        "DELETE FROM person_details WHERE person_id IN (?1, ?2)",
        params![s.survivor, s.folded],
    )?;
    for d in &s.details {
        d.insert(tx, "person_details", false)?;
    }
    for label in &s.added_labels {
        tx.execute(
            "DELETE FROM person_labels WHERE person_id = ?1 AND label_id = ?2",
            params![s.survivor, label],
        )?;
    }
    // A label (or entry, or project) deleted since can't come back: skip, never fail the Undo.
    for label in &s.folded_labels {
        tx.execute(
            "INSERT OR IGNORE INTO person_labels(person_id, label_id) \
             SELECT ?1, id FROM contact_labels WHERE id = ?2",
            params![s.folded, label],
        )?;
    }
    for entry in &s.added_timeline {
        tx.execute(
            "DELETE FROM interaction_people WHERE person_id = ?1 AND interaction_id = ?2",
            params![s.survivor, entry],
        )?;
    }
    for entry in &s.folded_timeline {
        tx.execute(
            "INSERT OR IGNORE INTO interaction_people(interaction_id, person_id) \
             SELECT id, ?2 FROM interactions WHERE id = ?1",
            params![entry, s.folded],
        )?;
    }
    for project in &s.added_projects {
        tx.execute(
            "DELETE FROM person_projects WHERE person_id = ?1 AND project_name = ?2",
            params![s.survivor, project],
        )?;
    }
    for (project, created_at) in &s.folded_projects {
        tx.execute(
            "INSERT OR IGNORE INTO person_projects(person_id, project_name, created_at) \
             SELECT ?1, name, ?3 FROM projects WHERE name = ?2",
            params![s.folded, project, created_at],
        )?;
    }
    for source in &s.moved_sources {
        tx.execute(
            "UPDATE person_sources SET person_id = ?1 WHERE id = ?2 AND person_id = ?3",
            params![s.folded, source, s.survivor],
        )?;
    }
    for (low, high) in &s.pairs_created {
        tx.execute(
            "DELETE FROM person_distinct_pairs WHERE low_uid = ?1 AND high_uid = ?2",
            params![low, high],
        )?;
    }
    for pair in &s.pairs_before {
        pair.insert(tx, "person_distinct_pairs", true)?;
    }
    Ok(())
}

/// Take back every step of one merge (or batch), newest first, in one transaction: any refusal
/// rolls all of it back. Returns how many people came back.
pub fn undo_steps(conn: &Connection, steps: &[UndoStep]) -> Result<usize> {
    in_tx(conn, |tx| {
        for step in steps.iter().rev() {
            undo_step(tx, step)?;
        }
        Ok(steps.len())
    })
}

/// The merges that can still be taken back: in memory, in `AppState`, never written anywhere. An
/// entry goes when it is used, when [`UNDO_HOLD`] is up, when the store changes, or when 20 newer
/// ones push it out.
#[derive(Default)]
pub struct UndoShelf {
    entries: Vec<ShelfEntry>,
}

struct ShelfEntry {
    token: String,
    epoch: u64,
    expires: Instant,
    steps: Vec<UndoStep>,
}

impl std::fmt::Debug for UndoShelf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UndoShelf")
            .field("merges", &self.entries.len())
            .finish()
    }
}

impl UndoShelf {
    /// Hold `steps` under a new token, for the store open now (`epoch`). `None` for nothing to hold.
    pub fn put(&mut self, epoch: u64, steps: Vec<UndoStep>, at: Instant) -> Option<String> {
        self.entries.retain(|e| e.expires > at && e.epoch == epoch);
        if steps.is_empty() {
            return None;
        }
        let token = new_ident().ok()?;
        self.entries.push(ShelfEntry {
            token: token.clone(),
            epoch,
            expires: at + UNDO_HOLD,
            steps,
        });
        while self.entries.len() > UNDO_SHELF_MAX {
            self.entries.remove(0);
        }
        Some(token)
    }

    /// Put steps back under their own `token` after an Undo failed for a reason that wrote nothing
    /// (a busy or failing database, not a refusal): the offer is still good, so a retry works.
    pub fn put_back(&mut self, token: String, epoch: u64, steps: Vec<UndoStep>, at: Instant) {
        self.entries.retain(|e| e.token != token);
        self.entries.push(ShelfEntry {
            token,
            epoch,
            expires: at + UNDO_HOLD,
            steps,
        });
        while self.entries.len() > UNDO_SHELF_MAX {
            self.entries.remove(0);
        }
    }

    /// Drop everything held: the store changed, so no before-image is of any use.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Take the steps for `token` off the shelf: only for the store they were made in, and only
    /// while they are fresh. A second try with the same token finds nothing.
    pub fn take(&mut self, token: &str, epoch: u64, at: Instant) -> Result<Vec<UndoStep>> {
        let found = self.entries.iter().position(|e| e.token == token);
        let Some(i) = found else {
            return Err(refuse(NO_LONGER));
        };
        let entry = self.entries.remove(i);
        if entry.epoch != epoch || entry.expires <= at {
            return Err(refuse(NO_LONGER));
        }
        Ok(entry.steps)
    }
}
