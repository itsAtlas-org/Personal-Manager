// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Also save mod data in my backups": the switch reads the stored value, writes on a flip, only moves
// once the store says so, and says (inline, not folded) what turning it off costs.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const getBackupModData = vi.fn();
const saveBackupModData = vi.fn();

vi.mock("../../lib/ipc", () => ({
  getBackupModData: () => getBackupModData(),
  saveBackupModData: (...a: unknown[]) => saveBackupModData(...a),
}));

import { BackupModDataSection } from "./BackupModDataSection";

const LABEL = "Also save mod data in my backups";
const LOSS = /A restore then brings those mods back empty/;

beforeEach(() => {
  getBackupModData.mockResolvedValue(true);
  saveBackupModData.mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("BackupModDataSection", () => {
  it("is on by default and says passwords never go in either way", async () => {
    render(<BackupModDataSection busy={false} />);
    const sw = await screen.findByRole("switch", { name: LABEL });
    expect(sw.getAttribute("aria-checked")).toBe("true");
    expect(screen.getByText(/Their passwords and API keys never do/)).toBeTruthy();
    expect(screen.queryByText(LOSS)).toBeNull();
  });

  it("turning it off saves false and names what that drops", async () => {
    render(<BackupModDataSection busy={false} />);
    fireEvent.click(await screen.findByRole("switch", { name: LABEL }));
    await waitFor(() => expect(saveBackupModData).toHaveBeenCalledWith(false));
    expect((await screen.findByRole("switch", { name: LABEL })).getAttribute("aria-checked")).toBe(
      "false",
    );
    // The loss is on screen, not behind a fold.
    expect(screen.getByText(LOSS)).toBeTruthy();
    expect(screen.getByText(/Backups leave out what mods have saved in PM/)).toBeTruthy();
  });

  it("reads the stored value, so an off setting opens off", async () => {
    getBackupModData.mockResolvedValue(false);
    render(<BackupModDataSection busy={false} />);
    const sw = await screen.findByRole("switch", { name: LABEL });
    expect(sw.getAttribute("aria-checked")).toBe("false");
    expect(screen.getByText(LOSS)).toBeTruthy();
  });

  it("a save that fails leaves the switch where it was and says so", async () => {
    saveBackupModData.mockRejectedValue("the store is locked");
    render(<BackupModDataSection busy={false} />);
    fireEvent.click(await screen.findByRole("switch", { name: LABEL }));
    expect(await screen.findByText(/Couldn't save this setting: the store is locked/)).toBeTruthy();
    expect(screen.getByRole("switch", { name: LABEL }).getAttribute("aria-checked")).toBe("true");
    expect(screen.queryByText(LOSS)).toBeNull();
  });

  it("is not drawn on a guess when the value can't be read", async () => {
    getBackupModData.mockRejectedValue("no store");
    render(<BackupModDataSection busy={false} />);
    expect(await screen.findByText(/Couldn't read this setting: no store/)).toBeTruthy();
    expect(screen.queryByRole("switch")).toBeNull();
  });

  it("can't be flipped while a backup is running", async () => {
    render(<BackupModDataSection busy />);
    const sw = (await screen.findByRole("switch", { name: LABEL })) as HTMLButtonElement;
    expect(sw.disabled).toBe(true);
  });
});
