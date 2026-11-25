// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// PM's right-click menu (#1038). Bobby: "Add right click menu to pinboard / the whole app."
//
// One menu at a time, app-wide, so this is a tiny store rather than per-surface state: a surface
// calls `openContextMenu` from its own `onContextMenu`, and the one `ContextMenuHost` (mounted in
// main.tsx, so every window has it) draws it. Where no surface offers a menu, the host's fallback
// does: Copy for selected text, Open and Copy for a link, and nothing at all otherwise — never the
// browser's page menu, whose Back and Reload would throw away the app's state.
//
// Text you type into keeps the system menu (spelling suggestions, Paste), and
// Shift+right-click always gives it, anywhere: the way back to the system menu, and to Inspect in a
// dev build.
//
// The keyboard opens it too: the Menu key, or Shift+F10, on whatever has focus. Every surface already
// listens for `contextmenu`, so rather than ask each one for a second path the host turns the key into
// that same event (`openMenuFromKeyboard`), aimed at the focused element, and a surface can't tell the
// difference. Without it, Duplicate and the sidebar's Pin and Hide would be reachable by mouse alone.

import { useSyncExternalStore } from "react";
import { MOD } from "./setupGuide";

export type MenuItem =
  | {
      kind?: "item";
      label: string;
      onSelect: () => void;
      disabled?: boolean;
      /** Drawn in the warning colour: a delete. */
      danger?: boolean;
      /** A shortcut shown beside the label, such as "Ctrl+Z". */
      hint?: string;
      /** Set (either way) to make this one choice among several, such as a task's status: drawn
       *  with a tick when true and announced as a radio. Left out, it is an ordinary action. */
      checked?: boolean;
    }
  | { kind: "separator" }
  /** A row of colour choices (a note's tint), each a radio. */
  | {
      kind: "swatches";
      label: string;
      options: { name: string; color: string; active: boolean; onSelect: () => void }[];
    };

export interface OpenMenu {
  x: number;
  y: number;
  /** The menu's accessible name, such as "Note" or "Table". */
  label: string;
  items: MenuItem[];
  /** Focus before the menu opened, given back when it closes without a choice. */
  returnFocus: HTMLElement | null;
  /** Opened from the keyboard, so focus goes to the first item (the menu pattern's rule). A menu
   *  opened with the mouse takes focus itself and starts with no row lit that the pointer never
   *  touched. */
  viaKeyboard: boolean;
}

/** What a surface may ask of `openContextMenu` beyond its own items. */
export interface ContextMenuOptions {
  /** Add the fallback's Copy (for selected text) and Open link / Copy link (for a link) after the
   *  surface's own items, so a row of selectable text or links keeps them. */
  withFallback?: boolean;
}

/** What the fallback's items do. The host registers these once (it is the one place that knows how
 *  to open a link here, and how to copy and say so), so a surface asking for `withFallback` needs to
 *  know neither. `spoken` is what a screen reader hears after the copy, "Copied" by default. */
export interface FallbackActions {
  openUrl?: (url: string) => void;
  copy: (text: string, spoken?: string) => void;
}

let current: OpenMenu | null = null;
let fallbackActions: FallbackActions | null = null;
// Set around the synthetic `contextmenu` the keyboard makes, so the open it causes knows it came
// from a key. A flag rather than a field on the event: React hands surfaces its own wrapper of it.
let fromKeyboard = false;
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((l) => l());

/** The host's say-so on how the fallback's items act; `null` takes it back (the host unmounting). */
export function setFallbackActions(actions: FallbackActions | null): void {
  fallbackActions = actions;
}

/** The open menu right now, for code that is not a component (the host's key handler). */
export function currentContextMenu(): OpenMenu | null {
  return current;
}

/** Open `items` at the pointer of the `contextmenu` event `e`, in place of the system menu. */
export function openContextMenu(
  e: { clientX: number; clientY: number; preventDefault: () => void; target?: EventTarget | null },
  label: string,
  items: MenuItem[],
  options: ContextMenuOptions = {},
): void {
  e.preventDefault();
  if (options.withFallback && fallbackActions) {
    const extra = fallbackItems(
      e.target ?? null,
      window.getSelection()?.toString() ?? "",
      fallbackActions,
    );
    // A separator only when both halves have something; `tidy` below drops a stray one.
    if (extra.length) items = [...items, { kind: "separator" }, ...extra];
  }
  if (!items.some((i) => i.kind !== "separator")) return closeContextMenu();
  const active = document.activeElement;
  let { clientX: x, clientY: y } = e;
  let viaKeyboard = fromKeyboard;
  // A keyboard-opened menu the engine made itself (VoiceOver's "show menu", WebKit's Menu key) comes
  // with no position, 0,0, from the element that has focus. Opened there it would sit in the window's
  // corner, so put it at the element, as the Menu key does.
  if (
    !viaKeyboard &&
    x === 0 &&
    y === 0 &&
    active instanceof HTMLElement &&
    active !== document.body &&
    e.target instanceof Node &&
    e.target !== document.body &&
    e.target.contains(active)
  ) {
    ({ x, y } = keyboardAnchor(
      active.getBoundingClientRect(),
      window.innerWidth,
      window.innerHeight,
    ));
    viaKeyboard = true;
  }
  current = {
    x,
    y,
    label,
    items: tidy(items),
    returnFocus: active instanceof HTMLElement ? active : null,
    viaKeyboard,
  };
  emit();
}

/** Whether `e` is the key that asks for a context menu: the Menu key, or Shift+F10 (the same ask on a
 *  keyboard without one). Shift+F10 with Ctrl, Alt or the Meta key held is something else, and so is
 *  Shift+Menu: Shift is how a mouse user asks for the system menu, so the keyboard one can too. */
export function isMenuKey(e: {
  key: string;
  shiftKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  metaKey: boolean;
}): boolean {
  if (e.key === "ContextMenu") return !e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey;
  return e.key === "F10" && e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey;
}

/** Where a keyboard-opened menu goes for an element at `rect`, in a `vw`×`vh` window: a little in
 *  from its top-left corner and inside it, never off the screen, and no lower than its bottom edge,
 *  so a small row opens the menu just under its label and a tall card near its top. */
export function keyboardAnchor(
  rect: { left: number; top: number; right: number; bottom: number },
  vw: number,
  vh: number,
): { x: number; y: number } {
  const clamp = (n: number, lo: number, hi: number) => Math.max(lo, Math.min(n, hi));
  const left = clamp(rect.left, 0, vw);
  const top = clamp(rect.top, 0, vh);
  const right = clamp(rect.right, left, vw);
  const bottom = clamp(rect.bottom, top, vh);
  return { x: Math.min(left + 4, right), y: Math.min(top + 48, bottom) };
}

/** Ask for the context menu of `target` (the element with focus; the page when none) as if it had
 *  been right-clicked there: a `contextmenu` event with no Shift (Shift is the way to the system
 *  menu, and Shift+F10 carries it) at the element's own corner. Returns whether something took it,
 *  meaning PM's menu is up, or deliberately no menu, rather than the system's. */
export function openMenuFromKeyboard(target: Element | null): boolean {
  const el = target ?? document.body;
  const { x, y } = keyboardAnchor(
    el.getBoundingClientRect(),
    window.innerWidth,
    window.innerHeight,
  );
  const ev = new MouseEvent("contextmenu", {
    bubbles: true,
    cancelable: true,
    composed: true,
    button: 2,
    clientX: x,
    clientY: y,
  });
  fromKeyboard = true;
  try {
    el.dispatchEvent(ev);
  } finally {
    // Even when a handler threw: a stuck flag would make the next mouse open look like a key.
    fromKeyboard = false;
  }
  return ev.defaultPrevented;
}

export function closeContextMenu(): void {
  if (!current) return;
  current = null;
  emit();
}

export function useContextMenu(): OpenMenu | null {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => current,
  );
}

/** Separators only between groups: none at either end, never two in a row. Lets a surface build its
 *  list from optional groups without minding the joins. */
export function tidy(items: MenuItem[]): MenuItem[] {
  const out: MenuItem[] = [];
  const last = () => out[out.length - 1];
  for (const item of items) {
    if (item.kind === "separator" && (out.length === 0 || last().kind === "separator")) continue;
    out.push(item);
  }
  while (out.length && last().kind === "separator") out.pop();
  return out;
}

/** Whether right-clicking `target` should leave the system menu alone: it's in text you can type
 *  into, where the system menu has spelling suggestions and Paste. */
export function inEditable(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) return false;
  const field = target.closest("input, textarea, select, [contenteditable]");
  if (!field) return false;
  if (field instanceof HTMLInputElement) {
    // A checkbox, a button or a range is not text.
    return ["text", "search", "url", "email", "tel", "password", "number", ""].includes(field.type);
  }
  if (field.hasAttribute("contenteditable"))
    return field.getAttribute("contenteditable") !== "false";
  return true;
}

/** Whether a surface's own `onContextMenu` should open PM's menu for `e`: not when another surface
 *  already took it, not with Shift held (the system menu, on purpose), and not in editable text. */
export function wantsAppMenu(e: {
  defaultPrevented: boolean;
  shiftKey: boolean;
  target: EventTarget | null;
}): boolean {
  return !e.defaultPrevented && !e.shiftKey && !inEditable(e.target);
}

/** Where a `w`×`h` menu opened at the pointer (`x`, `y`) goes so it stays inside a `vw`×`vh`
 *  window, with `pad` px clear of each edge: flipped to the pointer's left or upward when it would
 *  run off, then pulled in. */
export function placeMenu(
  x: number,
  y: number,
  w: number,
  h: number,
  vw: number,
  vh: number,
  pad = 4,
): { left: number; top: number } {
  let left = x + w + pad > vw ? x - w : x;
  let top = y + h + pad > vh ? y - h : y;
  left = Math.max(pad, Math.min(left, vw - w - pad));
  top = Math.max(pad, Math.min(top, vh - h - pad));
  return { left, top };
}

/** The fallback menu for a right-click no surface claimed: a link's Open and Copy, and Copy for
 *  selected text. Empty means no menu at all. With no `openUrl` (a mod frame that cannot open links),
 *  a link offers Copy alone. */
export function fallbackItems(
  target: EventTarget | null,
  selection: string,
  actions: FallbackActions,
): MenuItem[] {
  const items: MenuItem[] = [];
  const link =
    target instanceof Element ? target.closest<HTMLAnchorElement>("a[href]")?.href : undefined;
  if (link && /^https?:\/\//i.test(link)) {
    const open = actions.openUrl;
    if (open) items.push({ label: "Open link", onSelect: () => open(link) });
    items.push(
      { label: "Copy link", onSelect: () => actions.copy(link, "Link copied") },
      { kind: "separator" },
    );
  }
  if (selection.trim())
    items.push({ label: "Copy", hint: `${MOD}C`, onSelect: () => actions.copy(selection) });
  return tidy(items);
}
