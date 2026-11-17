// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What PM's own surface does for the two experimental calls that put something on screen: `secrets.set`
// (the field where the person types a mod's key) and `notify` (a strip with the mod's name). The
// backend authorises each and counts it (Rust holds the ask and daily limits), then ModFrame performs
// it here, synchronously, from arguments the backend wrote; the frame's own words never reach this
// file. The drawing itself belongs to whoever provides the hooks (ModSecretFields and ModNoticeStrips,
// provided at the app's root): until a provider exists the calls answer as PM's own failure, never as
// a success that showed nothing.

import { createContext } from "react";

/** The mod a call came from, as PM's own record has it (never anything the frame said). */
export interface ModSource {
  modId: string;
  name: string;
  tier: "first" | "second" | "third";
}

/** One key field to show: the manifest's slot, where its key will go, and whether PM holds one. */
export interface SecretAsk {
  name: string;
  host: string;
  header: string;
  set: boolean;
}

export const NOTICE_TONES = ["info", "warn", "good"] as const;
export type NoticeTone = (typeof NOTICE_TONES)[number];

/** The most characters a notice carries (the backend's `NOTIFY_MAX_CHARS`, held equal by a test). */
export const NOTICE_MAX_CHARS = 140;

export interface Notice {
  text: string;
  tone: NoticeTone;
}

export interface ModPageHooks {
  /** Show PM's key field for `ask`'s slot of `from`. The key goes from the field to PM's backend
   *  and nowhere else; the hook is given no way to hand it to the frame. */
  askSecret?: (from: ModSource, ask: SecretAsk) => void;
  /** Show `notice` as a strip naming `from`. */
  showNotice?: (from: ModSource, notice: Notice) => void;
}

export const ModPageHooksContext = createContext<ModPageHooks>({});

const isText = (v: unknown, max: number): v is string =>
  typeof v === "string" && v.length > 0 && v.length <= max;

/** The backend's `secrets.set` arguments, read as untrusted (they crossed IPC as JSON). */
export function readSecretAsk(args: unknown): SecretAsk {
  const a = (args ?? {}) as Record<string, unknown>;
  if (
    !isText(a.name, 64) ||
    !isText(a.host, 253) ||
    !isText(a.header, 64) ||
    typeof a.set !== "boolean"
  ) {
    throw new Error("secrets.set: not a key slot");
  }
  return { name: a.name, host: a.host, header: a.header, set: a.set };
}

/** The backend's `notify` arguments, read as untrusted. The backend already held the text to one
 *  plain line; this only refuses what is not a notice at all. */
export function readNotice(args: unknown): Notice {
  const a = (args ?? {}) as Record<string, unknown>;
  const tone = NOTICE_TONES.find((t) => t === a.tone);
  // Counted in characters, as the backend counts them (a code point, not a UTF-16 unit).
  if (!isText(a.text, NOTICE_MAX_CHARS * 2) || [...a.text].length > NOTICE_MAX_CHARS || !tone) {
    throw new Error("notify: not a notice");
  }
  return { text: a.text, tone };
}
