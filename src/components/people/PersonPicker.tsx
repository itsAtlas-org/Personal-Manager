// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A small person picker for the meeting card's "Someone else…": a search box over `search_people`
// (the same rules as the tab's own search and Ctrl K, which never returns anyone in Trash) and the
// first few matches to pick from. It uses ipc directly and holds no app state, so it works in the
// pop-out Calendar window too.

import { useEffect, useId, useState } from "react";
import { searchPeople } from "../../lib/ipc";
import { errorWords, secondaryLine } from "../../lib/people";
import type { PersonRow } from "../../lib/types";
import { Callout, Input } from "../ui";

const SEARCH_DELAY_MS = 200;
const RESULTS_MAX = 6;

interface Props {
  /** Who is on the card already: left out of the results. */
  exclude: ReadonlySet<number>;
  onPick: (person: PersonRow) => void;
  /** What the box is called to a screen reader; the card names its meeting. */
  label?: string;
}

export function PersonPicker({ exclude, onPick, label = "Search people" }: Props) {
  const inputId = useId();
  const [typed, setTyped] = useState("");
  const [found, setFound] = useState<PersonRow[]>([]);
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    const q = typed.trim();
    if (q === "") {
      setFound([]);
      setProblem(null);
      return;
    }
    let cancelled = false;
    const t = window.setTimeout(() => {
      void searchPeople(q, RESULTS_MAX + exclude.size)
        .then((rows) => {
          if (cancelled) return;
          setProblem(null);
          setFound(rows.filter((r) => !exclude.has(r.id)).slice(0, RESULTS_MAX));
        })
        .catch((err: unknown) => {
          if (!cancelled) setProblem(errorWords(err));
        });
    }, SEARCH_DELAY_MS);
    return () => {
      cancelled = true;
      window.clearTimeout(t);
    };
  }, [typed, exclude]);

  return (
    <div className="flex flex-col gap-1.5">
      <label htmlFor={inputId} className="sr-only">
        {label}
      </label>
      <Input
        id={inputId}
        type="search"
        autoComplete="off"
        // It only exists because "Someone else…" was just pressed: take the typing from there.
        autoFocus
        placeholder="A name, email, company or phone number"
        value={typed}
        onChange={(e) => setTyped(e.target.value)}
      />
      {problem && (
        <Callout tone="danger" size="sm">
          {problem}
        </Callout>
      )}
      {typed.trim() !== "" && found.length === 0 && !problem && (
        <p className="text-xs text-ink3">Nobody else matches.</p>
      )}
      {found.length > 0 && (
        <ul aria-label="People found" className="m-0 flex list-none flex-col gap-0.5 p-0">
          {found.map((r) => (
            <li key={r.id}>
              <button
                type="button"
                onClick={() => {
                  onPick(r);
                  setTyped("");
                }}
                className="flex w-full flex-col rounded-[var(--radius-sm)] px-2 py-1 text-left hover:bg-surface focus-visible:outline-2 focus-visible:outline-accent-text"
              >
                <span className="text-sm text-ink">{r.display_name}</span>
                {secondaryLine(r) && <span className="text-xs text-ink3">{secondaryLine(r)}</span>}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
