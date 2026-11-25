// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Opening a link from the Pinboard, and the one notice that says when it did not open. Core's copy of
// this file hands the address to `open_url`; a frame has no such command, only `shell.openLink`, which
// PM answers for https addresses and refuses for everything else (no mailto, no http, no address that
// is a literal IP or a name with no dot), in a sentence of its own that never repeats the address.
// That sentence is what the notice shows.
//
// The notice is one at a time, like core's, so it is a tiny store: a click reports here, and the one
// `LinkNotice` (frame/App.tsx) draws it.

import { useSyncExternalStore } from "react";
import { openUrl } from "./pm";

let current: string | null = null;
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((l) => l());

/** Show `text` in the link notice, in place of any notice already up. */
export function showLinkFailure(text: string): void {
  current = text;
  emit();
}

/** Take the link notice down (its Dismiss, or a new click on a link). */
export function dismissLinkFailure(): void {
  if (current === null) return;
  current = null;
  emit();
}

/** Open `href` through PM, for a click with no line of its own to say a failure on: the last link's
 *  notice goes at once, and this one's failure shows in it. */
export function openLink(href: string): void {
  dismissLinkFailure();
  void openUrl(href).catch((err: unknown) =>
    showLinkFailure(err instanceof Error ? err.message : "PM couldn't open the link."),
  );
}

/** What the link notice says now; null when it is down. */
export function useLinkFailure(): string | null {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => current,
  );
}
