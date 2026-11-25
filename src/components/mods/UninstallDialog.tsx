// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Uninstalling a mod (Mods spec §1.3 "Uninstall dialog"). The preview comes first and says, in the
// backend's counts, what leaves (the mod's own saved data, with its size, and its tab) and what
// stays (the tasks, milestones and notes it made). "Also delete what it made" is a choice that starts
// OFF: the mod's private storage goes with the mod either way, but what it made in the person's own
// lists is theirs until they say otherwise.
//
// The uninstall itself is a Mods action (lib/modActions): it runs to its end whether or not this
// dialog, or the Mods tab, is still on screen, and its follow-up (forgetting the mod's tab placement
// and folds on this device, saying what was removed) belongs to the action, not to this component.

import { useEffect, useRef, useState } from "react";
import { uninstallPreview } from "../../lib/ipc";
import { formatBytes } from "../../lib/format";
import { clearModError, useModActions } from "../../lib/modActions";
import { useModRows } from "../../lib/modRows";
import { failureText, type UninstallPreview } from "../../lib/modsTypes";
import { Button, Callout, Dialog } from "../ui";
import { ModName } from "./ModName";
import { modText } from "./modText";
import { uninstallAction } from "./uninstallActions";
import { keepChoiceOf, keepNotes, keptSentence, type KeepResult } from "./uninstallKeep";
import { plural, uninstallNote } from "./uninstallNote";
import { NotesChoices, NotesStopped } from "./UninstallNotes";

export function UninstallDialog({
  modId,
  onClose,
  refresh,
  onDone,
}: {
  modId: string;
  onClose: () => void;
  /** Read the rows again (the Mods tab's shared read). */
  refresh: () => Promise<void>;
  /** Called with what happened once the uninstall has worked, for a caller that has a place to say
   *  it (Settings → Storage, where the Mods tab's notes aren't on screen). */
  onDone?: (note: { warning: boolean; text: string }) => void;
}) {
  const [preview, setPreview] = useState<UninstallPreview | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [alsoDelete, setAlsoDelete] = useState(false);
  // The Pinboard's notes (D9): both start on, and only the Pinboard has them to offer.
  const [saveNotes, setSaveNotes] = useState(true);
  const [saveBoard, setSaveBoard] = useState(true);
  const [phase, setPhase] = useState<KeepPhase>({ kind: "idle" });
  const stopRef = useRef(false);
  const { busy, errors } = useModActions();

  useEffect(() => {
    let cancelled = false;
    // A refusal from an earlier try belongs to that try; this dialog starts clean.
    clearModError(modId);
    uninstallPreview(modId)
      .then((p) => {
        if (!cancelled) setPreview(p);
      })
      .catch((e: unknown) => {
        if (!cancelled)
          setLoadError(failureText(e, "PM couldn't work out what uninstalling would do."));
      });
    return () => {
      cancelled = true;
    };
  }, [modId]);

  const keeping = phase.kind === "keeping";
  const working = busy.has(modId) || keeping;
  const error = errors.get(modId) ?? null;
  // The row's sentence says why the safety list removed it; the preview carries no reason.
  const { rows } = useModRows();
  const row = rows.find((r) => r.id === modId);
  const reason = row?.state.kind === "removed-by-safety-list" ? modText(row.state.text) : "";

  if (!preview) {
    return (
      <Dialog
        open
        title="Uninstall this mod?"
        helpId="mods-uninstall"
        onClose={onClose}
        widthClassName="max-w-md"
        footer={
          <Button variant="tertiary" onClick={onClose}>
            Cancel
          </Button>
        }
      >
        <div className="mt-2 text-sm text-ink3">
          {loadError ? <Callout size="md">{loadError}</Callout> : "Looking at what it has…"}
        </div>
      </Dialog>
    );
  }

  const { leaves, stays } = preview;
  const made = stays.tasks + stays.milestones + stays.documents;
  const goes: string[] = [];
  if (leaves.contentBytes > 0) goes.push(`What it saved (${formatBytes(leaves.contentBytes)})`);
  if (leaves.documentDataBytes > 0) {
    goes.push(`What it kept about your documents (${formatBytes(leaves.documentDataBytes)})`);
  }
  if (leaves.cacheBytes > 0) goes.push(`Its temporary data (${formatBytes(leaves.cacheBytes)})`);
  if (leaves.filesBytes > 0) goes.push(`Its files (${formatBytes(leaves.filesBytes)})`);
  if (leaves.navTab) goes.push(`Its tab in the sidebar, “${modText(leaves.navTab)}”`);
  if (leaves.calendarOverlay) goes.push("Its calendar overlay");

  function uninstall(name: string, kept: string | null) {
    void uninstallAction(modId, name, made > 0 && alsoDelete, false, kept).then((r) => {
      void refresh();
      if (r.ok) {
        onDone?.(uninstallNote(name, r.value, kept));
        onClose();
      }
    });
  }

  // Keep what the person asked to keep, and only then uninstall. The keeping never uninstalls: a
  // failure, a Cancel between notes or a closed folder dialog ends here with nothing deleted.
  async function confirm() {
    if (!preview || working) return;
    const name = modText(preview.name);
    const choice = keepChoiceOf(preview.notes, saveNotes, saveBoard);
    if (choice.notes === "none" && !choice.board) return uninstall(name, null);
    stopRef.current = false;
    setPhase({ kind: "keeping", done: 0, total: null });
    const result = await keepNotes(modId, choice, {
      onProgress: (done, total) => setPhase({ kind: "keeping", done, total }),
      cancelled: () => stopRef.current,
    });
    if (result.kind === "stopped") {
      setPhase({ kind: "stopped", result });
      return;
    }
    setPhase({ kind: "idle" });
    uninstall(name, keptSentence(result));
  }

  return (
    <Dialog
      open
      tone="danger"
      title={
        <>
          Uninstall <ModName name={preview.name} />?
        </>
      }
      helpId="mods-uninstall"
      onClose={working ? () => {} : onClose}
      widthClassName="max-w-md"
      footer={
        <>
          {keeping ? (
            // Stops between notes: the note being saved finishes, nothing is uninstalled.
            <Button
              variant="tertiary"
              onClick={() => {
                stopRef.current = true;
                setPhase((p) => (p.kind === "keeping" ? { ...p, stopping: true } : p));
              }}
              disabled={phase.stopping === true}
              data-autofocus
            >
              {phase.stopping ? "Stopping…" : "Cancel"}
            </Button>
          ) : (
            <Button variant="tertiary" onClick={onClose} disabled={working} data-autofocus>
              Cancel
            </Button>
          )}
          <Button variant="danger" onClick={() => void confirm()} disabled={working}>
            {keeping ? "Saving your notes…" : working ? "Uninstalling…" : "Uninstall"}
          </Button>
        </>
      }
    >
      <div className="mt-2 space-y-3 text-sm leading-relaxed text-ink2">
        {preview.removedBySafetyList && (
          <div className="space-y-1">
            <p>
              PM&apos;s safety list switched this mod off. What it saved and what it made are still
              here until you remove them.
            </p>
            {/* Why, in the backend's own sentence from the row (this dialog is also opened from
                Settings, where no strip or row says it). */}
            {reason && <p className="break-words text-ink3">{reason}</p>}
          </div>
        )}
        <div>
          <p className="font-medium text-ink">What goes</p>
          {goes.length === 0 ? (
            <p className="text-ink3">It hasn&apos;t saved anything.</p>
          ) : (
            <ul className="list-disc pl-5">
              {goes.map((g) => (
                <li key={g} className="break-words">
                  {g}
                </li>
              ))}
            </ul>
          )}
          <p className="pt-1 text-ink3">What it saved can&apos;t be brought back.</p>
        </div>
        <div>
          <p className="font-medium text-ink">What stays</p>
          {made === 0 ? (
            <p className="text-ink3">It hasn&apos;t made any tasks, milestones or notes.</p>
          ) : (
            <>
              <p>
                {[
                  stays.tasks > 0 ? plural(stays.tasks, "task", "tasks") : null,
                  stays.milestones > 0 ? plural(stays.milestones, "milestone", "milestones") : null,
                  stays.documents > 0 ? plural(stays.documents, "note", "notes") : null,
                ]
                  .filter((x) => x !== null)
                  .join(", ")}{" "}
                it made stay in PM, marked as made by it.
              </p>
              <label className="mt-2 flex items-start gap-2 text-ink">
                <input
                  type="checkbox"
                  className="mt-1 accent-[var(--accent)]"
                  checked={alsoDelete}
                  disabled={working}
                  onChange={(e) => setAlsoDelete(e.target.checked)}
                />
                <span>
                  Also delete what it made ({plural(made, "thing", "things")}). This can&apos;t be
                  undone.
                </span>
              </label>
            </>
          )}
        </div>
        {stays.keptDocuments > 0 && (
          // Notes from before the mod existed: the person's own, kept whatever is chosen above.
          <p className="text-ink3">
            {plural(stays.keptDocuments, "note", "notes")} you saved before it was a mod stay in PM,
            and uninstalling never deletes them.
          </p>
        )}
        {preview.notes && (
          <NotesChoices
            plan={preview.notes}
            saveNotes={saveNotes}
            onSaveNotes={setSaveNotes}
            saveBoard={saveBoard}
            onSaveBoard={setSaveBoard}
            disabled={working}
          />
        )}
        {phase.kind === "keeping" && (
          <p className="text-ink" role="status">
            {phase.total === null
              ? "Getting your notes ready…"
              : phase.total === 0
                ? "Writing your files…"
                : `Saving ${Math.min(phase.done + 1, phase.total)} of ${phase.total}`}
          </p>
        )}
        {phase.kind === "stopped" && <NotesStopped result={phase.result} />}
        {error && <Callout size="md">{error}</Callout>}
      </div>
    </Dialog>
  );
}

/** Where keeping the Pinboard's notes stands. `total` is null until the plan is read. */
type KeepPhase =
  | { kind: "idle" }
  | { kind: "keeping"; done: number; total: number | null; stopping?: boolean }
  | { kind: "stopped"; result: Extract<KeepResult, { kind: "stopped" }> };
