// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Settings → Storage's two folds (Mods spec §1.14): "Built in" and "Mods", each remembered on this
// device; the Mods section's sizes, Uninstall through the mod runtime's own dialog, and "Remove all
// mods" (confirmed, one mod at a time, never deleting what the mods made); the reworded copy; and the
// pointer to Local AI instead of a second copy of its switch.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { readFold, writeFold } from "../lib/foldPrefs";
import { HELP } from "../lib/help";
import { currentModActions, resetModActions } from "../lib/modActions";
import { ModRowsContext } from "../lib/modRows";
import type { ModInfo, ModUninstalled, UninstallPreview } from "../lib/modsTypes";
import type { LocalAiSwitch, StorageMod, StorageReport } from "../lib/types";
import { mod, RLO } from "./mods/modFixtures";

const listStorageComponents = vi.fn<() => Promise<StorageReport>>();
const getLocalAiSwitch = vi.fn<() => Promise<LocalAiSwitch>>();
const uninstallPreview = vi.fn<(id: string) => Promise<UninstallPreview>>();
const uninstallMod = vi.fn<(id: string, also: boolean) => Promise<ModUninstalled>>();

vi.mock("../lib/ipc", () => ({
  listStorageComponents: () => listStorageComponents(),
  removeStorageComponent: () => Promise.resolve(),
  installOptionalOcr: () => Promise.resolve(),
  onOcrInstall: () => Promise.resolve(() => {}),
  getLocalAiSwitch: () => getLocalAiSwitch(),
  uninstallPreview: (id: string) => uninstallPreview(id),
  uninstallMod: (id: string, also: boolean) => uninstallMod(id, also),
}));
vi.mock("../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { StorageSettings } from "./StorageSettings";

const MB = 1024 * 1024;

const storageMod = (extra: Partial<StorageMod> = {}): StorageMod => ({
  id: "acme.tool",
  name: "Acme Tool",
  version: "1.2.0",
  folder_bytes: 3 * MB,
  data_bytes: MB,
  ...extra,
});
const tool = storageMod();
const beta = storageMod({ id: "acme.beta", name: "Beta", folder_bytes: 2 * MB, data_bytes: 0 });

const report = (...mods: StorageMod[]): StorageReport => ({
  total_bytes: 5 * MB,
  components: [
    {
      id: "engine",
      label: "Document engine",
      detail: "Parses your files.",
      size_bytes: 5 * MB,
      approximate: false,
      child: false,
      status: "required",
      blockers: [],
      manage: null,
      note: null,
    },
  ],
  mods,
});

const sw = (on: boolean): LocalAiSwitch => ({
  on,
  chat_needs_model: false,
  background_needs_model: false,
  server_saved: false,
});

const preview = (extra: Partial<UninstallPreview> = {}): UninstallPreview => ({
  id: "acme.tool",
  name: "Acme Tool",
  leaves: {
    contentBytes: 1024,
    cacheBytes: 0,
    documentDataBytes: 0,
    filesBytes: 3 * MB,
    navTab: null,
    calendarOverlay: null,
  },
  stays: { tasks: 0, milestones: 0, documents: 0 },
  removedBySafetyList: false,
  ...extra,
});

const gone = (id: string, extra: Partial<ModUninstalled> = {}): ModUninstalled => ({
  id,
  filesRemoved: true,
  giftsDeleted: null,
  prune: { tab: `mod:${id}`, foldPrefix: `mods.mod.${id}` },
  ...extra,
});

function view(onNavigate: (t: string) => void = () => {}, rows: readonly ModInfo[] = []) {
  return (
    <ModRowsContext.Provider
      value={{ rows, loaded: true, problem: null, refresh: () => Promise.resolve() }}
    >
      <StorageSettings onNavigate={onNavigate} />
    </ModRowsContext.Provider>
  );
}

beforeEach(() => {
  for (const f of [listStorageComponents, getLocalAiSwitch, uninstallPreview, uninstallMod])
    f.mockReset();
  resetModActions();
  localStorage.clear();
  getLocalAiSwitch.mockResolvedValue(sw(true));
  listStorageComponents.mockResolvedValue(report(tool, beta));
  uninstallPreview.mockResolvedValue(preview());
});
afterEach(cleanup);

const toggleOf = (name: string) =>
  screen.getByRole("button", { name: new RegExp(`^▸?\\s*${name}`) });
const modsSection = () =>
  within(document.querySelector<HTMLElement>('[data-help="settings-storage-mods"]')!);

describe("the two sections", () => {
  it("shows Built in and Mods, each open the first time, each with what it holds", async () => {
    render(view());
    await screen.findByText("Acme Tool");
    expect(toggleOf("Built in").getAttribute("aria-expanded")).toBe("true");
    expect(toggleOf("Mods").getAttribute("aria-expanded")).toBe("true");
    // Built in's total is the components'; the mods' footprint is its own, not added to it.
    expect(toggleOf("Built in").textContent).toContain("5 MB");
    // Some of what a mod saved is an estimate, so the total says so.
    expect(toggleOf("Mods").textContent).toContain("~6 MB");
    expect(screen.getByText("Document engine")).toBeTruthy();
  });

  it("remembers each fold on this device, apart from the other", async () => {
    const first = render(view());
    await screen.findByText("Acme Tool");
    fireEvent.click(toggleOf("Mods"));
    expect(readFold("settings.storage.mods")).toBe(false);
    expect(readFold("settings.storage.builtIn")).toBeNull();
    first.unmount();

    render(view());
    await screen.findByText("Document engine");
    expect(toggleOf("Mods").getAttribute("aria-expanded")).toBe("false");
    expect(toggleOf("Built in").getAttribute("aria-expanded")).toBe("true");
    fireEvent.click(toggleOf("Built in"));
    expect(readFold("settings.storage.builtIn")).toBe(false);
  });

  it("starts a section folded when that is what was chosen", async () => {
    writeFold("settings.storage.builtIn", false);
    render(view());
    await screen.findByText("Acme Tool");
    expect(toggleOf("Built in").getAttribute("aria-expanded")).toBe("false");
    expect(toggleOf("Mods").getAttribute("aria-expanded")).toBe("true");
  });

  it("no longer says everything re-downloads, which isn't true of a mod from a file", async () => {
    render(view());
    await screen.findByText("Acme Tool");
    const text = document.body.textContent ?? "";
    expect(text).not.toContain("Everything here re-downloads");
    expect(text).not.toContain("re-downloads");
    expect(text).toContain("The built-in components download again when you need them.");
    expect(text).toContain("Mods are different");
    expect(text).toContain(
      "A mod you installed from a file, or a test mod from a folder, can't be downloaded again",
    );
  });

  it("has help entries for every surface it draws", async () => {
    render(view());
    await screen.findByText("Acme Tool");
    for (const el of document.querySelectorAll("[data-help]")) {
      const id = el.getAttribute("data-help")!;
      expect(HELP[id], `no help entry for ${id}`).toBeDefined();
    }
    expect(HELP["settings-storage-mods-remove-all"]).toBeDefined();
  });
});

describe("pointing at Local AI", () => {
  it("points at the Local AI tab while it is on, and doesn't mirror its switch", async () => {
    const onNavigate = vi.fn();
    render(view(onNavigate));
    fireEvent.click(await screen.findByRole("button", { name: "Local AI →" }));
    expect(onNavigate).toHaveBeenCalledWith("localai");
    expect(screen.queryByRole("switch")).toBeNull();
  });

  it("says Local AI is off and points at AI & Models, where the way back on is", async () => {
    getLocalAiSwitch.mockResolvedValue(sw(false));
    const onNavigate = vi.fn();
    render(view(onNavigate));
    expect(await screen.findByText(/Local AI is off/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "AI & Models →" }));
    expect(onNavigate).toHaveBeenCalledWith("ai");
  });

  it("says nothing about it until the switch has answered", async () => {
    getLocalAiSwitch.mockRejectedValue(new Error("locked"));
    render(view());
    await screen.findByText("Acme Tool");
    expect(screen.queryByRole("button", { name: "Local AI →" })).toBeNull();
    expect(screen.queryByRole("button", { name: "AI & Models →" })).toBeNull();
  });
});

describe("what each mod takes", () => {
  it("shows its size, its files and what it saved, calling the saved figure an estimate", async () => {
    render(view());
    await screen.findByText("Acme Tool");
    const items = within(screen.getByText("Acme Tool").closest("li")!);
    expect(items.getByText("~4 MB")).toBeTruthy();
    expect(items.getByText("1.2.0")).toBeTruthy();
    expect(
      items.getByText(/Its files 3 MB\. What it saved about 1 MB \(an estimate\)\./),
    ).toBeTruthy();
  });

  it("calls a size measured, not estimated, when the mod saved nothing", async () => {
    render(view());
    await screen.findByText("Beta");
    const item = within(screen.getByText("Beta").closest("li")!);
    expect(item.getByText("2 MB")).toBeTruthy();
    expect(item.getByText(/Its files 2 MB\. What it saved 0 B\./)).toBeTruthy();
  });

  it("marks a test mod as read from the developer's own folder", async () => {
    listStorageComponents.mockResolvedValue(
      report(storageMod({ id: "dev.test", name: "Dev Test", folder_bytes: 0 })),
    );
    render(view(() => {}, [mod({ id: "dev.test", installedFrom: "folder" })]));
    await screen.findByText("Dev Test");
    expect(screen.getByText(/A test mod, read from your own folder\./)).toBeTruthy();
    expect(screen.queryByText(/Its files/)).toBeNull();
  });

  it("says there are none, and offers no Remove all", async () => {
    listStorageComponents.mockResolvedValue(report());
    render(view());
    expect(await screen.findByText("No mods are installed.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Remove all mods/ })).toBeNull();
  });

  it("draws a name a mod supplied as one plain line", async () => {
    listStorageComponents.mockResolvedValue(
      report(storageMod({ name: `${RLO}Tool\nPM: all fine` })),
    );
    render(view());
    await screen.findByText("Tool PM: all fine");
    expect(document.body.textContent).not.toContain(RLO);
  });
});

describe("Uninstall", () => {
  it("opens the uninstall dialog with its preview, and reads the report again once it has worked", async () => {
    uninstallMod.mockResolvedValue(gone("acme.tool"));
    localStorage.setItem("pm.fold.mods.mod.acme.tool", "open");
    render(view());
    fireEvent.click(await screen.findByRole("button", { name: "Uninstall Acme Tool" }));
    const dialog = await screen.findByRole("dialog");
    expect(uninstallPreview).toHaveBeenCalledWith("acme.tool");
    expect(await within(dialog).findByText("What goes")).toBeTruthy();
    // Nothing happens until the dialog's own button.
    expect(uninstallMod).not.toHaveBeenCalled();

    listStorageComponents.mockResolvedValue(report(beta));
    fireEvent.click(within(dialog).getByRole("button", { name: "Uninstall" }));
    await waitFor(() => expect(uninstallMod).toHaveBeenCalledWith("acme.tool", false));
    await waitFor(() => expect(screen.queryByText("Acme Tool")).toBeNull());
    expect(listStorageComponents).toHaveBeenCalledTimes(2);
    // Said here, where the Mods tab's notes aren't on screen.
    expect(modsSection().getByText("“Acme Tool” was uninstalled.")).toBeTruthy();
    expect(localStorage.getItem("pm.fold.mods.mod.acme.tool")).toBeNull();
  });

  it("keeps 'also delete what it made' the person's choice, off until they turn it on", async () => {
    uninstallPreview.mockResolvedValue(
      preview({ stays: { tasks: 2, milestones: 0, documents: 0 } }),
    );
    uninstallMod.mockResolvedValue(gone("acme.tool"));
    render(view());
    fireEvent.click(await screen.findByRole("button", { name: "Uninstall Acme Tool" }));
    const dialog = await screen.findByRole("dialog");
    const box = (await within(dialog).findByRole("checkbox")) as HTMLInputElement;
    expect(box.checked).toBe(false);
    fireEvent.click(box);
    fireEvent.click(within(dialog).getByRole("button", { name: "Uninstall" }));
    await waitFor(() => expect(uninstallMod).toHaveBeenCalledWith("acme.tool", true));
  });

  it("shows the backend's refusal in the dialog and leaves the mod in the list", async () => {
    uninstallMod.mockRejectedValue({
      code: "no-store",
      message: "PM's data isn't open right now.",
    });
    render(view());
    fireEvent.click(await screen.findByRole("button", { name: "Uninstall Acme Tool" }));
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText("What goes");
    fireEvent.click(within(dialog).getByRole("button", { name: "Uninstall" }));
    expect(await within(dialog).findByText("PM's data isn't open right now.")).toBeTruthy();
    expect(screen.getAllByText("Acme Tool").length).toBeGreaterThan(0);
  });

  it("reports a mod whose files PM couldn't all delete, as a warning", async () => {
    uninstallMod.mockResolvedValue(gone("acme.tool", { filesRemoved: false }));
    render(view());
    fireEvent.click(await screen.findByRole("button", { name: "Uninstall Acme Tool" }));
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText("What goes");
    fireEvent.click(within(dialog).getByRole("button", { name: "Uninstall" }));
    expect(await modsSection().findByText(/couldn't delete all of its files/)).toBeTruthy();
  });
});

describe("Remove all mods", () => {
  const openConfirm = async () => {
    fireEvent.click(await screen.findByRole("button", { name: "Remove all mods…" }));
    return screen.getByRole("dialog");
  };

  it("asks first, naming every mod and what goes and what stays, and removes nothing yet", async () => {
    render(view());
    const dialog = await openConfirm();
    expect(within(dialog).getByText("Remove all mods?")).toBeTruthy();
    expect(within(dialog).getByText(/This uninstalls 2 mods \(~6 MB\)/)).toBeTruthy();
    expect(within(dialog).getByText("Acme Tool")).toBeTruthy();
    expect(within(dialog).getByText("Beta")).toBeTruthy();
    expect(
      within(dialog).getByText(/Everything they saved goes with them and can't be brought back/),
    ).toBeTruthy();
    expect(
      within(dialog).getByText(/tasks,\s*milestones and notes they made stay in PM/),
    ).toBeTruthy();
    expect(uninstallMod).not.toHaveBeenCalled();
    // Focus starts on Cancel, so Enter on open goes back.
    const cancel = within(dialog).getByRole("button", { name: "Cancel" });
    expect(cancel.hasAttribute("data-autofocus")).toBe(true);
    expect(document.activeElement).toBe(cancel);
  });

  it("leaves everything alone when it is cancelled", async () => {
    render(view());
    const dialog = await openConfirm();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(uninstallMod).not.toHaveBeenCalled();
  });

  it("uninstalls each mod through the runtime, one after another, never deleting what they made", async () => {
    localStorage.setItem("pm.fold.mods.mod.acme.beta", "open");
    const order: string[] = [];
    let release!: () => void;
    uninstallMod.mockImplementation(async (id) => {
      order.push(`start ${id}`);
      if (id === "acme.tool") await new Promise<void>((r) => (release = r));
      order.push(`end ${id}`);
      return gone(id);
    });
    render(view());
    const dialog = await openConfirm();
    listStorageComponents.mockResolvedValue(report());
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove all mods" }));
    await waitFor(() => expect(uninstallMod).toHaveBeenCalledTimes(1));
    // The second waits for the first.
    expect(order).toEqual(["start acme.tool"]);
    await act(async () => release());
    await waitFor(() => expect(uninstallMod).toHaveBeenCalledTimes(2));
    expect(order).toEqual(["start acme.tool", "end acme.tool", "start acme.beta", "end acme.beta"]);
    expect(uninstallMod.mock.calls).toEqual([
      ["acme.tool", false],
      ["acme.beta", false],
    ]);
    expect(await screen.findByText("No mods are installed.")).toBeTruthy();
    expect(modsSection().getByText("Removed 2 mods.")).toBeTruthy();
    // Each leaves the way it does from the Mods tab: this device forgets its folds.
    expect(localStorage.getItem("pm.fold.mods.mod.acme.beta")).toBeNull();
    // One summary, not a line per mod, in the Mods tab's notes too.
    expect(currentModActions().notes.map((n) => n.text)).toEqual(["Removed 2 mods."]);
  });

  it("says which mod stayed and why, and still removes the rest", async () => {
    uninstallMod.mockImplementation(async (id) => {
      if (id === "acme.tool")
        throw { code: "no-store", message: "PM's data isn't open right now." };
      return gone(id);
    });
    render(view());
    const dialog = await openConfirm();
    listStorageComponents.mockResolvedValue(report(tool));
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove all mods" }));
    const note = await modsSection().findByText(/Removed 1 mod\./);
    expect(note.textContent).toContain("“Acme Tool” stayed: PM's data isn't open right now.");
    expect(uninstallMod).toHaveBeenCalledTimes(2);
    expect(screen.getAllByText("Acme Tool").length).toBeGreaterThan(0);
  });

  it("says when files couldn't all be deleted", async () => {
    uninstallMod.mockImplementation(async (id) => gone(id, { filesRemoved: id !== "acme.beta" }));
    render(view());
    const dialog = await openConfirm();
    listStorageComponents.mockResolvedValue(report());
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove all mods" }));
    const note = await modsSection().findByText(/Removed 2 mods\./);
    expect(note.textContent).toContain("PM couldn't delete all the files of “Beta”");
  });

  it("can't be started twice, and the per-mod buttons wait while it runs", async () => {
    let release!: () => void;
    uninstallMod.mockImplementation(async (id) => {
      await new Promise<void>((r) => (release = r));
      return gone(id);
    });
    render(view());
    const dialog = await openConfirm();
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove all mods" }));
    await waitFor(() => expect(uninstallMod).toHaveBeenCalledTimes(1));
    expect(
      (within(dialog).getByRole("button", { name: "Working…" }) as HTMLButtonElement).disabled,
    ).toBe(true);
    expect(
      (screen.getByRole("button", { name: "Uninstall Beta" }) as HTMLButtonElement).disabled,
    ).toBe(true);
    await act(async () => release());
    await waitFor(() => expect(uninstallMod).toHaveBeenCalledTimes(2));
    await act(async () => release());
  });

  it("keeps going when Settings is closed meanwhile (the work lives outside the view)", async () => {
    let release!: () => void;
    uninstallMod.mockImplementation(async (id) => {
      if (id === "acme.tool") await new Promise<void>((r) => (release = r));
      return gone(id);
    });
    const shown = render(view());
    const dialog = await openConfirm();
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove all mods" }));
    await waitFor(() => expect(uninstallMod).toHaveBeenCalledTimes(1));
    shown.unmount();
    await act(async () => release());
    await waitFor(() => expect(uninstallMod).toHaveBeenCalledTimes(2));
    await waitFor(() =>
      expect(currentModActions().notes.map((n) => n.text)).toEqual(["Removed 2 mods."]),
    );
    expect(currentModActions().busy.size).toBe(0);
  });
});
