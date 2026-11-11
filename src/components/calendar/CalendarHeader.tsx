// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The calendar's shared chrome — one header, no second sidebar. ‹ Today › nav first, at the far left
// where the eye starts, then the date-range label; at the right the view switcher (active = accent)
// and the window's own controls. In Agenda, a Runway | List switch sits beside the view switcher; the
// Runway always runs from today, so with it on, the nav and the date label give way to "From today".
//
// THE ORDER AT THE RIGHT, set once for every control that joins it: view-specific controls, the view
// switcher, + Task (the Runway only), + New event, then Pop out (main window) or Calendars n/m (the pop-out, which has no
// sidebar to hold the list), then Refresh in the corner. Refresh always takes the last corner because
// that is where a hand goes for it (Bobby: "i instinctively click top right for refresh").
//
// LAYOUT: a grid, not one wrapping row, because a wrapping row cannot promise the corner. Refresh
// has its own cell at the first row's right end; the controls group is the part that wraps. Wide
// (the header at least 56rem across) that group is the middle column, in line with the nav and the
// date label; narrower (the pop-out's 480 px minimum, a narrow main window) it drops to a full-width
// second row, right-aligned, and wraps there, while Refresh stays beside the nav on the first row.
// DOM order stays nav, controls, Refresh, so Tab ends on Refresh in either layout.
//
// "+ New event" (#884, C10) is here only in the views without a grid corner to hold it (Month, Year,
// Agenda, and Terminal's), at the right end beside Refresh; in Week and Day it sits in the grid's
// top-left corner instead (see `NewEventButton`). It shows once a Google account is connected.
//
// THE RUNWAY'S PIECES (Agenda, Runway layout) join the left group, where the nav sits in the other
// views: the board draws its readout ("3/7 done", the bar, Next up), Hide events and + Window into
// a slot beside "From today" (the board stays the owner of what they open), and + Task is this
// header's own button, handed to CalendarView, which owns the editor. That group is wide, so the
// Runway keeps it on the first row until the header is much wider than the other views need.
// Besides the Runway's, the rest here only changes what you see.
//
// Refresh is an icon (circular arrows) at every Depth. When it last synced is the one fact that
// tells you whether pressing it is worth it, so it is in the tooltip (hover and keyboard focus) and
// in a hidden description a screen reader reads, never in the button's name: see `calendarRefresh`.

import { useId, type ReactNode, type Ref } from "react";
import {
  DAY_COUNT_MAX,
  DAY_COUNT_MIN,
  type AgendaMode,
  type CalendarRange,
  type CalendarViewMode,
  type RangeBounds,
} from "../../lib/calendarPrefs";
import { pageKeyHint } from "../../lib/calendarNavKey";
import { refreshDescription, refreshTip } from "../../lib/calendarRefresh";
import { useTheme, type Coords } from "../../theme";
import {
  Button,
  PopOutIcon,
  RefreshIcon,
  SegmentedControl,
  Select,
  Tooltip,
  VisuallyHidden,
  cn,
  type SegOption,
} from "../ui";
import { MiniCalendarPopover } from "./MiniCalendarPopover";
import { NewEventButton } from "./NewEventButton";
import { RangeControl } from "./RangeControl";

interface Props {
  view: CalendarViewMode;
  availableViews: readonly CalendarViewMode[];
  onViewChange: (v: CalendarViewMode) => void;
  /** Time-grid vertical scale (Week/Day only). */
  range: CalendarRange;
  onRangeChange: (r: CalendarRange) => void;
  /** How many days the Day view shows (1-6). Week is fixed at 7 and hides this control. */
  dayCount: number;
  onDayCountChange: (n: number) => void;
  /** Custom Work/Day hour windows + the setter (null clears back to the computed default). */
  customBounds: Partial<Record<CalendarRange, RangeBounds>>;
  onBoundsChange: (range: CalendarRange, bounds: RangeBounds | null) => void;
  /** The user's coordinates, for the Day range's sunrise/sunset default. */
  coords: Coords | null;
  /** The current period label (e.g. "July 2026") shown between the nav arrows. */
  label: string;
  /** The anchor day, for the mini-calendar picker. */
  cursor: Date;
  onPickDate: (d: Date) => void;
  onPrev: () => void;
  onNext: () => void;
  onToday: () => void;
  onRefresh: () => void;
  syncing: boolean;
  lastSync: string | null;
  /** Start a new event. Absent: no Google account is connected, or the button is in the grid's
   *  corner (Week and Day), and there's none here. */
  onCreate?: () => void;
  /** Why no new event can be made now, or null when one can. */
  createReason?: string | null;
  /** The "+ New event" button: quick create opens under it, and gives focus back to it. */
  createRef?: Ref<HTMLButtonElement>;
  /** The Agenda's layout: the Runway board or the dated list. Its switch shows in Agenda only. */
  agendaMode?: AgendaMode;
  onAgendaModeChange?: (mode: AgendaMode) => void;
  /** The Runway's slot beside "From today", given to the board to draw its readout, Hide events
   *  and + Window into. Called with the element when the Runway shows, and with null when it goes. */
  runwaySlotRef?: (el: HTMLDivElement | null) => void;
  /** Start a new task (the Runway's + Task, before + New event). Absent outside the Runway. */
  onCreateTask?: () => void;
  /** Open the Calendar in a window of its own (#1039), or bring that window forward. Absent in the
   *  pop-out itself. */
  onPopOut?: () => void;
  /** The pop-out's "Calendars n/m" button, just before Refresh. Absent in the main window, whose
   *  sidebar is the one home of that list. */
  roster?: ReactNode;
}

const AGENDA_OPTIONS: SegOption<AgendaMode>[] = [
  { value: "runway", label: "Runway", title: "Your tasks in columns, from today on" },
  { value: "list", label: "List", title: "Your events, day by day" },
];

const VIEW_LABEL: Record<CalendarViewMode, string> = {
  month: "Month",
  week: "Week",
  day: "Day",
  year: "Year",
  agenda: "Agenda",
};

export function CalendarHeader({
  view,
  availableViews,
  onViewChange,
  range,
  onRangeChange,
  dayCount,
  onDayCountChange,
  customBounds,
  onBoundsChange,
  coords,
  label,
  cursor,
  onPickDate,
  onPrev,
  onNext,
  onToday,
  onRefresh,
  syncing,
  lastSync,
  onCreate,
  createReason = null,
  createRef,
  agendaMode = "list",
  onAgendaModeChange,
  runwaySlotRef,
  onCreateTask,
  onPopOut,
  roster,
}: Props) {
  const { system } = useTheme();
  // The Refresh button's description, always mounted (a tooltip exists only while shown, so an
  // aria-describedby at it would dangle).
  const syncedId = useId();
  const now = new Date();
  // The Work/Day/24h scale only drives the pixel time-grid (Slate/Editorial). Terminal renders
  // Week/Day as a mono agenda with no vertical scale, so the control has nothing to act on there.
  const showRange = (view === "week" || view === "day") && system !== "terminal";

  const viewOptions: SegOption<CalendarViewMode>[] = availableViews.map((v) => ({
    value: v,
    label: VIEW_LABEL[v],
  }));
  // The Runway always runs from today, so paging and the date picker have nothing to move there.
  const runway = view === "agenda" && agendaMode === "runway" && !!onAgendaModeChange;
  // ↑ and ↓ page too where the periods run down the screen (see `calendarNavKey`).
  const vertical = view === "month" || view === "year";
  const prevKeys = pageKeyHint("prev", vertical);
  const nextKeys = pageKeyHint("next", vertical);

  return (
    <header className="@container border-b border-rule px-4 py-2" data-help="calendar-header">
      <div
        className={cn(
          "grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-4 gap-y-2",
          runway
            ? "@7xl:grid-cols-[minmax(0,1fr)_auto_auto]"
            : "@4xl:grid-cols-[auto_minmax(0,1fr)_auto]",
        )}
      >
        <div className="col-start-1 row-start-1 flex min-w-0 flex-wrap items-center gap-x-4 gap-y-2">
          {runway ? (
            <>
              <span className="px-1 font-head text-sm text-ink">From today</span>
              {/* The Runway draws its readout, Hide events and + Window here (portaled by the board,
                which owns them). A real box, not display:contents, so help mode has a rect; hidden
                while empty so it takes no gap before the board has drawn. */}
              <div
                ref={runwaySlotRef}
                className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 empty:hidden"
              />
            </>
          ) : (
            <>
              <div className="flex items-center gap-1">
                {/* Each says its keys, in the repo's "label · key" form (and aria-keyshortcuts, which a
                screen reader announces): the keys are the only way to them otherwise. */}
                <Tooltip label={`Previous · ${prevKeys.glyphs}`}>
                  <Button
                    variant="tertiary"
                    onClick={onPrev}
                    aria-label="Previous"
                    aria-keyshortcuts={prevKeys.shortcuts}
                  >
                    ‹
                  </Button>
                </Tooltip>
                <Tooltip label="Jump to today · t">
                  <Button variant="tertiary" onClick={onToday} aria-keyshortcuts="t">
                    Today
                  </Button>
                </Tooltip>
                <Tooltip label={`Next · ${nextKeys.glyphs}`}>
                  <Button
                    variant="tertiary"
                    onClick={onNext}
                    aria-label="Next"
                    aria-keyshortcuts={nextKeys.shortcuts}
                  >
                    ›
                  </Button>
                </Tooltip>
              </div>

              <MiniCalendarPopover label={label} cursor={cursor} onPick={onPickDate} />
            </>
          )}
        </div>

        <div
          className={cn(
            "col-span-2 row-start-2 flex flex-wrap items-center justify-end gap-3",
            // The Runway's left group is wide (the readout), so it keeps the first row to itself
            // until the header is wider than the other views need.
            runway
              ? "@7xl:col-span-1 @7xl:col-start-2 @7xl:row-start-1"
              : "@4xl:col-span-1 @4xl:col-start-2 @4xl:row-start-1",
          )}
        >
          {view === "agenda" && onAgendaModeChange && (
            // Agenda only, beside the view switcher: it changes how this one view lays out, so it
            // sits by what it changes and nowhere else (one control, one home).
            <SegmentedControl
              ariaLabel="Agenda layout"
              options={AGENDA_OPTIONS}
              value={agendaMode}
              onChange={onAgendaModeChange}
            />
          )}
          {view === "day" && (
            // Day only: Week IS the 7-day window, so offering a width control there would be two
            // controls producing the same picture. Sits with the range control because both answer
            // "how much am I looking at", one across and one down.
            <label className="flex items-center gap-1.5 text-xs text-ink3">
              Days
              <Select
                compact
                value={String(dayCount)}
                onChange={(e) => onDayCountChange(Number(e.currentTarget.value))}
                aria-label="How many days to show"
              >
                {Array.from({ length: DAY_COUNT_MAX - DAY_COUNT_MIN + 1 }, (_, i) => {
                  const n = DAY_COUNT_MIN + i;
                  return (
                    <option key={n} value={n}>
                      {n}
                    </option>
                  );
                })}
              </Select>
            </label>
          )}
          {showRange && (
            <>
              <RangeControl
                range={range}
                onRangeChange={onRangeChange}
                customBounds={customBounds}
                onBoundsChange={onBoundsChange}
                coords={coords}
                cursor={cursor}
              />
            </>
          )}
          {viewOptions.length > 1 && (
            <SegmentedControl
              ariaLabel="Calendar view"
              options={viewOptions}
              value={view}
              onChange={onViewChange}
            />
          )}
          {/* The "Calendars x/x" dropdown used to sit here (#556); it moved to the left sidebar, which
            lists every calendar inline — one control, one home per window. The main window keeps
            that and gets no button here. The pop-out has no sidebar, so its home is `roster` below:
            a stated deviation from Google's sidebar. */}
          {onCreateTask && (
            // The Runway's new task, beside + New event: both make something, and this one is the
            // board's (it opens the editor CalendarView owns, starting today). In the Runway's help.
            <Button variant="secondary" onClick={onCreateTask} data-help="calendar-runway">
              + Task
            </Button>
          )}
          {onCreate && (
            <NewEventButton onCreate={onCreate} createReason={createReason} createRef={createRef} />
          )}
          {onPopOut && (
            <Tooltip label="Pop out · keep the calendar in a window of its own" placement="below">
              <Button
                variant="tertiary"
                size="sm"
                onClick={onPopOut}
                aria-label="Pop out"
                data-help="calendar-popout"
              >
                <PopOutIcon />
              </Button>
            </Tooltip>
          )}
          {roster}
        </div>

        {/* Refresh, in the corner: its own grid cell, pinned to the first row's right end, so no
          amount of wrapping in the controls beside it can move it (the controls group is the one
          that drops to a second row when the header is narrow). Its name is constant ("Refresh":
          copy that says "press Refresh" still names a real control, and speech users say it); when
          it last synced is a description, not part of the name. Syncing is aria-busy and
          aria-disabled with clicks ignored, NOT `disabled`: a focused button that turns disabled
          drops keyboard focus. */}
        <div
          className={cn(
            "col-start-2 row-start-1 flex items-center",
            runway ? "@7xl:col-start-3" : "@4xl:col-start-3",
          )}
        >
          <Tooltip label={refreshTip(lastSync, syncing, now)} placement="below">
            <Button
              variant="tertiary"
              size="sm"
              aria-label="Refresh"
              aria-describedby={syncedId}
              aria-busy={syncing || undefined}
              aria-disabled={syncing || undefined}
              className="aria-busy:cursor-progress"
              onClick={() => {
                if (!syncing) onRefresh();
              }}
            >
              <RefreshIcon spinning={syncing} />
            </Button>
          </Tooltip>
          <VisuallyHidden id={syncedId}>
            {refreshDescription(lastSync, syncing, now)}
          </VisuallyHidden>
        </div>
      </div>
    </header>
  );
}
