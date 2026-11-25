// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Keeping the Pinboard's notes before it is uninstalled (Mods spec D9; PR C, K5b). The order is the
// whole point and it lives here, in one place: the files first (a quick step, and the folder is asked
// for while the person is still looking at the dialog), then the notes as documents, ONE note per
// call, then, and only when every step before answered, the caller's uninstall. Any failure, and a
// Cancel between notes, stops with nothing deleted. A retry plans afresh, so a note already saved is
// no longer on the list: nothing is saved twice.
//
// Every sentence that reaches the person is the backend's fixed one. A note's title is the person's
// own text and travels only as data for the dialog to draw as React text.

import { uninstallExportFiles, uninstallNotesPlan, uninstallSaveNote } from "../../lib/ipc";
import { failureText, type NoteRef, type NotesPlan } from "../../lib/modsTypes";
import { modText } from "./modText";

/** How many unsaved notes before the dialog says saving them can take minutes. */
export const MANY_NOTES = 50;

/** What the person asked to keep. */
export interface KeepChoice {
  /** `documents`: each note as a document (needs the engine). `files`: one `.md` file per note in a
   *  folder (needs nothing). `none`: leave the notes out. */
  notes: "documents" | "files" | "none";
  /** A copy of the whole board as one file, in the same folder. */
  board: boolean;
}

export interface KeepHooks {
  /** Saving note `done + 1` of `total` is about to start (and `done === total` once all are in). */
  onProgress: (done: number, total: number) => void;
  /** Asked before every note and once more after the last step: true stops the run there, so a Cancel
   *  pressed at any moment is heard before the uninstall. */
  cancelled: () => boolean;
}

/** Why a run did not reach the uninstall. */
export type StopReason = "cancelled" | "failed" | "no-folder";

export type KeepResult =
  | {
      kind: "ready";
      /** Notes saved as documents in this run. */
      saved: NoteRef[];
      /** Notes that were documents already (not embedded again). */
      alreadySaved: number;
      files: { folder: string; notes: number; boardFile: string | null } | null;
    }
  | {
      kind: "stopped";
      reason: StopReason;
      /** Saved as documents in this run, before it stopped. They stay. */
      saved: NoteRef[];
      /** How many notes the run meant to save, for "12 of 540". */
      total: number;
      /** What went wrong, in the backend's own sentence. */
      problem: string | null;
      /** The note that could not be saved, when one could not. */
      failedNote: NoteRef | null;
      /** Files written before it stopped. */
      filesWritten: number;
    };

/**
 * Keep what `choice` names. Never uninstalls anything: the caller does that on `ready` and never
 * otherwise.
 */
export async function keepNotes(
  modId: string,
  choice: KeepChoice,
  hooks: KeepHooks,
): Promise<KeepResult> {
  let plan: NotesPlan | null;
  try {
    plan = await uninstallNotesPlan(modId);
  } catch (e) {
    return stopped("failed", [], 0, failureText(e, "PM couldn't read your notes."), null, 0);
  }
  if (!plan) return { kind: "ready", saved: [], alreadySaved: 0, files: null };
  const toSave = choice.notes === "documents" ? plan.toSave : [];

  let files: { folder: string; notes: number; boardFile: string | null } | null = null;
  const wantFiles = choice.notes === "files" || choice.board;
  if (wantFiles) {
    let out;
    try {
      out = await uninstallExportFiles(modId, choice.notes === "files", choice.board);
    } catch (e) {
      return stopped(
        "failed",
        [],
        toSave.length,
        failureText(e, "PM couldn't write the files."),
        null,
        0,
      );
    }
    // The person closed the folder dialog: they changed their mind, so nothing goes further.
    if (!out) return stopped("no-folder", [], toSave.length, null, null, 0);
    if (out.failed) {
      return stopped("failed", [], toSave.length, out.failed.message, null, out.notesWritten);
    }
    files = { folder: out.folder, notes: out.notesWritten, boardFile: out.boardFile };
  }

  const saved: NoteRef[] = [];
  hooks.onProgress(0, toSave.length);
  for (const note of toSave) {
    if (hooks.cancelled()) {
      return stopped("cancelled", saved, toSave.length, null, null, files?.notes ?? 0);
    }
    try {
      await uninstallSaveNote(modId, note.key);
    } catch (e) {
      return stopped(
        "failed",
        saved,
        toSave.length,
        failureText(e, "PM couldn't save this note."),
        note,
        files?.notes ?? 0,
      );
    }
    saved.push(note);
    hooks.onProgress(saved.length, toSave.length);
  }
  // A Cancel pressed while the LAST note was being saved, or while the files were (a files-only run
  // has no note to look between): whatever was kept stays, and nothing is uninstalled. The dialog
  // told the person it was stopping.
  if (hooks.cancelled()) {
    return stopped("cancelled", saved, toSave.length, null, null, files?.notes ?? 0);
  }
  return {
    kind: "ready",
    saved,
    alreadySaved: choice.notes === "documents" ? plan.alreadySaved : 0,
    files,
  };
}

function stopped(
  reason: StopReason,
  saved: NoteRef[],
  total: number,
  problem: string | null,
  failedNote: NoteRef | null,
  filesWritten: number,
): KeepResult {
  return { kind: "stopped", reason, saved, total, problem, failedNote, filesWritten };
}

/** What the dialog's two boxes come to, for this plan. Documents need the engine; with it off the
 *  notes go as `.md` files instead; with nothing left to save, no notes step at all. */
export function keepChoiceOf(
  plan: NotesPlan | null,
  wantNotes: boolean,
  wantBoard: boolean,
): KeepChoice {
  if (!plan) return { notes: "none", board: false };
  let notes: KeepChoice["notes"] = "none";
  if (wantNotes && plan.total > 0 && plan.toSave.length > 0) {
    notes = plan.engineReady ? "documents" : "files";
  }
  return { notes, board: wantBoard && plan.boardBytes > 0 };
}

const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** What was kept, for the line the Mods tab says once the mod is gone. Null when nothing was. */
export function keptSentence(done: Extract<KeepResult, { kind: "ready" }>): string | null {
  const parts: string[] = [];
  if (done.saved.length > 0) {
    parts.push(`${count(done.saved.length, "note", "notes")} saved as documents`);
  }
  if (done.files && done.files.notes > 0) {
    parts.push(`${count(done.files.notes, "note", "notes")} written as .md files`);
  }
  if (done.files?.boardFile) parts.push("a copy of the whole board saved as a file");
  return parts.length > 0 ? `Before it went, PM kept ${parts.join(", ")}.` : null;
}

/** The sentence about PM's own copy of the board, said whenever the board file is on offer. */
export const OLD_COPY_MOVED =
  "PM keeps the board as it was when the Pinboard moved, until a later update clears it.";
export const OLD_COPY_NOT_MOVED = "PM also still keeps your board in its own storage.";
export const BOARD_FILE_NOTE = "A plain-text safety copy. PM can't open it yet.";

const TITLE_CLAMP = 60;

/** A note's title as one clamped line, or a word for a note with none. */
export function shownTitle(note: NoteRef): string {
  const t = modText(note.title);
  if (!t) return "a note with no title";
  return t.length > TITLE_CLAMP ? `${t.slice(0, TITLE_CLAMP)}…` : t;
}
