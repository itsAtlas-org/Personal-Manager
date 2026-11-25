// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Pinboard's freeform timeline entries as the Calendar's overlay dots, the TypeScript original of
// the rule Rust now reads them by (`src-tauri/src/pinboard_legacy.rs`: `pinboard_entries` and
// `clamp_overlay`). The Calendar no longer calls this: it reads `calendar_pinboard_entries`. It stays
// so that the shared fixtures (`tests/pinboard-overlay-fixtures.json`) hold the two implementations
// to one behaviour. Pure and separate from the view so the awkward cases (folders, project-bound
// timelines, opted-out widgets, a corrupt stored board, a date the overlay table cannot hold) are
// unit-testable.

import type { PinboardEntry } from "../types";
import { BOARD_VERSION, type Board, type Widget } from "./types";

/** Shown when an entry has a date but no text yet, so a dated row never renders as an empty chip.
 *  Matches the fallback `linkProject` writes when the same entry becomes a real milestone
 *  (PinboardView), so a blank row doesn't get renamed the moment its timeline is linked. */
const UNTITLED = "deadline";

/** The overlay table's limits, as `calendar.overlay.set` holds them (`limits.rs`). */
const MAX_ENTRIES = 2000;
const MAX_LABEL_CHARS = 140;
const MAX_ID_CHARS = 200;
const FIRST_YEAR = 1900;
const LAST_YEAR = 2200;

/** A real calendar day written `YYYY-MM-DD` (ASCII digits) within the years the overlay draws. */
function isRealDay(date: string): boolean {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(date);
  if (!m) return false;
  const [y, mo, d] = [Number(m[1]), Number(m[2]), Number(m[3])];
  if (y < FIRST_YEAR || y > LAST_YEAR) return false;
  const t = new Date(Date.UTC(y, mo - 1, d));
  return t.getUTCFullYear() === y && t.getUTCMonth() === mo - 1 && t.getUTCDate() === d;
}

// Zero-width, direction and filler characters, tags (`is_invisible` in `package.rs`) without the
// emoji joiner (U+200D) and the variation selectors (U+FE00 to U+FE0F), which a label may keep.
const HIDDEN =
  "\\u00AD|\\u034F|\\u061C|\\u115F|\\u1160|\\u17B4|\\u17B5|[\\u180B-\\u180F]|\\u200B|\\u200C|[\\u200E\\u200F]|[\\u2028-\\u202E]|[\\u2060-\\u206F]|\\u3164|\\uFEFF|\\uFFA0|[\\uFFF0-\\uFFFB]|[\\u{E0000}-\\u{E0FFF}]";
// One alternation, not character classes: the joiner and the selectors are "combining" to a class.
const HIDDEN_OR_CONTROL = new RegExp(`(?:${HIDDEN})|\\p{Cc}`, "gu");
const ID_FAULT = new RegExp(
  `(?:${HIDDEN})|\\u200D|[\\uFE00-\\uFE0F]|[^\\S ]|\\p{Cc}|\\p{M}{5}`,
  "u",
);

/** One plain line of 1 to 200 characters (`calendar.overlay.set`'s id rule: no control, hidden or
 *  odd-space character, no space at either end, no more than four accents stacked). */
function isPlainId(id: string): boolean {
  const n = Array.from(id).length;
  return n >= 1 && n <= MAX_ID_CHARS && id === id.replace(/^ +| +$/g, "") && !ID_FAULT.test(id);
}

/** A line of the person's words as PM keeps it (`tidy_user_line` in `package.rs`): a tab or line
 *  break becomes a space, a control or hidden character goes (an emoji's joiner and variation
 *  selectors stay), whitespace of every kind is trimmed off both ends, a run of more than four
 *  combining marks is cut to four, and a line with only joiners and selectors left is empty. */
export function tidyLabel(label: string): string {
  const kept = label
    .replace(/[\t\n\r\v\f\u0085]/g, " ")
    .replace(HIDDEN_OR_CONTROL, "")
    .replace(/^\s+|\s+$/g, "");
  if (/^(?:\u200D|[\uFE00-\uFE0F])*$/u.test(kept)) return "";
  return kept.replace(/(\p{M}{4})\p{M}+/gu, "$1");
}

/** What the overlay table may hold, from entries read off a board. Never throws; in board order:
 *  an entry whose date (its first ten characters) is not a real `YYYY-MM-DD` day within 1900 to
 *  2200 is skipped; so is one whose id a mod could not publish (one plain line of at most 200
 *  characters); the label is tidied (`tidyLabel`), falls back to "deadline", is cut to 140
 *  characters (code points, as Rust counts them) and is tidied again; an entry whose id was already
 *  kept is skipped, so the first stays; at most 2,000 are kept. The Rust `clamp_overlay` is the same
 *  rule, and the pre-move reader and the move's seeding both use it. */
export function clampOverlay(entries: PinboardEntry[]): PinboardEntry[] {
  const seen = new Set<string>();
  const out: PinboardEntry[] = [];
  for (const e of entries) {
    if (out.length >= MAX_ENTRIES) break;
    const date = e.date.slice(0, 10);
    if (!isRealDay(date) || !isPlainId(e.id) || seen.has(e.id)) continue;
    // Tidied again after the cut, which may land after a space or inside an emoji sequence (the rule
    // reads its own rows back, and a mod can publish what it keeps).
    const cut = Array.from(tidyLabel(e.label.trim()) || UNTITLED)
      .slice(0, MAX_LABEL_CHARS)
      .join("");
    const label = tidyLabel(cut) || UNTITLED;
    seen.add(e.id);
    out.push({ id: e.id, date, label });
  }
  return out;
}

/** Every dated entry of every freeform timeline on the board, in board order.
 *
 *  Skipped deliberately:
 *  - a timeline **bound to a project** (`project` set) — its entries are real `project_milestones`
 *    already drawn by the milestone overlay, so including them here would double-draw;
 *  - a timeline the user opted out of (`showOnCalendar === false`) — unset means shown, so entries
 *    land on the calendar by default;
 *  - entries with no date — they have no day to sit on.
 *
 *  Folders are walked at any depth. The board is stored JSON, so this parses defensively: anything
 *  malformed yields no entries rather than throwing into the calendar. Version mismatch returns
 *  nothing, matching the board's own parse (which discards it wholesale). The result is then
 *  clamped (`clampOverlay`) to what the overlay table can hold. */
export function pinboardEntries(raw: string | null): PinboardEntry[] {
  return clampOverlay(walkBoard(raw));
}

function walkBoard(raw: string | null): PinboardEntry[] {
  if (!raw) return [];
  let parsed: Board;
  try {
    parsed = JSON.parse(raw) as Board;
  } catch {
    return [];
  }
  if (!parsed || parsed.version !== BOARD_VERSION || !Array.isArray(parsed.widgets)) return [];

  const out: PinboardEntry[] = [];
  const visit = (w: Widget | undefined) => {
    if (!w || typeof w !== "object") return;
    if (w.kind === "folder") {
      if (Array.isArray(w.children)) for (const child of w.children) visit(child);
      return;
    }
    if (w.kind !== "timeline") return;
    if (w.project) return;
    if (w.showOnCalendar === false) return;
    if (typeof w.id !== "string") return;
    if (!Array.isArray(w.items)) return;
    for (const it of w.items) {
      if (!it || typeof it.id !== "string" || typeof it.date !== "string") continue;
      const date = it.date.slice(0, 10);
      if (!date) continue;
      out.push({
        id: `${w.id}:${it.id}`,
        date,
        label: (typeof it.label === "string" ? it.label.trim() : "") || UNTITLED,
      });
    }
  };
  for (const w of parsed.widgets) visit(w);
  return out;
}
