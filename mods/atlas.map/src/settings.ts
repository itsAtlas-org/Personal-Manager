// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Map's view settings: ONE content key, `settings`, holding `{layoutMode, cohesion, labels,
// nodeCap}`. In the core Map these were split across per-device localStorage (arrangement, cohesion,
// labels: src/lib/mapPrefs.ts and GraphView) and a per-vault database setting (the node cap); a sealed
// frame only has per-vault storage, so all four live there and travel with the vault and its backups
// (Mods PR B plan, Q4; Bobby approved the per-vault default). A content key is the user's: kept on
// update and asked about on uninstall (manifest.json declares it so).
//
// Reading is forgiving, writing is exact. Whatever is stored (a value from an older or newer Map, from
// the install-time handover that writes only `nodeCap`, or garbage) parses to a complete, in-range
// settings object: a field that is missing or not understood takes its default, a number out of range is
// clamped, as the core clamps them. What is written is always the complete object.

import { codeOf, CODE, isCount, readKey, writeKey, type MapHost } from "./hostApi";

export const KEY_SETTINGS = "settings";

/** The Map's arrangement: the by-project force layout, or semantic proximity. */
export type LayoutMode = "project" | "semantic";

export interface MapSettings {
  layoutMode: LayoutMode;
  /** The project-cohesion pull for the by-meaning layout, 0 (pure meaning) to 0.5. */
  cohesion: number;
  /** Show each document's file name on its node. */
  labels: boolean;
  /** The most documents the by-meaning layout places individually. */
  nodeCap: number;
}

export const NODE_CAP_MIN = 200;
export const NODE_CAP_MAX = 5000;
export const NODE_CAP_DEFAULT = 1000;
export const COHESION_MAX = 0.5;

export const DEFAULT_SETTINGS: Readonly<MapSettings> = {
  layoutMode: "project",
  cohesion: 0,
  labels: false,
  nodeCap: NODE_CAP_DEFAULT,
};

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

/** The settings a stored value means. Never throws. */
export function parseSettings(value: unknown): MapSettings {
  const raw = typeof value === "object" && value !== null ? (value as Record<string, unknown>) : {};
  return {
    // Anything but "semantic" is the by-project default, as the core read its stored arrangement.
    layoutMode: raw.layoutMode === "semantic" ? "semantic" : "project",
    cohesion:
      typeof raw.cohesion === "number" && Number.isFinite(raw.cohesion)
        ? clamp(raw.cohesion, 0, COHESION_MAX)
        : DEFAULT_SETTINGS.cohesion,
    labels: raw.labels === true,
    // A whole number is kept within range; anything else is the default (a fraction or a negative was
    // not a value the core's reader accepted either).
    nodeCap: isCount(raw.nodeCap)
      ? clamp(raw.nodeCap, NODE_CAP_MIN, NODE_CAP_MAX)
      : DEFAULT_SETTINGS.nodeCap,
  };
}

/** The settings and the revision they were read at. */
export async function loadSettings(
  host: MapHost,
): Promise<{ settings: MapSettings; revision: number }> {
  const { value, revision } = await readKey(host, KEY_SETTINGS);
  return { settings: parseSettings(value), revision };
}

/** How many times a save re-reads and tries again when another frame wrote first. */
const SAVE_ATTEMPTS = 3;

/**
 * Change some settings: read what is stored, apply `patch` over it, write the whole object. A write
 * based on a stale copy (a second frame of the Map saved in between) is refused by PM; this re-reads
 * and applies the patch again, so a toggle in one frame is never lost to a toggle in another. Returns
 * what was saved. Rejects with PM's error when it is not a stale write, or after the last attempt.
 */
export async function saveSettings(
  host: MapHost,
  patch: Partial<MapSettings>,
): Promise<MapSettings> {
  for (let attempt = 1; ; attempt++) {
    const { settings, revision } = await loadSettings(host);
    // `parseSettings` again over the merged object: the patch gets the same clamping a stored value does.
    const next = parseSettings({ ...settings, ...patch });
    try {
      await writeKey(host, KEY_SETTINGS, next, revision);
      return next;
    } catch (error) {
      if (codeOf(error) !== CODE.staleRevision || attempt >= SAVE_ATTEMPTS) throw error;
    }
  }
}
