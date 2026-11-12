// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";

// The capability layer reaches the store through ipc; a join must not touch it (the shared store
// holds the vault's other devices' dev-mode choice), so every call is a failure here.
const ipc = vi.hoisted(() => ({
  getPref: vi.fn(),
  setPref: vi.fn(),
}));
vi.mock("./ipc", () => ipc);

import { markJustJoinedVault, justJoinedVault } from "./joinedVault";

describe("joining a shared vault", () => {
  beforeEach(() => {
    localStorage.clear();
    ipc.getPref.mockReset();
    ipc.setPref.mockReset();
  });

  it("switches developer mode off on this device without writing the shared store", () => {
    localStorage.setItem("pm:dev:mode", "true");
    markJustJoinedVault();
    // Off, and present: the next boot is not "empty", so it never hydrates another device's "on"
    // from the shared store's `dev_mode` row.
    expect(localStorage.getItem("pm:dev:mode")).toBe("false");
    expect(justJoinedVault()).toBe(true);
    expect(ipc.setPref).not.toHaveBeenCalled();
    expect(ipc.getPref).not.toHaveBeenCalled();
  });

  it("writes the off switch on a fresh profile too, where the key was absent", () => {
    expect(localStorage.getItem("pm:dev:mode")).toBeNull();
    markJustJoinedVault();
    expect(localStorage.getItem("pm:dev:mode")).toBe("false");
  });
});
