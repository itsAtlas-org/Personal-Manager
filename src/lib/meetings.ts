// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Who did you meet?" (spec 6.3), the parts that need no screen: the strip's words and when it
// stays quiet, the card's ticks, the fold of a long guest list, and the card a past event opens.
// No IPC and no clock of their own (callers hand in `now`), so each is tested on its own.
//
// What "Not now" remembers lives on this device only: the civil day it was said on, in the user's
// zone, and the meetings that were owed then. The strip comes back the next day, or at once when a
// meeting is owed that wasn't. The answers themselves are stored by the backend, never here.

import { wallTimeOf } from "../kit/lib/wallTime";
import type { CalendarEvent, EventGuests, OwedMeeting, OwedUnknownGuest, PersonRef } from "./types";

/** More unknown guests than this fold behind "Show {n} more guests". */
export const UNKNOWN_FOLD = 8;

/** The device-local key of what "Not now" hid. */
export const MEET_DISMISS_KEY = "pm.people.meetStrip";

// --- the civil day -----------------------------------------------------------------------------------

/** `YYYY-MM-DD`: today in `zone` (an IANA name), or on this device's own clock when there is no zone
 *  or it isn't one the webview knows. */
export function civilToday(zone: string | null | undefined, now: Date): string {
  if (zone) {
    try {
      return wallTimeOf(now, zone).date;
    } catch {
      // An unknown zone name: fall through to the device's own day.
    }
  }
  const p = (n: number) => String(n).padStart(2, "0");
  return `${now.getFullYear()}-${p(now.getMonth() + 1)}-${p(now.getDate())}`;
}

// --- the strip --------------------------------------------------------------------------------------

/** What a meeting is keyed on: the iCal id and the occurrence's start, as the backend keys answers. */
export const meetingKey = (m: Pick<OwedMeeting, "uid" | "start">) => `${m.uid}\n${m.start}`;

/** What "Not now" hid: the day, and every meeting owed when it was said. */
export interface Dismissal {
  on: string;
  keys: string[];
}

/** The remembered "Not now", or null (nothing stored, or something that isn't one). */
export function readDismissal(): Dismissal | null {
  try {
    const raw = localStorage.getItem(MEET_DISMISS_KEY);
    if (raw === null) return null;
    const v: unknown = JSON.parse(raw);
    if (!v || typeof v !== "object") return null;
    const { on, keys } = v as Partial<Dismissal>;
    if (typeof on !== "string" || !/^\d{4}-\d{2}-\d{2}$/.test(on)) return null;
    if (!Array.isArray(keys) || keys.some((k) => typeof k !== "string")) return null;
    return { on, keys };
  } catch {
    return null;
  }
}

export function writeDismissal(d: Dismissal): void {
  try {
    localStorage.setItem(MEET_DISMISS_KEY, JSON.stringify(d));
  } catch {
    // Best-effort: without it the strip simply comes back at the next look.
  }
}

/** "Not now" for what is owed on `today`. */
export function dismissFor(owed: readonly OwedMeeting[], today: string): Dismissal {
  return { on: today, keys: owed.map(meetingKey) };
}

/** Whether the strip shows: something is owed, and it isn't all covered by a "Not now" said today.
 *  A new meeting owed since, or the next day, brings it back. */
export function stripVisible(
  owed: readonly OwedMeeting[],
  dismissal: Dismissal | null,
  today: string,
): boolean {
  if (owed.length === 0) return false;
  if (!dismissal || dismissal.on !== today) return true;
  const hidden = new Set(dismissal.keys);
  return owed.some((m) => !hidden.has(meetingKey(m)));
}

/** The strip's question (spec 9.3): one meeting names itself. */
export function stripWords(owed: readonly OwedMeeting[]): string {
  return owed.length === 1
    ? `You had a meeting with guests: ${owed[0].title.trim() || "(no title)"}. Who did you meet?`
    : `You had ${owed.length} meetings with guests recently. Who did you meet?`;
}

// --- the card ----------------------------------------------------------------------------------------

/** A guest's line: the invite's name and address, or the address alone. */
export function guestLine(g: { name?: string; email: string }): string {
  const name = g.name?.trim() ?? "";
  return name !== "" && name !== g.email ? `${name} (${g.email})` : g.email;
}

/** The ticks a card starts with: everyone PM knows, no one it doesn't. Keys are `person:{id}` for a
 *  known guest, `guest:{email}` for an unknown one. */
export function initialTicks(m: Pick<OwedMeeting, "known">): Set<string> {
  return new Set(m.known.map((g) => personKey(g.id)));
}

export const personKey = (id: number) => `person:${id}`;
export const guestKey = (g: Pick<OwedUnknownGuest, "email">) => `guest:${g.email}`;

/** The unknown guests a card shows at first, and how many are folded behind "Show {n} more guests". */
export function foldGuests(
  unknown: readonly OwedUnknownGuest[],
  expanded: boolean,
  limit = UNKNOWN_FOLD,
): { shown: OwedUnknownGuest[]; hidden: number } {
  if (expanded || unknown.length <= limit) return { shown: [...unknown], hidden: 0 };
  return { shown: unknown.slice(0, limit), hidden: unknown.length - limit };
}

/** Who a card's ticks name, for `answer_meeting`: a known guest or someone picked is `{ id }`; an
 *  unknown guest is a new person, unless the address belongs to someone in Trash, who comes back. */
export function refsFor(
  m: Pick<OwedMeeting, "known" | "unknown">,
  ticks: ReadonlySet<string>,
  extra: readonly { id: number }[],
): PersonRef[] {
  const out: PersonRef[] = [];
  const ids = new Set<number>();
  const addId = (id: number) => {
    if (ids.has(id)) return;
    ids.add(id);
    out.push({ id });
  };
  for (const g of m.known) if (ticks.has(personKey(g.id))) addId(g.id);
  for (const g of m.unknown) {
    if (!ticks.has(guestKey(g))) continue;
    if (g.trashed_id !== null) addId(g.trashed_id);
    else out.push({ new: { name: g.name, email: g.email } });
  }
  for (const p of extra) addId(p.id);
  return out;
}

// --- a past event ------------------------------------------------------------------------------------

const pad2 = (n: number) => String(n).padStart(2, "0");
const localDay = (d: Date) => `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;

/** Whether an event is over, so "Log who you met" applies: a timed one by its end (its start when it
 *  has none), an all-day one once its last day has passed (its `end`, when it has one, is the day
 *  after). Days are this device's, as the Calendar draws them. */
export function eventIsPast(ev: Pick<CalendarEvent, "start" | "end" | "all_day">, now: Date) {
  if (ev.all_day || !ev.start.includes("T")) {
    let after = ev.end && ev.end.length >= 10 ? ev.end.slice(0, 10) : null;
    if (after === null) {
      const first = new Date(`${ev.start.slice(0, 10)}T00:00:00`);
      if (Number.isNaN(first.getTime())) return false;
      first.setDate(first.getDate() + 1);
      after = localDay(first);
    }
    return after <= localDay(now);
  }
  const ended = new Date(ev.end ?? ev.start);
  return !Number.isNaN(ended.getTime()) && ended.getTime() < now.getTime();
}

/** Whether `ev` is the occurrence an owed meeting is about. */
export function isOwedEvent(
  m: Pick<OwedMeeting, "uid" | "start">,
  ev: Pick<CalendarEvent, "uid" | "start">,
) {
  return !!ev.uid && m.uid === ev.uid && m.start === ev.start;
}

/** The card for a past event that isn't on the owed list, from the event and its sorted guests. An
 *  event with no iCal id gets an empty `uid`: it can only be logged. */
export function cardForEvent(ev: CalendarEvent, guests: EventGuests): OwedMeeting {
  return {
    uid: ev.uid ?? "",
    start: ev.start,
    end: ev.end ?? null,
    title: ev.summary.trim(),
    all_day: ev.all_day,
    recurs: !!ev.recurring || !!ev.series_id,
    known: guests.known,
    unknown: guests.unknown,
  };
}
