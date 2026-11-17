// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the Mods tab has in flight, kept outside the view. PM's tab router unmounts a view on every
// switch, but a command already sent to the backend keeps going: an uninstall that was asked for
// finishes whether or not the Mods tab is on screen. Held in the view's own state, the busy flag, the
// refusal and the "removed" confirmation would all vanish with the view, and the user coming back
// would see a row that looks idle, press the button again and ask for the same thing twice.
//
// So the store lives at module level:
//
//   busy     one action at a time per key (a mod's id for its row, `strip:…` for a strip). A second
//            request for a busy key is refused here, whichever mount asks, until the first settles.
//   errors   the backend's sentence for the last refusal, until the key's next action or a dismissal.
//   notes    what an action wants said that no row can say any more (a removed mod's row is gone):
//            dismissible lines at the top of the Mods tab.
//   screens  a consent screen the backend staged for a mod (an install or update to approve): its
//            token is good for a few minutes, so the screen is kept for the person to come back to,
//            and a stale one is answered by the backend's own `consent-expired`.
//   restarts how to get a fresh screen for a staged one (read the file again, download again), so
//            an expired screen can offer "Start again" instead of a dead end.
//   expired  the staged screens the backend has said are out of date (nothing more can be done with
//            their token).
//
// `run` follows its action to the end whether or not anything is mounted: the follow-up (pruning an
// uninstalled mod's tab and folds, reading the rows again) is part of the action, not of a component.

import { useSyncExternalStore } from "react";
import type { ConsentScreen } from "./modsTypes";
import { failureText } from "./modsTypes";

export interface ModNote {
  id: number;
  tone: "info" | "warning";
  text: string;
}

export interface ModActionsSnapshot {
  /** Keys with an action in flight, each with what it is doing (a few words, for a button). */
  busy: ReadonlyMap<string, string>;
  errors: ReadonlyMap<string, string>;
  notes: readonly ModNote[];
  screens: ReadonlyMap<string, ConsentScreen>;
  restarts: ReadonlyMap<string, ScreenRestart>;
  expired: ReadonlySet<string>;
}

/** Asks the backend for a fresh consent screen for the same thing (the file read again, the mod
 *  downloaded again). */
export type ScreenRestart = () => Promise<ConsentScreen>;

const EMPTY: ModActionsSnapshot = {
  busy: new Map(),
  errors: new Map(),
  notes: [],
  screens: new Map(),
  restarts: new Map(),
  expired: new Set(),
};
let snapshot: ModActionsSnapshot = EMPTY;
let nextNote = 1;
const listeners = new Set<() => void>();

function publish(next: Partial<ModActionsSnapshot>): void {
  snapshot = { ...snapshot, ...next };
  for (const l of [...listeners]) l();
}

function withKey<V>(map: ReadonlyMap<string, V>, key: string, value: V | null): Map<string, V> {
  const next = new Map(map);
  if (value === null) next.delete(key);
  else next.set(key, value);
  return next;
}

export type ModActionResult<T> = { ok: true; value: T } | { ok: false; error: string | null };

/**
 * Run one action under `key`. Resolves `{ok: false, error: null}` without running it when the key is
 * already busy (the earlier action is still going), `{ok: false, error}` with the backend's sentence
 * when it refuses, and `{ok: true, value}` when it works. `after` runs on success, still inside the
 * action: it is how a follow-up survives the view that asked for it going away.
 */
export async function runModAction<T>(
  key: string,
  doing: string,
  failure: string,
  action: () => Promise<T>,
  after?: (value: T) => void,
): Promise<ModActionResult<T>> {
  if (snapshot.busy.has(key)) return { ok: false, error: null };
  publish({
    busy: withKey(snapshot.busy, key, doing),
    errors: withKey(snapshot.errors, key, null),
  });
  try {
    const value = await action();
    try {
      after?.(value);
    } catch {
      // A follow-up that fails (a storage write) must not make a finished action read as failed.
    }
    publish({ busy: withKey(snapshot.busy, key, null) });
    return { ok: true, value };
  } catch (e) {
    const error = failureText(e, failure);
    publish({
      busy: withKey(snapshot.busy, key, null),
      errors: withKey(snapshot.errors, key, error),
    });
    return { ok: false, error };
  }
}

export function clearModError(key: string): void {
  if (snapshot.errors.has(key)) publish({ errors: withKey(snapshot.errors, key, null) });
}

export function addModNote(tone: ModNote["tone"], text: string): void {
  publish({ notes: [...snapshot.notes, { id: nextNote++, tone, text }] });
}

export function dismissModNote(id: number): void {
  publish({ notes: snapshot.notes.filter((n) => n.id !== id) });
}

/** Keep the consent screen the backend staged for `modId`, or drop it with `null`. A new screen is
 *  never expired; `restart` is how to get another like it if this one runs out. */
export function setModScreen(
  modId: string,
  screen: ConsentScreen | null,
  restart?: ScreenRestart,
): void {
  const expired = new Set(snapshot.expired);
  expired.delete(modId);
  publish({
    screens: withKey(snapshot.screens, modId, screen),
    restarts: withKey(snapshot.restarts, modId, screen && restart ? restart : null),
    expired,
  });
}

/** The backend said the staged screen for `modId` is out of date: its token can't be used again. */
export function markScreenExpired(modId: string): void {
  if (!snapshot.screens.has(modId) || snapshot.expired.has(modId)) return;
  publish({ expired: new Set(snapshot.expired).add(modId) });
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** What is in flight right now, for code that is not a component (and for tests). */
export function currentModActions(): ModActionsSnapshot {
  return snapshot;
}

export function useModActions(): ModActionsSnapshot {
  return useSyncExternalStore(
    subscribe,
    () => snapshot,
    () => snapshot,
  );
}

/** Forget everything (a test's reset; nothing in the app calls it). */
export function resetModActions(): void {
  snapshot = EMPTY;
  nextNote = 1;
  for (const l of [...listeners]) l();
}
