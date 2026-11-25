// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Made by X" on a task or milestone a mod made (Mods spec §1.5, §4; PR C, K5). The ledger's record of
// the maker is a mod's name, which the mod chose, so this is the one place it is made fit to show:
// cleaned by the rule every mod-supplied name is (control, direction-changing and other format
// characters, long runs of accents), cut short, and NEVER shown without the tier
// word the ledger recorded beside it. An unsigned mod named "Atlas" therefore reads "Atlas · unsigned",
// and a name can't pass as PM's own or as a reviewed mod. The label is for the eye: nothing in PM
// decides anything from it.

import type { MadeBy } from "../../lib/types";

/** The most characters of a name the page shows (the backend already cuts it at 60). */
export const MADE_BY_NAME_MAX = 40;

/** The rule PM draws every mod-supplied name by (`lib/modTextClean`, which the kit may not import
 *  from core, so a test pins the two together): control, format (the bidirectional marks, zero-width
 *  and tag characters) and line/paragraph separators become spaces, and a run of combining marks is
 *  cut to three so a name can't stack accents over what is beside it. */
const INVISIBLE = /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu;
const MARK_RUN = /(\p{M}{3})\p{M}+/gu;

/** The tier word for a maker: what the ledger says, in plain words. Any unsigned maker is "unsigned",
 *  whatever tier it claims. */
export function tierWord(made: MadeBy): string {
  if (!made.signed) return "unsigned";
  switch (made.tier) {
    case "first":
      return "first party";
    case "second":
      return "reviewed by Atlas";
    default:
      return "third party";
  }
}

/** A mod's name fit to draw: no control or invisible characters, at most [`MADE_BY_NAME_MAX`], and "a
 *  mod" for one that has nothing left. */
export function madeByName(made: MadeBy): string {
  const clean = String(made.name ?? "")
    .replace(INVISIBLE, " ")
    .replace(MARK_RUN, "$1")
    .replace(/\s+/g, " ")
    .trim();
  if (clean === "") return "a mod";
  const chars = [...clean];
  return chars.length > MADE_BY_NAME_MAX
    ? `${chars.slice(0, MADE_BY_NAME_MAX - 1).join("")}…`
    : clean;
}

/** The whole label as one plain string, for a tooltip or a line: "Pinboard (first party)". */
export function madeByText(made: MadeBy): string {
  return `${madeByName(made)} (${tierWord(made)})`;
}
