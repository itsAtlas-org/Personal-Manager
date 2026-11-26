// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Export {n} people" (spec 5.3): who is covered, one tick for the timeline, then the backend opens
// the save dialog (the path never passes through the webview) and writes a vCard file, with the
// timeline as a spreadsheet beside it when ticked. People in Trash are never written. A cancelled
// save dialog leaves this open and says nothing. The result names the files written.
//
// The caller hands in the choices: from a selection it is exactly those people; from the header it
// is everyone, or, when one label is showing, that label's people first. A choice whose count isn't
// known is counted here (the same list read the tab uses), so the title can say how many.

import { useEffect, useId, useState } from "react";
import { exportPeople, listPeople } from "../../lib/ipc";
import { errorWords } from "../../lib/people";
import {
  exportTitle,
  exportedLine,
  peopleCount,
  type ExportChoice,
} from "../../lib/peopleTransfer";
import type { PeopleExportDone } from "../../lib/types";
import { Button, Callout, Dialog, RadioChoice } from "../ui";

interface Props {
  choices: readonly ExportChoice[];
  onClose: () => void;
}

export function ExportPeopleDialog({ choices, onClose }: Props) {
  const base = useId();
  const [key, setKey] = useState(choices[0]?.key ?? "all");
  const [timeline, setTimeline] = useState(false);
  const [counts, setCounts] = useState<Record<string, number>>({});
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [done, setDone] = useState<PeopleExportDone | null>(null);

  // Count every choice that arrived without a number. A failed read leaves it unknown: the export
  // still works, the title just doesn't say how many.
  useEffect(() => {
    let cancelled = false;
    for (const c of choices) {
      if (c.count !== null || c.filter === null) continue;
      void Promise.resolve(listPeople(c.filter, null, "First"))
        .then((rows) => {
          if (!cancelled && Array.isArray(rows))
            setCounts((prev) => ({ ...prev, [c.key]: rows.length }));
        })
        .catch(() => {});
    }
    return () => {
      cancelled = true;
    };
  }, [choices]);

  const countOf = (c: ExportChoice) => c.count ?? counts[c.key] ?? null;
  const current = choices.find((c) => c.key === key) ?? choices[0];
  const count = current ? countOf(current) : null;

  async function run() {
    if (!current || busy) return;
    setBusy(true);
    setProblem(null);
    try {
      const result = await exportPeople(current.scope, timeline);
      // null: the save dialog was cancelled. Nothing was written, and nothing needs saying.
      if (result !== null) setDone(result);
    } catch (err) {
      setProblem(errorWords(err, "PM couldn't export. Nothing was written."));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog
      open
      onClose={() => {
        if (!busy) onClose();
      }}
      title={done ? "Export finished" : exportTitle(count)}
      helpId="people-export"
      widthClassName="max-w-lg"
      footer={
        done ? (
          <Button variant="primary" onClick={onClose}>
            Done
          </Button>
        ) : (
          <>
            <Button variant="tertiary" onClick={onClose} disabled={busy}>
              Cancel
            </Button>
            <Button variant="primary" onClick={() => void run()} disabled={busy || count === 0}>
              {busy ? "Exporting…" : "Export"}
            </Button>
          </>
        )
      }
    >
      {done ? (
        <div className="mt-2 flex flex-col gap-2 text-sm text-ink2">
          <p role="status" className="text-ink">
            {exportedLine(done)}
          </p>
          <p>{done.files.length === 1 ? "The file:" : "The files:"}</p>
          <ul className="list-disc pl-5">
            {done.files.map((f) => (
              <li key={f} className="break-all">
                {f}
              </li>
            ))}
          </ul>
        </div>
      ) : (
        <>
          {choices.length > 1 && (
            <fieldset className="mt-2">
              <legend className="text-xs font-medium text-ink3">Who to export</legend>
              {choices.map((c) => {
                const n = countOf(c);
                return (
                  <RadioChoice
                    key={c.key}
                    name={`${base}-who`}
                    value={c.key}
                    current={key}
                    onSelect={setKey}
                    label={c.label}
                    detail={n === null ? "Counting…" : peopleCount(n)}
                    disabled={busy}
                  />
                );
              })}
            </fieldset>
          )}
          {count === 0 && <p className="mt-2 text-sm text-ink3">There is nobody here to export.</p>}
          <label
            htmlFor={`${base}-timeline`}
            className="mt-3 flex cursor-pointer items-center gap-2 text-sm text-ink2"
          >
            <input
              id={`${base}-timeline`}
              type="checkbox"
              className="accent-[var(--accent)]"
              checked={timeline}
              disabled={busy}
              onChange={(e) => setTimeline(e.target.checked)}
            />
            Include the timeline (a spreadsheet file)
          </label>
          <p className="mt-2 text-xs text-ink4">
            People in Trash are left out. PM asks where to save the file next.
          </p>
        </>
      )}
      {problem && (
        <Callout tone="danger" size="sm" className="mt-3">
          {problem}
        </Callout>
      )}
    </Dialog>
  );
}
