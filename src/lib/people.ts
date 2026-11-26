// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The People tab's pure helpers: what a row says, the colour of an initials circle, the birthdays
// coming up, the sort the list is ordered by. No IPC, no clock of their own (callers hand in
// `today`), so each is tested on its own.

import { describeBirthday } from "./partialDate";
import type { PeopleSort, PersonLabelRef, PersonRow } from "./types";

/** Words for the person from a rejected write: the backend's refusals are fixed sentences (a
 *  string over IPC), so they are shown as they are; anything else gets a plain line, never the
 *  error's own text, which could hold a contact's value. */
export function errorWords(err: unknown, fallback = "PM couldn't do that. Try again."): string {
  return typeof err === "string" && err ? err : fallback;
}

/** A label name as the backend compares it (`fold_key`, E10): NFKD, the Latin combining marks
 *  U+0300-U+036F dropped, NFC, lowercase, whitespace collapsed. "Cafe" and "Café" are one label, so
 *  the front end finds the existing one instead of asking for a second the backend would refuse. */
export function foldName(name: string): string {
  return name
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .normalize("NFC")
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .join(" ");
}

// --- initials and their colour --------------------------------------------------------------------

/** The first letter or digit of `word`, as one whole character (a code point, so an emoji or a
 *  character outside the Basic Multilingual Plane is never cut in half). */
function firstLetter(word: string): string {
  for (const ch of word) if (/[\p{L}\p{N}]/u.test(ch)) return ch.toUpperCase();
  return "";
}

/** One or two letters for the circle: the first letters of the first and last words of the name
 *  ("Ada Lovelace" is AL, "Ada Byron Lovelace" is still AL), or the one letter of a one-word name
 *  (a company, a mononym, a name in a script that doesn't use spaces). A name with no letter at all
 *  is "?". */
export function personInitials(name: string): string {
  const words = name
    .trim()
    .split(/\s+/)
    .map(firstLetter)
    .filter((l) => l !== "");
  if (words.length === 0) return "?";
  if (words.length === 1) return words[0];
  return words[0] + words[words.length - 1];
}

/** The design tokens an initials circle is tinted from. A fixed list of existing tokens, never a
 *  hex value: the design system has no per-person colour and this one decorates, it never carries
 *  meaning (the name is always beside it). */
export const PERSON_TOKENS = [
  "--st-due",
  "--st-blocked",
  "--st-quick",
  "--st-look",
  "--st-part",
  "--st-track",
] as const;

/** FNV-1a over the UTF-16 units: small, stable across sessions and machines, and needs no library.
 *  The same person always lands on the same colour, whatever the list around them looks like. */
function hash(text: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h;
}

/** The token of a person's circle, picked by a hash of their `uid`. */
export function personToken(ident: string): (typeof PERSON_TOKENS)[number] {
  return PERSON_TOKENS[hash(ident) % PERSON_TOKENS.length];
}

/** The circle's background: the token mixed into `--bg`, so the initials in `--ink` stay readable
 *  on every theme and Accent. */
export function personCircleBackground(ident: string): string {
  return `color-mix(in oklab, var(${personToken(ident)}) 28%, var(--bg))`;
}

// --- what a row says -------------------------------------------------------------------------------

/** The row's one secondary line: the company, else the primary email or phone, else nothing. */
export function secondaryLine(row: Pick<PersonRow, "company" | "primary">): string {
  return row.company?.trim() || row.primary?.trim() || "";
}

/** The chips a row shows and how many more there are: three is what fits beside a name. */
export function visibleLabels(
  labels: readonly PersonLabelRef[],
  max = 3,
): { shown: PersonLabelRef[]; more: number } {
  return { shown: labels.slice(0, max), more: Math.max(0, labels.length - max) };
}

// --- birthdays -------------------------------------------------------------------------------------

const MONTHS = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
];

/** A parsed birthday: the year is null when it is unknown (`--MM-DD`). */
export interface BornOn {
  year: number | null;
  month: number;
  day: number;
}

const isLeap = (y: number) => (y % 4 === 0 && y % 100 !== 0) || y % 400 === 0;
const daysIn = (y: number, m: number) =>
  [31, isLeap(y) ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][m - 1];

/** Read `YYYY-MM-DD` or `--MM-DD`. Anything else, or a day that doesn't exist (a year-less 29
 *  February does), is null: the backend stores only valid ones, so this is a guard, not a rule. */
export function parseBornOn(value: string | null | undefined): BornOn | null {
  if (!value) return null;
  const full = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  const bare = /^--(\d{2})-(\d{2})$/.exec(value);
  const parts = full
    ? { year: Number(full[1]), month: Number(full[2]), day: Number(full[3]) }
    : bare
      ? { year: null, month: Number(bare[1]), day: Number(bare[2]) }
      : null;
  if (!parts || parts.month < 1 || parts.month > 12 || parts.day < 1) return null;
  if (parts.day > daysIn(parts.year ?? 2000, parts.month)) return null;
  return parts;
}

/** "15 April": the year-less form, and the strip's. */
export function dayAndMonth(born: Pick<BornOn, "month" | "day">): string {
  return `${born.day} ${MONTHS[born.month - 1]}`;
}

/** A birthday as a line: "Birthday 15 April" while the year is unknown, "Birthday 15-04-1990 (36)"
 *  once it is (DD-MM-YYYY, with the age as of `today`). The record words the value alone through
 *  `describeBirthday`, the one place the age is counted. */
export function birthdayText(value: string | null | undefined, today: Date): string | null {
  const text = describeBirthday(value, today);
  return text === null ? null : `Birthday ${text}`;
}

/** One entry of the strip: who, the day it falls on and how far off that is. */
export interface UpcomingBirthday {
  row: PersonRow;
  /** 0 for today. */
  inDays: number;
  /** "Birthday 15 April", with "turns 36" when the year is known. */
  text: string;
}

const DAY_MS = 86_400_000;

/** The days from `today` to the next time this birthday falls (0 is today), and the year it falls
 *  in. A 29 February in a common year is kept on 1 March, as UK law counts it. */
function nextOccurrence(born: BornOn, today: Date): { inDays: number; year: number } {
  const todayUtc = Date.UTC(today.getFullYear(), today.getMonth(), today.getDate());
  for (const year of [today.getFullYear(), today.getFullYear() + 1]) {
    const leapDay = born.month === 2 && born.day === 29 && !isLeap(year);
    const at = Date.UTC(year, leapDay ? 2 : born.month - 1, leapDay ? 1 : born.day);
    if (at >= todayUtc) return { inDays: Math.round((at - todayUtc) / DAY_MS), year };
  }
  // Unreachable: next year always has the day (or its 1 March stand-in).
  return { inDays: 366, year: today.getFullYear() + 1 };
}

/** Who has a birthday in the next `days` days (today included), soonest first, then by name. Takes
 *  the list as read, so people in Trash are never in it (the All filter leaves them out). */
export function upcomingBirthdays(
  rows: readonly PersonRow[],
  today: Date,
  days = 30,
): UpcomingBirthday[] {
  const out: UpcomingBirthday[] = [];
  for (const row of rows) {
    if (row.trashed_at) continue;
    const born = parseBornOn(row.birthday);
    if (!born) continue;
    const { inDays, year } = nextOccurrence(born, today);
    if (inDays > days) continue;
    // A year ahead of this one (a typo, or a birth year of 2999) has no age to say.
    const turns = born.year === null || year - born.year <= 0 ? "" : `, turns ${year - born.year}`;
    out.push({ row, inDays, text: `Birthday ${dayAndMonth(born)}${turns}` });
  }
  return out.sort(
    (a, b) => a.inDays - b.inDays || a.row.display_name.localeCompare(b.row.display_name),
  );
}

// --- sort, remembered on this device -----------------------------------------------------------------

/** The three orders, with the words the control shows. */
export const SORT_OPTIONS: readonly { value: PeopleSort; label: string }[] = [
  { value: "First", label: "First name" },
  { value: "Last", label: "Last name" },
  { value: "RecentlyMet", label: "Recently met" },
];

const SORT_KEY = "pm.people.sort";

/** The remembered sort: a closed enumeration, device-local like every other list order, and the
 *  first-name default when nothing (or something unknown) is stored. */
export function readPeopleSort(): PeopleSort {
  try {
    const v = localStorage.getItem(SORT_KEY);
    return SORT_OPTIONS.some((o) => o.value === v) ? (v as PeopleSort) : "First";
  } catch {
    return "First";
  }
}

export function writePeopleSort(sort: PeopleSort): void {
  try {
    localStorage.setItem(SORT_KEY, sort);
  } catch {
    // Best-effort: the choice just isn't remembered on this device.
  }
}

// --- selection ---------------------------------------------------------------------------------------

/** The ids from `anchor` to `target` in list order, both ends included (Shift-click). Either end
 *  missing from the list leaves just `target`. */
export function idsBetween(rows: readonly { id: number }[], anchor: number | null, target: number) {
  const a = anchor === null ? -1 : rows.findIndex((r) => r.id === anchor);
  const b = rows.findIndex((r) => r.id === target);
  if (a < 0 || b < 0) return [target];
  const [from, to] = a < b ? [a, b] : [b, a];
  return rows.slice(from, to + 1).map((r) => r.id);
}

/** The labels every one of `rows` carries (what the label picker shows ticked), and those only
 *  some do (shown as mixed). */
export function labelCoverage(rows: readonly { labels: readonly PersonLabelRef[] }[]): {
  all: Set<number>;
  some: Set<number>;
} {
  const counts = new Map<number, number>();
  for (const r of rows) for (const l of r.labels) counts.set(l.id, (counts.get(l.id) ?? 0) + 1);
  const all = new Set<number>();
  const some = new Set<number>();
  for (const [id, n] of counts) (n === rows.length ? all : some).add(id);
  return { all, some };
}
