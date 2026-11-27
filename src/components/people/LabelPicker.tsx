// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Label control (the selection bar's, and the open record's) and the dialog that names a label.
// The picker is a small popover of checkboxes, one per label: ticked when everyone it acts on
// carries it, mixed when only some do, and a tick or untick puts the label on all of them or takes
// it off all of them. "New label…" opens the same name dialog the right-click menu uses, which makes
// the label and puts it on the same people in one go. The same dialog also names a label with
// nobody on it yet (the filter list's "Add a label") and renames one.

import { useId, useState, type FormEvent } from "react";
import { errorWords, labelCoverage } from "../../lib/people";
import type { PersonLabel, PersonLabelRef } from "../../lib/types";
import { Button, Callout, Dialog, Input, Popover } from "../ui";

interface PickerProps {
  labels: readonly PersonLabel[];
  /** The people the control acts on: all of the selection, or the one record. */
  targets: readonly { labels: readonly PersonLabelRef[] }[];
  onToggle: (labelId: number, on: boolean) => void;
  onNew: () => void;
  /** The trigger's words; "Label" in the selection bar, "Add label" on a record. */
  buttonLabel?: string;
}

export function LabelPicker({
  labels,
  targets,
  onToggle,
  onNew,
  buttonLabel = "Label",
}: PickerProps) {
  const { all, some } = labelCoverage(targets);
  return (
    <Popover
      ariaLabel="Labels"
      trigger={({ open, toggle }) => (
        <Button
          variant="secondary"
          size="xs"
          aria-haspopup="true"
          aria-expanded={open}
          onClick={toggle}
        >
          {buttonLabel}
        </Button>
      )}
      panelClassName="min-w-[14rem] p-2"
    >
      {({ close }) => (
        <div className="flex flex-col gap-1">
          {labels.length === 0 && (
            <p className="px-2 py-1 text-xs text-ink3">No labels yet. Make the first one below.</p>
          )}
          {labels.map((l) => (
            <label
              key={l.id}
              className="flex cursor-pointer items-center gap-2 rounded-[var(--radius-sm)] px-2 py-1 text-sm text-ink2 hover:bg-surface"
            >
              <input
                type="checkbox"
                className="accent-[var(--accent)]"
                checked={all.has(l.id)}
                ref={(el) => {
                  if (el) el.indeterminate = some.has(l.id);
                }}
                onChange={() => onToggle(l.id, !all.has(l.id))}
              />
              <span className="min-w-0 flex-1 truncate">{l.name}</span>
            </label>
          ))}
          <Button
            variant="tertiary"
            size="xs"
            className="mt-1 self-start"
            onClick={() => {
              close();
              onNew();
            }}
          >
            New label…
          </Button>
        </div>
      )}
    </Popover>
  );
}

interface DialogProps {
  /** How many people the new label goes on; 0 for a label that starts with nobody. */
  count: number;
  /** A label's current name: the dialog renames it instead of making one. */
  renaming?: string;
  /** Make the label (or find the one that already has the name) and put it on them, or rename it.
   *  Rejects with words for the person when it can't. */
  onSubmit: (name: string) => Promise<void>;
  onClose: () => void;
}

export function LabelNameDialog({ count, renaming, onSubmit, onClose }: DialogProps) {
  const [name, setName] = useState(renaming ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const formId = useId();
  const inputId = useId();

  async function submit(e: FormEvent) {
    e.preventDefault();
    const clean = name.trim();
    if (!clean || busy) return;
    setBusy(true);
    setError(null);
    try {
      await onSubmit(clean);
      onClose();
    } catch (err) {
      setError(
        errorWords(
          err,
          renaming !== undefined
            ? "PM couldn't rename that label. Try again."
            : "PM couldn't make that label. Try again.",
        ),
      );
      setBusy(false);
    }
  }

  return (
    <Dialog
      open
      onClose={() => {
        if (!busy) onClose();
      }}
      title={renaming !== undefined ? "Rename label" : "New label"}
      helpId="people-labels"
      subtitle={
        renaming !== undefined
          ? "Everyone who has it keeps it under the new name."
          : count === 0
            ? "It starts with nobody on it."
            : count > 1
              ? `It goes on the ${count} people selected.`
              : "It goes on this person."
      }
      footer={
        <>
          <Button variant="tertiary" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button type="submit" form={formId} disabled={busy || name.trim() === ""}>
            {renaming !== undefined ? "Rename" : "Add label"}
          </Button>
        </>
      }
    >
      <form id={formId} onSubmit={(e) => void submit(e)} className="mt-4 flex flex-col gap-2">
        <label htmlFor={inputId} className="text-xs text-ink3">
          Label name
        </label>
        <Input
          id={inputId}
          value={name}
          maxLength={80}
          autoFocus
          onChange={(e) => setName(e.target.value)}
        />
        {error && (
          <Callout tone="danger" size="sm">
            {error}
          </Callout>
        )}
      </form>
    </Dialog>
  );
}
