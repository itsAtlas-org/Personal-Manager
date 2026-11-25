// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Mods tab as a whole (spec §1.13): the disclaimer first, the strips, the installed rows and the
// dialogs they open, re-read on every `mods://changed`, and honest across tab switches: the view
// unmounts on every switch, so what is in flight and what was staged live outside it.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { readFold, writeFold } from "../../lib/foldPrefs";
import { resetModActions } from "../../lib/modActions";
import { ModRowsContext } from "../../lib/modRows";
import { ModRowsProvider } from "../../lib/modRowsProvider";
import type {
  ModInfo,
  ModNotice,
  ModUninstalled,
  ModsChanged,
  ModsChangedCause,
} from "../../lib/modsTypes";
import { consentPerm, consentScreen, mod, perm, safety, standing } from "./modFixtures";

const listMods = vi.fn();
const modsDisclaimerState = vi.fn();
const modsSafetyStatus = vi.fn();
const consentMod = vi.fn();
const approveUpdate = vi.fn();
const uninstallPreview = vi.fn();
const uninstallMod = vi.fn();
const pauseMod = vi.fn();
const listCatalogue = vi.fn();
// Three readers listen to `mods://changed` (the shared rows, the disclaimer, the strips) and two
// kinds to the notice, so an event goes to every one that subscribed.
const changedHandlers = new Set<(e: ModsChanged) => void>();
const noticeHandlers = new Set<(n: ModNotice) => void>();
const changed = (e: ModsChanged) => changedHandlers.forEach((h) => h(e));
const notice = (n: ModNotice) => noticeHandlers.forEach((h) => h(n));

vi.mock("../../lib/ipc", () => ({
  listMods: () => listMods(),
  modsDisclaimerState: () => modsDisclaimerState(),
  modsSafetyStatus: () => modsSafetyStatus(),
  consentMod: (id: string, a: Record<string, boolean>) => consentMod(id, a),
  approveUpdate: (id: string) => approveUpdate(id),
  uninstallPreview: (id: string) => uninstallPreview(id),
  uninstallMod: (id: string, also: boolean) => uninstallMod(id, also),
  pauseMod: (id: string) => pauseMod(id),
  listCatalogue: () => listCatalogue(),
  modIcon: () => Promise.reject(new Error("none")),
  dismissModResumed: () => Promise.resolve(),
  onModsChanged: (h: (e: ModsChanged) => void) => {
    changedHandlers.add(h);
    return Promise.resolve(() => changedHandlers.delete(h));
  },
  onModsNotice: (h: (n: ModNotice) => void) => {
    noticeHandlers.add(h);
    return Promise.resolve(() => noticeHandlers.delete(h));
  },
}));
vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));
vi.mock("../../lib/capabilities", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useDevMode: () => ({ devMode: false, setDevMode: () => {} }),
}));

import { ModsView } from "./ModsView";

const refresh = vi.fn(() => Promise.resolve());

/** The view over fixed rows (the shared read's own behaviour is tested with the provider below). */
function view(rows: ModInfo[], extra: { loaded?: boolean; problem?: string | null } = {}) {
  return (
    <ModRowsContext.Provider
      value={{ rows, loaded: extra.loaded ?? true, problem: extra.problem ?? null, refresh }}
    >
      <ModsView />
    </ModRowsContext.Provider>
  );
}
const draw = (rows: ModInfo[], extra = {}) => render(view(rows, extra));

beforeEach(() => {
  vi.clearAllMocks();
  resetModActions();
  localStorage.clear();
  changedHandlers.clear();
  noticeHandlers.clear();
  modsDisclaimerState.mockResolvedValue({ accepted: true, problem: null });
  modsSafetyStatus.mockResolvedValue(safety());
  listCatalogue.mockResolvedValue({ entries: [], usable: true, unavailable: null });
});
afterEach(cleanup);

describe("first open", () => {
  it("shows the disclaimer and none of the sections until it is accepted", async () => {
    modsDisclaimerState.mockResolvedValue({ accepted: false, problem: null });
    draw([mod()]);
    expect(await screen.findByRole("heading", { name: "Before you use mods" })).toBeTruthy();
    expect(screen.queryByRole("heading", { name: "Installed" })).toBeNull();
    expect(screen.queryByRole("heading", { name: "Available" })).toBeNull();
    expect(screen.queryByRole("heading", { name: "Install from file" })).toBeNull();
    expect(screen.queryByText("Acme Tool")).toBeNull();
  });

  it("still shows the safety list's strips before the disclaimer: they are about mods already there", async () => {
    modsDisclaimerState.mockResolvedValue({ accepted: false, problem: null });
    modsSafetyStatus.mockResolvedValue(
      safety({
        notices: [{ kind: "removed", id: "a.b", name: "Acme", version: "1", reason: "Bad." }],
      }),
    );
    draw([]);
    expect(await screen.findByText(/“Acme” was removed by PM's safety list/)).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Before you use mods" })).toBeTruthy();
  });

  it("once accepted, shows the three sections in order", async () => {
    draw([mod()]);
    await screen.findByRole("heading", { name: "Installed" });
    const names = [...document.querySelectorAll("[data-mods-section]")].map((s) =>
      s.getAttribute("data-mods-section"),
    );
    expect(names).toEqual(["installed", "available", "from-file"]);
  });
});

describe("the installed section", () => {
  it("leaves out a mod PM is still installing: it is no row, so it has nothing to pause or remove", async () => {
    draw([mod(), mod({ id: "atlas.map", name: "Map", pending: true })]);
    expect(await screen.findByRole("heading", { name: "Acme Tool" })).toBeTruthy();
    expect(screen.queryByRole("heading", { name: "Map" })).toBeNull();
    expect(screen.getByText("1 mod installed")).toBeTruthy();
    cleanup();
    draw([mod({ id: "atlas.map", name: "Map", pending: true })]);
    expect(await screen.findByText("No mods are installed.")).toBeTruthy();
    expect(screen.getByText("No mods installed")).toBeTruthy();
  });

  it("has one row per mod, and says when there are none", async () => {
    draw([mod(), mod({ id: "acme.other", name: "Other" })]);
    expect(await screen.findByRole("heading", { name: "Acme Tool" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Other" })).toBeTruthy();
    cleanup();
    draw([]);
    expect(await screen.findByText("No mods are installed.")).toBeTruthy();
  });

  it("says it is loading, and says why when the list can't be read, with Try again", async () => {
    draw([], { loaded: false });
    expect(await screen.findAllByText("Loading…")).toBeTruthy();
    cleanup();
    draw([], { loaded: false, problem: "PM is locked." });
    expect((await screen.findAllByText(/PM is locked\./)).length).toBeGreaterThan(0);
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(refresh).toHaveBeenCalled();
  });
});

describe("refreshing on mods://changed", () => {
  const CAUSES: ModsChangedCause[] = [
    "recovered",
    "installed",
    "uninstalled",
    "paused",
    "resumed",
    "consented",
    "safety",
    "updated",
    "loaded",
    "decided",
    "dev-mode",
    "test-mod",
    "auto-installed",
    "secret",
    "ai-cap",
  ];

  it.each(CAUSES)("reads the rows and the strips again after %s", async (cause) => {
    listMods.mockResolvedValueOnce([mod()]);
    render(
      <ModRowsProvider>
        <ModsView />
      </ModRowsProvider>,
    );
    expect(await screen.findByText("Running.")).toBeTruthy();
    await waitFor(() => expect(changedHandlers.size).toBeGreaterThan(0));
    listMods.mockResolvedValue([
      mod({ state: standing("paused-by-you", { text: "You paused this mod.", fix: "resume" }) }),
    ]);
    modsSafetyStatus.mockResolvedValue(
      safety({
        notices: [{ kind: "updateWaiting", id: "acme.tool", name: "Acme Tool", version: "9" }],
      }),
    );
    await act(async () => changed({ cause }));
    expect(await screen.findByText("You paused this mod.")).toBeTruthy();
    expect(await screen.findByText(/has an update, version 9/)).toBeTruthy();
  });

  it("reads the strips again on the notice cue alone", async () => {
    draw([mod()]);
    await screen.findByRole("heading", { name: "Installed" });
    await waitFor(() => expect(noticeHandlers.size).toBeGreaterThan(0));
    modsSafetyStatus.mockResolvedValue(
      safety({
        notices: [
          {
            kind: "paused",
            id: "acme.tool",
            name: "Acme Tool",
            version: "1",
            reason: "Bad",
            fixedIn: null,
          },
        ],
      }),
    );
    await act(async () => notice({ kind: "resumed", id: "x", name: "x", version: "1" }));
    expect(
      await screen.findByText(/“Acme Tool” is paused while we fix a problem: Bad/),
    ).toBeTruthy();
  });
});

describe("turning on a mod PM installed", () => {
  const waiting = () =>
    mod({
      consented: false,
      state: standing("waiting-for-you", {
        fix: "turn-on",
        text: "Waiting for you to turn it on.",
      }),
      permissions: [perm("documents.meta"), perm("ai", { class: "money" })],
    });

  it("Review and turn on opens the first-open screen in a dialog, even for a mod with no tab", async () => {
    draw([{ ...waiting(), navTab: null }]);
    fireEvent.click(await screen.findByRole("button", { name: "Review and turn on Acme Tool" }));
    const dialog = screen.getByRole("dialog", { name: "Turn on “Acme Tool”?" });
    expect(dialog.textContent).toContain("About mods");
    expect(dialog.textContent).toContain("What this mod asks to do");
    expect(consentMod).not.toHaveBeenCalled();
  });

  it("Turn on sends an answer for each install-time permission, reads the rows, and closes", async () => {
    consentMod.mockResolvedValue({ state: "active", reason: null, serving: true });
    const { rerender } = draw([waiting()]);
    fireEvent.click(await screen.findByRole("button", { name: "Review and turn on Acme Tool" }));
    const dialog = screen.getByRole("dialog");
    fireEvent.click(within(dialog).getByRole("checkbox", { name: /Allow: Ask an AI model/ }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Turn on" }));
    await waitFor(() =>
      expect(consentMod).toHaveBeenCalledWith("acme.tool", { "documents.meta": true, ai: false }),
    );
    await waitFor(() => expect(refresh).toHaveBeenCalled());
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    // The row is read again and now runs: nothing is left open.
    rerender(view([mod()]));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("Not now closes it without turning anything on", async () => {
    draw([waiting()]);
    fireEvent.click(await screen.findByRole("button", { name: "Review and turn on Acme Tool" }));
    fireEvent.click(screen.getByRole("button", { name: "Not now" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(consentMod).not.toHaveBeenCalled();
  });

  it("closes by itself when the row stops waiting (turned on elsewhere)", async () => {
    const { rerender } = draw([waiting()]);
    fireEvent.click(await screen.findByRole("button", { name: "Review and turn on Acme Tool" }));
    expect(screen.getByRole("dialog")).toBeTruthy();
    rerender(view([mod()]));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("stays open while Turning on is in flight, so a refusal that arrives is seen, not lost", async () => {
    // Closing the dialog would put the refusal on a screen nobody is looking at, and the row would
    // go on saying "Waiting for you" with nothing to say why.
    let refuse!: (e: unknown) => void;
    consentMod.mockReturnValue(
      new Promise((_, reject) => {
        refuse = reject;
      }),
    );
    draw([waiting()]);
    fireEvent.click(await screen.findByRole("button", { name: "Review and turn on Acme Tool" }));
    const dialog = screen.getByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Turn on" }));
    await waitFor(() =>
      expect(within(dialog).getByRole("button", { name: "Turning on…" })).toBeTruthy(),
    );
    // Escape, the scrim and Not now all leave it be.
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.getByRole("dialog")).toBeTruthy();
    const notNow = within(dialog).getByRole("button", { name: "Not now" }) as HTMLButtonElement;
    expect(notNow.disabled).toBe(true);
    fireEvent.click(notNow);
    expect(screen.getByRole("dialog")).toBeTruthy();
    // The refusal lands in the dialog, once, and the dialog can be closed again.
    await act(async () => refuse({ code: "grant-refused", message: "PM couldn't turn that on." }));
    expect(await within(dialog).findByText("PM couldn't turn that on.")).toBeTruthy();
    expect(screen.getAllByText("PM couldn't turn that on.")).toHaveLength(1);
    expect(
      (within(dialog).getByRole("button", { name: "Not now" }) as HTMLButtonElement).disabled,
    ).toBe(false);
    fireEvent.click(within(dialog).getByRole("button", { name: "Not now" }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("says the backend's refusal in the dialog and lets the person try again", async () => {
    consentMod.mockRejectedValue({ code: "locked", message: "PM is locked." });
    draw([waiting()]);
    fireEvent.click(await screen.findByRole("button", { name: "Review and turn on Acme Tool" }));
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Turn on" }));
    expect(await screen.findByText("PM is locked.")).toBeTruthy();
    expect(
      (
        within(screen.getByRole("dialog")).getByRole("button", {
          name: "Turn on",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(false);
  });
});

describe("approving an update", () => {
  const waitingUpdate = () =>
    mod({
      update: {
        version: "1.3.0",
        kind: "approval",
        widenings: ["permission:ai"],
        needsApi: null,
        text: "Version 1.3.0 asks for more than you allowed.",
      },
    });
  const screenFor = () =>
    consentScreen({
      permissions: [consentPerm("ai", { isNew: true, class: "money" })],
      widening: ["permission:ai"],
    });

  it("Review update shows the widening consent the backend returned", async () => {
    approveUpdate.mockResolvedValue(screenFor());
    draw([waitingUpdate()]);
    fireEvent.click(await screen.findByRole("button", { name: "Review update for Acme Tool" }));
    const dialog = await screen.findByRole("dialog", {
      name: "Update “Acme Tool” to version 1.3.0?",
    });
    expect(dialog.textContent).toContain("This update asks for more than you allowed before:");
    expect(dialog.textContent).toContain(
      "Ask an AI model to do things for it, which can cost money",
    );
  });

  it("the staged screen waits for the person when the view goes away and comes back", async () => {
    approveUpdate.mockResolvedValue(screenFor());
    const first = draw([waitingUpdate()]);
    fireEvent.click(await screen.findByRole("button", { name: "Review update for Acme Tool" }));
    await screen.findByRole("dialog");
    first.unmount();
    draw([waitingUpdate()]);
    expect(
      await screen.findByRole("dialog", { name: "Update “Acme Tool” to version 1.3.0?" }),
    ).toBeTruthy();
  });

  it("a screen that arrives after the view left is there on return", async () => {
    let arrive!: (s: ReturnType<typeof screenFor>) => void;
    approveUpdate.mockReturnValue(
      new Promise((res) => {
        arrive = res;
      }),
    );
    const first = draw([waitingUpdate()]);
    fireEvent.click(await screen.findByRole("button", { name: "Review update for Acme Tool" }));
    first.unmount();
    await act(async () => arrive(screenFor()));
    draw([waitingUpdate()]);
    expect(await screen.findByRole("dialog")).toBeTruthy();
  });
});

describe("uninstalling from a row", () => {
  const preview = {
    id: "acme.tool",
    name: "Acme Tool",
    leaves: {
      contentBytes: 10,
      cacheBytes: 0,
      documentDataBytes: 0,
      filesBytes: 0,
      navTab: "Acme Tool",
      calendarOverlay: null,
    },
    stays: { tasks: 0, milestones: 0, documents: 0 },
    removedBySafetyList: false,
    notes: null,
  };
  const gone: ModUninstalled = {
    id: "acme.tool",
    filesRemoved: true,
    giftsDeleted: null,
    prune: { tab: "mod:acme.tool", foldPrefix: "mods.mod.acme.tool" },
  };

  it("opens the preview first; nothing is uninstalled until the dialog's own button", async () => {
    uninstallPreview.mockResolvedValue(preview);
    draw([mod()]);
    const detailsToggle = await screen.findByRole("button", { name: /Details/ });
    fireEvent.click(detailsToggle);
    fireEvent.click(screen.getByRole("button", { name: "Uninstall Acme Tool" }));
    expect(await screen.findByRole("dialog", { name: "Uninstall “Acme Tool”?" })).toBeTruthy();
    expect(uninstallMod).not.toHaveBeenCalled();
  });

  it("removes the mod's folds, says so, and the note outlives the view", async () => {
    uninstallPreview.mockResolvedValue(preview);
    uninstallMod.mockResolvedValue(gone);
    writeFold("mods.mod.acme.tool", true);
    writeFold("mods.mod.acme.tool:permissions", true);
    const first = draw([mod()]);
    fireEvent.click(await screen.findByRole("button", { name: /Details/ }));
    fireEvent.click(screen.getByRole("button", { name: "Uninstall Acme Tool" }));
    fireEvent.click(await screen.findByRole("button", { name: "Uninstall" }));
    expect(await screen.findByText("“Acme Tool” was uninstalled.")).toBeTruthy();
    expect(readFold("mods.mod.acme.tool")).toBeNull();
    expect(readFold("mods.mod.acme.tool:permissions")).toBeNull();
    // Leave and come back: the note is still there, and Dismiss clears it.
    first.unmount();
    draw([]);
    expect(await screen.findByText("“Acme Tool” was uninstalled.")).toBeTruthy();
    fireEvent.click(screen.getAllByRole("button", { name: "Dismiss" })[0]);
    expect(screen.queryByText("“Acme Tool” was uninstalled.")).toBeNull();
  });

  it("a view that comes back mid-uninstall sees the row busy and can't start another action", async () => {
    uninstallPreview.mockResolvedValue(preview);
    let finish!: (d: ModUninstalled) => void;
    uninstallMod.mockReturnValue(
      new Promise<ModUninstalled>((res) => {
        finish = res;
      }),
    );
    const first = draw([mod()]);
    fireEvent.click(await screen.findByRole("button", { name: /Details/ }));
    fireEvent.click(screen.getByRole("button", { name: "Uninstall Acme Tool" }));
    fireEvent.click(await screen.findByRole("button", { name: "Uninstall" }));
    await screen.findByRole("button", { name: "Uninstalling…" });
    first.unmount();
    draw([mod()]);
    expect(await screen.findByText("Uninstalling…")).toBeTruthy();
    expect(
      (screen.getByRole("button", { name: "Pause Acme Tool" }) as HTMLButtonElement).disabled,
    ).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Pause Acme Tool" }));
    expect(pauseMod).not.toHaveBeenCalled();
    await act(async () => finish(gone));
    expect(await screen.findByText("“Acme Tool” was uninstalled.")).toBeTruthy();
    await waitFor(() => expect(screen.queryByText("Uninstalling…")).toBeNull());
  });
});
