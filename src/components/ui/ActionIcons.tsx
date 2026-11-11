// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Pictograms for icon-only action buttons. Hand-rolled in the house style (24x24 viewBox,
// stroke=currentColor, round caps) because no icon library is a dependency — see FormatIcons.tsx and
// settings/tabIcons.tsx. Decorative: the button around one carries the accessible name, so the SVGs
// are aria-hidden.

/** Two arrows chasing round a circle: Refresh. `spinning` turns it while the work runs, and not at
 *  all under reduced motion (the button's description still says it is refreshing). */
export function RefreshIcon({ spinning = false }: { spinning?: boolean }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className={`h-3.5 w-3.5 ${spinning ? "animate-spin motion-reduce:animate-none" : ""}`}
    >
      <path d="M21 12a9 9 0 1 1-2.64-6.36" />
      <path d="M21 3v6h-6" />
    </svg>
  );
}

/** A window with an arrow leaving its corner: Pop out, into a window of its own. */
export function PopOutIcon() {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className="h-3.5 w-3.5"
    >
      <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
      <path d="M15 3h6v6" />
      <path d="M10 14 21 3" />
    </svg>
  );
}
