// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Year view: a continuously vertically-scrolling stream of mini-months laid out in width-
// responsive columns, with CSS scroll-snap to each MONTH row (never a whole year). It spans 25 years
// around an anchor month, and draws only the rows near the viewport (each absolutely placed in a
// scroller of the full height, see `streamWindow`), so a fling never reaches its end. Scrolling
// reports the year filling the view up via onFocusDate so the header tracks it; the nav arrows, Today
// and the mini-calendar scroll here. Event presence per day comes from the real mirror (soft disc /
// span pill), today is the accent circle, and the current real month's title reads in accent-text.
// Clicking a day drops the cursor there (the caller decides which view to open).

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type UIEvent,
} from "react";
import type { CalendarEvent } from "../../../lib/types";
import {
  dayKey,
  eventDaySpan,
  isMultiDay,
  parseLocal,
  startOfDay,
} from "../../../lib/calendar-layout";
import {
  coverSlices,
  focusRow,
  glideOrJump,
  nextSlice,
  rowAtTop,
  sliceRows,
  type RowSlice,
} from "../../../lib/streamWindow";
import { cn } from "../../ui";
import { MiniMonth, type MiniSpan } from "../parts/MiniMonth";
import { useProgrammaticScroll } from "../parts/useProgrammaticScroll";
import { COL_GAP_PX, ROW_GAP_PX } from "../parts/useYearGridLayout";

interface Props {
  cursor: Date;
  /** Already filtered to visible (non-hidden) calendars. */
  events: CalendarEvent[];
  onSelectDay: (d: Date) => void;
  /** Reports the year filling the view as the user scrolls, so the header label can track it. */
  onFocusDate: (d: Date) => void;
}

const TITLE_PX = 26; // the month-name row above each mini-month
const WEEKDAY_PX = 16;
const MIN_CELL_PX = 18;
const MAX_CELL_PX = 40;
// The stream's extent each side of the anchor month: 25 years each way, 75 rows of mini-months at
// four columns, 300 at one. It was ±24 months (two years) because the event mirror then ended there; PM
// keeps every event now (#1047). A jump past it re-anchors.
const PAST_MONTHS = 300;
const FUTURE_MONTHS = 300;
// A mini-month row is tall (a third of a pane at the narrowest), so the overscan is short: 6 rows
// each side is up to 24 mini-months at four columns.
const YEAR_OVERSCAN = 6;
const SLICE_STEP_ROWS = 2;
// A PM-driven scroll farther than this many MONTHS jumps rather than glides (a held ↓ outruns a glide):
// two years, which a single year step (‹ › ↓) never exceeds. In rows it depends on the columns (a year
// is 3 rows at four columns, 12 at one), so the limit is a month count turned into rows below.
const MAX_GLIDE_MONTHS = 24;
const maxGlideRows = (cols: number) => Math.ceil(MAX_GLIDE_MONTHS / cols) + 1; // +1: the half-scrolled top row

const monthKey = (d: Date) => `${d.getFullYear()}-${d.getMonth()}`;
const firstOfMonth = (d: Date) => new Date(d.getFullYear(), d.getMonth(), 1);

export function YearView({ cursor, events, onSelectDay, onFocusDate }: Props) {
  const today = new Date();
  const [anchor, setAnchor] = useState(() => firstOfMonth(cursor));
  const windowStart = useMemo(
    () => new Date(anchor.getFullYear(), anchor.getMonth() - PAST_MONTHS, 1),
    [anchor],
  );
  const months = useMemo(
    () =>
      Array.from(
        { length: PAST_MONTHS + FUTURE_MONTHS + 1 },
        (_, i) => new Date(windowStart.getFullYear(), windowStart.getMonth() + i, 1),
      ),
    [windowStart],
  );

  const { singleDays, spans } = useMemo(() => {
    const singleDays = new Set<string>();
    const spans: MiniSpan[] = [];
    for (const ev of events) {
      if (isMultiDay(ev)) {
        const span = eventDaySpan(ev);
        if (span) spans.push({ startDay: span.startDay, endDay: span.endDay });
        continue;
      }
      const start = parseLocal(ev.start, ev.all_day);
      if (start) singleDays.add(dayKey(startOfDay(start)));
    }
    return { singleDays, spans };
  }, [events]);
  const hasEvent = useCallback((date: Date) => singleDays.has(dayKey(date)), [singleDays]);

  const scrollRef = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el) setSize({ w: el.clientWidth, h: el.clientHeight });
  }, []);
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      const r = entries[0]?.contentRect;
      if (r) setSize({ w: r.width, h: r.height });
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const cols = size.w >= 1400 ? 4 : size.w >= 1000 ? 3 : size.w >= 620 ? 2 : 1;
  const colW = size.w > 0 ? (size.w - 32 - COL_GAP_PX * (cols - 1)) / cols : 0; // 32 = p-4 both sides
  const cellPx =
    colW > 0 ? Math.max(MIN_CELL_PX, Math.min(MAX_CELL_PX, Math.floor(colW / 7.4))) : 0;
  const monthH = TITLE_PX + WEEKDAY_PX + 6 * cellPx + 4;
  const rowStride = monthH + ROW_GAP_PX;
  const ready = cellPx > 0 && size.h > 0;

  const rows = Math.ceil(months.length / cols);
  // The rows drawn, in rows of `cols` mini-months: set where PM scrolls to, and moved as the view
  // scrolls. Nothing is drawn until the pane is measured.
  const [slice, setSlice] = useState<RowSlice>({ first: 0, last: 0 });

  const focusKeyRef = useRef(monthKey(cursor));
  const { programmaticRef, scrollTo } = useProgrammaticScroll();
  const rafRef = useRef(0);
  const didInitRef = useRef(false);
  const prevAnchorRef = useRef(anchor);
  // The month at the top-left (fractional while half-scrolled), kept when the rows change height or
  // the columns change count, and the offset PM's own write landed on, so its scroll event isn't
  // taken for the user moving it. As the day grids' `topHourRef`.
  const topMonthRef = useRef(0);
  const ownTopRef = useRef<number | null>(null);

  const monthOffset = useCallback(
    (d: Date) =>
      (d.getFullYear() - windowStart.getFullYear()) * 12 + (d.getMonth() - windowStart.getMonth()),
    [windowStart],
  );
  const inWindow = useCallback(
    (d: Date) => {
      const o = monthOffset(d);
      return o >= 0 && o < months.length;
    },
    [monthOffset, months.length],
  );
  const rowOfMonth = useCallback(
    (d: Date) => Math.floor(Math.max(0, Math.min(months.length - 1, monthOffset(d))) / cols),
    [monthOffset, months.length, cols],
  );

  const scrollToRow = useCallback(
    (row: number, smooth: boolean) => {
      const el = scrollRef.current;
      if (!el) return;
      // Draw the target's rows before scrolling to them, and keep a glide's starting rows until the
      // scroll events trim them (a jump drops them at once).
      const target = sliceRows(row * rowStride, el.clientHeight, rowStride, rows, YEAR_OVERSCAN);
      setSlice((cur) => (smooth ? coverSlices(cur, target) : target));
      topMonthRef.current = row * cols;
      ownTopRef.current = null;
      scrollTo(el, row * rowStride, smooth);
      if (!smooth) ownTopRef.current = el.scrollTop;
    },
    [rowStride, rows, cols, scrollTo],
  );

  // Keep the MONTH at the top-left, not the pixel, when the rows change height or the columns change
  // count (a resize): the offset is rows × height, and at 25 years each way the same few pixels would
  // slide the view by years. Before paint, from `topMonthRef`, not from `scrollTop`, which holds the old
  // layout's offset by now.
  const laidOutRef = useRef({ rowStride, cols });
  useLayoutEffect(() => {
    const el = scrollRef.current;
    const was = laidOutRef.current;
    laidOutRef.current = { rowStride, cols };
    if (!el || !ready || !didInitRef.current) return;
    if (was.rowStride === rowStride && was.cols === cols) return;
    el.scrollTop = (topMonthRef.current / cols) * rowStride;
    ownTopRef.current = el.scrollTop;
    setSlice((cur) =>
      nextSlice(
        cur,
        sliceRows(el.scrollTop, el.clientHeight, rowStride, rows, YEAR_OVERSCAN),
        rows,
        SLICE_STEP_ROWS,
      ),
    );
  }, [rowStride, cols, rows, ready]);

  // Position on first paint (instant), window rebuild (instant), and external cursor move (a glide
  // when it's near, else a jump). A scroll-driven cursor value has key === focusKeyRef and is skipped
  // — no feedback loop.
  useLayoutEffect(() => {
    if (!ready) return;
    const key = monthKey(cursor);
    const anchorChanged = prevAnchorRef.current !== anchor;
    if (!anchorChanged && didInitRef.current && key === focusKeyRef.current) return;
    if (!inWindow(cursor)) {
      setAnchor(firstOfMonth(cursor));
      return;
    }
    const target = rowOfMonth(cursor);
    const from = rowAtTop(scrollRef.current?.scrollTop ?? 0, rowStride);
    const smooth =
      didInitRef.current &&
      !anchorChanged &&
      glideOrJump(from, target, maxGlideRows(cols)) === "smooth";
    didInitRef.current = true;
    prevAnchorRef.current = anchor;
    focusKeyRef.current = key;
    scrollToRow(target, smooth);
  }, [cursor, anchor, ready, cols, inWindow, rowOfMonth, rowStride, scrollToRow]);

  const onScroll = useCallback(
    (e: UIEvent<HTMLDivElement>) => {
      if (!ready) return;
      // Where the user has put the top: not PM's own write (see `ownTopRef`), nor a glide's passing
      // frames (the target is already `topMonthRef`; a resize mid-glide must land there, not between).
      const top = e.currentTarget.scrollTop;
      if (!programmaticRef.current && top !== ownTopRef.current) {
        ownTopRef.current = null;
        topMonthRef.current = rowAtTop(top, rowStride) * cols;
      }
      if (rafRef.current) return;
      rafRef.current = window.requestAnimationFrame(() => {
        rafRef.current = 0;
        const el = scrollRef.current;
        if (!el) return;
        // The rows drawn follow every scroll, PM's own included.
        setSlice((cur) =>
          nextSlice(
            cur,
            sliceRows(el.scrollTop, el.clientHeight, rowStride, rows, YEAR_OVERSCAN),
            rows,
            SLICE_STEP_ROWS,
          ),
        );
        // The focus isn't reported while PM scrolls: the cursor it was sent to is already right.
        if (programmaticRef.current) return;
        const idx = Math.min(
          months.length - 1,
          focusRow(el.scrollTop, el.clientHeight, rowStride, rows) * cols,
        );
        const key = monthKey(months[idx]);
        if (key === focusKeyRef.current) return;
        focusKeyRef.current = key;
        onFocusDate(months[idx]);
      });
    },
    [ready, rowStride, rows, months, cols, onFocusDate, programmaticRef],
  );

  useEffect(() => () => cancelAnimationFrame(rafRef.current), []);

  return (
    <div
      ref={scrollRef}
      onScroll={onScroll}
      className="min-h-0 flex-1 snap-y snap-proximity overflow-y-auto p-4"
      // Rows come and go around the viewport; the browser mustn't shift the scroll to follow them.
      style={{ overflowAnchor: "none" }}
    >
      {ready && (
        // The whole stream's height, so the scrollbar and every offset are the stream's, with only
        // the drawn rows in it, each month placed at its own row and column.
        <div className="relative" style={{ height: `${rows * rowStride - ROW_GAP_PX}px` }}>
          {months.slice(slice.first * cols, slice.last * cols).map((m, i) => {
            const at = slice.first * cols + i;
            const col = at % cols;
            const isCurrent =
              m.getFullYear() === today.getFullYear() && m.getMonth() === today.getMonth();
            return (
              <div
                key={monthKey(m)}
                className="absolute snap-start overflow-hidden"
                style={{
                  top: `${Math.floor(at / cols) * rowStride}px`,
                  // The columns share the width less their gaps, as the grid's 1fr tracks did.
                  left: `calc((100% - ${COL_GAP_PX * (cols - 1)}px) / ${cols} * ${col} + ${COL_GAP_PX * col}px)`,
                  width: `calc((100% - ${COL_GAP_PX * (cols - 1)}px) / ${cols})`,
                  height: `${monthH}px`,
                }}
              >
                <div
                  className={cn(
                    "mb-1 flex items-baseline gap-2 truncate font-head text-sm",
                    isCurrent ? "text-accent-text" : "text-ink",
                  )}
                  style={{ height: `${TITLE_PX}px` }}
                >
                  {m.toLocaleDateString(undefined, { month: "long" })}
                  <span className="font-mono text-[0.6875rem] text-ink4">{m.getFullYear()}</span>
                  {isCurrent && (
                    <span className="font-mono text-[0.625rem] text-accent-text">this month</span>
                  )}
                </div>
                <MiniMonth
                  year={m.getFullYear()}
                  month={m.getMonth()}
                  today={today}
                  onSelectDay={onSelectDay}
                  hasEvent={hasEvent}
                  spans={spans}
                  showWeekdays
                  cellPx={cellPx}
                />
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
