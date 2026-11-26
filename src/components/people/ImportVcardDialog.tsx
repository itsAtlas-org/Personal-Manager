// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Import…" (spec 5.2): pick a vCard file, see what is in it, then import it. Three steps in one
// dialog, each a state of the same component so a close at any point drops everything:
//   picking   the backend opens the file picker (the path never passes through the webview); a
//             cancelled picker closes the dialog without a word, a refusal (a file over 128 MB, a
//             file that can't be opened) is said and the file can be picked again;
//   preview   what the file holds, counted by the backend, and the one option (a label for
//             everyone imported); nothing is written until Import;
//   done      what the import did: who came back from Trash, where PM kept its own details, and
//             "Review possible duplicates".
// The parse is held in the backend for ten minutes. If it has gone stale when Import is pressed the
// backend refuses in plain words, writes nothing, and the dialog offers to pick the file again.
//
// The file's name and the people's names come from outside: they are rendered as text only.

import { useEffect, useId, useRef, useState } from "react";
import { commitVcardImport, pickVcardAndPreview } from "../../lib/ipc";
import { errorWords, foldName } from "../../lib/people";
import {
  KEPT_SHOWN,
  importedLine,
  isStalePreview,
  keptLine,
  previewHeading,
  previewLines,
  restoredLine,
} from "../../lib/peopleTransfer";
import type { PeopleImportPreview, PeopleImportResult, PersonLabel } from "../../lib/types";
import { Button, Callout, Dialog, Input, Select } from "../ui";

/** The "A new label…" choice in the label picker. */
const NEW_LABEL = "new";

type Step =
  | { kind: "picking" }
  | { kind: "failed"; words: string }
  | { kind: "preview"; preview: PeopleImportPreview }
  | { kind: "done"; result: PeopleImportResult };

interface Props {
  /** Every label there is, for the "Label everyone imported" choice. */
  labels: readonly PersonLabel[];
  /** Close the dialog (after a cancelled picker, Cancel or Close). */
  onClose: () => void;
  /** "Review possible duplicates" was pressed after an import: the tab opens its panel. */
  onReview: () => void;
}

export function ImportVcardDialog({ labels, onClose, onReview }: Props) {
  const [step, setStep] = useState<Step>({ kind: "picking" });
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [labelling, setLabelling] = useState(false);
  // An existing label's id as text (the first, when there is one), or NEW_LABEL with a typed name.
  const [choice, setChoice] = useState<string>(labels[0] ? String(labels[0].id) : NEW_LABEL);
  const [typed, setTyped] = useState("");
  const base = useId();
  const picked = useRef(false);
  // True once the dialog has gone: a picker that answers late (or never) must not touch it. The
  // person can close the dialog while the picker is out, so a hidden or stuck picker never traps them.
  const gone = useRef(false);

  /** Open the picker, then read the file. Nothing else is on screen while it is out. */
  async function pick() {
    setStep({ kind: "picking" });
    setProblem(null);
    try {
      const preview = await pickVcardAndPreview();
      if (gone.current) return;
      if (preview === null) {
        onClose();
        return;
      }
      setStep({ kind: "preview", preview });
    } catch (err) {
      if (gone.current) return;
      setStep({ kind: "failed", words: errorWords(err, "PM couldn't read that file.") });
    }
  }

  // The picker opens once, when the dialog does (a development double-mount mustn't open two).
  useEffect(() => {
    gone.current = false;
    if (!picked.current) {
      picked.current = true;
      void pick();
    }
    return () => {
      gone.current = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** The label's name as the backend takes it, or null when the option is off. */
  function labelName(): string | null {
    if (!labelling) return null;
    if (choice === NEW_LABEL) return typed.trim() || null;
    return labels.find((l) => String(l.id) === choice)?.name ?? null;
  }
  const labelMissing = labelling && labelName() === null;

  async function commit(preview: PeopleImportPreview) {
    if (busy || labelMissing) return;
    setBusy(true);
    setProblem(null);
    try {
      const result = await commitVcardImport(preview.preview_id, { label: labelName() });
      setStep({ kind: "done", result });
    } catch (err) {
      if (isStalePreview(err)) {
        // Nothing was written: the file has to be read again, which is a new picker.
        setStep({ kind: "failed", words: errorWords(err) });
      } else {
        setProblem(errorWords(err, "PM couldn't import that file. Nothing was changed."));
      }
    } finally {
      setBusy(false);
    }
  }

  const locked = busy;

  let title = "Import people";
  if (step.kind === "preview") title = previewHeading(step.preview);
  if (step.kind === "done") title = "Import finished";

  let footer = (
    <Button variant="tertiary" onClick={onClose} disabled={locked}>
      Cancel
    </Button>
  );
  if (step.kind === "preview") {
    footer = (
      <>
        <Button variant="tertiary" onClick={onClose} disabled={busy}>
          Cancel
        </Button>
        <Button
          variant="primary"
          onClick={() => void commit(step.preview)}
          disabled={busy || step.preview.people === 0 || labelMissing}
        >
          {busy ? "Importing…" : "Import"}
        </Button>
      </>
    );
  } else if (step.kind === "failed") {
    footer = (
      <>
        <Button variant="tertiary" onClick={onClose}>
          Cancel
        </Button>
        <Button variant="primary" onClick={() => void pick()}>
          Pick the file again
        </Button>
      </>
    );
  } else if (step.kind === "done") {
    footer = (
      <>
        {step.result.review > 0 && (
          <Button
            variant="secondary"
            onClick={() => {
              onReview();
              onClose();
            }}
          >
            Review possible duplicates
          </Button>
        )}
        <Button variant="primary" onClick={onClose}>
          Done
        </Button>
      </>
    );
  }

  return (
    <Dialog
      open
      onClose={() => {
        if (!locked) onClose();
      }}
      title={title}
      helpId="people-import"
      widthClassName="max-w-lg"
      footer={footer}
    >
      {step.kind === "picking" && (
        <p role="status" className="mt-2 text-sm text-ink3">
          Choose a contacts file in the window that opened…
        </p>
      )}

      {step.kind === "failed" && (
        <Callout tone="danger" size="sm" className="mt-3">
          {step.words}
        </Callout>
      )}

      {step.kind === "preview" && (
        <>
          <ul className="mt-2 flex flex-col gap-1.5">
            {previewLines(step.preview).map((l) => (
              <li
                key={l.key}
                className={l.tone === "warn" ? "text-sm text-ink" : "text-sm text-ink2"}
              >
                {l.text}
              </li>
            ))}
          </ul>

          <div className="mt-4 flex flex-wrap items-center gap-2">
            <label className="flex items-center gap-2 text-sm text-ink2" htmlFor={`${base}-label`}>
              <input
                id={`${base}-label`}
                type="checkbox"
                className="accent-[var(--accent)]"
                checked={labelling}
                disabled={busy}
                onChange={(e) => setLabelling(e.target.checked)}
              />
              Label everyone imported:
            </label>
            {labelling && (
              <>
                {labels.length > 0 && (
                  <Select
                    compact
                    aria-label="Label for everyone imported"
                    value={choice}
                    disabled={busy}
                    onChange={(e) => setChoice(e.target.value)}
                  >
                    {labels.map((l) => (
                      <option key={l.id} value={String(l.id)}>
                        {l.name}
                      </option>
                    ))}
                    <option value={NEW_LABEL}>A new label…</option>
                  </Select>
                )}
                {(labels.length === 0 || choice === NEW_LABEL) && (
                  <Input
                    aria-label="Name of the new label"
                    placeholder="Label name"
                    value={typed}
                    maxLength={80}
                    disabled={busy}
                    onChange={(e) => setTyped(e.target.value)}
                  />
                )}
              </>
            )}
          </div>
          {labelling && choice === NEW_LABEL && typed.trim() !== "" && (
            <p className="mt-1 text-xs text-ink4">
              {labels.some((l) => foldName(l.name) === foldName(typed))
                ? "You already have a label with that name, so PM uses it."
                : "PM makes this label."}
            </p>
          )}
        </>
      )}

      {step.kind === "done" && <ImportResultView result={step.result} />}

      {problem && (
        <Callout tone="danger" size="sm" className="mt-3">
          {problem}
        </Callout>
      )}
    </Dialog>
  );
}

function ImportResultView({ result }: { result: PeopleImportResult }) {
  const back = restoredLine(result.restored);
  const kept = result.kept;
  return (
    <div className="mt-2 flex flex-col gap-2 text-sm text-ink2">
      <p role="status" className="text-ink">
        {importedLine(result)}
      </p>
      {back && <p>{back}</p>}
      {kept.length > 0 && (
        <div>
          <p>Kept PM's details where they differed:</p>
          <ul className="mt-1 list-disc pl-5">
            {kept.slice(0, KEPT_SHOWN).map((k, i) => (
              <li key={`${i}-${k.name}`}>{keptLine(k)}</li>
            ))}
          </ul>
          {kept.length > KEPT_SHOWN && (
            <p className="mt-1 text-xs text-ink4">and {kept.length - KEPT_SHOWN} more.</p>
          )}
        </div>
      )}
      {result.review > 0 && (
        <p className="text-xs text-ink3">
          {result.review === 1
            ? "1 of the people you imported may be someone you already have."
            : `${result.review} of the people you imported may be people you already have.`}
        </p>
      )}
    </div>
  );
}
