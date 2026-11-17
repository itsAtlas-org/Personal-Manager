// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Mods tab's remembered folds and what an uninstall forgets (Mods spec §1.3, §1.13). A mod's own
// fold is keyed `mods.mod.<id>` and a section inside it `mods.mod.<id>:<section>`; a mod id can't
// contain a `:` (the grammar the sidebar's `mod:<id>` ids already rely on), so one mod's keys can
// never be taken for another's.

import { pruneFolds } from "./foldPrefs";
import type { ModPrune } from "./modsTypes";
import { forgetNavTab } from "./nav";

/** The fold key of a mod's row, or of one section inside it. */
export function modFoldKey(modId: string, section?: string): string {
  return section === undefined ? `mods.mod.${modId}` : `mods.mod.${modId}:${section}`;
}

/**
 * Forget what this device remembered about a mod that is gone: its sidebar placement and its folds.
 * The backend says what to prune; this only does it for the mod that was asked about, so a reply that
 * named anything else (another mod, or a prefix that would take another mod's keys) prunes nothing
 * for that part rather than something the page didn't ask for.
 */
export function pruneAfterUninstall(modId: string, prune: ModPrune): void {
  if (prune.tab === `mod:${modId}`) forgetNavTab(prune.tab);
  if (prune.foldPrefix === modFoldKey(modId)) pruneFolds(prune.foldPrefix);
}
