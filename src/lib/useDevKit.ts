// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Developer kit's readouts for Settings → Developer (Mods spec §1.15): one read, `devKitInfo`, of
// everything the page shows (API version, the dispatch table, the limits, the web engine's seal and
// each frame's event counters), kept in step by asking again.
//
// - Only while developer mode is on. The backend refuses the read otherwise (`dev-mode-off`), so with it
//   off nothing is asked and nothing is kept: switching it on again starts from a fresh read, not from
//   what the last time left behind.
// - Asked on mount and on every `mods://changed` (a frame ending, a mod loaded or paused changes the
//   sessions list). Answers are applied in the order the questions were asked, so a slow read can't
//   overwrite a newer one, and one that lands after the page is gone is dropped.
// - A failed read keeps what is shown and says why in the backend's words.

import { useCallback, useEffect, useRef, useState } from "react";
import { devKitInfo, onModsChanged } from "./ipc";
import { failureText, type DevKitInfo } from "./modsTypes";
import { subscribeUntilCleanup } from "./subscribe";

interface Read {
  kit: DevKitInfo | null;
  problem: string | null;
}

const NONE: Read = { kit: null, problem: null };

export interface DevKit extends Read {
  /** Read it again now. Never rejects. */
  refresh: () => Promise<void>;
}

export function useDevKit(on: boolean): DevKit {
  const [read, setRead] = useState<Read>(NONE);
  const asked = useRef(0);
  // False while developer mode is off or the page is gone: an answer that lands then belongs to a
  // page that is no longer asking.
  const alive = useRef(false);

  const refresh = useCallback(async () => {
    const mine = ++asked.current;
    try {
      const kit = await devKitInfo();
      if (alive.current && mine === asked.current) setRead({ kit, problem: null });
    } catch (e) {
      if (alive.current && mine === asked.current) {
        const problem = failureText(e, "PM couldn't read the developer kit.");
        setRead((prev) => ({ ...prev, problem }));
      }
    }
  }, []);

  useEffect(() => {
    if (!on) return;
    alive.current = true;
    void refresh();
    const stop = subscribeUntilCleanup(() => onModsChanged(() => void refresh()));
    return () => {
      alive.current = false;
      stop();
      setRead(NONE);
    };
  }, [on, refresh]);

  return { ...(on ? read : NONE), refresh };
}
