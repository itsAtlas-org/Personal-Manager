// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import {
  coverSlices,
  focusRow,
  glideOrJump,
  nextSlice,
  rowAtTop,
  sliceRows,
  type RowSlice,
} from "./streamWindow";

// A pane 600 px tall of 100 px rows over a stream of 1,000 rows, unless a test says otherwise.
const H = 600;
const STRIDE = 100;
const ROWS = 1000;

describe("sliceRows", () => {
  it("draws the rows in view and the overscan each side", () => {
    // Rows 50..55 in view (scrollTop 5,000, six rows tall), 12 each side.
    expect(sliceRows(5000, H, STRIDE, ROWS)).toEqual({ first: 38, last: 68 });
  });

  it("counts a half-scrolled row as in view at both ends", () => {
    // Top row 50 half out, bottom row 56 half in: rows 50..56 are shown.
    expect(sliceRows(5050, H, STRIDE, ROWS, 0)).toEqual({ first: 50, last: 57 });
  });

  it("clamps at the top of the stream", () => {
    expect(sliceRows(0, H, STRIDE, ROWS)).toEqual({ first: 0, last: 18 });
    expect(sliceRows(500, H, STRIDE, ROWS)).toEqual({ first: 0, last: 23 });
  });

  it("clamps at the bottom of the stream", () => {
    const max = (ROWS - 6) * STRIDE;
    expect(sliceRows(max, H, STRIDE, ROWS)).toEqual({ first: 982, last: ROWS });
    // Past the end, as a stale offset after a shorter stream would be: still the last rows.
    expect(sliceRows(ROWS * STRIDE * 2, H, STRIDE, ROWS)).toEqual({ first: 987, last: ROWS });
  });

  it("draws nothing without rows or a measured stride", () => {
    expect(sliceRows(0, H, 0, ROWS)).toEqual({ first: 0, last: 0 });
    expect(sliceRows(0, H, STRIDE, 0)).toEqual({ first: 0, last: 0 });
  });

  it("takes its own overscan", () => {
    expect(sliceRows(5000, H, STRIDE, ROWS, 2)).toEqual({ first: 48, last: 58 });
  });
});

describe("nextSlice", () => {
  const at = (first: number, last: number): RowSlice => ({ first, last });

  it("keeps the same slice until it has drifted a step", () => {
    const current = at(38, 68);
    expect(nextSlice(current, at(39, 69), ROWS)).toBe(current);
    expect(nextSlice(current, at(41, 71), ROWS)).toBe(current);
  });

  it("moves once either end is a step off", () => {
    const current = at(38, 68);
    const ideal = at(42, 72);
    expect(nextSlice(current, ideal, ROWS)).toBe(ideal);
    // Only the bottom: a taller pane after a resize.
    const taller = at(38, 72);
    expect(nextSlice(current, taller, ROWS)).toBe(taller);
  });

  it("takes the top and bottom of the stream at once, however small the drift", () => {
    // A fling back to the top between two frames: row 1 is in view, and must be drawn.
    const top = at(0, 19);
    expect(nextSlice(at(2, 22), top, ROWS)).toBe(top);
    const bottom = at(ROWS - 20, ROWS);
    expect(nextSlice(at(ROWS - 22, ROWS - 2), bottom, ROWS)).toBe(bottom);
  });

  it("holds when it already reaches the end", () => {
    const current = at(0, 18);
    expect(nextSlice(current, at(0, 19), ROWS)).toBe(current);
  });
});

describe("coverSlices", () => {
  it("is the smallest slice holding both", () => {
    expect(coverSlices({ first: 100, last: 130 }, { first: 110, last: 140 })).toEqual({
      first: 100,
      last: 140,
    });
    expect(coverSlices({ first: 110, last: 140 }, { first: 100, last: 130 })).toEqual({
      first: 100,
      last: 140,
    });
  });
});

describe("focusRow", () => {
  it("is the row 40% of the way down the pane", () => {
    // scrollTop 5,000 + 240 px = row 52.4, which rounds to 52.
    expect(focusRow(5000, H, STRIDE, ROWS)).toBe(52);
    // Half a row further on, the next one is in focus.
    expect(focusRow(5005, H, STRIDE, ROWS)).toBe(52);
    expect(focusRow(5060, H, STRIDE, ROWS)).toBe(53);
  });

  it("clamps at the top, the middle's neighbours and the end of the stream", () => {
    expect(focusRow(0, H, STRIDE, ROWS)).toBe(2);
    expect(focusRow(-50, 0, STRIDE, ROWS)).toBe(0);
    expect(focusRow((ROWS - 6) * STRIDE, H, STRIDE, ROWS)).toBe(ROWS - 4);
    expect(focusRow(ROWS * STRIDE * 2, H, STRIDE, ROWS)).toBe(ROWS - 1);
  });

  it("is row 0 for an empty or unmeasured stream", () => {
    expect(focusRow(300, H, 0, ROWS)).toBe(0);
    expect(focusRow(300, H, STRIDE, 0)).toBe(0);
  });
});

describe("rowAtTop", () => {
  it("is scrollTop in rows, fractionally", () => {
    expect(rowAtTop(5000, STRIDE)).toBe(50);
    expect(rowAtTop(5050, STRIDE)).toBe(50.5);
    expect(rowAtTop(0, STRIDE)).toBe(0);
  });

  it("is 0 without a stride", () => {
    expect(rowAtTop(5000, 0)).toBe(0);
  });

  // The re-height that keeps the week at the top: the row at the top, put back at the new stride.
  it("round-trips a half-scrolled row through a new row height", () => {
    const top = rowAtTop(5050, 100);
    expect(top * 90).toBe(4545);
  });
});

describe("glideOrJump", () => {
  it("glides within the limit, either way", () => {
    expect(glideOrJump(100, 105, 13)).toBe("smooth");
    expect(glideOrJump(105, 100, 13)).toBe("smooth");
    expect(glideOrJump(100, 113, 13)).toBe("smooth");
    expect(glideOrJump(113, 100, 13)).toBe("smooth");
    expect(glideOrJump(100, 100, 13)).toBe("smooth");
  });

  it("jumps beyond the limit, either way", () => {
    expect(glideOrJump(100, 114, 13)).toBe("auto");
    expect(glideOrJump(114, 100, 13)).toBe("auto");
    expect(glideOrJump(0, 500, 13)).toBe("auto");
    expect(glideOrJump(500, 0, 3)).toBe("auto");
  });

  it("takes its limit per view: three rows in Year", () => {
    expect(glideOrJump(10, 13, 3)).toBe("smooth");
    expect(glideOrJump(10, 14, 3)).toBe("auto");
  });

  it("reads a fractional top row", () => {
    expect(glideOrJump(100.5, 113.4, 13)).toBe("smooth");
    expect(glideOrJump(100.5, 113.6, 13)).toBe("auto");
  });
});
