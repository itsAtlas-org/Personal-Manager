// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! vCard import (spec 5.2): the preview, the held parse and the commit.
//!
//! **Two steps, so a wrong file is harmless.** The command opens the picker, reads the file with a
//! cap, parses it *outside* the DB guard, and [`ImportHold`] keeps that one parse in memory for
//! ten minutes under a `preview_id`, bound to the store epoch (a new preview replaces it). Nothing
//! is written. [`commit_held`] then runs the whole import as **one transaction**: all of it or
//! none. A preview that is stale (replaced, expired) or from another store refuses in plain words
//! and writes nothing.
//!
//! **Matching each card, in order:** its `UID` against `people.uid`, then against the uids folded
//! into someone by a merge, then against `vcard` sources' `external_id`. A hit is that person (the
//! survivor, for a folded id), and re-importing PM's own export adds nothing. A card whose `UID`
//! already appeared earlier in the same file is a new person with no id on its source row, so the
//! unique index is never hit. An email or phone match is *not* a match: it only counts toward "K
//! look like people you have", and the duplicate review handles it afterwards. A match on someone
//! in Trash brings them back.
//!
//! **A match updates additively** and never overwrites: blank fields fill, new values join, and
//! where a single value differs PM keeps its own and the result lists it. Details are compared with
//! the merge's own rule ([`merge::plan_details`]), so "the same email" means the same thing here
//! and there.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use super::card::flat_parts;
use super::card::{Card, CardDetail};
use super::dupes::{phone_group, phones_match, DupeInput};
use super::fold::{digits_key, email_key, value_key};
use super::merge::{plan_details, DetailFacts, Fate};
use super::partial_date;
use super::sources::{merged_ident_owners, person_for_own_ident, person_for_source};
use super::vcard::{network_of_url, VcfParse, Warning, FILE_CAP};
use super::{
    create_person_in, ensure_label_in, get_person_core, in_tx, new_ident, now, refuse,
    require_transaction, restore_people_in, update_person_in, DetailDraft, PersonDraft,
    DETAILS_MAX, LABELS_PER_PERSON_MAX, PROVIDER_VCARD, STATE_LINKED,
};
use crate::error::Result;

/// How long a held preview stays good.
pub const PREVIEW_HOLD: Duration = Duration::from_secs(10 * 60);

/// What a refusal of an old or foreign preview says.
pub const STALE: &str =
    "That file was read a while ago, or PM has changed since. Pick the file again.";

/// What a file over the cap says: photos are what make a contacts file this big.
pub const TOO_BIG: &str =
    "This file is over 128 MB, almost always because of photos. Export again without photos and import that.";

// --- reading the file ------------------------------------------------------------------------------------------

/// Read a file, refusing one over [`FILE_CAP`] without reading past it.
pub fn read_capped(path: &Path) -> Result<Vec<u8>> {
    let cannot = || refuse("PM couldn't open that file.");
    let file = std::fs::File::open(path).map_err(|_| cannot())?;
    let meta = file.metadata().map_err(|_| cannot())?;
    if !meta.is_file() {
        return Err(refuse("That isn't a file."));
    }
    if meta.len() > FILE_CAP as u64 {
        return Err(refuse(TOO_BIG));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
    file.take(FILE_CAP as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| cannot())?;
    if bytes.len() > FILE_CAP {
        return Err(refuse(TOO_BIG));
    }
    Ok(bytes)
}

// --- the held parse ----------------------------------------------------------------------------------------------

/// One card of the held parse. `repeated` marks a card whose `UID` an earlier card of the file
/// already used: it imports as a new person with no id on its source row.
pub struct HeldCard {
    pub card: Card,
    pub repeated: bool,
}

/// A parse, ready to preview and hold.
pub struct ParsedFile {
    pub cards: Vec<HeldCard>,
    pub complete: bool,
    pub warnings: BTreeMap<Warning, usize>,
    pub ignored: BTreeMap<String, usize>,
    /// How many cards repeated an earlier card's id.
    pub repeated: usize,
}

/// Mark the repeated ids of a parse.
pub fn prepare(parse: VcfParse) -> ParsedFile {
    let mut seen: HashSet<String> = HashSet::new();
    let mut repeated = 0;
    let cards = parse
        .cards
        .into_iter()
        .map(|card| {
            let again = card
                .ident
                .as_ref()
                .is_some_and(|ident| !seen.insert(ident.clone()));
            if again {
                repeated += 1;
            }
            HeldCard {
                card,
                repeated: again,
            }
        })
        .collect();
    ParsedFile {
        cards,
        complete: parse.complete,
        warnings: parse.warnings,
        ignored: parse.ignored,
        repeated,
    }
}

struct Held {
    id: String,
    epoch: u64,
    at: Instant,
    cards: Arc<Vec<HeldCard>>,
}

/// The one preview PM keeps in memory (in `AppState`; always locked while holding, or after, the DB
/// guard). Holds contact values, so it dies with the store: [`ImportHold::clear`] runs whenever the
/// store changes.
#[derive(Default)]
pub struct ImportHold {
    entry: Option<Held>,
}

impl std::fmt::Debug for ImportHold {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportHold")
            .field("held", &self.entry.is_some())
            .finish()
    }
}

impl ImportHold {
    /// Hold a parse for the store open now (`epoch`), replacing any earlier one. Returns its id.
    pub fn put(&mut self, epoch: u64, cards: Vec<HeldCard>, at: Instant) -> Result<String> {
        // The new parse replaces whatever is held, expired or not, so there is no eviction here;
        // `get` is where an expired parse is let go. An id that can't be made keeps the old entry.
        let id = new_ident()?;
        self.entry = Some(Held {
            id: id.clone(),
            epoch,
            at,
            cards: Arc::new(cards),
        });
        Ok(id)
    }

    /// The held cards, if `id` is the one held, from this store, and fresh. Anything else is the
    /// same plain refusal.
    pub fn get(&mut self, id: &str, epoch: u64, at: Instant) -> Result<Arc<Vec<HeldCard>>> {
        // An expired parse is let go here, not kept in memory for the rest of the session.
        self.evict_expired(at);
        match &self.entry {
            Some(h) if h.id == id && h.epoch == epoch => Ok(Arc::clone(&h.cards)),
            _ => Err(refuse(STALE)),
        }
    }

    /// Let go of the held parse when it is over [`PREVIEW_HOLD`] old: it holds contact values.
    fn evict_expired(&mut self, at: Instant) {
        if self
            .entry
            .as_ref()
            .is_some_and(|h| at.saturating_duration_since(h.at) >= PREVIEW_HOLD)
        {
            self.entry = None;
        }
    }

    /// Let go of the preview `id` (after it was imported, or when its dialog was dismissed).
    pub fn finish(&mut self, id: &str) {
        if self.entry.as_ref().is_some_and(|h| h.id == id) {
            self.entry = None;
        }
    }

    /// Drop everything held: the store changed.
    pub fn clear(&mut self) {
        self.entry = None;
    }
}

// --- matching ------------------------------------------------------------------------------------------------------

struct Matched {
    id: i64,
    trashed: bool,
    /// Found through a `vcard` source's id (its snapshot is refreshed), not through a PM uid.
    by_source: bool,
}

/// `merged` is [`merged_ident_owners`], read once per preview or commit: asking the JSON column
/// per card would scan every person for every card of a big file.
fn match_card(
    conn: &Connection,
    merged: &HashMap<String, i64>,
    ident: &str,
) -> Result<Option<Matched>> {
    let trashed_of = |id: i64| -> Result<bool> {
        Ok(conn.query_row(
            "SELECT trashed_at IS NOT NULL FROM people WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )?)
    };
    if let Some((id, trashed)) = person_for_own_ident(conn, ident)? {
        return Ok(Some(Matched {
            id,
            trashed,
            by_source: false,
        }));
    }
    if let Some(&id) = merged.get(ident) {
        return Ok(Some(Matched {
            id,
            trashed: trashed_of(id)?,
            by_source: false,
        }));
    }
    if let Some(id) = person_for_source(conn, PROVIDER_VCARD, "", ident)? {
        let trashed = trashed_of(id)?;
        return Ok(Some(Matched {
            id,
            trashed,
            by_source: true,
        }));
    }
    Ok(None)
}

/// Whether a new card's email or phone is one a person outside Trash already has. A phone is
/// matched by the duplicate finder's own rule ([`phones_match`]: a national number against its
/// international form), so this count and the review the import ends with agree.
///
/// `phones` is [`stored_phones`], read once per preview: a lookup per phone of every card would
/// scan every stored phone each time (a `substr` of the key cannot use the index).
fn looks_like_someone(
    conn: &Connection,
    phones: &HashMap<String, Vec<String>>,
    card: &Card,
) -> Result<bool> {
    for d in &card.details {
        match d.kind.as_str() {
            "email" => {
                let key = email_key(&d.value);
                if key.is_empty() {
                    continue;
                }
                let hit: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM person_details d JOIN people p ON p.id = d.person_id \
                                    WHERE d.kind = 'email' AND d.value_key = ?1 AND p.trashed_at IS NULL)",
                    params![key],
                    |r| r.get(0),
                )?;
                if hit {
                    return Ok(true);
                }
            }
            "phone" => {
                let key = digits_key(&d.value);
                if key.is_empty() {
                    continue;
                }
                // The candidates are the stored keys in the finder's group for this number; the
                // finder's own rule confirms.
                if phones
                    .get(&phone_group(&key))
                    .is_some_and(|have| have.iter().any(|h| phones_match(h, &key)))
                {
                    return Ok(true);
                }
            }
            _ => {}
        }
    }
    Ok(false)
}

/// Every phone key of a person outside Trash, grouped the way the duplicate finder groups them.
fn stored_phones(conn: &Connection) -> Result<HashMap<String, Vec<String>>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT d.value_key FROM person_details d \
         JOIN people p ON p.id = d.person_id \
         WHERE d.kind = 'phone' AND p.trashed_at IS NULL AND d.value_key <> ''",
    )?;
    let keys = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();
    for key in keys {
        groups.entry(phone_group(&key)).or_default().push(key);
    }
    Ok(groups)
}

/// What the preview counts from the store.
#[derive(Clone, Copy, Default)]
pub struct Counts {
    /// Cards that are already someone in PM (by id).
    pub already: usize,
    /// Of those, people in Trash, who come back.
    pub from_trash: usize,
    /// New cards that look like someone PM has.
    pub look_alike: usize,
}

/// Count what an import would do, writing nothing. Hold the DB guard for this only.
pub fn preview_counts(conn: &Connection, cards: &[HeldCard]) -> Result<Counts> {
    let mut counts = Counts::default();
    let merged = merged_ident_owners(conn)?;
    let phones = stored_phones(conn)?;
    for held in cards {
        let matched = match (&held.card.ident, held.repeated) {
            (Some(ident), false) => match_card(conn, &merged, ident)?,
            _ => None,
        };
        match matched {
            Some(m) => {
                counts.already += 1;
                if m.trashed {
                    counts.from_trash += 1;
                }
            }
            None => {
                if looks_like_someone(conn, &phones, &held.card)? {
                    counts.look_alike += 1;
                }
            }
        }
    }
    Ok(counts)
}

// --- what the webview is told ----------------------------------------------------------------------------------

/// A warning kind and how many times it came up.
#[derive(Clone, Serialize)]
pub struct WarningCount {
    pub kind: String,
    pub count: usize,
}

/// An ignored property name and how many times.
#[derive(Clone, Serialize)]
pub struct IgnoredCount {
    pub name: String,
    pub count: usize,
}

/// The preview dialog's facts (spec 5.2 and 9.2). Counts and property names; the file's name is the
/// only text of the user's own.
#[derive(Clone, Serialize)]
pub struct ImportPreview {
    pub preview_id: String,
    pub file_name: String,
    /// People in the file.
    pub people: usize,
    /// Already in PM.
    pub already: usize,
    /// Of those, in Trash: they come back.
    pub from_trash: usize,
    /// New ones that look like people PM has.
    pub look_alike: usize,
    /// Cards skipped for having no name, email or phone.
    pub no_name: usize,
    /// Cards that repeated an earlier card's id.
    pub repeated: usize,
    pub photos_skipped: bool,
    /// False when the file was cut short or hit a limit.
    pub complete: bool,
    pub warnings: Vec<WarningCount>,
    pub ignored: Vec<IgnoredCount>,
}

impl std::fmt::Debug for ImportPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportPreview")
            .field("people", &self.people)
            .finish_non_exhaustive()
    }
}

impl ImportPreview {
    /// The preview of a parse held under `preview_id`.
    pub fn of(
        preview_id: String,
        file_name: String,
        parsed: &ParsedFile,
        counts: Counts,
    ) -> ImportPreview {
        ImportPreview {
            preview_id,
            file_name,
            people: parsed.cards.len(),
            already: counts.already,
            from_trash: counts.from_trash,
            look_alike: counts.look_alike,
            no_name: parsed.warnings.get(&Warning::NoName).copied().unwrap_or(0),
            repeated: parsed.repeated,
            photos_skipped: parsed.ignored.contains_key("PHOTO"),
            complete: parsed.complete,
            warnings: parsed
                .warnings
                .iter()
                .map(|(w, n)| WarningCount {
                    kind: w.as_str().to_string(),
                    count: *n,
                })
                .collect(),
            ignored: parsed
                .ignored
                .iter()
                .map(|(name, n)| IgnoredCount {
                    name: name.clone(),
                    count: *n,
                })
                .collect(),
        }
    }
}

/// What the import dialog sends back.
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub struct ImportOptions {
    /// A label to put on everyone in the file.
    pub label: Option<String>,
}

impl std::fmt::Debug for ImportOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportOptions")
            .field("label", &self.label.is_some())
            .finish()
    }
}

/// A person PM kept its own values for, and which.
#[derive(Clone, Serialize)]
pub struct KeptNote {
    pub name: String,
    /// "name", "birthday", "where you met" or "notes".
    pub fields: Vec<String>,
}

/// What an import did (spec 5.2).
#[derive(Clone, Default, Serialize)]
pub struct ImportResult {
    /// New people.
    pub added: usize,
    /// People already in PM that gained something.
    pub updated: usize,
    /// People already in PM that gained nothing.
    pub unchanged: usize,
    /// The names of people who came back from Trash.
    pub restored: Vec<String>,
    pub kept: Vec<KeptNote>,
    /// People in this import who now have a possible duplicate ("Review possible duplicates").
    pub review: usize,
    /// Everyone created or matched, for the review count. Never sent.
    #[serde(skip)]
    pub touched: Vec<i64>,
}

impl std::fmt::Debug for ImportResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportResult")
            .field("added", &self.added)
            .field("updated", &self.updated)
            .field("unchanged", &self.unchanged)
            .finish_non_exhaustive()
    }
}

/// How many of the people in `touched` appear in a pair of the duplicate scan of `input`.
pub fn review_count(input: &DupeInput, touched: &[i64]) -> usize {
    let report = super::dupes::scan(input);
    let wanted: HashSet<i64> = touched.iter().copied().collect();
    let mut hit: HashSet<i64> = HashSet::new();
    for pair in &report.pairs {
        for id in [pair.a, pair.b] {
            if wanted.contains(&id) {
                hit.insert(id);
            }
        }
    }
    hit.len()
}

// --- the additive update of someone already in PM --------------------------------------------------------------

/// What applying a card to a person did.
struct Applied {
    changed: bool,
    kept: Vec<&'static str>,
}

/// A detail as the match sees it. A street-only address has no parts here, on either side, because
/// that is how a file reads one back ([`flat_parts`]).
fn facts_of(d: &CardDetail, id: i64, position: i64) -> DetailFacts {
    let parts = flat_parts(&d.kind, d.parts.as_ref());
    DetailFacts {
        id,
        kind: d.kind.clone(),
        label: d.label.clone(),
        value: d.value.clone(),
        value_key: value_key(&d.kind, &d.value, parts.as_ref()),
        parts,
        from_date: d.from_date.clone(),
        to_date: d.to_date.clone(),
        is_current: d.is_current,
        position,
    }
}

/// Fill a blank text field from the card; where both hold a value and they differ, note it.
fn fill_text(have: &mut String, card: &str, changed: &mut bool, differs: &mut bool) {
    if card.is_empty() {
        return;
    }
    if have.is_empty() {
        *have = card.to_string();
        *changed = true;
    } else if have != card {
        *differs = true;
    }
}

/// Whether two birthdays are the same one: equal, or the card's has no year and the same month and
/// day (it knows less, it doesn't disagree).
fn same_born_on(have: &str, card: &str) -> bool {
    have == card || (card.starts_with("--") && have.ends_with(&card[1..]))
}

/// Apply a card to a person already in PM, additively (spec 5.2): blank fields fill, new values
/// join, a differing single value keeps PM's and is noted. The person is saved only if something
/// changed, so re-importing a file that matches changes nothing.
fn apply_card(tx: &Connection, id: i64, card: &Card) -> Result<Applied> {
    let person = get_person_core(tx, id)?;
    let mut draft = PersonDraft {
        display_name: person.display_name.clone(),
        given_name: person.given_name.clone(),
        middle_name: person.middle_name.clone(),
        family_name: person.family_name.clone(),
        name_prefix: person.name_prefix.clone(),
        name_suffix: person.name_suffix.clone(),
        born_on: person.born_on.clone(),
        leap_day: person.leap_day,
        met_where: person.met_where.clone(),
        met_on: person.met_on.clone(),
        notes: person.notes.clone(),
        starred: person.starred,
        details: person
            .details
            .iter()
            .map(|d| DetailDraft {
                id: Some(d.id),
                kind: d.kind.clone(),
                label: d.label.clone(),
                value: d.value.clone(),
                parts: d.parts.clone(),
                from_date: d.from_date.clone(),
                to_date: d.to_date.clone(),
                is_current: d.is_current,
            })
            .collect(),
    };
    let mut changed = false;
    let mut kept: Vec<&'static str> = Vec::new();

    let mut parts_differ = false;
    for (have, theirs) in [
        (&mut draft.given_name, &card.given_name),
        (&mut draft.middle_name, &card.middle_name),
        (&mut draft.family_name, &card.family_name),
        (&mut draft.name_prefix, &card.name_prefix),
        (&mut draft.name_suffix, &card.name_suffix),
    ] {
        fill_text(have, theirs, &mut changed, &mut parts_differ);
    }
    // PM keeps the name it has; the result says so when the file's was another.
    if !card.display_name.is_empty() && (parts_differ || card.display_name != person.display_name) {
        kept.push("name");
    }

    match (&draft.born_on, &card.born_on) {
        (None, Some(theirs)) => {
            draft.born_on = Some(theirs.clone());
            draft.leap_day = card.leap_day;
            changed = true;
        }
        (Some(have), Some(theirs)) if !same_born_on(have, theirs) => kept.push("birthday"),
        // The same birthday: an answer PM doesn't hold yet fills in (the save drops it again
        // unless the birthday is 29 February).
        (Some(_), Some(_)) if draft.leap_day.is_none() && card.leap_day.is_some() => {
            let filled = partial_date::kept_leap_day(draft.born_on.as_deref(), card.leap_day);
            if filled.is_some() {
                draft.leap_day = filled;
                changed = true;
            }
        }
        _ => {}
    }

    let mut met_differs = false;
    fill_text(
        &mut draft.met_where,
        &card.met_where,
        &mut changed,
        &mut met_differs,
    );
    match (&draft.met_on, &card.met_on) {
        (None, Some(theirs)) => {
            draft.met_on = Some(theirs.clone());
            changed = true;
        }
        (Some(have), Some(theirs)) if have != theirs => met_differs = true,
        _ => {}
    }
    if met_differs {
        kept.push("where you met");
    }

    let mut notes_differs = false;
    if !card.notes.trim().is_empty() {
        if draft.notes.trim().is_empty() {
            draft.notes = card.notes.clone();
            changed = true;
        } else if draft.notes.trim() != card.notes.trim() {
            notes_differs = true;
        }
    }
    if notes_differs {
        kept.push("notes");
    }

    // Details: the ones PM already has stay (taking only what fills a blank), the rest join.
    let existing: Vec<DetailFacts> = person
        .details
        .iter()
        .map(|d| {
            let parts = flat_parts(&d.kind, d.parts.as_ref());
            DetailFacts {
                id: d.id,
                kind: d.kind.clone(),
                label: d.label.clone(),
                value: d.value.clone(),
                value_key: value_key(&d.kind, &d.value, parts.as_ref()),
                parts,
                from_date: d.from_date.clone(),
                to_date: d.to_date.clone(),
                is_current: d.is_current,
                position: d.position,
            }
        })
        .collect();
    let theirs: Vec<DetailFacts> = card
        .details
        .iter()
        .enumerate()
        .map(|(i, d)| facts_of(d, 0, i64::try_from(i).unwrap_or(0)))
        .collect();
    let plan = plan_details(&existing, &theirs);
    for (d, fate) in card.details.iter().zip(plan.fates) {
        match fate {
            Fate::Move => {
                if draft.details.len() < DETAILS_MAX {
                    let mut new = d.to_draft();
                    new.id = None;
                    draft.details.push(new);
                    changed = true;
                }
            }
            Fate::Drop(at, fill) => {
                let row = &mut draft.details[at];
                // Only what fills a blank. Whether a job is current is PM's to say, not a file's.
                // "Website" is what a plain `URL:` line is called when the file names nothing
                // better: PM writes a link with no label that way, so reading it back must not
                // make a label out of it (that would "change" the person on every re-import).
                // The same goes for the name of the network the URL itself is on ("LinkedIn"):
                // the reader derives that from the address when the file names no label.
                if let Some(label) = fill.label.filter(|l| {
                    !(row.kind == "web"
                        && (l == "Website"
                            || network_of_url(&row.value).is_some_and(|n| n.label == l.as_str())))
                }) {
                    row.label = label;
                    changed = true;
                }
                if let Some(from) = fill.from_date {
                    row.from_date = Some(from);
                    changed = true;
                }
                if let Some(parts) = fill.parts {
                    row.parts = Some(parts);
                    changed = true;
                }
            }
        }
    }

    if changed {
        update_person_in(tx, id, &draft, &person.updated_at)?;
    }
    Ok(Applied { changed, kept })
}

/// Whether a person can carry one more label.
fn label_room(tx: &Connection, person_id: i64) -> Result<bool> {
    let held: i64 = tx.query_row(
        "SELECT COUNT(*) FROM person_labels WHERE person_id = ?1",
        params![person_id],
        |r| r.get(0),
    )?;
    Ok(held < LABELS_PER_PERSON_MAX as i64)
}

/// Put a label on a person unless they already carry it or hold the most a person may.
fn attach_label(tx: &Connection, person_id: i64, label_id: i64) -> Result<bool> {
    let held: i64 = tx.query_row(
        "SELECT COUNT(*) FROM person_labels WHERE person_id = ?1",
        params![person_id],
        |r| r.get(0),
    )?;
    let has: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM person_labels WHERE person_id = ?1 AND label_id = ?2)",
        params![person_id, label_id],
        |r| r.get(0),
    )?;
    if has || held >= LABELS_PER_PERSON_MAX as i64 {
        return Ok(false);
    }
    super::label_people_in(tx, &[person_id], label_id)?;
    Ok(true)
}

// --- the commit ---------------------------------------------------------------------------------------------------

/// Import the held cards in the caller's transaction (spec 5.2): new people get a `vcard` source
/// row holding the card as its snapshot; matches update additively; the optional label goes on
/// everyone in the file. Everything or nothing: any refusal rolls the whole file back.
pub fn commit_in(
    tx: &Connection,
    cards: &[HeldCard],
    options: &ImportOptions,
) -> Result<ImportResult> {
    require_transaction(tx)?;
    let label_id = options
        .label
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|name| ensure_label_in(tx, name))
        .transpose()?;
    let stamp = now(tx)?;
    let merged = merged_ident_owners(tx)?;
    let mut result = ImportResult::default();
    for held in cards {
        let mut card = held.card.clone();
        if held.repeated {
            card.ident = None;
        }
        let matched = match &card.ident {
            Some(ident) => match_card(tx, &merged, ident)?,
            None => None,
        };
        // Set when a person already in PM gained nothing from the card's fields, so a label they
        // gain below can still move them from "unchanged" to "updated".
        let mut counted_unchanged = false;
        let id = match matched {
            Some(m) => {
                if m.trashed {
                    restore_people_in(tx, &[m.id])?;
                    result
                        .restored
                        .push(get_person_core(tx, m.id)?.display_name);
                }
                let applied = apply_card(tx, m.id, &card)?;
                if applied.changed {
                    result.updated += 1;
                } else {
                    result.unchanged += 1;
                    counted_unchanged = true;
                }
                if !applied.kept.is_empty() {
                    result.kept.push(KeptNote {
                        name: get_person_core(tx, m.id)?.display_name,
                        fields: applied.kept.iter().map(|s| (*s).to_string()).collect(),
                    });
                }
                if m.by_source {
                    if let Some(ident) = &card.ident {
                        tx.execute(
                            "UPDATE person_sources SET snapshot = ?1, synced_at = ?2 \
                              WHERE provider = ?3 AND account = '' AND external_id = ?4",
                            params![snapshot_of(&card)?, stamp, PROVIDER_VCARD, ident],
                        )?;
                    }
                }
                m.id
            }
            None => {
                let id = create_person_in(tx, &card.to_draft())?;
                tx.execute(
                    "INSERT INTO person_sources(person_id, provider, account, external_id, \
                                                snapshot, state, synced_at) \
                     VALUES (?1, ?2, '', ?3, ?4, ?5, ?6)",
                    params![
                        id,
                        PROVIDER_VCARD,
                        card.ident,
                        snapshot_of(&card)?,
                        STATE_LINKED,
                        stamp
                    ],
                )?;
                result.added += 1;
                id
            }
        };
        let mut gained = false;
        for name in &card.labels {
            // A label is made only if this person has room to carry it, so a full person never
            // leaves behind a label nobody holds.
            if label_room(tx, id)? {
                let label = ensure_label_in(tx, name)?;
                gained |= attach_label(tx, id, label)?;
            }
        }
        if let Some(label) = label_id {
            gained |= attach_label(tx, id, label)?;
        }
        if gained && counted_unchanged {
            result.unchanged -= 1;
            result.updated += 1;
        }
        result.touched.push(id);
    }
    Ok(result)
}

/// The card as a source row keeps it. A failure says nothing of the card.
fn snapshot_of(card: &Card) -> Result<String> {
    serde_json::to_string(card).map_err(|_| refuse("PM couldn't keep a copy of that contact."))
}

/// Import the preview `preview_id` as one transaction, if it is the one held, from this store
/// (`epoch`) and fresh. A stale or foreign preview refuses and writes nothing. The hold is let go
/// of only after the commit.
pub fn commit_held(
    conn: &Connection,
    hold: &Mutex<ImportHold>,
    epoch: u64,
    preview_id: &str,
    options: &ImportOptions,
    at: Instant,
) -> Result<ImportResult> {
    let cards = hold
        .lock()
        .map_err(|_| refuse(STALE))?
        .get(preview_id, epoch, at)?;
    let result = in_tx(conn, |tx| commit_in(tx, &cards, options))?;
    if let Ok(mut h) = hold.lock() {
        h.finish(preview_id);
    }
    Ok(result)
}

/// Forget the preview `preview_id`: its dialog closed without importing, so the parse (contact
/// values, in memory) goes now rather than at its ten minutes. Another preview, a newer one the
/// user has since picked, is left alone; so is an id nothing holds.
pub fn discard_held(hold: &Mutex<ImportHold>, preview_id: &str) {
    if let Ok(mut h) = hold.lock() {
        h.finish(preview_id);
    }
}
