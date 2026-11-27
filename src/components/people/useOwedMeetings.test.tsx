// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// How the app keeps its list of owed meetings fresh (Contacts, slice 7b): read on mount and when
// the watched value moves, one debounced read per notice (a sync and its people notice are one), a
// late older reply dropped, a failed read keeping the list. App has no test of its own, so a source
// scan holds the strip out of Settings.

import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import appSource from "../../App.tsx?raw";
import type { OwedMeeting } from "../../lib/types";

const ipc = vi.hoisted(() => ({
  owedMeetings: vi.fn(),
  peopleListeners: [] as (() => void)[],
  syncListeners: [] as (() => void)[],
}));
vi.mock("../../lib/ipc", () => ({
  owedMeetings: ipc.owedMeetings,
  onPeopleChanged: vi.fn(async (h: () => void) => {
    ipc.peopleListeners.push(h);
    return () => {};
  }),
  onCalendarSynced: vi.fn(async (h: () => void) => {
    ipc.syncListeners.push(h);
    return () => {};
  }),
}));

import { useOwedMeetings } from "./useOwedMeetings";

const meeting = (uid: string): OwedMeeting => ({
  uid,
  start: "2026-11-25T09:00:00Z",
  end: "2026-11-25T09:30:00Z",
  title: uid,
  all_day: false,
  recurs: false,
  known: [],
  unknown: [],
});

const flush = () => act(async () => {});

beforeEach(() => {
  vi.useFakeTimers();
  ipc.owedMeetings.mockReset();
  ipc.peopleListeners.length = 0;
  ipc.syncListeners.length = 0;
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("useOwedMeetings", () => {
  it("reads on mount and again when the watched value moves, and never while disabled", async () => {
    ipc.owedMeetings.mockResolvedValue([meeting("a")]);
    const { result, rerender } = renderHook(({ w }) => useOwedMeetings(true, w), {
      initialProps: { w: "calendar" },
    });
    await flush();
    expect(ipc.owedMeetings).toHaveBeenCalledTimes(1);
    expect(result.current.meetings.map((m) => m.uid)).toEqual(["a"]);
    rerender({ w: "people" });
    await flush();
    expect(ipc.owedMeetings).toHaveBeenCalledTimes(2);

    ipc.owedMeetings.mockClear();
    renderHook(() => useOwedMeetings(false));
    await flush();
    expect(ipc.owedMeetings).not.toHaveBeenCalled();
  });

  it("reads once for a sync and its people notice together", async () => {
    ipc.owedMeetings.mockResolvedValue([]);
    renderHook(() => useOwedMeetings(true));
    await flush();
    ipc.owedMeetings.mockClear();
    ipc.syncListeners.forEach((f) => f());
    ipc.peopleListeners.forEach((f) => f());
    await act(async () => {
      vi.advanceTimersByTime(1000);
    });
    expect(ipc.owedMeetings).toHaveBeenCalledTimes(1);
  });

  it("drops a reply that lands after a newer read was sent, and keeps the list on a failure", async () => {
    let first: (list: OwedMeeting[]) => void = () => {};
    ipc.owedMeetings.mockImplementationOnce(() => new Promise((r) => (first = r)));
    ipc.owedMeetings.mockResolvedValueOnce([meeting("new")]);
    const { result, rerender } = renderHook(({ w }) => useOwedMeetings(true, w), {
      initialProps: { w: 1 },
    });
    rerender({ w: 2 });
    await flush();
    expect(result.current.meetings.map((m) => m.uid)).toEqual(["new"]);
    await act(async () => first([meeting("old")]));
    expect(result.current.meetings.map((m) => m.uid)).toEqual(["new"]);

    ipc.owedMeetings.mockRejectedValueOnce("nope");
    rerender({ w: 3 });
    await flush();
    expect(result.current.meetings.map((m) => m.uid)).toEqual(["new"]);
  });
});

describe("App's strip", () => {
  it("is drawn only outside Settings", () => {
    const at = appSource.indexOf("<MeetStrip");
    expect(at).toBeGreaterThan(-1);
    expect(appSource.slice(appSource.lastIndexOf("\n", at - 80), at)).toContain("!showSettings");
  });
});
