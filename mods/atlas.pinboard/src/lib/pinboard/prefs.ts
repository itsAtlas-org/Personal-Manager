// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Local UI preferences for the Pinboard — deliberately NOT board state. The board itself is user content
// (see usePinboard); these are three nags the user has waved away.
//
// In the core Pinboard they were three `localStorage` keys. A mod's frame has no `localStorage` at all,
// so the three live in ONE mod-storage key, `prefs` (Mods plan K7). The page reads the key once before
// it draws (`loadPrefs`, from the frame's start-up), the readers below answer from that copy, and a
// write updates the copy and saves the whole key in the background. The readers stay synchronous and
// read at the point of use, as core's did: nothing here is a React store. A failed read leaves every
// preference at its default (the safe one: keep asking, show everything), and a failed write just means
// the choice is not remembered: the page never throws over a preference.

/** The three preferences, and what each is until the person says otherwise. */
export interface Prefs {
  confirmDelete: boolean;
  showPast: boolean;
  showCompleted: boolean;
}

const DEFAULTS: Prefs = { confirmDelete: true, showPast: true, showCompleted: true };

/** Where the preferences are kept: the frame's own storage key. */
export const PREFS_KEY = "prefs";

/** The Prefs a stored value means: a field that is not exactly `false` is its default (the same
 *  reading core's `localStorage` strings had: only the literal "false" turned a nag off). */
export function readPrefs(raw: unknown): Prefs {
  const r = typeof raw === "object" && raw !== null && !Array.isArray(raw) ? raw : {};
  const field = (name: keyof Prefs) => (r as Record<string, unknown>)[name] !== false;
  return {
    confirmDelete: field("confirmDelete"),
    showPast: field("showPast"),
    showCompleted: field("showCompleted"),
  };
}

let current: Prefs = DEFAULTS;
let saver: ((prefs: Prefs) => void) | null = null;

/** Adopt what storage held (`readPrefs` of it), and the way later changes are saved. */
export function startPrefs(stored: unknown, save: (prefs: Prefs) => void): void {
  current = readPrefs(stored);
  saver = save;
}

/** Back to the defaults with nothing to save to (a page that was torn down, and tests). */
export function resetPrefs(): void {
  current = DEFAULTS;
  saver = null;
}

function change(patch: Partial<Prefs>): void {
  current = { ...current, ...patch };
  saver?.(current);
}

/** Ask before deleting a note or timeline? On by default — the first time you delete something must
 *  not be the time you find out there was no confirmation. Only an explicit "don't ask again" turns
 *  it off, and Settings turns it back on. */
export function readConfirmDelete(): boolean {
  return current.confirmDelete;
}

export function writeConfirmDelete(on: boolean): void {
  change({ confirmDelete: on });
}

/** Show entries whose date has already gone by, on a timeline card? True by default — same reasoning
 *  as the project panel's "Completed" checkbox: opening a card must not look like it lost data.
 *
 *  Deliberately keyed on the DATE, not on whether a milestone is marked done. The project panel hides
 *  *completed* work because there the question is "what's left to do"; a timeline is a picture of
 *  when things happen, so what crowds it is everything already behind you — including things that
 *  passed without being ticked off. A separate pref from the panel's for the same reason: they answer
 *  different questions, and one switch would make each of them wrong half the time. */
export function readShowPastTimelineItems(): boolean {
  return current.showPast;
}

export function writeShowPastTimelineItems(show: boolean): void {
  change({ showPast: show });
}

/** Show milestones marked Done on a project-linked timeline card? True by default, for the same
 *  reason as its two siblings: opening a card must not look like it lost data.
 *
 *  This is the "what's left to do" question the project panel's "Completed" checkbox answers, asked
 *  again on the board — and it is a SEPARATE pref from that checkbox on purpose. The panel and a
 *  board card are looked at in different moods, and sharing one switch would mean tidying the board
 *  silently tidied the project page too. It is also separate from `showPast` above, which keys on
 *  the DATE: a milestone can be done but still ahead of you, or long past and never ticked, so one
 *  switch could not express both. Freeform timelines have no notion of done and get no such toggle. */
export function readShowCompletedTimelineItems(): boolean {
  return current.showCompleted;
}

export function writeShowCompletedTimelineItems(show: boolean): void {
  change({ showCompleted: show });
}

/**
 * Whether a timeline entry's date is already behind us. Compared as `YYYY-MM-DD` STRINGS against
 * today's local calendar day rather than by constructing a `Date`: a bare date parsed by `new Date`
 * is read as UTC midnight, which lands on the previous day west of Greenwich (F-14) — so a milestone
 * due today would read as past for a whole timezone's worth of users.
 *
 * Undated entries are never past. They have no position on the timeline to have gone by, and hiding
 * them would silently swallow the ones a user has typed but not dated yet.
 */
export function isPastTimelineDate(date: string | null | undefined, todayIso: string): boolean {
  if (!date) return false;
  const day = date.slice(0, 10);
  return /^\d{4}-\d{2}-\d{2}$/.test(day) && day < todayIso;
}
