// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// useRunwayFolds, below the board: what a click writes when this window's copy of the folds is
// stale (the other webview folded since, and no storage event crossed), and when a deleted named
// window's fold is dropped (only once the stored prefs are loaded, or every window would look dead).

import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { RunwayWindow } from "../../../lib/calendarPrefs";
import { buildColumns, type RunwayCard, type RunwayColumn } from "../../../lib/runway";
import type { Task } from "../../../lib/types";

const prefs = vi.hoisted(() => ({ hydrated: false, windows: [] as RunwayWindow[] }));
vi.mock("../../../lib/ipc", () => ({}));
vi.mock("../../../lib/storedPrefs", () => ({ storedPrefsHydrated: () => prefs.hydrated }));
vi.mock("../../../lib/calendarPrefs", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  readRunwayWindows: () => prefs.windows,
}));

import { moveChoices, runMoveChoice, useRunwayFolds } from "./runwayLogic";

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

// The Move… popover and the card's right-click menu draw one list, so what a card can do is decided
// in one place. Below it, the list itself: its order, what is disabled, and what choosing does.
describe("moveChoices", () => {
  const task = (over: Partial<Task> = {}): Task =>
    ({
      id: 1,
      lane: "foreground",
      parked: false,
      do_start: "2026-11-04",
      do_end: null,
      ...over,
    }) as Task;
  const columns = buildColumns(new Date(2026, 10, 4), []);
  const lane: RunwayCard[] = [1, 2].map(
    (id) => ({ kind: "task", task: task({ id }) }) as unknown as RunwayCard,
  );

  it("lists up and down, each column, the other lane, then parking, the current column marked", () => {
    const list = moveChoices({
      tasks: [task({ id: 1 })],
      columns,
      here: "today",
      laneCards: lane,
      canReorder: true,
    });
    expect(list.map((c) => c.key)).toEqual([
      "up",
      "down",
      ...columns.filter((c) => c.kind !== "past").map((c) => `column:${c.id}`),
      "lane",
      "park",
    ]);
    expect(list.find((c) => c.key === "up")?.disabled).toBe(true);
    expect(list.find((c) => c.key === "down")?.disabled).toBe(false);
    const here = list.find((c) => c.key === "column:today");
    expect(here).toMatchObject({ current: true, disabled: true });
    expect(list.find((c) => c.key === "lane")?.label).toBe("Background lane");
    expect(list.find((c) => c.key === "park")?.label).toBe("Park it");
  });

  it("has no up or down without a lane to step in, and offers Bring back for a parked task", () => {
    const list = moveChoices({
      tasks: [task({ parked: true })],
      columns,
      here: null,
      canReorder: true,
    });
    expect(list.some((c) => c.part === "step")).toBe(false);
    expect(list.find((c) => c.key === "park")?.label).toBe("Bring back from Parked");
    // Parked, no column is "the one it is in".
    expect(list.some((c) => c.current)).toBe(false);
  });

  it("runs what each choice means: a plan for a column, an order for a step", () => {
    const list = moveChoices({
      tasks: [task({ id: 2 })],
      columns,
      here: "today",
      laneCards: lane,
      canReorder: true,
    });
    const replan = vi.fn();
    const reorder = vi.fn();
    runMoveChoice(
      list.find((c) => c.key === "column:tomorrow")!,
      replan,
      reorder,
    );
    expect(replan).toHaveBeenCalledWith(expect.any(Function), "Tomorrow");
    runMoveChoice(
      list.find((c) => c.key === "up")!,
      replan,
      reorder,
    );
    expect(reorder).toHaveBeenCalledWith([2, 1], "Moved up in Today");
    // A step with nowhere to go does nothing.
    runMoveChoice(
      list.find((c) => c.key === "down")!,
      replan,
      reorder,
    );
    expect(reorder).toHaveBeenCalledTimes(1);
  });
});
