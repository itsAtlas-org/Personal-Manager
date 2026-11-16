// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A mod's own tab (Mods spec §1.17, D17): the sealed frame for a mod that can run, and a screen PM
// draws for every standing that can't, each with its own sentence and the one action that brings the
// mod back. The frame's session must end with the tab, with the backend's `session.closed`, and when
// the store closes under it.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useState, type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { HELP } from "../../lib/help";
import { ModRowsContext, type ModRows } from "../../lib/modRows";
import type { ModInfo, ModPermission, ModStanding, ModsChanged } from "../../lib/modsTypes";
import { LRI, RLO } from "./modFixtures";

const modFrameOpen = vi.fn();
const modFrameClose = vi.fn();
const consentMod = vi.fn();
const resumeMod = vi.fn();
const repairMod = vi.fn();
let faulted: (() => void) | null = null;
let changed: ((e: ModsChanged) => void) | null = null;

vi.mock("../../lib/ipc", () => ({
  modsHeartbeat: () => Promise.resolve(),
  modFrameOpen: (modId: string, onEvent: (message: unknown) => void) =>
    modFrameOpen(modId, onEvent),
  modFrameClose: (session: string) => modFrameClose(session),
  modCall: () => Promise.resolve({ kind: "value", value: null }),
  modEventsAck: () => Promise.resolve(),
  modThemeChanged: () => Promise.resolve(),
  consentMod: (id: string, answers: Record<string, boolean>) => consentMod(id, answers),
  resumeMod: (id: string) => resumeMod(id),
  repairMod: (id: string) => repairMod(id),
  onVaultFault: (handler: () => void) => {
    faulted = handler;
    return Promise.resolve(() => {});
  },
  onModsChanged: (handler: (e: ModsChanged) => void) => {
    changed = handler;
    return Promise.resolve(() => {});
  },
}));

vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({
    system: "slate",
    mode: "dark",
    modePref: "system",
    modeSource: "system",
    accent: "mono",
    colorblind: false,
    depth: "standard",
    autoLocation: "",
  }),
}));
vi.mock("../../lib/reader", () => ({ useReader: () => ({ openReaderById: () => {} }) }));

import { ModTab } from "./ModTab";

const ENTRY = "pmmod://localhost/acme.tool/app/index.html?entry=" + "a".repeat(64);

const RUNNING: ModStanding = {
  state: "active",
  reason: null,
  kind: "running",
  text: "Running.",
  fix: "none",
  running: true,
};

function standing(kind: ModStanding["kind"], extra: Partial<ModStanding> = {}): ModStanding {
  return { state: "paused", reason: null, kind, text: "", fix: "none", running: false, ...extra };
}

const perm = (id: string, extra: Partial<ModPermission> = {}): ModPermission => ({
  id,
  why: `why ${id}`,
  class: "read",
  when: "install",
  decision: null,
  access: "off",
  offBecause: null,
  ...extra,
});

function mod(extra: Partial<ModInfo> = {}): ModInfo {
  return {
    id: "acme.tool",
    name: "Acme Tool",
    description: "Does a thing.",
    tier: "second",
    author: "Acme",
    repo: "acme/tool",
    commit: "abc",
    version: "1.2.0",
    installedFrom: "catalogue",
    signed: true,
    consented: true,
    state: RUNNING,
    permissions: [],
    internetHosts: [],
    navTab: "Acme Tool",
    calendarOverlay: null,
    hasIcon: false,
    update: null,
    resumed: null,
    installedAt: "",
    updatedAt: "",
    ...extra,
  };
}

const onOpenMods = vi.fn();
const onOpenSettings = vi.fn();
const refresh = vi.fn(() => Promise.resolve());

function rowsValue(rows: ModInfo[], extra: Partial<ModRows> = {}): ModRows {
  return { rows, loaded: true, problem: null, refresh, ...extra };
}

function Tab({ rows, extra }: { rows: ModInfo[]; extra?: Partial<ModRows> }): ReactNode {
  return (
    <ModRowsContext.Provider value={rowsValue(rows, extra)}>
      <ModTab modId="acme.tool" onOpenMods={onOpenMods} onOpenSettings={onOpenSettings} />
    </ModRowsContext.Provider>
  );
}

const iframe = () => document.querySelector("iframe");
const channel = () =>
  modFrameOpen.mock.calls[modFrameOpen.mock.calls.length - 1][1] as (message: unknown) => void;
const goodbye = (reason: string, seq = 1) =>
  act(() => channel()({ seq, name: "session.closed", payload: { reason } }));

beforeEach(() => {
  vi.clearAllMocks();
  faulted = null;
  changed = null;
  let n = 0;
  modFrameOpen.mockImplementation(() => {
    n += 1;
    return Promise.resolve({ entryUrl: ENTRY, session: `session-${n}` });
  });
  modFrameClose.mockResolvedValue(undefined);
  consentMod.mockResolvedValue({ state: "active", reason: null, serving: true });
  resumeMod.mockResolvedValue({ state: "active", reason: null, serving: true });
  repairMod.mockResolvedValue({ id: "acme.tool", version: "1.2.0", state: "active" });
});
afterEach(cleanup);

describe("a mod that can run", () => {
  it("opens the sealed frame under the band, for the mod's id", async () => {
    render(<Tab rows={[mod()]} />);
    await waitFor(() => expect(iframe()).not.toBeNull());
    expect(modFrameOpen).toHaveBeenCalledWith("acme.tool", expect.any(Function));
    expect(screen.getByLabelText("Mod: Acme Tool")).toBeTruthy();
    expect(screen.getByText("Reviewed by Atlas")).toBeTruthy();
  });

  it("ends the frame's session when the tab goes away (a tab switch unmounts it)", async () => {
    const view = render(<Tab rows={[mod()]} />);
    await waitFor(() => expect(iframe()).not.toBeNull());
    view.unmount();
    expect(modFrameClose).toHaveBeenCalledWith("session-1");
  });

  it("ends the session and shows PM's panel when the row says the mod is paused", async () => {
    const view = render(<Tab rows={[mod()]} />);
    await waitFor(() => expect(iframe()).not.toBeNull());
    view.rerender(
      <Tab
        rows={[
          mod({
            state: standing("paused-by-you", {
              reason: "user",
              text: "Paused by you.",
              fix: "resume",
            }),
          }),
        ]}
      />,
    );
    expect(iframe()).toBeNull();
    expect(modFrameClose).toHaveBeenCalledWith("session-1");
    expect(screen.getByText("Paused by you.")).toBeTruthy();
  });

  it("does not start a frame for a row that is active but not turned on", async () => {
    render(<Tab rows={[mod({ state: { ...RUNNING, running: false } })]} />);
    await act(async () => {});
    expect(iframe()).toBeNull();
    expect(modFrameOpen).not.toHaveBeenCalled();
  });
});

describe("the backend ending the session", () => {
  it("replaces the frame with a panel for a pause, closes the session and reads the rows again", async () => {
    render(<Tab rows={[mod()]} />);
    await waitFor(() => expect(iframe()).not.toBeNull());
    goodbye("paused");
    expect(iframe()).toBeNull();
    expect(screen.getByText("This mod was paused.")).toBeTruthy();
    expect(modFrameClose).toHaveBeenCalledWith("session-1");
    expect(refresh).toHaveBeenCalled();
    // A paused mod isn't offered a reopen the backend would refuse.
    expect(screen.queryByRole("button", { name: "Open it again" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Open the Mods tab" }));
    expect(onOpenMods).toHaveBeenCalledOnce();
  });

  it("says a removed mod was removed", async () => {
    render(<Tab rows={[mod()]} />);
    await waitFor(() => expect(iframe()).not.toBeNull());
    goodbye("uninstalled");
    expect(screen.getByText("This mod was removed.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Open it again" })).toBeNull();
  });

  it("opens a fresh frame, and so a fresh session, when the mod was replaced by an update", async () => {
    render(<Tab rows={[mod()]} />);
    await waitFor(() => expect(iframe()).not.toBeNull());
    goodbye("replaced");
    await waitFor(() => expect(modFrameOpen).toHaveBeenCalledTimes(2));
    expect(modFrameClose).toHaveBeenCalledWith("session-1");
    await waitFor(() => expect(iframe()).not.toBeNull());
    expect(screen.queryByText("This mod's session ended.")).toBeNull();
  });

  it("offers to open it again after any other end, and that starts a new session", async () => {
    render(<Tab rows={[mod()]} />);
    await waitFor(() => expect(iframe()).not.toBeNull());
    goodbye("something-new");
    expect(screen.getByText("This mod's session ended.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Open it again" }));
    await waitFor(() => expect(modFrameOpen).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(iframe()).not.toBeNull());
  });

  it("treats a reason it can't read as an ordinary end", async () => {
    render(<Tab rows={[mod()]} />);
    await waitFor(() => expect(iframe()).not.toBeNull());
    act(() => channel()({ seq: 1, name: "session.closed", payload: { reason: { x: 1 } } }));
    expect(screen.getByText("This mod's session ended.")).toBeTruthy();
  });
});

describe("a store that closes under the frame", () => {
  it("clears the frame on the vault fault, which sends no goodbye, and brings it back when the store is read again", async () => {
    render(<Tab rows={[mod()]} />);
    await waitFor(() => expect(iframe()).not.toBeNull());
    act(() => faulted?.());
    expect(iframe()).toBeNull();
    expect(modFrameClose).toHaveBeenCalledWith("session-1");
    expect(screen.getByText(/can't reach your vault/)).toBeTruthy();

    // An unrelated change doesn't bring it back; the store opening (`loaded`) does.
    act(() => changed?.({ cause: "paused" }));
    expect(iframe()).toBeNull();
    act(() => changed?.({ cause: "loaded" }));
    await waitFor(() => expect(iframe()).not.toBeNull());
    expect(modFrameOpen).toHaveBeenCalledTimes(2);
  });
});

describe("a mod waiting for its first open (D17)", () => {
  const waiting = (permissions: ModPermission[] = []) =>
    mod({
      tier: "first",
      consented: false,
      state: { ...RUNNING, kind: "waiting-for-you", fix: "turn-on", running: false },
      permissions,
    });

  it("draws one PM screen with the disclaimer and every permission, and starts no frame", async () => {
    render(
      <Tab
        rows={[
          waiting([
            perm("documents.meta", { why: "To draw the map." }),
            perm("tasks.read"),
            perm("storage", { when: "auto", class: "local", access: "allowed" }),
            perm("ai", { when: "first-use", class: "money", access: "asks" }),
          ]),
        ]}
      />,
    );
    await act(async () => {});
    expect(modFrameOpen).not.toHaveBeenCalled();
    expect(iframe()).toBeNull();
    expect(screen.getByRole("heading", { name: "Turn on Acme Tool?" })).toBeTruthy();
    expect(screen.getByText("What PM checks.")).toBeTruthy();
    expect(screen.getByText("What PM can't check.")).toBeTruthy();
    // PM's words for each permission, in the backend's order, with the maker's reason beside them.
    const items = within(screen.getByRole("list")).getAllByRole("listitem");
    expect(items).toHaveLength(4);
    expect(items[0].textContent).toContain("See your documents' titles");
    expect(items[0].textContent).toContain("The maker says: “To draw the map.”");
    expect(items[1].textContent).toContain("Read your tasks");
    expect(items[2].textContent).toContain("Always on.");
    expect(items[3].textContent).toContain("PM asks you the first time");
  });

  it("turns on with an answer for every permission asked at install, and nothing else", async () => {
    render(
      <Tab
        rows={[
          waiting([
            perm("documents.meta"),
            perm("tasks.read"),
            perm("storage", { when: "auto" }),
            perm("ai", { when: "first-use", class: "money" }),
            perm("calendar.read", { when: "install-then-first-use" }),
          ]),
        ]}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Turn on" }));
    await waitFor(() => expect(consentMod).toHaveBeenCalledTimes(1));
    expect(consentMod).toHaveBeenCalledWith("acme.tool", {
      "documents.meta": true,
      "tasks.read": true,
    });
    await waitFor(() => expect(refresh).toHaveBeenCalled());
  });

  it("sends a no for a permission the user turned off", async () => {
    render(<Tab rows={[waiting([perm("documents.meta"), perm("tasks.read")])]} />);
    fireEvent.click(screen.getByRole("checkbox", { name: /Allow: Read your tasks/ }));
    fireEvent.click(screen.getByRole("button", { name: "Turn on" }));
    await waitFor(() => expect(consentMod).toHaveBeenCalled());
    expect(consentMod).toHaveBeenCalledWith("acme.tool", {
      "documents.meta": true,
      "tasks.read": false,
    });
  });

  it("starts a permission the user once refused as a no", async () => {
    render(<Tab rows={[waiting([perm("tasks.read", { decision: "denied" })])]} />);
    fireEvent.click(screen.getByRole("button", { name: "Turn on" }));
    await waitFor(() => expect(consentMod).toHaveBeenCalled());
    expect(consentMod).toHaveBeenCalledWith("acme.tool", { "tasks.read": false });
  });

  it("can't be pressed twice while it works, and the frame starts only once the row says so", async () => {
    let finish: (v: unknown) => void = () => {};
    consentMod.mockReturnValue(new Promise((resolve) => (finish = resolve)));

    function Harness() {
      const [rows, setRows] = useState([waiting([perm("tasks.read")])]);
      const value = rowsValue(rows, {
        refresh: () => {
          setRows([mod({ tier: "first" })]);
          return Promise.resolve();
        },
      });
      return (
        <ModRowsContext.Provider value={value}>
          <ModTab modId="acme.tool" onOpenMods={onOpenMods} onOpenSettings={onOpenSettings} />
        </ModRowsContext.Provider>
      );
    }
    render(<Harness />);
    const button = screen.getByRole("button", { name: "Turn on" });
    fireEvent.click(button);
    expect(screen.getByRole("button", { name: "Turning on…" }).hasAttribute("disabled")).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Turning on…" }));
    expect(consentMod).toHaveBeenCalledTimes(1);
    expect(modFrameOpen).not.toHaveBeenCalled();
    await act(async () => finish({ state: "active", reason: null, serving: true }));
    await waitFor(() => expect(iframe()).not.toBeNull());
    expect(screen.queryByRole("heading", { name: "Turn on Acme Tool?" })).toBeNull();
  });

  it("says why when the backend refuses, keeps the screen, and lets the user try again", async () => {
    consentMod.mockRejectedValueOnce({ code: "grant-refused", message: "That isn't allowed." });
    render(<Tab rows={[waiting([perm("tasks.read")])]} />);
    fireEvent.click(screen.getByRole("button", { name: "Turn on" }));
    expect(await screen.findByText("That isn't allowed.")).toBeTruthy();
    expect(refresh).not.toHaveBeenCalled();
    expect(iframe()).toBeNull();
    const again = screen.getByRole("button", { name: "Turn on" });
    expect(again.hasAttribute("disabled")).toBe(false);
    fireEvent.click(again);
    await waitFor(() => expect(consentMod).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.queryByText("That isn't allowed.")).toBeNull());
  });

  it("lists the sites a mod may reach as plain text", async () => {
    render(
      <Tab
        rows={[
          {
            ...waiting([perm("internet", { class: "egress" })]),
            internetHosts: ["api.example.com"],
          },
        ]}
      />,
    );
    expect(screen.getByText("Sites it may reach: api.example.com")).toBeTruthy();
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("warns about a mod nobody has checked, and not about one somebody has", () => {
    const view = render(<Tab rows={[{ ...waiting(), tier: "third", signed: false }]} />);
    expect(screen.getByText("Third party")).toBeTruthy();
    expect(screen.getByText(/Nobody has checked this mod/)).toBeTruthy();
    view.rerender(<Tab rows={[waiting()]} />);
    expect(screen.queryByText(/Nobody has checked this mod/)).toBeNull();
  });
});

describe("a mod that can't run", () => {
  it("shows an installing placeholder with the backend's sentence, and no frame", async () => {
    render(
      <Tab
        rows={[
          mod({
            state: standing("installing", {
              state: "installing",
              text: "Installing. If it doesn't finish, PM tries again at the next safety list check.",
            }),
          }),
        ]}
      />,
    );
    await act(async () => {});
    expect(screen.getByRole("heading", { name: "Installing Acme Tool" })).toBeTruthy();
    expect(screen.getByText(/PM tries again at the next safety list check/)).toBeTruthy();
    expect(modFrameOpen).not.toHaveBeenCalled();
  });

  it("keeps the installing placeholder across a tab switch, because the row is what holds it", async () => {
    const rows = [
      mod({ state: standing("installing", { state: "installing", text: "Installing." }) }),
    ];
    const first = render(<Tab rows={rows} />);
    first.unmount();
    render(<Tab rows={rows} />);
    expect(screen.getByRole("heading", { name: "Installing Acme Tool" })).toBeTruthy();
  });

  it("resumes a mod the user paused, then reads the rows again", async () => {
    render(
      <Tab
        rows={[
          mod({
            state: standing("paused-by-you", {
              reason: "user",
              text: "Paused by you.",
              fix: "resume",
            }),
          }),
        ]}
      />,
    );
    expect(screen.getByText("Paused by you.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Resume" }));
    await waitFor(() => expect(resumeMod).toHaveBeenCalledWith("acme.tool"));
    await waitFor(() => expect(refresh).toHaveBeenCalled());
  });

  it("offers Resume for a frozen mod, in the backend's sentence", () => {
    render(
      <Tab
        rows={[
          mod({
            state: standing("paused-frozen", {
              reason: "frozen",
              text: "Acme Tool stopped responding, and PM paused it so the rest of PM keeps working.",
              fix: "resume",
            }),
          }),
        ]}
      />,
    );
    expect(screen.getByText(/stopped responding/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Resume" })).toBeTruthy();
  });

  it("shows a refused resume's own words and reads the rows again", async () => {
    resumeMod.mockRejectedValueOnce({
      code: "safety-list",
      message: "The safety list still names it.",
    });
    render(
      <Tab
        rows={[
          mod({
            state: standing("paused-by-you", {
              reason: "user",
              text: "Paused by you.",
              fix: "resume",
            }),
          }),
        ]}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Resume" }));
    expect(await screen.findByText("The safety list still names it.")).toBeTruthy();
    expect(refresh).toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Resume" }).hasAttribute("disabled")).toBe(false);
  });

  it("offers no Resume for a pause the safety list owns, only the way to the Mods tab", () => {
    render(
      <Tab
        rows={[
          mod({
            state: standing("paused-by-safety-list", {
              reason: "blacklist",
              text: "Paused while we fix a problem: It loses notes.",
            }),
          }),
        ]}
      />,
    );
    expect(screen.getByText(/It loses notes/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Resume" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Open the Mods tab" }));
    expect(onOpenMods).toHaveBeenCalledOnce();
  });

  it("points a mod paused for developer mode at Settings, where that switch is", () => {
    render(
      <Tab
        rows={[
          mod({
            tier: "third",
            state: standing("paused-dev-mode", {
              reason: "dev-mode",
              text: "Paused because developer mode is off. Turn developer mode on to use it again.",
            }),
          }),
        ]}
      />,
    );
    expect(screen.getByText(/developer mode is off/)).toBeTruthy();
    expect(screen.getByText(/under Developer/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Resume" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Open Settings" }));
    expect(onOpenSettings).toHaveBeenCalledOnce();
  });

  it("offers Repair for a mod whose files are missing, and reads the rows after", async () => {
    render(
      <Tab
        rows={[
          mod({
            state: standing("files-missing", {
              state: "parked",
              reason: "files-missing",
              text: "Install Acme Tool to get this back.",
              fix: "repair",
            }),
          }),
        ]}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Repair" }));
    await waitFor(() => expect(repairMod).toHaveBeenCalledWith("acme.tool"));
    await waitFor(() => expect(refresh).toHaveBeenCalled());
  });

  it("sends a removed mod to the Mods tab, where it is uninstalled", () => {
    render(
      <Tab
        rows={[
          mod({
            state: standing("removed-by-safety-list", {
              reason: "blacklist-removed",
              text: "PM's safety list removed it: It sends your documents elsewhere.",
              fix: "uninstall",
            }),
          }),
        ]}
      />,
    );
    expect(screen.getByText(/removed it/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Open the Mods tab" }));
    expect(onOpenMods).toHaveBeenCalledOnce();
  });

  it("says a mod that isn't in the list isn't installed", () => {
    render(<Tab rows={[]} />);
    expect(screen.getByRole("heading", { name: "This mod isn't installed" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Open the Mods tab" }));
    expect(onOpenMods).toHaveBeenCalledOnce();
    expect(modFrameOpen).not.toHaveBeenCalled();
  });

  it("waits for the first read, and offers to try again when it failed", () => {
    const view = render(<Tab rows={[]} extra={{ loaded: false }} />);
    expect(screen.getByText("Loading…")).toBeTruthy();
    view.rerender(<Tab rows={[]} extra={{ loaded: false, problem: "PM is locked." }} />);
    expect(screen.getByText(/PM is locked\./)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(refresh).toHaveBeenCalled();
  });
});

describe("what a mod supplied is plain text", () => {
  const hostile = '<img src=x onerror="alert(1)"><b>bold</b>';

  it("never becomes markup on the first-open screen, in its name, description, author or reasons", () => {
    render(
      <Tab
        rows={[
          mod({
            name: hostile,
            description: hostile,
            author: hostile,
            consented: false,
            state: { ...RUNNING, kind: "waiting-for-you", fix: "turn-on", running: false },
            permissions: [perm("tasks.read", { why: hostile })],
            internetHosts: [hostile],
          }),
        ]}
      />,
    );
    expect(document.querySelector("img")).toBeNull();
    expect(document.querySelector("b")).toBeNull();
    expect(
      screen.getAllByText(new RegExp(hostile.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"))).length,
    ).toBeGreaterThan(0);
  });

  it("loses direction-changing characters and line breaks on the first-open screen", () => {
    render(
      <Tab
        rows={[
          mod({
            name: `Ac${RLO}me`,
            description: "one\nPM says: fine",
            author: `Mal${LRI}lory`,
            consented: false,
            state: { ...RUNNING, kind: "waiting-for-you", fix: "turn-on", running: false },
            permissions: [perm("tasks.read", { why: `to\nmap${RLO}` })],
            internetHosts: [`h${RLO}.example`],
          }),
        ]}
      />,
    );
    const text = document.body.textContent ?? "";
    expect(text).toContain("Turn on Ac me?");
    expect(text).toContain("one PM says: fine");
    expect(text).toContain("Made by Mal lory");
    expect(text).toContain("The maker says: “to map”");
    expect(text).toContain("Sites it may reach: h .example");
    expect(text).not.toContain(RLO);
    expect(text).not.toContain(LRI);
  });

  it("never becomes markup on a status panel", () => {
    render(
      <Tab
        rows={[
          mod({
            name: hostile,
            state: standing("paused-by-you", { reason: "user", text: hostile, fix: "resume" }),
          }),
        ]}
      />,
    );
    expect(document.querySelector("img")).toBeNull();
    expect(document.querySelector("b")).toBeNull();
  });
});

describe("help", () => {
  it("has an entry for every help id the Mods surfaces name", () => {
    const sources = Object.entries(
      import.meta.glob<string>(["./*.tsx", "../../lib/nav.ts"], {
        query: "?raw",
        import: "default",
        eager: true,
      }),
    ).filter(([path]) => !/\.test\.tsx$/.test(path));
    const named = new Set<string>();
    for (const [, text] of sources) {
      for (const m of text.matchAll(/(?:data-help|helpId|HELP_ID)\W{1,4}"([a-z][a-z0-9-]*)"/g)) {
        named.add(m[1]);
      }
    }
    expect([...named].sort()).toEqual(
      expect.arrayContaining([
        "mod-first-open",
        "mod-tab-status",
        "mods-view",
        "nav-mod-tab",
        "nav-mods",
      ]),
    );
    for (const id of named) expect(HELP[id], id).toBeDefined();
  });
});
