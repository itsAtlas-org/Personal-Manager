// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Calendars 5/7": the calendar visibility list inside a header button, for the pop-out Calendar
// window (#1039), which has no sidebar to hold it. Bobby: "Put the list of show hide calendars from
// the sidebar somewhere reasonable on the popout calendar screen since its useful to have."
//
// The same `CalendarSourceList` rows the sidebar block renders (the colour dots, and the shapes that
// make them readable in the colour-blind axis), in an anchored Popover that stays open while you tick,
// so the grid underneath redraws as you go. Esc or a click elsewhere closes it.
//
// Only the pop-out gets this button. The main window's one home for the list is its sidebar, so it is
// NOT mirrored into that window's header (one control, one home per window); Google's own web
// calendar keeps this list in a sidebar too, so the popover is a deviation made for a narrow window
// meant to sit beside other apps, where a side panel would take a third of its width.

import { rosterCount } from "../../lib/calendarPrefs";
import type { Calendar, CalendarAccount } from "../../lib/types";
import { Button, Popover } from "../ui";
import { CalendarSourceList } from "./CalendarSourceList";

interface Props {
  accounts: CalendarAccount[];
  calendars: Calendar[];
  /** Calendar ids the user has hidden from the view. */
  hidden: Set<string>;
  onToggle: (calendarId: string) => void;
  colorOf: (calendarId: string) => string;
  shapeOf?: (calendarId: string) => number | undefined;
}

export function CalendarsPopover({
  accounts,
  calendars,
  hidden,
  onToggle,
  colorOf,
  shapeOf,
}: Props) {
  const { shown, total } = rosterCount(calendars, hidden);
  return (
    <Popover
      align="right"
      escapeClipping
      ariaLabel="Calendars to show"
      // A long list scrolls inside the panel rather than past the window's edge.
      panelClassName="max-h-[min(24rem,70vh)] overflow-y-auto"
      trigger={({ open, toggle }) => (
        <Button
          variant="tertiary"
          size="sm"
          onClick={toggle}
          aria-expanded={open}
          // The sidebar's help entry: it is the same list.
          data-help="calendar-filter"
        >
          {`Calendars ${shown}/${total}`}
        </Button>
      )}
    >
      <CalendarSourceList
        accounts={accounts}
        calendars={calendars}
        hidden={hidden}
        onToggle={onToggle}
        colorOf={colorOf}
        shapeOf={shapeOf}
        hideUnselected
      />
    </Popover>
  );
}
