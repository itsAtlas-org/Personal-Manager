// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Every sentence the Map shows, in one place and as plain functions, so a test can hold the words (the
// core Map's where it had them) and a reviewer reads them together. Nothing here takes text from PM or
// from a document except a count: a failure is always one of these sentences, never PM's own words.

import type { ChangeNotice } from "./changes";
import type { LayoutMode } from "./settings";
import type { ProjectPhase } from "./projectLayout";
import type { SemanticPhase } from "./semantic";

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

const HINT = "scroll to zoom, drag to pan";

/** The line under the title. By meaning says how many are placed when the cap left some out. */
export function subtitle(
  mode: LayoutMode,
  documents: number,
  projects: number,
  placed: number,
): string {
  if (mode === "semantic") {
    return placed > 0 && placed < documents
      ? `showing the top ${placed} of ${documents} documents by meaning · ${HINT}`
      : `${plural(documents, "document", "documents")} by meaning · ${HINT}`;
  }
  return `${plural(documents, "document", "documents")} across ${plural(projects, "project", "projects")} · ${HINT}, click a document to read it`;
}

/** What the strip says while the map is being worked out; null for the quick check of what is saved
 *  (a saved map opens at once and should not flash a strip). */
export function phaseText(phase: SemanticPhase | ProjectPhase): string | null {
  switch (phase) {
    case "checking":
      return null;
    case "reading":
      return "Reading what your documents are about…";
    case "reducing":
      return "Placing documents by meaning…";
    case "computing":
      return "Arranging your projects…";
    case "saving":
      return "Saving the map for next time…";
  }
}

/** The notice that documents changed while the map was open. Counts are a floor when PM dropped some
 *  events (`missed`), so then it says only that things changed. */
export function noticeText(notice: ChangeNotice): string {
  if (!notice.missed) {
    if (notice.added > 0 && notice.deleted > 0) {
      return `${plural(notice.added, "document was", "documents were")} added and ${notice.deleted} removed.`;
    }
    if (notice.added > 0) return `${plural(notice.added, "document was", "documents were")} added.`;
    if (notice.deleted > 0) {
      return `${plural(notice.deleted, "document was", "documents were")} removed.`;
    }
  }
  return "Your documents changed while the map was open.";
}

export const TITLE = "Map";
export const NO_DOCUMENTS =
  "No documents yet. Ingest some in the Documents view and they'll appear here.";
export const NOTHING_TO_PLACE =
  "Nothing to place by meaning yet. Documents appear here once PM has read them.";
export const PREPARING = "Preparing the map…";
export const NOT_FRAMED = "The Map opens inside PM.";

export const LOAD_FAILED = "The map couldn't be loaded. Your documents are unchanged.";
export const ARRANGE_FAILED = "The map couldn't be laid out. Your documents are unchanged.";
export const OPEN_FAILED = "That document couldn't be opened.";
export const SETTINGS_READ_FAILED =
  "Your map settings couldn't be read, so the standard ones are showing.";
export const SETTINGS_SAVE_FAILED =
  "Your map settings couldn't be saved. They will be as they were next time.";

/** The cohesion choices, as the core Map offers them. */
export const COHESION_CHOICES: readonly { value: number; label: string }[] = [
  { value: 0, label: "Off" },
  { value: 0.15, label: "Low" },
  { value: 0.3, label: "Medium" },
  { value: 0.5, label: "High" },
];

/** The choice nearest `cohesion`, for showing a stored value the menu has no exact entry for. */
export function nearestCohesion(cohesion: number): number {
  let best = COHESION_CHOICES[0].value;
  for (const c of COHESION_CHOICES) {
    if (Math.abs(c.value - cohesion) < Math.abs(best - cohesion)) best = c.value;
  }
  return best;
}

export const STARTING = "Starting the map…";
export const NO_ANSWER = "PM didn't answer. Close this tab and open it again.";
