// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The install flows of the Mods tab outside any view: which command a file goes to (the signed screen,
// or, after the backend's `unsigned`, developer mode asked and the stark screen), what is staged with
// a way to start again, what the person is told when a step refuses, and the finish that marks a
// screen out of date only when the backend says so.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { consentScreen } from "../components/mods/modFixtures";
import {
  currentModActions as getSnapshotForTest,
  resetModActions,
  setModScreen,
} from "./modActions";
import {
  DEV_MODE_OFF_TEXT,
  FILE_KEY,
  FOLDER_KEY,
  finishInstall,
  isFolderScreen,
  restartScreen,
  startCatalogueInstall,
  startFileInstall,
  startFolderLoad,
} from "./modInstall";
import type { InstallResult } from "./modsTypes";

const installModFile = vi.fn();
const installUnsignedFile = vi.fn();
const installFromCatalogue = vi.fn();
const installModConsented = vi.fn();
const devModeState = vi.fn();
const loadTestMod = vi.fn();

vi.mock("./ipc", () => ({
  loadTestMod: (p: string) => loadTestMod(p),
  installModFile: (p: string) => installModFile(p),
  installUnsignedFile: (p: string) => installUnsignedFile(p),
  installFromCatalogue: (id: string) => installFromCatalogue(id),
  installModConsented: (t: string, d: Record<string, boolean>, c?: string) =>
    installModConsented(t, d, c),
  devModeState: () => devModeState(),
}));

beforeEach(() => {
  vi.clearAllMocks();
  resetModActions();
});
afterEach(resetModActions);

const signed = consentScreen({ id: "acme.signed", token: "tok-s", tier: "first", kind: "fresh" });
const stark = consentScreen({
  id: "acme.raw",
  token: "tok-u",
  tier: "third",
  kind: "fresh",
  unsigned: {
    confirmId: "acme.raw",
    reinstall: false,
    engineSealed: true,
    readOrInternet: null,
    testFolder: null,
  },
});
const unsignedRefusal = { code: "unsigned", message: "That file isn't signed." };

describe("installing from a file", () => {
  it("a signed file stages the signed screen and never asks about developer mode", async () => {
    installModFile.mockResolvedValue(signed);
    const r = await startFileInstall("/a/b.pmmod");
    expect(r).toEqual({ ok: true, value: signed });
    expect(installModFile).toHaveBeenCalledWith("/a/b.pmmod");
    expect(devModeState).not.toHaveBeenCalled();
    expect(installUnsignedFile).not.toHaveBeenCalled();
    const snap = getSnapshotForTest();
    expect(snap.screens.get("acme.signed")).toBe(signed);
    expect(snap.restarts.has("acme.signed")).toBe(true);
    expect(snap.busy.has(FILE_KEY)).toBe(false);
  });

  it("an unsigned file with developer mode ON gets the stark screen from the unsigned command", async () => {
    installModFile.mockRejectedValue(unsignedRefusal);
    devModeState.mockResolvedValue({ on: true, problem: null });
    installUnsignedFile.mockResolvedValue(stark);
    const r = await startFileInstall("/a/raw.pmmod");
    expect(r.ok).toBe(true);
    expect(installUnsignedFile).toHaveBeenCalledWith("/a/raw.pmmod");
    expect(getSnapshotForTest().screens.get("acme.raw")).toBe(stark);
  });

  it("an unsigned file with developer mode OFF says so and where to switch it on, and reads no further", async () => {
    installModFile.mockRejectedValue(unsignedRefusal);
    devModeState.mockResolvedValue({ on: false, problem: null });
    const r = await startFileInstall("/a/raw.pmmod");
    expect(r).toEqual({ ok: false, error: DEV_MODE_OFF_TEXT });
    expect(DEV_MODE_OFF_TEXT).toContain("developer mode is on");
    expect(DEV_MODE_OFF_TEXT).toContain("Settings, under Developer");
    expect(DEV_MODE_OFF_TEXT).toContain("nobody has checked it");
    expect(installUnsignedFile).not.toHaveBeenCalled();
    const snap = getSnapshotForTest();
    expect(snap.screens.size).toBe(0);
    expect(snap.errors.get(FILE_KEY)).toBe(DEV_MODE_OFF_TEXT);
  });

  it("when developer mode reads as off for a reason, the reason is added", async () => {
    installModFile.mockRejectedValue(unsignedRefusal);
    devModeState.mockResolvedValue({ on: false, problem: "This computer couldn't be named." });
    const r = await startFileInstall("/a/raw.pmmod");
    expect(r).toEqual({
      ok: false,
      error: `${DEV_MODE_OFF_TEXT} (This computer couldn't be named.)`,
    });
  });

  it("if the backend finds developer mode off at the stark step, the same sentence", async () => {
    installModFile.mockRejectedValue(unsignedRefusal);
    devModeState.mockResolvedValue({ on: true, problem: null });
    installUnsignedFile.mockRejectedValue({ code: "dev-mode-off", message: "Off." });
    const r = await startFileInstall("/a/raw.pmmod");
    expect(r).toEqual({ ok: false, error: DEV_MODE_OFF_TEXT });
  });

  it("any other refusal of the file is shown in the backend's words and asks nothing more", async () => {
    installModFile.mockRejectedValue({
      code: "file-too-large",
      message: "That file is bigger than a mod can be.",
    });
    const r = await startFileInstall("/a/huge.pmmod");
    expect(r).toEqual({ ok: false, error: "That file is bigger than a mod can be." });
    expect(devModeState).not.toHaveBeenCalled();
    expect(installUnsignedFile).not.toHaveBeenCalled();
  });

  it("another refusal of the stark step is shown as it is", async () => {
    installModFile.mockRejectedValue(unsignedRefusal);
    devModeState.mockResolvedValue({ on: true, problem: null });
    installUnsignedFile.mockRejectedValue({
      code: "safety-list",
      message: "PM's safety list stops this file.",
    });
    expect(await startFileInstall("/a/raw.pmmod")).toEqual({
      ok: false,
      error: "PM's safety list stops this file.",
    });
  });

  it("a developer-mode question that can't be answered shows the backend's sentence", async () => {
    installModFile.mockRejectedValue(unsignedRefusal);
    devModeState.mockRejectedValue({ code: "no-store", message: "PM's data isn't open." });
    expect(await startFileInstall("/a/raw.pmmod")).toEqual({
      ok: false,
      error: "PM's data isn't open.",
    });
  });

  it("is busy while the file is read, and a second pick meanwhile is refused", async () => {
    let finish!: (s: typeof signed) => void;
    installModFile.mockReturnValue(
      new Promise((res) => {
        finish = res;
      }),
    );
    const first = startFileInstall("/a/b.pmmod");
    expect(getSnapshotForTest().busy.get(FILE_KEY)).toBe("Reading the file");
    expect(await startFileInstall("/a/c.pmmod")).toEqual({ ok: false, error: null });
    expect(installModFile).toHaveBeenCalledTimes(1);
    finish(signed);
    await first;
    expect(getSnapshotForTest().busy.has(FILE_KEY)).toBe(false);
  });

  it("a new pick clears the last refusal", async () => {
    installModFile.mockRejectedValueOnce({ code: "not-a-mod", message: "Not a mod." });
    await startFileInstall("/a/x.pmmod");
    expect(getSnapshotForTest().errors.get(FILE_KEY)).toBe("Not a mod.");
    installModFile.mockResolvedValueOnce(signed);
    await startFileInstall("/a/y.pmmod");
    expect(getSnapshotForTest().errors.has(FILE_KEY)).toBe(false);
  });

  it("starting again reads the same file through the same steps", async () => {
    installModFile.mockRejectedValue(unsignedRefusal);
    devModeState.mockResolvedValue({ on: true, problem: null });
    installUnsignedFile.mockResolvedValue(stark);
    await startFileInstall("/a/raw.pmmod");
    const restart = getSnapshotForTest().restarts.get("acme.raw")!;
    installUnsignedFile.mockResolvedValue({ ...stark, token: "tok-u2" });
    const fresh = await restart();
    expect(fresh.token).toBe("tok-u2");
    expect(installModFile).toHaveBeenCalledTimes(2);
    expect(installUnsignedFile).toHaveBeenLastCalledWith("/a/raw.pmmod");
  });
});

describe("installing from the catalogue", () => {
  it("downloads, stages the screen with a way to start again, under the mod's own key", async () => {
    installFromCatalogue.mockResolvedValue(signed);
    const p = startCatalogueInstall("acme.signed");
    const seen = getSnapshotForTest().busy.get("acme.signed");
    await p;
    expect(seen).toBe("Downloading");
    expect(installFromCatalogue).toHaveBeenCalledWith("acme.signed");
    const snap = getSnapshotForTest();
    expect(snap.screens.get("acme.signed")).toBe(signed);
    expect(snap.restarts.has("acme.signed")).toBe(true);
    expect(snap.busy.size).toBe(0);
  });

  it("keeps the backend's refusal (offline, refused download) for that mod", async () => {
    installFromCatalogue.mockRejectedValue({
      code: "offline",
      message: "PM couldn't reach the internet.",
    });
    const r = await startCatalogueInstall("acme.signed");
    expect(r).toEqual({ ok: false, error: "PM couldn't reach the internet." });
    expect(getSnapshotForTest().errors.get("acme.signed")).toBe("PM couldn't reach the internet.");
    expect(getSnapshotForTest().screens.size).toBe(0);
  });

  it("a refusal with no message of its own gets PM's sentence for its code", async () => {
    installFromCatalogue.mockRejectedValue({ code: "download-refused", message: "" });
    const r = await startCatalogueInstall("acme.signed");
    expect(r).toEqual({
      ok: false,
      error: "PM refused the download because it wasn't what the catalogue promised.",
    });
  });
});

describe("starting again", () => {
  it("replaces the old screen with the fresh one", async () => {
    const old = consentScreen({ id: "acme.signed", token: "old" });
    const fresh = consentScreen({ id: "acme.signed", token: "new" });
    const restart = vi.fn().mockResolvedValue(fresh);
    await restartScreen(old, restart);
    expect(getSnapshotForTest().screens.get("acme.signed")).toBe(fresh);
  });

  it("drops the old screen when the fresh one is for another mod", async () => {
    const old = consentScreen({ id: "acme.signed", token: "old" });
    setModScreen(old.id, old);
    const fresh = consentScreen({ id: "acme.other", token: "new" });
    await restartScreen(old, () => Promise.resolve(fresh));
    const keys = [...getSnapshotForTest().screens.keys()];
    expect(keys).toEqual(["acme.other"]);
  });

  it("a failed restart keeps the old screen and says why", async () => {
    const old = consentScreen({ id: "acme.signed", token: "old" });
    setModScreen(old.id, old);
    const r = await restartScreen(old, () =>
      Promise.reject({ code: "file-unreadable", message: "PM couldn't read that file." }),
    );
    expect(r).toEqual({ ok: false, error: "PM couldn't read that file." });
    expect(getSnapshotForTest().screens.get("acme.signed")).toBe(old);
  });
});

describe("finishing an install", () => {
  const done: InstallResult = {
    id: "acme.signed",
    version: "1.0.0",
    state: "active",
    kind: "fresh",
    pruned: [],
    grantsSaved: true,
    grantsProblem: null,
  };

  it("sends the token, the answers and the typed id, and drops the screen", async () => {
    setModScreen(signed.id, signed);
    installModConsented.mockResolvedValue(done);
    const r = await finishInstall(signed, "Acme", { storage: true }, "acme.signed");
    expect(r.ok).toBe(true);
    expect(installModConsented).toHaveBeenCalledWith("tok-s", { storage: true }, "acme.signed");
    expect(getSnapshotForTest().screens.size).toBe(0);
  });

  it("is busy as 'Installing' while it runs, 'Updating' for an update", async () => {
    let finish!: (d: InstallResult) => void;
    installModConsented.mockReturnValue(
      new Promise<InstallResult>((res) => {
        finish = res;
      }),
    );
    const p = finishInstall(signed, "Acme", {});
    expect(getSnapshotForTest().busy.get("acme.signed")).toBe("Installing");
    finish(done);
    await p;
    resetModActions();
    installModConsented.mockReturnValue(
      new Promise<InstallResult>((res) => {
        finish = res;
      }),
    );
    const q = finishInstall({ ...signed, kind: "update" }, "Acme", {});
    expect(getSnapshotForTest().busy.get("acme.signed")).toBe("Updating");
    finish(done);
    await q;
  });

  it("marks the screen out of date only when the backend says it expired", async () => {
    setModScreen(signed.id, signed);
    installModConsented.mockRejectedValueOnce({ code: "refused", message: "No." });
    await finishInstall(signed, "Acme", {});
    expect(getSnapshotForTest().expired.has("acme.signed")).toBe(false);
    installModConsented.mockRejectedValueOnce({ code: "consent-expired", message: "Old." });
    await finishInstall(signed, "Acme", {});
    expect(getSnapshotForTest().expired.has("acme.signed")).toBe(true);
    // A fresh screen for the same mod is not expired.
    setModScreen(signed.id, { ...signed, token: "tok-2" });
    expect(getSnapshotForTest().expired.has("acme.signed")).toBe(false);
  });

  it("says when the answers could not be saved, using the cleaned name it was given", async () => {
    installModConsented.mockResolvedValue({
      ...done,
      grantsSaved: false,
      grantsProblem: { code: "internal", message: "Busy." },
    });
    await finishInstall(signed, "Clean Name", {});
    const note = getSnapshotForTest().notes[0];
    expect(note.tone).toBe("warning");
    expect(note.text).toContain(
      "Clean Name was installed, but PM couldn't save your answers: Busy.",
    );
  });
});

describe("loading a test folder", () => {
  const folder = consentScreen({
    id: "acme.dev",
    token: "tok-f",
    tier: "third",
    kind: "fresh",
    from: "folder",
    unsigned: {
      confirmId: "acme.dev",
      reinstall: false,
      engineSealed: true,
      readOrInternet: null,
      testFolder: "/home/dev/acme",
    },
  });

  it("reads the folder, loads nothing, and stages the screen with a way to start again", async () => {
    loadTestMod.mockResolvedValue(folder);
    const r = await startFolderLoad("/home/dev/acme");
    expect(r).toEqual({ ok: true, value: folder });
    expect(loadTestMod).toHaveBeenCalledWith("/home/dev/acme");
    expect(installModConsented).not.toHaveBeenCalled();
    const snap = getSnapshotForTest();
    expect(snap.screens.get("acme.dev")).toEqual(folder);
    // Starting again reads the SAME folder again.
    await snap.restarts.get("acme.dev")!();
    expect(loadTestMod).toHaveBeenCalledTimes(2);
    expect(loadTestMod).toHaveBeenLastCalledWith("/home/dev/acme");
  });

  it("is busy under its own key while it reads, and a second ask waits for the first", async () => {
    let read!: (s: typeof folder) => void;
    loadTestMod.mockReturnValue(new Promise((r) => (read = r)));
    const first = startFolderLoad("/a");
    expect(getSnapshotForTest().busy.get(FOLDER_KEY)).toBe("Reading the folder");
    expect(await startFolderLoad("/b")).toEqual({ ok: false, error: null });
    expect(loadTestMod).toHaveBeenCalledTimes(1);
    read(folder);
    await first;
    expect(getSnapshotForTest().busy.has(FOLDER_KEY)).toBe(false);
  });

  it("keeps the backend's sentence when it refuses, and stages nothing", async () => {
    loadTestMod.mockRejectedValue({
      code: "dev-mode-off",
      message: "Developer mode is off on this computer.",
    });
    const r = await startFolderLoad("/home/dev/acme");
    expect(r).toEqual({ ok: false, error: "Developer mode is off on this computer." });
    expect(getSnapshotForTest().errors.get(FOLDER_KEY)).toBe(
      "Developer mode is off on this computer.",
    );
    expect(getSnapshotForTest().screens.size).toBe(0);
  });

  it("forgets the last refusal when it tries again", async () => {
    loadTestMod.mockRejectedValueOnce(new Error("no"));
    await startFolderLoad("/a");
    expect(getSnapshotForTest().errors.has(FOLDER_KEY)).toBe(true);
    loadTestMod.mockResolvedValueOnce(folder);
    await startFolderLoad("/a");
    expect(getSnapshotForTest().errors.has(FOLDER_KEY)).toBe(false);
  });

  it("a folder screen belongs to Settings, a file's or a catalogue's to the Mods tab", () => {
    expect(isFolderScreen(folder)).toBe(true);
    // Either mark alone is enough to keep it out of the Mods tab.
    expect(isFolderScreen({ ...folder, from: "file" })).toBe(true);
    expect(isFolderScreen({ ...folder, unsigned: { ...folder.unsigned!, testFolder: null } })).toBe(
      true,
    );
    expect(isFolderScreen(stark)).toBe(false);
    expect(isFolderScreen(signed)).toBe(false);
    expect(isFolderScreen({ ...signed, from: "catalogue" })).toBe(false);
  });
});
