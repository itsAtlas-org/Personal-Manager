// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Whether the open vault has the Mods notice folded ("Don't show this again", modsDisclaimerShown.ts),
// and the way to bring it back. Settings > General is the one home of that control. The answer is read
// when the tab opens and again on the app-wide settings signal; a failed read is "not folded", so the
// row simply isn't offered (the notice itself shows in full, which is the safe side).

import { useCallback, useEffect, useRef, useState } from "react";
import { isHidden, showAgain } from "./modsDisclaimerShown";

export function useModsNoticeFolded(): { folded: boolean; showAgain: () => Promise<void> } {
  const [folded, setFolded] = useState(false);
  const alive = useRef(true);

  const refresh = useCallback(async () => {
    const now = await isHidden().catch(() => false);
    if (alive.current) setFolded(now);
  }, []);

  useEffect(() => {
    alive.current = true;
    void refresh();
    const onChanged = () => void refresh();
    window.addEventListener("pm:settings-changed", onChanged);
    return () => {
      alive.current = false;
      window.removeEventListener("pm:settings-changed", onChanged);
    };
  }, [refresh]);

  const bringBack = useCallback(async () => {
    try {
      await showAgain();
    } catch {
      // Best-effort; what is true is read again below.
    }
    // Say what is true, not what was asked: a write that failed leaves the row there to try again.
    await refresh();
  }, [refresh]);

  return { folded, showAgain: bringBack };
}
