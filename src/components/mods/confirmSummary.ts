// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What PM's yes/no draws from the backend's summary (ModConfirmDialog): line 1 the question, the lines
// after it an address or the note that one was clamped. Pure, so the bounds are tested without a dialog.

/** The most combining marks in a row a line keeps (`MAX_STACKED_MARKS` in `mods/package.rs`). */
export const CONFIRM_MAX_MARKS = 4;

// What Rust's `tidy_user_line` removes or refuses in a person's own words: the controls (Cc) and
// `is_invisible` in `mods/package.rs` (soft hyphen, fillers, direction and zero-width marks, line and
// paragraph separators, word joiners, the BOM, tags), without the emoji joiner (U+200D) and the
// variation selectors (U+FE00 to U+FE0F), which that rule KEEPS. A parity test reads `package.rs`.
// Not `cleanModChars`: its stricter rule (every Cf, marks cut to three) is for a mod's own name and
// would draw the family emoji as three people and strip a Cf the rule allows.
// One alternation, not a character class: several of these are combining marks to a class.
const HIDDEN_IN_USER_LINE = new RegExp(
  "\\p{Cc}|\\u00AD|\\u034F|\\u061C|\\u115F|\\u1160|\\u17B4|\\u17B5|[\\u180B-\\u180F]|\\u200B|\\u200C|[\\u200E\\u200F]" +
    "|[\\u2028-\\u202E]|[\\u2060-\\u206F]|\\u3164|\\uFEFF|\\uFFA0|[\\uFFF0-\\uFFFB]|[\\u{E0000}-\\u{E0FFF}]",
  "gu",
);
const MARK_RUN = new RegExp(`(\\p{M}{${CONFIRM_MAX_MARKS}})\\p{M}+`, "gu");

/** A line as the dialog draws it: exactly what the shared rule for a person's words
 *  (`tidy_user_line`) removes or refuses becomes a space, and a run of more than four combining marks
 *  is cut to four. An emoji's joiner, its variation selectors, a no-break space and any other
 *  character the rule keeps are drawn as stored, so what the person is shown is what is made. Pure. */
export function cleanUserLine(text: string): string {
  return text.replace(HIDDEN_IN_USER_LINE, " ").replace(MARK_RUN, "$1");
}

/** The most characters of one line the dialog draws, and the most lines after the question. The backend
 *  clamps an address at 300 characters on its own; these are the page's own bound on what crossed IPC. */
export const CONFIRM_LINE_CHARS = 400;
export const CONFIRM_MAX_ADDRESS_LINES = 6;

/** The question and the lines after it, from the backend's summary, with `hidden` the number of
 *  lines past the bound that are not in `lines` (the dialog says so: nothing is dropped silently).
 *  Control and direction-changing characters are made spaces ({@link cleanUserLine}), so a line can't
 *  reorder or hide what is beside it. Pure. */
export function readSummary(summary: string): {
  question: string;
  lines: string[];
  hidden: number;
} {
  const clip = (s: string) => {
    const clean = cleanUserLine(s);
    return clean.length > CONFIRM_LINE_CHARS ? `${clean.slice(0, CONFIRM_LINE_CHARS)}…` : clean;
  };
  const [first = "", ...rest] = summary.split(/\r\n|\r|\n/);
  const later = rest.filter((l) => l !== "");
  return {
    question: clip(first),
    lines: later.slice(0, CONFIRM_MAX_ADDRESS_LINES).map(clip),
    hidden: Math.max(0, later.length - CONFIRM_MAX_ADDRESS_LINES),
  };
}
