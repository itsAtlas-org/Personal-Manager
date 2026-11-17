// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The developer's loaded test folders (`listTestMods`), for Settings → Developer: each one's problem
// (why the last reload didn't take) and whether PM is watching it. Read on mount and on every
// `mods://changed`: the spec names cause `test-mod` (a load, a reload or an unload), but a test mod
// also goes with an uninstall, a pause or developer mode switching off, and the read is a few rows in
// memory, so every cause asks again rather than the page keeping a list of which ones matter.
// Answers are applied in the order they were asked; a failed read keeps what is shown and says why.

import { useCallback, useEffect, useRef, useState } from "react";
import { listTestMods, onModsChanged } from "./ipc";
import { failureText, type TestModStatus } from "./modsTypes";
import { subscribeUntilCleanup } from "./subscribe";

interface Read {
  tests: readonly TestModStatus[] | null;
  problem: string | null;
}

export interface TestMods extends Read {
  /** Read the list again now. Never rejects. */
  refresh: () => Promise<void>;
}

export function useTestMods(): TestMods {
  const [read, setRead] = useState<Read>({ tests: null, problem: null });
  const asked = useRef(0);
  // An answer that lands after the page is gone belongs to a page that is no longer asking.
  const alive = useRef(true);

  const refresh = useCallback(async () => {
    const mine = ++asked.current;
    try {
      const tests = await listTestMods();
      if (alive.current && mine === asked.current) setRead({ tests, problem: null });
    } catch (e) {
      if (alive.current && mine === asked.current) {
        const problem = failureText(e, "PM couldn't read the list of test mods.");
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

  return { ...read, refresh };
}
