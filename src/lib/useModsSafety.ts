// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the Mods tab knows about the safety list (Mods spec §1.10): the strips, how fresh the list is,
// and whether this machine's record of it is damaged. The backend derives all of it from the rows,
// its notes and the cached index, so the status is read:
//
//   - on mount, because `mods://notice` can fire before this view exists (the safety pass runs at
//     launch, and a tab switch unmounts the view), and
//   - on `mods://notice` and on every `mods://changed`, as a cue to read it again. The event's own
//     payload is not drawn: the status is the one source, so a strip can't be drawn from an event the
//     backend has since taken back (a paused mod that was resumed a second later).
//
// Answers are applied in the order the questions were asked, and a failed read keeps what was shown
// and says why.

import { useCallback, useEffect, useRef, useState } from "react";
import { modsSafetyStatus, onModsChanged, onModsNotice } from "./ipc";
import { failureText, type ModsSafetyStatus } from "./modsTypes";
import { subscribeUntilCleanup } from "./subscribe";

export interface SafetyRead {
  status: ModsSafetyStatus | null;
  /** Why the last read failed, in the backend's words, until one succeeds. */
  problem: string | null;
  refresh: () => Promise<void>;
}

export function useModsSafety(): SafetyRead {
  const [status, setStatus] = useState<ModsSafetyStatus | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const asked = useRef(0);
  const alive = useRef(true);

  const refresh = useCallback(async () => {
    const mine = ++asked.current;
    try {
      const next = await modsSafetyStatus();
      if (alive.current && mine === asked.current) {
        setStatus(next);
        setProblem(null);
      }
    } catch (e) {
      if (alive.current && mine === asked.current) {
        setProblem(failureText(e, "PM couldn't read the safety list's notices."));
      }
    }
  }, []);

  useEffect(() => {
    alive.current = true;
    void refresh();
    const stopNotice = subscribeUntilCleanup(() => onModsNotice(() => void refresh()));
    const stopChanged = subscribeUntilCleanup(() => onModsChanged(() => void refresh()));
    return () => {
      alive.current = false;
      stopNotice();
      stopChanged();
    };
  }, [refresh]);

  return { status, problem, refresh };
}
