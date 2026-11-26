// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The right-click menu of a task row in the kit's list (the project page, the Pinboard's Task card
// and the Pinboard mod's frame). Every item repeats a control the row already shows (the tick, the
// ✎ and the ↑/↓ arrows), so the menu is a shortcut and never the only way; the one exception is
// Delete task…, which a row has never had, and which is offered only where the host can ask the
// question and do the delete (`TaskData.remove`). A mod frame has no delete in its Host API, so its
// rows simply don't show it. The status radios are shared with the Calendar's task chip and the
// Runway's cards, so the three can't word them differently.

import type { Task, TaskStatus } from "../../lib/types";
import type { MenuItem } from "../lib/contextMenu";
import { TASK_STATUS_LABEL } from "../lib/tasks";

/** To do, Doing, Done as one choice among three, the current one ticked. Choosing the status a task
 *  already has does nothing, so a stray press can't send a write that changes nothing. */
export function statusItems(current: TaskStatus, pick: (next: TaskStatus) => void): MenuItem[] {
  return (["todo", "doing", "done"] as const).map((s) => ({
    label: TASK_STATUS_LABEL[s],
    checked: s === current,
    onSelect: () => {
      if (s !== current) pick(s);
    },
  }));
}

/** What a row's menu may call: the same handlers the row's own buttons call. */
export interface TaskRowMenuHandlers {
  onCycle: (next: TaskStatus) => void;
  onEdit: () => void;
  /** Absent when the row can't be reordered (a sorted list, or a parked or done row). */
  onMove?: (dir: -1 | 1) => void;
  isFirst: boolean;
  isLast: boolean;
  /** Absent where the host can't delete a task (a mod frame): the item is left out. */
  onRemove?: () => void;
}

/** The items for `task`'s row, in the order the row's controls read: edit, status, position, then
 *  the delete last. Move up and down are disabled at the ends of the list, as the arrows are
 *  (position state, which the menu may show); every other action the row doesn't offer is left out. */
export function taskRowMenuItems(task: Task, h: TaskRowMenuHandlers): MenuItem[] {
  const move = h.onMove;
  return [
    { label: "Edit task…", onSelect: h.onEdit },
    ...statusItems(task.status, h.onCycle),
    ...(move
      ? ([
          { kind: "separator" },
          { label: "Move up", disabled: h.isFirst, onSelect: () => move(-1) },
          { label: "Move down", disabled: h.isLast, onSelect: () => move(1) },
        ] satisfies MenuItem[])
      : []),
    ...(h.onRemove
      ? ([
          { kind: "separator" },
          { label: "Delete task…", danger: true, onSelect: h.onRemove },
        ] satisfies MenuItem[])
      : []),
  ];
}
