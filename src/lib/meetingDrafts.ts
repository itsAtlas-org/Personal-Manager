// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What a "Who did you meet?" card has been given but not saved yet: the guests ticked, the people
// picked with "Someone else…" and the note. Clicking a guest's name opens that person's record and
// closes the dialog, so without somewhere to keep this the card would come back empty.
//
// It lives in this module's memory and nowhere else: nothing is written to disk or sent anywhere,
// and it goes when the app does. A card's entry goes as soon as the card is answered (saved or
// skipped), so a reopened card starts clean.

import { guestKey, initialTicks, meetingKey, personKey } from "./meetings";
import type { OwedMeeting, PersonRow } from "./types";

/** One card's unsaved input. */
export interface MeetingDraft {
  /** The ticked keys (`person:{id}`, `guest:{email}`). */
  ticks: string[];
  /** Every key the card offered when this was kept, so a guest who has become known since can still
   *  start ticked. */
  offered: string[];
  extra: PersonRow[];
  note: string;
}

const drafts = new Map<string, MeetingDraft>();

/** What a card is known by: its iCal id and occurrence, and for an event with no iCal id (which can
 *  only be logged) its title too, since those share an empty id. */
export function draftKey(m: Pick<OwedMeeting, "uid" | "start" | "title">): string {
  return m.uid === "" ? `${meetingKey(m)}\n${m.title}` : meetingKey(m);
}

/** Every key a card offers a tick for. */
function offeredKeys(m: Pick<OwedMeeting, "known" | "unknown">): string[] {
  return [...m.known.map((g) => personKey(g.id)), ...m.unknown.map(guestKey)];
}

/** What a card starts with: the kept input where there is some (mapped onto the guests the card has
 *  now), else the usual start. */
export function restoreDraft(m: OwedMeeting): {
  ticks: Set<string>;
  extra: PersonRow[];
  note: string;
} {
  const kept = drafts.get(draftKey(m));
  if (!kept) return { ticks: initialTicks(m), extra: [], note: "" };
  const offered = new Set(offeredKeys(m));
  const before = new Set(kept.offered);
  const ticks = new Set(kept.ticks.filter((k) => offered.has(k)));
  // A guest PM only knows since: ticked, as a card opened fresh would have them.
  for (const k of initialTicks(m)) if (!before.has(k)) ticks.add(k);
  const known = new Set(m.known.map((g) => g.id));
  return { ticks, extra: kept.extra.filter((p) => !known.has(p.id)), note: kept.note };
}

/** Keep a card's input, or let go of its entry when the card is as it started. */
export function keepDraft(
  m: OwedMeeting,
  input: { ticks: ReadonlySet<string>; extra: readonly PersonRow[]; note: string },
): void {
  const start = initialTicks(m);
  const pristine =
    input.extra.length === 0 &&
    input.note === "" &&
    input.ticks.size === start.size &&
    [...input.ticks].every((k) => start.has(k));
  const key = draftKey(m);
  if (pristine) {
    drafts.delete(key);
    return;
  }
  drafts.set(key, {
    ticks: [...input.ticks],
    offered: offeredKeys(m),
    extra: [...input.extra],
    note: input.note,
  });
}

/** Forget a card's input: it was answered. */
export function forgetDraft(m: Pick<OwedMeeting, "uid" | "start" | "title">): void {
  drafts.delete(draftKey(m));
}

/** Forget everything: the store closed behind a lock (App), and between tests. */
export function forgetAllDrafts(): void {
  drafts.clear();
}
