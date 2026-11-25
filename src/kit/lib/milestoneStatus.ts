// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A milestone's progress status, as the kit's task and timeline code reads it. Pure: it lived in
// `MilestoneList.tsx`, which imports `ipc`, so it moved here (the kit imports only the kit) and the
// list imports it back.

import type { Milestone, MilestoneStatus } from "../../lib/types";

/** Display order + labels for the progress control, coarsest-first (mirrors `milestones::STATUSES`). */
export const MILESTONE_STATUSES: readonly { value: MilestoneStatus; label: string }[] = [
  { value: "not_started", label: "Not started" },
  { value: "in_progress", label: "In progress" },
  { value: "almost_done", label: "Almost done" },
  { value: "done", label: "Done" },
];

/** A milestone's effective status: its stored value, or — for a row predating the status column —
 *  the one implied by whether it's been ticked off, so an old row never renders blank. It reads only
 *  `state` and `status`, so a mod's own milestone type (which never says who made a milestone) fits. */
export function milestoneStatus(m: Pick<Milestone, "state" | "status">): MilestoneStatus {
  return m.status ?? (m.state === "met" ? "done" : "not_started");
}
