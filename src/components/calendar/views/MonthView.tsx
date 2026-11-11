// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Month view: a continuously vertically-scrolling stream of Monday-first week rows spanning ten
// years each side of an anchor month, with CSS scroll-snap to each WEEK (never a whole month). Only
// the rows near the viewport are drawn (each absolutely placed in a scroller of the full height, see
// `streamWindow`), so the stream can be that long without a fling reaching its end. Days stay full-
// emphasis; the month name shows inline at each month's first day and alternating months carry a faint
// tint so boundaries read at a glance. Scrolling reports the month filling the view up via onFocusDate
// so the header label tracks it; the nav arrows, Today and the mini-calendar scroll here in turn. The
// per-week chips + multi-day band overlay reuse buildWeekRows, so they match the single-month grid.
// Given an `interaction` (the calendar's Month, C16c), it draws what a drag needs: data attributes the
// drag measures (the stream's weeks, each chip's and band's event), what a press must never create
// on, the ghost over the days it would land on, and the pending look; and it stops snapping to weeks
// while a drag scrolls it. The drag itself is the calendar's (`useCalendarGesture`).

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
  addDays,
  clampSpanToRange,
  dayDiff,
  dayKey,
  DRAFT_EVENT_CLASS,
  DRAG_ORIGIN_CLASS,
  draftNote,
  eventDaySpan,
  isEventPast,
  PAST_EVENT_CLASS,
  PENDING_EVENT_CLASS,
  spokenName,
  startOfDay,
} from "../../../lib/calendar-layout";
import { formatClockIso } from "../../../lib/format";
import {
  coverSlices,
  focusRow,
  glideOrJump,
  nextSlice,
  rowAtTop,
  sliceRows,
  type RowSlice,
} from "../../../lib/streamWindow";
import { useDepth } from "../../../theme";
import { eventColour, eventFill, eventHover, inkOn } from "../../../theme/eventPalette";
import { cn } from "../../ui";
import { declinedName, isDeclined } from "../../../lib/calendarEdit/rsvp";
import { DragGhost, type GhostView } from "../parts/DragGhost";
import { EventChip } from "../parts/EventChip";
import { SourceDot } from "../parts/SourceDot";
import { useProgrammaticScroll } from "../parts/useProgrammaticScroll";
import { buildWeekRows, chipsShown, weekStartMonday, weekdayLabels } from "../parts/useMonthGrid";

interface Props {
  /** The anchor day; its month is what the view scrolls to when it changes from outside. */
  cursor: Date;
  /** Already filtered to visible (non-hidden) calendars. */
  events: CalendarEvent[];
  colorOf: (calendarId: string) => string;
  /** The source's shape slot, for the colour-blind axis's redundant dot shapes. Optional — the dots
   *  fall back to plain circles without it (and whenever the axis is off). */
  shapeOf?: (calendarId: string) => number | undefined;
  /** Reports the month filling the view as the user scrolls, so the header label can track it. */
  onFocusDate: (d: Date) => void;
  /** The ticking "now" (device-local) so past chips/bands grey back. Defaults to the render clock. */
  now?: Date;
  /** Open an event's detail popup, anchored at the chip/band's on-screen rect. */
  onEventClick?: (ev: CalendarEvent, anchor: DOMRect) => void;
  /** Events can be dragged here (the calendar's Month, C16c). Absent, the DOM is as it always was. */
  interaction?: MonthInteraction;
}

/** What a dragging calendar tells Month (C16c). Month has no edges to draw: an event moves by whole
 *  days, keeping its times. */
export interface MonthInteraction {
  /** The event being dragged: dimmed where it is (the weeks don't re-pack mid-drag). */
  originId: string | null;
  /** Where it would go, or the new event a drag across days would make. */
  ghost: GhostView | null;
  /** Moves being asked about or saved, drawn where they were dropped. */
  pendingIds: ReadonlySet<string>;
  /** A drag is under way: the weeks stop snapping, so its scrolling isn't pulled back. */
  live: boolean;
}

/** A band's look on a dragging Month (a chip's is `EventChip`'s): dimmed while dragged, dashed
 *  while its move is asked about or saved, else greyed once past. One look, so one opacity utility. */
function lookOf(
  ev: CalendarEvent,
  interaction: MonthInteraction | undefined,
  past: boolean,
): string | false {
  if (interaction?.originId === ev.id) return DRAG_ORIGIN_CLASS;
  if (interaction?.pendingIds.has(ev.id)) return PENDING_EVENT_CLASS;
  return past && PAST_EVENT_CLASS;
}

const BAND_H = 16;
const NUM_H = 22;
const CELL_PAD = 4; // matches the cells' py-1, so the row-relative band overlay clears the number row
const MAX_DOTS = 5;

// The stream's extent each side of the anchor month: ten years, ~1,045 week rows. It was ±20 months
// (#358) because the event mirror then ended there; PM keeps every event now (#1047), so a bound only
// has to be too far to reach by scrolling. A jump past it (search reveal, the mini-calendar) re-anchors.
const PAST_MONTHS = 120;
const FUTURE_MONTHS = 120;
const VISIBLE_WEEKS = 6; // weeks that fill the pane (a full month) → sets the week-row height
const MIN_WEEK_H = 56;
const MS_PER_WEEK = 7 * 86_400_000;
// A PM-driven scroll farther than a quarter ahead jumps rather than glides (a held ↓ outruns a glide).
const MAX_GLIDE_WEEKS = 13;

const monthKey = (d: Date) => `${d.getFullYear()}-${d.getMonth()}`;
const firstOfMonth = (d: Date) => new Date(d.getFullYear(), d.getMonth(), 1);
/** A row's representative month = its Thursday (ISO), so a boundary week picks the majority side.
 *  Read from the row's index, not a drawn row: most rows aren't drawn. */
const rowMonthKey = (gridStart: Date, row: number) =>
  monthKey(
    new Date(gridStart.getFullYear(), gridStart.getMonth(), gridStart.getDate() + row * 7 + 3),
  );
/** The stream around an anchor month: its first Monday and its week count. */
function streamBounds(anchor: Date): { gridStart: Date; rows: number } {
  const start = weekStartMonday(new Date(anchor.getFullYear(), anchor.getMonth() - PAST_MONTHS, 1));
  const endExclusive = startOfDay(
    new Date(anchor.getFullYear(), anchor.getMonth() + FUTURE_MONTHS + 1, 1),
  );
  return {
    gridStart: start,
    rows: Math.round((endExclusive.getTime() - start.getTime()) / MS_PER_WEEK),
  };
}
/** The row whose week holds the first of `d`'s month, clamped to a stream of `rows`. */
function monthRow(gridStart: Date, rows: number, d: Date): number {
  const target = weekStartMonday(firstOfMonth(d));
  const idx = Math.round((target.getTime() - gridStart.getTime()) / MS_PER_WEEK);
  return Math.max(0, Math.min(rows - 1, idx));
}

export function MonthView({
  cursor,
  events,
  colorOf,
  shapeOf,
  onFocusDate,
  now,
  onEventClick,
  interaction,
}: Props) {
  const { minimal, showMeta, showPower } = useDepth();
  const maxChips = showPower ? 4 : 3;
  const nowDate = now ?? new Date();
  // What a press on the grid must never create on: the day number, the month's name, the counts.
  const noCreate = interaction ? "" : undefined;
  const ghost = interaction?.ghost ?? null;
  const ghostSpan = useMemo(() => (ghost ? eventDaySpan(ghost.event) : null), [ghost]);

  // The stream is anchored on a month and rebuilt only when the cursor jumps outside it.
  const [anchor, setAnchor] = useState(() => firstOfMonth(cursor));
  const { gridStart, rows } = useMemo(() => streamBounds(anchor), [anchor]);
  // The rows drawn. Seeded from the cursor's own row (the pane isn't measured yet, so a row is one
  // unit tall here), and moved as the view scrolls or PM scrolls it.
  const [slice, setSlice] = useState<RowSlice>(() => {
    const b = streamBounds(anchor);
    return sliceRows(monthRow(b.gridStart, b.rows, cursor), VISIBLE_WEEKS + 1, 1, b.rows);
  });
  // Lanes are packed over the whole stream; only the drawn rows are built (`weeks[i]` is row
  // `slice.first + i`), so a keystroke in quick create's draft doesn't rebuild a decade.
  const weeks = useMemo(
    () => buildWeekRows(gridStart, rows, events, undefined, { from: slice.first, to: slice.last }),
    [gridStart, rows, events, slice],
  );
  const labels = useMemo(() => weekdayLabels(), []);

  const scrollRef = useRef<HTMLDivElement>(null);
  const [weekH, setWeekH] = useState(0);
  // The month the scroll last reported / was last driven to — the guard that breaks the feedback loop
  // between "scroll updates the cursor" and "a cursor change scrolls the view".
  const focusKeyRef = useRef(monthKey(cursor));
  const { programmaticRef, scrollTo } = useProgrammaticScroll();
  const rafRef = useRef(0);
  const didInitRef = useRef(false);
  const prevAnchorRef = useRef(anchor);
  // The week the user means to have at the top (a row, fractional while half-scrolled), kept when the
  // rows change height; and the offset PM's own write landed on, so its scroll event, which may have
  // landed short of the week (the engine clamps), isn't taken for the user moving it. As the day
  // grids' `topHourRef`.
  const topRowRef = useRef(0);
  const ownTopRef = useRef<number | null>(null);

  const weekIndexOfMonth = useCallback(
    (d: Date) => monthRow(gridStart, rows, d),
    [gridStart, rows],
  );

  const inWindow = useCallback(
    (d: Date) => {
      const f = firstOfMonth(d).getTime();
      const last = addDays(gridStart, rows * 7 - 1).getTime();
      return f >= gridStart.getTime() && f <= last;
    },
    [gridStart, rows],
  );

  const scrollToIndex = useCallback(
    (idx: number, smooth: boolean) => {
      const el = scrollRef.current;
      if (!el || weekH === 0) return;
      // Draw the target's rows before scrolling to them, so a search reveal finds its piece, and a
      // glide keeps what it starts from until the scroll events trim it (a jump drops it at once).
      const target = sliceRows(idx * weekH, el.clientHeight, weekH, rows);
      setSlice((cur) => (smooth ? coverSlices(cur, target) : target));
      topRowRef.current = idx;
      ownTopRef.current = null;
      scrollTo(el, idx * weekH, smooth);
      if (!smooth) ownTopRef.current = el.scrollTop;
    },
    [weekH, rows, scrollTo],
  );

  // Measure the pane → week-row height (a full month fills it), first synchronously to avoid a flash,
  // then track resizes.
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el && el.clientHeight > 0) {
      setWeekH(Math.max(MIN_WEEK_H, Math.round(el.clientHeight / VISIBLE_WEEKS)));
    }
  }, []);
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      const h = entries[0]?.contentRect.height;
      if (h && h > 0) setWeekH(Math.max(MIN_WEEK_H, Math.round(h / VISIBLE_WEEKS)));
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Keep the WEEK at the top, not the pixel, when the rows change height: the strip above the grid
  // appearing, a window resize, the header wrapping. Ten years out, the same few pixels of row height
  // would otherwise slide the view by months (the offset is rows × height). Before paint, from
  // `topRowRef` (the row the user left at the top), not from `scrollTop`, which holds the old rows'
  // offset by now. As TimeGridView does for its hour.
  const laidOutWeekHRef = useRef(weekH);
  useLayoutEffect(() => {
    const el = scrollRef.current;
    const was = laidOutWeekHRef.current;
    laidOutWeekHRef.current = weekH;
    if (!el || was === weekH || was === 0 || weekH === 0 || !didInitRef.current) return;
    el.scrollTop = topRowRef.current * weekH;
    ownTopRef.current = el.scrollTop;
    setSlice((cur) => nextSlice(cur, sliceRows(el.scrollTop, el.clientHeight, weekH, rows), rows));
  }, [weekH, rows]);

  // Position the view: first paint (instant), a window rebuild (instant), and an external cursor move
  // (a glide when it's near, else a jump). A scroll-driven cursor value has key === focusKeyRef and is
  // skipped — no feedback loop.
  useLayoutEffect(() => {
    if (weekH === 0) return;
    const key = monthKey(cursor);
    const anchorChanged = prevAnchorRef.current !== anchor;
    if (!anchorChanged && didInitRef.current && key === focusKeyRef.current) return;
    if (!inWindow(cursor)) {
      setAnchor(firstOfMonth(cursor)); // rebuild around the jumped-to month; repositions next pass
      return;
    }
    const target = weekIndexOfMonth(cursor);
    const from = rowAtTop(scrollRef.current?.scrollTop ?? 0, weekH);
    const smooth =
      didInitRef.current &&
      !anchorChanged &&
      glideOrJump(from, target, MAX_GLIDE_WEEKS) === "smooth";
    didInitRef.current = true;
    prevAnchorRef.current = anchor;
    focusKeyRef.current = key;
    scrollToIndex(target, smooth);
  }, [cursor, anchor, weekH, inWindow, weekIndexOfMonth, scrollToIndex]);

  const onScroll = useCallback(
    (e: UIEvent<HTMLDivElement>) => {
      if (weekH === 0) return;
      // Where the user has put the top: not PM's own write (see `ownTopRef`), nor a glide's passing
      // frames (the target is already `topRowRef`; a resize mid-glide must land there, not between).
      const top = e.currentTarget.scrollTop;
      if (!programmaticRef.current && top !== ownTopRef.current) {
        ownTopRef.current = null;
        topRowRef.current = rowAtTop(top, weekH);
      }
      if (rafRef.current) return;
      rafRef.current = window.requestAnimationFrame(() => {
        rafRef.current = 0;
        const el = scrollRef.current;
        if (!el) return;
        // The rows drawn follow every scroll, PM's own included (a drag's auto-scroll needs its rows).
        setSlice((cur) =>
          nextSlice(cur, sliceRows(el.scrollTop, el.clientHeight, weekH, rows), rows),
        );
        // The focus isn't reported while PM scrolls: the cursor it was sent to is already right.
        if (programmaticRef.current) return;
        const key = rowMonthKey(gridStart, focusRow(el.scrollTop, el.clientHeight, weekH, rows));
        if (key === focusKeyRef.current) return;
        focusKeyRef.current = key;
        const [y, m] = key.split("-").map(Number);
        onFocusDate(new Date(y, m, 1));
      });
    },
    [weekH, rows, gridStart, onFocusDate, programmaticRef],
  );

  useEffect(() => () => cancelAnimationFrame(rafRef.current), []);

  return (
    <div className="flex h-full min-h-0 flex-1 flex-col">
      {/* Weekday header (fixed above the scroll) */}
      <div className="grid grid-cols-7 border-b border-rule">
        {labels.map((w, i) => (
          <div
            key={i}
            className="px-2 py-1 text-center font-head text-[0.6875rem] uppercase tracking-wide text-ink3"
          >
            {w}
          </div>
        ))}
      </div>

      {/* Scrolling week stream */}
      {/* Scrolling week stream. On a calendar that drags, the drag reads its weeks here; it isn't
          selectable, and while a drag is under way it stops snapping to weeks, which would pull
          the drag's own scrolling back. */}
      <div
        ref={scrollRef}
        onScroll={onScroll}
        className={cn(
          "min-h-0 flex-1 overflow-y-auto",
          interaction?.live ? "snap-none" : "snap-y snap-proximity",
          interaction && "select-none",
        )}
        data-surface={interaction ? "month" : undefined}
        data-grid-start={interaction ? dayKey(gridStart) : undefined}
        data-week-h={interaction ? weekH : undefined}
        data-weeks={interaction ? rows : undefined}
        // Rows come and go around the viewport; the browser mustn't shift the scroll to follow them.
        style={{ overflowAnchor: "none" }}
      >
        {/* The whole stream's height, so the scrollbar and every offset are the stream's, with only
            the drawn rows in it, each placed at its own row. */}
        <div className="relative" style={{ height: `${rows * weekH}px` }}>
          {weekH > 0 &&
            weeks.map((week, i) => {
              // The ghost's days in this week, if any.
              const ghostHere =
                ghost && ghostSpan
                  ? clampSpanToRange(
                      dayDiff(week.cells[0].date, ghostSpan.startDay),
                      dayDiff(week.cells[0].date, ghostSpan.endDay),
                      6,
                    )
                  : null;
              return (
                <div
                  key={dayKey(week.cells[0].date)}
                  className="absolute inset-x-0 grid snap-start grid-cols-7 border-b border-rule"
                  style={{ top: `${(slice.first + i) * weekH}px`, height: `${weekH}px` }}
                >
                  {week.cells.map((cell) => {
                    const hidden = Math.max(0, cell.chips.length - maxChips) + cell.hiddenBands;
                    const dotsHidden = Math.max(0, cell.chips.length - MAX_DOTS) + cell.hiddenBands;
                    const firstOfM = cell.date.getDate() === 1;
                    const tint = (cell.date.getFullYear() * 12 + cell.date.getMonth()) % 2 === 1;
                    return (
                      <div
                        key={dayKey(cell.date)}
                        className={cn(
                          "flex min-h-0 flex-col overflow-hidden border-l border-rule px-1 py-1 first:border-l-0",
                          tint && "bg-panel",
                        )}
                        data-day={interaction ? dayKey(cell.date) : undefined}
                      >
                        <div
                          className="flex items-start justify-between"
                          style={{ height: `${NUM_H}px` }}
                        >
                          <span className="flex items-center gap-1 overflow-hidden">
                            {firstOfM && (
                              <span
                                className="truncate font-head text-[0.625rem] uppercase tracking-wide text-accent-text"
                                data-no-create={noCreate}
                              >
                                {cell.date.toLocaleDateString(undefined, { month: "short" })}
                                {cell.date.getMonth() === 0 ? ` ${cell.date.getFullYear()}` : ""}
                              </span>
                            )}
                            <span
                              className={cn(
                                "flex h-5 w-5 shrink-0 items-center justify-center font-mono text-xs",
                                cell.isToday
                                  ? "rounded-full bg-accent font-medium text-accent-ink"
                                  : "text-ink2",
                              )}
                              data-no-create={noCreate}
                            >
                              {cell.date.getDate()}
                            </span>
                          </span>
                          {showPower && cell.chips.length > 0 && (
                            <span
                              className="font-mono text-[0.625rem] text-ink4"
                              data-no-create={noCreate}
                            >
                              {cell.chips.length}
                            </span>
                          )}
                        </div>

                        {/* Reserve room so chips clear the band overlay. */}
                        <div style={{ height: `${week.laneCount * BAND_H}px` }} />

                        {minimal ? (
                          <div className="flex flex-wrap items-center gap-0.5">
                            {chipsShown(cell.chips, MAX_DOTS).map((ev) => {
                              const note = draftNote(ev);
                              const dot = (
                                <SourceDot
                                  key={ev.id}
                                  // The event's own colour when it has one; the shape stays its
                                  // calendar's.
                                  color={eventColour(ev.color_id)?.hex ?? colorOf(ev.calendar_id)}
                                  shapeIndex={shapeOf?.(ev.calendar_id)}
                                  ghost={note !== null}
                                  className="h-2 w-2"
                                />
                              );
                              if (note === null) return dot;
                              // The ghost's ring keeps a click on it from closing quick create, and is
                              // the one dot with a name: the others are saved events, this isn't one.
                              return (
                                <span key={ev.id} data-draft-event="" className="inline-flex">
                                  {dot}
                                  <span className="sr-only">{spokenName(note, ev.summary)}</span>
                                </span>
                              );
                            })}
                            {dotsHidden > 0 && (
                              <span
                                className="font-mono text-[0.5625rem] text-ink4"
                                data-no-create={noCreate}
                              >
                                +{dotsHidden}
                              </span>
                            )}
                          </div>
                        ) : (
                          <div className="flex min-h-0 flex-col gap-0.5 overflow-hidden">
                            {chipsShown(cell.chips, maxChips).map((ev) => (
                              <EventChip
                                key={ev.id}
                                summary={ev.summary}
                                color={colorOf(ev.calendar_id)}
                                own={eventColour(ev.color_id)}
                                timeLabel={ev.all_day ? "" : formatClockIso(ev.start)}
                                showTime={showMeta}
                                isPast={isEventPast(ev, nowDate)}
                                ghost={draftNote(ev)}
                                declined={isDeclined(ev)}
                                pieceOf={ev.id}
                                onClick={
                                  onEventClick ? (rect) => onEventClick(ev, rect) : undefined
                                }
                                drag={
                                  interaction
                                    ? {
                                        eventId: ev.id,
                                        dimmed: interaction.originId === ev.id,
                                        pending: interaction.pendingIds.has(ev.id),
                                      }
                                    : undefined
                                }
                              />
                            ))}
                            {hidden > 0 && (
                              <span
                                className="pl-1 font-mono text-[0.625rem] text-ink4"
                                data-no-create={noCreate}
                              >
                                +{hidden} more
                              </span>
                            )}
                          </div>
                        )}
                      </div>
                    );
                  })}

                  {/* Multi-day band overlay for this week row, aligned to the in-cell reserved spacer. */}
                  <div
                    className="pointer-events-none absolute inset-x-0"
                    style={{ top: `${NUM_H + CELL_PAD}px` }}
                  >
                    {week.bands.map((b) => {
                      const color = colorOf(b.ev.calendar_id);
                      const own = eventColour(b.ev.color_id);
                      const leftPct = (b.startIdx / 7) * 100;
                      const widthPct = ((b.endIdx - b.startIdx + 1) / 7) * 100;
                      const draft = draftNote(b.ev);
                      const declined = isDeclined(b.ev);
                      return (
                        <div
                          key={b.ev.id}
                          className={cn(
                            // The band overlay is pointer-events-none so day cells stay clickable; a band
                            // re-enables its own pointer events to open the event popup.
                            "absolute overflow-hidden px-1.5 text-[0.6875rem] leading-[0.875rem]",
                            onEventClick && `pointer-events-auto cursor-pointer ${eventHover(own)}`,
                            draft !== null
                              ? DRAFT_EVENT_CLASS
                              : lookOf(b.ev, interaction, isEventPast(b.ev, nowDate)),
                          )}
                          data-draft-event={draft !== null ? "" : undefined}
                          data-event-id={interaction ? b.ev.id : undefined}
                          style={{
                            top: `${b.lane * BAND_H}px`,
                            left: `${leftPct}%`,
                            width: `${widthPct}%`,
                            height: `${BAND_H - 2}px`,
                            background: eventFill(own, color, 20),
                            borderLeft: b.continuesLeft ? undefined : `3px solid ${color}`,
                            borderTopLeftRadius: b.continuesLeft ? 0 : "var(--radius-sm)",
                            borderBottomLeftRadius: b.continuesLeft ? 0 : "var(--radius-sm)",
                            borderTopRightRadius: b.continuesRight ? 0 : "var(--radius-sm)",
                            borderBottomRightRadius: b.continuesRight ? 0 : "var(--radius-sm)",
                          }}
                          title={b.ev.summary}
                          // A band's name is its text; the ghost's says first that it's the new event,
                          // and a declined one's says so last.
                          aria-label={
                            draft !== null || declined
                              ? declinedName(
                                  spokenName(
                                    draft,
                                    [b.ev.all_day ? "" : formatClockIso(b.ev.start), b.ev.summary]
                                      .filter(Boolean)
                                      .join(", "),
                                  ),
                                  declined,
                                )
                              : undefined
                          }
                          data-piece-of={b.ev.id}
                          role={onEventClick ? "button" : undefined}
                          tabIndex={onEventClick ? 0 : undefined}
                          onClick={
                            onEventClick
                              ? (e) => onEventClick(b.ev, e.currentTarget.getBoundingClientRect())
                              : undefined
                          }
                          onKeyDown={
                            onEventClick
                              ? (e) => {
                                  if (e.key === "Enter" || e.key === " ") {
                                    e.preventDefault();
                                    onEventClick(b.ev, e.currentTarget.getBoundingClientRect());
                                  }
                                }
                              : undefined
                          }
                        >
                          <span className="truncate font-head text-ink" style={inkOn(own)}>
                            {b.continuesLeft ? "‹ " : ""}
                            {/* A timed event across days says when it starts, so it doesn't read as
                            an all-day one. --ink2: small text on the tint needs it for 4.5:1. */}
                            {!b.ev.all_day && !b.continuesLeft && (
                              <span className="font-mono text-ink2" style={inkOn(own)}>
                                {formatClockIso(b.ev.start)}{" "}
                              </span>
                            )}
                            <span className={declined ? "line-through" : undefined}>
                              {b.ev.summary}
                            </span>
                          </span>
                        </div>
                      );
                    })}
                    {/* Where a drag would put it, over the days in this week, above the bands (the
                    weeks don't re-pack mid-drag). */}
                    {ghost && ghostHere && (
                      <DragGhost
                        ghost={ghost}
                        color={colorOf(ghost.event.calendar_id)}
                        band
                        box={{
                          top: 0,
                          // The ghost's one line is a little taller than a band's.
                          height: `${BAND_H + 2}px`,
                          left: `${(ghostHere.startDay / 7) * 100}%`,
                          width: `${((ghostHere.endDay - ghostHere.startDay + 1) / 7) * 100}%`,
                        }}
                      />
                    )}
                  </div>
                </div>
              );
            })}
        </div>
      </div>
    </div>
  );
}
