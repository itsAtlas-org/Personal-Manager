// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Reads the installed mods for the whole main window (modRows.ts says why one read is shared).
//
// - On mount, and on every `mods://changed` whatever its cause: the page's answer is the same for
//   all of them (spec: read the rows again). A listener that missed an event reads on mount anyway.
// - Answers are applied in the order the questions were asked, so a slow read that started before
//   a change can't overwrite the read the change caused.
// - A failed read (the vault is locked, another PM window is the writer) keeps the rows it has and
//   records why; the next `mods://changed` (the store opening sends `loaded`) reads again.

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { listMods, onModsChanged } from "./ipc";
import { ModRowsContext, type ModRows } from "./modRows";
import { failureText, type ModInfo } from "./modsTypes";
import { subscribeUntilCleanup } from "./subscribe";

interface Read {
  rows: readonly ModInfo[];
  loaded: boolean;
  problem: string | null;
}

export function ModRowsProvider({ children }: { children: ReactNode }) {
  const [read, setRead] = useState<Read>({ rows: [], loaded: false, problem: null });
  const asked = useRef(0);
  const alive = useRef(true);

  const refresh = useCallback(async () => {
    const mine = ++asked.current;
    try {
      const rows = await listMods();
      if (alive.current && mine === asked.current) setRead({ rows, loaded: true, problem: null });
    } catch (e) {
      if (alive.current && mine === asked.current) {
        const problem = failureText(e, "PM couldn't read the list of mods.");
        setRead((prev) => ({ ...prev, problem }));
      }
    }
  }, []);

  useEffect(() => {
    alive.current = true;
    void refresh();
    const stop = subscribeUntilCleanup(() => onModsChanged(() => void refresh()));
    return () => {
      alive.current = false;
      stop();
    };
  }, [refresh]);

  const value = useMemo<ModRows>(() => ({ ...read, refresh }), [read, refresh]);
  return <ModRowsContext.Provider value={value}>{children}</ModRowsContext.Provider>;
}
