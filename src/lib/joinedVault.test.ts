// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";

// A join writes nothing to the shared store from the page: developer mode is decided in Rust, per
// device (`mods::devmode`, INVARIANTS.md I-34), and the join's one write there happens in the
// backend. So every ipc call is a failure here.
const ipc = vi.hoisted(() => ({
  getPref: vi.fn(),
  setPref: vi.fn(),
}));
vi.mock("./ipc", () => ipc);

import { clearJustJoinedVault, markJustJoinedVault, justJoinedVault } from "./joinedVault";

describe("joining a shared vault", () => {
  beforeEach(() => {
    localStorage.clear();
    ipc.getPref.mockReset();
    ipc.setPref.mockReset();
  });

  it("flags the profile without touching the shared store or the developer-mode cache", () => {
    localStorage.setItem("pm:dev:mode", "true");
    markJustJoinedVault();
    expect(justJoinedVault()).toBe(true);
    // The page's copy of developer mode is a cache the backend's answer overwrites, not something
    // a join decides.
    expect(localStorage.getItem("pm:dev:mode")).toBe("true");
    expect(ipc.setPref).not.toHaveBeenCalled();
    expect(ipc.getPref).not.toHaveBeenCalled();
  });

  it("is cleared by dismissing the explanation", () => {
    markJustJoinedVault();
    clearJustJoinedVault();
    expect(justJoinedVault()).toBe(false);
  });
});
