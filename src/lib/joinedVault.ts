// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The one-shot "just joined a shared vault" flag (issue #337), following the mapPrefs
// pattern for shared localStorage keys. Set right before the post-join reload; read by
// onboarding-adjacent surfaces (the Connectors tab banner) to explain, once, what stays
// personal on this account. localStorage lives in the per-Windows-user webview profile,
// so the flag can never leak to the vault owner's side. Dismissing clears it for good.

import { switchDevModeOffOnThisDevice } from "./devModeStorage";

const JUST_JOINED_VAULT_KEY = "pm:justJoinedVault";

/** Mark this profile as having just joined a shared vault (called before the reload). Joining also
 *  switches this device's developer mode off (spec §1.15: adopting a shared vault switches it
 *  off): written to this webview's own switch, not the shared store, which holds the choice of
 *  the vault's other devices. Both join paths (the boot gate and the vault card) call this before
 *  they reload. */
export function markJustJoinedVault() {
  switchDevModeOffOnThisDevice();
  localStorage.setItem(JUST_JOINED_VAULT_KEY, "1");
}

/** Whether the one-shot joined-vault explanation is still owed on this profile. */
export function justJoinedVault(): boolean {
  return localStorage.getItem(JUST_JOINED_VAULT_KEY) === "1";
}

/** Clear the flag (the user dismissed the explanation). */
export function clearJustJoinedVault() {
  localStorage.removeItem(JUST_JOINED_VAULT_KEY);
}
