// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Between full syncs, changes made in Google show within about half a minute (#884, F3): while the
// window is on screen, a cheap check asks Google which calendars changed and fetches only those (the
// backend spaces checks out and skips while a sync runs). Hidden or in the tray it pauses; coming back
// into view or into focus checks at once. The views re-read on `calendar://synced`.
//
// A hook so the pop-out Calendar can run it too: with PM's main window hidden in the tray the pop-out
// may be the only one on screen, and each window pauses on ITS OWN visibility, so main's check would
// not be running for it. Two windows asking is safe: the backend is single-flight and keeps checks at
// least 10 s apart.

import { useEffect } from "react";
import { nextCheckDelay, shouldCheck } from "./calendarPoll";
import { checkGoogleCalendars } from "./ipc";

/**
 * Run the ~30 s Google change check while `enabled` is true.
 *
 * @param onTeardown the app's "PM is being erased" subscription (`lib/teardown`), for the window that
 *   has one: it ends the schedule when the wipe starts. The pop-out doesn't pass it, since it is
 *   destroyed before the wipe runs.
 */
export function useCalendarChangeCheck(
  enabled: boolean,
  onTeardown?: (stop: () => void) => () => void,
): void {
  useEffect(() => {
    if (!enabled) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let stopped = false;
    const check = () => {
      if (!stopped && shouldCheck(document.visibilityState)) {
        void checkGoogleCalendars().catch(() => {
          // A provider being unreachable is surfaced by the full sync, not here.
        });
      }
    };
    const schedule = () => {
      timer = setTimeout(() => {
        check();
        if (!stopped) schedule();
      }, nextCheckDelay());
    };
    schedule();
    const onVisible = () => {
      if (document.visibilityState === "visible") check();
    };
    document.addEventListener("visibilitychange", onVisible);
    window.addEventListener("focus", check);
    const stop = () => {
      stopped = true;
      clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVisible);
      window.removeEventListener("focus", check);
    };
    const off = onTeardown?.(stop);
    return () => {
      stop();
      off?.();
    };
  }, [enabled, onTeardown]);
}
