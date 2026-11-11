// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom

import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useSettledDate } from "./useSettledDate";

const month = (m: number) => new Date(2026, m, 1);

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

function setup(initial: Date) {
  return renderHook(({ date }) => useSettledDate(date, 250), { initialProps: { date: initial } });
}

describe("useSettledDate", () => {
  it("starts at the date it is given, and waits for nothing", () => {
    const { result } = setup(month(0));
    expect(result.current.getTime()).toBe(month(0).getTime());
    act(() => void vi.advanceTimersByTime(1000));
    expect(result.current.getTime()).toBe(month(0).getTime());
  });

  it("passes a single move at once", () => {
    const { result, rerender } = setup(month(0));
    rerender({ date: month(5) });
    expect(result.current.getTime()).toBe(month(5).getTime());
  });

  it("passes the first move of a burst at once and the last 250 ms after it", () => {
    const { result, rerender } = setup(month(0));
    rerender({ date: month(1) });
    expect(result.current.getTime()).toBe(month(1).getTime());
    act(() => void vi.advanceTimersByTime(100));
    rerender({ date: month(2) });
    act(() => void vi.advanceTimersByTime(100));
    rerender({ date: month(3) });
    // The moves in between didn't pass.
    expect(result.current.getTime()).toBe(month(1).getTime());
    act(() => void vi.advanceTimersByTime(249));
    expect(result.current.getTime()).toBe(month(1).getTime());
    act(() => void vi.advanceTimersByTime(1));
    expect(result.current.getTime()).toBe(month(3).getTime());
  });

  it("treats a move after the burst has settled as the start of a new one", () => {
    const { result, rerender } = setup(month(0));
    rerender({ date: month(1) });
    act(() => void vi.advanceTimersByTime(300));
    rerender({ date: month(8) });
    expect(result.current.getTime()).toBe(month(8).getTime());
  });

  it("keeps the same object when the burst ends where it began passing", () => {
    const { result, rerender } = setup(month(0));
    rerender({ date: month(1) });
    const first = result.current;
    act(() => void vi.advanceTimersByTime(100));
    rerender({ date: month(2) });
    act(() => void vi.advanceTimersByTime(100));
    rerender({ date: month(1) });
    act(() => void vi.advanceTimersByTime(300));
    // Back on the day it last passed: nothing keyed on it runs again.
    expect(result.current).toBe(first);
  });

  it("is not moved by a new object for the same instant", () => {
    const { result, rerender } = setup(month(0));
    const first = result.current;
    rerender({ date: new Date(month(0).getTime()) });
    act(() => void vi.advanceTimersByTime(1000));
    expect(result.current).toBe(first);
  });

  it("drops its timer on unmount", () => {
    const { rerender, unmount } = setup(month(0));
    rerender({ date: month(1) });
    unmount();
    expect(vi.getTimerCount()).toBe(0);
  });
});
