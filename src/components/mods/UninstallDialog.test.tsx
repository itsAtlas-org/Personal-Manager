// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Uninstalling a mod (spec §1.3): the preview first (what goes, what stays, the count), "Also delete
// what it made" off by default, then the uninstall, then this device forgets the mod's tab placement
// and folds with what the backend returned. The uninstall runs to its end even if the view is gone.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { readFold, writeFold } from "../../lib/foldPrefs";
import { resetModActions, useModActions } from "../../lib/modActions";
import { NAV_PLACEMENT_KEY, parsePlacement, writeNavPlacement } from "../../lib/nav";
import type { ModUninstalled, UninstallPreview } from "../../lib/modsTypes";
import { ModRowsContext } from "../../lib/modRows";
import { LRI, mod, RLO, standing } from "./modFixtures";
import { plural, uninstallNote } from "./uninstallNote";

const uninstallPreview = vi.fn();
const uninstallMod = vi.fn();

vi.mock("../../lib/ipc", () => ({
  uninstallPreview: (id: string) => uninstallPreview(id),
  uninstallMod: (id: string, also: boolean) => uninstallMod(id, also),
}));
vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { UninstallDialog } from "./UninstallDialog";

const preview = (extra: Partial<UninstallPreview> = {}): UninstallPreview => ({
  id: "acme.tool",
  name: "Acme Tool",
  leaves: {
    contentBytes: 2048,
    cacheBytes: 0,
    documentDataBytes: 0,
    filesBytes: 1024 * 1024,
    navTab: "Acme Tool",
    calendarOverlay: null,
  },
  stays: { tasks: 0, milestones: 0, documents: 0, keptDocuments: 0 },
  removedBySafetyList: false,
  ...extra,
});

const done = (extra: Partial<ModUninstalled> = {}): ModUninstalled => ({
  id: "acme.tool",
  filesRemoved: true,
  giftsDeleted: null,
  prune: { tab: "mod:acme.tool", foldPrefix: "mods.mod.acme.tool" },
  ...extra,
});

const refresh = vi.fn(() => Promise.resolve());
const onClose = vi.fn();

function Notes() {
  return (
    <ul aria-label="notes">
      {useModActions().notes.map((n) => (
        <li key={n.id}>{n.text}</li>
      ))}
    </ul>
  );
}

const open = () =>
  render(
    <>
      <UninstallDialog modId="acme.tool" refresh={refresh} onClose={onClose} />
      <Notes />
    </>,
  );

beforeEach(() => {
  vi.clearAllMocks();
  resetModActions();
  localStorage.clear();
});
afterEach(cleanup);

describe("the preview", () => {
  it("says what goes, with sizes, and the tab, before anything is asked", async () => {
    uninstallPreview.mockResolvedValue(preview());
    open();
    expect(screen.getByText("Looking at what it has…")).toBeTruthy();
    const dialog = await screen.findByRole("dialog", { name: "Uninstall “Acme Tool”?" });
    expect(dialog.textContent).toContain("What it saved (2 KB)");
    expect(dialog.textContent).toContain("Its files (1 MB)");
    expect(dialog.textContent).toContain("Its tab in the sidebar, “Acme Tool”");
    expect(dialog.textContent).toContain("What it saved can't be brought back.");
    expect(uninstallMod).not.toHaveBeenCalled();
  });

  it("says what stays, with the count, and 'Also delete what it made' starts off", async () => {
    uninstallPreview.mockResolvedValue(
      preview({ stays: { tasks: 3, milestones: 1, documents: 2, keptDocuments: 0 } }),
    );
    open();
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).toContain("3 tasks, 1 milestone, 2 notes it made stay in PM");
    const choice = within(dialog).getByRole("checkbox") as HTMLInputElement;
    expect(choice.checked).toBe(false);
    expect(dialog.textContent).toContain(
      "Also delete what it made (6 things). This can't be undone.",
    );
  });

  it("offers no deletion choice when the mod made nothing", async () => {
    uninstallPreview.mockResolvedValue(preview());
    open();
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).queryByRole("checkbox")).toBeNull();
    expect(dialog.textContent).toContain("It hasn't made any tasks, milestones or notes.");
  });

  it("says a mod that saved nothing has nothing to lose", async () => {
    uninstallPreview.mockResolvedValue(
      preview({
        leaves: {
          contentBytes: 0,
          cacheBytes: 0,
          documentDataBytes: 0,
          filesBytes: 0,
          navTab: null,
          calendarOverlay: null,
        },
      }),
    );
    open();
    expect((await screen.findByRole("dialog")).textContent).toContain("It hasn't saved anything.");
  });

  it("says why a mod the safety list removed is being uninstalled", async () => {
    uninstallPreview.mockResolvedValue(preview({ removedBySafetyList: true }));
    open();
    expect((await screen.findByRole("dialog")).textContent).toContain(
      "PM's safety list switched this mod off.",
    );
  });

  it("shows the reason the safety list removed it, from the row, when opened where no row or strip says it", async () => {
    uninstallPreview.mockResolvedValue(preview({ removedBySafetyList: true }));
    const removed = mod({
      state: standing("removed-by-safety-list", {
        fix: "uninstall",
        text: `PM removed “Acme Tool”${RLO}: it was copying notes to a stranger's server.`,
      }),
    });
    render(
      <ModRowsContext.Provider
        value={{ rows: [removed], loaded: true, problem: null, refresh: () => Promise.resolve() }}
      >
        <UninstallDialog modId="acme.tool" refresh={refresh} onClose={onClose} />
      </ModRowsContext.Provider>,
    );
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).toContain("PM's safety list switched this mod off.");
    // The backend's sentence, as plain text with the invisible character taken out.
    expect(dialog.textContent).toContain(
      "PM removed “Acme Tool” : it was copying notes to a stranger's server.",
    );
    expect(dialog.textContent).not.toContain(RLO);
  });

  it("says no reason it doesn't have: a removed mod whose row isn't read yet shows the plain line only", async () => {
    uninstallPreview.mockResolvedValue(preview({ removedBySafetyList: true }));
    open();
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).toContain("PM's safety list switched this mod off.");
    expect(dialog.textContent).not.toContain("PM removed");
  });

  it("says the backend's sentence when the preview can't be made, and only offers Cancel", async () => {
    uninstallPreview.mockRejectedValue({
      code: "not-installed",
      message: "That mod isn't installed.",
    });
    open();
    expect(await screen.findByText("That mod isn't installed.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Uninstall" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onClose).toHaveBeenCalled();
  });

  it("draws the mod's own name as text", async () => {
    const evil = '<img src=x onerror="alert(1)">';
    uninstallPreview.mockResolvedValue(preview({ name: evil }));
    open();
    const dialog = await screen.findByRole("dialog");
    expect(dialog.querySelector("img")).toBeNull();
    // The title quotes the name, and a double quote inside it can't close that quote.
    expect(dialog.textContent).toContain("<img src=x onerror='alert(1)'>");
  });

  it("draws the mod's name and tab with invisible and direction-changing characters taken out", async () => {
    uninstallPreview.mockResolvedValue(
      preview({
        name: `Acme${RLO} Tool\nPM`,
        leaves: { ...preview().leaves, navTab: `Tab${RLO}${LRI}x` },
      }),
    );
    open();
    const dialog = await screen.findByRole("dialog", { name: "Uninstall “Acme Tool PM”?" });
    expect(dialog.textContent).toContain("Its tab in the sidebar, “Tab x”");
    expect(dialog.textContent).not.toContain(RLO);
    expect(dialog.textContent).not.toContain(LRI);
  });

  it("says the uninstall in the note with the name cleaned the same way", async () => {
    uninstallPreview.mockResolvedValue(preview({ name: `Acme${RLO}Tool` }));
    uninstallMod.mockResolvedValue(done());
    open();
    await screen.findByRole("dialog");
    fireEvent.click(screen.getByRole("button", { name: "Uninstall" }));
    expect(await screen.findByText("“Acme Tool” was uninstalled.")).toBeTruthy();
  });
});

describe("uninstalling", () => {
  it("keeps what the mod made unless asked: the flag is false, and nothing is sent before Uninstall", async () => {
    uninstallPreview.mockResolvedValue(
      preview({ stays: { tasks: 2, milestones: 0, documents: 0, keptDocuments: 0 } }),
    );
    uninstallMod.mockResolvedValue(done());
    open();
    await screen.findByRole("dialog");
    expect(uninstallMod).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Uninstall" }));
    await waitFor(() => expect(uninstallMod).toHaveBeenCalledWith("acme.tool", false));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(refresh).toHaveBeenCalled();
  });

  it("deletes what it made only when the person ticked the choice", async () => {
    uninstallPreview.mockResolvedValue(
      preview({ stays: { tasks: 2, milestones: 0, documents: 0, keptDocuments: 0 } }),
    );
    uninstallMod.mockResolvedValue(
      done({ giftsDeleted: { tasks: 2, milestones: 0, documents: 0, problem: null } }),
    );
    open();
    fireEvent.click(await screen.findByRole("checkbox"));
    fireEvent.click(screen.getByRole("button", { name: "Uninstall" }));
    await waitFor(() => expect(uninstallMod).toHaveBeenCalledWith("acme.tool", true));
    expect(await screen.findByText(/Deleted what it made: 2 tasks\./)).toBeTruthy();
  });

  it("never sends the delete flag for a mod that made nothing, whatever was ticked before", async () => {
    uninstallPreview.mockResolvedValue(preview());
    uninstallMod.mockResolvedValue(done());
    open();
    await screen.findByRole("dialog");
    fireEvent.click(screen.getByRole("button", { name: "Uninstall" }));
    await waitFor(() => expect(uninstallMod).toHaveBeenCalledWith("acme.tool", false));
  });

  it("then forgets the mod's tab placement and folds on this device, and nothing else's", async () => {
    uninstallPreview.mockResolvedValue(preview());
    uninstallMod.mockResolvedValue(done());
    writeNavPlacement({ hidden: ["mod:acme.tool", "mod:acme.other"], pinned: ["mod:acme.tool"] });
    writeFold("mods.mod.acme.tool", true);
    writeFold("mods.mod.acme.tool:permissions", false);
    writeFold("mods.mod.acme.other", true);
    open();
    await screen.findByRole("dialog");
    fireEvent.click(screen.getByRole("button", { name: "Uninstall" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(parsePlacement(localStorage.getItem(NAV_PLACEMENT_KEY))).toEqual({
      hidden: ["mod:acme.other"],
      pinned: [],
    });
    expect(readFold("mods.mod.acme.tool")).toBeNull();
    expect(readFold("mods.mod.acme.tool:permissions")).toBeNull();
    expect(readFold("mods.mod.acme.other")).toBe(true);
  });

  it("says it was uninstalled in a note the tab keeps, since the row is gone", async () => {
    uninstallPreview.mockResolvedValue(preview());
    uninstallMod.mockResolvedValue(done());
    open();
    await screen.findByRole("dialog");
    fireEvent.click(screen.getByRole("button", { name: "Uninstall" }));
    expect(await screen.findByText("“Acme Tool” was uninstalled.")).toBeTruthy();
  });

  it("is working, and can't be closed or pressed again, until the backend answers", async () => {
    uninstallPreview.mockResolvedValue(preview());
    let finish!: (d: ModUninstalled) => void;
    uninstallMod.mockReturnValue(
      new Promise<ModUninstalled>((res) => {
        finish = res;
      }),
    );
    open();
    await screen.findByRole("dialog");
    fireEvent.click(screen.getByRole("button", { name: "Uninstall" }));
    const working = await screen.findByRole("button", { name: "Uninstalling…" });
    expect((working as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Cancel" }) as HTMLButtonElement).disabled).toBe(
      true,
    );
    fireEvent.click(working);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => finish(done()));
    expect(uninstallMod).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(onClose).toHaveBeenCalled());
  });

  it("finishes the job when the view goes away mid-way: the prune and the note still happen", async () => {
    uninstallPreview.mockResolvedValue(preview());
    let finish!: (d: ModUninstalled) => void;
    uninstallMod.mockReturnValue(
      new Promise<ModUninstalled>((res) => {
        finish = res;
      }),
    );
    writeNavPlacement({ hidden: ["mod:acme.tool"], pinned: [] });
    writeFold("mods.mod.acme.tool", true);
    const view = open();
    await screen.findByRole("dialog");
    fireEvent.click(screen.getByRole("button", { name: "Uninstall" }));
    await screen.findByRole("button", { name: "Uninstalling…" });
    // The person switches tab: the Mods view, and this dialog with it, unmounts.
    view.unmount();
    await act(async () => finish(done()));
    expect(readFold("mods.mod.acme.tool")).toBeNull();
    expect(parsePlacement(localStorage.getItem(NAV_PLACEMENT_KEY)).hidden).toEqual([]);
    // Back on the tab, the note is there to be read.
    render(<Notes />);
    expect(screen.getByText("“Acme Tool” was uninstalled.")).toBeTruthy();
  });

  it("says the backend's refusal, keeps the dialog open, and prunes nothing", async () => {
    uninstallPreview.mockResolvedValue(preview());
    uninstallMod.mockRejectedValue({ code: "internal", message: "PM couldn't remove its files." });
    writeFold("mods.mod.acme.tool", true);
    open();
    await screen.findByRole("dialog");
    fireEvent.click(screen.getByRole("button", { name: "Uninstall" }));
    expect(await screen.findByText("PM couldn't remove its files.")).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
    expect(readFold("mods.mod.acme.tool")).toBe(true);
    expect(screen.getByRole("button", { name: "Uninstall" })).toBeTruthy();
  });
});

describe("what is said afterwards", () => {
  it("pluralises", () => {
    expect(plural(1, "task", "tasks")).toBe("1 task");
    expect(plural(2, "task", "tasks")).toBe("2 tasks");
    expect(plural(0, "task", "tasks")).toBe("0 tasks");
  });

  it("a plain uninstall is good news", () => {
    expect(uninstallNote("Acme", done())).toEqual({
      warning: false,
      text: "“Acme” was uninstalled.",
    });
  });

  it("says when files couldn't all be deleted, as a warning", () => {
    const note = uninstallNote("Acme", done({ filesRemoved: false }));
    expect(note.warning).toBe(true);
    expect(note.text).toContain("couldn't delete all of its files");
  });

  it("says what was deleted, and the first thing that couldn't be, as a warning", () => {
    const note = uninstallNote(
      "Acme",
      done({
        giftsDeleted: { tasks: 1, milestones: 2, documents: 3, problem: "One task is open." },
      }),
    );
    expect(note.warning).toBe(true);
    expect(note.text).toContain("Deleted what it made: 1 task, 2 milestones, 3 notes.");
    expect(note.text).toContain("PM couldn't delete everything it made: One task is open.");
  });

  it("says nothing was deleted when nothing was", () => {
    const note = uninstallNote(
      "Acme",
      done({ giftsDeleted: { tasks: 0, milestones: 0, documents: 0, problem: null } }),
    );
    expect(note).toEqual({ warning: false, text: "“Acme” was uninstalled." });
  });
});
