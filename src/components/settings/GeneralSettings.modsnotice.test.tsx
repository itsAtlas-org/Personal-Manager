// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The way back from "Don't show this again" on a mod's first-open screen (Bobby, 2026-11-17). Its one
// home is Settings > General: a row that is there only while the open vault has the Mods notice
// folded, a "Show it again" button that turns the fold off (the count of showings is kept, so the
// tick box is offered at the next showing), and the tab's "Reset to defaults" that does the same.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const vaultStatus = vi.fn();

vi.mock("../../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  vaultStatus: () => vaultStatus(),
  getPref: () => Promise.resolve(null),
  setPref: () => Promise.resolve(),
  getSettings: () => Promise.resolve({ time_zone: "UTC" }),
  setBriefingWindowVisible: () => Promise.resolve(),
  consentMod: () => Promise.resolve(),
  modsDisclaimerState: () => Promise.resolve({ accepted: false, problem: null }),
  onModsChanged: () => Promise.resolve(() => {}),
}));
vi.mock("./TrayIconRow", () => ({ TrayIconRow: () => null }));
vi.mock("../../theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({
    system: "slate",
    setSystem: () => {},
    mode: "dark",
    modePref: "dark",
    setModePref: () => {},
    modeSource: "pref",
    modeCoords: null,
    modeNextChange: null,
    autoLocation: true,
    setAutoLocation: () => {},
    depth: "standard",
    setDepth: () => {},
    accent: "mono",
    setAccent: () => {},
    fontScale: 1,
    setFontScale: () => {},
    modelsVisible: true,
    setModelsVisible: () => {},
    appearanceIsDefault: true,
    resetAppearance: () => {},
  }),
}));

import { HELP } from "../../lib/help";
import { readShown } from "../../lib/modsDisclaimerShown";
import { mod, perm, standing } from "../mods/modFixtures";
import { ModFirstOpen } from "../mods/ModFirstOpen";
import { GeneralSettings } from "./GeneralSettings";

const A = "pm.mods.disclaimerShown.vault-a";
const B = "pm.mods.disclaimerShown.vault-b";
const ROW = { name: "Show it again" };
const BOX = { name: "Don't show this again" };

const waiting = mod({
  consented: false,
  state: standing("waiting-for-you", { fix: "turn-on" }),
  permissions: [perm("documents.meta")],
});

function put(key: string, shown: number, hide: boolean) {
  localStorage.setItem(key, JSON.stringify({ v: 1, shown, hide }));
}

async function general() {
  const view = render(<GeneralSettings />);
  await act(async () => {});
  return view;
}

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  vaultStatus.mockResolvedValue({ vault_id: "vault-a" });
});
afterEach(cleanup);

describe("the row", () => {
  it("is not there while the notice is not folded", async () => {
    put(A, 3, false);
    await general();
    expect(screen.queryByRole("button", ROW)).toBeNull();
    expect(screen.queryByText("Mods notice")).toBeNull();
  });

  it("is not there when nothing was ever stored", async () => {
    await general();
    expect(screen.queryByRole("button", ROW)).toBeNull();
  });

  it("is there while the open vault has it folded, with its help entry", async () => {
    put(A, 3, true);
    const { container } = await general();
    expect(screen.getByRole("button", ROW)).toBeTruthy();
    expect(screen.getByText(/folded to one line on a new mod's first screen/)).toBeTruthy();
    const id = container.querySelector("[data-help='settings-mods-notice']");
    expect(id).toBeTruthy();
    expect(HELP["settings-mods-notice"]).toBeDefined();
  });

  it("follows the open vault: another vault's fold does not show it", async () => {
    put(B, 3, true);
    await general();
    expect(screen.queryByRole("button", ROW)).toBeNull();
  });

  it("is not there when the vault can't be told (no notice is ever folded then)", async () => {
    put(A, 3, true);
    vaultStatus.mockRejectedValue(new Error("locked"));
    await general();
    expect(screen.queryByRole("button", ROW)).toBeNull();
  });
});

describe("Show it again", () => {
  it("turns the fold off, keeps the count and takes the row away", async () => {
    put(A, 3, true);
    await general();
    fireEvent.click(screen.getByRole("button", ROW));
    await waitFor(() => expect(screen.queryByRole("button", ROW)).toBeNull());
    expect(readShown(A)).toEqual({ shown: 3, hide: false });
  });

  it("touches only the open vault", async () => {
    put(A, 3, true);
    put(B, 5, true);
    await general();
    fireEvent.click(screen.getByRole("button", ROW));
    await waitFor(() => expect(readShown(A).hide).toBe(false));
    expect(readShown(B)).toEqual({ shown: 5, hide: true });
  });

  it("makes the next first-open screen show the whole notice, with the box offered", async () => {
    put(A, 3, true);
    await general();
    fireEvent.click(screen.getByRole("button", ROW));
    await waitFor(() => expect(readShown(A).hide).toBe(false));
    cleanup();

    render(<ModFirstOpen mod={waiting} onTurnedOn={() => Promise.resolve()} />);
    await waitFor(() => expect(readShown(A).shown).toBe(4));
    await act(async () => {});
    expect(screen.getByRole("heading", { name: "About mods" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "About mods" })).toBeNull();
    expect((screen.getByRole("checkbox", BOX) as HTMLInputElement).checked).toBe(false);
  });

  it("leaves the row there when the write fails, so it can be tried again", async () => {
    put(A, 3, true);
    await general();
    const set = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("full");
    });
    fireEvent.click(screen.getByRole("button", ROW));
    await act(async () => {});
    expect(screen.getByRole("button", ROW)).toBeTruthy();
    set.mockRestore();
    expect(readShown(A)).toEqual({ shown: 3, hide: true });
  });
});

describe("Reset General", () => {
  async function reset() {
    fireEvent.click(screen.getByRole("button", { name: "Reset to defaults" }));
    fireEvent.click(await screen.findByRole("button", { name: "Reset" }));
    await act(async () => {});
  }

  it("brings the notice back when it is folded", async () => {
    put(A, 4, true);
    await general();
    expect(
      (screen.getByRole("button", { name: "Reset to defaults" }) as HTMLButtonElement).disabled,
    ).toBe(false);
    await reset();
    await waitFor(() => expect(readShown(A)).toEqual({ shown: 4, hide: false }));
    expect(screen.queryByRole("button", ROW)).toBeNull();
  });

  it("counts a folded notice as something to reset, and nothing folded as default", async () => {
    await general();
    expect(
      (screen.getByRole("button", { name: "Reset to defaults" }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });
});
