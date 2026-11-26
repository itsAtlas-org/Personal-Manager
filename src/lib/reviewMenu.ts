// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The right-click menu of a row in the Review queue: Open, Approve and "Apply this filing to N other
// files", each calling the handler the row's own control calls, so the gating (an approval waits for
// its own suggestion; nothing files while a commit is in flight) lives in one place. An item the row
// isn't offering right now is left out, never greyed, as the row hides or disables its button.
// There is deliberately no Delete here: a triage screen files things, and deleting is the Documents
// tab's job.

import { copyViaHost, type MenuItem } from "../kit/lib/contextMenu";

export interface ReviewMenuHandlers {
  open: () => void;
  /** Absent while the row's Approve is unavailable (its suggestion is awaited, or a commit runs). */
  approve?: () => void;
  /** Absent while the row offers no folder action (no real project yet, or no siblings). */
  applyFolder?: () => void;
  /** How many other files the folder action would file, for its label. */
  applyCount: number;
}

export function reviewRowMenuItems(title: string, h: ReviewMenuHandlers): MenuItem[] {
  return [
    { label: "Open", onSelect: h.open },
    ...(h.approve ? [{ label: "Approve", onSelect: h.approve } satisfies MenuItem] : []),
    ...(h.applyFolder && h.applyCount > 0
      ? [
          {
            label: `Apply this filing to ${h.applyCount} other file${h.applyCount === 1 ? "" : "s"}…`,
            onSelect: h.applyFolder,
          } satisfies MenuItem,
        ]
      : []),
    { kind: "separator" },
    { label: "Copy title", onSelect: () => copyViaHost(title, "Title copied") },
  ];
}
