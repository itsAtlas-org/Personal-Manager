// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Counts one showing of the Mods disclaimer for the screen that uses it (modsDisclaimerShown.ts says
// what is counted and where it is kept).
//
// ONE count per time the screen opens, not per render or per effect run. React's StrictMode runs an
// effect, undoes it and runs it again on the same instance, and a read of the vault id is
// asynchronous, so the count is started once per "episode" (a stretch of `active` being true) and
// kept in a ref as a promise: the second run of the effect waits for the same promise instead of
// counting again, and the run that was undone simply never shows its answer. When `active` goes false
// and then true again (the gate's screen coming back after a vault change) that is a new opening and
// counts again; a screen that is unmounted and mounted again is a new instance and counts again.
//
// Returns null until the count is known: the screen shows the full text meanwhile and offers no box,
// so a slow or failed read can only show more, never less.

import { useEffect, useRef, useState } from "react";
import { beginShowing, type Showing } from "./modsDisclaimerShown";

export function useDisclaimerShowing(active: boolean, foldable: boolean): Showing | null {
  const episode = useRef<Promise<Showing> | null>(null);
  const [showing, setShowing] = useState<Showing | null>(null);

  useEffect(() => {
    if (!active) {
      episode.current = null;
      setShowing(null);
      return;
    }
    episode.current ??= beginShowing(foldable);
    let alive = true;
    void episode.current.then((s) => {
      if (alive) setShowing(s);
    });
    return () => {
      alive = false;
    };
  }, [active, foldable]);

  return showing;
}
