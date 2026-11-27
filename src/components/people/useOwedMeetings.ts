// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The meetings owed an answer, as the app shows them: the strip, the People row's badge and the top
// of the People tab's All view all read this one list. PM works it out whenever it looks (nothing
// fires when an event ends), so it is read on mount, whenever `watch` moves (the open tab), and when
// a calendar sync or a change to People says the answer may have moved. A reply that lands after a
// newer request was sent is dropped, and a failed read keeps the list from before: the strip is a
// hint, never an error.

import { useCallback, useEffect, useRef, useState } from "react";
import { onCalendarSynced, onPeopleChanged, owedMeetings } from "../../lib/ipc";
import { subscribeUntilCleanup } from "../../lib/subscribe";
import type { OwedMeeting } from "../../lib/types";

/** How long after a notice before the list is read again (a sync and its people notice are one read). */
const REFRESH_DELAY_MS = 300;

export interface OwedMeetings {
  meetings: OwedMeeting[];
  /** Read again now. Stable. */
  reload: () => void;
}

export function useOwedMeetings(enabled: boolean, watch?: unknown): OwedMeetings {
  const [meetings, setMeetings] = useState<OwedMeeting[]>([]);
  const sent = useRef(0);

  const reload = useCallback(() => {
    const n = ++sent.current;
    owedMeetings()
      .then((list) => {
        if (n === sent.current && Array.isArray(list)) setMeetings(list);
      })
      .catch(() => {
        // A hint: keep what was there.
      });
  }, []);

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
