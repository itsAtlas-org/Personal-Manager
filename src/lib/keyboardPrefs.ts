// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Whether PM's single-key shortcuts (the Calendar's t, c, Shift+C, j, k, n and p) are on. WCAG 2.2
// SC 2.1.4 (Character Key Shortcuts) asks for a way to turn such shortcuts off, since speech input
// can fire them from ordinary words and a stray key press can too. Bobby: "yes lets add a settings
// where reasonable to comply with wcga".
//
// A per-device preference in localStorage, next to the other `pm:a11y:*` axes: keyboards and
// assistive technology differ per machine, and it is not user content, so not the encrypted store.
// The Calendar reads it at the moment of the key press (as `swipeDays` is read at the swipe), so a
// change takes effect at once; what advertises the keys (tooltips, aria-keyshortcuts) follows
// through {@link useCharacterShortcuts}.

import { useSyncExternalStore } from "react";

const KEY = "pm:a11y:characterShortcuts";
const CHANGED_EVENT = "pm:settings-changed";

/** On unless it was turned off: a missing, unreadable or unrecognised value means on, so nothing
 *  changes for anyone who never opens the switch. */
export function readCharacterShortcuts(): boolean {
  try {
    return localStorage.getItem(KEY) !== "false";
  } catch {
    // Unavailable (a locked-down webview): the default.
    return true;
  }
}

function announce(): void {
  try {
    window.dispatchEvent(new Event(CHANGED_EVENT));
  } catch {
    /* non-browser context (tests) */
  }
}

export function writeCharacterShortcuts(on: boolean): void {
  try {
    localStorage.setItem(KEY, String(on));
  } catch {
    // Best-effort.
  }
  announce();
}

/** Whether the switch is where it starts (on). */
export function keyboardPrefsAreDefault(): boolean {
  return readCharacterShortcuts();
}

/** Back to the default: single-key shortcuts on. */
export function resetKeyboardPrefs(): void {
  try {
    localStorage.removeItem(KEY);
  } catch {
    // Best-effort.
  }
  announce();
}

// Settings opens over the still-mounted Calendar, and the pop-out Calendar is another window sharing
// this storage, so the switch is followed as it lands: the app's own write, a `storage` event from
// the other window, and, for a webview that doesn't deliver those between windows, the window being
// looked at again (the pattern CalendarView uses for its other per-device view state).
function subscribe(onChange: () => void): () => void {
  const onStorage = (e: StorageEvent) => {
    // A null key is storage being cleared.
    if (e.key === null || e.key === KEY) onChange();
  };
  const onVisible = () => {
    if (document.visibilityState === "visible") onChange();
  };
  window.addEventListener(CHANGED_EVENT, onChange);
  window.addEventListener("storage", onStorage);
  window.addEventListener("focus", onChange);
  document.addEventListener("visibilitychange", onVisible);
  return () => {
    window.removeEventListener(CHANGED_EVENT, onChange);
    window.removeEventListener("storage", onStorage);
    window.removeEventListener("focus", onChange);
    document.removeEventListener("visibilitychange", onVisible);
  };
}

/** Whether single-key shortcuts are on, live: what a tooltip or `aria-keyshortcuts` should say of
 *  them. A key press reads {@link readCharacterShortcuts} itself instead. */
export function useCharacterShortcuts(): boolean {
  return useSyncExternalStore(subscribe, readCharacterShortcuts, () => true);
}
