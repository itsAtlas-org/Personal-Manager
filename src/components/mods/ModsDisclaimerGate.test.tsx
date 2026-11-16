// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Mods tab's first open (spec §1.13): nothing the tab offers shows until the backend says this
// device has accepted the disclaimer in this vault. "I understand" is `accept_mods_disclaimer`; a
// "Turn on" elsewhere writes the same record, and the next `mods://changed` lifts the gate.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { resetModActions } from "../../lib/modActions";
import type { ModsChanged, ModsDisclaimerState } from "../../lib/modsTypes";

const modsDisclaimerState = vi.fn();
const acceptModsDisclaimer = vi.fn();
let changed: ((e: ModsChanged) => void) | null = null;

vi.mock("../../lib/ipc", () => ({
  modsDisclaimerState: () => modsDisclaimerState(),
  acceptModsDisclaimer: () => acceptModsDisclaimer(),
  onModsChanged: (h: (e: ModsChanged) => void) => {
    changed = h;
    return Promise.resolve(() => {});
  },
}));

vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { ModsDisclaimerGate } from "./ModsDisclaimerGate";

const state = (accepted: boolean, problem: string | null = null): ModsDisclaimerState => ({
  accepted,
  problem,
});

function gate() {
  return render(
    <ModsDisclaimerGate>
      <p>the sections</p>
    </ModsDisclaimerGate>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  resetModActions();
  changed = null;
});
afterEach(cleanup);

describe("ModsDisclaimerGate", () => {
  it("shows nothing of the tab until the backend has answered", async () => {
    let answer!: (s: ModsDisclaimerState) => void;
    modsDisclaimerState.mockReturnValue(
      new Promise<ModsDisclaimerState>((res) => {
        answer = res;
      }),
    );
    gate();
    expect(screen.getByText("Loading…")).toBeTruthy();
    expect(screen.queryByText("the sections")).toBeNull();
    expect(screen.queryByRole("button", { name: "I understand" })).toBeNull();
    await act(async () => answer(state(true)));
    expect(screen.getByText("the sections")).toBeTruthy();
  });

  it("shows the sections at once to a device that has accepted", async () => {
    modsDisclaimerState.mockResolvedValue(state(true));
    gate();
    expect(await screen.findByText("the sections")).toBeTruthy();
    expect(screen.queryByText("Before you use mods")).toBeNull();
  });

  it("shows the disclaimer, and not the sections, to a device that hasn't", async () => {
    modsDisclaimerState.mockResolvedValue(state(false));
    gate();
    expect(await screen.findByRole("heading", { name: "Before you use mods" })).toBeTruthy();
    expect(screen.getByText(/Mods are small add-ons made by people/)).toBeTruthy();
    expect(screen.getByText("What PM checks.")).toBeTruthy();
    expect(screen.getByText(/can.t check\./)).toBeTruthy();
    expect(screen.queryByText("the sections")).toBeNull();
  });

  it('"I understand" accepts, reads again, and lets the sections through', async () => {
    modsDisclaimerState.mockResolvedValueOnce(state(false)).mockResolvedValue(state(true));
    acceptModsDisclaimer.mockResolvedValue(undefined);
    gate();
    fireEvent.click(await screen.findByRole("button", { name: "I understand" }));
    expect(await screen.findByText("the sections")).toBeTruthy();
    expect(acceptModsDisclaimer).toHaveBeenCalledTimes(1);
  });

  it("can't be pressed twice while it is being saved", async () => {
    modsDisclaimerState.mockResolvedValueOnce(state(false)).mockResolvedValue(state(true));
    let done!: () => void;
    acceptModsDisclaimer.mockReturnValue(
      new Promise<void>((res) => {
        done = res;
      }),
    );
    gate();
    const button = await screen.findByRole("button", { name: "I understand" });
    fireEvent.click(button);
    const saving = await screen.findByRole("button", { name: "Saving…" });
    expect((saving as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(saving);
    await act(async () => done());
    expect(acceptModsDisclaimer).toHaveBeenCalledTimes(1);
  });

  it("says the backend's sentence when saving is refused, and keeps asking", async () => {
    modsDisclaimerState.mockResolvedValue(state(false));
    acceptModsDisclaimer.mockRejectedValue({ code: "locked", message: "PM is locked." });
    gate();
    fireEvent.click(await screen.findByRole("button", { name: "I understand" }));
    expect(await screen.findByText("PM is locked.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "I understand" })).toBeTruthy();
    expect(screen.queryByText("the sections")).toBeNull();
  });

  it("asks again, and says why, when the backend couldn't tell whether it was accepted", async () => {
    modsDisclaimerState.mockResolvedValue(state(false, "The record couldn't be read."));
    gate();
    expect(await screen.findByText(/The record couldn.t be read\./)).toBeTruthy();
    expect(screen.getByRole("button", { name: "I understand" })).toBeTruthy();
  });

  it("is not claimed accepted when the read itself fails: the backend's words and Try again", async () => {
    modsDisclaimerState.mockRejectedValueOnce({
      code: "no-store",
      message: "Your vault is closed.",
    });
    modsDisclaimerState.mockResolvedValue(state(true));
    gate();
    expect(await screen.findByText("Your vault is closed.")).toBeTruthy();
    expect(screen.queryByText("the sections")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByText("the sections")).toBeTruthy();
  });

  it("reads again on mods://changed: a Turn on elsewhere counts as accepting", async () => {
    modsDisclaimerState.mockResolvedValueOnce(state(false)).mockResolvedValue(state(true));
    gate();
    await screen.findByRole("button", { name: "I understand" });
    await waitFor(() => expect(changed).not.toBeNull());
    await act(async () => changed?.({ cause: "consented" }));
    expect(await screen.findByText("the sections")).toBeTruthy();
    expect(acceptModsDisclaimer).not.toHaveBeenCalled();
  });

  it("keeps the sections when a later read fails after one that said accepted", async () => {
    modsDisclaimerState.mockResolvedValueOnce(state(true));
    gate();
    await screen.findByText("the sections");
    modsDisclaimerState.mockRejectedValue({ code: "locked", message: "PM is locked." });
    await act(async () => changed?.({ cause: "loaded" }));
    expect(screen.getByText("the sections")).toBeTruthy();
  });
});
