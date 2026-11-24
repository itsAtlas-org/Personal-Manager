// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { ComponentPropsWithRef } from "react";
import { useTheme } from "../theme";
import { cn } from "./cn";

interface Props extends ComponentPropsWithRef<"select"> {
  /** Compact sizing for dense toolbars. Height comes from symmetric padding + the font's own
   *  line-height — never a fixed `h-*` — so the selected text stays vertically centred on every
   *  engine. (WebKitGTK on Linux anchors native-<select> text lower than Blink, so a fixed height
   *  with zeroed padding clipped the descenders there; padding-driven sizing renders identically.) */
  compact?: boolean;
  /** The height of a SegmentedControl, for a select that sits in a row with one (the Calendar
   *  header's Days). Same sizing as Button's `bar`; wins over `compact`. */
  bar?: boolean;
}

// Token-driven wrapper over the native <select>. color-scheme (set by applyTheme) makes the
// native dropdown follow light/dark automatically.
export function Select({ className, children, compact = false, bar = false, ...rest }: Props) {
  const { system } = useTheme();
  // Swap the size classes, never layer them: cn() is a plain joiner, so emitting both py-1.5 and py-1
  // would leave the winner to stylesheet order — the ambiguity that let the clip through.
  // `bar` also carries its own height floor, since a select has none of Button's.
  const sizing = bar
    ? "min-h-[calc(max(var(--tap-min,24px),1.5rem)_+_0.25rem_+_2px)] px-2 py-1.5 text-xs"
    : compact
      ? "px-2 py-1 text-xs"
      : "px-2 py-1.5 text-sm";
  return (
    <select
      className={cn(
        // Edge `--ink4` (3:1, WCAG 1.4.11), focus `--accent-text`; see Input.tsx for the measurements.
        "rounded-[var(--radius-sm)] border border-ink4 bg-surface text-ink2 outline-none transition focus:border-accent-text disabled:cursor-not-allowed disabled:border-border2 disabled:bg-transparent disabled:text-ink3",
        sizing,
        system === "terminal" && "font-mono",
        className,
      )}
      {...rest}
    >
      {children}
    </select>
  );
}
