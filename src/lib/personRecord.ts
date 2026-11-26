// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the person's record and its editor share, as pure functions: which sections exist and which
// kinds of detail each holds, what a link may be, how a dated detail and a source are worded, and
// "Last met 12-11 (2 weeks ago)". No IPC and no clock of their own (callers hand in `today`).
//
// Contact text is untrusted (a vCard file or, later, Google wrote it), so nothing here builds markup:
// callers render what these return as React text, and the only thing that ever leaves for the OS is
// a target `linkTarget` or `mailTarget` has passed.

import { formatDate, formatDateOnly } from "./format";
import { formatPartial } from "./partialDate";
import type { Person, PersonDetail, PersonDetailKind, PersonSource } from "./types";

// --- sections ----------------------------------------------------------------------------------------

/** The record's sections, and the editor's. `met` and `notes` hold columns, not details. */
export type SectionId =
  "name" | "contact" | "work" | "dates" | "met" | "now" | "relations" | "custom" | "notes";

/** Which kinds of detail a section holds, in the order the section lists them. */
export const SECTION_KINDS: Record<SectionId, readonly PersonDetailKind[]> = {
  name: ["nickname", "former_name"],
  contact: ["email", "phone", "address", "web"],
  work: ["work", "school"],
  dates: ["date"],
  met: [],
  now: ["location"],
  relations: ["relation"],
  custom: ["custom"],
  notes: [],
};

export const SECTION_TITLES: Record<SectionId, string> = {
  name: "Names",
  contact: "Contact",
  work: "Work and school",
  dates: "Dates",
  met: "Where you met",
  now: "Where they are now",
  relations: "Relations",
  custom: "Custom fields",
  notes: "Notes",
};

/** The section a kind of detail belongs to. */
export function sectionOfKind(kind: PersonDetailKind): SectionId {
  for (const id of Object.keys(SECTION_KINDS) as SectionId[]) {
    if (SECTION_KINDS[id].includes(kind)) return id;
  }
  return "custom";
}

/** The details of `kinds`, grouped by kind in the order given and in their own order within a kind
 *  (the backend numbers each kind from 0, and 0 is the primary). */
export function detailsOfKinds(
  person: Pick<Person, "details">,
  kinds: readonly PersonDetailKind[],
): PersonDetail[] {
  return kinds.flatMap((k) =>
    person.details
      .filter((d) => d.kind === k)
      .sort((a, b) => a.position - b.position || a.id - b.id),
  );
}

/** Whether a section has anything to show on this person. */
export function sectionHasContent(person: Person, id: SectionId): boolean {
  switch (id) {
    case "met":
      return person.met_where.trim() !== "" || !!person.met_on;
    case "notes":
      return person.notes.trim() !== "";
    case "dates":
      return !!person.birthday || detailsOfKinds(person, SECTION_KINDS.dates).length > 0;
    default:
      return detailsOfKinds(person, SECTION_KINDS[id]).length > 0;
  }
}

/** The sections "Add details" offers: the ones the person has nothing in yet, in the record's order. */
export function emptySections(person: Person): SectionId[] {
  const order: SectionId[] = [
    "contact",
    "work",
    "dates",
    "met",
    "now",
    "relations",
    "custom",
    "notes",
  ];
  const out = order.filter((id) => !sectionHasContent(person, id));
  // Nicknames and former names show in the name block, so they are offered here, not as a section.
  if (!sectionHasContent(person, "name")) out.push("name");
  return out;
}

// --- what a value may open ---------------------------------------------------------------------------

/** The address to hand to the OS for a web value, or null when the value is not one to open: only
 *  `http://`, `https://` and `mailto:` ever are, and only when it parses and holds no space or
 *  control character. A handle ("@ada_example"), a bare domain and `javascript:` are all null, and
 *  show as text with a Copy button. */
export function linkTarget(value: string): string | null {
  const v = value.trim();
  // Spaces and control characters are checked by code, not by a regex literal that names them
  // (the linter's no-control-regex rule rightly distrusts those).
  if (v === "" || /\s/.test(v) || [...v].some((c) => c < " " || c === "\u007f")) return null;
  const web = /^https?:\/\/[^/?#]/i.test(v);
  const mail = /^mailto:[^?#]/i.test(v);
  if (!web && !mail) return null;
  try {
    const url = new URL(v);
    return ["http:", "https:", "mailto:"].includes(url.protocol) ? v : null;
  } catch {
    return null;
  }
}

/** `mailto:` for an email address, or null when it holds anything that could become a second
 *  header of the message (`?`, `&`, a space) or isn't one `name@host`. */
export function mailTarget(email: string): string | null {
  const v = email.trim();
  return /^[^\s@?#&<>"'%]+@[^\s@?#&<>"'%]+$/.test(v) ? `mailto:${v}` : null;
}

/** What the toast says after Copy: "Phone number copied". */
export function copiedWords(kind: PersonDetailKind): string {
  switch (kind) {
    case "email":
      return "Email address copied";
    case "phone":
      return "Phone number copied";
    case "address":
      return "Address copied";
    case "web":
      return "Link copied";
    default:
      return "Copied";
  }
}

// --- words -------------------------------------------------------------------------------------------

/** How long before `today` a day was, in a few words; null when it is still ahead. */
export function agoText(day: Date, today: Date): string | null {
  const a = Date.UTC(day.getFullYear(), day.getMonth(), day.getDate());
  const b = Date.UTC(today.getFullYear(), today.getMonth(), today.getDate());
  const days = Math.round((b - a) / 86_400_000);
  if (days < 0) return null;
  if (days === 0) return "today";
  if (days === 1) return "yesterday";
  if (days < 14) return `${days} days ago`;
  // 61, not 60: day 60 is the first whole month by the divisor below, and "1 months" is wrong.
  if (days < 61) return `${Math.floor(days / 7)} weeks ago`;
  if (days < 365) return `${Math.floor(days / 30.44)} months ago`;
  const years = Math.floor(days / 365.25);
  return years <= 1 ? "a year ago" : `${years} years ago`;
}

/** "Last met 12-11 (2 weeks ago)", or "Not met yet" with no meeting on record. */
export function lastMetLine(lastMet: string | null | undefined, today: Date): string {
  if (!lastMet) return "Not met yet";
  const m = /^(\d{4})-(\d{2})-(\d{2})/.exec(lastMet);
  if (!m) return `Last met ${formatDate(lastMet)}`;
  const day = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
  const ago = agoText(day, today);
  return `Last met ${formatDateOnly(lastMet)}${ago ? ` (${ago})` : ""}`;
}

/** A dated detail's span: "03-2019 to now", "2008 to 2012", "since 03-2026" (a place), or "" with
 *  neither date. `current` stands in for a missing end ("to now"). */
export function rangeText(d: Pick<PersonDetail, "kind" | "from_date" | "to_date" | "is_current">) {
  const from = d.from_date ? formatPartial(d.from_date) : "";
  const to = d.to_date ? formatPartial(d.to_date) : d.is_current ? "now" : "";
  if (d.kind === "location") return from ? `since ${from}` : "";
  // A job with no start that is still going is "current", not "until now".
  if (!from && !d.to_date && d.is_current) return "current";
  if (from && to) return `${from} to ${to}`;
  if (from) return `from ${from}`;
  if (to) return `until ${to}`;
  return "";
}

/** One source row of the record: "Imported from a file on 12-11". A provider this build doesn't
 *  know gets a plain line that says nothing about it. */
export function sourceLine(source: Pick<PersonSource, "provider" | "created_at">): string {
  // With its year, as in the spec's example: `formatDate` would drop it for an import this year.
  const at = new Date(source.created_at);
  const on = Number.isNaN(at.getTime())
    ? formatDate(source.created_at)
    : `${String(at.getDate()).padStart(2, "0")}-${String(at.getMonth() + 1).padStart(2, "0")}-${at.getFullYear()}`;
  return source.provider === "vcard"
    ? `Imported from a file on ${on}`
    : `Linked from another address book on ${on}`;
}
