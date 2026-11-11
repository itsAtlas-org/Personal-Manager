// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// useRunwayFolds, below the board: what a click writes when this window's copy of the folds is
// stale (the other webview folded since, and no storage event crossed), and when a deleted named
// window's fold is dropped (only once the stored prefs are loaded, or every window would look dead).

import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { RunwayWindow } from "../../../lib/calendarPrefs";
import { buildColumns, type RunwayColumn } from "../../../lib/runway";

const prefs = vi.hoisted(() => ({ hydrated: false, windows: [] as RunwayWindow[] }));
vi.mock("../../../lib/ipc", () => ({}));
vi.mock("../../../lib/storedPrefs", () => ({ storedPrefsHydrated: () => prefs.hydrated }));
vi.mock("../../../lib/calendarPrefs", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  readRunwayWindows: () => prefs.windows,
}));

import { useRunwayFolds } from "./runwayLogic";

const KEY = "pm.calendar.runwayFolded";
const stored = () => JSON.parse(localStorage.getItem(KEY) ?? "[]") as string[];
const COLUMNS = buildColumns(new Date(2026, 10, 4), []);
const col = (id: string): RunwayColumn => COLUMNS.find((c) => c.id === id)!;

beforeEach(() => {
  localStorage.clear();
  prefs.hydrated = false;
  prefs.windows = [];
});

describe("useRunwayFolds", () => {
  it("folds what the column shows open, even when storage already holds that fold", () => {
    const { result } = renderHook(() => useRunwayFolds());
    expect(result.current.isFolded(col("later"))).toBe(false);
    // The pop-out folded Later after this window read its folds; no storage event arrived.
    localStorage.setItem(KEY, JSON.stringify(["later"]));
    act(() => result.current.toggle(col("later")));
    expect(stored()).toEqual(["later"]);
    expect(result.current.isFolded(col("later"))).toBe(true);
    // And the next click, on a column that now shows folded, opens it.
    act(() => result.current.toggle(col("later")));
    expect(stored()).toEqual([]);
    expect(result.current.isFolded(col("later"))).toBe(false);
  });

  it("opens Before today from what it shows, not from what is stored", () => {
    const { result } = renderHook(() => useRunwayFolds());
    expect(result.current.isFolded(col("past"))).toBe(true);
    localStorage.setItem("pm.calendar.runwayPastOpen", "true");
    act(() => result.current.toggle(col("past")));
    expect(localStorage.getItem("pm.calendar.runwayPastOpen")).toBe("true");
    expect(result.current.isFolded(col("past"))).toBe(false);
  });

  it("keeps a stored fold of an unknown window before the prefs are loaded", () => {
    localStorage.setItem(KEY, JSON.stringify(["window:gone"]));
    const { result } = renderHook(() => useRunwayFolds());
    act(() => result.current.toggle(col("today")));
    expect(stored()).toEqual(["window:gone", "today"]);
  });

  it("drops a deleted window's fold once the prefs are loaded, and keeps a live one's", () => {
    prefs.hydrated = true;
    prefs.windows = [{ id: "w1", label: "Lisbon", start: "2026-11-07", end: "2026-11-07" }];
    localStorage.setItem(KEY, JSON.stringify(["window:gone", "window:w1"]));
    const { result } = renderHook(() => useRunwayFolds());
    act(() => result.current.toggle(col("today")));
    expect(stored()).toEqual(["window:w1", "today"]);
  });
});
