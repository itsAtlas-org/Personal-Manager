// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What PM's strips about the Pinboard's move into its mod say (PR C, K8). The backend works the state
// out from its rows (`pinboardMoveState`: a conflict between two boards, a copy with characters
// replaced, a board a restore put back, a board that cannot move yet), so it is read on mount,
// because the move can happen at launch before this exists, and again on every `mods://changed`,
// which an install, a settle and the strips' own answers all send. It answers all-false unless a
// move or a restore left a note, so most launches draw nothing.
//
// A failed read (a locked vault, another window writing) keeps what was shown; the next change reads
// again. The state holds booleans and a word, never the board's own text.

import { useCallback, useEffect, useRef, useState } from "react";
import { onModsChanged, pinboardMoveState } from "./ipc";
import type { PinboardMoveState } from "./modsTypes";
import { subscribeUntilCleanup } from "./subscribe";

export function usePinboardMove(): {
  state: PinboardMoveState | null;
  refresh: () => Promise<void>;
} {
  const [state, setState] = useState<PinboardMoveState | null>(null);
  const asked = useRef(0);
  const alive = useRef(true);

  const refresh = useCallback(async () => {
    const mine = ++asked.current;
    try {
      const next = await pinboardMoveState();
      if (alive.current && mine === asked.current) setState(next);
    } catch {
      /* locked, or another window writes: read again on the next change */
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

  return { state, refresh };
}
