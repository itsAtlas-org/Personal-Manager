// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The quick box's Task side (Event | Task): the draft read as a PM task. A task has plan days and no
// time, so a timed slot gives its start day, a Month drag keeps its stretch, and the clicked day is
// the plan, never a deadline.

import { describe, expect, it } from "vitest";
import { TASK_CALENDAR_ID } from "../calendar-layout";
import type { TimeDraft } from "../types";
import { newEventSeed, type EditorFields } from "./eventDraft";
import {
  quickTaskDefaults,
  quickTaskInput,
  quickTaskNotes,
  taskDaysOf,
  taskGhostFields,
} from "./quickTask";

const allDay = (first_day: string, last_day = first_day): TimeDraft => ({
  kind: "all_day",
  first_day,
  last_day,
});
const timed = (start_date: string, end_date = start_date): TimeDraft => ({
  kind: "timed",
  start_date,
  start_time: "09:00",
  start_zone: "Europe/London",
  end_date,
  end_time: "10:00",
  end_zone: "Europe/London",
});
const draft = (summary: string, time: TimeDraft): EditorFields => ({
  ...newEventSeed("2026-11-12", new Date(2026, 10, 12, 8, 0), "Europe/London", 3_600_000),
  summary,
  time,
});

describe("taskDaysOf", () => {
  it("an all-day day is one plan day", () => {
    expect(taskDaysOf(allDay("2026-11-12"))).toEqual({ doStart: "2026-11-12", doEnd: null });
  });
  it("an all-day range keeps its stretch", () => {
    expect(taskDaysOf(allDay("2026-11-12", "2026-11-15"))).toEqual({
      doStart: "2026-11-12",
      doEnd: "2026-11-15",
    });
  });
  it("a timed slot is its start day, with no time", () => {
    expect(taskDaysOf(timed("2026-11-12"))).toEqual({ doStart: "2026-11-12", doEnd: null });
  });
  it("a timed slot crossing midnight is still its start day", () => {
    expect(taskDaysOf(timed("2026-11-12", "2026-11-13"))).toEqual({
      doStart: "2026-11-12",
      doEnd: null,
    });
  });
  it("a day the field couldn't read is null", () => {
    expect(taskDaysOf(allDay("12-11-2026"))).toBeNull();
    expect(taskDaysOf(timed(""))).toBeNull();
  });
});

describe("quickTaskInput", () => {
  it("trims the title and takes the plan days from the draft", () => {
    expect(quickTaskInput(draft("  Pay rent  ", allDay("2026-11-12", "2026-11-14")))).toMatchObject(
      { title: "Pay rent", doStart: "2026-11-12", doEnd: "2026-11-14" },
    );
  });
  it("is a plain foreground task with no deadline", () => {
    expect(quickTaskInput(draft("Pay rent", timed("2026-11-12")))).toEqual({
      title: "Pay rent",
      why: null,
      project: null,
      milestoneId: null,
      eventUid: null,
      doStart: "2026-11-12",
      doEnd: null,
      deadline: null,
      deadlineKind: "hard",
      deadlineFrom: "date",
      lane: "foreground",
      stackId: null,
      parked: false,
    });
  });
  it("the task editor opens on just the title and days", () => {
    expect(quickTaskDefaults(draft(" Call ", allDay("2026-11-12")))).toEqual({
      title: "Call",
      doStart: "2026-11-12",
      doEnd: null,
    });
  });
});

describe("taskGhostFields", () => {
  it("is always one all-day day on the first plan day", () => {
    for (const time of [timed("2026-11-12", "2026-11-13"), allDay("2026-11-12", "2026-11-15")]) {
      expect(taskGhostFields(draft("x", time)).time).toEqual(allDay("2026-11-12"));
    }
  });
  it("leaves a draft with no readable day alone, so no ghost is drawn", () => {
    const d = draft("x", allDay("nope"));
    expect(taskGhostFields(d)).toBe(d);
  });
});

describe("quickTaskNotes", () => {
  const today = "2026-11-12";
  it("says when Tasks is hidden in the calendar list", () => {
    expect(quickTaskNotes(new Set([TASK_CALENDAR_ID]), today, today)).toEqual([
      "Tasks are hidden in your calendar list, so the new task won't show until you show Tasks again.",
    ]);
  });
  it("says a day that has passed shows on today", () => {
    expect(quickTaskNotes(new Set(), "2026-11-10", today)).toEqual([
      "That day has passed, so the task shows on today until it's done.",
    ]);
  });
  it("says nothing otherwise", () => {
    expect(quickTaskNotes(new Set(), today, today)).toEqual([]);
    expect(quickTaskNotes(new Set(), "2026-11-20", today)).toEqual([]);
    expect(quickTaskNotes(new Set(["gcal:x"]), null, today)).toEqual([]);
  });
});
