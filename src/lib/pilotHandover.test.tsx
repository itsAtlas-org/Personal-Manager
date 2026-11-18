// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The page's half of a pilot's handover (PR B plan B8): the Map's per-device prefs go to the mod's
// storage and stay on this device for the next vault, and the sidebar rule runs once per
// device, only when the core Map tab is gone. Dormant: with nothing owed, nothing is read or written.

import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ModRows } from "./modRows";
import { ModRowsContext } from "./modRows";
import type { ModsChanged, ModsHandoverState } from "./modsTypes";
import { NAV_PLACEMENT_KEY } from "./nav";

const adoptMapPrefs = vi.fn();
const modsHandoverState = vi.fn();
let changed: ((e: ModsChanged) => void) | null = null;

vi.mock("./ipc", () => ({
  adoptMapPrefs: (mode: string, cohesion: number, labels: boolean) =>
    adoptMapPrefs(mode, cohesion, labels),
  modsHandoverState: () => modsHandoverState(),
  onModsChanged: (h: (e: ModsChanged) => void) => {
    changed = h;
    return Promise.resolve(() => {});
  },
}));

// The core Map tab still exists in this branch; the rule's switch is the one fact the tests flip.
let coreMapGone = true;
vi.mock("./nav", async (importOriginal) => {
  const real = await importOriginal<typeof import("./nav")>();
  return {
    ...real,
    pilotHiddenRuleApplies: () => coreMapGone,
    placementAfterPilotLanded: (
      p: import("./nav").NavPlacement,
      id: string,
      depth: import("../theme").Depth,
    ) => real.placementAfterPilotLanded(p, id, depth, new Set(["focus"])),
  };
});

let depth: "min" | "standard" | "power" = "min";
vi.mock("../theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ depth }),
}));

const {
  applyHandover,
  usePilotHandover,
  readDeviceMapPrefs,
  readHandled,
  MAP_MODE_KEY,
  MAP_COHESION_KEY,
  MAP_LABELS_KEY,
  PILOT_HANDLED_KEY,
} = await import("./pilotHandover");

const EMPTY: ModsHandoverState = { landed: [], adoptMapPrefs: false };

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  coreMapGone = true;
  depth = "min";
  changed = null;
  adoptMapPrefs.mockResolvedValue(true);
  modsHandoverState.mockResolvedValue(EMPTY);
});
afterEach(cleanup);

describe("the Map's device-local prefs", () => {
  const setPrefs = () => {
    localStorage.setItem(MAP_MODE_KEY, "semantic");
    localStorage.setItem(MAP_COHESION_KEY, "0.25");
    localStorage.setItem(MAP_LABELS_KEY, "1");
  };

  it("are read the way the core Map read them", () => {
    expect(readDeviceMapPrefs()).toBeNull();
    setPrefs();
    expect(readDeviceMapPrefs()).toEqual({ layoutMode: "semantic", cohesion: 0.25, labels: true });
    localStorage.setItem(MAP_MODE_KEY, "grid");
    localStorage.setItem(MAP_COHESION_KEY, "9");
    localStorage.setItem(MAP_LABELS_KEY, "0");
    expect(readDeviceMapPrefs()).toEqual({ layoutMode: "project", cohesion: 0.5, labels: false });
    localStorage.setItem(MAP_COHESION_KEY, "lots");
    expect(readDeviceMapPrefs()?.cohesion).toBe(0);
    localStorage.removeItem(MAP_MODE_KEY);
    localStorage.removeItem(MAP_COHESION_KEY);
    expect(readDeviceMapPrefs()).toEqual({ layoutMode: "project", cohesion: 0, labels: false });
  });

  it("go to the mod and stay on this device, so the next vault to land here gets them too", async () => {
    setPrefs();
    let finish: (v: boolean) => void = () => {};
    adoptMapPrefs.mockReturnValueOnce(new Promise<boolean>((r) => (finish = r)));
    const done = applyHandover({ landed: [], adoptMapPrefs: true }, "standard");
    await waitFor(() => expect(adoptMapPrefs).toHaveBeenCalledWith("semantic", 0.25, true));
    finish(true);
    await done;
    for (const k of [MAP_MODE_KEY, MAP_COHESION_KEY, MAP_LABELS_KEY]) {
      expect(localStorage.getItem(k), k).not.toBeNull();
    }
    // A second vault on this device lands later (the backend's flag is per vault): same values.
    await applyHandover({ landed: [], adoptMapPrefs: true }, "standard");
    expect(adoptMapPrefs).toHaveBeenCalledTimes(2);
    expect(adoptMapPrefs).toHaveBeenLastCalledWith("semantic", 0.25, true);
  });

  it("are sent again after PM refuses", async () => {
    setPrefs();
    adoptMapPrefs.mockRejectedValueOnce({ code: "not-installed", message: "No." });
    await applyHandover({ landed: [], adoptMapPrefs: true }, "standard");
    expect(localStorage.getItem(MAP_MODE_KEY)).toBe("semantic");
    await applyHandover({ landed: [], adoptMapPrefs: true }, "standard");
    expect(adoptMapPrefs).toHaveBeenCalledTimes(2);
  });

  it("are read from the keys the core Map wrote (pinned as text, the core Map being gone)", () => {
    // The core Map is no longer in PM, so nothing else holds these strings: an edit that renames one
    // would silently stop handing a person's saved arrangement over.
    expect(MAP_MODE_KEY).toBe("pm.map.layoutMode");
    expect(MAP_COHESION_KEY).toBe("pm.map.cohesion");
    expect(MAP_LABELS_KEY).toBe("pm.map.labels");
  });

  it("aren't sent when PM says nothing waits, or when the device has none", async () => {
    setPrefs();
    await applyHandover(EMPTY, "standard");
    expect(adoptMapPrefs).not.toHaveBeenCalled();
    expect(localStorage.getItem(MAP_MODE_KEY)).toBe("semantic");
    localStorage.clear();
    await applyHandover({ landed: [], adoptMapPrefs: true }, "standard");
    expect(adoptMapPrefs).not.toHaveBeenCalled();
  });
});

describe("the sidebar rule", () => {
  const landed: ModsHandoverState = { landed: ["atlas.map"], adoptMapPrefs: false };

  it("hides the new tab once on a Minimal device, and records that it ran", async () => {
    await applyHandover(landed, "min");
    expect(JSON.parse(localStorage.getItem(NAV_PLACEMENT_KEY) ?? "null")).toEqual({
      hidden: ["mod:atlas.map"],
      pinned: [],
    });
    expect(readHandled()).toEqual(["atlas.map"]);
    // The user shows it again (Show removes it from hidden); the next pass must not hide it a
    // second time, on this device it has already run.
    localStorage.setItem(NAV_PLACEMENT_KEY, JSON.stringify({ hidden: [], pinned: [] }));
    await applyHandover(landed, "min");
    expect(JSON.parse(localStorage.getItem(NAV_PLACEMENT_KEY) ?? "null")).toEqual({
      hidden: [],
      pinned: [],
    });
  });

  it("changes nothing at Standard, but still counts as done on this device", async () => {
    await applyHandover(landed, "standard");
    expect(localStorage.getItem(NAV_PLACEMENT_KEY)).toBeNull();
    expect(readHandled()).toEqual(["atlas.map"]);
  });

  it("waits while the core Map tab still exists, and does not count that as done", async () => {
    coreMapGone = false;
    await applyHandover(landed, "min");
    expect(localStorage.getItem(NAV_PLACEMENT_KEY)).toBeNull();
    expect(localStorage.getItem(PILOT_HANDLED_KEY)).toBeNull();
    coreMapGone = true;
    await applyHandover(landed, "min");
    expect(readHandled()).toEqual(["atlas.map"]);
  });

  it("reads a damaged record as nobody being handled", () => {
    for (const bad of ["{", "5", '{"a":1}', "null"]) {
      localStorage.setItem(PILOT_HANDLED_KEY, bad);
      expect(readHandled(), bad).toEqual([]);
    }
    localStorage.setItem(PILOT_HANDLED_KEY, JSON.stringify(["atlas.map", 3, null]));
    expect(readHandled()).toEqual(["atlas.map"]);
  });
});

describe("the hook", () => {
  function Probe() {
    usePilotHandover();
    return null;
  }
  const draw = (loaded: boolean) => {
    const value: ModRows = { rows: [], loaded, problem: null, refresh: () => Promise.resolve() };
    return render(
      <ModRowsContext.Provider value={value}>
        <Probe />
      </ModRowsContext.Provider>,
    );
  };

  it("asks nothing until the mods are read, and does nothing for a vault with no auto-install", async () => {
    draw(false);
    await act(async () => {});
    expect(modsHandoverState).not.toHaveBeenCalled();
    cleanup();
    draw(true);
    await waitFor(() => expect(modsHandoverState).toHaveBeenCalledTimes(1));
    await act(async () => {});
    expect(adoptMapPrefs).not.toHaveBeenCalled();
    expect(localStorage.length).toBe(0);
  });

  it("asks again after mods://changed, which is how an auto-install arrives, and carries it out", async () => {
    localStorage.setItem(MAP_MODE_KEY, "semantic");
    draw(true);
    await waitFor(() => expect(modsHandoverState).toHaveBeenCalledTimes(1));
    modsHandoverState.mockResolvedValue({ landed: ["atlas.map"], adoptMapPrefs: true });
    await waitFor(() => expect(changed).not.toBeNull());
    await act(async () => changed?.({ cause: "auto-installed" }));
    await waitFor(() => expect(adoptMapPrefs).toHaveBeenCalledWith("semantic", 0, false));
    await waitFor(() => expect(readHandled()).toEqual(["atlas.map"]));
    expect(localStorage.getItem(MAP_MODE_KEY)).toBe("semantic");
    expect(readHandled()).toEqual(["atlas.map"]);
  });

  it("survives the vault being locked", async () => {
    modsHandoverState.mockRejectedValue({ code: "no-store", message: "Locked." });
    draw(true);
    await waitFor(() => expect(modsHandoverState).toHaveBeenCalled());
    await act(async () => {});
    expect(localStorage.length).toBe(0);
  });
});
