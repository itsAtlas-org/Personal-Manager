// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The question a 29 February birthday asks: which day to mark it on in years without one. A required
// choice with neither answer preselected (E22): people born on 29 February celebrate on 28 February
// or on 1 March and it varies, so PM asks rather than guesses.

import { useId } from "react";
import type { LeapDay } from "../../lib/types";

interface Props {
  value: LeapDay | null;
  onChange: (next: LeapDay) => void;
  /** Why the choice is still wanted, said under it. */
  error?: string;
}

const CHOICES: { value: LeapDay; label: string }[] = [
  { value: "feb28", label: "28 February" },
  { value: "mar1", label: "1 March" },
];

export function LeapDayChoice({ value, onChange, error }: Props) {
  const name = useId();
  return (
    <fieldset className="m-0 flex flex-col gap-1 border-0 p-0">
      <legend className="p-0 text-sm text-ink2">In years without 29 February, mark it on</legend>
      <div className="flex flex-wrap gap-x-4 gap-y-1">
        {CHOICES.map((c) => {
          const id = `${name}-${c.value}`;
          return (
            <label key={c.value} htmlFor={id} className="flex cursor-pointer items-center gap-2">
              <input
                id={id}
                type="radio"
                name={name}
                checked={value === c.value}
                aria-invalid={error ? true : undefined}
                onChange={() => onChange(c.value)}
                className="shrink-0"
              />
              <span className="text-sm text-ink2">{c.label}</span>
            </label>
          );
        })}
      </div>
      {error && (
        <p role="alert" className="text-xs text-st-due">
          {error}
        </p>
      )}
    </fieldset>
  );
}
