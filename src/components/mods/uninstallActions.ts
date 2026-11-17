// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Uninstalling mods as Mods actions (lib/modActions): one mod, and "Remove all mods" (Mods spec §1.14).
// Both are the SAME runtime uninstall the Mods tab's dialog runs (`uninstallMod`), so a mod leaves the
// way it always does (its sessions end, its row and storage go, its tab and folds are forgotten on this
// device). They live here, outside any view, for the reason lib/modActions gives: Settings and the Mods
// tab unmount while the backend's work keeps going, and the follow-up (forgetting the tab and the folds,
// saying what happened) belongs to the action, not to a dialog that may be gone.
//
// "Remove all" never deletes what the mods made (tasks, milestones, notes): that choice is the
// per-mod dialog's, with its preview, and a bulk button doesn't make it for the person. It goes one mod
// at a time, each its own step; a mod that can't be removed is reported and the rest still go.

import { uninstallMod } from "../../lib/ipc";
import { addModNote, runModAction, type ModActionResult } from "../../lib/modActions";
import { pruneAfterUninstall } from "../../lib/modFolds";
import type { ModUninstalled } from "../../lib/modsTypes";
import { modText, quoteName } from "./modText";
import { plural, uninstallNote } from "./uninstallNote";

/** The busy/error key of "Remove all mods". */
export const REMOVE_ALL_KEY = "mods:remove-all";

/**
 * Uninstall one mod. `alsoDeleteGifts` is the person's choice from the preview; `quiet` leaves the
 * "was uninstalled" line out of the Mods tab's notes (a caller that says it once for many does).
 * Resolves `{ok: false, error: null}` when that mod already has something going.
 */
export function uninstallAction(
  modId: string,
  name: string,
  alsoDeleteGifts: boolean,
  quiet = false,
): Promise<ModActionResult<ModUninstalled>> {
  return runModAction(
    modId,
    "Uninstalling",
    "PM couldn't uninstall this mod.",
    () => uninstallMod(modId, alsoDeleteGifts),
    (done) => {
      // Before anything is drawn again, and whether or not the caller's screen is still there.
      pruneAfterUninstall(modId, done.prune);
      if (quiet) return;
      const note = uninstallNote(name, done);
      addModNote(note.warning ? "warning" : "info", note.text);
    },
  );
}

export interface RemoveAllResult {
  /** Names (already made plain lines) of the mods that went. */
  removed: string[];
  /** Mods whose files PM couldn't all delete: they are gone, and PM clears the rest out later. */
  filesLeft: string[];
  /** A mod that stayed, and why. */
  failed: { name: string; error: string }[];
}

/** What "Remove all" says once it has finished. A warning when anything didn't go cleanly. */
export function removeAllNote(r: RemoveAllResult): { warning: boolean; text: string } {
  const parts: string[] = [];
  if (r.removed.length > 0) parts.push(`Removed ${plural(r.removed.length, "mod", "mods")}.`);
  for (const f of r.failed) parts.push(`${quoteName(f.name)} stayed: ${f.error}`);
  if (r.filesLeft.length > 0) {
    parts.push(
      `PM couldn't delete all the files of ${r.filesLeft.map(quoteName).join(", ")}, and will clear them out itself later.`,
    );
  }
  if (parts.length === 0) parts.push("There were no mods to remove.");
  return { warning: r.failed.length > 0 || r.filesLeft.length > 0, text: parts.join(" ") };
}

/**
 * Uninstall every mod in `mods` (the list the person saw and confirmed), one at a time. The result
 * is `ok` even when some mods stayed: those are in `failed`, and one note in the Mods tab says it all.
 */
export function removeAllMods(
  mods: readonly { id: string; name: string }[],
): Promise<ModActionResult<RemoveAllResult>> {
  return runModAction(
    REMOVE_ALL_KEY,
    "Removing all mods",
    "PM couldn't remove the mods.",
    async () => {
      const result: RemoveAllResult = { removed: [], filesLeft: [], failed: [] };
      for (const m of mods) {
        const name = modText(m.name) || m.id;
        const r = await uninstallAction(m.id, name, false, true);
        if (r.ok) {
          result.removed.push(name);
          if (!r.value.filesRemoved) result.filesLeft.push(name);
        } else {
          result.failed.push({
            name,
            error: r.error ?? "It was busy with something else. Try again in a moment.",
          });
        }
      }
      return result;
    },
    (result) => {
      const note = removeAllNote(result);
      addModNote(note.warning ? "warning" : "info", note.text);
    },
  );
}
