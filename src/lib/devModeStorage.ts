// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// This webview's own copy of the Developer-mode switch (`capabilities.tsx` reads and writes it).
// Kept apart from that file so a join can switch it off without pulling the provider in.

/** The `localStorage` key of this device's developer-mode switch. */
export const DEV_MODE_LS_KEY = "pm:dev:mode";

/** Developer mode off on THIS device only: this webview's own switch, never the `dev_mode` row. A
 *  shared vault's store belongs to its other accounts and machines too, so a join must not write
 *  it, and the provider's hydration would otherwise take another device's "on" from it on a fresh
 *  profile. Called right before the post-join reload (`markJustJoinedVault`). */
export function switchDevModeOffOnThisDevice(): void {
  try {
    localStorage.setItem(DEV_MODE_LS_KEY, "false");
  } catch {
    /* locked-down webview: the switch just won't persist on this device */
  }
}
