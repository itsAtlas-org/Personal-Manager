// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Merge with…" from a row's right-click menu: pick the other person, then the merge dialog opens.
// A small search over `search_people` (the same rules as the tab's own search and Ctrl K), which
// never returns anyone in Trash, so a person can't be picked who couldn't be merged.

import { useEffect, useId, useState } from "react";
import { searchPeople } from "../../lib/ipc";
import { errorWords, secondaryLine } from "../../lib/people";
import type { PersonRow } from "../../lib/types";
import { Button, Callout, Dialog, Input } from "../ui";

const SEARCH_DELAY_MS = 200;
const RESULTS_MAX = 12;

interface Props {
  /** The person being merged: left out of the results. */
  from: PersonRow;
  onPick: (other: PersonRow) => void;
  onClose: () => void;
}

export function MergeWithDialog({ from, onPick, onClose }: Props) {
  const inputId = useId();
  const [typed, setTyped] = useState("");
  const [found, setFound] = useState<PersonRow[]>([]);
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    const q = typed.trim();
    if (q === "") {
      setFound([]);
      return;
    }
    let cancelled = false;
    const t = window.setTimeout(() => {
      void searchPeople(q, RESULTS_MAX + 1)
        .then((rows) => {
          if (cancelled) return;
          setProblem(null);
          setFound(rows.filter((r) => r.id !== from.id).slice(0, RESULTS_MAX));
        })
        .catch((err: unknown) => {
          if (!cancelled) setProblem(errorWords(err));
        });
    }, SEARCH_DELAY_MS);
    return () => {
      cancelled = true;
      window.clearTimeout(t);
    };
  }, [typed, from.id]);

  return (
    <Dialog
      open
      onClose={onClose}
      title={`Merge ${from.display_name} with…`}
      subtitle="Pick the person to merge them with."
      helpId="people-merge"
      footer={
        <Button variant="tertiary" onClick={onClose}>
          Cancel
        </Button>
      }
    >
      <label htmlFor={inputId} className="mt-4 block text-xs text-ink3">
        Search people
      </label>
      <Input
        id={inputId}
        type="search"
        autoFocus
        value={typed}
        placeholder="A name, email, company or phone number"
        onChange={(e) => setTyped(e.target.value)}
      />
      {problem && (
        <Callout tone="danger" size="sm" className="mt-2">
          {problem}
        </Callout>
      )}
      {typed.trim() !== "" && found.length === 0 && !problem && (
        <p className="mt-3 text-sm text-ink3">Nobody else matches.</p>
      )}
      {found.length > 0 && (
        <ul aria-label="People to merge with" className="mt-3 flex flex-col gap-1">
          {found.map((r) => (
            <li key={r.id}>
              <button
                type="button"
                onClick={() => onPick(r)}
                className="flex w-full flex-col rounded-[var(--radius-sm)] px-2 py-1.5 text-left hover:bg-surface focus-visible:outline-2 focus-visible:outline-accent-text"
              >
                <span className="text-sm text-ink">{r.display_name}</span>
                {secondaryLine(r) && <span className="text-xs text-ink3">{secondaryLine(r)}</span>}
              </button>
            </li>
          ))}
        </ul>
      )}
    </Dialog>
  );
}
