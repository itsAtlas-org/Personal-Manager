// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Which disclosure groups you folded, remembered on this device. Settings → Connectors' provider
// groups (This device, Google, Microsoft, Apple, Calendar subscriptions) used to reopen at their
// Depth default every time Settings did. Fold state is chrome, so it lives in localStorage like the
// calendar roster's fold (`storedPrefs` says why chrome stays there), never in the encrypted store.
//
// A group you never touched keeps following its default, so changing Depth still folds the groups
// you haven't chosen for; one you did keeps your choice.

import { useState } from "react";

const PREFIX = "pm.fold.";

/** The fold chosen for `key`: true open, false closed, null never chosen (or unreadable). */
export function readFold(key: string): boolean | null {
  try {
    const v = localStorage.getItem(PREFIX + key);
    return v === "open" ? true : v === "closed" ? false : null;
  } catch {
    return null;
  }
}

export function writeFold(key: string, open: boolean): void {
  try {
    localStorage.setItem(PREFIX + key, open ? "open" : "closed");
  } catch {
    // Best-effort: a failed write just means the fold isn't remembered on this device.
  }
}

/** Forget every fold that is `prefix` itself or a section of it (`prefix:<section>`). The Mods tab
 *  keys a mod's folds `mods.mod.<id>` and `mods.mod.<id>:<section>` and calls this when the mod is
 *  uninstalled, so a mod installed again later starts folded as the tab would have it, not as the
 *  last one was left. A `:` is required after the prefix, so `mods.mod.a` never takes `mods.mod.ab`. */
export function pruneFolds(prefix: string): void {
  if (prefix === "") return;
  try {
    const own = PREFIX + prefix;
    const doomed: string[] = [];
    for (let i = 0; i < localStorage.length; i++) {
      const k = localStorage.key(i);
      if (k !== null && (k === own || k.startsWith(own + ":"))) doomed.push(k);
    }
    for (const k of doomed) localStorage.removeItem(k);
  } catch {
    // Best-effort, like writeFold: a leftover fold is only a remembered choice.
  }
}

/** A remembered fold for `key`, open by `fallback` until it's chosen. */
export function useRememberedFold(
  key: string,
  fallback: boolean,
): [boolean, (open: boolean) => void] {
  const [chosen, setChosen] = useState<boolean | null>(() => readFold(key));
  const choose = (open: boolean) => {
    setChosen(open);
    writeFold(key, open);
  };
  return [chosen ?? fallback, choose];
}
