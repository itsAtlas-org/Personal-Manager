// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Keeping the Pinboard's notes before it is uninstalled (D9, K5b): the ORDER is what is tested. Files,
// then one note per call in board order, and nothing else until every call answered; a failure, a
// Cancel between notes and a closed folder dialog each end it with the uninstall never asked for.

import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ExportOutcome, NotesPlan, NoteSaved } from "../../lib/modsTypes";
import { keepChoiceOf, keepNotes, keptSentence, type KeepResult } from "./uninstallKeep";

const calls: string[] = [];
const uninstallNotesPlan = vi.fn<(id: string) => Promise<NotesPlan | null>>();
const uninstallSaveNote = vi.fn<(id: string, key: string) => Promise<NoteSaved>>();
const uninstallExportFiles =
  vi.fn<(id: string, notes: boolean, board: boolean) => Promise<ExportOutcome | null>>();
vi.mock("../../lib/ipc", () => ({
  uninstallNotesPlan: (id: string) => uninstallNotesPlan(id),
  uninstallSaveNote: (id: string, key: string) => uninstallSaveNote(id, key),
  uninstallExportFiles: (id: string, n: boolean, b: boolean) => uninstallExportFiles(id, n, b),
}));

const plan = (n: number, over: Partial<NotesPlan> = {}): NotesPlan => ({
  total: n,
  toSave: Array.from({ length: n }, (_, i) => ({ key: `n${i + 1}`, title: `Note ${i + 1}` })),
  alreadySaved: 0,
  unsaveable: 0,
  engineReady: true,
  boardBytes: 500,
  source: "mod",
  ...over,
});

const written = (over: Partial<ExportOutcome> = {}): ExportOutcome => ({
  folder: "/home/x/Notes",
  notesWritten: 0,
  boardFile: null,
  failed: null,
  ...over,
});

const hooks = (cancelled = () => false) => {
  const progress: string[] = [];
  return {
    progress,
    h: { onProgress: (d: number, t: number) => progress.push(`${d}/${t}`), cancelled },
  };
};

beforeEach(() => {
  calls.length = 0;
  uninstallNotesPlan.mockReset();
  uninstallSaveNote.mockReset();
  uninstallExportFiles.mockReset();
  uninstallNotesPlan.mockImplementation(async () => {
    calls.push("plan");
    return plan(3);
  });
  uninstallSaveNote.mockImplementation(async (_id, key) => {
    calls.push(`save:${key}`);
    return { outcome: "saved", documentId: 1 };
  });
  uninstallExportFiles.mockImplementation(async (_id, notes, board) => {
    calls.push(`files:${notes}:${board}`);
    return written({
      notesWritten: notes ? 3 : 0,
      boardFile: board ? "pinboard-board-x.json" : null,
    });
  });
});

const stoppedOf = (r: KeepResult) => {
  expect(r.kind).toBe("stopped");
  return r as Extract<KeepResult, { kind: "stopped" }>;
};

describe("keeping the notes as documents", () => {
  it("plans afresh, then saves one note per call in board order, and is ready only after the last", async () => {
    const { h, progress } = hooks();
    const r = await keepNotes("atlas.pinboard", { notes: "documents", board: false }, h);
    expect(calls).toEqual(["plan", "save:n1", "save:n2", "save:n3"]);
    expect(progress).toEqual(["0/3", "1/3", "2/3", "3/3"]);
    expect(r.kind === "ready" && r.saved.map((n) => n.key)).toEqual(["n1", "n2", "n3"]);
  });

  it("asks for the files before the first note, so the folder is chosen at once", async () => {
    const { h } = hooks();
    await keepNotes("atlas.pinboard", { notes: "documents", board: true }, h);
    expect(calls).toEqual(["plan", "files:false:true", "save:n1", "save:n2", "save:n3"]);
  });

  it("stops at the note that fails: the earlier saves stand, the later notes are never tried", async () => {
    uninstallSaveNote.mockImplementation(async (_id, key) => {
      calls.push(`save:${key}`);
      if (key === "n3")
        throw { code: "internal", message: "PM couldn't save this note as a document." };
      return { outcome: "saved", documentId: 1 };
    });
    uninstallNotesPlan.mockResolvedValue(plan(5));
    const { h } = hooks();
    const r = stoppedOf(await keepNotes("atlas.pinboard", { notes: "documents", board: false }, h));
    expect(calls).toEqual(["save:n1", "save:n2", "save:n3"]);
    expect(r.reason).toBe("failed");
    expect(r.saved.map((n) => n.key)).toEqual(["n1", "n2"]);
    expect(r.failedNote).toEqual({ key: "n3", title: "Note 3" });
    expect(r.total).toBe(5);
    expect(r.problem).toBe("PM couldn't save this note as a document.");
  });

  it("Cancel between notes stops there, with what was saved kept and nothing more tried", async () => {
    // Pressed while note 2 was being saved: heard before note 3.
    const { h } = hooks(() => calls.includes("save:n2"));
    const r = stoppedOf(await keepNotes("atlas.pinboard", { notes: "documents", board: false }, h));
    expect(r.reason).toBe("cancelled");
    expect(calls).toEqual(["plan", "save:n1", "save:n2"]);
    expect(r.saved).toHaveLength(2);
  });

  it("Cancel pressed while the LAST note is being saved still stops: the uninstall is never asked for", async () => {
    uninstallNotesPlan.mockResolvedValue(plan(1));
    const { h } = hooks(() => calls.includes("save:n1"));
    const r = stoppedOf(await keepNotes("atlas.pinboard", { notes: "documents", board: false }, h));
    expect(r.reason).toBe("cancelled");
    expect(r.saved.map((n) => n.key)).toEqual(["n1"]);
    expect(r.total).toBe(1);
  });

  it("Cancel pressed while the files are being written stops a files-only run", async () => {
    const { h } = hooks(() => calls.includes("files:true:true"));
    const r = stoppedOf(await keepNotes("atlas.pinboard", { notes: "files", board: true }, h));
    expect(r.reason).toBe("cancelled");
    expect(r.filesWritten).toBe(3);
    expect(uninstallSaveNote).not.toHaveBeenCalled();
  });

  it("Cancel pressed during the board-only file stops it too", async () => {
    const { h } = hooks(() => calls.includes("files:false:true"));
    const r = stoppedOf(await keepNotes("atlas.pinboard", { notes: "none", board: true }, h));
    expect(r.reason).toBe("cancelled");
  });

  it("a 600-note board goes note by note, sequentially, with progress to 600", async () => {
    uninstallNotesPlan.mockResolvedValue(plan(600));
    let inFlight = 0;
    let most = 0;
    uninstallSaveNote.mockImplementation(async () => {
      most = Math.max(most, ++inFlight);
      await Promise.resolve();
      inFlight--;
      return { outcome: "saved", documentId: 1 };
    });
    const { h, progress } = hooks();
    const r = await keepNotes("atlas.pinboard", { notes: "documents", board: false }, h);
    expect(r.kind).toBe("ready");
    expect(uninstallSaveNote).toHaveBeenCalledTimes(600);
    expect(most).toBe(1);
    expect(progress[0]).toBe("0/600");
    expect(progress[progress.length - 1]).toBe("600/600");
    expect(progress).toContain("12/600");
  });

  it("a retry plans afresh, so notes saved before are not saved again", async () => {
    uninstallNotesPlan.mockResolvedValueOnce(plan(3));
    uninstallSaveNote.mockRejectedValueOnce({ code: "internal", message: "no" });
    const first = await keepNotes(
      "atlas.pinboard",
      { notes: "documents", board: false },
      hooks().h,
    );
    expect(first.kind).toBe("stopped");
    // The backend now lists only what is not a document yet.
    uninstallNotesPlan.mockResolvedValueOnce(
      plan(3, { toSave: [{ key: "n1", title: "Note 1" }], alreadySaved: 2 }),
    );
    uninstallSaveNote.mockClear();
    const second = await keepNotes(
      "atlas.pinboard",
      { notes: "documents", board: false },
      hooks().h,
    );
    expect(uninstallSaveNote.mock.calls.map((c) => c[1])).toEqual(["n1"]);
    expect(second.kind === "ready" && second.alreadySaved).toBe(2);
  });
});

describe("keeping the notes as files", () => {
  it("writes the files and saves no documents", async () => {
    const { h } = hooks();
    const r = await keepNotes("atlas.pinboard", { notes: "files", board: true }, h);
    expect(calls).toEqual(["plan", "files:true:true"]);
    expect(r.kind === "ready" && r.files).toEqual({
      folder: "/home/x/Notes",
      notes: 3,
      boardFile: "pinboard-board-x.json",
    });
  });

  it("a closed folder dialog stops everything: nothing saved, nothing uninstalled", async () => {
    uninstallExportFiles.mockResolvedValue(null);
    const r = stoppedOf(
      await keepNotes("atlas.pinboard", { notes: "documents", board: true }, hooks().h),
    );
    expect(r.reason).toBe("no-folder");
    expect(uninstallSaveNote).not.toHaveBeenCalled();
  });

  it("a file that could not be written stops it, in the backend's words", async () => {
    uninstallExportFiles.mockResolvedValue(
      written({
        notesWritten: 2,
        failed: { noteIndex: 2, message: "There is no room left there." },
      }),
    );
    const r = stoppedOf(
      await keepNotes("atlas.pinboard", { notes: "files", board: true }, hooks().h),
    );
    expect(r).toMatchObject({
      reason: "failed",
      problem: "There is no room left there.",
      filesWritten: 2,
    });
    expect(uninstallSaveNote).not.toHaveBeenCalled();
  });

  it("a plan that cannot be read stops it before anything is written", async () => {
    uninstallNotesPlan.mockRejectedValue({ code: "no-store", message: "PM's data isn't open." });
    const r = stoppedOf(
      await keepNotes("atlas.pinboard", { notes: "documents", board: true }, hooks().h),
    );
    expect(r.problem).toBe("PM's data isn't open.");
    expect(uninstallExportFiles).not.toHaveBeenCalled();
    expect(uninstallSaveNote).not.toHaveBeenCalled();
  });
});

describe("what the two boxes come to", () => {
  it("documents with the engine, files without it, nothing when everything is saved", () => {
    expect(keepChoiceOf(plan(2), true, true)).toEqual({ notes: "documents", board: true });
    expect(keepChoiceOf(plan(2, { engineReady: false }), true, false)).toEqual({
      notes: "files",
      board: false,
    });
    expect(keepChoiceOf(plan(2, { toSave: [], alreadySaved: 2 }), true, true)).toEqual({
      notes: "none",
      board: true,
    });
    expect(keepChoiceOf(plan(2), false, false)).toEqual({ notes: "none", board: false });
    expect(keepChoiceOf(plan(0, { boardBytes: 0 }), true, true)).toEqual({
      notes: "none",
      board: false,
    });
    expect(keepChoiceOf(null, true, true)).toEqual({ notes: "none", board: false });
  });

  it("says what was kept, and nothing when nothing was", () => {
    const ready = (over: Partial<Extract<KeepResult, { kind: "ready" }>>) =>
      ({ kind: "ready", saved: [], alreadySaved: 0, files: null, ...over }) as Extract<
        KeepResult,
        { kind: "ready" }
      >;
    expect(keptSentence(ready({}))).toBeNull();
    expect(
      keptSentence(
        ready({
          saved: [{ key: "a", title: "A" }],
          files: { folder: "/x", notes: 2, boardFile: "b.json" },
        }),
      ),
    ).toBe(
      "Before it went, PM kept 1 note saved as documents, 2 notes written as .md files, a copy of the whole board saved as a file.",
    );
  });
});
