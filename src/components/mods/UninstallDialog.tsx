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

import { useEffect, useState } from "react";
import { uninstallMod, uninstallPreview } from "../../lib/ipc";
import { formatBytes } from "../../lib/format";
import { addModNote, clearModError, runModAction, useModActions } from "../../lib/modActions";
import { pruneAfterUninstall } from "../../lib/modFolds";
import { failureText, type UninstallPreview } from "../../lib/modsTypes";
import { Button, Callout, Dialog } from "../ui";
import { modText } from "./modText";
import { plural, uninstallNote } from "./uninstallNote";

export function UninstallDialog({
  modId,
  onClose,
  refresh,
}: {
  modId: string;
  onClose: () => void;
  /** Read the rows again (the Mods tab's shared read). */
  refresh: () => Promise<void>;
}) {
  const [preview, setPreview] = useState<UninstallPreview | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [alsoDelete, setAlsoDelete] = useState(false);
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

  const working = busy.has(modId);
  const error = errors.get(modId) ?? null;

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

  function confirm() {
    if (!preview) return;
    const name = modText(preview.name);
    void runModAction(
      modId,
      "Uninstalling",
      "PM couldn't uninstall this mod.",
      () => uninstallMod(modId, made > 0 && alsoDelete),
      (done) => {
        // Before anything is drawn again, and whether or not this dialog is still on screen.
        pruneAfterUninstall(modId, done.prune);
        const note = uninstallNote(name, done);
        addModNote(note.warning ? "warning" : "info", note.text);
      },
    ).then((r) => {
      void refresh();
      if (r.ok) onClose();
    });
  }

  return (
    <Dialog
      open
      tone="danger"
      title={`Uninstall ${modText(preview.name)}?`}
      helpId="mods-uninstall"
      onClose={working ? () => {} : onClose}
      widthClassName="max-w-md"
      footer={
        <>
          <Button variant="tertiary" onClick={onClose} disabled={working} data-autofocus>
            Cancel
          </Button>
          <Button variant="danger" onClick={confirm} disabled={working}>
            {working ? "Uninstalling…" : "Uninstall"}
          </Button>
        </>
      }
    >
      <div className="mt-2 space-y-3 text-sm leading-relaxed text-ink2">
        {preview.removedBySafetyList && (
          <p>
            PM&apos;s safety list switched this mod off. What it saved and what it made are still
            here until you remove them.
          </p>
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
        {error && <Callout size="md">{error}</Callout>}
      </div>
    </Dialog>
  );
}
