// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The install flows through the Mods tab as a person meets them: the catalogue entry, the file, the
// screen each one brings, the answers, the install, and what survives the tab being switched away
// (the view unmounts; the backend's work doesn't).

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { resetModActions } from "../../lib/modActions";
import { ModRowsContext } from "../../lib/modRows";
import type { Catalogue, ConsentScreen, InstallResult, ModsChanged } from "../../lib/modsTypes";
import { consentPerm, consentScreen, safety } from "./modFixtures";

const listCatalogue = vi.fn();
const installFromCatalogue = vi.fn();
const installModFile = vi.fn();
const installUnsignedFile = vi.fn();
const installModConsented = vi.fn();
const devModeState = vi.fn();
const openDialog = vi.fn();
const changedHandlers = new Set<(e: ModsChanged) => void>();
const changed = (e: ModsChanged) => changedHandlers.forEach((h) => h(e));

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: (o: unknown) => openDialog(o) }));
vi.mock("../../lib/ipc", () => ({
  listCatalogue: () => listCatalogue(),
  installFromCatalogue: (id: string) => installFromCatalogue(id),
  installModFile: (p: string) => installModFile(p),
  installUnsignedFile: (p: string) => installUnsignedFile(p),
  installModConsented: (t: string, d: Record<string, boolean>, c?: string) =>
    installModConsented(t, d, c),
  devModeState: () => devModeState(),
  modsDisclaimerState: () => Promise.resolve({ accepted: true, problem: null }),
  modsSafetyStatus: () => Promise.resolve(safety()),
  modIcon: () => Promise.reject(new Error("none")),
  onModsChanged: (h: (e: ModsChanged) => void) => {
    changedHandlers.add(h);
    return Promise.resolve(() => changedHandlers.delete(h));
  },
  onModsNotice: () => Promise.resolve(() => {}),
}));
vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { ModsView } from "./ModsView";

const refresh = vi.fn(() => Promise.resolve());
const view = () => (
  <ModRowsContext.Provider value={{ rows: [], loaded: true, problem: null, refresh }}>
    <ModsView />
  </ModRowsContext.Provider>
);

const catalogue = (): Catalogue => ({
  usable: true,
  unavailable: null,
  entries: [
    {
      id: "atlas.map",
      name: "Map",
      description: "Your documents on a map.",
      tier: "first",
      author: "Atlas",
      repo: "itsAtlas-org/PM-Mods",
      commit: "0123456789abcdef",
      version: "1.0.0",
      permissions: [{ id: "documents.meta", class: "read" }],
      installed: false,
      installedVersion: null,
      updateAvailable: false,
      needsNewerPm: false,
      apiRetired: false,
      blocked: null,
      installable: true,
      whyNot: null,
    },
  ],
});

const done = (extra: Partial<InstallResult> = {}): InstallResult => ({
  id: "atlas.map",
  version: "1.0.0",
  state: "active",
  kind: "fresh",
  pruned: [],
  grantsSaved: true,
  grantsProblem: null,
  ...extra,
});

const mapScreen = (extra: Partial<ConsentScreen> = {}) =>
  consentScreen({
    id: "atlas.map",
    name: "Map",
    tier: "first",
    author: "Atlas",
    kind: "fresh",
    installedVersion: null,
    from: "catalogue",
    version: "1.0.0",
    token: "tok-map",
    permissions: [consentPerm("documents.meta")],
    ...extra,
  });

const rawScreen = (extra: Partial<ConsentScreen> = {}) =>
  consentScreen({
    id: "acme.raw",
    name: "Raw",
    tier: "third",
    kind: "fresh",
    installedVersion: null,
    from: "file",
    token: "tok-raw",
    permissions: [consentPerm("tasks.read", { when: "ask", access: "asks", defaultAnswer: false })],
    unsigned: {
      confirmId: "acme.raw",
      reinstall: false,
      engineSealed: true,
      readOrInternet: null,
      testFolder: null,
    },
    ...extra,
  });

beforeEach(() => {
  vi.clearAllMocks();
  resetModActions();
  changedHandlers.clear();
  listCatalogue.mockResolvedValue(catalogue());
});
afterEach(cleanup);

describe("from the catalogue", () => {
  it("Install, then the screen, then Install again: two steps, and the second sends the answers", async () => {
    installFromCatalogue.mockResolvedValue(mapScreen());
    installModConsented.mockResolvedValue(done());
    render(view());
    fireEvent.click(await screen.findByRole("button", { name: "Install Map" }));
    const dialog = await screen.findByRole("dialog", { name: "Install Map?" });
    expect(installModConsented).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Install" }));
    await waitFor(() =>
      expect(installModConsented).toHaveBeenCalledWith(
        "tok-map",
        { "documents.meta": true },
        undefined,
      ),
    );
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(refresh).toHaveBeenCalled();
  });

  it("reads the catalogue again when mods change, so an installed entry loses its button", async () => {
    render(view());
    await screen.findByRole("button", { name: "Install Map" });
    await waitFor(() => expect(changedHandlers.size).toBeGreaterThan(0));
    const next = catalogue();
    next.entries[0] = { ...next.entries[0], installed: true, installedVersion: "1.0.0" };
    listCatalogue.mockResolvedValue(next);
    act(() => changed({ cause: "installed" }));
    expect(await screen.findByText("Installed, version 1.0.0")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Install Map" })).toBeNull();
  });

  it("an install in flight survives a tab switch: coming back shows Installing, then it is done", async () => {
    installFromCatalogue.mockResolvedValue(mapScreen());
    let finish!: (r: InstallResult) => void;
    installModConsented.mockReturnValue(
      new Promise<InstallResult>((res) => {
        finish = res;
      }),
    );
    const first = render(view());
    fireEvent.click(await screen.findByRole("button", { name: "Install Map" }));
    const dialog = await screen.findByRole("dialog", { name: "Install Map?" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Install" }));
    // The dialog's button and the entry's both say so.
    expect(
      (await screen.findAllByRole("button", { name: "Installing…", hidden: true })).length,
    ).toBe(2);
    first.unmount();
    render(view());
    // Back on the tab: the entry and the dialog both say it is still going.
    expect((await screen.findAllByText("Installing…")).length).toBeGreaterThan(0);
    expect(
      (screen.getAllByRole("button", { name: "Installing…" })[0] as HTMLButtonElement).disabled,
    ).toBe(true);
    await act(async () => finish(done()));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(screen.queryByText("Installing…")).toBeNull();
  });

  it("an expired screen says so; Start again downloads again and the new token is the one used", async () => {
    installFromCatalogue.mockResolvedValueOnce(mapScreen());
    installModConsented.mockRejectedValueOnce({ code: "consent-expired", message: "Old." });
    render(view());
    fireEvent.click(await screen.findByRole("button", { name: "Install Map" }));
    fireEvent.click(
      within(await screen.findByRole("dialog", { name: "Install Map?" })).getByRole("button", {
        name: "Install",
      }),
    );
    await screen.findByRole("dialog", { name: "This screen ran out of time" });
    installFromCatalogue.mockResolvedValueOnce(mapScreen({ token: "tok-map-2" }));
    fireEvent.click(screen.getByRole("button", { name: "Start again" }));
    const again = await screen.findByRole("dialog", { name: "Install Map?" });
    installModConsented.mockResolvedValue(done());
    fireEvent.click(within(again).getByRole("button", { name: "Install" }));
    await waitFor(() =>
      expect(installModConsented).toHaveBeenLastCalledWith(
        "tok-map-2",
        { "documents.meta": true },
        undefined,
      ),
    );
    expect(installFromCatalogue).toHaveBeenCalledTimes(2);
  });

  it("a refusal of the install is said once, in the dialog that is open, not again on the entry", async () => {
    installFromCatalogue.mockResolvedValue(mapScreen());
    installModConsented.mockRejectedValue({ code: "refused", message: "This one can't go in." });
    render(view());
    fireEvent.click(await screen.findByRole("button", { name: "Install Map" }));
    const dialog = await screen.findByRole("dialog", { name: "Install Map?" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Install" }));
    await within(dialog).findByText("This one can't go in.");
    expect(screen.getAllByText("This one can't go in.").length).toBe(1);
    // Cancelling forgets it: the entry doesn't keep nagging about a screen that is gone.
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(screen.queryByText("This one can't go in.")).toBeNull();
  });

  it("a download that fails is said on the entry, with the button back", async () => {
    installFromCatalogue.mockRejectedValue({ code: "offline", message: "No connection." });
    render(view());
    fireEvent.click(await screen.findByRole("button", { name: "Install Map" }));
    expect(await screen.findByText("No connection.")).toBeTruthy();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(
      (screen.getByRole("button", { name: "Install Map" }) as HTMLButtonElement).disabled,
    ).toBe(false);
  });
});

describe("from a file", () => {
  it("a signed file: read, the tiered screen, then the install", async () => {
    openDialog.mockResolvedValue("/x/map.pmmod");
    installModFile.mockResolvedValue(mapScreen({ from: "file" }));
    installModConsented.mockResolvedValue(done());
    render(view());
    fireEvent.click(await screen.findByRole("button", { name: "Choose a mod file…" }));
    const dialog = await screen.findByRole("dialog", { name: "Install Map?" });
    expect(dialog.textContent).toContain("From a file you chose.");
    fireEvent.click(within(dialog).getByRole("button", { name: "Install" }));
    await waitFor(() => expect(installModConsented).toHaveBeenCalledTimes(1));
    expect(installModFile).toHaveBeenCalledWith("/x/map.pmmod");
  });

  it("an unsigned file, developer mode off: the section says so; no screen, nothing installed", async () => {
    openDialog.mockResolvedValue("/x/raw.pmmod");
    installModFile.mockRejectedValue({ code: "unsigned", message: "Not signed." });
    devModeState.mockResolvedValue({ on: false, problem: null });
    render(view());
    fireEvent.click(await screen.findByRole("button", { name: "Choose a mod file…" }));
    expect(await screen.findByText(/switch it on in Settings, under Developer/)).toBeTruthy();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(installUnsignedFile).not.toHaveBeenCalled();
    expect(installModConsented).not.toHaveBeenCalled();
  });

  it("an unsigned file, developer mode on: the stark screen, the typed id, then the install with it", async () => {
    openDialog.mockResolvedValue("/x/raw.pmmod");
    installModFile.mockRejectedValue({ code: "unsigned", message: "Not signed." });
    devModeState.mockResolvedValue({ on: true, problem: null });
    installUnsignedFile.mockResolvedValue(rawScreen());
    installModConsented.mockResolvedValue(done({ id: "acme.raw" }));
    render(view());
    fireEvent.click(await screen.findByRole("button", { name: "Choose a mod file…" }));
    const dialog = await screen.findByRole("dialog", {
      name: "Install Raw, which nobody has checked?",
    });
    expect(dialog.textContent).toContain("Nobody has checked this mod.");
    const go = within(dialog).getByRole("button", { name: "Install" }) as HTMLButtonElement;
    expect(go.disabled).toBe(true);
    fireEvent.change(within(dialog).getByLabelText(/To go on, type the mod's id/), {
      target: { value: "acme.raw" },
    });
    expect(go.disabled).toBe(false);
    fireEvent.click(go);
    await waitFor(() =>
      expect(installModConsented).toHaveBeenCalledWith(
        "tok-raw",
        { "tasks.read": false },
        "acme.raw",
      ),
    );
  });

  it("a stark screen kept while the tab was away is there on return, with its id still to type", async () => {
    openDialog.mockResolvedValue("/x/raw.pmmod");
    installModFile.mockRejectedValue({ code: "unsigned", message: "Not signed." });
    devModeState.mockResolvedValue({ on: true, problem: null });
    installUnsignedFile.mockResolvedValue(rawScreen());
    const first = render(view());
    fireEvent.click(await screen.findByRole("button", { name: "Choose a mod file…" }));
    await screen.findByRole("dialog", { name: /which nobody has checked/ });
    first.unmount();
    render(view());
    const dialog = await screen.findByRole("dialog", { name: /which nobody has checked/ });
    expect(
      (within(dialog).getByRole("button", { name: "Install" }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });

  it("the Install from file section sits last, after Available", async () => {
    render(view());
    await screen.findByRole("heading", { name: "Install from file" });
    const names = [...document.querySelectorAll("[data-mods-section]")].map((s) =>
      s.getAttribute("data-mods-section"),
    );
    expect(names).toEqual(["installed", "available", "from-file"]);
  });
});
