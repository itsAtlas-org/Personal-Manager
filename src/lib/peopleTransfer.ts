// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the vCard import and export dialogs say (spec 5.2, 5.3 and 9.2), as pure functions: the
// preview's lines for each count and warning, the result of an import, who an export covers and
// what it wrote. No IPC and no clock, so each is tested alone. The rules (what matches, what is
// kept, what a file may hold) live in Rust (`people::import`, `people::export`); this file only
// words them. Every number arrives as a count: the file's name and the people's names are the only
// text of anyone's own, and the dialogs render them as React text.

import type {
  PeopleExportDone,
  PeopleExportScope,
  PeopleFilter,
  PeopleImportPreview,
  PeopleImportResult,
  PeopleImportWarningKind,
} from "./types";

/** "1 person", "2 people". */
export const peopleCount = (n: number) => `${n} ${n === 1 ? "person" : "people"}`;

/** What the backend says when a held preview can no longer be used (`people::import::STALE`). The
 *  dialog offers to pick the file again when it sees this. */
export const STALE_PREVIEW_START = "That file was read a while ago";

/** The backend's refusal of an old, replaced or foreign preview, as it words it. */
export const isStalePreview = (err: unknown): boolean =>
  typeof err === "string" && err.startsWith(STALE_PREVIEW_START);

// --- the preview ------------------------------------------------------------------------------------

/** "{n} people in {file}": the preview's heading. */
export function previewHeading(p: Pick<PeopleImportPreview, "people" | "file_name">): string {
  return p.file_name
    ? `${peopleCount(p.people)} in ${p.file_name}`
    : `${peopleCount(p.people)} in this file`;
}

const cardsHad = (n: number, rest: string) =>
  `${n === 1 ? "1 card had" : `${n} cards had`} ${rest}`;

/** The warnings other than "no name" (which has its own count on the preview), in plain words. */
const WARNING_WORDS: Record<Exclude<PeopleImportWarningKind, "no_name">, (n: number) => string> = {
  bad_encoding: (n) =>
    `${n === 1 ? "1 line wasn't" : `${n} lines weren't`} in a text encoding PM could be sure of, so PM read ${n === 1 ? "it" : "them"} as plain Western text.`,
  value_cut: (n) =>
    `${n === 1 ? "1 very long value was" : `${n} very long values were`} cut short.`,
  bad_date: (n) =>
    `${n === 1 ? "1 date" : `${n} dates`} couldn't be read, so ${n === 1 ? "it was" : "they were"} kept as written or left out.`,
  unfinished_card: (n) =>
    `${n === 1 ? "1 card stopped" : `${n} cards stopped`} before ${n === 1 ? "its" : "their"} end. PM read as much as it could.`,
  too_many_properties: (n) =>
    `${cardsHad(n, "more details than PM keeps, so the extra ones were left out.")}`,
  too_many_details: (n) =>
    `${cardsHad(n, "more details or labels than one person holds, so the extra ones were left out.")}`,
  unreadable_line: (n) =>
    `${n === 1 ? "1 line" : `${n} lines`} inside ${n === 1 ? "a card" : "cards"} couldn't be read and ${n === 1 ? "was" : "were"} skipped.`,
};

/** Property names in a vCard file, in words, for the "left out" line. Photos have their own line. */
const IGNORED_WORDS: Record<string, string> = {
  LOGO: "logos",
  SOUND: "sounds",
  KEY: "keys",
  SOURCE: "source links",
  FBURL: "free/busy links",
  CALURI: "calendar links",
  GEO: "map positions",
  TZ: "time zones",
  PRODID: "the program that made the file",
  "X-ABUID": "address book ids",
};

/** The most kinds the "left out" line names before it says how many more there were. */
const IGNORED_SHOWN = 6;

/** What the file held that PM doesn't keep, or null when there is nothing to say. Photos are said
 *  on their own line, so they are left out here. */
export function ignoredLine(ignored: readonly { name: string; count: number }[]): string | null {
  const rows = ignored.filter((i) => i.name !== "PHOTO" && i.count > 0);
  if (rows.length === 0) return null;
  const shown = rows.slice(0, IGNORED_SHOWN).map((i) => {
    const words = IGNORED_WORDS[i.name] ?? (i.name.startsWith("X-") ? `${i.name} fields` : i.name);
    return `${words} (${i.count})`;
  });
  const more = rows.length - shown.length;
  return `Left out, because PM doesn't keep them: ${shown.join(", ")}${more > 0 ? `, and ${more} more ${more === 1 ? "kind" : "kinds"}` : ""}.`;
}

/** One line the preview shows below its heading. `tone` is `warn` for something the person may want
 *  to know went wrong or was left out, `info` for how the file compares with PM. */
export interface PreviewLine {
  key: string;
  text: string;
  tone: "info" | "warn";
}

/** Every line under the preview's heading, in order: how the file compares with PM, then what was
 *  skipped or left out. A zero count is never a line. */
export function previewLines(p: PeopleImportPreview): PreviewLine[] {
  const lines: PreviewLine[] = [];
  const add = (key: string, text: string, tone: PreviewLine["tone"] = "info") =>
    lines.push({ key, text, tone });

  if (p.already > 0) {
    add(
      "already",
      `${p.already} ${p.already === 1 ? "is" : "are"} already in PM. PM fills in what's missing and keeps what you have.`,
    );
  }
  if (p.from_trash > 0) {
    add(
      "trash",
      p.from_trash === 1
        ? "1 of them is in Trash, and comes back."
        : `${p.from_trash} of them are in Trash, and come back.`,
    );
  }
  if (p.look_alike > 0) {
    add(
      "alike",
      p.look_alike === 1
        ? "1 looks like someone you have. You can merge them after the import."
        : `${p.look_alike} look like people you have. You can merge them after the import.`,
    );
  }
  if (p.no_name > 0) {
    add(
      "noname",
      p.no_name === 1
        ? "1 card had no name, email or phone and was skipped."
        : `${p.no_name} cards had no name, email or phone and were skipped.`,
      "warn",
    );
  }
  if (p.repeated > 0) {
    add(
      "repeated",
      p.repeated === 1
        ? "1 card repeated an earlier card's id, so it was added as a new person."
        : `${p.repeated} cards repeated an earlier card's id, so they were added as new people.`,
      "warn",
    );
  }
  for (const w of p.warnings) {
    if (w.kind === "no_name" || w.count <= 0) continue;
    const words = WARNING_WORDS[w.kind];
    // A kind this build doesn't know (a newer backend) is skipped, never shown as a raw word.
    if (words) add(`w-${w.kind}`, words(w.count), "warn");
  }
  if (!p.complete) {
    add(
      "incomplete",
      "The file ended early or went past what PM reads at once, so some people may be missing.",
      "warn",
    );
  }
  if (p.photos_skipped) add("photos", "Photos are skipped.");
  const ignored = ignoredLine(p.ignored);
  if (ignored) add("ignored", ignored);
  return lines;
}

// --- the result of an import -------------------------------------------------------------------------

/** One person PM kept its own details for: "Ada Example: birthday, notes". */
export const keptLine = (k: { name: string; fields: readonly string[] }) =>
  `${k.name}: ${k.fields.join(", ")}`;

/** "Imported {n} people." and, when some were already there, how that split. */
export function importedLine(r: PeopleImportResult): string {
  const total = r.added + r.updated + r.unchanged;
  const head = `Imported ${peopleCount(total)}.`;
  const already = r.updated + r.unchanged;
  if (already === 0) return head;
  const parts = [`${r.added} new`];
  if (r.updated > 0) parts.push(`${r.updated} filled in`);
  if (r.unchanged > 0) parts.push(`${r.unchanged} already complete`);
  return `${head} ${parts.join(", ")}.`;
}

/** The most kept notes (and restored names) the result lists before it says how many more. */
export const KEPT_SHOWN = 12;

/** The people who came back from Trash, named: "2 people came back from Trash: Ada, Ben." A long
 *  list names the first few and says how many more. */
export function restoredLine(names: readonly string[]): string | null {
  if (names.length === 0) return null;
  const head = names.length === 1 ? "1 person came back" : `${names.length} people came back`;
  const more = names.length - KEPT_SHOWN;
  const shown = names.slice(0, KEPT_SHOWN).join(", ");
  return `${head} from Trash: ${shown}${more > 0 ? `, and ${more} more` : ""}.`;
}

// --- export ---------------------------------------------------------------------------------------------

/** One way to say who an export covers. `count` is how many people it holds when known; a scope
 *  that doesn't know yet carries the filter to count it with. */
export interface ExportChoice {
  key: string;
  scope: PeopleExportScope;
  /** "Everyone" or "People labelled Friends". */
  label: string;
  count: number | null;
  /** How to count the people when `count` is null. */
  filter: PeopleFilter | null;
}

/** "Export 3 people", or plain "Export people" while the count isn't known. */
export const exportTitle = (count: number | null) =>
  count === null ? "Export people" : `Export ${peopleCount(count)}`;

/** The choices the header's Export… offers. Looking at one label offers that label's people first
 *  and everyone second; any other view offers everyone (the filter is a way to look, not a scope). */
export function headerExportChoices(
  filter: PeopleFilter,
  labelName: string | null,
  visible: { count: number; searching: boolean },
): ExportChoice[] {
  const everyone = (count: number | null): ExportChoice => ({
    key: "all",
    scope: "All",
    label: "Everyone",
    count,
    filter: "All",
  });
  if (typeof filter === "object") {
    return [
      {
        key: "label",
        scope: { Label: filter.Label },
        label: `People labelled ${labelName ?? "this label"}`,
        count: visible.searching ? null : visible.count,
        filter: { Label: filter.Label },
      },
      everyone(null),
    ];
  }
  return [everyone(filter === "All" && !visible.searching ? visible.count : null)];
}

/** The choice of a selection: exactly these people. */
export function selectionExportChoice(ids: readonly number[]): ExportChoice {
  return {
    key: "ids",
    scope: { Ids: [...ids] },
    label: ids.length === 1 ? "The person you ticked" : "The people you ticked",
    count: ids.length,
    filter: null,
  };
}

/** What an export wrote: "Exported 3 people." The files are named beneath it, by the dialog. */
export const exportedLine = (done: Pick<PeopleExportDone, "people">) =>
  `Exported ${peopleCount(done.people)}.`;
