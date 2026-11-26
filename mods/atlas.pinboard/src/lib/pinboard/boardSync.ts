// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// How the Pinboard's board is read from storage and written back to it, as pure functions (Mods plan
// K7, the must-fix list). Core's Pinboard parsed the stored board into a normalised, reflowed copy and
// wrote that copy back on the next change, so ANY edit rewrote every card's position, dropped what it
// could not read, and a board it could not read was shown empty and then overwritten. The mod keeps
// two things apart instead:
//
//   - the STORED board: the JSON as PM holds it, kept verbatim (unknown cards, unknown fields of known
//     cards, unknown top-level fields), the only thing that is ever written;
//   - the DISPLAY board: what the view draws (usePinboard's `deriveDisplay`: sanitised, reflowed into
//     the frame's width), which edits change.
//
// A save is the stored board with the DIFFERENCE between the display board as it was loaded and the
// display board now applied to it: a card that was added is added, one that was removed is removed, a
// field that changed is set, and every other card, field and rect is sent with the very same values
// it was stored with (the same objects, in the same key order). What PM's store then keeps is those
// values; it may write an object's keys in another order, so after a save the board is the same, not
// necessarily the same bytes, and nothing here treats a different key order as a change. The same two
// functions merge: after a revision conflict the board is read again and the same difference is
// applied to what is there now, so the other writer's changes are kept and ours are added to them.
//
// Nothing here throws on a hostile value (any JSON, any depth) and nothing here touches the clock, PM
// or React.

import { BOARD_VERSION } from "./types";

export type Obj = Record<string, unknown>;

export const isObj = (v: unknown): v is Obj =>
  typeof v === "object" && v !== null && !Array.isArray(v);

/** The stored board, as far as it can be read. `empty` is a board nobody has written yet (a real
 *  answer: a fresh install); `unreadable` is one this build must not touch. */
export type StoredRead =
  | { kind: "empty" }
  | { kind: "ok"; board: Obj & { widgets: unknown[] } }
  | { kind: "unreadable"; why: "version" | "shape" };

/** Read the JSON text `getPref` answered. Never throws. A board whose version this build does not
 *  know (a newer one, or none) is unreadable: showing it empty and saving would destroy it. */
export function readStored(raw: string | null): StoredRead {
  if (raw === null || raw === "") return { kind: "empty" };
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return { kind: "unreadable", why: "shape" };
  }
  if (!isObj(parsed) || !Array.isArray(parsed.widgets)) return { kind: "unreadable", why: "shape" };
  if (parsed.version !== BOARD_VERSION) return { kind: "unreadable", why: "version" };
  return { kind: "ok", board: parsed as Obj & { widgets: unknown[] } };
}

/** The board a save starts from when nothing is stored yet. */
export function freshStored(): Obj & { widgets: unknown[] } {
  return { version: BOARD_VERSION, widgets: [] };
}

// --- structural comparison ------------------------------------------------------------------------

/** Deeper than any real board; past it two values are called different, which only costs a write of
 *  what the display already holds. */
const MAX_DEPTH = 64;

/** Whether two JSON values are the same. Identical objects are the same without a look inside: the
 *  display board shares every value the person did not touch with the one it was derived from. */
export function sameJson(a: unknown, b: unknown, depth = 0): boolean {
  if (a === b) return true;
  if (depth > MAX_DEPTH) return false;
  if (typeof a !== typeof b || typeof a !== "object" || a === null || b === null) return false;
  if (Array.isArray(a)) {
    if (!Array.isArray(b) || a.length !== b.length) return false;
    return a.every((v, i) => sameJson(v, b[i], depth + 1));
  }
  if (Array.isArray(b)) return false;
  const ka = Object.keys(a as Obj).filter((k) => (a as Obj)[k] !== undefined);
  const kb = Object.keys(b as Obj).filter((k) => (b as Obj)[k] !== undefined);
  if (ka.length !== kb.length) return false;
  return ka.every((k) => k in (b as Obj) && sameJson((a as Obj)[k], (b as Obj)[k], depth + 1));
}

// --- the difference between two display boards -----------------------------------------------------

/** The fields that hold a list of things with ids of their own, diffed item by item so a save of one
 *  entry's change leaves the others (and what this build cannot read in them) as they were. */
const LIST_FIELDS = ["children", "items"] as const;

export interface ItemPatch {
  /** Fields whose value changed or appeared, with their new value. */
  set: Obj;
  /** Fields that are gone. */
  unset: string[];
  /** The change to a list field, by field name. */
  lists: Record<string, ListPatch>;
  /** The whole item as it is now, for putting back one the other writer deleted. */
  full: Obj;
}

export interface ListPatch {
  /** The ids now, in order (each id once, first occurrence). */
  order: string[];
  /** Whether the ids both lists have are in a different order now (a card was raised, say). */
  reordered: boolean;
  added: Map<string, Obj>;
  removed: Set<string>;
  /** For a removed item that held a list of children (a folder being ungrouped): the ids of the
   *  children that were drawn. The person saw those and chose what became of them (they are in
   *  `added` when they were spilled onto the board). Any other child the stored item holds is one
   *  this build could not draw, and is not the person's to lose: see {@link applyList}. */
  removedKids: Map<string, Set<string>>;
  changed: Map<string, ItemPatch>;
}

const idOf = (item: unknown): string | null =>
  isObj(item) && typeof item.id === "string" ? item.id : null;

/** The items that can be told apart by id: objects with a string id, the first of each. */
function indexById(list: readonly unknown[]): Map<string, Obj> {
  const out = new Map<string, Obj>();
  for (const item of list) {
    const id = idOf(item);
    if (id !== null && !out.has(id)) out.set(id, item as Obj);
  }
  return out;
}

function diffItem(before: Obj, now: Obj): ItemPatch | null {
  const set: Obj = {};
  const unset: string[] = [];
  const lists: Record<string, ListPatch> = {};
  for (const key of Object.keys(now)) {
    const value = now[key];
    if (value === undefined) {
      if (before[key] !== undefined) unset.push(key);
      continue;
    }
    const was = before[key];
    if (was === value) continue;
    if (
      (LIST_FIELDS as readonly string[]).includes(key) &&
      Array.isArray(was) &&
      Array.isArray(value)
    ) {
      const patch = diffList(was, value);
      if (patch) lists[key] = patch;
      continue;
    }
    if (!sameJson(was, value)) set[key] = value;
  }
  for (const key of Object.keys(before)) {
    if (before[key] !== undefined && (!(key in now) || now[key] === undefined)) {
      if (!unset.includes(key)) unset.push(key);
    }
  }
  if (Object.keys(set).length === 0 && unset.length === 0 && Object.keys(lists).length === 0) {
    return null;
  }
  return { set, unset, lists, full: now };
}

/** What changed between two lists of items (null when nothing did). */
export function diffList(before: readonly unknown[], now: readonly unknown[]): ListPatch | null {
  const was = indexById(before);
  const is = indexById(now);
  const added = new Map<string, Obj>();
  const removed = new Set<string>();
  const removedKids = new Map<string, Set<string>>();
  const changed = new Map<string, ItemPatch>();
  for (const [id, item] of is) if (!was.has(id)) added.set(id, item);
  for (const [id, item] of was) {
    if (is.has(id)) continue;
    removed.add(id);
    if (Array.isArray(item.children)) {
      const kids = new Set<string>();
      for (const kid of item.children) {
        const kidId = idOf(kid);
        if (kidId !== null) kids.add(kidId);
      }
      removedKids.set(id, kids);
    }
  }
  for (const [id, item] of is) {
    const old = was.get(id);
    if (old === undefined || old === item) continue;
    const patch = diffItem(old, item);
    if (patch) changed.set(id, patch);
  }
  const order = [...is.keys()];
  const common = (ids: Iterable<string>, other: Map<string, Obj>) =>
    [...ids].filter((id) => other.has(id));
  const a = common(was.keys(), is);
  const b = common(is.keys(), was);
  const reordered = a.some((id, i) => id !== b[i]);
  if (added.size === 0 && removed.size === 0 && changed.size === 0 && !reordered) return null;
  return { order, reordered, added, removed, removedKids, changed };
}

/** What changed between the board as loaded and the board now (null when nothing did). */
export function diffBoards(
  before: { widgets: readonly unknown[] },
  now: { widgets: readonly unknown[] },
) {
  return diffList(before.widgets, now.widgets);
}

// --- applying a difference to another list ---------------------------------------------------------

function applyItem(raw: unknown, patch: ItemPatch): Obj {
  if (!isObj(raw)) return patch.full;
  const out: Obj = { ...raw };
  for (const [key, value] of Object.entries(patch.set)) out[key] = value;
  for (const key of patch.unset) delete out[key];
  for (const [key, listPatch] of Object.entries(patch.lists)) {
    const there = out[key];
    out[key] = Array.isArray(there) ? applyList(there, listPatch) : patch.full[key];
  }
  return out;
}

/** `raw` (a stored or freshly read list) with `patch` applied: added items are added, removed ones are
 *  removed, changed ones get only their changed fields, and everything else in `raw` is returned as it
 *  is, including items this build could not read and items the other writer added. An item the other
 *  writer deleted that the patch changed comes back (the edit is the person's; nothing is lost). */
export function applyList(raw: readonly unknown[], patch: ListPatch): unknown[] {
  const first = indexById(raw);
  // Items that cannot be addressed (no id, or an id an earlier item already has) are never touched.
  const kept: { id: string | null; item: unknown }[] = [];
  const present = new Set<string>();
  raw.forEach((item) => {
    const id = idOf(item);
    const addressable = id !== null && first.get(id) === item && !present.has(id);
    if (addressable) present.add(id);
    const key = addressable ? id : null;
    if (key !== null && patch.removed.has(key)) {
      // A folder that is ungrouped takes its drawn children out onto the board (they are in the
      // patch). What it holds that this build never drew (a card kind a newer build wrote, a child
      // with a rect it could not read, a folder inside it) is not ungrouped away: it is kept, as it
      // is, in the folder's place.
      const drawn = patch.removedKids.get(key);
      if (drawn !== undefined && isObj(item) && Array.isArray(item.children)) {
        for (const child of item.children) {
          const childId = idOf(child);
          if (childId !== null && (drawn.has(childId) || patch.added.has(childId))) continue;
          kept.push({ id: null, item: child });
        }
      }
      return;
    }
    let next = item;
    if (key !== null) {
      const change = patch.changed.get(key);
      const replaced = patch.added.get(key);
      if (replaced) next = replaced;
      else if (change) next = applyItem(item, change);
    }
    kept.push({ id: key, item: next });
  });
  // What the patch brings that `raw` does not have, in the order it has it now.
  const fresh = new Map<string, unknown>();
  for (const id of patch.order) {
    if (present.has(id)) continue;
    const added = patch.added.get(id);
    if (added) fresh.set(id, added);
    else {
      const change = patch.changed.get(id);
      if (change) fresh.set(id, change.full);
    }
  }
  if (!patch.reordered) {
    return [...kept.map((k) => k.item), ...fresh.values()];
  }
  // The order changed: items the patch knows go in its order; the others (unreadable, or added by
  // someone else) stay right after the known item that preceded them.
  const known = new Set(patch.order);
  const byId = new Map<string, unknown>();
  const after = new Map<string | null, unknown[]>();
  let anchor: string | null = null;
  for (const { id, item } of kept) {
    if (id !== null && known.has(id)) {
      byId.set(id, item);
      anchor = id;
    } else {
      const list = after.get(anchor) ?? [];
      list.push(item);
      after.set(anchor, list);
    }
  }
  const out: unknown[] = [...(after.get(null) ?? [])];
  for (const id of patch.order) {
    const item = byId.has(id) ? byId.get(id) : fresh.get(id);
    if (item === undefined) continue;
    out.push(item, ...(after.get(id) ?? []));
  }
  return out;
}

/** The stored board with `patch` applied: the same object when there is nothing to apply. */
export function applyBoard<B extends Obj & { widgets: unknown[] }>(
  stored: B,
  patch: ListPatch | null,
): B {
  if (!patch) return stored;
  return { ...stored, widgets: applyList(stored.widgets, patch) };
}

// --- the size of a save ---------------------------------------------------------------------------

/** The most bytes of arguments one call may carry: PM's cap (`MAX_ARGS_BYTES`, limits.rs), which the
 *  frame's own bridge enforces first (`modBridge.ts`; a test holds all three equal). */
export const MAX_ARGS_BYTES = 1_048_576;

const encoder = new TextEncoder();

/** The bytes `storage.set` would send for this board, as the JSON text of the whole call's
 *  arguments: the board plus the call's own envelope (its key and the revision, counted at their
 *  longest so the answer is never too small). */
export function saveBytes(key: string, boardJson: string): number {
  const envelope = JSON.stringify({ key, value: 0, expectedRevision: Number.MAX_SAFE_INTEGER });
  // `value` is the board: its JSON text replaces the one character standing in for it.
  return encoder.encode(envelope).length - 1 + encoder.encode(boardJson).length;
}

/** How full a board is, as a whole percentage of the cap (never under 1 for a board with anything). */
export function fullness(bytes: number): number {
  if (bytes <= 0) return 0;
  return Math.max(1, Math.min(100, Math.round((bytes / MAX_ARGS_BYTES) * 100)));
}
