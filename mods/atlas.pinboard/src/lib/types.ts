// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The few of PM's own types the Pinboard names. A mod may import nothing from PM's src/ but the kit
// (the lint fence), and the type file the kit itself reads (src/lib/types.ts) is not a door for a mod,
// so these are written out here in the shape `milestones.list` and the overlay answer with. Each is
// structurally the one the kit's own helpers take (`milestoneStatus`), so they pass between the two.

/** The four progress values a milestone's `status` admits, coarsest-first (`milestones::STATUSES`). */
export type MilestoneStatus = "not_started" | "in_progress" | "almost_done" | "done";

/** One project milestone, as `milestones.list` answers a mod that holds `projects.read`. */
export interface Milestone {
  id: number;
  project_name: string;
  label: string;
  /** The effective date: user-set for a PM-native milestone, synced from the event for a linked one. */
  due_date: string | null;
  event_uid: string | null;
  /** True when the date comes from a linked calendar event (read-only). */
  calendar_linked: boolean;
  /** True for a linked milestone whose event is not in the current mirror. */
  event_missing: boolean;
  state: "met" | "unmet" | null;
  status: MilestoneStatus | null;
  /** Never set for a mod: an externally owned milestone's source is not in the answer. */
  source_type: string | null;
  external_id: string | null;
  sort_order: number;
}

/** One dated entry of the Pinboard on the Calendar (`calendar.overlay.set`'s `{id, date, label}`):
 *  `id` is `<widgetId>:<itemId>`, `date` a real `YYYY-MM-DD` day, `label` never empty. */
export interface PinboardEntry {
  id: string;
  date: string;
  label: string;
}
