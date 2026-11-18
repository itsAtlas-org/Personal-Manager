// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// How often this device has shown the full Mods disclaimer in the open vault, and whether the person
// chose "Don't show this again" on a mod's first-open screen (Bobby, 2026-11-17: the box appears only
// from the SECOND showing on, so the notice is seen at least twice before it can be folded away).
//
// Where it lives: one `localStorage` key per vault, `pm.mods.disclaimerShown.<vault id>`, holding
// `{"v":1,"shown":<n>,"hide":<bool>}`. The vault id is `vault_status().vault_id`, the stable id in the
// vault's own settings file; a new machine has no key and a joined vault has another id, so both start
// again, which is the disclaimer's own scope (the backend's acceptance record is per device AND per
// vault). It is a UI convenience, not a consent record: the backend's rules for accepting are
// untouched, and this key only decides how the screen is laid out.
//
// Every failure reads toward MORE disclaimer, never less. No vault id (the status can't say, the id
// isn't a plain token), no `localStorage`, a value that isn't ours or isn't whole: zero showings and
// not opted out, so the full text shows and no box is offered. A write that fails just means the
// count doesn't advance.

import { vaultStatus } from "./ipc";

const PREFIX = "pm.mods.disclaimerShown.";

/** A count past this is not a count (a hand-edited value); it reads as broken. */
const MAX_SHOWN = 1_000_000;

/** What this device remembers about the disclaimer in one vault. */
export interface Shown {
  /** Times the full text has been shown. */
  shown: number;
  /** "Don't show this again" was ticked at a Turn on. */
  hide: boolean;
}

const NOTHING: Shown = { shown: 0, hide: false };

/** A vault id fit to be part of a key: the id is a plain token, so anything else is not trusted. */
export function vaultKeyOf(id: unknown): string | null {
  return typeof id === "string" && /^[A-Za-z0-9_-]{1,64}$/.test(id) ? PREFIX + id : null;
}

/** The stored value for `key`, or nothing shown and not opted out when it is missing or broken. */
export function readShown(key: string): Shown {
  try {
    const raw = localStorage.getItem(key);
    if (raw === null) return NOTHING;
    const v: unknown = JSON.parse(raw);
    if (typeof v !== "object" || v === null) return NOTHING;
    const r = v as Record<string, unknown>;
    if (
      r.v !== 1 ||
      typeof r.shown !== "number" ||
      !Number.isInteger(r.shown) ||
      r.shown < 0 ||
      r.shown > MAX_SHOWN ||
      typeof r.hide !== "boolean"
    ) {
      return NOTHING;
    }
    return { shown: r.shown, hide: r.hide };
  } catch {
    return NOTHING;
  }
}

function writeShown(key: string, value: Shown): void {
  try {
    localStorage.setItem(key, JSON.stringify({ v: 1, shown: value.shown, hide: value.hide }));
  } catch {
    // Best-effort: a count that can't be kept just doesn't advance.
  }
}

/** The open vault's key, or null when the page can't tell which vault it is in. */
async function currentKey(): Promise<string | null> {
  try {
    return vaultKeyOf((await vaultStatus())?.vault_id);
  } catch {
    return null;
  }
}

/** One showing of the full disclaimer, as it was when the screen opened. */
export interface Showing {
  /** Which showing this is: 1 for the first ever in this vault on this device. */
  number: number;
  /** "Don't show this again" was chosen before this screen opened. */
  hide: boolean;
  /** The key this showing was counted under; null when it couldn't be (nothing is remembered). */
  key: string | null;
}

/**
 * Count one showing of the full disclaimer and say what it was. Called once per time a screen
 * carrying the text opens (the hook below makes sure of that); the count is read, raised by one and
 * written back in the same step, so two screens opening together each get their own number.
 *
 * `foldable` is a screen that folds the text away once "Don't show this again" is on (a mod's
 * first-open screen): then the full text isn't shown and nothing is counted. The Mods tab's own gate
 * isn't foldable, so it always shows the text and always counts.
 */
export async function beginShowing(foldable: boolean): Promise<Showing> {
  const key = await currentKey();
  if (key === null) return { number: 1, hide: false, key: null };
  const before = readShown(key);
  if (foldable && before.hide) return { number: before.shown, hide: true, key };
  const shown = Math.min(before.shown + 1, MAX_SHOWN);
  writeShown(key, { shown, hide: before.hide });
  return { number: shown, hide: before.hide, key };
}

/** Remember "Don't show this again" for the vault a showing was counted in. Reads the count again so
 *  a showing counted since is kept. A showing with no key can't be remembered. */
export function rememberHide(showing: Showing): void {
  if (showing.key === null) return;
  writeShown(showing.key, { shown: readShown(showing.key).shown, hide: true });
}

/** The box is offered from the second showing on, never the first, and not at all once it is on. */
export function offersHide(showing: Showing): boolean {
  return showing.number >= 2 && !showing.hide;
}

/** Whether the open vault has "Don't show this again" on. Anything that can't be told reads as not
 *  folded (the notice shows), so this is only ever true for a vault that really has it on. */
export async function isHidden(): Promise<boolean> {
  const key = await currentKey();
  return key !== null && readShown(key).hide;
}

/**
 * Bring the Mods notice back for the open vault: "Don't show this again" goes off and the count of
 * showings is kept, so the box is offered again at the next showing (it is offered from the second
 * showing on, and the person has seen the notice at least twice to have folded it). Nothing is done
 * when the vault can't be told or nothing is folded. A write that fails leaves it folded; the caller
 * reads `isHidden()` again to say what is true.
 */
export async function showAgain(): Promise<void> {
  const key = await currentKey();
  if (key === null) return;
  const before = readShown(key);
  if (before.hide) writeShown(key, { shown: before.shown, hide: false });
}
