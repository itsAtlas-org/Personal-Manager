// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Inline tag chips + an add field (Review tab + the project focus panel). Commas are stripped —
// the vault serializes tags comma-separated.

import { useState } from "react";
import { cn } from "./ui";
import { CHIP_INPUT } from "./ui/chipInput";

export function TagEditor({
  tags,
  onChange,
  label = "Add a tag",
}: {
  tags: string[];
  onChange: (tags: string[]) => void;
  /** The add field's accessible name. A list of documents names each one's, so a screen reader
   *  can tell the rows apart. */
  label?: string;
}) {
  const [draft, setDraft] = useState("");

  function add() {
    // Commas aren't allowed in tags (the vault serializes them comma-separated).
    const tag = draft.replace(/,/g, "").trim().toLowerCase();
    setDraft("");
    if (tag && !tags.includes(tag)) onChange([...tags, tag]);
  }

  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {tags.map((tag) => (
        <span
          key={tag}
          className="inline-flex items-center gap-1 rounded-[var(--radius-sm)] bg-accent-soft px-2 py-0.5 text-xs text-accent-text"
        >
          {tag}
          <button
            onClick={() => onChange(tags.filter((t) => t !== tag))}
            className="text-ink4 hover:text-ink"
            title="Remove tag"
            aria-label={`Remove tag ${tag}`}
          >
            ×
          </button>
        </span>
      ))}
      <input
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === ",") {
            e.preventDefault();
            add();
          }
        }}
        onBlur={add}
        aria-label={label}
        placeholder="add tag…"
        className={cn("w-24", CHIP_INPUT)}
      />
    </div>
  );
}
