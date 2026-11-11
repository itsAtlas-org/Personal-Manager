// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The ~30 s Google change check (#884, F3), now a hook so the pop-out Calendar runs it too. The loop
// lived inline in App.tsx with no test; these pin what it promises: it asks about every 30 s while the
// window is on screen, pauses while hidden, checks at once on coming back, and stops on unmount, on
// being disabled, and when the app's erase starts.

import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CHECK_EVERY_MS } from "./calendarPoll";

const ipc = vi.hoisted(() => ({
  checkGoogleCalendars: vi.fn(() => Promise.resolve({})),
}));
vi.mock("./ipc", () => ipc);

import { useCalendarChangeCheck } from "./useCalendarChangeCheck";

let visibility: DocumentVisibilityState = "visible";

beforeEach(() => {
  vi.useFakeTimers();
  // The middle of the spread, so a tick is always inside [0.75, 1.25] × 30 s.
  vi.spyOn(Math, "random").mockReturnValue(0.5);
  visibility = "visible";
  Object.defineProperty(document, "visibilityState", { configurable: true, get: () => visibility });
  ipc.checkGoogleCalendars.mockClear();
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const advance = (ms: number) => act(() => void vi.advanceTimersByTime(ms));

describe("useCalendarChangeCheck", () => {
  it("asks on the schedule nextCheckDelay gives, over and over", () => {
    renderHook(() => useCalendarChangeCheck(true));
    advance(CHECK_EVERY_MS - 1);
    expect(ipc.checkGoogleCalendars).not.toHaveBeenCalled();
    advance(1);
    expect(ipc.checkGoogleCalendars).toHaveBeenCalledTimes(1);
    advance(CHECK_EVERY_MS);
    expect(ipc.checkGoogleCalendars).toHaveBeenCalledTimes(2);
  });

  it("does nothing while disabled, and starts when enabled", () => {
    const { rerender } = renderHook(({ on }) => useCalendarChangeCheck(on), {
      initialProps: { on: false },
    });
    advance(CHECK_EVERY_MS * 3);
    expect(ipc.checkGoogleCalendars).not.toHaveBeenCalled();
    rerender({ on: true });
    advance(CHECK_EVERY_MS);
    expect(ipc.checkGoogleCalendars).toHaveBeenCalledTimes(1);
  });

  it("skips a check while the window is hidden, and keeps its schedule", () => {
    renderHook(() => useCalendarChangeCheck(true));
    visibility = "hidden";
    advance(CHECK_EVERY_MS * 2);
    expect(ipc.checkGoogleCalendars).not.toHaveBeenCalled();
    visibility = "visible";
    advance(CHECK_EVERY_MS);
    expect(ipc.checkGoogleCalendars).toHaveBeenCalledTimes(1);
  });

  it("checks at once on becoming visible, and on focus", () => {
    renderHook(() => useCalendarChangeCheck(true));
    visibility = "hidden";
    act(() => void document.dispatchEvent(new Event("visibilitychange")));
    expect(ipc.checkGoogleCalendars).not.toHaveBeenCalled();
    visibility = "visible";
    act(() => void document.dispatchEvent(new Event("visibilitychange")));
    expect(ipc.checkGoogleCalendars).toHaveBeenCalledTimes(1);
    act(() => void window.dispatchEvent(new Event("focus")));
    expect(ipc.checkGoogleCalendars).toHaveBeenCalledTimes(2);
  });

  it("swallows a check that fails", async () => {
    ipc.checkGoogleCalendars.mockRejectedValueOnce(new Error("offline"));
    renderHook(() => useCalendarChangeCheck(true));
    advance(CHECK_EVERY_MS);
    await act(async () => {});
    expect(ipc.checkGoogleCalendars).toHaveBeenCalledTimes(1);
    advance(CHECK_EVERY_MS);
    expect(ipc.checkGoogleCalendars).toHaveBeenCalledTimes(2);
  });

  it("stops on unmount", () => {
    const { unmount } = renderHook(() => useCalendarChangeCheck(true));
    unmount();
    advance(CHECK_EVERY_MS * 3);
    act(() => void window.dispatchEvent(new Event("focus")));
    expect(ipc.checkGoogleCalendars).not.toHaveBeenCalled();
  });

  it("stops when the app's erase begins (onTeardown), and gives the subscription back on unmount", () => {
    let stop: (() => void) | undefined;
    const off = vi.fn();
    const onTeardown = vi.fn((s: () => void) => {
      stop = s;
      return off;
    });
    const { unmount } = renderHook(() => useCalendarChangeCheck(true, onTeardown));
    act(() => stop?.());
    advance(CHECK_EVERY_MS * 3);
    expect(ipc.checkGoogleCalendars).not.toHaveBeenCalled();
    unmount();
    expect(off).toHaveBeenCalledOnce();
  });
});
