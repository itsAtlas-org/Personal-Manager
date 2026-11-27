// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The meetings owed an answer, as the app shows them: the strip, the People row's badge and the top
// of the People tab's All view all read this one list. PM works it out whenever it looks (nothing
// fires when an event ends), so it is read on mount, whenever `watch` moves (the open tab), and when
// a calendar sync or a change to People says the answer may have moved. A meeting that ends while
// the tab just stays open fires none of those, so after each read one timer is set for the soonest
// end among those that finish before the user's next midnight (never a poll, and never more than
// {@link END_WATCH_MAX_MS} away) and the list is read again then. A reply that lands after a newer
// request was sent (or after the strip is gone) is dropped, and a failed read keeps the list from
// before: the strip is a hint, never an error.

import { useCallback, useEffect, useRef, useState } from "react";
import { listCalendarEvents, onCalendarSynced, onPeopleChanged, owedMeetings } from "../../lib/ipc";
import { subscribeUntilCleanup } from "../../lib/subscribe";
import type { AgendaEvent, OwedMeeting } from "../../lib/types";

/** How long after a notice before the list is read again (a sync and its people notice are one read). */
const REFRESH_DELAY_MS = 300;

/** The longest the end-of-meeting timer waits before it looks again (a sync may have moved things). */
export const END_WATCH_MAX_MS = 30 * 60 * 1000;
/** After an end, so the backend's "ended before now" holds when it is asked. */
const END_SLACK_MS = 1000;

/** Milliseconds until the soonest timed event that hasn't ended is over (plus a moment), or null
 *  when none is left today. An all-day event ends at midnight and is left to the next look, and an
 *  event ending after the user's next local midnight is too (the agenda reaches three weeks out;
 *  without this bound the timer would stand for any event in them). */
function untilNextEnd(events: readonly AgendaEvent[], now: number): number | null {
  const midnight = new Date(now);
  midnight.setHours(24, 0, 0, 0);
  let soonest: number | null = null;
  for (const e of events) {
    if (e.all_day || e.ended || !e.end) continue;
    const end = Date.parse(e.end);
    if (Number.isNaN(end) || end <= now || end > midnight.getTime()) continue;
    if (soonest === null || end < soonest) soonest = end;
  }
  return soonest === null ? null : soonest - now + END_SLACK_MS;
}

export interface OwedMeetings {
  meetings: OwedMeeting[];
  /** Read again now. Stable. */
  reload: () => void;
}

export function useOwedMeetings(enabled: boolean, watch?: unknown): OwedMeetings {
  const [meetings, setMeetings] = useState<OwedMeeting[]>([]);
  const sent = useRef(0);
  const endTimer = useRef<number | undefined>(undefined);

  // After every read, one timer for the next meeting to end, so each end is waited for once. The
  // newest read's answer is the one that sets it; a read of the calendar that fails sets none (the
  // next notice or tab change starts it again).
  const reload = useCallback(function run() {
    const n = ++sent.current;
    owedMeetings()
      .then((list) => {
        if (n === sent.current && Array.isArray(list)) setMeetings(list);
      })
      .catch(() => {
        // A hint: keep what was there.
      })
      .finally(() => {
        listCalendarEvents()
          .then((events) => {
            if (n !== sent.current) return;
            window.clearTimeout(endTimer.current);
            const wait = Array.isArray(events) ? untilNextEnd(events, Date.now()) : null;
            if (wait !== null) {
              endTimer.current = window.setTimeout(run, Math.min(wait, END_WATCH_MAX_MS));
            }
          })
          .catch(() => {});
      });
  }, []);

  // Gone with the strip (or while the app can't show it): nothing fires a read afterwards. Bumping
  // `sent` too makes a calendar read still in flight drop its answer instead of arming a timer
  // nothing would clear.
  useEffect(() => {
    if (enabled) return;
    sent.current++;
    window.clearTimeout(endTimer.current);
  }, [enabled]);
  useEffect(
    () => () => {
      sent.current++;
      window.clearTimeout(endTimer.current);
    },
    [],
  );

  useEffect(() => {
    if (enabled) reload();
  }, [enabled, watch, reload]);

  const timer = useRef<number | undefined>(undefined);
  useEffect(() => {
    if (!enabled) return;
    const soon = () => {
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(reload, REFRESH_DELAY_MS);
    };
    const offs = [
      subscribeUntilCleanup(() => onPeopleChanged(soon)),
      subscribeUntilCleanup(() => onCalendarSynced(soon)),
    ];
    return () => {
      offs.forEach((off) => off());
      window.clearTimeout(timer.current);
    };
  }, [enabled, reload]);

  return { meetings, reload };
}
