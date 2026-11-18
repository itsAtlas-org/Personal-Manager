// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The header's date label doubles as a mini-calendar picker: click the period label to open a small
// month grid, page months with ‹ ›, and click a day to jump the cursor there (then it closes). Purely
// navigational — it moves the view, it never edits an event. Reuses the Popover primitive and the
// shared MiniMonth; no colour of its own.

import { Popover } from "../ui";
import { MonthPicker } from "./parts/MonthPicker";

interface Props {
  /** The current period label shown in the header (e.g. "July 2026"). */
  label: string;
  /** The current anchor day (highlighted as selected). */
  cursor: Date;
  /** Jump the view to a day. */
  onPick: (d: Date) => void;
}

export function MiniCalendarPopover({ label, cursor, onPick }: Props) {
  return (
    <Popover
      align="left"
      ariaLabel="Jump to a date"
      trigger={({ open, toggle }) => (
        <button
          type="button"
          onClick={toggle}
          aria-expanded={open}
          // The title stays text-sm (it is the header's heading, not a control label), but its box is
          // the header row's one height, so its hover highlight is as tall as the buttons beside it.
          // Same floor as Button's `bar` size; the text is centred in it by flex.
          className="flex min-h-[calc(max(var(--tap-min,24px),1.5rem)_+_0.25rem_+_2px)] items-center gap-1 rounded-[var(--radius-sm)] px-1 font-head text-sm text-ink hover:bg-surface"
          title="Pick a date"
        >
          {label}
          <span className="text-ink4">⌄</span>
        </button>
      )}
    >
      {({ close }) => (
        <MonthPicker
          selected={cursor}
          onPick={(d) => {
            onPick(d);
            close();
          }}
        />
      )}
    </Popover>
  );
}
