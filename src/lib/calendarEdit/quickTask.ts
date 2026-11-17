// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Task side of quick create's Event | Task switch: the one draft the box holds, read as a PM task.
// Pure, so what the box saves, hands to the task editor and draws as the ghost is decided here and not
// in the component. A PM task has a plan day (or a stretch of days) and no time of day, so a time
// clicked in Week is dropped, and the day clicked is the task's PLAN day (the Runway's + Task seeds
// the same field), never its deadline.

import { TASK_CALENDAR_ID } from "../calendar-layout";
import { formatDateOnly } from "../format";
import { emptyTaskInput } from "../tasks";
import type { TaskInput, TimeDraft } from "../types";
import type { EditorFields } from "./eventDraft";

const ISO_DAY = /^\d{4}-\d{2}-\d{2}$/;

/** The plan days a draft's time stands for: an all-day span keeps its stretch (a Month drag), a
 *  timed slot is its start day alone (one that crosses midnight too). Null for a day the field
 *  couldn't read. */
export function taskDaysOf(time: TimeDraft): { doStart: string; doEnd: string | null } | null {
  if (time.kind === "timed") {
    return ISO_DAY.test(time.start_date) ? { doStart: time.start_date, doEnd: null } : null;
  }
  if (!ISO_DAY.test(time.first_day) || !ISO_DAY.test(time.last_day)) return null;
  return {
    doStart: time.first_day,
    doEnd: time.last_day > time.first_day ? time.last_day : null,
  };
}

/** What the task editor opens on from the box's More options: the title and the plan days. */
export function quickTaskDefaults(draft: EditorFields): Partial<TaskInput> {
  return { title: draft.summary.trim(), ...(taskDaysOf(draft.time) ?? {}) };
}

/** The task Save makes: a blank task (foreground, no deadline) with the title and the plan days. The
 *  backend sets its source. */
export function quickTaskInput(draft: EditorFields): TaskInput {
  return emptyTaskInput(quickTaskDefaults(draft));
}

/** The day a new task's chip draws on: its first plan day, or today once that has passed, as the
 *  tasks overlay moves unfinished work on (`taskOverlayEvents`: it reads the first day alone, so a
 *  stretch that is still running sits on today too). Both are `YYYY-MM-DD`. */
export function taskShownDay(doStart: string, todayKey: string): string {
  return doStart < todayKey ? todayKey : doStart;
}

/** The ghost of the new task: one all-day day, as the tasks overlay draws one chip on one day and
 *  never a bar (`taskOverlayEvents`). It is on the first plan day, or, given `todayKey` (the grid
 *  drawing it), on the day the chip will land (`taskShownDay`). A draft with no readable day is left
 *  as it is, so no ghost is drawn for it. */
export function taskGhostFields(draft: EditorFields, todayKey?: string): EditorFields {
  const days = taskDaysOf(draft.time);
  if (!days) return draft;
  const day = todayKey === undefined ? days.doStart : taskShownDay(days.doStart, todayKey);
  return { ...draft, time: { kind: "all_day", first_day: day, last_day: day } };
}

/** Why a task planned on days that have started to pass shows on today, in words, or null when its
 *  first day has not passed. A stretch whose end is today or later is still running: only its first day has
 *  passed. */
function passedWords(
  days: { doStart: string; doEnd: string | null },
  todayKey: string,
): string | null {
  if (days.doStart >= todayKey) return null;
  const shows = `the task shows on today, ${formatDateOnly(todayKey)}, until it's done.`;
  if (days.doEnd === null) return `That day has passed, so ${shows}`;
  if (days.doEnd >= todayKey) return `The first day has passed, so ${shows}`;
  return `Those days have passed, so ${shows}`;
}

/** What the box says before Save that would otherwise make the new task look lost (the event side's
 *  `createNotes`): Tasks is hidden in the calendar list, or the plan's first day has passed, when the
 *  overlay draws the task on today until it's done. With Tasks hidden nothing draws, so the passed-day
 *  line is left out. `todayKey` is `YYYY-MM-DD`. */
export function quickTaskNotes(
  hidden: ReadonlySet<string>,
  days: { doStart: string; doEnd: string | null } | null,
  todayKey: string,
): string[] {
  if (hidden.has(TASK_CALENDAR_ID)) {
    return [
      "Tasks are hidden in your calendar list, so the new task won't show until you show Tasks again.",
    ];
  }
  const passed = days ? passedWords(days, todayKey) : null;
  return passed ? [passed] : [];
}

/** The polite announcement after Save on Task: what was made and on which day, where its chip is when
 *  that is not the day it was planned for (today, with its date, as the week on screen may not hold
 *  today), and that Tasks is hidden when it is (then nothing draws,
 *  so "shows on today" is left out). */
export function taskAddedAnnouncement(
  title: string,
  days: { doStart: string; doEnd: string | null } | null,
  hiddenTasks: boolean,
  todayKey: string,
): string {
  const head = `Task added: ${title}${days ? `, ${formatDateOnly(days.doStart)}` : ""}.`;
  if (hiddenTasks) return `${head} Tasks are hidden in your calendar list.`;
  return days && days.doStart < todayKey
    ? `${head} It shows on today, ${formatDateOnly(todayKey)}, until it's done.`
    : head;
}

/** What coming back to the Calendar tab does with a titled quick-create draft that went with it:
 *  "box" opens the box again (a calendar can take an event and "+ New event" is there to open
 *  under); "task-editor" reopens a task draft in the task editor, as More options does, because a
 *  task needs neither a calendar nor that button; "keep" leaves an event draft where it is until a
 *  calendar can take events again, as it has nowhere to go and a titled draft is never lost. */
export type ResumeAction = "box" | "task-editor" | "keep";

export function resumeAction(
  kind: "event" | "task" | undefined,
  canOpenBox: boolean,
): ResumeAction {
  if (canOpenBox) return "box";
  return kind === "task" ? "task-editor" : "keep";
}
