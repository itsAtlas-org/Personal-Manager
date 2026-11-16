// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { isLetterKey } from "./letterKey";

/** What a key press asks the Calendar tab to do: step back or forward a period, jump to today, or
 *  start a new event (`c`, the full editor; Shift+C, the quick one, as in Google Calendar). */
export type CalendarNav = "prev" | "next" | "today" | "create" | "quick-create";

/** The parts of a keyboard event the decision reads. */
export interface NavKeyEvent {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  /** Read for `c` only: Shift+C is quick create, and Caps Lock alone isn't. */
  shiftKey?: boolean;
  target: EventTarget | null;
}

/** The parts of a keyboard event the Undo decision reads, on top of {@link NavKeyEvent}. */
export interface UndoKeyEvent extends NavKeyEvent {
  shiftKey: boolean;
  /** The physical key ("KeyZ"), for a layout that types no Latin letter ({@link isLetterKey}). */
  code: string;
  repeat: boolean;
}

/** Whether a key went to something being typed in, whose own keys (and own Undo) come first. */
export function isTypingTarget(target: EventTarget | null): boolean {
  const t = target as HTMLElement | null;
  return !!t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName ?? ""));
}

/** The keys that page the calendar, in one table so the decision and the hints on the ‹ › buttons
 *  can't drift. `glyph` is how a hint draws it; `vertical` keys page only where the periods run down
 *  the screen (Month and Year), since the day grids' own scrolling owns ↑ and ↓. The letters are
 *  Google Calendar's: j or n for next, k or p for previous. */
const PAGE_KEYS: { key: string; glyph: string; nav: "prev" | "next"; vertical?: true }[] = [
  { key: "ArrowLeft", glyph: "←", nav: "prev" },
  { key: "ArrowUp", glyph: "↑", nav: "prev", vertical: true },
  { key: "k", glyph: "k", nav: "prev" },
  { key: "p", glyph: "p", nav: "prev" },
  { key: "ArrowRight", glyph: "→", nav: "next" },
  { key: "ArrowDown", glyph: "↓", nav: "next", vertical: true },
  { key: "j", glyph: "j", nav: "next" },
  { key: "n", glyph: "n", nav: "next" },
];

/** What a ‹ or › button says of its keys: the glyphs for a tooltip ("←, ↑, k or p") and the key names
 *  for `aria-keyshortcuts` ("ArrowLeft ArrowUp k p"). `vertical` is the view's, as for
 *  {@link calendarNavKey}. */
export function pageKeyHint(
  nav: "prev" | "next",
  vertical: boolean,
): { glyphs: string; shortcuts: string } {
  const keys = PAGE_KEYS.filter((k) => k.nav === nav && (vertical || !k.vertical));
  const glyphs = keys.map((k) => k.glyph);
  const last = glyphs.pop();
  return {
    glyphs: glyphs.length > 0 ? `${glyphs.join(", ")} or ${last}` : String(last),
    shortcuts: keys.map((k) => k.key).join(" "),
  };
}

/**
 * The Calendar tab's keyboard shortcuts (← / → / `j` `k` `n` `p` / `t` / `c`, and ↑ / ↓ where
 * `vertical`), as a pure decision. Nothing when a modifier is held (app shortcuts, and Ctrl+C stays
 * copy), when typing in a field, or when a dialog is open: the listener sits on `window`, so without
 * the dialog check a key pressed on a dialog's button paged the grid behind it. `vertical` is the
 * view's: true in Month and Year, whose periods run down the screen, so ↑ is back and ↓ forward as ←
 * and → are; false in the day grids and the agenda, where ↑ and ↓ scroll. The letters read `key`, as
 * `t` always has (`NavKeyEvent` has no `code`), so they all change together if a layout ever needs it,
 * and Caps Lock doesn't change them; `c` asks Shift itself, so Caps Lock types the full editor's `c`,
 * not quick create's.
 */
export function calendarNavKey(
  e: NavKeyEvent,
  dialogOpen: boolean,
  vertical = false,
): CalendarNav | null {
  if (dialogOpen) return null;
  if (e.metaKey || e.ctrlKey || e.altKey) return null;
  if (isTypingTarget(e.target)) return null;
  const lower = e.key.toLowerCase();
  const page = PAGE_KEYS.find(
    (k) => (vertical || !k.vertical) && (k.key.length === 1 ? k.key === lower : k.key === e.key),
  );
  if (page) return page.nav;
  if (lower === "t") return "today";
  if (lower === "c") return e.shiftKey ? "quick-create" : "create";
  return null;
}

/**
 * Whether a key press is the Calendar tab's Undo: Ctrl+Z, or Cmd+Z on a Mac (either works on any
 * computer, as on the Pinboard), which presses the newest Undo on screen (#884, F5; Bobby: "can we
 * make control z also undo it within 8 seconds"). It stands down where the arrow keys do: in a field,
 * which has its own Undo for its text, and behind a dialog. Not with Shift (redo has nothing to redo:
 * deleting again goes through its confirmation), with Alt, or for a held-down key, which would undo
 * one delete after another.
 */
export function calendarUndoKey(e: UndoKeyEvent, dialogOpen: boolean): boolean {
  if (dialogOpen || e.repeat) return false;
  if (!(e.ctrlKey || e.metaKey) || e.altKey || e.shiftKey) return false;
  if (isTypingTarget(e.target)) return false;
  return isLetterKey(e, "z");
}
