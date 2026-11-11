// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the calendar header's icon-only Refresh says about itself. The words can't ride on the button
// (it is a circular-arrows icon in the header's corner), so they live in two places that must agree:
// the tooltip, which shows on hover and on keyboard focus, and a visually hidden description that a
// screen reader reads after the button's constant name "Refresh". The accessible NAME never carries
// the time: it would change on every sync, and voice control's "click Refresh" needs it stable.
//
// "Last synced" is the last COMPLETE sync (DECISIONS: the full sync stamps it, the ~30 s change check
// does not), so it can read a quarter of an hour old while changes are still arriving.

import { formatClock, formatDateTime } from "./format";

/** "14:32" if the sync was today, else "10-11 14:32" (date and clock, in the app's date format).
 *  Null for an unparseable stamp. */
function syncedAt(lastSync: string, now: Date): string | null {
  const d = new Date(lastSync);
  if (Number.isNaN(d.getTime())) return null;
  const sameDay =
    d.getFullYear() === now.getFullYear() &&
    d.getMonth() === now.getMonth() &&
    d.getDate() === now.getDate();
  return sameDay ? formatClock(d) : formatDateTime(lastSync);
}

/** The tooltip: starts with "Refresh", the button's name, so a speech user who reads it can say it. */
export function refreshTip(
  lastSync: string | null,
  syncing: boolean,
  now: Date = new Date(),
): string {
  if (syncing) return "Refreshing…";
  if (lastSync === null) return "Refresh · never synced";
  const at = syncedAt(lastSync, now);
  return at === null ? "Refresh" : `Refresh · last synced ${at}`;
}

/** The hidden description, the same fact without the name: "Last synced 14:32". Empty when the
 *  stamp can't be read, so nothing is announced rather than the raw value. */
export function refreshDescription(
  lastSync: string | null,
  syncing: boolean,
  now: Date = new Date(),
): string {
  if (syncing) return "Refreshing…";
  if (lastSync === null) return "Never synced";
  const at = syncedAt(lastSync, now);
  return at === null ? "" : `Last synced ${at}`;
}
