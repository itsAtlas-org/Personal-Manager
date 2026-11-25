// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The page's half of a pilot's handover (the Map moving into a mod; PR B plan B8). The backend moves
// what the store held (`mods/autoinstall.rs`); this moves what only this device has:
//
//   - the Map's per-device view prefs (arrangement, cohesion, names), kept in localStorage by the core
//     Map. A sealed frame can't read them, so once the backend says they are waiting, they go to the
//     mod's storage through `adoptMapPrefs`. They stay on the device: the mod's storage is per vault,
//     and every vault this device opens is owed them once;
//   - the sidebar: the mod's tab is added to this device's hidden set once, when the core Map tab
//     wasn't showing here (Minimal and not pinned), so installing it doesn't make a tab appear that
//     this sidebar had left out (`placementAfterPilotLanded`).
//
//   - the Pinboard's three nags ("ask before deleting a card", "show past entries", "show completed
//     entries"), kept in localStorage by the core Pinboard, go to the mod's `prefs` key through
//     `adoptPinboardPrefs` the same way (dormant until PM's move of the Pinboard has run).
//
// Both start from `modsHandoverState`, which is empty for a vault with no auto-install (a fresh one,
// or one with no documents: v61 writes the Map's marker only where a document exists), so this does
// nothing there. The sidebar rule additionally waits for the core Map tab to be gone from PM's own
// entries (`pilotHiddenRuleApplies`), which it now is.

import { useEffect, useState } from "react";
import { useTheme, type Depth } from "../theme";
import { adoptMapPrefs, adoptPinboardPrefs, modsHandoverState, onModsChanged } from "./ipc";
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

// The core Map's own localStorage keys (written by the Map and by Settings before the Map became a
// mod; they stay named here so the handover can still read what a device saved).
export const MAP_MODE_KEY = "pm.map.layoutMode";
export const MAP_COHESION_KEY = "pm.map.cohesion";
export const MAP_LABELS_KEY = "pm.map.labels";

// The core Pinboard's own localStorage keys (`lib/pinboard/prefs.ts`): each is "false" when the person
// said "don't ask again" or hid that kind of entry, and anything else (or nothing) means the default.
export const PINBOARD_CONFIRM_DELETE_KEY = "pm.pinboard.confirmDelete";
export const PINBOARD_SHOW_PAST_KEY = "pm.pinboard.timeline.showPast";
export const PINBOARD_SHOW_COMPLETED_KEY = "pm.pinboard.timeline.showCompleted";

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

/** This device's Pinboard nags, each read the way the core Pinboard read it (only the literal "false"
 *  turns a nag off) or null when this device never set it. */
export interface DevicePinboardPrefs {
  confirmDelete: boolean | null;
  showPast: boolean | null;
  showCompleted: boolean | null;
}

/** This device's Pinboard nags, or null when it set none (there is nothing to hand over). */
export function readDevicePinboardPrefs(): DevicePinboardPrefs | null {
  const nag = (key: string): boolean | null => {
    const value = lsGet(key);
    return value === null ? null : value !== "false";
  };
  const prefs = {
    confirmDelete: nag(PINBOARD_CONFIRM_DELETE_KEY),
    showPast: nag(PINBOARD_SHOW_PAST_KEY),
    showCompleted: nag(PINBOARD_SHOW_COMPLETED_KEY),
  };
  return prefs.confirmDelete === null && prefs.showPast === null && prefs.showCompleted === null
    ? null
    : prefs;
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
    // The Map's device-local prefs go to the mod's per-vault storage. The device's keys are NEVER
    // removed: they belong to the device, not to the vault that landed first, and a second vault on
    // this device that lands later is owed the same arrangement (its own backend flag says so; each
    // vault takes them once, only into fields it doesn't hold). Nothing writes the keys once the core
    // Map is gone, so leaving them costs three short strings.
    if (state.adoptMapPrefs) {
      const prefs = readDeviceMapPrefs();
      if (prefs) {
        try {
          await adoptMapPrefs(prefs.layoutMode, prefs.cohesion, prefs.labels);
        } catch {
          /* the backend keeps its flag: tried again on the next change */
        }
      }
    }
    // The Pinboard's nags: the same way. Only the fields the mod doesn't hold are filled, and the
    // device's keys stay for the same reason.
    if (state.adoptPinboardPrefs) {
      const prefs = readDevicePinboardPrefs();
      if (prefs) {
        try {
          await adoptPinboardPrefs(prefs.confirmDelete, prefs.showPast, prefs.showCompleted);
        } catch {
          /* the backend keeps its flag: tried again on the next change */
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
