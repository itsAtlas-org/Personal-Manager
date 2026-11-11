// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A small hover/focus tooltip. The app otherwise uses the native `title` attribute, which is fine
// almost everywhere — but WebView2 does NOT reliably show a `title` on a control while a <textarea>
// has focus, which is exactly the pinboard note's formatting toolbar (and tint dots) while editing.
// This JS tooltip guarantees the label + shortcut shows there. It renders through a portal to
// document.body so the note tile's `overflow-hidden` can never clip it.
//
// WCAG 2.2 1.4.13 (Content on Hover or Focus) is why it behaves as it does:
//   - Dismissible: Esc hides it without moving focus. The listener is on `document` and does NOT stop
//     the key, so an open dialog's own Esc still closes the dialog in the same press.
//   - Hoverable: the pointer may move from the control onto the tooltip (a short grace covers the
//     gap), so low-vision users who magnify can read it without it vanishing.
//   - Persistent: it stays while the pointer or keyboard focus is on the control or the tooltip.
// It is also kept inside the window: a tooltip centred on a control in a corner (the calendar's
// Refresh) would otherwise run off the edge, measured and clamped the way `Popover` is.

import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";

export interface TooltipProps {
  /** The text to show (e.g. `Bullet list · Ctrl+Shift+8`). */
  label: string;
  /** Above the control (the default), or below it for one at the top of the window, where above is
   *  off-screen (the title bar's window controls). */
  placement?: "above" | "below";
  children: ReactNode;
}

/** Breathing room kept between the tooltip and the window edge (as Popover's MARGIN). */
const MARGIN = 8;
/** The gap between the control and the tooltip. */
const GAP = 6;
/** How long the pointer may be between the control and the tooltip before it goes. */
const GRACE_MS = 150;

/** Whether this focus came from the keyboard (`:focus-visible`: a click on a button does not match).
 *  An engine that doesn't know the selector throws; treat that as keyboard, so the tooltip still
 *  shows on focus there. */
function isKeyboardFocus(target: EventTarget): boolean {
  try {
    return (target as Element).matches(":focus-visible");
  } catch {
    return true;
  }
}

export function Tooltip({ label, placement = "above", children }: TooltipProps) {
  const [shown, setShown] = useState(false);
  // Viewport coords for the shown tooltip. Null until measured, which also keeps it invisible for
  // that first frame so it never flashes at 0,0.
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const triggerRef = useRef<HTMLSpanElement>(null);
  const tipRef = useRef<HTMLSpanElement>(null);
  const showTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const hideTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  // Where the pointer and the focus are now: it stays while either is on the control or the tip.
  const hovering = useRef(false);
  const focused = useRef(false);

  const open = () => {
    clearTimeout(hideTimer.current);
    clearTimeout(showTimer.current);
    showTimer.current = setTimeout(() => setShown(true), 120);
  };
  const close = (grace: boolean) => {
    clearTimeout(showTimer.current);
    clearTimeout(hideTimer.current);
    if (grace) hideTimer.current = setTimeout(() => setShown(false), GRACE_MS);
    else setShown(false);
  };
  const enter = () => {
    hovering.current = true;
    open();
  };
  const leave = () => {
    hovering.current = false;
    if (!focused.current) close(true);
  };

  useEffect(
    () => () => {
      clearTimeout(showTimer.current);
      clearTimeout(hideTimer.current);
    },
    [],
  );

  // Esc dismisses it. Only listening while it is up, so a closed tooltip costs no listener.
  useEffect(() => {
    if (!shown) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close(false);
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [shown]);

  // Place it against the control once it is in the DOM and can be measured (a layout effect, so
  // this happens before the browser paints): centred, then clamped inside the window. Re-placed when
  // the words change under it ("Refresh · last synced …" becoming "Refreshing…").
  useLayoutEffect(() => {
    if (!shown) {
      setPos(null);
      return;
    }
    const anchor = triggerRef.current?.getBoundingClientRect();
    const tip = tipRef.current;
    if (!anchor || !tip) return;
    const w = tip.offsetWidth;
    const h = tip.offsetHeight;
    const left = Math.max(
      MARGIN,
      Math.min(anchor.left + anchor.width / 2 - w / 2, window.innerWidth - w - MARGIN),
    );
    const wanted = placement === "below" ? anchor.bottom + GAP : anchor.top - GAP - h;
    const top = Math.max(MARGIN, Math.min(wanted, window.innerHeight - h - MARGIN));
    setPos((p) => (p && p.left === left && p.top === top ? p : { left, top }));
  }, [shown, label, placement]);

  return (
    <span
      ref={triggerRef}
      className="inline-flex"
      onPointerEnter={enter}
      onPointerLeave={leave}
      // A press on the control dismisses it (the tooltip is in the way of what was pressed); a press
      // on the tooltip itself, which React bubbles through the portal, does not.
      onPointerDown={(e) => {
        if (triggerRef.current?.contains(e.target as Node)) close(false);
      }}
      onFocusCapture={(e) => {
        // Only KEYBOARD focus is the 1.4.13 "persistent" case. A click also focuses a button, and
        // counting that would pin the tooltip under it after the pointer left (leave() defers to
        // focus), so a mouse press would end with a tooltip stuck over the grid.
        if (!isKeyboardFocus(e.target)) return;
        focused.current = true;
        open();
      }}
      onBlurCapture={() => {
        focused.current = false;
        if (!hovering.current) close(false);
      }}
    >
      {children}
      {shown &&
        createPortal(
          <span
            ref={tipRef}
            role="tooltip"
            style={{ left: pos?.left ?? 0, top: pos?.top ?? 0 }}
            // No pointer-events-none: the pointer has to be able to rest on it (1.4.13).
            className={`fixed z-[60] whitespace-nowrap rounded-[var(--radius-sm)] border border-border2 bg-panel px-1.5 py-0.5 text-[0.625rem] text-ink2 shadow-md ${
              pos ? "" : "invisible"
            }`}
            onPointerEnter={enter}
            onPointerLeave={(e) => {
              // Back onto the control: React hands the control no pointerenter (the tip is its
              // React child, so the pointer never left it), so leaving would close it under a
              // pointer that is on the control.
              const to = e.relatedTarget;
              if (to instanceof Node && triggerRef.current?.contains(to)) return;
              leave();
            }}
          >
            {label}
          </span>,
          document.body,
        )}
    </span>
  );
}
