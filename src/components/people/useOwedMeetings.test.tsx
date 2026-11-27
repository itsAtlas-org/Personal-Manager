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
  listCalendarEvents: vi.fn(),
  peopleListeners: [] as (() => void)[],
  syncListeners: [] as (() => void)[],
}));
vi.mock("../../lib/ipc", () => ({
  owedMeetings: ipc.owedMeetings,
  listCalendarEvents: ipc.listCalendarEvents,
  onPeopleChanged: vi.fn(async (h: () => void) => {
    ipc.peopleListeners.push(h);
    return () => {};
  }),
  onCalendarSynced: vi.fn(async (h: () => void) => {
    ipc.syncListeners.push(h);
    return () => {};
  }),
}));

import { END_WATCH_MAX_MS, useOwedMeetings } from "./useOwedMeetings";
import type { AgendaEvent } from "../../lib/types";

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
  // Morning, local time: "today's" end-of-day bound then doesn't depend on when the suite runs.
  vi.setSystemTime(new Date(2026, 10, 25, 8, 0, 0));
  ipc.owedMeetings.mockReset();
  ipc.listCalendarEvents.mockReset();
  ipc.listCalendarEvents.mockResolvedValue([]);
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

const MINUTE = 60 * 1000;

/** An agenda row ending `minutes` from the pinned clock. */
function ending(minutes: number, over: Partial<AgendaEvent> = {}): AgendaEvent {
  const end = new Date(Date.now() + minutes * MINUTE).toISOString();
  return {
    id: `e${minutes}`,
    calendar_id: "cal",
    summary: "Meeting",
    description: null,
    location: null,
    start: new Date(Date.now() + (minutes - 30) * MINUTE).toISOString(),
    end,
    all_day: false,
    html_link: null,
    uid: `u${minutes}`,
    ended: false,
    declined: false,
    ...over,
  };
}

describe("a meeting that ends while the tab stays open", () => {
  it("reads again once, when the soonest of today's meetings is over", async () => {
    ipc.owedMeetings.mockResolvedValueOnce([]).mockResolvedValue([meeting("late")]);
    ipc.listCalendarEvents.mockResolvedValue([ending(20), ending(10), ending(25)]);
    const { result } = renderHook(() => useOwedMeetings(true));
    await flush();
    expect(ipc.owedMeetings).toHaveBeenCalledTimes(1);
    // Nothing fires before it ends (no polling).
    await act(async () => {
      vi.advanceTimersByTime(9 * MINUTE);
    });
    expect(ipc.owedMeetings).toHaveBeenCalledTimes(1);
    // The one that ends first: a read, and the strip has it.
    ipc.listCalendarEvents.mockResolvedValue([]);
    await act(async () => {
      vi.advanceTimersByTime(MINUTE + 2000);
    });
    expect(ipc.owedMeetings).toHaveBeenCalledTimes(2);
    expect(result.current.meetings.map((m) => m.uid)).toEqual(["late"]);
  });

  it("waits for the next end after each read, and is capped when the end is far off", async () => {
    ipc.owedMeetings.mockResolvedValue([]);
    ipc.listCalendarEvents.mockResolvedValue([ending(300)]);
    renderHook(() => useOwedMeetings(true));
    await flush();
    expect(ipc.owedMeetings).toHaveBeenCalledTimes(1);
    await act(async () => {
      vi.advanceTimersByTime(END_WATCH_MAX_MS - 1000);
    });
    expect(ipc.owedMeetings).toHaveBeenCalledTimes(1);
    await act(async () => {
      vi.advanceTimersByTime(2000);
    });
    expect(ipc.owedMeetings).toHaveBeenCalledTimes(2);
    // Never a tight loop: one more only after another wait.
    await act(async () => {
      vi.advanceTimersByTime(MINUTE);
    });
    expect(ipc.owedMeetings).toHaveBeenCalledTimes(2);
  });

  it("sets no timer when nothing is left to end, all-day events and ended ones aside", async () => {
    ipc.owedMeetings.mockResolvedValue([]);
    ipc.listCalendarEvents.mockResolvedValue([
      ending(5, { all_day: true }),
      ending(6, { ended: true }),
      ending(-4),
      ending(7, { end: null }),
    ]);
    renderHook(() => useOwedMeetings(true));
    await flush();
    await act(async () => {
      vi.advanceTimersByTime(2 * END_WATCH_MAX_MS);
    });
    expect(ipc.owedMeetings).toHaveBeenCalledTimes(1);
  });

  it("sets no timer for timed events that end on a later day", async () => {
    ipc.owedMeetings.mockResolvedValue([]);
    // 8:00 now: 17 h is tomorrow morning; 3 and 10 days out are the rest of the agenda window.
    ipc.listCalendarEvents.mockResolvedValue([
      ending(17 * 60),
      ending(3 * 24 * 60),
      ending(10 * 24 * 60),
    ]);
    renderHook(() => useOwedMeetings(true));
    await flush();
    await act(async () => {
      vi.advanceTimersByTime(4 * END_WATCH_MAX_MS);
    });
    expect(ipc.owedMeetings).toHaveBeenCalledTimes(1);
  });

  it("sets no timer when the calendar read lands after the strip is gone or the store closed", async () => {
    ipc.owedMeetings.mockResolvedValue([]);
    let land: (events: AgendaEvent[]) => void = () => {};
    ipc.listCalendarEvents.mockImplementation(() => new Promise((r) => (land = r)));
    const { unmount } = renderHook(() => useOwedMeetings(true));
    await flush();
    unmount();
    await act(async () => land([ending(10)]));
    await act(async () => {
      vi.advanceTimersByTime(60 * MINUTE);
    });
    expect(ipc.owedMeetings).toHaveBeenCalledTimes(1);

    ipc.owedMeetings.mockClear();
    const { rerender } = renderHook(({ on }) => useOwedMeetings(on), {
      initialProps: { on: true },
    });
    await flush();
    rerender({ on: false });
    await act(async () => land([ending(10)]));
    await act(async () => {
      vi.advanceTimersByTime(60 * MINUTE);
    });
    expect(ipc.owedMeetings).toHaveBeenCalledTimes(1);
  });

  it("is cleared when the strip goes, and a failed calendar read sets nothing", async () => {
    ipc.owedMeetings.mockResolvedValue([]);
    ipc.listCalendarEvents.mockResolvedValue([ending(10)]);
    const { unmount } = renderHook(() => useOwedMeetings(true));
    await flush();
    unmount();
    await act(async () => {
      vi.advanceTimersByTime(60 * MINUTE);
    });
    expect(ipc.owedMeetings).toHaveBeenCalledTimes(1);

    ipc.owedMeetings.mockClear();
    ipc.listCalendarEvents.mockRejectedValue("nope");
    renderHook(() => useOwedMeetings(true));
    await flush();
    await act(async () => {
      vi.advanceTimersByTime(60 * MINUTE);
    });
    expect(ipc.owedMeetings).toHaveBeenCalledTimes(1);
  });

  it("stops watching when the store closes", async () => {
    ipc.owedMeetings.mockResolvedValue([]);
    ipc.listCalendarEvents.mockResolvedValue([ending(10)]);
    const { rerender } = renderHook(({ on }) => useOwedMeetings(on), {
      initialProps: { on: true },
    });
    await flush();
    rerender({ on: false });
    await act(async () => {
      vi.advanceTimersByTime(60 * MINUTE);
    });
    expect(ipc.owedMeetings).toHaveBeenCalledTimes(1);
  });
});

describe("App's strip", () => {
  it("lets go of kept card input when the store closes", () => {
    const at = appSource.indexOf("forgetAllDrafts()");
    expect(at).toBeGreaterThan(-1);
    expect(appSource.slice(appSource.lastIndexOf("\n", at - 60), at)).toContain("!storeOpen");
  });

  it("is drawn only outside Settings", () => {
    const at = appSource.indexOf("<MeetStrip");
    expect(at).toBeGreaterThan(-1);
    expect(appSource.slice(appSource.lastIndexOf("\n", at - 80), at)).toContain("!showSettings");
  });
});
