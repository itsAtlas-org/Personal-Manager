// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// How Settings → Storage says what a mod takes (Mods spec §1.14). A mod's files are a real
// measurement of its own folder; what it saved in the database is an estimate (the bytes of the values
// it stored, not the file's growth). So a size is drawn with a "~" whenever any of it is estimated.

import { formatBytes } from "./format";
import type { StorageMod } from "./types";

/** A mod's footprint: its files plus what it saved. */
export function modBytes(m: StorageMod): number {
  return m.folder_bytes + m.data_bytes;
}

/** "~1.2 MB" when any of it is an estimate (what the mod saved always is), "1.2 MB" when it is all
 *  measured (a mod that saved nothing). */
export function modSize(m: StorageMod): string {
  const s = formatBytes(modBytes(m));
  return m.data_bytes > 0 ? `~${s}` : s;
}

/** The mods' footprint together, with the same "~" rule. */
export function modsTotal(mods: readonly StorageMod[]): string {
  const bytes = mods.reduce((n, m) => n + modBytes(m), 0);
  return mods.some((m) => m.data_bytes > 0) ? `~${formatBytes(bytes)}` : formatBytes(bytes);
}
