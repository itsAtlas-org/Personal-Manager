// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Draws PM's right-click menu (kit/lib/contextMenu) and answers every right-click no surface claimed.
// Mounted once per window, in main.tsx, and once per mod frame (a frame is a page of its own, with its
// own document and its own copy of the store, so the host a frame mounts draws only what that frame
// opened). The kit holds no way to open a link: the host is given `openUrl` (core's is
// `lib/linkFailure`'s `openLink`, a frame's the bridge's `shell.openLink`) and, with none, a link's menu
// offers Copy link alone.
//
// The fallback listens on the document in the bubble phase, so it runs after React has delivered the
// event to every surface: a surface that opened its own menu has called preventDefault by then, and
// is left alone. Core's Open link says in the link notice when the link didn't open (lib/linkFailure).
//
// The keyboard: the Menu key and Shift+F10 are caught on the document before anything else sees them
// and turned into a `contextmenu` at the element with focus (see `openMenuFromKeyboard`). Not in text
// you type into, which keeps the system menu on those keys as it does for the mouse.

import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import { createPortal } from "react-dom";
import {
  closeContextMenu,
  currentContextMenu,
  inEditable,
  isMenuKey,
  openContextMenu,
  openMenuFromKeyboard,
  placeMenu,
  setFallbackActions,
  useContextMenu,
  type MenuItem,
} from "../lib/contextMenu";

/** A shortcut as the menu shows it ("Ctrl+Z", "⌘⇧Z") in the form `aria-keyshortcuts` reads
 *  ("Control+Z", "Meta+Shift+Z"). */
const keyShortcut = (hint: string) =>
  hint.replace("Ctrl+", "Control+").replace("⌘", "Meta+").replace("⇧", "Shift+");

/** How long a native `contextmenu` that follows our own keydown is thrown away. Some engines send
 *  one at key-up (Windows) as well as at key-down, and the second would open the system menu. */
const NATIVE_FOLLOW_UP_MS = 500;

/** Rows are the menu's own buttons: items and the colour swatches. A disabled row keeps the native
 *  `disabled` attribute for now, so the arrow keys pass over it (the APG allows either way). Keeping
 *  it focusable with `aria-disabled` instead waits for the Pinboard mod's next version, whose own test
 *  reads `.disabled` (#1136). */
const ROWS = "[role^=menuitem]:not(:disabled)";

export function ContextMenuHost({ openUrl }: { openUrl?: (url: string) => void }) {
  const menu = useContextMenu();
  // What a screen reader hears after a copy: the menu has closed and shows nothing.
  // `n` makes the same words twice in a row a change: a reader says nothing for text that didn't change.
  const [said, setSaid] = useState({ text: "", n: 0 });
  const saidTimer = useRef<ReturnType<typeof setTimeout>>(undefined);

  useEffect(() => {
    // The one place that knows how to open a link and how to copy, for the fallback below and for
    // any surface that asks for `withFallback`.
    const actions = {
      openUrl,
      copy: (text: string, spoken = "Copied") => {
        // Said only once it has worked: with no clipboard to write to, nothing was copied.
        navigator.clipboard?.writeText(text).then(
          () => {
            setSaid((s) => ({ text: spoken, n: s.n + 1 }));
            clearTimeout(saidTimer.current);
            saidTimer.current = setTimeout(() => setSaid((s) => ({ text: "", n: s.n })), 2000);
          },
          () => {},
        );
      },
    };
    setFallbackActions(actions);
    // Windows sends its own contextmenu when the key comes UP, so a key held a while must still be
    // covered: `keyHeld` from our key-down to the key-up, then the grace period after it. A mouse
    // press in between means the next one is a real right-click, which is let through.
    let swallowUntil = 0;
    let keyHeld = false;
    const onMenu = (e: MouseEvent) => {
      if (e.defaultPrevented || e.shiftKey || inEditable(e.target)) return;
      // With nothing to offer, still no system menu: its Back and Reload would reset PM.
      openContextMenu(e, "Menu", [], { withFallback: true });
    };
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (!isMenuKey(e) || e.repeat) return;
      const open = currentContextMenu();
      if (open) {
        // The key that asked for the menu puts it away again. Windows still sends its own contextmenu
        // when this key comes up, which would open a menu again: swallow that one too.
        e.preventDefault();
        closeContextMenu();
        open.returnFocus?.focus();
        keyHeld = true;
        swallowUntil = performance.now() + NATIVE_FOLLOW_UP_MS;
        return;
      }
      const at = document.activeElement;
      if (inEditable(at)) return;
      if (openMenuFromKeyboard(at)) {
        e.preventDefault();
        e.stopPropagation();
        keyHeld = true;
        swallowUntil = performance.now() + NATIVE_FOLLOW_UP_MS;
      }
    };
    const onKeyUp = (e: globalThis.KeyboardEvent) => {
      if (!keyHeld || (e.key !== "ContextMenu" && e.key !== "F10")) return;
      keyHeld = false;
      swallowUntil = performance.now() + NATIVE_FOLLOW_UP_MS;
    };
    const onMouseDown = () => {
      keyHeld = false;
      swallowUntil = 0;
    };
    // Only a real one: our own is not trusted, and must get through.
    const onNative = (e: MouseEvent) => {
      if (e.isTrusted && (keyHeld || performance.now() < swallowUntil)) {
        e.preventDefault();
        e.stopImmediatePropagation();
      }
    };
    document.addEventListener("contextmenu", onMenu);
    document.addEventListener("contextmenu", onNative, true);
    document.addEventListener("keydown", onKey, true);
    document.addEventListener("keyup", onKeyUp, true);
    document.addEventListener("mousedown", onMouseDown, true);
    return () => {
      document.removeEventListener("contextmenu", onMenu);
      document.removeEventListener("contextmenu", onNative, true);
      document.removeEventListener("keydown", onKey, true);
      document.removeEventListener("keyup", onKeyUp, true);
      document.removeEventListener("mousedown", onMouseDown, true);
      setFallbackActions(null);
      clearTimeout(saidTimer.current);
    };
  }, [openUrl]);

  return (
    <>
      {/* Always there, so a change to it is announced: the menu is gone by the time a copy lands. A
          live region and not role="status", which the page's own notices already use and tests
          and readers look for by that name. */}
      <div aria-live="polite" aria-atomic="true" style={SR_ONLY}>
        <span key={said.n}>{said.text}</span>
      </div>
      {/* Keyed on the open, so each opening starts fresh (focus, placement) even at the same spot. */}
      {menu && createPortal(<Menu key={`${menu.x},${menu.y},${menu.label}`} />, document.body)}
    </>
  );
}

/** Off-screen but read aloud. Inline, because a mod's frame has its own stylesheet and the kit
 *  can't count on a utility class being in it. */
const SR_ONLY = {
  position: "absolute",
  width: 1,
  height: 1,
  margin: -1,
  padding: 0,
  overflow: "hidden",
  clip: "rect(0 0 0 0)",
  whiteSpace: "nowrap",
  border: 0,
} as const;

function Menu() {
  const menu = useContextMenu()!;
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);

  // Measured before paint, so it never shows in the wrong place first.
  useLayoutEffect(() => {
    const r = ref.current!.getBoundingClientRect();
    setPos(placeMenu(menu.x, menu.y, r.width, r.height, window.innerWidth, window.innerHeight));
  }, [menu.x, menu.y]);

  // Focus only once it is placed and showing: until then it is `visibility: hidden`, and a browser
  // will not focus a hidden element, so the arrow keys and Escape did nothing. Opened by key, the
  // first item (the menu pattern's rule); with the mouse, the menu itself, so the arrow keys work at
  // once and no row starts lit up that the pointer never touched.
  const placed = pos !== null;
  useLayoutEffect(() => {
    if (!placed) return;
    const el = ref.current!;
    const first = menu.viaKeyboard ? el.querySelector<HTMLElement>(ROWS) : null;
    (first ?? el).focus();
  }, [placed, menu.viaKeyboard]);

  // Anything else ends it: a press outside, the window losing focus or resizing, a scroll.
  useEffect(() => {
    const onDown = (e: PointerEvent) => {
      if (!ref.current?.contains(e.target as Node)) closeContextMenu();
    };
    const close = () => closeContextMenu();
    const onScroll = (e: Event) => {
      if (!ref.current?.contains(e.target as Node)) closeContextMenu();
    };
    document.addEventListener("pointerdown", onDown, true);
    window.addEventListener("blur", close);
    window.addEventListener("resize", close);
    document.addEventListener("scroll", onScroll, true);
    return () => {
      document.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("blur", close);
      window.removeEventListener("resize", close);
      document.removeEventListener("scroll", onScroll, true);
    };
  }, []);

  const choose = (onSelect: () => void) => {
    const back = menu.returnFocus;
    closeContextMenu();
    // Back to where the user was BEFORE the action runs: a dialog the action opens remembers the
    // focused element as its opener, and the focused row is about to be removed with the menu.
    if (back?.isConnected) back.focus();
    onSelect();
    // And once more after it, for an action that left focus on nothing (the row it was on went away,
    // a field it focused was removed). Not when focus went somewhere on purpose, a dialog's own
    // field included: that stays where the action put it.
    queueMicrotask(() => {
      const at = document.activeElement;
      const lost = !at || at === document.body || ref.current?.contains(at);
      if (lost && back?.isConnected) back.focus();
    });
  };

  function onKeyDown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      closeContextMenu();
      menu.returnFocus?.focus();
      return;
    }
    if (e.key === "Tab") {
      e.preventDefault();
      closeContextMenu();
      menu.returnFocus?.focus();
      return;
    }
    const step =
      e.key === "ArrowDown" || e.key === "ArrowRight"
        ? 1
        : e.key === "ArrowUp" || e.key === "ArrowLeft"
          ? -1
          : 0;
    const items = [...ref.current!.querySelectorAll<HTMLElement>(ROWS)];
    if (!items.length) return;
    const at = items.indexOf(document.activeElement as HTMLElement);
    if (step) {
      e.preventDefault();
      // From the menu itself (nothing chosen yet), Down starts at the top and Up at the bottom.
      const next = at < 0 ? (step > 0 ? 0 : items.length - 1) : at + step;
      items[(next + items.length) % items.length].focus();
    } else if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      items[e.key === "Home" ? 0 : items.length - 1].focus();
    } else if (e.key.length === 1 && e.key !== " " && !e.ctrlKey && !e.metaKey && !e.altKey) {
      // Typing a letter jumps to the next row that starts with it.
      const from = items.indexOf(document.activeElement as HTMLElement) + 1;
      const hit = [...items.slice(from), ...items.slice(0, from)].find((el) =>
        rowName(el).toLowerCase().startsWith(e.key.toLowerCase()),
      );
      if (hit) {
        e.preventDefault();
        hit.focus();
      }
    }
  }

  return (
    <div
      ref={ref}
      role="menu"
      tabIndex={-1}
      aria-label={menu.label}
      // A press inside is a press inside whatever opened it: the pinboard note keeps its edit going
      // through a press in any popover panel (NoteBody's outside-press rule).
      data-popover-panel=""
      onKeyDown={onKeyDown}
      onContextMenu={(e) => e.preventDefault()}
      // Keep the selection and the caret where they were, so Copy and the table changes act on them.
      onMouseDown={(e) => e.preventDefault()}
      className="fixed z-[70] min-w-[11rem] outline-none max-w-[18rem] rounded-[var(--radius-sm)] border border-border2 bg-panel p-1 text-[0.8125rem] shadow-lg"
      style={pos ?? { left: menu.x, top: menu.y, visibility: "hidden" }}
    >
      {menu.items.map((item, i) => (
        <Row key={i} item={item} choose={choose} />
      ))}
    </div>
  );
}

/** What a row is called, for jumping to it by its first letter. */
const rowName = (el: HTMLElement) => el.dataset.label ?? el.getAttribute("aria-label") ?? "";

function Row({ item, choose }: { item: MenuItem; choose: (f: () => void) => void }) {
  if (item.kind === "separator")
    return <div role="separator" className="my-1 border-t border-[var(--rule)]" />;
  if (item.kind === "swatches")
    return (
      // `gap-2` between 16px dots puts their centres 24px apart, so the 24px target each one is owed
      // (WCAG 2.5.8) never overlaps its neighbour's.
      <div role="group" aria-label={item.label} className="flex items-center gap-2 px-2 py-1.5">
        <span className="mr-auto text-ink3">{item.label}</span>
        {item.options.map((o) => (
          <button
            key={o.color}
            type="button"
            role="menuitemradio"
            aria-checked={o.active}
            aria-label={o.name}
            title={o.name}
            onClick={() => choose(o.onSelect)}
            className={`h-4 w-4 rounded-full border focus:outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)] ${
              o.active ? "ring-1 ring-ink3 ring-offset-1 ring-offset-[var(--panel)]" : ""
            }`}
            style={{
              background: `var(--${o.color})`,
              borderColor: "color-mix(in oklab, var(--ink) 20%, transparent)",
            }}
          />
        ))}
      </div>
    );
  return (
    <button
      type="button"
      // One choice among several (a status) is a radio; anything else an action.
      role={item.checked === undefined ? "menuitem" : "menuitemradio"}
      aria-checked={item.checked}
      // See ROWS for why this is still the native attribute.
      disabled={item.disabled}
      data-label={item.label}
      aria-keyshortcuts={item.hint ? keyShortcut(item.hint) : undefined}
      onClick={() => choose(item.onSelect)}
      // `min-h-6`: a row is never under 24px tall (WCAG 2.5.8), whatever the text size.
      className={`flex min-h-6 w-full items-center gap-4 rounded-[var(--radius-sm)] px-2 py-1 text-left hover:bg-surface focus:bg-surface focus:outline-none disabled:opacity-50 disabled:hover:bg-transparent ${
        item.danger ? "text-st-due" : "text-ink2"
      }`}
    >
      {item.checked !== undefined && (
        <span aria-hidden className="-mr-2 w-3 shrink-0 text-accent-text">
          {item.checked ? "✓" : ""}
        </span>
      )}
      <span className="min-w-0 flex-1 break-words">{item.label}</span>
      {/* The shortcut is for the eye; its key is named in `aria-keyshortcuts` instead, so the item's
          name stays "Undo" rather than "UndoCtrl+Z". */}
      {item.hint && (
        <span aria-hidden className="shrink-0 font-mono text-[0.6875rem] text-ink4">
          {item.hint}
        </span>
      )}
    </button>
  );
}
