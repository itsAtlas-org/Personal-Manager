// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The signed catalogue for the Mods tab's Available section (Mods spec §1.13). The backend lists it
// from the cached index with, for each entry, whether it is installed, whether a newer version is
// listed and whether it may be installed now (and why not, in words). It is read on mount and again
// on every `mods://changed`, because what an install, an uninstall or the safety list does changes
// those marks. Answers are applied in the order the questions were asked; a failed read keeps what
// was shown and says why.

import { useCallback, useEffect, useRef, useState } from "react";
import { listCatalogue, onModsChanged } from "./ipc";
import { failureText, type Catalogue } from "./modsTypes";
import { subscribeUntilCleanup } from "./subscribe";

export interface CatalogueRead {
  catalogue: Catalogue | null;
  /** Why the last read failed, in the backend's words, until one succeeds. */
  problem: string | null;
  refresh: () => Promise<void>;
}

export function useCatalogue(): CatalogueRead {
  const [catalogue, setCatalogue] = useState<Catalogue | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const asked = useRef(0);
  const alive = useRef(true);

  const refresh = useCallback(async () => {
    const mine = ++asked.current;
    try {
      const next = await listCatalogue();
      if (alive.current && mine === asked.current) {
        setCatalogue(next);
        setProblem(null);
      }
    } catch (e) {
      if (alive.current && mine === asked.current) {
        setProblem(failureText(e, "PM couldn't read the catalogue."));
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

  return { catalogue, problem, refresh };
}
