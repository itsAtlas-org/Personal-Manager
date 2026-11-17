// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Mods tab's read of the safety list. `mods://notice` can fire before the view listens, so the
// status is read on mount; the event and `mods://changed` are only cues to read it again, and answers
// are applied in the order asked.

import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { safety } from "../components/mods/modFixtures";
import type { ModNotice, ModsChanged, ModsSafetyStatus } from "./modsTypes";

const modsSafetyStatus = vi.fn();
let notice: ((n: ModNotice) => void) | null = null;
let changed: ((e: ModsChanged) => void) | null = null;
const unlistenNotice = vi.fn();
const unlistenChanged = vi.fn();

vi.mock("./ipc", () => ({
  modsSafetyStatus: () => modsSafetyStatus(),
  onModsNotice: (h: (n: ModNotice) => void) => {
    notice = h;
    return Promise.resolve(unlistenNotice);
  },
  onModsChanged: (h: (e: ModsChanged) => void) => {
    changed = h;
    return Promise.resolve(unlistenChanged);
  },
}));

import { useModsSafety } from "./useModsSafety";

beforeEach(() => {
  vi.clearAllMocks();
  notice = null;
  changed = null;
});
afterEach(cleanup);

const paused = (name: string): ModsSafetyStatus =>
  safety({
    notices: [{ kind: "paused", id: "a", name, version: "1", reason: "bad", fixedIn: null }],
  });

describe("useModsSafety", () => {
  it("reads the status on mount, without waiting for an event that may have come first", async () => {
    modsSafetyStatus.mockResolvedValue(paused("Acme"));
    const view = renderHook(() => useModsSafety());
    await waitFor(() => expect(view.result.current.status?.notices).toHaveLength(1));
    expect(modsSafetyStatus).toHaveBeenCalledTimes(1);
  });

  it("reads again on a notice and on every change, and stops listening on unmount", async () => {
    modsSafetyStatus.mockResolvedValue(safety());
    const view = renderHook(() => useModsSafety());
    await waitFor(() => expect(view.result.current.status).not.toBeNull());
    modsSafetyStatus.mockResolvedValue(paused("Acme"));
    await act(async () => notice?.({ kind: "resumed", id: "a", name: "Acme", version: "1" }));
    await waitFor(() => expect(view.result.current.status?.notices).toHaveLength(1));
    modsSafetyStatus.mockResolvedValue(safety());
    await act(async () => changed?.({ cause: "safety" }));
    await waitFor(() => expect(view.result.current.status?.notices).toHaveLength(0));
    expect(modsSafetyStatus).toHaveBeenCalledTimes(3);
    view.unmount();
    await waitFor(() => {
      expect(unlistenNotice).toHaveBeenCalled();
      expect(unlistenChanged).toHaveBeenCalled();
    });
  });

  it("draws the status the backend now derives, not the event's own payload", async () => {
    // A paused strip is announced, then the mod is resumed before the read: the status says nothing.
    modsSafetyStatus.mockResolvedValue(safety());
    const view = renderHook(() => useModsSafety());
    await waitFor(() => expect(view.result.current.status).not.toBeNull());
    await act(async () =>
      notice?.({ kind: "paused", id: "a", name: "Acme", version: "1", reason: "x", fixedIn: null }),
    );
    expect(view.result.current.status?.notices).toEqual([]);
  });

  it("applies answers in the order asked: a slow early read can't overwrite a later one", async () => {
    let finishFirst!: (s: ModsSafetyStatus) => void;
    modsSafetyStatus.mockReturnValueOnce(
      new Promise<ModsSafetyStatus>((res) => {
        finishFirst = res;
      }),
    );
    const view = renderHook(() => useModsSafety());
    modsSafetyStatus.mockResolvedValueOnce(paused("Newer"));
    await act(async () => changed?.({ cause: "safety" }));
    await waitFor(() => expect(view.result.current.status?.notices).toHaveLength(1));
    await act(async () => finishFirst(safety()));
    expect(view.result.current.status?.notices).toHaveLength(1);
  });

  it("keeps what it had when a read fails, and says why until one works", async () => {
    modsSafetyStatus.mockResolvedValue(paused("Acme"));
    const view = renderHook(() => useModsSafety());
    await waitFor(() => expect(view.result.current.status).not.toBeNull());
    modsSafetyStatus.mockRejectedValue({ code: "locked", message: "PM is locked." });
    await act(async () => changed?.({ cause: "safety" }));
    await waitFor(() => expect(view.result.current.problem).toBe("PM is locked."));
    expect(view.result.current.status?.notices).toHaveLength(1);
    modsSafetyStatus.mockResolvedValue(safety());
    await act(async () => view.result.current.refresh());
    expect(view.result.current.problem).toBeNull();
    expect(view.result.current.status?.notices).toHaveLength(0);
  });
});
