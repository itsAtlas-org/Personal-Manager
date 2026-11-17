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
// - A store that opens (`loaded`) may be a different vault's data: this provider sits above the tree
//   a lock or a vault switch replaces, so it is what remembers the old vault. `loaded` empties the
//   rows (a failed read would otherwise keep the old vault's mods and their sidebar tabs; `loaded:
//   false` is also what tells App not to treat an open mod tab as gone while the rows are unknown),
//   and drops what the Mods tab remembered about the vault that went away (modActions
//   `forgetVaultState`: its notes, staged install screens and refusals). `recovered` is NOT a
//   switch: the background pass sends it seconds after `loaded`, for the same vault, and the install
//   screens staged meanwhile are still good, so it only reads again. `loaded` also drops the notices
//   and key fields mods put on screen (modSurface), and every read drops those of a mod that is gone.

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { listMods, onModsChanged } from "./ipc";
import { forgetVaultState } from "./modActions";
import { ModRowsContext, type ModRows } from "./modRows";
import { forgetModSurface, keepOnlyMods } from "./modSurface";
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
      if (alive.current && mine === asked.current) {
        setRead({ rows, loaded: true, problem: null });
        // A mod that is gone (or a key slot it dropped) takes its notices and key field with it.
        keepOnlyMods(new Map(rows.map((r) => [r.id, r.secrets.map((s) => s.name)])));
      }
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
    const stop = subscribeUntilCleanup(() =>
      onModsChanged((e) => {
        if (e.cause === "loaded") {
          forgetVaultState();
          forgetModSurface();
          setRead({ rows: [], loaded: false, problem: null });
        }
        void refresh();
      }),
    );
    return () => {
      alive.current = false;
      stop();
    };
  }, [refresh]);

  const value = useMemo<ModRows>(() => ({ ...read, refresh }), [read, refresh]);
  return <ModRowsContext.Provider value={value}>{children}</ModRowsContext.Provider>;
}
