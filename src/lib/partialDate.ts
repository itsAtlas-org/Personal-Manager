// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Dates a person can hold with parts missing: a birthday with no year, "met in 2019", a school that
// ran "2008" to "2012". The stored forms are `YYYY-MM-DD`, `--MM-DD` (no year), `YYYY-MM` and `YYYY`,
// and this file reads and writes them exactly as `src-tauri/src/people/partial_date.rs` does (the
// Rust side is the authority: it refuses what this one lets through, never the other way round).
// Pure: no clock of its own (callers hand in `today`), no IPC.
//
// The editor shows a partial date as three small fields (day, month, year) rather than a native date
// input, which WebKitGTK doesn't draw. `DateFields` is what those three hold as typed text, and
// `isoFromFields` turns them into a stored value or says in plain words why it can't.

/** One parsed date. At least a year, or a month and a day, is always present. */
export interface PartialDate {
  year: number | null;
  month: number | null;
  day: number | null;
}

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

const isLeap = (y: number) => (y % 4 === 0 && y % 100 !== 0) || y % 400 === 0;

/** The year used only to check a year-less day: a leap one, so 29 February passes (Rust's
 *  `LEAP_PLACEHOLDER`). */
const LEAP_PLACEHOLDER = 2000;

function daysIn(year: number, month: number): number {
  if ([1, 3, 5, 7, 8, 10, 12].includes(month)) return 31;
  if ([4, 6, 9, 11].includes(month)) return 30;
  return isLeap(year) ? 29 : 28;
}

/** Exactly `n` ASCII digits, as a number; null for anything else. */
function digits(text: string, n: number): number | null {
  return text.length === n && /^[0-9]+$/.test(text) ? Number(text) : null;
}

/** Read `YYYY`, `YYYY-MM`, `YYYY-MM-DD` or `--MM-DD`, refusing anything that is not a real date (a
 *  13th month, 31 April, 29 February of a common year). Year 0000 is refused, as in Rust. */
export function parsePartial(input: string | null | undefined): PartialDate | null {
  if (input == null) return null;
  const text = input.trim();
  if (text.startsWith("--")) {
    const [m, d, ...extra] = text.slice(2).split("-");
    if (m === undefined || d === undefined || extra.length > 0) return null;
    const month = digits(m, 2);
    const day = digits(d, 2);
    if (month === null || day === null) return null;
    if (month < 1 || month > 12 || day === 0 || day > daysIn(LEAP_PLACEHOLDER, month)) return null;
    return { year: null, month, day };
  }
  // Rust splits into at most three pieces, so a fourth "-" lands inside the day and fails there.
  const [y, m, ...rest] = text.split("-");
  const d = rest.length > 0 ? rest.join("-") : undefined;
  const year = digits(y, 4);
  if (year === null || year === 0) return null;
  if (m === undefined) return { year, month: null, day: null };
  const month = digits(m, 2);
  if (month === null || month < 1 || month > 12) return null;
  if (d === undefined) return { year, month, day: null };
  const day = digits(d, 2);
  if (day === null || day === 0 || day > daysIn(year, month)) return null;
  return { year, month, day };
}

const p2 = (n: number) => String(n).padStart(2, "0");
const p4 = (n: number) => String(n).padStart(4, "0");

/** The stored text: `YYYY-MM-DD`, `--MM-DD`, `YYYY-MM` or `YYYY`. */
export function toIso(d: PartialDate): string {
  if (d.year !== null && d.month !== null && d.day !== null)
    return `${p4(d.year)}-${p2(d.month)}-${p2(d.day)}`;
  if (d.year === null && d.month !== null && d.day !== null) return `--${p2(d.month)}-${p2(d.day)}`;
  if (d.year !== null && d.month !== null) return `${p4(d.year)}-${p2(d.month)}`;
  if (d.year !== null) return p4(d.year);
  return "";
}

/** How PM shows it, always with its year when known: `DD-MM-YYYY`, `MM-YYYY`, `YYYY`, or
 *  "15 April" when the year is unknown (Rust's `to_display`). */
export function toDisplay(d: PartialDate): string {
  if (d.year !== null && d.month !== null && d.day !== null)
    return `${p2(d.day)}-${p2(d.month)}-${p4(d.year)}`;
  if (d.year === null && d.month !== null && d.day !== null)
    return `${d.day} ${MONTHS[d.month - 1]}`;
  if (d.year !== null && d.month !== null) return `${p2(d.month)}-${p4(d.year)}`;
  if (d.year !== null) return p4(d.year);
  return "";
}

/** A stored date as the record words it: always with its year when it has one (DD-MM-YYYY, MM-YYYY,
 *  YYYY), unlike `format.ts`, which drops the current year from a timestamp: "05-04" beside
 *  "04-2026" would read as 5 April or April 2005. A value that isn't one of the stored shapes is
 *  shown as it is, as text. */
export function formatPartial(iso: string | null | undefined): string {
  const d = parsePartial(iso);
  if (!d) return iso ?? "";
  return toDisplay(d);
}

/** The age on `today` of someone born on `d`, or null with no year or day to count from, or when the
 *  year is still ahead (a typo has no age to say). */
export function ageOn(d: PartialDate, today: Date): number | null {
  if (d.year === null || d.month === null || d.day === null) return null;
  const m = today.getMonth() + 1;
  const hadIt = m > d.month || (m === d.month && today.getDate() >= d.day);
  const age = today.getFullYear() - d.year - (hadIt ? 0 : 1);
  return age >= 0 ? age : null;
}

/** A birthday as the record words it: "15 April" while the year is unknown, "15-04-1990 (36)" once it
 *  is (always with the year, never dropped, and with the age as of `today`). Null for a value that
 *  isn't a stored birthday. */
export function describeBirthday(iso: string | null | undefined, today: Date): string | null {
  const d = parsePartial(iso);
  if (!d || d.month === null || d.day === null) return null;
  if (d.year === null) return toDisplay(d);
  const age = ageOn(d, today);
  return `${toDisplay(d)}${age === null ? "" : ` (${age})`}`;
}

// --- the three fields ------------------------------------------------------------------------------

/** What a date is allowed to be. `born`: a day and a month, the year optional (a birthday). `dated`:
 *  a year, with a month and a day when known (when you met, a job's start). `any`: either (an
 *  anniversary may have no year). */
export type DateMode = "born" | "dated" | "any";

/** The three small fields as typed. */
export interface DateFields {
  day: string;
  month: string;
  year: string;
}

export const NO_DATE: DateFields = { day: "", month: "", year: "" };

/** The fields for a stored date; empty for none, or for a value that isn't a stored shape. */
export function fieldsFromIso(iso: string | null | undefined): DateFields {
  const d = parsePartial(iso);
  if (!d) return { ...NO_DATE };
  return {
    day: d.day === null ? "" : p2(d.day),
    month: d.month === null ? "" : p2(d.month),
    year: d.year === null ? "" : p4(d.year),
  };
}

const NEEDS: Record<DateMode, string> = {
  born: "A birthday needs a day and a month. The year is optional.",
  dated: "Give a year first, then a month and a day if you know them.",
  any: "Give a year, or a day and a month.",
};

/** The stored value for what was typed, or the plain-words reason it can't be. All three empty is
 *  no date at all (`iso: null`), never an error. */
export function isoFromFields(
  fields: DateFields,
  mode: DateMode,
): { iso: string | null; error: null } | { iso: null; error: string } {
  const day = fields.day.trim();
  const month = fields.month.trim();
  const year = fields.year.trim();
  if (day === "" && month === "" && year === "") return { iso: null, error: null };
  if ([day, month, year].some((f) => f !== "" && !/^[0-9]+$/.test(f)))
    return { iso: null, error: "Use numbers only." };
  if (year !== "" && year.length !== 4) return { iso: null, error: "Use a four-digit year." };
  if (day.length > 2 || month.length > 2) return { iso: null, error: "That isn't a real date." };
  const shape = (day ? "d" : "") + (month ? "m" : "") + (year ? "y" : "");
  // Which of the four stored shapes this is: day+month, year, year+month or all three.
  const allowed =
    mode === "born"
      ? ["dm", "dmy"]
      : mode === "dated"
        ? ["y", "my", "dmy"]
        : ["dm", "dmy", "y", "my"];
  if (!allowed.includes(shape)) return { iso: null, error: NEEDS[mode] };
  const text =
    shape === "dm"
      ? `--${p2(Number(month))}-${p2(Number(day))}`
      : shape === "y"
        ? year
        : shape === "my"
          ? `${year}-${p2(Number(month))}`
          : `${year}-${p2(Number(month))}-${p2(Number(day))}`;
  const parsed = parsePartial(text);
  if (!parsed) return { iso: null, error: "That isn't a real date." };
  return { iso: toIso(parsed), error: null };
}
