// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A task as `tasks.list` answers a mod, read back into the `Task` the kit's list draws (Mods spec 1.5,
// PR C plan K2). The call is shaped by what the mod may know: without `projects.read` the project's
// name, archive flag and linked milestone are not in the answer, and without `calendar.read` the linked
// event is not. A field that is absent is NOT a link that was removed: core's readers (`deadlineSource`,
// `taskToInput`) take a null `milestone_id` or `event_uid` for "the linked milestone or event was
// removed", so an adapter that filled a redacted link with null would tell the person "the linked
// event was removed" of a task that is fine, and the editor would reset its deadline to "My own date".
// The answer therefore always carries `milestone_linked` and `event_linked`, and a link that exists but
// may not be looked through becomes a NON-null placeholder here, so the chip says "Event's date" with
// no title, and never "removed". A link that is really gone has its flag false and stays null.

import type { Task } from "../../lib/types";

/** What stands for a linked milestone the mod may not look through. Never a real row id. */
export const REDACTED_MILESTONE_ID = -1;
/** What stands for a linked event the mod may not look through. Never a real iCal uid. */
export const REDACTED_EVENT_UID = "pm-redacted-event-link";

/** A task as `tasks.list` answers: the fields a mod always gets, the two flags, and the fields only a
 *  grant adds (`projects.read`: the first four; `calendar.read`: the last two). */
export type ModTaskDto = Omit<
  Task,
  | "project_name"
  | "project_archived"
  | "milestone_id"
  | "milestone_label"
  | "event_uid"
  | "event_title"
> & {
  milestone_linked: boolean;
  event_linked: boolean;
  project_name?: string | null;
  project_archived?: boolean;
  milestone_id?: number | null;
  milestone_label?: string | null;
  event_uid?: string | null;
  event_title?: string | null;
};

/** The kit's `Task` for a task `tasks.list` answered. */
export function fromModTask(dto: ModTaskDto): Task {
  const { milestone_linked, event_linked, ...rest } = dto;
  return {
    ...rest,
    project_name: dto.project_name ?? null,
    project_archived: dto.project_archived ?? false,
    milestone_id: dto.milestone_id ?? (milestone_linked ? REDACTED_MILESTONE_ID : null),
    milestone_label: dto.milestone_label ?? null,
    event_uid: dto.event_uid ?? (event_linked ? REDACTED_EVENT_UID : null),
    event_title: dto.event_title ?? null,
  };
}
