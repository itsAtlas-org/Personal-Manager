// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The Year view is a stream of mini-months, 25 years each side of an anchor, only the rows near the
// viewport drawn, so scrolling never reaches an edge (the wall was two years out), and a jump past
// the range re-anchors. jsdom lays out nothing, so the pane is given a size and its scrollTop is set
// by hand.

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { YearView } from "./YearView";

let restore: (() => void) | null = null;
beforeAll(() => {
  if (!("ResizeObserver" in globalThis)) {
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
  const h = Object.getOwnPropertyDescriptor(Element.prototype, "clientHeight");
  const w = Object.getOwnPropertyDescriptor(Element.prototype, "clientWidth");
  const scrollTo = Object.getOwnPropertyDescriptor(Element.prototype, "scrollTo");
  // 700 px wide is two columns; 600 tall.
  Object.defineProperty(Element.prototype, "clientHeight", { configurable: true, get: () => 600 });
  Object.defineProperty(Element.prototype, "clientWidth", { configurable: true, get: () => 700 });
  Object.defineProperty(Element.prototype, "scrollTo", { configurable: true, value: () => {} });
  restore = () => {
    for (const [name, d] of [
      ["clientHeight", h],
      ["clientWidth", w],
      ["scrollTo", scrollTo],
    ] as const) {
      if (d) Object.defineProperty(Element.prototype, name, d);
      else delete (Element.prototype as unknown as Record<string, unknown>)[name];
    }
  };
});
afterAll(() => restore?.());

beforeEach(() => {
  vi.useFakeTimers({
    toFake: ["requestAnimationFrame", "cancelAnimationFrame", "setTimeout", "clearTimeout"],
  });
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

// Two columns of 40 px cells: a month is 26 + 16 + 6 × 40 + 4 = 286 px, and a row 302 with its gap.
const ROW = 302;
const COLS = 2;
// Anchor October 2026: 300 months before it is the first of the stream, so October is month 300.
const OCT = 300;

function mountAt(cursor: Date, onFocusDate: (d: Date) => void = () => {}) {
  const view = (c: Date) => (
    <YearView cursor={c} events={[]} onSelectDay={() => {}} onFocusDate={onFocusDate} />
  );
  const utils = render(view(cursor));
  return { rerender: (c: Date) => utils.rerender(view(c)) };
}
const stream = () => document.querySelector<HTMLElement>(".overflow-y-auto")!;
const settle = (ms: number) => act(() => void vi.advanceTimersByTime(ms));
/** The year labels of the drawn months (each mini-month's title carries its year). */
const years = () =>
  [...document.querySelectorAll<HTMLElement>("span.font-mono")]
    .map((el) => el.textContent ?? "")
    .filter((t) => /^\d{4}$/.test(t));
const drawn = () => stream().firstElementChild!.children.length;

describe("YearView: the stream has no wall", () => {
  it("draws the cursor's month and the rows around it, not the whole stream", () => {
    mountAt(new Date(2026, 9, 1));
    expect(years()).toContain("2026");
    // 601 months over two columns, but only the rows near the viewport: 2 in view, 6 either side.
    expect(drawn()).toBeGreaterThan(2);
    expect(drawn()).toBeLessThanOrEqual((2 + 2 * 6) * COLS + COLS);
    expect(years()).not.toContain("2001");
  });

  it("sizes the stream for every row, and places each month at its own row and column", () => {
    mountAt(new Date(2026, 9, 1));
    const inner = stream().firstElementChild as HTMLElement;
    const rows = Math.ceil(601 / COLS);
    expect(inner.style.height).toBe(`${rows * ROW - 16}px`);
    const tops = [...inner.children].map((el) => parseInt((el as HTMLElement).style.top, 10));
    expect(tops.every((t) => t % ROW === 0)).toBe(true);
    // October (month 300) is in row 150, as the first column's month.
    expect(tops).toContain(150 * ROW);
  });

  it("scrolls past the old ±24-month edge: those months are drawn, and their year is reported", () => {
    const onFocusDate = vi.fn();
    mountAt(new Date(2026, 9, 1), onFocusDate);
    // The opening scroll is PM's own, and mutes the report until it has ended (700 ms at most).
    settle(700);
    // Five years on: past where the old stream ended (two years).
    const row = (OCT + 60) / COLS;
    stream().scrollTop = row * ROW;
    fireEvent.scroll(stream());
    settle(20);
    expect(years()).toContain("2031");
    expect(years()).not.toContain("2026");
    expect(onFocusDate).toHaveBeenCalledTimes(1);
    expect((onFocusDate.mock.calls[0][0] as Date).getFullYear()).toBe(2031);
  });

  it("draws the stream's far ends", () => {
    mountAt(new Date(2026, 9, 1));
    stream().scrollTop = 0;
    fireEvent.scroll(stream());
    settle(20);
    // 300 months before October 2026 is October 2001.
    expect(years()).toContain("2001");
    const rows = Math.ceil(601 / COLS);
    stream().scrollTop = rows * ROW;
    fireEvent.scroll(stream());
    settle(20);
    // 300 months after: October 2051.
    expect(years()).toContain("2051");
  });

  it("re-anchors for a jump past the range, and draws the cursor's month", () => {
    const { rerender } = mountAt(new Date(2026, 9, 1));
    expect(years()).not.toContain("2090");
    rerender(new Date(2090, 4, 1));
    expect(years()).toContain("2090");
    expect(screen.getAllByText("May").length).toBeGreaterThan(0);
  });

  it("does not re-anchor for a jump inside the range", () => {
    const { rerender } = mountAt(new Date(2026, 9, 1));
    rerender(new Date(2040, 4, 1));
    expect(years()).toContain("2040");
    // Still the same stream: 25 years back of the old anchor is drawn when scrolled to.
    stream().scrollTop = 0;
    fireEvent.scroll(stream());
    settle(20);
    expect(years()).toContain("2001");
  });

  // The same few pixels of row height, or a column more or less, would otherwise slide a 25-year
  // stream by years: the month at the top-left stays at the top.
  it("keeps the month at the top when the columns change count", () => {
    const callbacks: ResizeObserverCallback[] = [];
    const had = (globalThis as { ResizeObserver?: unknown }).ResizeObserver;
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      constructor(cb: ResizeObserverCallback) {
        callbacks.push(cb);
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    };
    try {
      mountAt(new Date(2026, 9, 1));
      settle(700); // the opening scroll is PM's own, and isn't the user's until it has ended
      // 80 rows of two columns down: month 160 of the stream at the top-left.
      stream().scrollTop = 80 * ROW;
      fireEvent.scroll(stream());
      // The pane narrows to one column (400 px wide), cells a little smaller.
      act(() => {
        for (const cb of callbacks) {
          cb([{ contentRect: { width: 400, height: 600 } } as ResizeObserverEntry], {} as never);
        }
      });
      // One column of 400 px: cells of floor((400 - 32) / 7.4) = 49, capped at 40: the same
      // 302 px rows. Month 160 (row 80 × 2 columns) is now in row 160.
      expect(stream().scrollTop).toBe(160 * ROW);
    } finally {
      (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = had;
    }
  });

  // One year is 6 rows at two columns (3 at four): a single ‹ › ↓ step glides at any width; a held key
  // that is years ahead jumps rather than chase a glide.
  it("glides one year at two columns, and jumps when the cursor is far ahead", () => {
    const kept = Object.getOwnPropertyDescriptor(Element.prototype, "scrollTo");
    const scrollTo = vi.fn();
    Object.defineProperty(Element.prototype, "scrollTo", { configurable: true, value: scrollTo });
    try {
      const { rerender } = mountAt(new Date(2026, 9, 1));
      expect(scrollTo).toHaveBeenLastCalledWith({ top: (OCT / COLS) * ROW, behavior: "auto" });
      stream().scrollTop = (OCT / COLS) * ROW;
      rerender(new Date(2027, 9, 1));
      expect(scrollTo).toHaveBeenLastCalledWith({
        top: ((OCT + 12) / COLS) * ROW,
        behavior: "smooth",
      });
      stream().scrollTop = ((OCT + 12) / COLS) * ROW;
      rerender(new Date(2032, 9, 1)); // five years on
      expect(scrollTo.mock.lastCall![0].behavior).toBe("auto");
    } finally {
      if (kept) Object.defineProperty(Element.prototype, "scrollTo", kept);
    }
  });

  it("keeps a glide's target month at the top when the columns change mid-glide", () => {
    const callbacks: ResizeObserverCallback[] = [];
    const had = (globalThis as { ResizeObserver?: unknown }).ResizeObserver;
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      constructor(cb: ResizeObserverCallback) {
        callbacks.push(cb);
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    };
    const kept = Object.getOwnPropertyDescriptor(Element.prototype, "scrollTo");
    Object.defineProperty(Element.prototype, "scrollTo", { configurable: true, value: vi.fn() });
    try {
      const { rerender } = mountAt(new Date(2026, 9, 1));
      settle(700);
      stream().scrollTop = (OCT / COLS) * ROW;
      rerender(new Date(2027, 9, 1)); // a glide a year on: month OCT + 12
      stream().scrollTop = ((OCT + 6) / COLS) * ROW; // part-way
      fireEvent.scroll(stream());
      act(() => {
        for (const cb of callbacks) {
          cb([{ contentRect: { width: 400, height: 600 } } as ResizeObserverEntry], {} as never);
        }
      });
      // One column now: the target month is in row OCT + 12.
      expect(stream().scrollTop).toBe((OCT + 12) * ROW);
    } finally {
      if (kept) Object.defineProperty(Element.prototype, "scrollTo", kept);
      (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = had;
    }
  });
});
