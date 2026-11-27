// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! [`Card`]: the provider-neutral shape of one contact, and the one place a person is turned into a
//! card and a card into a [`PersonDraft`] (spec 2.1 and 5).
//!
//! A vCard file is parsed into cards (`vcard`), a person is read into a card to be exported
//! (`export`), and the Google PR will map Google's contacts to and from the same shape. Everything
//! that crosses the file boundary goes through here, so what an export writes is exactly what an
//! import reads back (`export_then_import_is_identity`). A card is also what a `vcard` source row
//! keeps as its snapshot: the copy of the source's contact at the last agreement.
//!
//! A card holds contact values, so it has no `Debug`; its `PartialEq` is for tests, which compare
//! with `assert!` and never print one.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::fold::{clean_multiline, clean_text, fold_key};
use super::{
    compose_display_name, partial_date, DetailDraft, LeapDay, NameParts, Person, PersonDraft,
    DETAILS_MAX, DETAIL_KINDS, LABELS_PER_PERSON_MAX, NOTES_MAX, PARTS_MAX_BYTES, VALUE_MAX,
};

/// The longest id a card keeps for itself (a vCard `UID`). Longer is not an id any exporter writes.
pub const IDENT_MAX: usize = 512;

/// One detail of a card: an email, a phone, an address, a job, a date, a link.
#[derive(Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CardDetail {
    pub kind: String,
    #[serde(default)]
    pub label: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_date: Option<String>,
    #[serde(default)]
    pub is_current: bool,
}

/// One contact, normalised. Names and details are as PM stores them; `labels` are the names of the
/// labels (a vCard `CATEGORIES`); `ident` is the file's `UID`; `rev` is its `REV`, kept only in the
/// snapshot.
#[derive(Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Card {
    pub display_name: String,
    #[serde(default)]
    pub given_name: String,
    #[serde(default)]
    pub middle_name: String,
    #[serde(default)]
    pub family_name: String,
    #[serde(default)]
    pub name_prefix: String,
    #[serde(default)]
    pub name_suffix: String,
    #[serde(default, rename = "birthday")]
    pub born_on: Option<String>,
    /// Which day a 29 February birthday is marked on in other years (E22). PM's own fact: only
    /// PM's own file export carries it (`X-PM-LEAP-DAY`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leap_day: Option<LeapDay>,
    #[serde(default)]
    pub met_where: String,
    #[serde(default)]
    pub met_on: Option<String>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub details: Vec<CardDetail>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default, rename = "uid")]
    pub ident: Option<String>,
    #[serde(default)]
    pub rev: Option<String>,
}

/// What [`Card::finish`] had to cut to fit PM's limits.
#[derive(Clone, Copy, Default)]
pub struct Cuts {
    /// Values (names, notes, labels, details) shortened to the cap.
    pub values: usize,
    /// Details or labels dropped past the per-person cap.
    pub dropped: usize,
}

impl Card {
    /// Read a person as a card, for export. The id is the person's own, so a reimport of the file
    /// finds them again.
    pub fn from_person(p: &Person) -> Card {
        Card {
            display_name: p.display_name.clone(),
            given_name: p.given_name.clone(),
            middle_name: p.middle_name.clone(),
            family_name: p.family_name.clone(),
            name_prefix: p.name_prefix.clone(),
            name_suffix: p.name_suffix.clone(),
            born_on: p.born_on.clone(),
            leap_day: p.leap_day,
            met_where: p.met_where.clone(),
            met_on: p.met_on.clone(),
            notes: p.notes.clone(),
            details: p
                .details
                .iter()
                .map(|d| CardDetail {
                    kind: d.kind.clone(),
                    label: d.label.clone(),
                    value: d.value.clone(),
                    parts: flat_parts(&d.kind, d.parts.as_ref()),
                    from_date: d.from_date.clone(),
                    to_date: d.to_date.clone(),
                    is_current: d.is_current,
                })
                .collect(),
            labels: p.labels.iter().map(|l| l.name.clone()).collect(),
            ident: Some(p.ident.clone()),
            rev: None,
        }
    }

    /// The draft that creates this card as a new person. [`Card::finish`] has made it valid, so a
    /// create never refuses a card for its content.
    pub fn to_draft(&self) -> PersonDraft {
        PersonDraft {
            display_name: self.display_name.clone(),
            given_name: self.given_name.clone(),
            middle_name: self.middle_name.clone(),
            family_name: self.family_name.clone(),
            name_prefix: self.name_prefix.clone(),
            name_suffix: self.name_suffix.clone(),
            born_on: self.born_on.clone(),
            leap_day: self.leap_day,
            met_where: self.met_where.clone(),
            met_on: self.met_on.clone(),
            notes: self.notes.clone(),
            starred: false,
            details: self.details.iter().map(CardDetail::to_draft).collect(),
        }
    }

    /// Clean and bound everything to what PM stores (spec 2.2), so one hostile card can't refuse a
    /// whole import: control and spoofing characters out, values cut to the caps, blank rows gone,
    /// dates that aren't real dates dropped, details in PM's order (by kind, the order a person is
    /// read back in). Composes a display name when the card had none. Returns what it cut, or `None`
    /// when there is nothing to call the person by (no name, nickname, company, email or phone).
    pub fn finish(&mut self) -> Option<Cuts> {
        let mut cuts = Cuts::default();
        self.given_name = cut_chars(&self.given_name, &mut cuts);
        self.middle_name = cut_chars(&self.middle_name, &mut cuts);
        self.family_name = cut_chars(&self.family_name, &mut cuts);
        self.name_prefix = cut_chars(&self.name_prefix, &mut cuts);
        self.name_suffix = cut_chars(&self.name_suffix, &mut cuts);
        self.display_name = cut_chars(&self.display_name, &mut cuts);
        self.met_where = cut_chars(&self.met_where, &mut cuts);
        self.born_on = self
            .born_on
            .as_deref()
            .and_then(partial_date::normalise_born_on);
        self.leap_day = partial_date::kept_leap_day(self.born_on.as_deref(), self.leap_day);
        self.met_on = self
            .met_on
            .as_deref()
            .and_then(partial_date::normalise_dated);
        let notes = clean_multiline(&self.notes);
        let (notes, was_cut) = truncate_bytes(&notes, NOTES_MAX);
        if was_cut {
            cuts.values += 1;
        }
        self.notes = notes;

        let mut details: Vec<CardDetail> = Vec::with_capacity(self.details.len());
        for mut d in std::mem::take(&mut self.details) {
            if !DETAIL_KINDS.contains(&d.kind.as_str()) {
                continue;
            }
            let multiline = d.kind == "address";
            let value = if multiline {
                clean_multiline(&d.value)
            } else {
                clean_text(&d.value)
            };
            let (value, was_cut) = truncate_chars(&value, VALUE_MAX);
            if was_cut {
                cuts.values += 1;
            }
            if value.is_empty() {
                continue;
            }
            d.value = if d.kind == "date" {
                match partial_date::normalise_any(&value) {
                    Some(v) => v,
                    None => continue,
                }
            } else {
                value
            };
            d.label = cut_chars(&d.label, &mut cuts);
            d.from_date = d
                .from_date
                .as_deref()
                .and_then(partial_date::normalise_dated);
            d.to_date = d.to_date.as_deref().and_then(partial_date::normalise_dated);
            d.parts = d.parts.take().and_then(|p| clean_parts(p, multiline));
            // The parts are cut per field, so their total can still pass what a save accepts
            // (four 1,024-character fields of 4-byte characters do). A save refuses that and would
            // roll the whole file back, so what only a hostile card holds is let go: the value
            // (always kept, and already capped) says the same thing.
            if d.parts
                .as_ref()
                .is_some_and(|p| p.to_string().len() > PARTS_MAX_BYTES)
            {
                d.parts = None;
                cuts.values += 1;
            }
            details.push(d);
        }
        // PM reads a person's details back ordered by kind, then in the order given.
        details.sort_by(|a, b| a.kind.cmp(&b.kind));
        if details.len() > DETAILS_MAX {
            cuts.dropped += details.len() - DETAILS_MAX;
            details.truncate(DETAILS_MAX);
        }
        self.details = details;

        let mut seen = HashSet::new();
        let mut labels = Vec::new();
        for l in std::mem::take(&mut self.labels) {
            let l = cut_chars(&l, &mut cuts);
            if !l.is_empty() && seen.insert(fold_key(&l)) {
                labels.push(l);
            }
        }
        if labels.len() > LABELS_PER_PERSON_MAX {
            cuts.dropped += labels.len() - LABELS_PER_PERSON_MAX;
            labels.truncate(LABELS_PER_PERSON_MAX);
        }
        self.labels = labels;

        self.ident = self
            .ident
            .take()
            .map(|i| i.trim().to_string())
            .filter(|i| !i.is_empty() && i.chars().count() <= IDENT_MAX);
        self.rev = self
            .rev
            .take()
            .map(|r| clean_text(&r))
            .filter(|r| !r.is_empty() && r.chars().count() <= VALUE_MAX);

        if self.display_name.is_empty() {
            let name = NameParts {
                prefix: self.name_prefix.clone(),
                given: self.given_name.clone(),
                middle: self.middle_name.clone(),
                family: self.family_name.clone(),
                suffix: self.name_suffix.clone(),
            };
            let pairs: Vec<(String, String)> = self
                .details
                .iter()
                .map(|d| (d.kind.clone(), d.value.clone()))
                .collect();
            let (display, _) = truncate_chars(&compose_display_name(&name, &pairs), VALUE_MAX);
            self.display_name = display;
        }
        if self.display_name.is_empty() {
            return None;
        }
        Some(cuts)
    }
}

/// One cleaned single-line value, cut to the cap (counted in `cuts`).
fn cut_chars(s: &str, cuts: &mut Cuts) -> String {
    let (kept, was_cut) = truncate_chars(&clean_text(s), VALUE_MAX);
    if was_cut {
        cuts.values += 1;
    }
    kept
}

impl CardDetail {
    /// The draft of one detail of a new person.
    pub fn to_draft(&self) -> DetailDraft {
        DetailDraft {
            id: None,
            kind: self.kind.clone(),
            label: self.label.clone(),
            value: self.value.clone(),
            parts: self.parts.clone(),
            from_date: self.from_date.clone(),
            to_date: self.to_date.clone(),
            is_current: self.is_current,
        }
    }
}

/// An address whose only part is the street is its text: a vCard `ADR` with only a street reads
/// back as a plain address (the street is the whole address), so PM's own street-only address is
/// read the same way, and a file written from one matches it again.
pub fn flat_parts(kind: &str, parts: Option<&Value>) -> Option<Value> {
    let street_only = kind == "address"
        && parts
            .and_then(Value::as_object)
            .is_some_and(|o| o.len() == 1 && o.contains_key("street"));
    if street_only {
        None
    } else {
        parts.cloned()
    }
}

/// A `parts` object with each string cleaned and bounded and blank entries gone; `None` when
/// nothing is left. Anything that isn't an object of strings is not a card's.
fn clean_parts(parts: Value, multiline: bool) -> Option<Value> {
    let Value::Object(map) = parts else {
        return None;
    };
    let mut out = Map::new();
    for (k, v) in map {
        let Value::String(s) = v else { continue };
        let s = if multiline {
            clean_multiline(&s)
        } else {
            clean_text(&s)
        };
        let (s, _) = truncate_chars(&s, VALUE_MAX);
        if !s.is_empty() {
            out.insert(clean_text(&k), Value::String(s));
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(Value::Object(out))
    }
}

/// The text cut to at most `max` characters, and whether it was.
pub fn truncate_chars(text: &str, max: usize) -> (String, bool) {
    match text.char_indices().nth(max) {
        Some((at, _)) => (text[..at].to_string(), true),
        None => (text.to_string(), false),
    }
}

/// The text cut to at most `max` bytes on a character boundary, and whether it was.
pub fn truncate_bytes(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_string(), false);
    }
    let mut at = max;
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    (text[..at].to_string(), true)
}
