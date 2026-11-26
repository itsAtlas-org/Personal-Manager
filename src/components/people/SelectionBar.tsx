// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The "N selected" bar above the People list. It owns only its frame and the count: the view hands
// it the actions as children, so Merge (exactly two) and Export join later as one entry each, and
// the Trash view hands it Restore and Delete now instead of Star, Label and Move to Trash.

import type { ReactNode } from "react";
import { Button } from "../ui";

interface Props {
  count: number;
  onClear: () => void;
  children: ReactNode;
}

export function SelectionBar({ count, onClear, children }: Props) {
  return (
    <div
      role="region"
      aria-label="Selected people"
      data-help="people-list"
      className="flex flex-wrap items-center gap-2 border-b border-border bg-accent-soft px-4 py-2"
    >
      <span role="status" className="text-sm font-semibold text-ink">
        {count} selected
      </span>
      <div className="flex flex-wrap items-center gap-2">{children}</div>
      <Button variant="tertiary" size="xs" className="ml-auto" onClick={onClear}>
        Clear
      </Button>
    </div>
  );
}
