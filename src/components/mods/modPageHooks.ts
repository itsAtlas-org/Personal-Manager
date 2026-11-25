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

/** What `ui.openTaskEditor` opens: an existing task by id, or a new one starting from text the mod
 *  prefilled (title, project, date) that the editor shows as the value of its own fields. */
export type TaskEditorRequest =
  | { id: number }
  | { new: { title: string | null; project: string | null; deadline: string | null } };

export interface ModPageHooks {
  /** Show PM's key field for `ask`'s slot of `from`. The key goes from the field to PM's backend
   *  and nowhere else; the hook is given no way to hand it to the frame. */
  askSecret?: (from: ModSource, ask: SecretAsk) => void;
  /** Show `notice` as a strip naming `from`. */
  showNotice?: (from: ModSource, notice: Notice) => void;
  /** Open PM's own task editor on `request` for `from`. Returns false when an editor is already
   *  open (one at a time): the frame is then told it is asking too fast. The provider (the app's
   *  root, beside the reader) draws it; the call does not wait for the editor to close. */
  openTaskEditor?: (from: ModSource, request: TaskEditorRequest) => boolean;
  /** Take the person to where the document engine is set up (the Documents tab). The frame offers it
   *  as "Set it up" after a call answered `MOD-5007`; PM provisions nothing for a mod. */
  openEngineSetup?: () => void;
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

const isNullableText = (v: unknown, max: number): v is string | null =>
  v === null || (typeof v === "string" && v.length > 0 && v.length <= max);

/** The backend's `ui.openTaskEditor` arguments, read as untrusted. */
export function readTaskEditorRequest(args: unknown): TaskEditorRequest {
  const a = (args ?? {}) as Record<string, unknown>;
  if (typeof a.id === "number") {
    if (!Number.isSafeInteger(a.id) || a.id <= 0) throw new Error("ui.openTaskEditor: not a task");
    return { id: a.id };
  }
  const n = (a.new ?? null) as Record<string, unknown> | null;
  if (
    n === null ||
    typeof n !== "object" ||
    !isNullableText(n.title, 1000) ||
    !isNullableText(n.project, 1000) ||
    !isNullableText(n.deadline, 10)
  ) {
    throw new Error("ui.openTaskEditor: not a task to start from");
  }
  return { new: { title: n.title, project: n.project, deadline: n.deadline } };
}
