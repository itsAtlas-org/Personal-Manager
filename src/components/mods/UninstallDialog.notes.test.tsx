// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Uninstalling the Pinboard keeps its notes first (Mods spec D9, K5b): the offer, the order (files,
// then one note per call, then the uninstall and nothing before every call answered), the progress, a
// Cancel between notes, a failure that stops with nothing deleted, and a retry that saves nothing twice.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { resetModActions } from "../../lib/modActions";
import type {
  ExportOutcome,
  ModUninstalled,
  NotesPlan,
  NoteSaved,
  UninstallPreview,
} from "../../lib/modsTypes";

const order: string[] = [];
const uninstallPreview = vi.fn();
const uninstallMod = vi.fn();
const uninstallNotesPlan = vi.fn<(id: string) => Promise<NotesPlan | null>>();
const uninstallSaveNote = vi.fn<(id: string, key: string) => Promise<NoteSaved>>();
const uninstallExportFiles =
  vi.fn<(id: string, notes: boolean, board: boolean) => Promise<ExportOutcome | null>>();

vi.mock("../../lib/ipc", () => ({
  uninstallPreview: (id: string) => uninstallPreview(id),
  uninstallMod: (id: string, also: boolean) => uninstallMod(id, also),
  uninstallNotesPlan: (id: string) => uninstallNotesPlan(id),
  uninstallSaveNote: (id: string, key: string) => uninstallSaveNote(id, key),
  uninstallExportFiles: (id: string, n: boolean, b: boolean) => uninstallExportFiles(id, n, b),
}));
vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { UninstallDialog } from "./UninstallDialog";

const plan = (n: number, over: Partial<NotesPlan> = {}): NotesPlan => ({
  total: n,
  toSave: Array.from({ length: n }, (_, i) => ({ key: `n${i + 1}`, title: `Note ${i + 1}` })),
  alreadySaved: 0,
  unsaveable: 0,
  engineReady: true,
  boardBytes: 4096,
  source: "mod",
  ...over,
});

const preview = (notes: NotesPlan | null): UninstallPreview => ({
  id: "atlas.pinboard",
  name: "Pinboard",
  leaves: {
    contentBytes: 4096,
    cacheBytes: 0,
    documentDataBytes: 0,
    filesBytes: 0,
    navTab: "Pinboard",
    calendarOverlay: "Pinboard",
  },
  stays: { tasks: 0, milestones: 0, documents: 0, keptDocuments: 0 },
  removedBySafetyList: false,
  notes,
});

const uninstalled: ModUninstalled = {
  id: "atlas.pinboard",
  filesRemoved: true,
  giftsDeleted: null,
  prune: { tab: "mod:atlas.pinboard", foldPrefix: "mods.mod.atlas.pinboard" },
};

const refresh = vi.fn(() => Promise.resolve());
const onClose = vi.fn();
const onDone = vi.fn();

const open = (notes: NotesPlan | null) => {
  uninstallPreview.mockResolvedValue(preview(notes));
  uninstallNotesPlan.mockResolvedValue(notes);
  return render(
    <UninstallDialog modId="atlas.pinboard" refresh={refresh} onClose={onClose} onDone={onDone} />,
  );
};

const press = async (name: string) => {
  const dialog = await screen.findByRole("dialog");
  fireEvent.click(within(dialog).getByRole("button", { name }));
};

beforeEach(() => {
  vi.clearAllMocks();
  order.length = 0;
  resetModActions();
  localStorage.clear();
  uninstallSaveNote.mockImplementation(async (_id, key) => {
    order.push(`save:${key}`);
    return { outcome: "saved", documentId: 1 };
  });
  uninstallExportFiles.mockImplementation(async (_id, notes, board) => {
    order.push(`files:${notes}:${board}`);
    return {
      folder: "/home/x/Pinboard",
      notesWritten: notes ? 3 : 0,
      boardFile: board ? "pinboard-board-2026-11-25.json" : null,
      failed: null,
    };
  });
  uninstallMod.mockImplementation(async () => {
    order.push("uninstall");
    return uninstalled;
  });
});
afterEach(cleanup);

describe("the offer", () => {
  it("offers to save the notes as documents and the whole board as a file, both ticked", async () => {
    open(plan(3));
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).toContain("Your notes");
    const boxes = within(dialog).getAllByRole("checkbox") as HTMLInputElement[];
    expect(boxes.map((b) => b.checked)).toEqual([true, true]);
    expect(dialog.textContent).toContain("Save my 3 notes as documents first");
    expect(dialog.textContent).toContain("Save a copy of my whole board as a file");
  });

  it("says the board file is a plain-text safety copy PM can't open, and that PM keeps the old copy", async () => {
    open(plan(3));
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).toContain("A plain-text safety copy. PM can't open it yet.");
    expect(dialog.textContent).toContain(
      "PM keeps the board as it was when the Pinboard moved, until a later update clears it.",
    );
    expect(dialog.textContent).not.toContain("—");
  });

  it("says something else about the old copy before the board has moved", async () => {
    open(plan(3, { source: "legacy" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).not.toContain("as it was when the Pinboard moved");
    expect(dialog.textContent).toContain("PM also still keeps your board in its own storage.");
  });

  it("offers .md files in a folder instead when the document engine is not set up", async () => {
    open(plan(3, { engineReady: false }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).toContain("Save my 3 notes as .md files in a folder");
    expect(dialog.textContent).toContain("document engine isn't set up");
    expect(dialog.textContent).not.toContain("as documents first");
  });

  it("warns past 50 unsaved notes that it can take minutes, and not at 50", async () => {
    const { unmount } = open(plan(51));
    expect((await screen.findByRole("dialog")).textContent).toContain("can take a few minutes");
    unmount();
    open(plan(50));
    expect((await screen.findByRole("dialog")).textContent).not.toContain("can take a few minutes");
  });

  it("says every note is a document already when there is nothing left to save", async () => {
    open(plan(4, { toSave: [], alreadySaved: 4 }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).toContain("All 4 of your notes are already saved as documents");
    expect(within(dialog).getAllByRole("checkbox")).toHaveLength(1); // only the board file
  });

  it("offers nothing for another mod", async () => {
    open(null);
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).not.toContain("Your notes");
    expect(within(dialog).queryByRole("checkbox")).toBeNull();
  });
});

describe("the order", () => {
  it("saves the files, then each note, and uninstalls only after the last one", async () => {
    open(plan(3));
    await press("Uninstall");
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(order).toEqual(["files:false:true", "save:n1", "save:n2", "save:n3", "uninstall"]);
    expect(uninstallMod).toHaveBeenCalledWith("atlas.pinboard", false);
    // The Mods tab's line says what was kept.
    expect(onDone.mock.calls[0][0].text).toContain("PM kept 3 notes saved as documents");
  });

  it("shows 'Saving 2 of 3' while the second note is being saved, and cannot be closed", async () => {
    let finish!: () => void;
    uninstallSaveNote.mockImplementation(async (_id, key) => {
      order.push(`save:${key}`);
      if (key === "n2") await new Promise<void>((r) => (finish = r));
      return { outcome: "saved", documentId: 1 };
    });
    open(plan(3));
    await press("Uninstall");
    expect((await screen.findByRole("status")).textContent).toBe("Saving 2 of 3");
    expect(order).not.toContain("uninstall");
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => finish());
    await waitFor(() => expect(order[order.length - 1]).toBe("uninstall"));
  });

  it("goes straight to the uninstall when both boxes are unticked", async () => {
    open(plan(3));
    const dialog = await screen.findByRole("dialog");
    for (const box of within(dialog).getAllByRole("checkbox")) fireEvent.click(box);
    await press("Uninstall");
    await waitFor(() => expect(order).toEqual(["uninstall"]));
    expect(uninstallNotesPlan).not.toHaveBeenCalled();
  });

  it("writes files only, in a folder, when the engine is off, and then uninstalls", async () => {
    open(plan(3, { engineReady: false }));
    await press("Uninstall");
    await waitFor(() => expect(order[order.length - 1]).toBe("uninstall"));
    expect(order).toEqual(["files:true:true", "uninstall"]);
    expect(uninstallSaveNote).not.toHaveBeenCalled();
  });
});

describe("when it stops", () => {
  it("a note that fails stops the uninstall with nothing deleted, and the dialog says what was saved", async () => {
    uninstallSaveNote.mockImplementation(async (_id, key) => {
      order.push(`save:${key}`);
      if (key === "n3")
        throw { code: "internal", message: "PM couldn't save this note as a document." };
      return { outcome: "saved", documentId: 1 };
    });
    open(plan(5));
    await press("Uninstall");
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(dialog.textContent).toContain("Nothing was uninstalled."));
    expect(dialog.textContent).toContain("PM couldn't save “Note 3”.");
    expect(dialog.textContent).toContain("PM couldn't save this note as a document.");
    expect(dialog.textContent).toContain(
      "2 of 5 notes were saved as documents first, and they stay.",
    );
    expect(dialog.textContent).toContain("Note 1");
    expect(dialog.textContent).toContain("Note 2");
    expect(order).toEqual(["files:false:true", "save:n1", "save:n2", "save:n3"]);
    expect(uninstallMod).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("pressing Uninstall again carries on from the plan the backend now gives, saving nothing twice", async () => {
    uninstallSaveNote.mockRejectedValueOnce({ code: "internal", message: "no" });
    open(plan(3));
    await press("Uninstall");
    await waitFor(() =>
      expect(screen.getByRole("dialog").textContent).toContain("Nothing was uninstalled."),
    );
    uninstallNotesPlan.mockResolvedValue(
      plan(3, { toSave: [{ key: "n1", title: "Note 1" }], alreadySaved: 2 }),
    );
    uninstallSaveNote.mockClear();
    order.length = 0;
    await press("Uninstall");
    await waitFor(() => expect(order[order.length - 1]).toBe("uninstall"));
    expect(order.filter((o) => o.startsWith("save:"))).toEqual(["save:n1"]);
  });

  it("Cancel between notes stops there and uninstalls nothing", async () => {
    let finish!: () => void;
    uninstallSaveNote.mockImplementation(async (_id, key) => {
      order.push(`save:${key}`);
      if (key === "n2") await new Promise<void>((r) => (finish = r));
      return { outcome: "saved", documentId: 1 };
    });
    open(plan(5));
    await press("Uninstall");
    await screen.findByRole("status");
    await waitFor(() => expect(order).toContain("save:n2"));
    await press("Cancel");
    await act(async () => finish());
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(dialog.textContent).toContain("You stopped it."));
    expect(dialog.textContent).toContain("Nothing was uninstalled.");
    expect(dialog.textContent).toContain("2 of 5 notes were saved as documents first");
    expect(order).toEqual(["files:false:true", "save:n1", "save:n2"]);
    expect(uninstallMod).not.toHaveBeenCalled();
  });

  it("Cancel pressed while the last note is being saved stops: nothing is uninstalled", async () => {
    let finish!: () => void;
    uninstallSaveNote.mockImplementation(async (_id, key) => {
      order.push(`save:${key}`);
      await new Promise<void>((r) => (finish = r));
      return { outcome: "saved", documentId: 1 };
    });
    open(plan(1));
    await press("Uninstall");
    await waitFor(() => expect(order).toContain("save:n1"));
    await press("Cancel");
    expect(screen.getByRole("button", { name: "Stopping…" })).toBeTruthy();
    await act(async () => finish());
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(dialog.textContent).toContain("You stopped it."));
    expect(dialog.textContent).toContain("Nothing was uninstalled.");
    expect(uninstallMod).not.toHaveBeenCalled();
  });

  it("a closed folder dialog stops it: nothing saved, nothing uninstalled", async () => {
    uninstallExportFiles.mockResolvedValue(null);
    open(plan(3));
    await press("Uninstall");
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(dialog.textContent).toContain("No folder was chosen."));
    expect(uninstallSaveNote).not.toHaveBeenCalled();
    expect(uninstallMod).not.toHaveBeenCalled();
  });

  it("draws a note's title as clean, clamped text", async () => {
    uninstallSaveNote.mockRejectedValue({ code: "internal", message: "no" });
    const hostile = `‮evil${"x".repeat(200)}\n<img src=x onerror=alert(1)>`;
    open(plan(1, { toSave: [{ key: "n1", title: hostile }] }));
    await press("Uninstall");
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(dialog.textContent).toContain("Nothing was uninstalled."));
    expect(dialog.textContent).not.toContain("‮");
    expect(dialog.querySelector("img")).toBeNull();
    expect(dialog.textContent).toContain("…");
  });
});
