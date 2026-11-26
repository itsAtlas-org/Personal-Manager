// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The right-click menu of a milestone row on the project page. Each item repeats a control the row
// already shows (the progress dropdown, the ↑/↓ arrows, Unlink and ×) and calls the handler that
// control calls, so the question each of the last two asks stays in one place: the menu never
// removes or unlinks by itself. An action the row doesn't offer is left out (Unlink on a row with no
// calendar link, the arrows under a sort); only Move up and down are ever disabled, and then only at
// the ends of the list. Most of the row is an input, a date field or a dropdown, which keep the
// system menu, so this is reached on the row's padding with the mouse and from its buttons with the
// keyboard.

import type { MenuItem } from "../kit/lib/contextMenu";
import { MILESTONE_STATUSES } from "../kit/lib/milestoneStatus";
import type { Milestone, MilestoneStatus } from "./types";

export interface MilestoneMenuHandlers {
  setStatus: (next: MilestoneStatus) => void;
  /** Absent under an active sort, where the arrows are hidden too. */
  onMove?: (dir: -1 | 1) => void;
  isFirst: boolean;
  isLast: boolean;
  /** Ask "Unlink from calendar?" (shown only for a calendar-linked milestone). */
  askUnlink: () => void;
  /** Ask "Remove this milestone?". */
  askRemove: () => void;
}

/** The items for `m`, whose effective progress is `status`. Choosing the status it already has does
 *  nothing, so a stray press can't send a write that changes nothing. */
export function milestoneMenuItems(
  m: Pick<Milestone, "calendar_linked">,
  status: MilestoneStatus,
  h: MilestoneMenuHandlers,
): MenuItem[] {
  const move = h.onMove;
  return [
    ...MILESTONE_STATUSES.map((s) => ({
      label: s.label,
      checked: s.value === status,
      onSelect: () => {
        if (s.value !== status) h.setStatus(s.value);
      },
    })),
    ...(move
      ? ([
          { kind: "separator" },
          { label: "Move up", disabled: h.isFirst, onSelect: () => move(-1) },
          { label: "Move down", disabled: h.isLast, onSelect: () => move(1) },
        ] satisfies MenuItem[])
      : []),
    { kind: "separator" },
    ...(m.calendar_linked
      ? [{ label: "Unlink from calendar…", onSelect: h.askUnlink } satisfies MenuItem]
      : []),
    { label: "Remove…", danger: true, onSelect: h.askRemove },
  ];
}
