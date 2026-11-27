// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The rows of the People list. Each row is its initials circle (which becomes a checkbox on hover
// or keyboard focus, Google's pattern, and stays one while anything is selected), the name with one
// secondary line, the label chips and a star. There is no virtualisation library: a long list is
// kept cheap by `content-visibility: auto` on each row with a placeholder height, the way the
// Documents table does it, so the scrollbar stays steady while off-screen rows are skipped.
//
// One tab stop for the whole list (the row you last touched), arrow keys between rows. The menu is
// ONE handler on the list: it finds the row under the pointer, so a right-click, the Menu key and
// Shift+F10 on any control in a row all land on it. Dialogs are rendered by the view, outside this
// element, because PM's Modal isn't portalled and a right-click inside one would otherwise bubble
// here and open the row menu over the dialog.

import { useState, type KeyboardEvent, type MouseEvent } from "react";
import { formatDate } from "../../lib/format";
import {
  personCircleBackground,
  personInitials,
  secondaryLine,
  visibleLabels,
} from "../../lib/people";
import type { PersonRow } from "../../lib/types";
import { Button } from "../ui";

/** The row's height before it is drawn, so `content-visibility: auto` keeps the scrollbar steady. */
const ROW_PLACEHOLDER = "auto 3.5rem";

interface Props {
  rows: readonly PersonRow[];
  /** The people ticked for the bulk actions. */
  checked: ReadonlySet<number>;
  /** The person whose record is open. */
  activeId: number | null;
  /** The list is the Trash: rows say since when, and offer Restore where the star is. */
  inTrash: boolean;
  onOpen: (row: PersonRow, e: MouseEvent) => void;
  /** Tick or untick one row; `shift` extends from the last one touched. */
  onToggle: (row: PersonRow, shift: boolean) => void;
  onStar: (row: PersonRow) => void;
  onRestore: (row: PersonRow) => void;
  onMenu: (e: MouseEvent, row: PersonRow) => void;
}

export function PeopleList({
  rows,
  checked,
  activeId,
  inTrash,
  onOpen,
  onToggle,
  onStar,
  onRestore,
  onMenu,
}: Props) {
  // The row that holds the list's one tab stop: the one last focused, else the open one, else the first.
  const [stop, setStop] = useState<number | null>(null);
  const tabId =
    [stop, activeId].find((id) => id !== null && rows.some((r) => r.id === id)) ?? rows[0]?.id;
  const selecting = checked.size > 0;

  function onKeyDown(e: KeyboardEvent<HTMLUListElement>) {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    const li = (e.target as HTMLElement).closest<HTMLElement>("[data-person-id]");
    if (!li) return;
    const at = rows.findIndex((r) => r.id === Number(li.dataset.personId));
    const to = rows[at + (e.key === "ArrowDown" ? 1 : -1)];
    if (!to) return;
    e.preventDefault();
    e.currentTarget.querySelector<HTMLElement>(`[data-person-main="${to.id}"]`)?.focus();
  }

  function onContextMenu(e: MouseEvent<HTMLUListElement>) {
    const li = (e.target as HTMLElement).closest<HTMLElement>("[data-person-id]");
    const row = li && rows.find((r) => r.id === Number(li.dataset.personId));
    if (row) onMenu(e, row);
  }

  return (
    <ul
      aria-label={inTrash ? "People in Trash" : "People"}
      className="m-0 list-none p-0"
      onKeyDown={onKeyDown}
      onContextMenu={onContextMenu}
    >
      {rows.map((row) => {
        const isChecked = checked.has(row.id);
        const isActive = row.id === activeId;
        const tab = row.id === tabId ? 0 : -1;
        const chips = visibleLabels(row.labels);
        const second =
          inTrash && row.trashed_at
            ? `In Trash since ${formatDate(row.trashed_at)}`
            : secondaryLine(row);
        return (
          <li
            key={row.id}
            data-person-id={row.id}
            onFocus={() => setStop(row.id)}
            style={{ contentVisibility: "auto", containIntrinsicSize: ROW_PLACEHOLDER }}
            className={`group flex items-center gap-3 border-b border-border px-4 py-2 ${
              isActive ? "bg-accent-soft" : isChecked ? "bg-surface" : "hover:bg-surface"
            }`}
          >
            <span className="relative grid h-9 w-9 shrink-0 place-items-center">
              {/* The circle gives way to the checkbox while the row is pointed at or focused, and
                  for good once anything is ticked, so a selection can be extended without hovering. */}
              <span
                aria-hidden
                style={{ background: personCircleBackground(row.uid) }}
                className={`grid h-9 w-9 place-items-center rounded-full text-xs font-semibold text-ink group-focus-within:opacity-0 group-hover:opacity-0 ${
                  selecting || isChecked ? "opacity-0" : ""
                }`}
              >
                {personInitials(row.display_name)}
              </span>
              <input
                type="checkbox"
                aria-label={`Select ${row.display_name}`}
                checked={isChecked}
                tabIndex={tab}
                onChange={() => {}}
                onClick={(e) => onToggle(row, e.shiftKey)}
                className={`absolute inset-0 m-auto h-5 w-5 cursor-pointer accent-[var(--accent)] group-focus-within:opacity-100 group-hover:opacity-100 ${
                  selecting || isChecked ? "opacity-100" : "opacity-0"
                }`}
              />
            </span>
            <button
              type="button"
              data-person-main={row.id}
              tabIndex={tab}
              aria-current={isActive ? "true" : undefined}
              onClick={(e) => onOpen(row, e)}
              className="min-w-0 flex-1 text-left"
            >
              <span className="block truncate text-sm text-ink">{row.display_name}</span>
              {second && <span className="block truncate text-xs text-ink3">{second}</span>}
            </button>
            {chips.shown.length > 0 && (
              <span className="hidden shrink-0 items-center gap-1 sm:flex">
                {chips.shown.map((l) => (
                  <span
                    key={l.id}
                    className="max-w-[8rem] truncate rounded-full border border-ink4 px-2 py-0.5 text-xs text-ink3"
                  >
                    {l.name}
                  </span>
                ))}
                {chips.more > 0 && <span className="text-xs text-ink3">+{chips.more}</span>}
              </span>
            )}
            {inTrash ? (
              <Button
                variant="tertiary"
                size="xs"
                tabIndex={tab}
                aria-label={`Restore ${row.display_name}`}
                onClick={() => onRestore(row)}
              >
                Restore
              </Button>
            ) : (
              <button
                type="button"
                tabIndex={tab}
                aria-pressed={row.starred}
                aria-label={`Star ${row.display_name}`}
                onClick={() => onStar(row)}
                className={`shrink-0 px-1 text-base leading-none ${
                  row.starred ? "text-st-look" : "text-ink4 hover:text-ink2"
                }`}
              >
                {row.starred ? "★" : "☆"}
              </button>
            )}
          </li>
        );
      })}
    </ul>
  );
}
