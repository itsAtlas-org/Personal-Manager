// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Who did you meet?" without a screen: the strip's words and when it stays quiet, the card's ticks
// and its fold, the references a Save sends, and the card a past event opens. The clock is always
// handed in. Invented people only.

import { afterEach, describe, expect, it } from "vitest";
import {
  cardForEvent,
  civilToday,
  dismissFor,
  eventIsPast,
  foldGuests,
  guestKey,
  guestLine,
  initialTicks,
  isOwedEvent,
  MEET_DISMISS_KEY,
  meetingKey,
  personKey,
  readDismissal,
  refsFor,
  stripVisible,
  stripWords,
  UNKNOWN_FOLD,
  writeDismissal,
} from "./meetings";
import type { CalendarEvent, OwedMeeting, OwedUnknownGuest } from "./types";

const meeting = (over: Partial<OwedMeeting> = {}): OwedMeeting => ({
  uid: "evt-1@example.com",
  start: "2026-11-26T09:00:00Z",
  end: "2026-11-26T09:30:00Z",
  title: "Standup",
  all_day: false,
  recurs: false,
  known: [],
  unknown: [],
  ...over,
});

const guest = (n: number, over: Partial<OwedUnknownGuest> = {}): OwedUnknownGuest => ({
  name: `Guest ${n}`,
  email: `guest${n}@example.com`,
  trashed_id: null,
  ...over,
});

afterEach(() => localStorage.clear());

describe("civilToday", () => {
  it("is the day in the user's zone, not the device's", () => {
    // 23:30 UTC on the 26th is already the 27th in Auckland and still the 26th in Honolulu.
    const at = new Date("2026-11-26T23:30:00Z");
    expect(civilToday("Pacific/Auckland", at)).toBe("2026-11-27");
    expect(civilToday("Pacific/Honolulu", at)).toBe("2026-11-26");
  });

  it("falls back to the device's own day for no zone or one the webview doesn't know", () => {
    const at = new Date(2026, 10, 26, 12, 0, 0);
    expect(civilToday(null, at)).toBe("2026-11-26");
    expect(civilToday("Not/AZone", at)).toBe("2026-11-26");
  });
});

describe("the strip", () => {
  it("names a lone meeting and counts several", () => {
    expect(stripWords([meeting()])).toBe(
      "You had a meeting with guests: Standup. Who did you meet?",
    );
    expect(stripWords([meeting(), meeting({ uid: "b" }), meeting({ uid: "c" })])).toBe(
      "You had 3 meetings with guests recently. Who did you meet?",
    );
  });

  it("shows while anything is owed and nothing was put off", () => {
    expect(stripVisible([], null, "2026-11-26")).toBe(false);
    expect(stripVisible([meeting()], null, "2026-11-26")).toBe(true);
  });

  it("stays quiet after Not now until tomorrow", () => {
    const owed = [meeting()];
    const said = dismissFor(owed, "2026-11-26");
    expect(stripVisible(owed, said, "2026-11-26")).toBe(false);
    expect(stripVisible(owed, said, "2026-11-27")).toBe(true);
  });

  it("comes back at once when a meeting is owed that wasn't when Not now was said", () => {
    const said = dismissFor([meeting()], "2026-11-26");
    const now = [meeting(), meeting({ uid: "evt-2@example.com" })];
    expect(stripVisible(now, said, "2026-11-26")).toBe(true);
    // An answered meeting leaving the list is not a new one.
    expect(stripVisible([], said, "2026-11-26")).toBe(false);
  });

  it("keys a meeting on its iCal id and its occurrence, so a series' next one is new", () => {
    expect(meetingKey(meeting())).not.toBe(meetingKey(meeting({ start: "2026-11-27T09:00:00Z" })));
    const said = dismissFor([meeting()], "2026-11-26");
    expect(stripVisible([meeting({ start: "2026-11-27T09:00:00Z" })], said, "2026-11-26")).toBe(
      true,
    );
  });

  it("remembers what Not now hid on this device and reads nothing it doesn't recognise", () => {
    expect(readDismissal()).toBeNull();
    writeDismissal({ on: "2026-11-26", keys: ["a"] });
    expect(readDismissal()).toEqual({ on: "2026-11-26", keys: ["a"] });
    for (const bad of ["not json", "[]", '{"on":5,"keys":[]}', '{"on":"2026-11-26","keys":[1]}']) {
      localStorage.setItem(MEET_DISMISS_KEY, bad);
      expect(readDismissal(), bad).toBeNull();
    }
  });
});

describe("the card's ticks", () => {
  it("pre-ticks the guests PM knows and nobody else", () => {
    const m = meeting({
      known: [{ id: 3, display_name: "Ann Able", email: "ann@example.com" }],
      unknown: [guest(1)],
    });
    expect([...initialTicks(m)]).toEqual([personKey(3)]);
  });

  it("folds more than eight unknown guests behind a count, and unfolds them", () => {
    const nine = Array.from({ length: UNKNOWN_FOLD + 1 }, (_, i) => guest(i));
    const folded = foldGuests(nine, false);
    expect(folded.shown).toHaveLength(UNKNOWN_FOLD);
    expect(folded.hidden).toBe(1);
    expect(foldGuests(nine, true)).toEqual({ shown: nine, hidden: 0 });
    // Exactly eight is not folded.
    expect(foldGuests(nine.slice(0, UNKNOWN_FOLD), false).hidden).toBe(0);
  });

  it("writes a guest's name and address, or the address alone", () => {
    expect(guestLine({ name: "Newt Nobody", email: "newt@example.com" })).toBe(
      "Newt Nobody (newt@example.com)",
    );
    expect(guestLine({ name: "", email: "newt@example.com" })).toBe("newt@example.com");
    expect(guestLine({ name: "newt@example.com", email: "newt@example.com" })).toBe(
      "newt@example.com",
    );
  });
});

describe("who a Save names", () => {
  const m = meeting({
    known: [{ id: 3, display_name: "Ann Able", email: "ann@example.com" }],
    unknown: [guest(1), guest(2, { trashed_id: 8 })],
  });

  it("sends a known guest and a picked person by id, a new guest as new", () => {
    const ticks = new Set([personKey(3), guestKey(guest(1))]);
    expect(refsFor(m, ticks, [{ id: 5 }])).toEqual([
      { id: 3 },
      { new: { name: "Guest 1", email: "guest1@example.com" } },
      { id: 5 },
    ]);
  });

  it("sends a guest in Trash by their id, so ticking brings them back", () => {
    expect(refsFor(m, new Set([guestKey(guest(2))]), [])).toEqual([{ id: 8 }]);
  });

  it("names nobody who is unticked, and nobody twice", () => {
    expect(refsFor(m, new Set(), [])).toEqual([]);
    expect(refsFor(m, new Set([personKey(3)]), [{ id: 3 }, { id: 3 }])).toEqual([{ id: 3 }]);
  });
});

describe("a past event", () => {
  const timed = (over: Partial<CalendarEvent> = {}) =>
    ({
      id: "e1",
      calendar_id: "cal",
      summary: "  Coffee  ",
      description: null,
      location: null,
      start: "2026-11-26T09:00:00Z",
      end: "2026-11-26T10:00:00Z",
      all_day: false,
      html_link: null,
      uid: "evt-1@example.com",
      ...over,
    }) as CalendarEvent;

  it("is over once a timed event has ended, by its end and by its start when it has none", () => {
    const now = new Date("2026-11-26T10:00:01Z");
    expect(eventIsPast(timed(), now)).toBe(true);
    expect(eventIsPast(timed({ end: "2026-11-26T11:00:00Z" }), now)).toBe(false);
    expect(eventIsPast(timed({ end: null }), now)).toBe(true);
    expect(eventIsPast(timed({ start: "2026-11-27T09:00:00Z", end: null }), now)).toBe(false);
  });

  it("is over for an all-day event only after its last day", () => {
    const day = (start: string, end: string | null) =>
      timed({ all_day: true, start, end }) as Pick<CalendarEvent, "start" | "end" | "all_day">;
    // `end` is the day after the last day: a one-day event on the 25th ends on the 26th.
    const noon = new Date(2026, 10, 26, 12, 0, 0);
    expect(eventIsPast(day("2026-11-25", "2026-11-26"), noon)).toBe(true);
    expect(eventIsPast(day("2026-11-26", "2026-11-27"), noon)).toBe(false);
    expect(eventIsPast(day("2026-11-25", null), noon)).toBe(true);
    expect(eventIsPast(day("2026-11-26", null), noon)).toBe(false);
  });

  it("is the owed meeting with the same iCal id and occurrence, and never an event with no id", () => {
    expect(isOwedEvent(meeting(), timed())).toBe(true);
    expect(isOwedEvent(meeting(), timed({ start: "2026-11-27T09:00:00Z" }))).toBe(false);
    expect(isOwedEvent(meeting({ uid: "" }), timed({ uid: null }))).toBe(false);
  });

  it("makes a card from the event and its sorted guests, empty for a lone event", () => {
    const card = cardForEvent(timed({ recurring: true }), { known: [], unknown: [guest(1)] });
    expect(card).toMatchObject({
      uid: "evt-1@example.com",
      start: "2026-11-26T09:00:00Z",
      title: "Coffee",
      recurs: true,
      all_day: false,
    });
    expect(card.unknown).toHaveLength(1);
    const lone = cardForEvent(timed({ uid: null }), { known: [], unknown: [] });
    expect(lone.uid).toBe("");
    expect(lone.recurs).toBe(false);
  });
});
