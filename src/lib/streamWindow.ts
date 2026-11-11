// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The arithmetic behind the Month and Year streams: fixed-height rows in a very long scroller, only
// the ones near the viewport drawn. Pure, so the views stay thin and jsdom (which lays nothing out)
// can still check the numbers. Rows are `stride` px apart; a row's top is `index × stride`.

/** A run of rows: `first` is the first drawn, `last` one past the last (so `last - first` rows). */
export interface RowSlice {
  first: number;
  last: number;
}

/** Rows drawn beyond the viewport each side: a fast fling runs this far before React catches up. */
export const STREAM_OVERSCAN = 12;

/** A slice moves only once it is this many rows off where it should be: crossing a row costs no
 *  render, the overscan absorbs it. */
export const SLICE_STEP = 4;

const clamp = (n: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, n));

/** The rows to draw for a viewport at `scrollTop`: those it shows, plus `overscan` each side, clamped
 *  to `[0, rows]`. Empty when there are no rows or no stride yet (the pane isn't measured). */
export function sliceRows(
  scrollTop: number,
  clientHeight: number,
  stride: number,
  rows: number,
  overscan = STREAM_OVERSCAN,
): RowSlice {
  if (rows <= 0 || !(stride > 0)) return { first: 0, last: 0 };
  const top = clamp(Math.floor(scrollTop / stride), 0, rows - 1);
  const bottom = clamp(Math.ceil((scrollTop + clientHeight) / stride), top + 1, rows);
  const first = clamp(top - overscan, 0, rows);
  return { first, last: clamp(bottom + overscan, first, rows) };
}

/**
 * The slice to hold now that `ideal` is where it should be: `current` (the very object, so a state
 * setter bails out) until it has drifted `step` rows at either end, and at once where it is short of
 * a stream's end that `ideal` reaches, since a drift under `step` there would leave a visible row
 * undrawn. `rows` is the stream's length.
 */
export function nextSlice(
  current: RowSlice,
  ideal: RowSlice,
  rows: number,
  step = SLICE_STEP,
): RowSlice {
  const drifted =
    Math.abs(ideal.first - current.first) >= step || Math.abs(ideal.last - current.last) >= step;
  const topShort = ideal.first === 0 && current.first > 0;
  const bottomShort = ideal.last === rows && current.last < rows;
  return drifted || topShort || bottomShort ? ideal : current;
}

/** The smallest slice holding both: a glide's target drawn while the rows it starts from stay. */
export function coverSlices(a: RowSlice, b: RowSlice): RowSlice {
  return { first: Math.min(a.first, b.first), last: Math.max(a.last, b.last) };
}

/** The row "in focus" at `scrollTop`: the one 40% of the way down the pane, so the header names the
 *  period filling the view rather than the one leaving it. Clamped to the stream. */
export function focusRow(scrollTop: number, clientHeight: number, stride: number, rows: number) {
  if (rows <= 0 || !(stride > 0)) return 0;
  return clamp(Math.round((scrollTop + clientHeight * 0.4) / stride), 0, rows - 1);
}

/** The row at the top of the pane, fractionally (a half-scrolled row is x.5): what a re-height keeps
 *  in place. Zero without a stride. */
export function rowAtTop(scrollTop: number, stride: number): number {
  return stride > 0 ? scrollTop / stride : 0;
}

/** How a PM-driven scroll from `fromRow` to `toRow` should travel: a glide when it is near, a jump
 *  when it is more than `maxGlideRows` away. A held key steps faster than a glide finishes, so each
 *  step starts from wherever the last one has got to; once it has run that far ahead, chasing it with
 *  a glide only adds lag. */
export function glideOrJump(
  fromRow: number,
  toRow: number,
  maxGlideRows: number,
): "smooth" | "auto" {
  return Math.abs(toRow - fromRow) <= maxGlideRows ? "smooth" : "auto";
}
