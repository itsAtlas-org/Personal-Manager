// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The page's half of a pilot's handover (the Map moving into a mod; PR B plan B8). The backend moves
// what the store held (`mods/autoinstall.rs`); this moves what only this device has:
//
//   - the Map's per-device view prefs (arrangement, cohesion, names), kept in localStorage by the core
//     Map. A sealed frame can't read them, so once the backend says they are waiting, they go to the
//     mod's storage through `adoptMapPrefs` and are removed from this device only after it resolves;
//   - the sidebar: the mod's tab is added to this device's hidden set once, when the core Map tab
//     wasn't showing here (Minimal and not pinned), so installing it doesn't make a tab appear that
//     this sidebar had left out (`placementAfterPilotLanded`).
//
// Both start from `modsHandoverState`, which is empty for a vault with no auto-install (every vault
// until a migration writes a marker), so this does nothing until then. The sidebar rule additionally
// waits for the core Map tab to be gone from PM's own entries (`pilotHiddenRuleApplies`).

import { useEffect, useState } from "react";
import { useTheme, type Depth } from "../theme";
import { adoptMapPrefs, modsHandoverState, onModsChanged } from "./ipc";
import { clamp } from "./math";
import { useModRows } from "./modRows";
import type { ModsHandoverState } from "./modsTypes";
import {
  pilotHiddenRuleApplies,
  placementAfterPilotLanded,
  readNavPlacement,
  writeNavPlacement,
} from "./nav";
import { subscribeUntilCleanup } from "./subscribe";

// The core Map's own localStorage keys (written by GraphView and Settings until the Map is removed;
// named here so that removal can't take the handover with it).
export const MAP_MODE_KEY = "pm.map.layoutMode";
export const MAP_COHESION_KEY = "pm.map.cohesion";
export const MAP_LABELS_KEY = "pm.map.labels";

/** The mods whose arrival this device has already dealt with (its sidebar rule ran): a JSON array of
 *  mod ids. Per device, because the sidebar is. */
export const PILOT_HANDLED_KEY = "pm.mods.pilotHandled";

const MAX_HANDLED = 32;

function lsGet(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function lsSet(key: string, value: string): boolean {
  try {
    localStorage.setItem(key, value);
    return true;
  } catch {
    return false;
  }
}

function lsRemove(key: string): void {
  try {
    localStorage.removeItem(key);
  } catch {
    /* best-effort */
  }
}

export function readHandled(): string[] {
  try {
    const value: unknown = JSON.parse(lsGet(PILOT_HANDLED_KEY) ?? "[]");
    return Array.isArray(value)
      ? value.filter((v): v is string => typeof v === "string").slice(0, MAX_HANDLED)
      : [];
  } catch {
    return [];
  }
}

export interface DeviceMapPrefs {
  layoutMode: "project" | "semantic";
  cohesion: number;
  labels: boolean;
}

/** This device's Map prefs as the core Map stored them, or null when it stored none. What it did
 *  store is read the way the core read it: anything but "semantic" is by-project, cohesion is clamped
 *  to 0..0.5 and a non-number is 0, names are on only for "1". */
export function readDeviceMapPrefs(): DeviceMapPrefs | null {
  const mode = lsGet(MAP_MODE_KEY);
  const cohesion = lsGet(MAP_COHESION_KEY);
  const labels = lsGet(MAP_LABELS_KEY);
  if (mode === null && cohesion === null && labels === null) return null;
  const raw = Number(cohesion);
  return {
    layoutMode: mode === "semantic" ? "semantic" : "project",
    cohesion: cohesion !== null && Number.isFinite(raw) ? clamp(raw, 0, 0.5) : 0,
    labels: labels === "1",
  };
}

function dropDeviceMapPrefs(): void {
  lsRemove(MAP_MODE_KEY);
  lsRemove(MAP_COHESION_KEY);
  lsRemove(MAP_LABELS_KEY);
}

let running = false;

/** Carry out what `state` says is owed on this device. Never throws: a failure leaves the device's
 *  own keys where they are, and the next `mods://changed` or launch tries again. */
export async function applyHandover(state: ModsHandoverState, depth: Depth): Promise<void> {
  if (running) return;
  running = true;
  try {
    // The sidebar, once per device and mod, and only when the core tab it replaced is gone.
    const handled = readHandled();
    for (const id of state.landed) {
      if (handled.includes(id) || !pilotHiddenRuleApplies(id)) continue;
      const before = readNavPlacement(depth);
      const after = placementAfterPilotLanded(before, id, depth);
      if (after !== before) writeNavPlacement(after);
      handled.push(id);
      lsSet(PILOT_HANDLED_KEY, JSON.stringify(handled.slice(-MAX_HANDLED)));
    }
    // The Map's device-local prefs, removed from this device only after the mod holds them.
    if (state.adoptMapPrefs) {
      const prefs = readDeviceMapPrefs();
      if (prefs) {
        try {
          await adoptMapPrefs(prefs.layoutMode, prefs.cohesion, prefs.labels);
          dropDeviceMapPrefs();
        } catch {
          /* kept: tried again later */
        }
      }
    }
  } finally {
    running = false;
  }
}

/** Mounted once in the main window: asks the backend what is owed once the mods are read, and again
 *  after every `mods://changed` (an auto-install lands as one) and Depth change. */
export function usePilotHandover(): void {
  const { loaded } = useModRows();
  const { depth } = useTheme();
  const [changes, setChanges] = useState(0);
  useEffect(() => subscribeUntilCleanup(() => onModsChanged(() => setChanges((n) => n + 1))), []);
  useEffect(() => {
    if (!loaded) return;
    let cancelled = false;
    void modsHandoverState()
      .then((state) => (cancelled ? undefined : applyHandover(state, depth)))
      .catch(() => {
        /* the vault is locked or another window writes: asked again on the next change */
      });
    return () => {
      cancelled = true;
    };
  }, [loaded, depth, changes]);
}
