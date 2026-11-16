// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the Mods tab says once an uninstall has finished, when the mod's row is no longer there to say
// it. Plain words; the mod's name is its own text, and so is a deletion problem's sentence (the
// backend's).

import type { ModUninstalled } from "../../lib/modsTypes";

export const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

export function uninstallNote(
  name: string,
  done: ModUninstalled,
): { warning: boolean; text: string } {
  const parts = [`${name} was uninstalled.`];
  if (!done.filesRemoved) {
    parts.push("PM couldn't delete all of its files, and will clear them out itself later.");
  }
  const g = done.giftsDeleted;
  if (g) {
    const gone = [
      g.tasks > 0 ? plural(g.tasks, "task", "tasks") : null,
      g.milestones > 0 ? plural(g.milestones, "milestone", "milestones") : null,
      g.documents > 0 ? plural(g.documents, "note", "notes") : null,
    ].filter((x): x is string => x !== null);
    if (gone.length > 0) parts.push(`Deleted what it made: ${gone.join(", ")}.`);
    if (g.problem) parts.push(`PM couldn't delete everything it made: ${g.problem}`);
  }
  return { warning: !done.filesRemoved || Boolean(g?.problem), text: parts.join(" ") };
}
