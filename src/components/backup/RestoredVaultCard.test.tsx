// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The restored-vault card: it lists the mods the backup held with spec §1.11's words, and it no
// longer reloads over the warnings and parked mods the switch reports (the switch's warnings used to
// be dropped altogether).

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { RestoredMod, RestoreSummary, SwitchOutcome } from "../../lib/types";

const switchToVault = vi.fn();

vi.mock("../../lib/ipc", () => ({
  switchToVault: (...a: unknown[]) => switchToVault(...a),
}));

// The Button primitive reaches for `useTheme`, and the real provider pulls in IPC (the same stub
// BackupSettings.test.tsx uses).
vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({
    system: "slate",
    mode: "dark",
    modePref: "system",
    modeSource: "system",
    accent: "mono",
    depth: "standard",
    autoLocation: "",
    teachVisible: true,
    mapVisible: true,
    setSystem: () => {},
    setModePref: () => {},
    setAccent: () => {},
    setDepth: () => {},
    setAutoLocation: () => {},
    setTeachVisible: () => {},
  }),
}));

import { RestoredVaultCard } from "./RestoredVaultCard";

const reload = vi.fn();

function mod(over: Partial<RestoredMod>): RestoredMod {
  return {
    id: "atlas.board",
    name: "Board",
    version: "1.0.0",
    tier: "first",
    signed: true,
    source: "catalogue",
    standing: "pending",
    note: "Install Board to get this back",
    ...over,
  };
}

function summary(mods: RestoredMod[]): RestoreSummary {
  return {
    vault_id: "v",
    key_mode: "device",
    markdown_encryption: "none",
    app_version: "1.0.0",
    created_at: "2026-11-11T10:00:00Z",
    target_dir: "/restored",
    mods,
  };
}

beforeEach(() => {
  Object.defineProperty(window, "location", {
    configurable: true,
    value: { ...window.location, reload },
  });
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

const outcome = (over: Partial<SwitchOutcome>): SwitchOutcome => ({
  warnings: [],
  mods: [],
  ...over,
});

describe("RestoredVaultCard", () => {
  it("lists the mods a backup held, with the words for missing signed and unsigned ones", () => {
    render(
      <RestoredVaultCard
        restored={summary([
          mod({}),
          mod({
            id: "friend.tool",
            name: "Tool",
            signed: false,
            note: "Install the file again to get this back",
          }),
        ])}
        setError={() => {}}
      />,
    );
    expect(screen.getByText("This backup held 2 mods.", { exact: false })).toBeTruthy();
    expect(screen.getByText(/Install Board to get this back/)).toBeTruthy();
    expect(screen.getByText(/Install the file again to get this back/)).toBeTruthy();
    // Said once for every restore: these reset on the way in.
    expect(screen.getByText(/Developer mode comes back off/)).toBeTruthy();
  });

  it("shows no mod list for a backup with none", () => {
    render(<RestoredVaultCard restored={summary([])} setError={() => {}} />);
    expect(screen.queryByText(/This backup held/)).toBeNull();
  });

  it("reloads at once when the switch is clean and every mod is back", async () => {
    switchToVault.mockResolvedValue(outcome({ mods: [mod({ standing: "back", note: null })] }));
    render(<RestoredVaultCard restored={summary([mod({})])} setError={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Switch to the restored vault" }));
    await waitFor(() => expect(reload).toHaveBeenCalledTimes(1));
    expect(switchToVault).toHaveBeenCalledWith("/restored", true);
  });

  it("shows the switch's warnings and the parked mods, and reloads only on Continue", async () => {
    switchToVault.mockResolvedValue(
      outcome({
        warnings: ["PM couldn't re-engage its shared-vault coordination (x)"],
        mods: [
          mod({ standing: "back", note: null, id: "a.back", name: "Fine" }),
          mod({ standing: "parked", note: "Install Board to get this back" }),
        ],
      }),
    );
    render(<RestoredVaultCard restored={summary([mod({})])} setError={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Switch to the restored vault" }));
    expect(
      await screen.findByText(/couldn't re-engage its shared-vault coordination/),
    ).toBeTruthy();
    expect(screen.getByText(/One mod isn't running yet/)).toBeTruthy();
    expect(screen.getByText(/Install Board to get this back/)).toBeTruthy();
    // The mod that is simply back is not listed among the ones to read about.
    expect(screen.queryByText("Fine")).toBeNull();
    expect(reload).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it("a warning alone, with no mods, also holds the reload", async () => {
    switchToVault.mockResolvedValue(outcome({ warnings: ["something to read"] }));
    render(<RestoredVaultCard restored={summary([])} setError={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Switch to the restored vault" }));
    expect(await screen.findByText("something to read")).toBeTruthy();
    expect(reload).not.toHaveBeenCalled();
  });

  it("a refused switch reports the error and doesn't reload", async () => {
    switchToVault.mockRejectedValue("the restored vault couldn't be made safe");
    const setError = vi.fn();
    render(<RestoredVaultCard restored={summary([])} setError={setError} />);
    fireEvent.click(screen.getByRole("button", { name: "Switch to the restored vault" }));
    await waitFor(() =>
      expect(setError).toHaveBeenLastCalledWith("the restored vault couldn't be made safe"),
    );
    expect(reload).not.toHaveBeenCalled();
  });
});
