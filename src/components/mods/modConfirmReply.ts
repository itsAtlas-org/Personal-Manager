// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The question a MOD-2003 carries (PR C, K1 and K5), read from the backend's raw reply. It lives in its
// own file, not in `modBridge.ts`: the mod kit imports that file for its constants, so a change there is
// a change in every mod's bundle (D12), and this is PM's page alone.

import { MOD_ERRORS } from "./modBridge";

/** The question PM asks the person for a call: the page's own, never the frame's. `token` is good once
 *  and for a minute and is only ever sent back to `mod_confirm`; it is never handed to the frame.
 *  `summary` is PM's text: line 1 the question, later lines an address or a clamp note, drawn as plain
 *  text. */
export interface Confirmation {
  token: string;
  summary: string;
}

/** The most characters of a summary the page keeps (the backend's own cap is far below it). */
export const MAX_CONFIRM_SUMMARY_CHARS = 2_000;

/** The `confirm` of a backend reply that is a `MOD-2003`, or null. PM's token is 128 bits as 32
 *  lowercase hex digits and its summary is text; anything else is not a question PM asked, so the call
 *  is answered as the refusal it is and nothing is drawn. */
export function confirmationOf(reply: unknown): Confirmation | null {
  if (typeof reply !== "object" || reply === null) return null;
  const { kind, error } = reply as { kind?: unknown; error?: unknown };
  if (kind !== "error" || typeof error !== "object" || error === null) return null;
  const { code, confirm } = error as { code?: unknown; confirm?: unknown };
  if (code !== MOD_ERRORS.needsConfirmation) return null;
  if (typeof confirm !== "object" || confirm === null) return null;
  const { token, summary } = confirm as { token?: unknown; summary?: unknown };
  if (typeof token !== "string" || !/^[0-9a-f]{32}$/.test(token)) return null;
  if (typeof summary !== "string" || summary.length === 0) return null;
  return { token, summary: summary.slice(0, MAX_CONFIRM_SUMMARY_CHARS) };
}
