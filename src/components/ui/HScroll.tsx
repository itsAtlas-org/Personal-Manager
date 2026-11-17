// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { ReactNode } from "react";

/**
 * A horizontally-scrollable container for a wide table / grid. Scroll-axis behaviour is now handled
 * globally (see lib/scrollAxis.ts, installed once at app scope): a vertical wheel scrolls the page
 * up/down even while the pointer is over this element, and a horizontal wheel / Shift+wheel scrolls it
 * sideways. So this is just a plain `overflow-x-auto` wrapper — it no longer binds the wheel itself
 * (which used to hijack a plain vertical wheel into horizontal panning, contradicting that global
 * rule). Wrap any wide table/grid in this rather than a bare `overflow-x-auto` so the intent stays
 * self-documenting.
 *
 * `label` names the region and makes it a tab stop: a scroll container with nothing focusable inside
 * can't be scrolled from the keyboard at all, so a table whose hidden columns matter passes one.
 */
export function HScroll({
  children,
  className,
  label,
}: {
  children: ReactNode;
  className?: string;
  label?: string;
}) {
  return (
    <div
      className={`overflow-x-auto ${className ?? ""}`}
      {...(label ? { role: "region", "aria-label": label, tabIndex: 0 } : {})}
    >
      {children}
    </div>
  );
}
