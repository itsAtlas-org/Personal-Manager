// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the duplicate review and the merge dialog say, as pure functions: the reason a pair was
// flagged in a plain sentence, the rows both cards draw so their labels line up, the "Nothing
// differs" group and its preview line, and the merge dialog's body. No IPC and no clock of their
// own (callers hand in `today`), so each is tested alone. The rules themselves (what differs, what a
// merge would lose) live in Rust (`people::merge`): this file only words and arranges them.

import { describeBirthday, formatPartial } from "./partialDate";
import type {
  DupeCard,
  DupePair,
  DupeReason,
  DupesReport,
  MergeDiffers,
  PeopleMergePreview,
} from "./types";

/** The reason, as the pair shows it ("Same email"). */
export const REASON_WORDS: Record<DupeReason, string> = {
  Email: "Same email",
  Phone: "Same phone number",
  Name: "Same name",
  Similar: "Similar name",
};

const REASON_SENTENCE: Record<DupeReason, string> = {
  Email: "They have the same email address.",
  Phone: "They have the same phone number.",
  Name: "They have the same name.",
  Similar: "Their names are alike, like Sam and Samuel.",
};

/** Why a pair was flagged, as sentences, strongest first. A reason is never a score. */
export function reasonSentence(reasons: readonly DupeReason[]): string {
  return reasons.map((r) => REASON_SENTENCE[r]).join(" ");
}

/** The pair's reasons as short labels ("Same email · Same name"). */
export function reasonLabels(reasons: readonly DupeReason[]): string {
  return reasons.map((r) => REASON_WORDS[r]).join(" · ");
}

/** "Possible duplicates (3)"; plain "Possible duplicates" when none were found but the scan skipped
 *  something or some pairs are hidden (else an incomplete scan would look clean, and "Show them
 *  again", which lives in the panel, could not be reached); null otherwise (the button isn't drawn). */
export function duplicatesButtonLabel(report: DupesReport | null): string | null {
  if (report === null) return null;
  const n = report.pairs.length;
  if (n > 0) return `Possible duplicates (${n})`;
  const incomplete = report.skipped.length > 0 || report.cut_short;
  return incomplete || report.hidden > 0 ? "Possible duplicates" : null;
}

/** The report as the panel shows it: pairs that merge with nothing lost, and the rest. A pair whose
 *  reasons include "Similar name" never joins the first group (the backend already says so; this
 *  repeats it so a stale or hand-built report can't). */
export function groupPairs(report: DupesReport): { identical: DupePair[]; rest: DupePair[] } {
  const identical: DupePair[] = [];
  const rest: DupePair[] = [];
  for (const p of report.pairs) {
    (p.nothing_differs && !p.reasons.includes("Similar") ? identical : rest).push(p);
  }
  return { identical, rest };
}

/** The "Nothing differs" preview line: "12 pairs merge with nothing lost. 4 are left for you." */
export function nothingDiffersLine(identical: number, rest: number): string {
  const lost = `${identical === 1 ? "1 pair merges" : `${identical} pairs merge`} with nothing lost.`;
  const left = rest === 1 ? "1 is left for you." : `${rest} are left for you.`;
  return `${lost} ${left}`;
}

/** The hidden-pairs line: "2 pairs you said aren't the same person are hidden." */
export function hiddenLine(n: number): string {
  return n === 1
    ? "1 pair you said isn't the same person is hidden."
    : `${n} pairs you said aren't the same person are hidden.`;
}

/** What the scan did not check, in words, or null when it checked everything. A report that skipped
 *  anything is never presented as clean. */
export function incompleteLine(report: DupesReport): string | null {
  const parts: string[] = [];
  if (report.skipped.length > 0) {
    const n = report.skipped.length;
    parts.push(
      `PM didn't compare ${n === 1 ? "1 very large group" : `${n} very large groups`} of people who share one email, number or name, so this list may be missing some.`,
    );
  }
  if (report.cut_short) {
    parts.push("PM stopped at the first pairs it found, so there may be more.");
  }
  return parts.length > 0 ? parts.join(" ") : null;
}

// --- the side-by-side grid -------------------------------------------------------------------------

/** One row of the side-by-side grid: a label and what each person has under it (nothing is an empty
 *  list, drawn as a dash, so an unknown on one side never shifts the other out of step). */
export interface CardRow {
  key: string;
  label: string;
  left: string[];
  right: string[];
}

const KIND_LABEL: Record<string, string> = {
  email: "Email",
  phone: "Phone",
  address: "Address",
  web: "Links",
  work: "Work",
  school: "School",
  nickname: "Nickname",
  former_name: "Former name",
  date: "Other dates",
  relation: "Relations",
  custom: "Custom fields",
  location: "Where they are now",
};

/** The kinds in the order the grid lists them. */
const KIND_ORDER = Object.keys(KIND_LABEL);

function metText(card: DupeCard): string[] {
  const when = card.met_on ? formatPartial(card.met_on) : "";
  const text = [card.met_where, when].filter(Boolean).join(", ");
  return text ? [text] : [];
}

function detailTexts(card: DupeCard, kind: string): string[] {
  return card.details
    .filter((d) => d.kind === kind)
    .map((d) => (d.label.trim() ? `${d.value} (${d.label})` : d.value));
}

/** Every row both cards draw, in one order, so the labels line up whatever either side holds. A
 *  row appears when either person has something for it; the name, birthday, where you met and the
 *  note always appear (they are what a merge asks about). */
export function cardRows(a: DupeCard, b: DupeCard, today: Date): CardRow[] {
  const born = (c: DupeCard) => {
    const text = describeBirthday(c.birthday, today);
    return text ? [text] : [];
  };
  const note = (c: DupeCard) => (c.note.trim() ? [c.note.trim()] : []);
  const rows: CardRow[] = [
    { key: "birthday", label: "Birthday", left: born(a), right: born(b) },
    { key: "met", label: "Where you met", left: metText(a), right: metText(b) },
  ];
  for (const kind of KIND_ORDER) {
    const left = detailTexts(a, kind);
    const right = detailTexts(b, kind);
    if (left.length > 0 || right.length > 0) {
      rows.push({ key: kind, label: KIND_LABEL[kind], left, right });
    }
  }
  rows.push({ key: "notes", label: "Notes", left: note(a), right: note(b) });
  return rows;
}

// --- the merge dialog ----------------------------------------------------------------------------------

const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** The dialog's body sentence with the counts of the merged record. When nothing differs it says
 *  nothing is dropped instead of asking for choices. */
export function mergeBody(p: PeopleMergePreview): string {
  const moved = `Everything from both moves to one record: ${count(p.details, "detail", "details")}, ${count(p.timeline, "timeline entry", "timeline entries")}, ${count(p.labels, "label", "labels")}.`;
  if (p.differs.name || p.differs.birthday || p.differs.met || p.differs.notes) {
    return `${moved} Where they differ, choose which to keep. The one you don't choose is dropped.`;
  }
  if (p.lossless) return `${moved} Nothing differs, so nothing is dropped.`;
  // Nothing to choose, but a detail both have (the same one, maybe spelled or dated another way) is
  // kept once: say so rather than promise nothing is dropped.
  return p.details_dropped > 0
    ? `${moved} Nothing needs a choice. ${count(p.details_dropped, "detail is", "details are")} the same on both and kept once.`
    : `${moved} Nothing needs a choice.`;
}

/** The dialog's second line: the project links and the places they came from (a vCard file, a
 *  Google contact) that move too, so every link is seen to be kept. Null when there are none. */
export function mergeExtrasLine(p: PeopleMergePreview): string | null {
  const parts: string[] = [];
  if (p.projects > 0) parts.push(count(p.projects, "project link", "project links"));
  if (p.sources > 0) parts.push(count(p.sources, "place they came from", "places they came from"));
  return parts.length > 0 ? `Also ${parts.join(" and ")}, all kept.` : null;
}

/** The fields that differ, in the order the dialog asks about them. */
export function differingFields(d: MergeDiffers): ("name" | "birthday" | "met" | "notes")[] {
  const out: ("name" | "birthday" | "met" | "notes")[] = [];
  if (d.name) out.push("name");
  if (d.birthday) out.push("birthday");
  if (d.met) out.push("met");
  if (d.notes) out.push("notes");
  return out;
}

/** The strip's words after one merge ("Merged into Sam Reed."), or after a batch ("Merged 3
 *  duplicates."). */
export function mergedNotice(done: { display_name: string; merged: number }): string {
  return done.merged === 1
    ? `Merged into ${done.display_name}.`
    : `Merged ${done.merged} duplicates.`;
}

/** How many pairs a batch names: "Merge these 12". */
export const mergeTheseLabel = (n: number) => `Merge these ${n}`;
