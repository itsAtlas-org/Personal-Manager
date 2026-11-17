// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What a mod has put on PM's own surface through its two page calls (`notify`, `secrets.set`), kept
// outside React for the same reason `modActions` is: a mod's frame is rebuilt on every tab switch, but a
// notice it posted, or a key field it asked for, has to outlive the frame that asked, and the strips are
// drawn in two places (App, and inside Settings under its scrim) that mount and unmount independently.
//
// Everything here is IN MEMORY: a notice or an open key field never reaches a database, a log or a
// backup, and a restart or a lock empties it. Nothing a frame said is kept as it was said. The source
// (id, name, tier) is PM's own record of the mod (ModFrame passes it, never anything from the frame),
// and the notice's text was already held to one plain line by the backend.
//
//   notices    at most MAX_NOTICES_PER_MOD live per mod: the newest replaces the oldest, so a mod that
//              posts all day (the backend allows five a day) can never push another mod's notice, or
//              PM's own strips, off the screen. Dismissing is the only way a notice goes (besides the
//              mod being removed, or the vault changing).
//   key asks   ONE open key field per mod. An ask that comes while that mod's field is already open is
//              ignored: swapping the slot under a person who is typing would be a way to redirect a key.
//
// A mod that is no longer installed has nothing here, nor a key field for a slot it dropped
// (`keepOnlyMods`), and a vault that goes away takes everything with it (`forgetModSurface`).

import { useSyncExternalStore } from "react";
import type { ModPageHooks, ModSource, Notice, SecretAsk } from "../components/mods/modPageHooks";

/** Live notices one mod may have at once. A UI limit (the backend's per-day count is separate). */
export const MAX_NOTICES_PER_MOD = 3;

export interface ModNoticeItem {
  id: number;
  from: ModSource;
  notice: Notice;
}

export interface ModKeyAsk {
  from: ModSource;
  /** The slot's name; its host and header are read from the installed mod when the field is drawn. */
  slot: string;
}

let nextId = 1;
let notices: readonly ModNoticeItem[] = [];
let asks: ReadonlyMap<string, ModKeyAsk> = new Map();
const listeners = new Set<() => void>();

function changed() {
  for (const l of [...listeners]) l();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Show `notice` for `from`, dropping that mod's oldest live notice if it would have more than
 *  MAX_NOTICES_PER_MOD. */
export function addNotice(from: ModSource, notice: Notice): void {
  const mine = notices.filter((n) => n.from.modId === from.modId);
  const drop = new Set(
    mine.slice(0, Math.max(0, mine.length + 1 - MAX_NOTICES_PER_MOD)).map((n) => n.id),
  );
  notices = [...notices.filter((n) => !drop.has(n.id)), { id: nextId++, from, notice }];
  changed();
}

export function dismissNotice(id: number): void {
  if (!notices.some((n) => n.id === id)) return;
  notices = notices.filter((n) => n.id !== id);
  changed();
}

export function dismissAllNotices(): void {
  if (notices.length === 0) return;
  notices = [];
  changed();
}

/** Open the key field for `ask`'s slot of `from`, unless that mod's field is already open. */
export function openKeyAsk(from: ModSource, ask: SecretAsk): void {
  if (asks.has(from.modId)) return;
  const next = new Map(asks);
  next.set(from.modId, { from, slot: ask.name });
  asks = next;
  changed();
}

export function closeKeyAsk(modId: string): void {
  if (!asks.has(modId)) return;
  const next = new Map(asks);
  next.delete(modId);
  asks = next;
  changed();
}

/** Forget everything (the vault that posted it went away). */
export function forgetModSurface(): void {
  if (notices.length === 0 && asks.size === 0) return;
  notices = [];
  asks = new Map();
  changed();
}

/** Forget what belongs to a mod that is not installed any more, and any key field for a slot its
 *  manifest no longer declares. `mods` is each installed mod's id with the names of its key slots. */
export function keepOnlyMods(mods: ReadonlyMap<string, readonly string[]>): void {
  const keptNotices = notices.filter((n) => mods.has(n.from.modId));
  const keptAsks = new Map(
    [...asks].filter(([id, ask]) => mods.get(id)?.includes(ask.slot) ?? false),
  );
  if (keptNotices.length === notices.length && keptAsks.size === asks.size) return;
  notices = keptNotices;
  asks = keptAsks;
  changed();
}

/** The live notices, oldest first. */
export function useModNotices(): readonly ModNoticeItem[] {
  return useSyncExternalStore(subscribe, () => notices);
}

/** The open key fields, one per mod, oldest first. */
export function useModKeyAsks(): readonly ModKeyAsk[] {
  const map = useSyncExternalStore(subscribe, () => asks);
  return [...map.values()];
}

/** What the two page calls do in the main window: the hooks `ModFrame` reads from its context. */
export const modPageHooks: ModPageHooks = {
  askSecret: openKeyAsk,
  showNotice: addNotice,
};
