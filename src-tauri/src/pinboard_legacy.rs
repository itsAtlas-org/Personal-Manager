// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What core still knows of the Pinboard's board format, now that the Pinboard is becoming a mod
//! (PR C). It stays small and legacy-named, and everything here reads the board as plain JSON, never
//! as the Pinboard's own types.
//!
//! Today (K4): the Calendar's dots. [`pinboard_entries`] is a port of `src/lib/pinboard/calendarEntries.ts`
//! (the freeform timeline entries of the stored board), and [`clamp_overlay`] is the one rule that
//! turns them into what `mod_overlay` may hold. The pre-move reader ([`calendar_entries`]) and the
//! move's own seeding of the table (K8) both go through that rule, so the dots a user sees do not
//! change at the move. The rule **never raises**: it drops what the table could not hold rather than
//! failing, because the overlay is derived data and the mod republishes it.
//!
//! The same JSON fixtures (`tests/pinboard-overlay-fixtures.json`) drive this port and the
//! TypeScript original, so the two cannot drift apart.

use std::collections::HashSet;

use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;

use crate::error::Result;
use crate::mods::api::limits::{
    OVERLAY_ID_MAX_CHARS, OVERLAY_LABEL_MAX_CHARS, OVERLAY_MAX_ENTRIES,
};
use crate::mods::api::{overlay_id_ok, real_overlay_date};
use crate::mods::package::tidy_user_line;

/// The Pinboard mod's id: the row set of `mod_overlay` the Calendar reads once the board has moved.
pub const PINBOARD_MOD_ID: &str = "atlas.pinboard";

/// The `settings` key of the pre-mod board (`PINBOARD_PREF_KEY` in `src/lib/pinboard/types.ts`).
pub const BOARD_KEY: &str = "pinboard";

/// The board format version this reader understands (`BOARD_VERSION`); another version is read as
/// no board, as the Pinboard's own parse discards it wholesale.
const BOARD_VERSION: f64 = 1.0;

/// Shown when an entry has a date but no text, so a dated row never renders as an empty chip. The
/// same word the Pinboard writes when such an entry becomes a milestone.
const UNTITLED: &str = "deadline";

/// The most rows read from the mod's table: room for the cap and for a planted run of rows the clamp
/// drops, and no more than that.
const READ_LIMIT: usize = OVERLAY_MAX_ENTRIES * 5;

/// One dated entry on the Calendar. `id` is `<widgetId>:<itemId>` (the Calendar's event is
/// `pinboard:<id>`); it is the row key of `mod_overlay`. Serialises as the wire shape of
/// `calendar_pinboard_entries`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub id: String,
    pub date: String,
    pub label: String,
}

/// JavaScript's `String.prototype.trim` set: Rust's `White_Space` minus U+0085 plus U+FEFF. The one
/// trim the TypeScript original and this port share, so a fixture reads the same in both.
fn is_js_space(c: char) -> bool {
    c == '\u{FEFF}' || (c.is_whitespace() && c != '\u{85}')
}

fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

/// JavaScript truthiness of a JSON value (`if (w.project)`).
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// Every dated entry of every freeform timeline on the stored board, in board order.
///
/// Skipped deliberately: a timeline **bound to a project** (its entries are real milestones, drawn
/// by the milestone overlay), a timeline with `showOnCalendar === false` (unset means shown), and an
/// entry with no date. Folders are walked at any depth. The board is stored JSON, read defensively:
/// anything malformed, or of another version, yields no entries. The date is the first ten characters
/// of what the entry holds and the label is trimmed (or "deadline"); neither is checked here. That
/// is [`clamp_overlay`]'s job.
pub fn pinboard_entries(raw: &str) -> Vec<Entry> {
    let Ok(board) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    if board.get("version").and_then(Value::as_f64) != Some(BOARD_VERSION) {
        return Vec::new();
    }
    let Some(widgets) = board.get("widgets").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for w in widgets {
        visit(w, &mut out);
    }
    out
}

fn visit(w: &Value, out: &mut Vec<Entry>) {
    let Some(w) = w.as_object() else { return };
    match w.get("kind").and_then(Value::as_str) {
        Some("folder") => {
            if let Some(children) = w.get("children").and_then(Value::as_array) {
                for child in children {
                    visit(child, out);
                }
            }
        }
        Some("timeline") => {
            if w.get("project").is_some_and(truthy)
                || w.get("showOnCalendar") == Some(&Value::Bool(false))
            {
                return;
            }
            let Some(widget_id) = w.get("id").and_then(Value::as_str) else {
                return;
            };
            let Some(items) = w.get("items").and_then(Value::as_array) else {
                return;
            };
            for it in items {
                let (Some(item_id), Some(date)) = (
                    it.get("id").and_then(Value::as_str),
                    it.get("date").and_then(Value::as_str),
                ) else {
                    continue;
                };
                let date: String = date.chars().take(10).collect();
                if date.is_empty() {
                    continue;
                }
                let label = it.get("label").and_then(Value::as_str).map_or("", js_trim);
                out.push(Entry {
                    id: format!("{widget_id}:{item_id}"),
                    date,
                    label: if label.is_empty() { UNTITLED } else { label }.to_owned(),
                });
            }
        }
        _ => {}
    }
}

/// What `mod_overlay` may hold, from entries read off a board that predates the mod. **Never
/// raises**; in board order:
/// - an entry whose date (its first ten characters) is not a real calendar day written
///   `YYYY-MM-DD` within the years 1900 to 2200 is skipped;
/// - an entry whose id is not one a mod could publish (one plain line of at most 200 characters:
///   `overlay_id_ok`) is skipped;
/// - the label is tidied (`tidy_user_line`: a tab or line break becomes a space, a control or
///   hidden character goes, an emoji's joiner stays, whitespace is trimmed off both ends, a run of
///   more than four combining marks is cut), falls back to "deadline" and is cut to 140 characters
///   (and tidied again, since the cut can leave a space at the end);
/// - an entry whose id was already kept is skipped, so the first stays;
/// - at most 2,000 entries are kept.
///
/// `calendar.overlay.set` refuses such input from a mod; this repairs it, because an old board can
/// hold anything and must neither fail the move's seeding nor draw differently from the table. What
/// it keeps is always something `calendar.overlay.set` accepts, so the mod can publish its first set
/// over the seeded rows.
pub fn clamp_overlay(entries: Vec<Entry>) -> Vec<Entry> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for e in entries {
        if out.len() >= OVERLAY_MAX_ENTRIES {
            break;
        }
        let date: String = e.date.chars().take(10).collect();
        if !real_overlay_date(&date) || !overlay_id_ok(&e.id) || seen.contains(&e.id) {
            continue;
        }
        let label = tidy_user_line(js_trim(&e.label));
        let label: String = if label.is_empty() { UNTITLED } else { &label }
            .chars()
            .take(OVERLAY_LABEL_MAX_CHARS)
            .collect();
        // The cut may land after a space or inside an emoji sequence, leaving what the tidy would
        // change again: tidied once more, so the rule gives the same answer when it reads its own
        // rows back (the table is read through it after the move) and the writer accepts the result.
        let label = tidy_user_line(&label);
        let label = if label.is_empty() {
            UNTITLED.to_owned()
        } else {
            label
        };
        seen.insert(e.id.clone());
        out.push(Entry {
            id: e.id,
            date,
            label,
        });
    }
    out
}

/// The overlay a stored board draws: [`clamp_overlay`] over [`pinboard_entries`]. The pre-move
/// reader, and what the move's seeding writes (K8).
pub fn derive_overlay(raw: &str) -> Vec<Entry> {
    clamp_overlay(pinboard_entries(raw))
}

/// What the Calendar draws for the Pinboard (`calendar_pinboard_entries`).
///
/// Once the move is recorded (`pinboard_moved_sha`), the mod's own `mod_overlay` rows, and an empty
/// table means no dots: the legacy row is a frozen snapshot by then and is never read as a fallback.
/// Before the move, derived from the stored board.
pub fn calendar_entries(conn: &Connection) -> Result<Vec<Entry>> {
    if crate::projects::pinboard_move_recorded(conn)? {
        // The table is the mod's, and a shared vault's member can write a row past what
        // `calendar.overlay.set` allows (the table's own check is only the date's shape). So what is
        // read is bounded in SQL (rows, and the length of each cell) and then goes through the same
        // rule as the seeding, which never raises: no more than the Calendar can draw, and nothing it
        // would draw on a day that does not exist.
        let mut stmt = conn.prepare(
            "SELECT substr(id, 1, ?2), date, substr(label, 1, ?3) FROM mod_overlay \
             WHERE mod_id = ?1 ORDER BY rowid LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![
                PINBOARD_MOD_ID,
                // One more than an id may hold, so a longer one still fails the id rule.
                (OVERLAY_ID_MAX_CHARS + 1) as i64,
                // Room for a label's hidden characters, which the tidy drops before the cut.
                (OVERLAY_LABEL_MAX_CHARS * 4) as i64,
                READ_LIMIT as i64
            ],
            |r| {
                Ok(Entry {
                    id: r.get(0)?,
                    date: r.get(1)?,
                    label: r.get(2)?,
                })
            },
        )?;
        // A row whose cell is not text (a BLOB, bytes that are not UTF-8) draws nothing and is never
        // a reason for the whole Calendar to fail; a database that fails (I/O, corruption) is an
        // error, not a shorter list of dots.
        let mut kept = Vec::new();
        for row in rows {
            match row {
                Ok(entry) => kept.push(entry),
                Err(e) if crate::mods::refs::is_wrong_type(&e) => {}
                Err(e) => return Err(e.into()),
            }
        }
        return Ok(clamp_overlay(kept));
    }
    Ok(crate::db::get_setting(conn, BOARD_KEY)?
        .map(|raw| derive_overlay(&raw))
        .unwrap_or_default())
}

#[cfg(test)]
mod tests;
