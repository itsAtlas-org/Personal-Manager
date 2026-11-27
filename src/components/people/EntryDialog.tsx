// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// One timeline entry, to log or to change (spec 6.1): what happened (met, call, message or note), the
// day (today unless it says otherwise) as three small fields (never a native date input, which
// WebKitGTK doesn't draw) and a note. "Log a meeting" opens it for a meeting; "+ Log" on the record
// and Edit on an entry open it too. The owner does the write and throws on a refusal, which is said
// here in words and leaves the dialog open with what was typed.

import { useId, useState } from "react";
import { fieldsFromIso, isoFromFields, type DateFields } from "../../lib/partialDate";
import { civilToday } from "../../lib/meetings";
import { errorWords } from "../../lib/people";
import { KIND_WORDS } from "../../lib/timeline";
import type { InteractionKind } from "../../lib/types";
import { Button, Callout, Dialog, Select, Textarea } from "../ui";
import { PartialDateFields } from "./PartialDateFields";

export interface EntryValues {
  kind: InteractionKind;
  /** `YYYY-MM-DD`. */
  on_date: string;
  note: string;
}

interface Props {
  title: string;
  subtitle?: string;
  /** What is being edited; absent for a new entry. */
  initial?: EntryValues;
  /** The kind a new entry starts as. */
  defaultKind?: InteractionKind;
  /** The user's zone, for "today". */
  zone?: string | null;
  save: (values: EntryValues) => Promise<void>;
  onClose: () => void;
}

const KINDS = Object.keys(KIND_WORDS) as InteractionKind[];

export function EntryDialog({
  title,
  subtitle,
  initial,
  defaultKind = "met",
  zone,
  save,
  onClose,
}: Props) {
  const base = useId();
  const [kind, setKind] = useState<InteractionKind>(initial?.kind ?? defaultKind);
  const [fields, setFields] = useState<DateFields>(() =>
    fieldsFromIso(initial?.on_date ?? civilToday(zone, new Date())),
  );
  const [note, setNote] = useState(initial?.note ?? "");
  const [dateError, setDateError] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit() {
    if (busy) return;
    const date = isoFromFields(fields, "dated");
    if (date.error !== null) {
      setDateError(date.error);
      return;
    }
    // A day, not a month or a year: an entry says which day it was.
    if (date.iso === null || !/^\d{4}-\d{2}-\d{2}$/.test(date.iso)) {
      setDateError("Give the day, the month and the year.");
      return;
    }
    setDateError(null);
    setProblem(null);
    setBusy(true);
    try {
      await save({ kind, on_date: date.iso, note });
      onClose();
    } catch (err) {
      setProblem(errorWords(err));
      setBusy(false);
    }
  }

  return (
    <Dialog
      open
      onClose={busy ? () => {} : onClose}
      title={title}
      subtitle={subtitle}
      helpId="people-timeline"
      footer={
        <>
          <Button variant="tertiary" disabled={busy} onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={busy} onClick={() => void submit()}>
            {initial ? "Save" : "Log it"}
          </Button>
        </>
      }
    >
      <div className="mt-4 flex flex-col gap-3">
        <div className="flex flex-col gap-1">
          <label htmlFor={`${base}-kind`} className="text-xs text-ink3">
            What happened
          </label>
          <Select
            id={`${base}-kind`}
            value={kind}
            disabled={busy}
            onChange={(e) => setKind(e.target.value as InteractionKind)}
          >
            {KINDS.map((k) => (
              <option key={k} value={k}>
                {KIND_WORDS[k]}
              </option>
            ))}
          </Select>
        </div>
        <div className="flex flex-col gap-1">
          <span aria-hidden className="text-xs text-ink3">
            Date
          </span>
          <PartialDateFields
            name="Date"
            fields={fields}
            onChange={setFields}
            error={dateError ?? undefined}
          />
        </div>
        <div className="flex flex-col gap-1">
          <label htmlFor={`${base}-note`} className="text-xs text-ink3">
            Note (optional)
          </label>
          <Textarea
            id={`${base}-note`}
            rows={3}
            value={note}
            disabled={busy}
            onChange={(e) => setNote(e.target.value)}
          />
        </div>
        {problem && (
          <Callout tone="danger" size="sm">
            {problem}
          </Callout>
        )}
      </div>
    </Dialog>
  );
}
