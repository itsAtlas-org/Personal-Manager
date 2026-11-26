// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useRef, useState } from "react";

import { directProvidersState } from "./ipc";
import type { DirectProvidersState } from "./types";

export interface UseDirectProviders {
  /** null until the backend has answered, or if it couldn't. Every reader treats null as "no provider
   *  to offer", which is the screen as it was before your own keys existed. */
  state: DirectProvidersState | null;
  error: string | null;
  /** Read it again (one cheap DB read, no network) and return what came back. Called after every
   *  write: the UI never works out a move or a verdict itself, it asks again. */
  reload: () => Promise<DirectProvidersState | null>;
}

/** The read model behind the "Your own keys" section, the role blocks and the first-run card. One
 *  instance per surface, shared down by props so the card and the roles never disagree. */
export function useDirectProviders(enabled = true): UseDirectProviders {
  const [state, setState] = useState<DirectProvidersState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const alive = useRef(true);

  const reload = useCallback(async () => {
    try {
      const next = await directProvidersState();
      if (alive.current) {
        setState(next);
        setError(null);
      }
      return next;
    } catch (e) {
      if (alive.current) setError(String(e));
      return null;
    }
  }, []);

  useEffect(() => {
    alive.current = true;
    // A surface that only sometimes needs the read (Settings, which shows the first-run card only
    // while onboarding) asks for none the rest of the time.
    if (enabled) void reload();
    return () => {
      alive.current = false;
    };
  }, [reload, enabled]);

  return { state, error, reload };
}
