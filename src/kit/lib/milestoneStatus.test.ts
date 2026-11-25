// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What a milestone's progress reads as, pulled out of MilestoneList (which imports `ipc`) so the kit's
// timeline code can read it. Pure, so it is pinned here, not through a mounted list.

import { describe, expect, it } from "vitest";
import type { Milestone } from "../../lib/types";
import { MILESTONE_STATUSES, milestoneStatus } from "./milestoneStatus";

const milestone = (over: Partial<Milestone>): Milestone => ({
  id: 1,
  project_name: "Atlas",
  label: "Pitch",
  due_date: null,
  event_uid: null,
  calendar_linked: false,
  event_missing: false,
  state: null,
  status: null,
  source_type: null,
  external_id: null,
  sort_order: 0,
  madeBy: null,
  ...over,
});

describe("milestoneStatus", () => {
  it("is the stored status when there is one", () => {
    expect(milestoneStatus(milestone({ status: "in_progress" }))).toBe("in_progress");
    expect(milestoneStatus(milestone({ status: "almost_done", state: "met" }))).toBe("almost_done");
  });

  it("reads a row from before the status column off whether it was ticked off", () => {
    expect(milestoneStatus(milestone({ state: "met" }))).toBe("done");
    expect(milestoneStatus(milestone({ state: null }))).toBe("not_started");
  });
});

describe("MILESTONE_STATUSES", () => {
  it("lists the four statuses, coarsest first, with the words the control shows", () => {
    expect(MILESTONE_STATUSES).toEqual([
      { value: "not_started", label: "Not started" },
      { value: "in_progress", label: "In progress" },
      { value: "almost_done", label: "Almost done" },
      { value: "done", label: "Done" },
    ]);
  });
});
