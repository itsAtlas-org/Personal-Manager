// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A date that may have parts missing, as three small fields: day, month and (often optional) year.
// Never a native date input: WebKitGTK draws none, and a birthday with no year has no place in one
// anyway. The fields hold typed text, so a half-typed date is a state the dialog can keep; the
// editor turns them into a stored value on Save (`isoFromFields`) and says what is wrong in words.

import type { DateFields } from "../../lib/partialDate";
import { Input } from "../ui";

interface Props {
  /** What the group is called: "Birthday", "Work 1 from". Each field's name starts with it. */
  name: string;
  fields: DateFields;
  onChange: (next: DateFields) => void;
  /** A problem with the date, said under it. */
  error?: string;
}

const FIELD: { key: keyof DateFields; word: string; hint: string; width: string; max: number }[] = [
  { key: "day", word: "day", hint: "DD", width: "w-14", max: 2 },
  { key: "month", word: "month", hint: "MM", width: "w-14", max: 2 },
  { key: "year", word: "year", hint: "YYYY", width: "w-20", max: 4 },
];

export function PartialDateFields({ name, fields, onChange, error }: Props) {
  return (
    <div className="flex flex-col gap-1">
      <div role="group" aria-label={name} className="flex items-center gap-1.5">
        {FIELD.map((f) => (
          <div key={f.key} className={f.width}>
            <Input
              inputMode="numeric"
              autoComplete="off"
              aria-label={`${name} ${f.word}`}
              aria-invalid={error ? true : undefined}
              placeholder={f.hint}
              value={fields[f.key]}
              // Typing past the width is refused rather than checked later: two digits of day,
              // four of year. The backend still checks the date is a real one.
              maxLength={f.max}
              onChange={(e) => onChange({ ...fields, [f.key]: e.target.value })}
            />
          </div>
        ))}
      </div>
      {error && (
        <p role="alert" className="text-xs text-st-due">
          {error}
        </p>
      )}
    </div>
  );
}
