// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The small pieces around the People list: the filter rail on its left (with the label controls: a
// button to add a label and, on each label's row, a small menu to rename or delete it), the
// upcoming-birthdays strip at the top of the All list, and the right pane's empty state. The record
// that fills the pane is `PersonRecord`.

import type { PeopleFilter, PersonLabel, PersonRow } from "../../lib/types";
import type { UpcomingBirthday } from "../../lib/people";
import { Button, Popover } from "../ui";

const sameFilter = (a: PeopleFilter, b: PeopleFilter) => JSON.stringify(a) === JSON.stringify(b);

interface RailProps {
  filter: PeopleFilter;
  labels: readonly PersonLabel[];
  onChange: (filter: PeopleFilter) => void;
  onNewLabel: () => void;
  onRenameLabel: (label: PersonLabel) => void;
  onDeleteLabel: (label: PersonLabel) => void;
}

export function FilterRail({
  filter,
  labels,
  onChange,
  onNewLabel,
  onRenameLabel,
  onDeleteLabel,
}: RailProps) {
  const item = (name: string, value: PeopleFilter, count?: number, label?: PersonLabel) => {
    const on = sameFilter(filter, value);
    return (
      <li key={JSON.stringify(value)} className="flex items-center gap-0.5">
        <button
          type="button"
          aria-current={on ? "true" : undefined}
          onClick={() => onChange(value)}
          className={`flex min-w-0 flex-1 items-center justify-between gap-2 rounded-[var(--radius-sm)] px-2 py-1.5 text-left text-sm ${
            on ? "bg-accent-soft text-ink" : "text-ink2 hover:bg-surface"
          }`}
        >
          <span className="min-w-0 truncate">{name}</span>
          {count !== undefined && <span className="shrink-0 text-xs text-ink3">{count}</span>}
        </button>
        {label && (
          <span data-help="people-labels">
            <Popover
              ariaLabel={`Label ${label.name}`}
              escapeClipping
              trigger={({ open, toggle }) => (
                <Button
                  variant="tertiary"
                  size="xs"
                  aria-haspopup="true"
                  aria-expanded={open}
                  aria-label={`Label options for ${label.name}`}
                  onClick={toggle}
                >
                  ⋯
                </Button>
              )}
              panelClassName="min-w-[9rem] p-1"
            >
              {({ close }) => (
                <div className="flex flex-col">
                  <Button
                    variant="tertiary"
                    size="xs"
                    className="justify-start"
                    onClick={() => {
                      close();
                      onRenameLabel(label);
                    }}
                  >
                    Rename…
                  </Button>
                  <Button
                    variant="tertiary"
                    size="xs"
                    className="justify-start"
                    onClick={() => {
                      close();
                      onDeleteLabel(label);
                    }}
                  >
                    Delete…
                  </Button>
                </div>
              )}
            </Popover>
          </span>
        )}
      </li>
    );
  };
  return (
    <nav aria-label="People filters" data-help="people-list" className="flex flex-col gap-2 p-2">
      <ul className="m-0 flex list-none flex-col gap-0.5 p-0">
        {item("All", "All")}
        {item("Starred", "Starred")}
        {labels.map((l) => item(l.name, { Label: l.id }, l.people, l))}
        {item("Unlabelled", "Unlabelled")}
        {item("Trash", "Trash")}
      </ul>
      <Button
        variant="tertiary"
        size="xs"
        data-help="people-labels"
        className="self-start"
        onClick={onNewLabel}
      >
        Add a label
      </Button>
    </nav>
  );
}

interface StripProps {
  birthdays: readonly UpcomingBirthday[];
  onOpen: (row: PersonRow) => void;
}

/** The next 30 days' birthdays; nothing at all when there are none. */
export function BirthdayStrip({ birthdays, onOpen }: StripProps) {
  if (birthdays.length === 0) return null;
  return (
    <section
      aria-label="Upcoming birthdays"
      data-help="people-list"
      className="border-b border-border px-4 py-2"
    >
      <h2 className="font-mono text-xs uppercase tracking-wide text-ink4">Coming up</h2>
      <ul className="m-0 mt-1 flex list-none flex-wrap gap-x-4 gap-y-1 p-0">
        {birthdays.map((b) => (
          <li key={b.row.id}>
            <button
              type="button"
              onClick={() => onOpen(b.row)}
              className="text-left text-sm text-ink2 hover:text-ink"
            >
              <span className="font-semibold text-ink">{b.row.display_name}</span>
              <span className="text-ink3">
                {" · "}
                {b.text}
                {b.inDays === 0 ? " (today)" : b.inDays === 1 ? " (tomorrow)" : ""}
              </span>
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}

/** The right pane with nobody open. */
export function NoPersonPane() {
  return (
    <div className="grid h-full place-items-center p-6 text-center">
      <p className="max-w-xs text-sm text-ink3">Nobody selected. Pick someone from the list.</p>
    </div>
  );
}
