// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The one cleaning rule for a mod's own words in PM's chrome, shared by every place that draws them
// (components/mods/modText, and the sidebar's tab label in lib/nav). Control, format (which includes
// the bidirectional controls and zero-width characters) and line/paragraph separators become spaces,
// and a run of combining marks is cut to MAX_MARKS. A name can be one letter and thirty-nine accents:
// the accents stack tens of ems above and below the line and paint over the badges and buttons
// beside it. Rust's name check lets them through too, so this is the page's own defence; the
// headings that draw a name also clip it (`overflow-hidden`). Three is the most any real script
// stacks on one letter (Thai, Hebrew pointing, Vietnamese with a dot below).

export const MAX_MARKS = 3;

const INVISIBLE = /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu;
const MARK_RUN = new RegExp(`(\\p{M}{${MAX_MARKS}})\\p{M}+`, "gu");

/** `text` with invisible characters turned into spaces and over-long runs of combining marks cut.
 *  Spaces are not collapsed here: each caller decides how. */
export function cleanModChars(text: string): string {
  return text.replace(INVISIBLE, " ").replace(MARK_RUN, "$1");
}

/** A mod's own words as one plain line: cleaned (above), runs of spaces collapsed, the ends trimmed. */
export function plainLine(text: string): string {
  return cleanModChars(text).replace(/ {2,}/g, " ").trim();
}

/** A mod's name as it sits inside one of PM's own sentences, titles and menu items: in quotes, which
 *  the name can't close (a double quote inside it becomes a single one), so a name like
 *  `Calendar. Nothing was changed.` reads as the mod's name rather than as more of PM's sentence.
 *  Rust quotes names in its own messages for the same reason (package.rs `quote`). */
export function quoteName(name: string): string {
  return `“${plainLine(name).replace(/[“”"]/g, "'")}”`;
}
