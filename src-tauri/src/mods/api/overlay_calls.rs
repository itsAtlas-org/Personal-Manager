// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `calendar.overlay.set` (PR C, K2): the dated entries a mod puts on the Calendar (the Pinboard's
//! timeline entries). PM draws them without running the mod's code (`mod_overlay`, read by the
//! Calendar's own command), so the pop-out Calendar keeps them and they are PM's text, never the
//! mod's markup.
//!
//! **The caller's whole set, replaced in one transaction**: the entries sent are the entries there
//! are, so a mod that republishes at every start heals a set that a pause (which deletes the rows) or
//! an edit left behind. Another mod's rows are never touched (every statement is keyed to the
//! caller). An uninstall removes the rows (the table cascades from the mod).
//!
//! **No permission of its own** (that would change the pinned permission set and every install
//! screen): the row is `storage`, local and automatic, and the call is gated here on the mod's
//! installed manifest declaring `contributions['calendar.overlay']` — a mod that never said it draws
//! on the Calendar is refused, whatever it holds.
//!
//! **It refuses bad input and never repairs it**, unlike the reader of a board that predates the
//! mod, which clamps what it finds (`clamp_overlay`, K4): a mod that sends a bad entry learns of it.
//! The two hold one rule, so a row the reader seeds is a row a mod can publish again: at most
//! [`limits::OVERLAY_MAX_ENTRIES`] entries; an id of one plain line ([`id_ok`]), 1 to
//! [`limits::OVERLAY_ID_MAX_CHARS`] characters, each used once; a label of 1 to
//! [`limits::OVERLAY_LABEL_MAX_CHARS`] characters that [`user_line_fault`] accepts (no control or
//! hidden character, no tab or line break; an emoji's joiner and variation selector are fine, as is a
//! no-break space inside, but not at an end);
//! a date written `YYYY-MM-DD` that is a real day between the years 1900 and 2200 (what the table's
//! own check admits, and the span PM's calendar draws). The reader tidies where the writer refuses.

use std::collections::HashSet;

use chrono::{Datelike, NaiveDate};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

use super::calls::{self, quiet};
use super::handlers::parse;
use super::{limits, Authorised, Code, Host, ModError, Reply};
use crate::mods::package::{plain_text_fault, user_line_fault};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetArgs {
    entries: Vec<EntryArgs>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EntryArgs {
    id: String,
    date: String,
    label: String,
}

/// One entry, checked. The sentences name the rule, never what was sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Entry {
    pub id: String,
    pub date: String,
    pub label: String,
}

fn refused(what: &str) -> ModError {
    ModError::bad_args(format!("An overlay entry's {what} is not allowed."))
}

/// A real `YYYY-MM-DD` day within [`limits::OVERLAY_YEARS`], exactly as written. Also the day rule of
/// the legacy reader's `clamp_overlay` (`pinboard_legacy`), so the two cannot disagree.
pub(crate) fn real_date(date: &str) -> bool {
    date.len() == 10
        && NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok_and(|d| {
            d.format("%Y-%m-%d").to_string() == date && limits::OVERLAY_YEARS.contains(&d.year())
        })
}

/// Whether `id` is one an overlay entry may have: one plain line of 1 to
/// [`limits::OVERLAY_ID_MAX_CHARS`] characters. The writer's rule and `clamp_overlay`'s.
pub(crate) fn id_ok(id: &str) -> bool {
    plain_text_fault(id, limits::OVERLAY_ID_MAX_CHARS, false, false).is_ok()
}

/// The whole set, or the first thing wrong with it.
pub(super) fn check_entries(raw: Vec<EntryArgs>) -> Result<Vec<Entry>, ModError> {
    if raw.len() > limits::OVERLAY_MAX_ENTRIES {
        return Err(ModError::bad_args(format!(
            "A mod may publish at most {} calendar entries.",
            limits::OVERLAY_MAX_ENTRIES
        )));
    }
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(raw.len());
    for e in raw {
        if !id_ok(&e.id) {
            return Err(refused("id"));
        }
        let label = e.label.trim_matches(' ');
        if user_line_fault(label, limits::OVERLAY_LABEL_MAX_CHARS, false).is_err() {
            return Err(refused("label"));
        }
        if !real_date(&e.date) {
            return Err(refused("date"));
        }
        if !seen.insert(e.id.clone()) {
            return Err(ModError::bad_args(
                "An overlay entry's id may be used only once.",
            ));
        }
        out.push(Entry {
            id: e.id,
            date: e.date,
            label: label.to_owned(),
        });
    }
    Ok(out)
}

/// Whether `calendar.overlay.set` would take these `(id, date, label)` entries (tests only: the
/// legacy reader's output must always be publishable again by the mod).
#[cfg(test)]
pub(crate) fn writer_accepts(entries: &[(&str, &str, &str)]) -> bool {
    let raw: Vec<EntryArgs> = entries
        .iter()
        .map(|(id, date, label)| {
            serde_json::from_value(json!({"id": id, "date": date, "label": label})).unwrap()
        })
        .collect();
    check_entries(raw).is_ok()
}

/// `calendar.overlay.set({entries: [{id, date, label}]})` → `{count}`.
pub(super) async fn set<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let SetArgs { entries } = parse(args)?;
    let entries = check_entries(entries)?;
    let value = calls::write(host, auth, move |cx| {
        if !cx.facts.contributions.contains("calendar.overlay") {
            return Err(ModError::new(
                Code::NotGranted,
                "This mod hasn't declared a calendar overlay.",
            ));
        }
        let mod_id = cx.auth.ticket.mod_id.as_str();
        cx.conn
            .execute("DELETE FROM mod_overlay WHERE mod_id = ?1", params![mod_id])
            .map_err(ModError::internal)?;
        let mut insert = cx
            .conn
            .prepare("INSERT INTO mod_overlay(mod_id, id, date, label) VALUES (?1, ?2, ?3, ?4)")
            .map_err(ModError::internal)?;
        for e in &entries {
            insert
                .execute(params![mod_id, e.id, e.date, e.label])
                .map_err(ModError::internal)?;
        }
        Ok(quiet(json!({"count": entries.len()})))
    })
    .await?;
    Ok(Reply::value(value))
}
