// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Documents changed, refresh": turning PM's document events into one quiet notice. While the Map is
// open PM tells it when documents land (`documents.added`) or go (`documents.deleted`, also an archive),
// coalesced on PM's side to a few events for an import of thousands. The Map does NOT redraw on each:
// a layout under the person's cursor that jumps is worse than one that is a little stale. It waits for
// the events to stop, then raises a single notice (the UI's strip) and lets the person press Refresh.
//
// `missed` is PM's "I dropped some" (its outbox holds 256 unacknowledged events and says how many it
// had to drop in the NEXT event it delivers, any event, so the Map cannot trust its own list any more).
// The only honest answer is to list again, so a `missed` on any event is a notice too, whatever the
// event was. The SDK acknowledges events by itself (cumulatively, batched: src/kit/host.ts), so this
// module never acknowledges anything.
//
// A session that ends (`session.closed`, `mod.paused`) is final for the frame: PM sends nothing more,
// so the pending notice is dropped, not delivered into a Map that can no longer refresh.

import type { MapHost } from "./hostApi";

/** Quiet time after the last event before the notice is raised, and the longest it is held back while
 *  events keep coming (a long import must still surface). */
export const QUIET_MS = 1000;
export const MAX_WAIT_MS = 5000;

/** What changed since the last notice. Counts are what the events said; `missed` is that PM dropped
 *  some, so the counts are a floor. Either way the answer is the same: list again. */
export interface ChangeNotice {
  added: number;
  deleted: number;
  missed: boolean;
}

export interface ChangeWatcher {
  /** Stop listening and drop whatever is pending. Idempotent. */
  stop(): void;
}

const lengthOf = (payload: unknown, field: string): number => {
  if (typeof payload !== "object" || payload === null) return 0;
  const list = (payload as Record<string, unknown>)[field];
  return Array.isArray(list) ? list.length : 0;
};

/** Raise `onNotice` once the document events have gone quiet. */
export function watchDocumentChanges(
  host: MapHost,
  onNotice: (notice: ChangeNotice) => void,
): ChangeWatcher {
  let pending: ChangeNotice | null = null;
  let quiet: ReturnType<typeof setTimeout> | null = null;
  let deadline: ReturnType<typeof setTimeout> | null = null;
  let stopped = false;

  const clear = () => {
    if (quiet !== null) clearTimeout(quiet);
    if (deadline !== null) clearTimeout(deadline);
    quiet = null;
    deadline = null;
  };

  const raise = () => {
    const notice = pending;
    pending = null;
    clear();
    if (notice && !stopped) onNotice(notice);
  };

  const touch = () => {
    if (quiet !== null) clearTimeout(quiet);
    quiet = setTimeout(raise, QUIET_MS);
    // The deadline starts with the first event of a burst and is not pushed back by the rest.
    deadline ??= setTimeout(raise, MAX_WAIT_MS);
  };

  const off = host.onEvent((event) => {
    if (stopped) return;
    if (event.name === "session.closed" || event.name === "mod.paused") {
      stopped = true;
      pending = null;
      clear();
      return;
    }
    const isAdd = event.name === "documents.added";
    const isDelete = event.name === "documents.deleted";
    const missed = typeof event.missed === "number" && event.missed > 0;
    // Theme and grant events change nothing the Map lists (unless PM dropped events before them).
    if (!isAdd && !isDelete && !missed) return;
    pending ??= { added: 0, deleted: 0, missed: false };
    if (isAdd) pending.added += lengthOf(event.payload, "documents");
    if (isDelete) pending.deleted += lengthOf(event.payload, "ids");
    pending.missed ||= missed;
    touch();
  });

  return {
    stop() {
      stopped = true;
      pending = null;
      clear();
      off();
    },
  };
}
