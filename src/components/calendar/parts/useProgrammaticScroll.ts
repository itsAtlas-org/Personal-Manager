// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// PM's own scrolls of the Month and Year streams (a cursor move from outside: ‹ ›, a key, Today, the
// mini-calendar). While one runs, the stream's scroll events are PM's, and must not be reported back
// as the user moving the cursor, or the view chases its own glide. `programmaticRef` is that guard.
//
// Latest wins: a held key starts a new scroll before the last has ended, and the guard belongs to the
// newest. An older call's 700 ms safety timer used to free it mid-glide, and the month the glide was
// passing then overwrote the one the key had stepped to.

import { useCallback, useEffect, useRef } from "react";
import { scrollBehavior } from "../../../theme";

/** How long to wait for `scrollend` before freeing the guard anyway (it doesn't fire when the scroll
 *  is already at its target). */
const SAFETY_MS = 700;

export function useProgrammaticScroll() {
  const programmaticRef = useRef(false);
  const tokenRef = useRef(0);
  // Drops the last call's `scrollend` listener and timer, leaving the guard as it is.
  const dropRef = useRef<(() => void) | null>(null);

  const scrollTo = useCallback((el: HTMLElement, top: number, smooth: boolean) => {
    dropRef.current?.();
    const token = ++tokenRef.current;
    programmaticRef.current = true;
    el.scrollTo({ top, behavior: scrollBehavior(smooth) });
    let timer = 0;
    const drop = () => {
      el.removeEventListener("scrollend", release);
      window.clearTimeout(timer);
    };
    const release = () => {
      drop();
      // Only the newest scroll frees the guard.
      if (tokenRef.current === token) programmaticRef.current = false;
    };
    el.addEventListener("scrollend", release);
    timer = window.setTimeout(release, SAFETY_MS);
    dropRef.current = drop;
  }, []);

  useEffect(() => () => dropRef.current?.(), []);

  return { programmaticRef, scrollTo };
}
