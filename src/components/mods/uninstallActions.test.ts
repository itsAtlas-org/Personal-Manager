// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Uninstalling mods as Mods actions: one mod (with its follow-up and its note) and "Remove all"
// (one at a time, never deleting what the mods made, one summary, a refusal reported and the rest still
// going). The sentences are pure, so they are tested as sentences.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { currentModActions, resetModActions, runModAction } from "../../lib/modActions";
import type { ModUninstalled, NotesPlan } from "../../lib/modsTypes";
import { REMOVE_ALL_KEY, removeAllMods, removeAllNote, uninstallAction } from "./uninstallActions";

const uninstallMod = vi.fn<(id: string, also: boolean) => Promise<ModUninstalled>>();
const uninstallNotesPlan = vi.fn<(id: string) => Promise<NotesPlan | null>>();
vi.mock("../../lib/ipc", () => ({
  uninstallMod: (id: string, also: boolean) => uninstallMod(id, also),
  uninstallNotesPlan: (id: string) => uninstallNotesPlan(id),
}));

const gone = (id: string, extra: Partial<ModUninstalled> = {}): ModUninstalled => ({
  id,
  filesRemoved: true,
  giftsDeleted: null,
  prune: { tab: `mod:${id}`, foldPrefix: `mods.mod.${id}` },
  ...extra,
});

beforeEach(() => {
  uninstallMod.mockReset();
  uninstallNotesPlan.mockReset();
  uninstallNotesPlan.mockResolvedValue(null);
  resetModActions();
  localStorage.clear();
});
afterEach(resetModActions);

const notes = () => currentModActions().notes.map((n) => `${n.tone}: ${n.text}`);

describe("uninstalling one mod", () => {
  it("passes the person's choice, forgets the mod's folds and says it was uninstalled", async () => {
    localStorage.setItem("pm.fold.mods.mod.acme.tool", "open");
    uninstallMod.mockResolvedValue(gone("acme.tool"));
    const r = await uninstallAction("acme.tool", "Acme Tool", true);
    expect(r.ok).toBe(true);
    expect(uninstallMod).toHaveBeenCalledWith("acme.tool", true);
    expect(localStorage.getItem("pm.fold.mods.mod.acme.tool")).toBeNull();
    expect(notes()).toEqual(["info: “Acme Tool” was uninstalled."]);
  });

  it("says nothing when asked to be quiet, but still forgets the folds", async () => {
    localStorage.setItem("pm.fold.mods.mod.acme.tool", "open");
    uninstallMod.mockResolvedValue(gone("acme.tool"));
    await uninstallAction("acme.tool", "Acme Tool", false, true);
    expect(notes()).toEqual([]);
    expect(localStorage.getItem("pm.fold.mods.mod.acme.tool")).toBeNull();
  });

  it("keeps the folds and says nothing when the backend refuses", async () => {
    localStorage.setItem("pm.fold.mods.mod.acme.tool", "open");
    uninstallMod.mockRejectedValue({
      code: "no-store",
      message: "PM's data isn't open right now.",
    });
    const r = await uninstallAction("acme.tool", "Acme Tool", false);
    expect(r).toEqual({ ok: false, error: "PM's data isn't open right now." });
    expect(currentModActions().errors.get("acme.tool")).toBe("PM's data isn't open right now.");
    expect(localStorage.getItem("pm.fold.mods.mod.acme.tool")).toBe("open");
    expect(notes()).toEqual([]);
  });

  it("waits for a mod that already has something going instead of asking twice", async () => {
    let release!: () => void;
    const pause = runModAction(
      "acme.tool",
      "Pausing",
      "no",
      () => new Promise<void>((r) => (release = r)),
    );
    const r = await uninstallAction("acme.tool", "Acme Tool", false);
    expect(r).toEqual({ ok: false, error: null });
    expect(uninstallMod).not.toHaveBeenCalled();
    release();
    await pause;
  });
});

describe("the sentence after Remove all", () => {
  it("counts the mods, and is not a warning when all went cleanly", () => {
    expect(removeAllNote({ removed: ["A"], filesLeft: [], failed: [] })).toEqual({
      warning: false,
      text: "Removed 1 mod.",
    });
    expect(removeAllNote({ removed: ["A", "B"], filesLeft: [], failed: [] }).text).toBe(
      "Removed 2 mods.",
    );
  });

  it("names a mod that stayed and why, and files left behind, and warns", () => {
    expect(
      removeAllNote({
        removed: ["A"],
        filesLeft: ["A"],
        failed: [{ name: "B", error: "It is busy." }],
      }),
    ).toEqual({
      warning: true,
      text: "Removed 1 mod. “B” stayed: It is busy. PM couldn't delete all the files of “A”, and will clear them out itself later.",
    });
  });

  it("says so when there was nothing to remove", () => {
    expect(removeAllNote({ removed: [], filesLeft: [], failed: [] })).toEqual({
      warning: false,
      text: "There were no mods to remove.",
    });
  });
});

describe("Remove all", () => {
  it("never asks for what the mods made to be deleted, whatever else happens", async () => {
    uninstallMod.mockImplementation(async (id) => gone(id));
    await removeAllMods([
      { id: "acme.a", name: "A" },
      { id: "acme.b", name: "B" },
    ]);
    expect(uninstallMod.mock.calls.map((c) => c[1])).toEqual([false, false]);
  });

  it("reports a mod that was busy with something else, and removes the rest", async () => {
    let release!: () => void;
    const pause = runModAction(
      "acme.a",
      "Pausing",
      "no",
      () => new Promise<void>((r) => (release = r)),
    );
    uninstallMod.mockImplementation(async (id) => gone(id));
    const r = await removeAllMods([
      { id: "acme.a", name: "A" },
      { id: "acme.b", name: "B" },
    ]);
    expect(r.ok && r.value).toEqual({
      removed: ["B"],
      filesLeft: [],
      failed: [{ name: "A", error: "It was busy with something else. Try again in a moment." }],
    });
    expect(uninstallMod).toHaveBeenCalledTimes(1);
    release();
    await pause;
  });

  it("leaves the Pinboard for its own dialog, which offers to keep its notes and board first", async () => {
    const plan = (over: Partial<NotesPlan>): NotesPlan => ({
      total: 3,
      toSave: [{ key: "n1", title: "One" }],
      alreadySaved: 2,
      unsaveable: 0,
      engineReady: true,
      boardBytes: 900,
      source: "mod",
      ...over,
    });
    uninstallMod.mockImplementation(async (id) => gone(id));
    uninstallNotesPlan.mockImplementation(async (id) =>
      id === "atlas.pinboard" ? plan({}) : null,
    );
    let r = await removeAllMods([
      { id: "atlas.pinboard", name: "Pinboard" },
      { id: "acme.b", name: "B" },
    ]);
    expect(r.ok && r.value.removed).toEqual(["B"]);
    expect(r.ok && r.value.failed).toHaveLength(1);
    expect(r.ok && r.value.failed[0].error).toContain("Uninstall it on its own");
    expect(uninstallMod.mock.calls.map((c) => c[0])).toEqual(["acme.b"]);
    // A Pinboard with a board and every note already saved still stays: the board is the person's.
    uninstallMod.mockClear();
    uninstallNotesPlan.mockResolvedValue(plan({ toSave: [], alreadySaved: 3 }));
    await removeAllMods([{ id: "atlas.pinboard", name: "Pinboard" }]);
    expect(uninstallMod).not.toHaveBeenCalled();
    // One with nothing at all to keep goes like any other mod.
    uninstallNotesPlan.mockResolvedValue(
      plan({ total: 0, toSave: [], alreadySaved: 0, boardBytes: 0 }),
    );
    r = await removeAllMods([{ id: "atlas.pinboard", name: "Pinboard" }]);
    expect(r.ok && r.value.removed).toEqual(["Pinboard"]);
  });

  it("holds a mod back, saying so, when it cannot be told whether it holds notes", async () => {
    uninstallNotesPlan.mockRejectedValue({ code: "no-store", message: "PM's data isn't open." });
    const r = await removeAllMods([{ id: "acme.a", name: "A" }]);
    expect(r.ok && r.value.failed).toEqual([{ name: "A", error: "PM's data isn't open." }]);
    expect(uninstallMod).not.toHaveBeenCalled();
  });

  it("uses a plain line for a name, falling back to the id when nothing is left of it", async () => {
    uninstallMod.mockImplementation(async (id) => gone(id));
    const r = await removeAllMods([{ id: "acme.a", name: "‮\n" }]);
    expect(r.ok && r.value.removed).toEqual(["acme.a"]);
  });

  it("holds its own key while it runs, so it can't be started twice, and frees it after", async () => {
    let release!: () => void;
    uninstallMod.mockImplementation(async (id) => {
      await new Promise<void>((r) => (release = r));
      return gone(id);
    });
    const first = removeAllMods([{ id: "acme.a", name: "A" }]);
    expect(currentModActions().busy.get(REMOVE_ALL_KEY)).toBe("Removing all mods");
    expect(await removeAllMods([{ id: "acme.a", name: "A" }])).toEqual({ ok: false, error: null });
    await vi.waitFor(() => expect(uninstallMod).toHaveBeenCalledTimes(1));
    release();
    await first;
    expect(currentModActions().busy.has(REMOVE_ALL_KEY)).toBe(false);
    expect(notes()).toEqual(["info: Removed 1 mod."]);
  });
});
