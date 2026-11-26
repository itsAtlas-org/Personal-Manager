// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The small pieces around the People list: the filter rail on its left, the upcoming-birthdays strip
// at the top of the All list, and the right pane, which for now says who is open (the record itself
// is the next slice's) or that nobody is.

import type { PeopleFilter, PersonLabel, PersonRow } from "../../lib/types";
import {
  personCircleBackground,
  personInitials,
  secondaryLine,
  type UpcomingBirthday,
} from "../../lib/people";

const sameFilter = (a: PeopleFilter, b: PeopleFilter) => JSON.stringify(a) === JSON.stringify(b);

interface RailProps {
  filter: PeopleFilter;
  labels: readonly PersonLabel[];
  onChange: (filter: PeopleFilter) => void;
}

export function FilterRail({ filter, labels, onChange }: RailProps) {
  const item = (name: string, value: PeopleFilter, count?: number) => {
    const on = sameFilter(filter, value);
    return (
      <li key={JSON.stringify(value)}>
        <button
          type="button"
          aria-current={on ? "true" : undefined}
          onClick={() => onChange(value)}
          className={`flex w-full items-center justify-between gap-2 rounded-[var(--radius-sm)] px-2 py-1.5 text-left text-sm ${
            on ? "bg-accent-soft text-ink" : "text-ink2 hover:bg-surface"
          }`}
        >
          <span className="min-w-0 truncate">{name}</span>
          {count !== undefined && <span className="shrink-0 text-xs text-ink3">{count}</span>}
        </button>
      </li>
    );
  };
  return (
    <nav aria-label="People filters" data-help="people-list" className="p-2">
      <ul className="m-0 flex list-none flex-col gap-0.5 p-0">
        {item("All", "All")}
        {item("Starred", "Starred")}
        {labels.map((l) => item(l.name, { Label: l.id }, l.people))}
        {item("Unlabelled", "Unlabelled")}
        {item("Trash", "Trash")}
      </ul>
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

interface PaneProps {
  row: PersonRow | null;
}

/** The right pane. With nobody open it says so; with someone open it shows who, until the record
 *  itself takes its place. */
export function PeoplePane({ row }: PaneProps) {
  if (!row) {
    return (
      <div className="grid h-full place-items-center p-6 text-center">
        <p className="max-w-xs text-sm text-ink3">Nobody selected. Pick someone from the list.</p>
      </div>
    );
  }
  const second = secondaryLine(row);
  return (
    <section aria-label={row.display_name} className="flex flex-col items-center gap-2 p-8">
      <span
        aria-hidden
        style={{ background: personCircleBackground(row.uid) }}
        className="grid h-16 w-16 place-items-center rounded-full text-xl font-semibold text-ink"
      >
        {personInitials(row.display_name)}
      </span>
      <h2 className="font-head text-lg font-semibold text-ink">{row.display_name}</h2>
      {second && <p className="text-sm text-ink3">{second}</p>}
      {row.labels.length > 0 && (
        <ul className="m-0 flex list-none flex-wrap justify-center gap-1 p-0">
          {row.labels.map((l) => (
            <li
              key={l.id}
              className="rounded-full border border-ink4 px-2 py-0.5 text-xs text-ink3"
            >
              {l.name}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
