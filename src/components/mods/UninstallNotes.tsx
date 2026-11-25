// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Pinboard's part of the uninstall dialog (Mods spec D9; PR C, K5b): what PM offers to keep first,
// and what it says when keeping stopped. A note's title is the person's own text: it is drawn as React
// text, cleaned of invisible characters and clamped.

import type { NotesPlan } from "../../lib/modsTypes";
import { Callout } from "../ui";
import { modText } from "./modText";
import {
  BOARD_FILE_NOTE,
  MANY_NOTES,
  OLD_COPY_MOVED,
  OLD_COPY_NOT_MOVED,
  shownTitle,
  type KeepResult,
} from "./uninstallKeep";
import { plural } from "./uninstallNote";

export function NotesChoices({
  plan,
  saveNotes,
  onSaveNotes,
  saveBoard,
  onSaveBoard,
  disabled,
}: {
  plan: NotesPlan;
  saveNotes: boolean;
  onSaveNotes: (on: boolean) => void;
  saveBoard: boolean;
  onSaveBoard: (on: boolean) => void;
  disabled: boolean;
}) {
  if (plan.total === 0 && plan.boardBytes === 0) return null;
  const needsSaving = plan.toSave.length > 0;
  const asFiles = needsSaving && !plan.engineReady;
  return (
    <div className="space-y-2">
      <p className="font-medium text-ink">Your notes</p>
      {plan.total > 0 && !needsSaving && (
        <p>
          {plan.total === 1 ? "Your note is" : `All ${plan.total} of your notes are`} already saved
          as documents, so they stay in Documents.
        </p>
      )}
      {needsSaving && (
        <label className="flex items-start gap-2 text-ink">
          <input
            type="checkbox"
            className="mt-1 accent-[var(--accent)]"
            checked={saveNotes}
            disabled={disabled}
            onChange={(e) => onSaveNotes(e.target.checked)}
          />
          <span>
            {asFiles
              ? `Save my ${plural(plan.total, "note", "notes")} as .md files in a folder`
              : `Save my ${plural(plan.toSave.length, "note", "notes")} as documents first`}
            <span className="block text-ink3">
              {asFiles
                ? "PM's document engine isn't set up, so notes can't become documents yet. You choose the folder next."
                : "They go into Documents, already marked as reviewed. Notes already saved there are left alone."}
            </span>
          </span>
        </label>
      )}
      {needsSaving && saveNotes && !asFiles && plan.toSave.length > MANY_NOTES && (
        <p className="text-ink3" role="note">
          That is a lot of notes. Saving them can take a few minutes, and you can stop between
          notes.
        </p>
      )}
      {plan.unsaveable > 0 && (
        <p className="text-ink3">
          {plural(plan.unsaveable, "note", "notes")} can&apos;t be saved as{" "}
          {plan.unsaveable === 1 ? "a document" : "documents"} (an unusual id). A copy of the whole
          board keeps {plan.unsaveable === 1 ? "it" : "them"}.
        </p>
      )}
      {plan.boardBytes > 0 && (
        <label className="flex items-start gap-2 text-ink">
          <input
            type="checkbox"
            className="mt-1 accent-[var(--accent)]"
            checked={saveBoard}
            disabled={disabled}
            onChange={(e) => onSaveBoard(e.target.checked)}
          />
          <span>
            Save a copy of my whole board as a file
            <span className="block text-ink3">
              {BOARD_FILE_NOTE} {plan.source === "mod" ? OLD_COPY_MOVED : OLD_COPY_NOT_MOVED}
            </span>
          </span>
        </label>
      )}
    </div>
  );
}

const SHOWN_SAVED = 8;

/** Why keeping stopped before the uninstall, and what was kept. Nothing was uninstalled. */
export function NotesStopped({ result }: { result: Extract<KeepResult, { kind: "stopped" }> }) {
  const { reason, saved, total, problem, failedNote, filesWritten } = result;
  let head: string;
  if (reason === "cancelled") head = "You stopped it.";
  else if (reason === "no-folder") head = "No folder was chosen.";
  else if (failedNote) head = `PM couldn't save “${shownTitle(failedNote)}”.`;
  else head = "PM couldn't keep your notes.";
  return (
    <Callout tone="warning" size="md" live>
      <p className="font-medium">{head} Nothing was uninstalled.</p>
      {problem && <p className="break-words">{modText(problem)}</p>}
      {filesWritten > 0 && <p>{plural(filesWritten, "file was", "files were")} written first.</p>}
      {total > 0 && (
        <p>
          {saved.length} of {plural(total, "note", "notes")} {saved.length === 1 ? "was" : "were"}{" "}
          saved as documents first, and they stay. Press Uninstall to carry on from there: PM
          won&apos;t save those again.
        </p>
      )}
      {saved.length > 0 && (
        <ul className="list-disc pl-5">
          {saved.slice(0, SHOWN_SAVED).map((n) => (
            <li key={n.key} className="break-words">
              {shownTitle(n)}
            </li>
          ))}
          {saved.length > SHOWN_SAVED && <li>and {saved.length - SHOWN_SAVED} more</li>}
        </ul>
      )}
    </Callout>
  );
}
