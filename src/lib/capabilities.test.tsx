// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Developer mode on the page: the backend's answer for THIS device is the authority (the stored
// copy in localStorage only avoids a flash on first paint), and the toggle awaits the backend,
// showing what it now holds and changing nothing when the write failed.

import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { DevModeState, DevSwitch, ModsChanged } from "./modsTypes";

const devModeState = vi.fn<() => Promise<DevModeState>>();
const devModeSwitch = vi.fn<(on: boolean) => Promise<DevSwitch>>();
let changedHandler: ((e: ModsChanged) => void) | null = null;
let devHandler: ((e: { on: boolean }) => void) | null = null;
const unlistenChanged = vi.fn();
const unlistenDev = vi.fn();

vi.mock("./ipc", () => ({
  devModeState: () => devModeState(),
  devModeSwitch: (on: boolean) => devModeSwitch(on),
  onModsChanged: (h: (e: ModsChanged) => void) => {
    changedHandler = h;
    return Promise.resolve(unlistenChanged);
  },
  onDevMode: (h: (e: { on: boolean }) => void) => {
    devHandler = h;
    return Promise.resolve(unlistenDev);
  },
}));

import { CapabilityProvider, useDevMode } from "./capabilities";

const LS_KEY = "pm:dev:mode";

let api: ReturnType<typeof useDevMode>;
function Probe() {
  api = useDevMode();
  return <span data-testid="dev">{api.devMode ? "on" : "off"}</span>;
}

function mount() {
  return render(
    <CapabilityProvider>
      <Probe />
    </CapabilityProvider>,
  );
}

const shown = () => screen.getByTestId("dev").textContent;
const state = (on: boolean): DevModeState => ({ on, problem: null });
const sw = (on: boolean): DevSwitch => ({ on, paused: [], resumed: [], unloaded: [] });

beforeEach(() => {
  localStorage.clear();
  devModeState.mockReset();
  devModeSwitch.mockReset();
  unlistenChanged.mockReset();
  unlistenDev.mockReset();
  changedHandler = null;
  devHandler = null;
});
afterEach(cleanup);

describe("hydration: the backend's answer for this device wins", () => {
  it("first paint reads the cache, then the backend's answer replaces it in either direction", async () => {
    localStorage.setItem(LS_KEY, "true");
    devModeState.mockResolvedValue(state(false));
    mount();
    expect(shown()).toBe("on"); // the cache, so nothing flashes
    await waitFor(() => expect(shown()).toBe("off"));
    expect(localStorage.getItem(LS_KEY)).toBe("false");
    cleanup();

    localStorage.setItem(LS_KEY, "false");
    devModeState.mockResolvedValue(state(true));
    mount();
    await waitFor(() => expect(shown()).toBe("on"));
    expect(localStorage.getItem(LS_KEY)).toBe("true");
  });

  it("a restored or joined vault that says off beats a stored on (the old rule let the cache win)", async () => {
    localStorage.setItem(LS_KEY, "true");
    devModeState.mockResolvedValue(state(false));
    mount();
    await waitFor(() => expect(shown()).toBe("off"));
  });

  it("with no answer yet (the vault isn't open) the cache stays, and the next open asks again", async () => {
    localStorage.setItem(LS_KEY, "true");
    devModeState.mockRejectedValueOnce({ code: "no-store", message: "closed" });
    mount();
    await act(async () => {});
    expect(shown()).toBe("on");
    // The vault opens: mods://changed (loaded) makes the page ask again, and the answer is taken.
    devModeState.mockResolvedValue(state(false));
    act(() => changedHandler?.({ cause: "loaded" }));
    await waitFor(() => expect(shown()).toBe("off"));
    expect(devModeState).toHaveBeenCalledTimes(2);
  });

  it("only the causes that can move developer mode make it ask again", async () => {
    devModeState.mockResolvedValue(state(false));
    mount();
    await act(async () => {});
    expect(devModeState).toHaveBeenCalledTimes(1);
    for (const cause of ["installed", "paused", "safety", "decided", "uninstalled"] as const) {
      act(() => changedHandler?.({ cause }));
    }
    expect(devModeState).toHaveBeenCalledTimes(1);
    for (const cause of ["loaded", "recovered", "dev-mode"] as const) {
      act(() => changedHandler?.({ cause }));
    }
    await act(async () => {});
    expect(devModeState).toHaveBeenCalledTimes(4);
  });

  it("another window's switch (mods://dev-mode) is taken as it is announced", async () => {
    devModeState.mockResolvedValue(state(false));
    mount();
    await waitFor(() => expect(shown()).toBe("off"));
    act(() => devHandler?.({ on: true }));
    expect(shown()).toBe("on");
    expect(localStorage.getItem(LS_KEY)).toBe("true");
  });

  it("a slow answer to an older question can't undo a newer one", async () => {
    let first!: (s: DevModeState) => void;
    devModeState.mockReturnValueOnce(new Promise((r) => (first = r)));
    mount();
    // A newer announcement lands before the first answer does.
    act(() => devHandler?.({ on: true }));
    expect(shown()).toBe("on");
    await act(async () => first(state(false)));
    expect(shown()).toBe("on");
  });

  it("unsubscribes when it unmounts", async () => {
    devModeState.mockResolvedValue(state(false));
    const view = mount();
    await act(async () => {});
    view.unmount();
    await waitFor(() => {
      expect(unlistenChanged).toHaveBeenCalledTimes(1);
      expect(unlistenDev).toHaveBeenCalledTimes(1);
    });
  });

  it("never throws without a cache (a locked-down webview)", async () => {
    const get = vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    const set = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    try {
      devModeState.mockResolvedValue(state(true));
      mount();
      expect(shown()).toBe("off");
      await waitFor(() => expect(shown()).toBe("on"));
    } finally {
      get.mockRestore();
      set.mockRestore();
    }
  });
});

describe("the toggle awaits the backend and reverts on failure", () => {
  it("shows what the backend now holds, not what was asked", async () => {
    devModeState.mockResolvedValue(state(false));
    mount();
    await waitFor(() => expect(shown()).toBe("off"));
    // Nothing changes while the write is pending.
    let done!: (s: DevSwitch) => void;
    devModeSwitch.mockReturnValueOnce(new Promise((r) => (done = r)));
    let settled = false;
    let pending!: Promise<void>;
    await act(async () => {
      pending = api.setDevMode(true).then(() => {
        settled = true;
      });
    });
    // Rendered with the write still pending: nothing moved, and it hasn't finished.
    expect(shown()).toBe("off");
    expect(settled).toBe(false);
    await act(async () => {
      done({ on: true, paused: [], resumed: ["acme.tool"], unloaded: [] });
      await pending;
    });
    expect(settled).toBe(true);
    expect(devModeSwitch).toHaveBeenCalledWith(true);
    expect(shown()).toBe("on");
    expect(localStorage.getItem(LS_KEY)).toBe("true");
  });

  it("takes the backend's answer even when it differs from the request", async () => {
    devModeState.mockResolvedValue(state(false));
    mount();
    await waitFor(() => expect(shown()).toBe("off"));
    devModeSwitch.mockResolvedValueOnce(sw(false));
    await act(async () => api.setDevMode(true));
    expect(shown()).toBe("off");
  });

  it("a failed write rejects, and leaves the value and the cache exactly as they were", async () => {
    devModeState.mockResolvedValue(state(true));
    mount();
    await waitFor(() => expect(shown()).toBe("on"));
    const refusal = { code: "no-store", message: "PM's data isn't open." };
    devModeSwitch.mockRejectedValueOnce(refusal);
    let caught: unknown;
    await act(async () => {
      try {
        await api.setDevMode(false);
      } catch (e) {
        caught = e;
      }
    });
    expect(caught).toBe(refusal);
    expect(shown()).toBe("on");
    expect(localStorage.getItem(LS_KEY)).toBe("true");
    // And a retry that works takes effect.
    devModeSwitch.mockResolvedValueOnce(sw(false));
    await act(async () => api.setDevMode(false));
    expect(shown()).toBe("off");
  });

  it("a switch that lands cancels an older question still in flight", async () => {
    let old!: (s: DevModeState) => void;
    devModeState.mockReturnValueOnce(new Promise((r) => (old = r)));
    mount();
    devModeSwitch.mockResolvedValueOnce(sw(true));
    await act(async () => api.setDevMode(true));
    expect(shown()).toBe("on");
    await act(async () => old(state(false)));
    expect(shown()).toBe("on");
  });
});
