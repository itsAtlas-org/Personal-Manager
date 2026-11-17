// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Load a test mod" in Settings → Developer (Mods spec §1.15): choosing a folder reads it and loads
// nothing; the stark screen (showing the folder) and the typed id are what load it; each loaded folder
// shows its problem and whether it is watched, with Reload and Unload; the list follows
// `mods://changed`; and everything a mod or a folder supplied is drawn as one plain line.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { resetModActions, setModScreen } from "../../lib/modActions";
import { ModRowsContext } from "../../lib/modRows";
import type {
  ConsentScreen,
  InstallResult,
  ModInfo,
  ModsChanged,
  Reloaded,
  TestModStatus,
} from "../../lib/modsTypes";
import { consentPerm, consentScreen, mod, RLO, RUNNING, standing } from "../mods/modFixtures";

const loadTestMod = vi.fn<(path: string) => Promise<ConsentScreen>>();
const listTestMods = vi.fn<() => Promise<TestModStatus[]>>();
const reloadTestMod = vi.fn<(id: string) => Promise<Reloaded>>();
const unloadTestMod = vi.fn<(id: string) => Promise<void>>();
const installModConsented =
  vi.fn<(t: string, d: Record<string, boolean>, c?: string) => Promise<InstallResult>>();
const openDialog = vi.fn();
const changedHandlers = new Set<(e: ModsChanged) => void>();
const changed = (e: ModsChanged) => changedHandlers.forEach((h) => h(e));

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: (o: unknown) => openDialog(o) }));
vi.mock("../../lib/ipc", () => ({
  loadTestMod: (p: string) => loadTestMod(p),
  listTestMods: () => listTestMods(),
  reloadTestMod: (id: string) => reloadTestMod(id),
  unloadTestMod: (id: string) => unloadTestMod(id),
  installModConsented: (t: string, d: Record<string, boolean>, c?: string) =>
    installModConsented(t, d, c),
  onModsChanged: (h: (e: ModsChanged) => void) => {
    changedHandlers.add(h);
    return Promise.resolve(() => changedHandlers.delete(h));
  },
}));
vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { TestMods } from "./TestMods";

const FOLDER = "/home/dev/acme-tool";

const folderScreen = (extra: Partial<ConsentScreen> = {}): ConsentScreen =>
  consentScreen({
    id: "acme.tool",
    name: "Acme Tool",
    tier: "third",
    kind: "fresh",
    installedVersion: null,
    from: "folder",
    version: "0.1.0",
    token: "tok-folder",
    permissions: [consentPerm("tasks.read", { when: "ask", access: "asks", defaultAnswer: false })],
    unsigned: {
      confirmId: "acme.tool",
      reinstall: false,
      engineSealed: true,
      readOrInternet: null,
      testFolder: FOLDER,
    },
    ...extra,
  });

const installed: InstallResult = {
  id: "acme.tool",
  version: "0.1.0",
  state: "active",
  kind: "fresh",
  pruned: [],
  grantsSaved: true,
  grantsProblem: null,
};

const test = (extra: Partial<TestModStatus> = {}): TestModStatus => ({
  id: "acme.tool",
  folder: FOLDER,
  problem: null,
  watching: true,
  ...extra,
});

function view(rows: readonly ModInfo[] = []) {
  return (
    <ModRowsContext.Provider
      value={{ rows, loaded: true, problem: null, refresh: () => Promise.resolve() }}
    >
      <TestMods />
    </ModRowsContext.Provider>
  );
}

beforeEach(() => {
  for (const f of [
    loadTestMod,
    listTestMods,
    reloadTestMod,
    unloadTestMod,
    installModConsented,
    openDialog,
  ])
    f.mockReset();
  changedHandlers.clear();
  resetModActions();
  localStorage.clear();
  listTestMods.mockResolvedValue([]);
});
afterEach(cleanup);

const choose = () => fireEvent.click(screen.getByRole("button", { name: "Choose a folder…" }));
const typedIdBox = () => screen.getByLabelText(/To go on, type the mod's id/) as HTMLInputElement;

describe("choosing a folder", () => {
  it("asks for a folder, reads it, and loads nothing until the screen's button", async () => {
    openDialog.mockResolvedValue(FOLDER);
    loadTestMod.mockResolvedValue(folderScreen());
    render(view());
    choose();
    expect(openDialog).toHaveBeenCalledWith({ directory: true, multiple: false });
    const dialog = await screen.findByRole("dialog");
    expect(loadTestMod).toHaveBeenCalledWith(FOLDER);
    expect(
      within(dialog).getByRole("heading", { name: "Load “Acme Tool” as a test mod?" }),
    ).toBeTruthy();
    // Which folder, and that it is read where it is and lasts until PM restarts.
    expect(
      within(dialog).getByText(/PM reads the folder \/home\/dev\/acme-tool where it is/),
    ).toBeTruthy();
    expect(within(dialog).getByText(/last until PM restarts/)).toBeTruthy();
    expect(installModConsented).not.toHaveBeenCalled();
  });

  it("loads the folder only once its id is typed, with every permission left at no", async () => {
    openDialog.mockResolvedValue(FOLDER);
    loadTestMod.mockResolvedValue(folderScreen());
    installModConsented.mockResolvedValue(installed);
    render(view());
    choose();
    const dialog = await screen.findByRole("dialog");
    const go = within(dialog).getByRole("button", { name: "Load test mod" }) as HTMLButtonElement;
    expect(go.disabled).toBe(true);
    fireEvent.change(typedIdBox(), { target: { value: "acme.too" } });
    expect(go.disabled).toBe(true);
    fireEvent.change(typedIdBox(), { target: { value: "acme.tool" } });
    expect(go.disabled).toBe(false);

    listTestMods.mockResolvedValue([test()]);
    fireEvent.click(go);
    await waitFor(() =>
      expect(installModConsented).toHaveBeenCalledWith(
        "tok-folder",
        { "tasks.read": false },
        "acme.tool",
      ),
    );
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    // The list is read again, so the new test mod shows without waiting for the event.
    expect(
      await screen.findByText("Watching the folder: saving a file reloads the mod."),
    ).toBeTruthy();
  });

  it("does nothing when the picker is cancelled", async () => {
    openDialog.mockResolvedValue(null);
    render(view());
    choose();
    await waitFor(() => expect(openDialog).toHaveBeenCalled());
    expect(loadTestMod).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("says so when the picker itself fails", async () => {
    openDialog.mockRejectedValue(new Error("no portal"));
    render(view());
    choose();
    expect(await screen.findByText("PM couldn't open the folder picker.")).toBeTruthy();
    expect(loadTestMod).not.toHaveBeenCalled();
  });

  it("shows the backend's own sentence when it won't read the folder, and stages nothing", async () => {
    openDialog.mockResolvedValue(FOLDER);
    loadTestMod.mockRejectedValue({
      code: "dev-mode-off",
      message: "Developer mode is off on this computer.",
    });
    render(view());
    choose();
    expect(await screen.findByText("Developer mode is off on this computer.")).toBeTruthy();
    expect(screen.queryByRole("dialog")).toBeNull();
    // The button is usable again.
    expect(
      (screen.getByRole("button", { name: "Choose a folder…" }) as HTMLButtonElement).disabled,
    ).toBe(false);
  });

  it("shows Reading the folder… and stays disabled while PM reads, so it can't be asked twice", async () => {
    openDialog.mockResolvedValue(FOLDER);
    let read!: (s: ConsentScreen) => void;
    loadTestMod.mockReturnValue(new Promise<ConsentScreen>((r) => (read = r)));
    render(view());
    choose();
    const busy = (await screen.findByRole("button", {
      name: "Reading the folder…",
    })) as HTMLButtonElement;
    expect(busy.disabled).toBe(true);
    await act(async () => read(folderScreen()));
    await screen.findByRole("dialog");
  });

  it("keeps the screen when Settings is closed and opened again (the work lives outside the view)", async () => {
    openDialog.mockResolvedValue(FOLDER);
    loadTestMod.mockResolvedValue(folderScreen());
    const first = render(view());
    choose();
    await screen.findByRole("dialog");
    first.unmount();
    render(view());
    expect(await screen.findByRole("dialog")).toBeTruthy();
    expect(loadTestMod).toHaveBeenCalledTimes(1);
  });
});

describe("a screen that ran out of time", () => {
  it("offers to start again, reading the same folder again", async () => {
    openDialog.mockResolvedValue(FOLDER);
    loadTestMod.mockResolvedValueOnce(folderScreen());
    installModConsented.mockRejectedValue({
      code: "consent-expired",
      message: "That screen ran out of time.",
    });
    render(view());
    choose();
    await screen.findByRole("dialog");
    fireEvent.change(typedIdBox(), { target: { value: "acme.tool" } });
    fireEvent.click(screen.getByRole("button", { name: "Load test mod" }));
    expect(await screen.findByText("This screen ran out of time")).toBeTruthy();

    loadTestMod.mockResolvedValueOnce(folderScreen({ token: "tok-folder-2" }));
    fireEvent.click(screen.getByRole("button", { name: "Start again" }));
    await waitFor(() => expect(loadTestMod).toHaveBeenCalledTimes(2));
    expect(loadTestMod).toHaveBeenLastCalledWith(FOLDER);
    expect(
      await screen.findByRole("heading", { name: "Load “Acme Tool” as a test mod?" }),
    ).toBeTruthy();
  });
});

describe("the loaded folders", () => {
  it("says there are none", async () => {
    render(view());
    expect(await screen.findByText("No test mods are loaded.")).toBeTruthy();
  });

  it("shows each one with its name and standing, its folder and whether it is watched", async () => {
    listTestMods.mockResolvedValue([
      test(),
      test({ id: "acme.other", folder: "/home/dev/other", watching: false }),
    ]);
    render(
      view([
        mod({ id: "acme.tool", name: "Acme Tool", installedFrom: "folder", state: RUNNING }),
        mod({
          id: "acme.other",
          name: "Other",
          installedFrom: "folder",
          state: standing("paused-frozen", { text: "Other stopped responding." }),
        }),
      ]),
    );
    const list = await screen.findByRole("list", { name: "Test mods" });
    const [tool, other] = within(list).getAllByRole("listitem");
    expect(within(tool).getByText("Acme Tool")).toBeTruthy();
    expect(within(tool).getByText("Running.")).toBeTruthy();
    expect(within(tool).getByText(FOLDER)).toBeTruthy();
    expect(
      within(tool).getByText("Watching the folder: saving a file reloads the mod."),
    ).toBeTruthy();
    expect(within(other).getByText("Other stopped responding.")).toBeTruthy();
    expect(
      within(other).getByText("Not watching the folder: press Reload after you save a change."),
    ).toBeTruthy();
  });

  it("falls back to the id when the shared rows don't have the mod yet", async () => {
    listTestMods.mockResolvedValue([test()]);
    render(view([]));
    const item = within(await screen.findByRole("list", { name: "Test mods" })).getByRole(
      "listitem",
    );
    expect(item.textContent).toContain("acme.tool");
  });

  it("says why the last reload didn't take, and that the old version keeps running", async () => {
    listTestMods.mockResolvedValue([
      test({ problem: "It now asks for more than you allowed: reading your tasks." }),
    ]);
    render(view());
    const note = await screen.findByText(
      /The last reload didn't take, so the old version keeps running/,
    );
    expect(note.textContent).toContain(
      "It now asks for more than you allowed: reading your tasks.",
    );
  });

  it("says when the list can't be read, in the backend's words", async () => {
    listTestMods.mockRejectedValue({
      code: "no-store",
      message: "PM's data isn't open right now.",
    });
    render(view());
    expect(await screen.findByText("PM's data isn't open right now.")).toBeTruthy();
  });

  it("follows mods://changed (a load, a reload, an unload, an uninstall)", async () => {
    render(view());
    await screen.findByText("No test mods are loaded.");
    await waitFor(() => expect(changedHandlers.size).toBe(1));
    listTestMods.mockResolvedValue([test()]);
    act(() => changed({ cause: "test-mod" }));
    expect(await screen.findByRole("list", { name: "Test mods" })).toBeTruthy();
    listTestMods.mockResolvedValue([]);
    act(() => changed({ cause: "uninstalled" }));
    expect(await screen.findByText("No test mods are loaded.")).toBeTruthy();
  });

  it("drops the answer to an older question when a newer one has been asked", async () => {
    let first!: (l: TestModStatus[]) => void;
    listTestMods.mockReturnValueOnce(new Promise<TestModStatus[]>((r) => (first = r)));
    render(view());
    await waitFor(() => expect(changedHandlers.size).toBe(1));
    listTestMods.mockResolvedValueOnce([test({ id: "acme.new" })]);
    act(() => changed({ cause: "test-mod" }));
    await screen.findByRole("list", { name: "Test mods" });
    // The slow first read finally answers with what is no longer true.
    await act(async () => first([]));
    expect(screen.getByRole("list", { name: "Test mods" }).textContent).toContain("acme.new");
    expect(screen.queryByText("No test mods are loaded.")).toBeNull();
  });

  it("stops listening when it is closed", async () => {
    const { unmount } = render(view());
    await waitFor(() => expect(changedHandlers.size).toBe(1));
    unmount();
    await waitFor(() => expect(changedHandlers.size).toBe(0));
  });

  it("draws what a mod or a folder supplied as one plain line", async () => {
    listTestMods.mockResolvedValue([
      test({
        id: `acme.tool`,
        folder: `/home/dev/${RLO}evil`,
        problem: `bad\nSAFE: PM says ok`,
      }),
    ]);
    render(
      view([mod({ id: "acme.tool", name: `${RLO}Tool\nPM: all fine`, installedFrom: "folder" })]),
    );
    const item = within(await screen.findByRole("list", { name: "Test mods" })).getByRole(
      "listitem",
    );
    expect(item.textContent).not.toContain(RLO);
    expect(item.textContent).not.toContain("\n");
    expect(item.textContent).toContain("Tool PM: all fine");
    expect(item.textContent).toContain("/home/dev/ evil");
    expect(item.textContent).toContain("bad SAFE: PM says ok");
  });

  it("draws an id the same way, though one with such characters should never reach it", async () => {
    // No row knows this id yet, so the id is what names the test mod.
    listTestMods.mockResolvedValue([test({ id: `acme.${RLO}tool` })]);
    render(view());
    const item = within(await screen.findByRole("list", { name: "Test mods" })).getByRole(
      "listitem",
    );
    expect(item.textContent).not.toContain(RLO);
    expect(item.textContent).toContain("acme. tool");
  });
});

describe("Reload", () => {
  const rowButton = (name: string) =>
    within(screen.getByRole("list", { name: "Test mods" })).getByRole("button", { name });

  it("reads the folder again and says what happened", async () => {
    listTestMods.mockResolvedValue([test()]);
    reloadTestMod.mockResolvedValue({ changed: true, version: "0.2.0" });
    render(view());
    await screen.findByRole("list", { name: "Test mods" });
    fireEvent.click(rowButton("Reload acme.tool"));
    expect(reloadTestMod).toHaveBeenCalledWith("acme.tool");
    expect(await screen.findByText("Reloaded: it is now version 0.2.0.")).toBeTruthy();
    // The reason a reload didn't take lives on the list, so it is read again.
    await waitFor(() => expect(listTestMods).toHaveBeenCalledTimes(2));
  });

  it("says when nothing had changed", async () => {
    listTestMods.mockResolvedValue([test()]);
    reloadTestMod.mockResolvedValue({ changed: false, version: "0.1.0" });
    render(view());
    await screen.findByRole("list", { name: "Test mods" });
    fireEvent.click(rowButton("Reload acme.tool"));
    expect(await screen.findByText("Nothing had changed, so nothing restarted.")).toBeTruthy();
  });

  it("shows the refusal, reads the list again so the reason shows, and forgets the last good note", async () => {
    listTestMods.mockResolvedValue([test()]);
    reloadTestMod.mockResolvedValueOnce({ changed: true, version: "0.2.0" });
    render(view());
    await screen.findByRole("list", { name: "Test mods" });
    fireEvent.click(rowButton("Reload acme.tool"));
    await screen.findByText("Reloaded: it is now version 0.2.0.");

    reloadTestMod.mockRejectedValueOnce({
      code: "grant-refused",
      message: "It asks for more than you allowed.",
    });
    listTestMods.mockResolvedValue([test({ problem: "It asks for more than you allowed." })]);
    fireEvent.click(rowButton("Reload acme.tool"));
    const alerts = await screen.findAllByRole("alert");
    expect(alerts.some((a) => a.textContent === "It asks for more than you allowed.")).toBe(true);
    expect(screen.queryByText("Reloaded: it is now version 0.2.0.")).toBeNull();
    expect(await screen.findByText(/The last reload didn't take/)).toBeTruthy();
  });

  it("can't be pressed twice while it is working", async () => {
    listTestMods.mockResolvedValue([test()]);
    let done!: (r: Reloaded) => void;
    reloadTestMod.mockReturnValue(new Promise<Reloaded>((r) => (done = r)));
    render(view());
    await screen.findByRole("list", { name: "Test mods" });
    fireEvent.click(rowButton("Reload acme.tool"));
    await waitFor(() =>
      expect((rowButton("Reload acme.tool") as HTMLButtonElement).disabled).toBe(true),
    );
    expect((rowButton("Unload acme.tool") as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(rowButton("Reload acme.tool"));
    expect(reloadTestMod).toHaveBeenCalledTimes(1);
    await act(async () => done({ changed: false, version: "0.1.0" }));
  });
});

describe("Unload", () => {
  it("ends the test mod, forgets its tab and folds on this device, and reads the list again", async () => {
    localStorage.setItem("pm.fold.mods.mod.acme.tool", "open");
    localStorage.setItem("pm.fold.mods.mod.acme.tool:permissions", "closed");
    localStorage.setItem("pm.fold.mods.mod.acme.toolbox", "open");
    listTestMods.mockResolvedValue([test()]);
    unloadTestMod.mockResolvedValue();
    render(view());
    const list = await screen.findByRole("list", { name: "Test mods" });
    listTestMods.mockResolvedValue([]);
    fireEvent.click(within(list).getByRole("button", { name: "Unload acme.tool" }));
    expect(unloadTestMod).toHaveBeenCalledWith("acme.tool");
    expect(await screen.findByText("No test mods are loaded.")).toBeTruthy();
    expect(localStorage.getItem("pm.fold.mods.mod.acme.tool")).toBeNull();
    expect(localStorage.getItem("pm.fold.mods.mod.acme.tool:permissions")).toBeNull();
    // Another mod whose id merely starts the same keeps its fold.
    expect(localStorage.getItem("pm.fold.mods.mod.acme.toolbox")).toBe("open");
  });

  it("says why when it can't, and keeps the mod's folds", async () => {
    localStorage.setItem("pm.fold.mods.mod.acme.tool", "open");
    listTestMods.mockResolvedValue([test()]);
    unloadTestMod.mockRejectedValue({
      code: "no-store",
      message: "PM's data isn't open right now.",
    });
    render(view());
    const list = await screen.findByRole("list", { name: "Test mods" });
    fireEvent.click(within(list).getByRole("button", { name: "Unload acme.tool" }));
    expect(await within(list).findByText("PM's data isn't open right now.")).toBeTruthy();
    expect(localStorage.getItem("pm.fold.mods.mod.acme.tool")).toBe("open");
  });
});

describe("a folder's screen is this section's, not the Mods tab's", () => {
  it("draws a folder screen staged from anywhere", async () => {
    render(view());
    act(() => setModScreen("acme.tool", folderScreen()));
    expect(
      await screen.findByRole("heading", { name: "Load “Acme Tool” as a test mod?" }),
    ).toBeTruthy();
  });

  it("does not draw a file's screen", async () => {
    render(view());
    act(() =>
      setModScreen(
        "acme.raw",
        folderScreen({
          id: "acme.raw",
          from: "file",
          unsigned: { ...folderScreen().unsigned!, testFolder: null },
        }),
      ),
    );
    await screen.findByText("No test mods are loaded.");
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
