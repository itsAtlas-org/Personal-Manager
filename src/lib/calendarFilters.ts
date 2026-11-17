// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The calendar's two view filters, as pure functions over the events a view is about to draw:
//
//   - `hideFinished` (the Agenda's Hide done): drops what has ended, so the Runway's strips and the
//     List show what is still ahead.
//   - `keepAllDay` (the header's All | All-day): drops the timed events, so a busy Month or List
//     thins to the day-level items.
//
// Both go on a view's props and nowhere earlier. The unfiltered lists (`visibleEvents`,
// `shownEvents`) also feed the search reveal, the drag, and a task's link to its event, and those
// must still find an event the filter is hiding. The order CalendarView applies them in: the
// Calendars list (the hidden set), then the kind, then done.
//
// What "done" means is decided here for events and in `runway.placeCards` for tasks; the milestone
// test is `runway.milestoneDone`, shared, so the board and the List can't disagree about it.

import {
  eventDaySpan,
  inTimeGridBand,
  isDraftEvent,
  isEventPast,
  isMilestoneEvent,
  milestoneOverlayId,
  startOfDay,
} from "./calendar-layout";
import { milestoneDone } from "./runway";
import type { CalendarEvent, Milestone } from "./types";

/** Which events the views that carry the filter draw: every one, or only the all-day ones. */
export type EventKinds = "all" | "all-day";

/** The overlay ids (`milestone:<id>`) of the milestones that are met: the rows Hide done takes off
 *  the List. Built from the milestones themselves, so the List agrees with the board. */
export function metMilestoneIds(milestones: readonly Milestone[]): Set<string> {
  const out = new Set<string>();
  for (const m of milestones) if (milestoneDone(m)) out.add(milestoneOverlayId(m.id));
  return out;
}

/** The event UIDs milestones point at. A milestone linked to an event that is in the mirror gets no
 *  overlay of its own: it is drawn as that event, which may be timed, so All-day keeps it. A
 *  milestone whose event is missing does get an overlay, so it doesn't count here. */
export function milestoneEventUids(milestones: readonly Milestone[]): Set<string> {
  const out = new Set<string>();
  for (const m of milestones) if (m.event_uid !== null && !m.event_missing) out.add(m.event_uid);
  return out;
}

/** `events` less the timed ones: what "All-day" keeps is every event with Google's all-day flag
 *  (the time grid's own all-day band, `inTimeGridBand`: holidays, birthdays, trips, multi-day
 *  all-day spans), PM's task, milestone and pinboard overlays (all-day by construction), the event
 *  a milestone links to (`milestoneUids`: it has no overlay, so it is the milestone), and the new
 *  event's ghost. A timed event is hidden however many days it runs. Returns `events` itself when
 *  nothing goes, so an unchanged list doesn't rebuild what is keyed on it. */
export function keepAllDay(
  events: readonly CalendarEvent[],
  milestoneUids: ReadonlySet<string>,
): CalendarEvent[] {
  const kept = events.filter(
    (ev) =>
      inTimeGridBand(ev) || isDraftEvent(ev) || (ev.uid !== null && milestoneUids.has(ev.uid)),
  );
  return kept.length === events.length ? (events as CalendarEvent[]) : kept;
}

/** What `hideFinished` needs. */
export interface FinishedInput {
  /** The ticking clock: an event is over once its end is before it. */
  now: Date;
  /** Overlay ids of the met milestones (`metMilestoneIds`). */
  metIds: ReadonlySet<string>;
  /** The first day the view draws. Only rows ending on or after it count as hidden: the List
   *  starts at the cursor and the Runway's strips at today, and a row before that was never going
   *  to be drawn. */
  fromDay: Date;
}

/** `events` without what has finished, and how many such rows the view would have drawn.
 *
 *  Finished is: a synced event or pinboard entry that is over (`isEventPast`, so an event running
 *  now or later, and an all-day one on today, stay), and a met milestone's overlay, on any day.
 *  Never finished: a task overlay (all open work, an overdue "Due:" chip included), an unmet
 *  milestone's overlay, late or not, and the new event's ghost. Declined events are not special:
 *  they go when they are over like any other. */
export function hideFinished(
  events: readonly CalendarEvent[],
  { now, metIds, fromDay }: FinishedInput,
): { shown: CalendarEvent[]; hidden: number } {
  const from = startOfDay(fromDay).getTime();
  const shown: CalendarEvent[] = [];
  let hidden = 0;
  for (const ev of events) {
    const finished =
      !isDraftEvent(ev) && (isMilestoneEvent(ev) ? metIds.has(ev.id) : isEventPast(ev, now));
    if (!finished) {
      shown.push(ev);
      continue;
    }
    const span = eventDaySpan(ev);
    if (span && span.endDay.getTime() >= from) hidden += 1;
  }
  return { shown, hidden };
}
