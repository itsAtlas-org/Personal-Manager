// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The calendar's view filters (Hide done, All | All-day): what each keeps and drops, with the cases
// the ask turns on: an event that ended this morning against one running now, an overdue "Due:"
// chip, a late milestone, a timed event a milestone links to, and a timed event crossing midnight.

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  DRAFT_EVENT_ID,
  inTimeGridBand,
  MILESTONE_CALENDAR_ID,
  milestoneOverlayId,
  PINBOARD_CALENDAR_ID,
  TASK_CALENDAR_ID,
} from "./calendar-layout";
import { hideFinished, keepAllDay, metMilestoneIds, milestoneEventUids } from "./calendarFilters";
import type { CalendarEvent, Milestone } from "./types";

afterEach(() => vi.unstubAllEnvs());

// Wednesday 11 November 2026, 10:00 on this device's clock.
const NOW = new Date(2026, 10, 11, 10, 0);

function ev(id: string, start: string, over: Partial<CalendarEvent> = {}): CalendarEvent {
  return {
    id,
    calendar_id: "work",
    summary: id,
    description: null,
    location: null,
    start,
    end: null,
    all_day: !start.includes("T"),
    html_link: null,
    uid: id,
    ...over,
  };
}

const timed = (id: string, start: string, end: string, over: Partial<CalendarEvent> = {}) =>
  ev(id, `2026-${start}:00`, { end: `2026-${end}:00`, ...over });

function milestone(over: Partial<Milestone> = {}): Milestone {
  return {
    id: 1,
    project_name: "Atlas",
    label: "Pitch",
    due_date: "2026-11-12",
    event_uid: null,
    calendar_linked: false,
    event_missing: false,
    state: null,
    status: null,
    source_type: null,
    external_id: null,
    sort_order: 0,
    ...over,
  };
}

const overlay = (calendar_id: string, id: string, start: string, summary = id): CalendarEvent =>
  ev(id, start, { calendar_id, summary, uid: null, end: null });

const ids = (events: readonly CalendarEvent[]) => events.map((e) => e.id);

describe("hideFinished", () => {
  const input = { now: NOW, metIds: new Set<string>(), fromDay: new Date(2026, 10, 1) };

  it("hides an event that ended this morning and one from last week, keeps one running now", () => {
    const events = [
      timed("breakfast", "11-11T08:00", "11-11T09:00"),
      timed("last week", "11-04T14:00", "11-04T15:00"),
      timed("running", "11-11T09:30", "11-11T11:00"),
      timed("dinner", "11-11T19:00", "11-11T21:00"),
      ev("tomorrow", "2026-11-12T09:00:00"),
    ];
    const { shown, hidden } = hideFinished(events, input);
    expect(ids(shown)).toEqual(["running", "dinner", "tomorrow"]);
    expect(hidden).toBe(2);
  });

  it("keeps today's all-day event, hides an all-day one from yesterday", () => {
    const { shown } = hideFinished(
      [
        ev("today", "2026-11-11", { end: "2026-11-12" }),
        ev("yesterday", "2026-11-10", { end: "2026-11-11" }),
        ev("trip", "2026-11-09", { end: "2026-11-13" }),
      ],
      input,
    );
    expect(ids(shown)).toEqual(["today", "trip"]);
  });

  it("keeps a declined event that hasn't ended, and the draft ghost", () => {
    // Struck through, as `markDeclined` makes it: no different from any other event to this filter.
    const declined = {
      ...timed("declined", "11-12T09:00", "11-12T10:00"),
      declined: true,
    } as CalendarEvent;
    // Even a ghost whose time has passed is the user's new event: never hidden.
    const draft = timed("ghost", "11-10T09:00", "11-10T10:00", { id: DRAFT_EVENT_ID });
    const { shown } = hideFinished([declined, draft], input);
    expect(ids(shown)).toEqual(["declined", DRAFT_EVENT_ID]);
  });

  it("keeps an overdue 'Due:' task chip on a past day and a late unmet milestone", () => {
    const due = overlay(TASK_CALENDAR_ID, "task:1", "2026-11-03", "Due: Send the deck");
    const late = overlay(
      MILESTONE_CALENDAR_ID,
      milestoneOverlayId(1),
      "2026-11-02",
      "Launch · Atlas",
    );
    const { shown, hidden } = hideFinished([due, late], {
      ...input,
      metIds: metMilestoneIds([milestone({ id: 1, due_date: "2026-11-02" })]),
    });
    expect(ids(shown)).toEqual(["task:1", "milestone:1"]);
    expect(hidden).toBe(0);
  });

  it("hides a met milestone's overlay on any day, today and the future included", () => {
    const met = metMilestoneIds([
      milestone({ id: 1, state: "met" }),
      milestone({ id: 2, status: "done" }),
      milestone({ id: 3 }),
    ]);
    const { shown, hidden } = hideFinished(
      [
        overlay(MILESTONE_CALENDAR_ID, "milestone:1", "2026-11-02"),
        overlay(MILESTONE_CALENDAR_ID, "milestone:2", "2026-11-20"),
        overlay(MILESTONE_CALENDAR_ID, "milestone:3", "2026-11-20"),
      ],
      { ...input, metIds: met },
    );
    expect(ids(shown)).toEqual(["milestone:3"]);
    expect(hidden).toBe(2);
  });

  it("hides a pinboard entry once its day has passed", () => {
    const { shown } = hideFinished(
      [
        overlay(PINBOARD_CALENDAR_ID, "pinboard:1", "2026-11-03"),
        overlay(PINBOARD_CALENDAR_ID, "pinboard:2", "2026-11-11"),
      ],
      input,
    );
    expect(ids(shown)).toEqual(["pinboard:2"]);
  });

  it("counts as hidden only the rows ending on or after the first day drawn", () => {
    const events = [
      timed("last month", "10-20T09:00", "10-20T10:00"),
      timed("this month", "11-04T09:00", "11-04T10:00"),
      timed("this morning", "11-11T08:00", "11-11T09:00"),
    ];
    // The List opened on the 1st: the October one was never going to be drawn.
    expect(hideFinished(events, input).hidden).toBe(2);
    // The Runway's strips start at today.
    expect(hideFinished(events, { ...input, fromDay: NOW }).hidden).toBe(1);
    // Hidden or not, the rows are gone from `shown`.
    expect(hideFinished(events, input).shown).toEqual([]);
  });

  it("reads an event ending at 00:00 as ending the day before, in the zone it's in", () => {
    vi.stubEnv("TZ", "Europe/London");
    // 23:00-00:00 on the 10th is over by Wednesday morning; the row's last day is the 10th.
    const late = timed("late", "11-10T23:00", "11-11T00:00");
    const { shown, hidden } = hideFinished([late], { ...input, fromDay: new Date(2026, 10, 10) });
    expect(shown).toEqual([]);
    expect(hidden).toBe(1);
    // …and with the first day drawn being the 11th, it was never on the list.
    expect(hideFinished([late], { ...input, fromDay: new Date(2026, 10, 11) }).hidden).toBe(0);
  });
});

describe("keepAllDay", () => {
  const none = new Set<string>();

  it("keeps single- and multi-day all-day events, the overlays and the draft", () => {
    const events = [
      ev("holiday", "2026-11-11", { end: "2026-11-12" }),
      ev("trip", "2026-11-11", { end: "2026-11-15" }),
      overlay(TASK_CALENDAR_ID, "task:1", "2026-11-11"),
      overlay(MILESTONE_CALENDAR_ID, "milestone:1", "2026-11-12"),
      overlay(PINBOARD_CALENDAR_ID, "pinboard:1:2", "2026-11-12"),
      timed("ghost", "11-11T09:00", "11-11T10:00", { id: DRAFT_EVENT_ID, uid: null }),
    ];
    expect(keepAllDay(events, none)).toBe(events);
  });

  it("drops a timed event crossing midnight and a three-day timed event", () => {
    const events = [
      timed("standup", "11-11T09:00", "11-11T09:15"),
      timed("overnight", "11-11T22:00", "11-12T02:00"),
      timed("conference", "11-11T09:00", "11-13T17:00"),
      ev("holiday", "2026-11-11"),
    ];
    expect(ids(keepAllDay(events, none))).toEqual(["holiday"]);
  });

  it("keeps a timed event a milestone links to, since the milestone has no overlay of its own", () => {
    const linked = timed("review", "11-11T14:00", "11-11T15:00", { uid: "review-uid" });
    const other = timed("lunch", "11-11T12:00", "11-11T13:00", { uid: "lunch-uid" });
    const uids = milestoneEventUids([
      milestone({ id: 1, event_uid: "review-uid" }),
      // Its event is missing from the mirror, so it has an overlay and there is nothing to keep.
      milestone({ id: 2, event_uid: "lunch-uid", event_missing: true }),
      milestone({ id: 3, event_uid: null }),
    ]);
    expect([...uids]).toEqual(["review-uid"]);
    expect(ids(keepAllDay([linked, other], uids))).toEqual(["review"]);
  });

  it("is tied to the time grid's own all-day band, so the two can't disagree", () => {
    const events = [
      ev("a", "2026-11-11"),
      timed("b", "11-11T09:00", "11-11T10:00"),
      timed("c", "11-11T09:00", "11-14T10:00"),
    ];
    expect(ids(keepAllDay(events, none))).toEqual(ids(events.filter(inTimeGridBand)));
  });
});
