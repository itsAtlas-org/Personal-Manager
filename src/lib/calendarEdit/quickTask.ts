// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Task side of quick create's Event | Task switch: the one draft the box holds, read as a PM task.
// Pure, so what the box saves, hands to the task editor and draws as the ghost is decided here and not
// in the component. A PM task has a plan day (or a stretch of days) and no time of day, so a time
// clicked in Week is dropped, and the day clicked is the task's PLAN day (the Runway's + Task seeds
// the same field), never its deadline.

import { TASK_CALENDAR_ID } from "../calendar-layout";
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

/** The ghost of the new task: one all-day day on its first plan day, as the tasks overlay draws one
 *  chip on one day and never a bar (`taskOverlayEvents`). A draft with no readable day is left as it
 *  is, so no ghost is drawn for it. */
export function taskGhostFields(draft: EditorFields): EditorFields {
  const days = taskDaysOf(draft.time);
  if (!days) return draft;
  return { ...draft, time: { kind: "all_day", first_day: days.doStart, last_day: days.doStart } };
}

/** What the box says before Save that would otherwise make the new task look lost (the event side's
 *  `createNotes`): Tasks is hidden in the calendar list, or the day has passed, when the overlay draws
 *  the task on today until it's done. `doStart` and `todayKey` are `YYYY-MM-DD`. */
export function quickTaskNotes(
  hidden: ReadonlySet<string>,
  doStart: string | null,
  todayKey: string,
): string[] {
  const notes: string[] = [];
  if (hidden.has(TASK_CALENDAR_ID)) {
    notes.push(
      "Tasks are hidden in your calendar list, so the new task won't show until you show Tasks again.",
    );
  }
  if (doStart !== null && doStart < todayKey) {
    notes.push("That day has passed, so the task shows on today until it's done.");
  }
  return notes;
}
