// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The page's side of the suspend handshake at the IPC boundary (K5): the wait never rejects and never
// outlasts PM's own deadline, and every path that ends a frame's session on the person's word asks the
// frames to save first. Loaded for real over a mocked `invoke`, like ipc.test.ts.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => invokeMock(cmd, args),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const ipc = await import("./ipc");

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
  vi.useFakeTimers();
});
afterEach(() => vi.useRealTimers());

describe("suspendModFrames", () => {
  it("asks the backend to tell one mod's frames, or every frame, and returns once it answers", async () => {
    await ipc.suspendModFrames("hidden", "acme.tool");
    await ipc.suspendModFrames("locked");
    expect(invokeMock).toHaveBeenNthCalledWith(1, "mod_frames_suspend", {
      reason: "hidden",
      modId: "acme.tool",
    });
    expect(invokeMock).toHaveBeenNthCalledWith(2, "mod_frames_suspend", {
      reason: "locked",
      modId: null,
    });
  });

  it("gives up at the page's deadline when the backend never answers, so nothing waits forever", async () => {
    invokeMock.mockReturnValue(new Promise(() => {}));
    let done = false;
    void ipc.suspendModFrames("quit").then(() => {
      done = true;
    });
    await vi.advanceTimersByTimeAsync(ipc.SUSPEND_PAGE_DEADLINE_MS - 1);
    expect(done).toBe(false);
    await vi.advanceTimersByTimeAsync(2);
    expect(done).toBe(true);
  });

  it("never rejects: a failing backend is a frame taken down without waiting", async () => {
    invokeMock.mockRejectedValue("MOD-0000");
    await expect(ipc.suspendModFrames("vault")).resolves.toBeUndefined();
    invokeMock.mockImplementation(() => {
      throw new Error("bridge gone");
    });
    await expect(ipc.suspendModFrames("vault")).resolves.toBeUndefined();
  });

  it("leaves no timer behind once it has answered", async () => {
    await ipc.suspendModFrames("hidden", "acme.tool");
    expect(vi.getTimerCount()).toBe(0);
  });

  it("the page's deadline is past the backend's, which is the one that normally ends the wait", () => {
    expect(ipc.SUSPEND_PAGE_DEADLINE_MS).toBeGreaterThan(1500);
  });
});

describe("what ends a frame's session on the person's word asks the frames to save first", () => {
  const order = () => invokeMock.mock.calls.map((c) => c[0] as string);

  it.each([
    ["makeVaultPrivate", () => ipc.makeVaultPrivate(), "make_vault_private", "vault"],
    ["moveVault", () => ipc.moveVault("/x"), "move_vault", "vault"],
    [
      "createShareableVault",
      () => ipc.createShareableVault("p", "/x"),
      "create_shareable_vault",
      "vault",
    ],
    ["adoptSharedVault", () => ipc.adoptSharedVault("/x", "p"), "adopt_shared_vault", "vault"],
    [
      "detachFromSharedVault",
      () => ipc.detachFromSharedVault(),
      "detach_from_shared_vault",
      "vault",
    ],
    ["switchToVault", () => ipc.switchToVault("/x", false), "switch_to_vault", "vault"],
    ["setAppLock(true)", () => ipc.setAppLock(true), "set_app_lock", "locked"],
  ])("%s", async (_name, run, command, reason) => {
    await run();
    expect(order()).toEqual(["mod_frames_suspend", command]);
    expect(invokeMock.mock.calls[0][1]).toMatchObject({ reason });
  });

  it("turning the lock OFF stops nothing, so it asks nothing", async () => {
    await ipc.setAppLock(false);
    expect(order()).toEqual(["set_app_lock"]);
  });

  it("does not hold the vault change for a backend that never answers", async () => {
    invokeMock.mockImplementation((cmd: string) =>
      cmd === "mod_frames_suspend" ? new Promise(() => {}) : Promise.resolve("done"),
    );
    const changed = ipc.moveVault("/x");
    await vi.advanceTimersByTimeAsync(ipc.SUSPEND_PAGE_DEADLINE_MS + 1);
    await expect(changed).resolves.toBe("done");
    expect(order()).toEqual(["mod_frames_suspend", "move_vault"]);
  });

  it("relays a frame's flushed under its session and nothing else", async () => {
    await ipc.modFrameFlushed("session-1");
    expect(invokeMock).toHaveBeenCalledWith("mod_frame_flushed", { session: "session-1" });
  });
});
