// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The quick box's Task side (Event | Task): the draft read as a PM task. A task has plan days and no
// time, so a timed slot gives its start day, a Month drag keeps its stretch, and the clicked day is
// the plan, never a deadline.

import { describe, expect, it } from "vitest";
import { TASK_CALENDAR_ID } from "../calendar-layout";
import { formatDateOnly } from "../format";
import type { TimeDraft } from "../types";
import { newEventSeed, type EditorFields } from "./eventDraft";
import {
  quickTaskDefaults,
  quickTaskInput,
  quickTaskNotes,
  resumeAction,
  taskAddedAnnouncement,
  taskDaysOf,
  taskGhostFields,
  taskShownDay,
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
  const today = "2026-11-12";
  it("is always one all-day day on the first plan day", () => {
    for (const time of [timed("2026-11-12", "2026-11-13"), allDay("2026-11-12", "2026-11-15")]) {
      expect(taskGhostFields(draft("x", time)).time).toEqual(allDay("2026-11-12"));
      expect(taskGhostFields(draft("x", time), today).time).toEqual(allDay("2026-11-12"));
    }
    expect(taskGhostFields(draft("x", allDay("2026-11-20")), today).time).toEqual(
      allDay("2026-11-20"),
    );
    // Without the grid's today it is the plan day, even a passed one: the announcement reads it.
    expect(taskGhostFields(draft("x", allDay("2026-11-10"))).time).toEqual(allDay("2026-11-10"));
  });
  it("sits on today when the first plan day has passed and the grid says what today is", () => {
    for (const time of [
      timed("2026-11-10"),
      allDay("2026-11-10"),
      allDay("2026-11-10", "2026-11-20"),
    ]) {
      expect(taskGhostFields(draft("x", time), today).time).toEqual(allDay(today));
    }
    expect(taskShownDay("2026-11-10", today)).toBe(today);
    expect(taskShownDay("2026-11-12", today)).toBe(today);
    expect(taskShownDay("2026-11-13", today)).toBe("2026-11-13");
  });
  it("leaves a draft with no readable day alone, so no ghost is drawn", () => {
    const d = draft("x", allDay("nope"));
    expect(taskGhostFields(d, today)).toBe(d);
  });
});

describe("quickTaskNotes", () => {
  const today = "2026-11-12";
  const todayWords = formatDateOnly(today);
  const one = (doStart: string) => ({ doStart, doEnd: null });
  it("says when Tasks is hidden in the calendar list", () => {
    expect(quickTaskNotes(new Set([TASK_CALENDAR_ID]), one(today), today)).toEqual([
      "Tasks are hidden in your calendar list, so the new task won't show until you show Tasks again.",
    ]);
  });
  it("says only that Tasks is hidden for a passed day: nothing draws on today then", () => {
    expect(quickTaskNotes(new Set([TASK_CALENDAR_ID]), one("2026-11-10"), today)).toEqual([
      "Tasks are hidden in your calendar list, so the new task won't show until you show Tasks again.",
    ]);
  });
  it("says a day that has passed shows on today, with today's date", () => {
    expect(quickTaskNotes(new Set(), one("2026-11-10"), today)).toEqual([
      `That day has passed, so the task shows on today, ${todayWords}, until it's done.`,
    ]);
  });
  it("says only the first day has passed for a stretch that ends today or later", () => {
    for (const doEnd of [today, "2026-11-20"]) {
      expect(quickTaskNotes(new Set(), { doStart: "2026-11-10", doEnd }, today)).toEqual([
        `The first day has passed, so the task shows on today, ${todayWords}, until it's done.`,
      ]);
    }
  });
  it("says the days have passed for a stretch that ended before today", () => {
    expect(
      quickTaskNotes(new Set(), { doStart: "2026-11-08", doEnd: "2026-11-10" }, today),
    ).toEqual([
      `Those days have passed, so the task shows on today, ${todayWords}, until it's done.`,
    ]);
  });
  it("says nothing otherwise", () => {
    expect(quickTaskNotes(new Set(), one(today), today)).toEqual([]);
    expect(quickTaskNotes(new Set(), one("2026-11-20"), today)).toEqual([]);
    expect(quickTaskNotes(new Set(["gcal:x"]), null, today)).toEqual([]);
  });
});

describe("taskAddedAnnouncement", () => {
  const today = "2026-11-12";
  it("names the title and the planned day", () => {
    expect(
      taskAddedAnnouncement("Pay rent", { doStart: "2026-11-20", doEnd: null }, false, today),
    ).toBe(`Task added: Pay rent, ${formatDateOnly("2026-11-20")}.`);
    expect(taskAddedAnnouncement("Pay rent", null, false, today)).toBe("Task added: Pay rent.");
  });
  it("says a passed day shows on today, with today's date, until it's done", () => {
    for (const doEnd of [null, "2026-11-20"]) {
      expect(
        taskAddedAnnouncement("Pay rent", { doStart: "2026-11-10", doEnd }, false, today),
      ).toBe(
        `Task added: Pay rent, ${formatDateOnly("2026-11-10")}. It shows on today, ${formatDateOnly(today)}, until it's done.`,
      );
    }
  });
  it("drops the on-today claim when Tasks is hidden, and says it is hidden", () => {
    const text = taskAddedAnnouncement(
      "Pay rent",
      { doStart: "2026-11-10", doEnd: null },
      true,
      today,
    );
    expect(text).toBe(
      `Task added: Pay rent, ${formatDateOnly("2026-11-10")}. Tasks are hidden in your calendar list.`,
    );
    expect(text).not.toContain("shows on today");
  });
});

describe("resumeAction", () => {
  it("opens the box again whenever it can", () => {
    for (const kind of [undefined, "event", "task"] as const) {
      expect(resumeAction(kind, true)).toBe("box");
    }
  });
  it("reopens a task draft in the task editor when the box can't open: it needs no calendar", () => {
    expect(resumeAction("task", false)).toBe("task-editor");
  });
  it("keeps an event draft (absent kind is an event) until a calendar can take it", () => {
    expect(resumeAction("event", false)).toBe("keep");
    expect(resumeAction(undefined, false)).toBe("keep");
  });
});
