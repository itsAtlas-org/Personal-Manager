// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// One installed mod in the Mods tab (spec §1.13): what it is, how it stands (in the backend's words),
// the one button that brings it back, Pause, each permission with its state and an Allow / Turn off,
// the update waiting, Show tab for a hidden tab, and Uninstall. Everything the mod supplied is text.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { readFold } from "../../lib/foldPrefs";
import { resetModActions, useModActions } from "../../lib/modActions";
import { ModRowsContext } from "../../lib/modRows";
import { NAV_PLACEMENT_KEY, parsePlacement, writeNavPlacement } from "../../lib/nav";
import type { ModInfo } from "../../lib/modsTypes";
import { consentScreen, LRI, mod, perm, RLO, standing } from "./modFixtures";

const resumeMod = vi.fn();
const pauseMod = vi.fn();
const repairMod = vi.fn();
const approveUpdate = vi.fn();
const modPermissionDecide = vi.fn();
const modIcon = vi.fn();

vi.mock("../../lib/ipc", () => ({
  resumeMod: (id: string) => resumeMod(id),
  pauseMod: (id: string) => pauseMod(id),
  repairMod: (id: string) => repairMod(id),
  approveUpdate: (id: string) => approveUpdate(id),
  modPermissionDecide: (id: string, p: string, g: boolean) => modPermissionDecide(id, p, g),
  modIcon: (id: string) => modIcon(id),
}));
vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));
vi.mock("../../lib/capabilities", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useDevMode: () => ({ devMode: false, setDevMode: () => {} }),
}));

import { ModRow } from "./ModRow";

const refresh = vi.fn(() => Promise.resolve());
const onUninstall = vi.fn();
const onTurnOn = vi.fn();

function Row({ row, quiet = false }: { row: ModInfo; quiet?: boolean }): ReactNode {
  return (
    <ModRowsContext.Provider value={{ rows: [row], loaded: true, problem: null, refresh }}>
      <ul>
        <ModRow
          mod={row}
          refresh={refresh}
          onUninstall={onUninstall}
          onTurnOn={onTurnOn}
          quiet={quiet}
        />
      </ul>
    </ModRowsContext.Provider>
  );
}

const draw = (row: ModInfo, quiet = false) => render(<Row row={row} quiet={quiet} />);

/** Open the row's Details fold (closed for a mod that is fine). */
function openDetails() {
  const toggle = screen.getByRole("button", { name: /Details/ });
  if (toggle.getAttribute("aria-expanded") !== "true") fireEvent.click(toggle);
}

beforeEach(() => {
  vi.clearAllMocks();
  resetModActions();
  localStorage.clear();
  vi.stubGlobal("URL", {
    ...URL,
    createObjectURL: vi.fn(() => "blob:icon"),
    revokeObjectURL: vi.fn(),
  });
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("what the row says", () => {
  it("names the mod, its version and its tier in PM's words", () => {
    draw(mod({ tier: "second" }));
    expect(screen.getByRole("heading", { name: "Acme Tool" })).toBeTruthy();
    expect(screen.getByText("version 1.2.0")).toBeTruthy();
    expect(screen.getByText("Reviewed by Atlas")).toBeTruthy();
    cleanup();
    draw(mod({ tier: "first" }));
    expect(screen.getByText("First party")).toBeTruthy();
    cleanup();
    draw(mod({ tier: "third", signed: false, installedFrom: "file" }));
    expect(screen.getByText("Third party")).toBeTruthy();
  });

  it("marks a folder test mod, and says it goes at restart", () => {
    draw(mod({ installedFrom: "folder", tier: "third", signed: false }));
    expect(screen.getByText("Test")).toBeTruthy();
    openDetails();
    expect(
      screen.getByText(/A test mod loaded from a folder\. It stays only until PM restarts\./),
    ).toBeTruthy();
    cleanup();
    draw(mod());
    expect(screen.queryByText("Test")).toBeNull();
  });

  it("shows the standing's label and the backend's sentence", () => {
    draw(
      mod({
        state: standing("paused-by-you", { text: "You paused this mod.", fix: "resume" }),
      }),
    );
    expect(screen.getByText("Paused")).toBeTruthy();
    expect(screen.getByText("You paused this mod.")).toBeTruthy();
  });

  it.each([
    ["running", "On"],
    ["waiting-for-you", "Waiting for you"],
    ["paused-by-you", "Paused"],
    ["paused-frozen", "Stopped responding"],
    ["paused-by-safety-list", "Paused by the safety list"],
    ["removed-by-safety-list", "Removed by the safety list"],
    ["paused-dev-mode", "Developer mode is off"],
    ["files-missing", "Files missing"],
    ["files-changed", "Files changed"],
    ["record-unreadable", "Can't be read"],
    ["api-unsupported", "Needs another PM"],
    ["parked", "Parked"],
    ["installing", "Installing"],
    ["paused-other", "Paused"],
  ] as const)("says %s as “%s”", (kind, label) => {
    draw(mod({ state: standing(kind, { text: `sentence for ${kind}` }) }));
    expect(screen.getByText(label)).toBeTruthy();
    expect(screen.getByText(`sentence for ${kind}`)).toBeTruthy();
  });

  it("says “Not running” for a standing PM has no word for, never nothing", () => {
    draw(mod({ state: standing("from-the-future" as never, { text: "x" }) }));
    expect(screen.getByText("Not running")).toBeTruthy();
  });

  it("draws everything the mod supplied as plain text, never markup or a link", () => {
    const evil = '<img src=x onerror="alert(1)"><a href="https://evil.example">click</a>';
    const { container } = draw(
      mod({
        name: evil,
        description: evil,
        author: evil,
        repo: evil,
        permissions: [perm("documents.meta", { why: evil })],
        state: standing("paused-by-you", { text: "x" }),
      }),
    );
    openDetails();
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("a")).toBeNull();
    expect(screen.getByRole("heading").textContent).toBe(evil);
  });

  it("takes direction-changing characters and line breaks out of everything the mod supplied", () => {
    const { container } = draw(
      mod({
        name: `Ac${RLO}me`,
        description: "line one\nPM says: trust me",
        author: `Mal${LRI}lory`,
        repo: `a${RLO}/b`,
        internetHosts: [`h${RLO}.example`],
        permissions: [perm("documents.meta", { why: `to\nmap${RLO}` })],
        state: standing("paused-by-you", { text: `Paused${RLO}.` }),
      }),
    );
    openDetails();
    expect(screen.getByRole("heading").textContent).toBe("Ac me");
    expect(container.textContent).toContain("line one PM says: trust me");
    expect(container.textContent).toContain("Made by Mal lory · a /b");
    expect(container.textContent).toContain("Sites it may reach: h .example");
    expect(container.textContent).toContain("The maker says: “to map”");
    expect(container.textContent).not.toContain(RLO);
    expect(container.textContent).not.toContain(LRI);
  });

  it("shows the first letter of the name when the mod has no icon", () => {
    draw(mod({ name: "zeta", hasIcon: false }));
    expect(screen.getByText("Z")).toBeTruthy();
    expect(modIcon).not.toHaveBeenCalled();
  });

  it("draws the mod's icon from the backend's bytes, and falls back when they can't be read", async () => {
    modIcon.mockResolvedValue({ mediaType: "image/png", data: btoa("png-bytes") });
    const { container } = draw(mod({ hasIcon: true }));
    await waitFor(() =>
      expect(container.querySelector("img")?.getAttribute("src")).toBe("blob:icon"),
    );
    expect(container.querySelector("img")?.getAttribute("alt")).toBe("");
    cleanup();
    modIcon.mockRejectedValue({ code: "refused", message: "no" });
    const again = draw(mod({ hasIcon: true, name: "Acme" }));
    await waitFor(() => expect(modIcon).toHaveBeenCalledTimes(2));
    expect(again.container.querySelector("img")).toBeNull();
    expect(screen.getByText("A")).toBeTruthy();
  });
});

describe("the one button for each standing", () => {
  it("waiting for you: Review and turn on opens the first-open screen", () => {
    draw(mod({ consented: false, state: standing("waiting-for-you", { fix: "turn-on" }) }));
    fireEvent.click(screen.getByRole("button", { name: "Review and turn on" }));
    expect(onTurnOn).toHaveBeenCalledWith("acme.tool");
    expect(pauseMod).not.toHaveBeenCalled();
  });

  it("paused by you: Resume resumes, and the rows are read again", async () => {
    resumeMod.mockResolvedValue({ state: "active", reason: null, serving: true });
    draw(mod({ state: standing("paused-by-you", { fix: "resume" }) }));
    fireEvent.click(screen.getByRole("button", { name: "Resume" }));
    await waitFor(() => expect(refresh).toHaveBeenCalled());
    expect(resumeMod).toHaveBeenCalledWith("acme.tool");
  });

  it("Resume shows the backend's refusal sentence when the list or developer mode holds it", async () => {
    resumeMod.mockRejectedValue({
      code: "safety-list",
      message: "PM's safety list still names this version.",
    });
    draw(mod({ state: standing("paused-by-you", { fix: "resume" }) }));
    fireEvent.click(screen.getByRole("button", { name: "Resume" }));
    expect(await screen.findByText("PM's safety list still names this version.")).toBeTruthy();
    // The row was read again anyway: a refusal can change what the row says.
    expect(refresh).toHaveBeenCalled();
    // And the button is back for another try.
    expect((screen.getByRole("button", { name: "Resume" }) as HTMLButtonElement).disabled).toBe(
      false,
    );
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByText("PM's safety list still names this version.")).toBeNull();
  });

  it("files changed: Repair repairs", async () => {
    repairMod.mockResolvedValue({ id: "acme.tool", version: "1.2.0", state: "active" });
    draw(mod({ state: standing("files-changed", { fix: "repair" }) }));
    fireEvent.click(screen.getByRole("button", { name: "Repair" }));
    await waitFor(() => expect(repairMod).toHaveBeenCalledWith("acme.tool"));
    await waitFor(() => expect(refresh).toHaveBeenCalled());
  });

  it("Repair says why when it can't", async () => {
    repairMod.mockRejectedValue({
      code: "not-from-catalogue",
      message: "This mod came from a file, so PM can't fetch it again.",
    });
    draw(mod({ state: standing("files-changed", { fix: "repair" }) }));
    fireEvent.click(screen.getByRole("button", { name: "Repair" }));
    expect(
      await screen.findByText("This mod came from a file, so PM can't fetch it again."),
    ).toBeTruthy();
  });

  it("a mod whose record can't be read, or the safety list removed: Uninstall… opens the dialog", () => {
    draw(
      mod({ state: standing("removed-by-safety-list", { fix: "uninstall", text: "Removed." }) }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Uninstall…" }));
    expect(onUninstall).toHaveBeenCalledWith("acme.tool");
    // One Uninstall… on the row, not two: the one in Details is for a mod that has another fix.
    expect(screen.getAllByRole("button", { name: "Uninstall…" })).toHaveLength(1);
  });

  it("offers no button of its own where the fix is elsewhere", () => {
    for (const fix of ["install-file-again", "update-pm", "none"] as const) {
      draw(mod({ state: standing("files-missing", { fix, text: "Sentence." }) }));
      for (const name of ["Resume", "Repair", "Review and turn on", "Pause"]) {
        expect(screen.queryByRole("button", { name }), `${fix} ${name}`).toBeNull();
      }
      cleanup();
    }
  });

  it("Pause is offered to a running mod only, and pauses it", async () => {
    pauseMod.mockResolvedValue(undefined);
    draw(mod());
    fireEvent.click(screen.getByRole("button", { name: "Pause" }));
    await waitFor(() => expect(pauseMod).toHaveBeenCalledWith("acme.tool"));
    await waitFor(() => expect(refresh).toHaveBeenCalled());
    cleanup();
    draw(mod({ state: standing("paused-by-you", { fix: "resume" }) }));
    expect(screen.queryByRole("button", { name: "Pause" })).toBeNull();
  });

  it("Pause says why when it can't", async () => {
    pauseMod.mockRejectedValue({ code: "not-installed", message: "That mod isn't installed." });
    draw(mod());
    fireEvent.click(screen.getByRole("button", { name: "Pause" }));
    expect(await screen.findByText("That mod isn't installed.")).toBeTruthy();
  });

  it("a dialog for this mod says its own refusals, so the row doesn't say them twice", async () => {
    pauseMod.mockRejectedValue({ code: "refused", message: "Said once." });
    const view = draw(mod(), false);
    fireEvent.click(screen.getByRole("button", { name: "Pause" }));
    expect(await screen.findByText("Said once.")).toBeTruthy();
    view.rerender(<Row row={mod()} quiet />);
    expect(screen.queryByText("Said once.")).toBeNull();
  });
});

describe("Show tab", () => {
  it("is offered for a mod tab the person hid, and brings it back", () => {
    writeNavPlacement({ hidden: ["mod:acme.tool"], pinned: [] });
    draw(mod());
    fireEvent.click(screen.getByRole("button", { name: "Show tab" }));
    const placement = parsePlacement(localStorage.getItem(NAV_PLACEMENT_KEY));
    expect(placement.hidden).not.toContain("mod:acme.tool");
  });

  it("isn't offered for a tab that is showing, or a mod with no tab", () => {
    draw(mod());
    expect(screen.queryByRole("button", { name: "Show tab" })).toBeNull();
    cleanup();
    draw(mod({ navTab: null }));
    expect(screen.queryByRole("button", { name: "Show tab" })).toBeNull();
  });
});

describe("an update waiting", () => {
  const waiting = {
    version: "1.3.0",
    kind: "approval" as const,
    widenings: ["permission:ai"],
    needsApi: null,
    text: "Version 1.3.0 asks for more than you allowed.",
  };

  it("says so, with the backend's sentence, and Review update fetches the consent screen", async () => {
    const screenFor = consentScreen({ widening: ["permission:ai"] });
    approveUpdate.mockResolvedValue(screenFor);
    const seen: ReturnType<typeof useModActions>["screens"][] = [];
    function Spy() {
      seen.push(useModActions().screens);
      return null;
    }
    render(
      <>
        <Row row={mod({ update: waiting })} />
        <Spy />
      </>,
    );
    expect(screen.getByText("Update waiting")).toBeTruthy();
    expect(screen.getByText("Version 1.3.0 asks for more than you allowed.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Review update" }));
    await waitFor(() => expect(approveUpdate).toHaveBeenCalledWith("acme.tool"));
    // The screen is kept outside the row, so it survives the view going away.
    await waitFor(() => expect(seen[seen.length - 1].get("acme.tool")).toBe(screenFor));
  });

  it("says why the update can't be fetched", async () => {
    approveUpdate.mockRejectedValue({ code: "offline", message: "PM is offline." });
    draw(mod({ update: waiting }));
    fireEvent.click(screen.getByRole("button", { name: "Review update" }));
    expect(await screen.findByText("PM is offline.")).toBeTruthy();
  });

  it("an update that needs a newer PM has no button: PM itself must update first", () => {
    draw(mod({ update: { ...waiting, kind: "newer-pm", text: "Update PM to get 1.3.0." } }));
    expect(screen.getByText("Update PM to get 1.3.0.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Review update" })).toBeNull();
  });
});

describe("what the mod can do", () => {
  const rows = [
    perm("documents.meta", {
      access: "allowed",
      decision: "granted",
      class: "read",
      why: "To map them.",
    }),
    perm("internet", {
      access: "off",
      decision: "denied",
      class: "egress",
      offBecause: "You turned this off.",
    }),
    perm("ai", { access: "asks", when: "first-use", class: "money" }),
    perm("storage", { access: "allowed", when: "auto", class: "local", decision: null }),
  ];

  it("lists each one in the order given, in PM's words, with the maker's reason beside it", () => {
    draw(mod({ permissions: rows, state: standing("paused-by-you", { fix: "resume" }) }));
    const items = screen.getAllByRole("listitem").slice(1); // the row's own <li> is first
    expect(items).toHaveLength(4);
    expect(items[0].textContent).toContain(
      "See your documents' titles, projects, tags, dates and types",
    );
    expect(items[1].textContent).toContain("Send and fetch things from sites on the internet");
    expect(items[2].textContent).toContain(
      "Ask an AI model to do things for it, which can cost money",
    );
    expect(items[3].textContent).toContain("Keep its own saved data inside PM");
    expect(items[0].textContent).toContain("The maker says: “To map them.”");
    expect(items[0].textContent).toContain("Reads your data");
  });

  it("says where each stands", () => {
    draw(mod({ permissions: rows, state: standing("paused-by-you", { fix: "resume" }) }));
    const items = screen.getAllByRole("listitem").slice(1);
    expect(items[0].textContent).toContain("Allowed.");
    expect(items[1].textContent).toContain("You turned this off.");
    expect(items[2].textContent).toContain("PM asks you the first time the mod uses it.");
    expect(items[3].textContent).toContain("Allowed.");
  });

  it("Allow and Turn off send the other answer, for that permission, and read the rows again", async () => {
    modPermissionDecide.mockResolvedValue(undefined);
    draw(mod({ permissions: rows, state: standing("paused-by-you", { fix: "resume" }) }));
    fireEvent.click(
      screen.getByRole("button", {
        name: "Turn off: See your documents' titles, projects, tags, dates and types",
      }),
    );
    await waitFor(() =>
      expect(modPermissionDecide).toHaveBeenCalledWith("acme.tool", "documents.meta", false),
    );
    await waitFor(() => expect(refresh).toHaveBeenCalled());
    fireEvent.click(
      screen.getByRole("button", {
        name: "Allow: Send and fetch things from sites on the internet",
      }),
    );
    await waitFor(() =>
      expect(modPermissionDecide).toHaveBeenCalledWith("acme.tool", "internet", true),
    );
  });

  it("shows the backend's refusal when a grant isn't allowed (an unsigned mod may not read AND reach out)", async () => {
    modPermissionDecide.mockRejectedValue({
      code: "grant-refused",
      message: "A mod nobody has checked can read your data or use the internet, not both.",
    });
    draw(mod({ permissions: rows, state: standing("paused-by-you", { fix: "resume" }) }));
    fireEvent.click(
      screen.getByRole("button", {
        name: "Allow: Send and fetch things from sites on the internet",
      }),
    );
    expect(
      await screen.findByText(
        "A mod nobody has checked can read your data or use the internet, not both.",
      ),
    ).toBeTruthy();
  });

  it("has no Allow or Turn off for a mod that hasn't been turned on, or one the safety list removed", () => {
    draw(
      mod({
        consented: false,
        permissions: rows,
        state: standing("waiting-for-you", { fix: "turn-on" }),
      }),
    );
    expect(screen.queryByRole("button", { name: /^(Allow|Turn off):/ })).toBeNull();
    // The list itself is still there to read.
    expect(screen.getAllByRole("listitem").length).toBeGreaterThan(1);
    cleanup();
    draw(
      mod({
        permissions: rows,
        state: standing("removed-by-safety-list", { fix: "uninstall" }),
      }),
    );
    expect(screen.queryByRole("button", { name: /^(Allow|Turn off):/ })).toBeNull();
  });

  it("says a mod asks for nothing when it asks for nothing", () => {
    draw(mod({ permissions: [], state: standing("paused-by-you", { fix: "resume" }) }));
    expect(screen.getByText("It doesn't ask for anything.")).toBeTruthy();
  });

  it("lists the sites a mod may reach", () => {
    draw(
      mod({
        internetHosts: ["api.example.com", "cdn.example.com"],
        state: standing("paused-by-you", { fix: "resume" }),
      }),
    );
    expect(screen.getByText("Sites it may reach: api.example.com, cdn.example.com")).toBeTruthy();
  });

  it("Uninstall… in the details opens the dialog", () => {
    draw(mod());
    openDetails();
    fireEvent.click(screen.getByRole("button", { name: "Uninstall…" }));
    expect(onUninstall).toHaveBeenCalledWith("acme.tool");
  });
});

describe("folds", () => {
  it("starts closed for a mod that is fine and open for one that needs the person", () => {
    draw(mod());
    expect(screen.getByRole("button", { name: /Details/ }).getAttribute("aria-expanded")).toBe(
      "false",
    );
    cleanup();
    draw(mod({ state: standing("files-changed", { fix: "repair" }) }));
    expect(screen.getByRole("button", { name: /Details/ }).getAttribute("aria-expanded")).toBe(
      "true",
    );
    cleanup();
    draw(
      mod({ update: { version: "2", kind: "approval", widenings: [], needsApi: null, text: "t" } }),
    );
    expect(screen.getByRole("button", { name: /Details/ }).getAttribute("aria-expanded")).toBe(
      "true",
    );
  });

  it("remembers a choice under mods.mod.<id> and mods.mod.<id>:permissions", () => {
    draw(mod({ permissions: [perm("storage", { when: "auto", access: "allowed" })] }));
    fireEvent.click(screen.getByRole("button", { name: /Details/ }));
    expect(readFold("mods.mod.acme.tool")).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: /What it can do/ }));
    expect(readFold("mods.mod.acme.tool:permissions")).toBe(false);
    cleanup();
    draw(mod({ permissions: [perm("storage", { when: "auto", access: "allowed" })] }));
    expect(screen.getByRole("button", { name: /Details/ }).getAttribute("aria-expanded")).toBe(
      "true",
    );
    expect(
      screen.getByRole("button", { name: /What it can do/ }).getAttribute("aria-expanded"),
    ).toBe("false");
  });
});

describe("an action in flight outlives the view", () => {
  it("a view that comes back mid-pause sees it still going and can't ask twice", async () => {
    let finish!: () => void;
    pauseMod.mockReturnValue(
      new Promise<void>((res) => {
        finish = res;
      }),
    );
    const first = draw(mod());
    fireEvent.click(screen.getByRole("button", { name: "Pause" }));
    expect(await screen.findByText("Pausing…")).toBeTruthy();
    // The person switches tab: the view unmounts. The backend's work doesn't stop.
    first.unmount();
    // ...and comes back.
    draw(mod());
    expect(screen.getByText("Pausing…")).toBeTruthy();
    const pause = screen.getByRole("button", { name: "Pause" }) as HTMLButtonElement;
    expect(pause.disabled).toBe(true);
    fireEvent.click(pause);
    expect(pauseMod).toHaveBeenCalledTimes(1);
    await act(async () => finish());
    await waitFor(() => expect(screen.queryByText("Pausing…")).toBeNull());
  });

  it("a refusal that arrives while the view was away is waiting when it comes back", async () => {
    let fail!: (e: unknown) => void;
    resumeMod.mockReturnValue(
      new Promise((_, rej) => {
        fail = rej;
      }),
    );
    const first = draw(mod({ state: standing("paused-by-you", { fix: "resume" }) }));
    fireEvent.click(screen.getByRole("button", { name: "Resume" }));
    first.unmount();
    await act(async () => fail({ code: "refused", message: "Not yet." }));
    draw(mod({ state: standing("paused-by-you", { fix: "resume" }) }));
    expect(screen.getByText("Not yet.")).toBeTruthy();
    expect(
      within(screen.getByText("Not yet.").parentElement!).getByRole("button", { name: "Dismiss" }),
    ).toBeTruthy();
  });
});
