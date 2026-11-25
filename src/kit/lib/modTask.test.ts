// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { REDACTED_EVENT_UID, REDACTED_MILESTONE_ID, fromModTask, type ModTaskDto } from "./modTask";
import { deadlineSource, taskToInput } from "./tasks";

/** A task as `tasks.list` answers a mod that holds neither `projects.read` nor `calendar.read`. */
const bare: ModTaskDto = {
  id: 7,
  title: "Send the deck",
  why: null,
  do_start: null,
  do_end: null,
  deadline: "2026-12-01",
  deadline_kind: "hard",
  deadline_from: "date",
  effective_deadline: "2026-12-01",
  deadline_missing: false,
  status: "todo",
  completed_at: null,
  lane: "foreground",
  stack_id: null,
  parked: false,
  ordinal: 3,
  source: "user",
  created_at: "2026-11-01T09:00:00.000Z",
  updated_at: "2026-11-01T09:00:00.000Z",
  milestone_linked: false,
  event_linked: false,
};

describe("a task read through the Host API", () => {
  it("a task linked to an event, read without calendar.read, says Event's date and never removed", () => {
    const task = fromModTask({ ...bare, deadline_from: "event", event_linked: true });
    expect(task.event_uid).toBe(REDACTED_EVENT_UID);
    expect(task.event_title).toBeNull();
    const words = deadlineSource(task);
    expect(words).toBe("Event's date");
    expect(words).not.toMatch(/removed/);
    // The editor's input keeps the event as the deadline's source: nothing resets it to "My own date".
    expect(taskToInput(task).deadlineFrom).toBe("event");
  });

  it("a task linked to a milestone, read without projects.read, says Milestone's date and never removed", () => {
    const task = fromModTask({ ...bare, deadline_from: "milestone", milestone_linked: true });
    expect(task.milestone_id).toBe(REDACTED_MILESTONE_ID);
    expect(deadlineSource(task)).toBe("Milestone's date");
    expect(taskToInput(task).deadlineFrom).toBe("milestone");
  });

  it("a task whose link is really gone still says removed", () => {
    const event = fromModTask({ ...bare, deadline_from: "event", event_linked: false });
    expect(event.event_uid).toBeNull();
    expect(deadlineSource(event)).toMatch(/linked event was removed/);
    const milestone = fromModTask({
      ...bare,
      deadline_from: "milestone",
      milestone_linked: false,
    });
    expect(deadlineSource(milestone)).toMatch(/linked milestone was removed/);
    expect(taskToInput(event).deadlineFrom).toBe("date");
  });

  it("a link the mod may look through keeps its real values, titles included", () => {
    const task = fromModTask({
      ...bare,
      deadline_from: "event",
      event_linked: true,
      event_uid: "uid-1@example.com",
      event_title: "Sprint review",
      milestone_linked: true,
      milestone_id: 12,
      milestone_label: "Launch",
      project_name: "Atlas",
      project_archived: true,
    });
    expect(deadlineSource(task)).toBe("Event's date: Sprint review");
    expect(task).toMatchObject({
      event_uid: "uid-1@example.com",
      milestone_id: 12,
      milestone_label: "Launch",
      project_name: "Atlas",
      project_archived: true,
    });
  });

  it("drops the flags, fills the rest with nothing, and keeps every other field as it came", () => {
    const task = fromModTask(bare);
    expect(task).not.toHaveProperty("milestone_linked");
    expect(task).not.toHaveProperty("event_linked");
    const fields = Object.fromEntries(Object.entries(bare).filter(([k]) => !k.endsWith("_linked")));
    expect(task).toEqual({
      ...fields,
      project_name: null,
      project_archived: false,
      milestone_id: null,
      milestone_label: null,
      event_uid: null,
      event_title: null,
      // PM's own decoration of what a mod made, never in a mod's answer.
      madeBy: null,
    });
  });
});
