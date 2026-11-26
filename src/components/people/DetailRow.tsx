// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// One value in the editor: its label, the value itself (and the parts a job or an address has beside
// it), dates where the kind has them, and the arrows and the remove button that go with every row.
// The row holds nothing of its own except whether "Other label…" was picked; the dialog owns the
// model and every change goes up as a patch.
//
// Labels come as a short list of suggestions per kind plus "Other label…", which opens a free text
// field: a label is the person's own word ("Spotify", "Godmother"), never a closed list. Kinds whose
// label IS the content (a relationship, a custom field's name) are always the free field.

import { useState } from "react";
import { hasDates, type EditorRow, type Problems } from "../../lib/personDraft";
import type { PersonDetailKind } from "../../lib/types";
import { Button, Input, Select, Textarea } from "../ui";
import { PartialDateFields } from "./PartialDateFields";

/** The labels each kind suggests. */
const SUGGESTIONS: Partial<Record<PersonDetailKind, readonly string[]>> = {
  email: ["Home", "Work"],
  phone: ["Mobile", "Home", "Work"],
  address: ["Home", "Work"],
  web: ["Website", "Instagram", "LinkedIn", "Spotify", "Discord"],
  date: ["Anniversary"],
};

const OTHER = "__other__";

// An arrow at the end of its kind is aria-disabled, not disabled (so it can keep focus), and Button
// styles only `:disabled`: this gives it the same inert look, hover included.
const ARROW_END =
  "aria-disabled:cursor-not-allowed aria-disabled:text-faint aria-disabled:hover:text-faint";

/** Placeholders and the word each kind's value field is known by. */
const VALUE_WORDS: Partial<Record<PersonDetailKind, string>> = {
  email: "name@example.com",
  phone: "Phone number",
  web: "Link or handle",
  nickname: "Nickname",
  former_name: "Former name",
  relation: "Their name",
  custom: "Value",
  location: "Town, country or address",
};

interface Props {
  row: EditorRow;
  /** What the row is called to a screen reader: "Email 1". */
  name: string;
  problems: Problems;
  /** First and last of its kind, for the arrows. */
  first: boolean;
  last: boolean;
  onChange: (patch: Partial<EditorRow>) => void;
  onMove: (by: -1 | 1) => void;
  onRemove: () => void;
}

export function DetailRow({ row, name, problems, first, last, onChange, onMove, onRemove }: Props) {
  const at = (field: string) => problems[`row:${row.key}:${field}`];
  const suggestions = SUGGESTIONS[row.kind];
  const [otherPicked, setOtherPicked] = useState(false);
  // aria-disabled, not disabled, on the arrows (below): a row that reaches the end of its kind would
  // otherwise disable the very button that has focus, and focus would fall to the page.

  const trimmed = row.label.trim();
  const match = suggestions?.find((s) => s.toLowerCase() === trimmed.toLowerCase());
  const mode = otherPicked ? OTHER : match ? match : trimmed === "" ? "" : OTHER;

  const labelControl = suggestions ? (
    <div className="flex items-center gap-1.5">
      <Select
        compact
        aria-label={`${name} label`}
        value={mode}
        onChange={(e) => {
          const v = e.target.value;
          setOtherPicked(v === OTHER);
          if (v === OTHER) onChange({ label: match ? "" : row.label });
          else onChange({ label: v });
        }}
      >
        <option value="">No label</option>
        {suggestions.map((s) => (
          <option key={s} value={s}>
            {s}
          </option>
        ))}
        <option value={OTHER}>Other label…</option>
      </Select>
      {mode === OTHER && (
        <div className="w-32">
          <Input
            aria-label={`${name} own label`}
            placeholder="Your own label"
            value={row.label}
            onChange={(e) => {
              // Typing here is choosing "own label", even when the text comes to equal a suggestion
              // or goes empty: the box must stay under the cursor, not turn back into the list.
              setOtherPicked(true);
              onChange({ label: e.target.value });
            }}
          />
        </div>
      )}
    </div>
  ) : row.kind === "relation" || row.kind === "custom" ? (
    <div className="w-36">
      <Input
        aria-label={`${name} ${row.kind === "relation" ? "relationship" : "field name"}`}
        placeholder={row.kind === "relation" ? "Relationship" : "Field name"}
        value={row.label}
        onChange={(e) => onChange({ label: e.target.value })}
      />
    </div>
  ) : null;

  const err = (field: string) =>
    at(field) && (
      <p role="alert" className="text-xs text-st-due">
        {at(field)}
      </p>
    );

  let body;
  if (row.kind === "date") {
    body = (
      <PartialDateFields
        name={name}
        fields={row.when}
        error={at("when")}
        onChange={(when) => onChange({ when })}
      />
    );
  } else if (row.kind === "work" || row.kind === "school") {
    const work = row.kind === "work";
    body = (
      <div className="flex min-w-0 flex-1 flex-col gap-2">
        <div className="flex flex-wrap gap-2">
          <div className="min-w-40 flex-1">
            <Input
              aria-label={`${name} ${work ? "organisation" : "school"}`}
              placeholder={work ? "Company or organisation" : "School or university"}
              value={row.value}
              onChange={(e) => onChange({ value: e.target.value })}
            />
          </div>
          <div className="min-w-32 flex-1">
            <Input
              aria-label={`${name} ${work ? "job title" : "field of study"}`}
              placeholder={work ? "Job title" : "Field of study"}
              value={row.title}
              onChange={(e) => onChange({ title: e.target.value })}
            />
          </div>
          {work && (
            <div className="min-w-32 flex-1">
              <Input
                aria-label={`${name} department`}
                placeholder="Department"
                value={row.department}
                onChange={(e) => onChange({ department: e.target.value })}
              />
            </div>
          )}
        </div>
        {err("value")}
        {err("title")}
        {err("department")}
        <div className="flex flex-wrap items-start gap-x-4 gap-y-2 text-xs text-ink3">
          <div className="flex flex-col gap-1">
            <span>From</span>
            <PartialDateFields
              name={`${name} from`}
              fields={row.from}
              error={at("from")}
              onChange={(from) => onChange({ from })}
            />
          </div>
          <div className="flex flex-col gap-1">
            <span>To</span>
            <PartialDateFields
              name={`${name} to`}
              fields={row.to}
              error={at("to")}
              onChange={(to) => onChange({ to })}
            />
          </div>
          <label className="flex items-center gap-1.5 self-center text-ink2">
            <input
              type="checkbox"
              className="accent-[var(--accent)]"
              aria-label={`${name} is current`}
              checked={row.isCurrent}
              onChange={(e) => onChange({ isCurrent: e.target.checked })}
            />
            Current
          </label>
        </div>
      </div>
    );
  } else if (row.kind === "address") {
    body = (
      <div className="min-w-48 flex-1">
        <Textarea
          rows={3}
          aria-label={name}
          placeholder="Street, town, postcode, country"
          value={row.value}
          onChange={(e) => onChange({ value: e.target.value })}
        />
        {err("value")}
      </div>
    );
  } else {
    body = (
      <div className="flex min-w-0 flex-1 flex-col gap-2">
        <div className="min-w-40">
          <Input
            aria-label={name}
            placeholder={VALUE_WORDS[row.kind] ?? "Value"}
            value={row.value}
            onChange={(e) => onChange({ value: e.target.value })}
          />
        </div>
        {err("value")}
        {hasDates(row.kind) && (
          <div className="flex flex-col gap-1 text-xs text-ink3">
            <span>Since</span>
            <PartialDateFields
              name={`${name} since`}
              fields={row.from}
              error={at("from")}
              onChange={(from) => onChange({ from })}
            />
          </div>
        )}
      </div>
    );
  }

  return (
    <li data-row={row.key} className="flex flex-wrap items-start gap-2">
      {labelControl}
      {body}
      {err("label")}
      <div className="flex shrink-0 items-center gap-0.5">
        <Button
          variant="tertiary"
          size="xs"
          aria-label={`Move ${name} up`}
          aria-disabled={first}
          className={ARROW_END}
          onClick={() => {
            if (!first) onMove(-1);
          }}
        >
          ↑
        </Button>
        <Button
          variant="tertiary"
          size="xs"
          aria-label={`Move ${name} down`}
          aria-disabled={last}
          className={ARROW_END}
          onClick={() => {
            if (!last) onMove(1);
          }}
        >
          ↓
        </Button>
        <Button variant="tertiary" size="xs" aria-label={`Remove ${name}`} onClick={onRemove}>
          ×
        </Button>
      </div>
    </li>
  );
}
