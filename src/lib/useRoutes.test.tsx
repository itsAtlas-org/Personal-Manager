// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The lists the editor draws are Rust's, read again after every edit. What must hold: a read that
// began BEFORE an edit's own answer never lands after it with the list as it was one edit ago (the
// next keypress would then carry stale indices and move or remove the wrong row).

import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const m = vi.hoisted(() => ({ routeStatus: vi.fn(), onRouteChanged: vi.fn() }));
vi.mock("./ipc", () => ({
  routeStatus: () => m.routeStatus(),
  onRouteChanged: (...a: unknown[]) => m.onRouteChanged(...a),
}));

import { orRow, routeView } from "./routeFixture";
import type { RouteView } from "./types";
import { useRoutes } from "./useRoutes";

afterEach(() => vi.clearAllMocks());

const status = (chat: RouteView) => ({ chat, background: routeView([orRow("bg/1")]) });

describe("useRoutes", () => {
  it("draws what the first read answers", async () => {
    m.onRouteChanged.mockResolvedValue(() => {});
    m.routeStatus.mockResolvedValue(status(routeView([orRow("a/1")])));
    const { result } = renderHook(() => useRoutes());
    await waitFor(() => expect(result.current.chat?.entries[0].model).toBe("a/1"));
  });

  it("says so when the first read fails, and clears it when a retry lands", async () => {
    m.onRouteChanged.mockResolvedValue(() => {});
    m.routeStatus.mockRejectedValueOnce(new Error("no"));
    const { result } = renderHook(() => useRoutes());
    await waitFor(() => expect(result.current.failed).toBe(true));
    expect(result.current.chat).toBeNull();
    m.routeStatus.mockResolvedValueOnce(status(routeView([orRow("a/1")])));
    await act(async () => {
      await result.current.reload();
    });
    expect(result.current.failed).toBe(false);
    expect(result.current.chat?.entries[0].model).toBe("a/1");
  });

  it("keeps a list it already read when a later read fails (nothing to apologise for)", async () => {
    m.onRouteChanged.mockResolvedValue(() => {});
    m.routeStatus.mockResolvedValueOnce(status(routeView([orRow("a/1")])));
    const { result } = renderHook(() => useRoutes());
    await waitFor(() => expect(result.current.chat).not.toBeNull());
    m.routeStatus.mockRejectedValueOnce(new Error("no"));
    await act(async () => {
      await result.current.reload();
    });
    expect(result.current.failed).toBe(false);
    expect(result.current.chat?.entries[0].model).toBe("a/1");
  });

  it("an edit's answer is never overwritten by a read that began before it", async () => {
    m.onRouteChanged.mockResolvedValue(() => {});
    m.routeStatus.mockResolvedValueOnce(status(routeView([orRow("a/1")])));
    const { result } = renderHook(() => useRoutes());
    await waitFor(() => expect(result.current.chat).not.toBeNull());

    // A read starts (the backend said something changed) and is slow...
    let land!: (v: ReturnType<typeof status>) => void;
    m.routeStatus.mockImplementationOnce(() => new Promise((r) => (land = r)));
    let reading!: Promise<void>;
    act(() => {
      reading = result.current.reload();
    });
    // ...an edit answers with the list as it now is...
    act(() => result.current.set("chat", routeView([orRow("b/2"), orRow("a/1")])));
    expect(result.current.chat?.entries.map((e) => e.model)).toEqual(["b/2", "a/1"]);
    // ...and the slow read lands with the list as it was one edit ago: it must not be drawn.
    await act(async () => {
      land(status(routeView([orRow("a/1")])));
      await reading;
    });
    expect(result.current.chat?.entries.map((e) => e.model)).toEqual(["b/2", "a/1"]);
  });

  it("the newest of two reads wins, whichever lands last", async () => {
    m.onRouteChanged.mockResolvedValue(() => {});
    m.routeStatus.mockResolvedValueOnce(status(routeView([orRow("a/1")])));
    const { result } = renderHook(() => useRoutes());
    await waitFor(() => expect(result.current.chat).not.toBeNull());
    const lands: Array<(v: ReturnType<typeof status>) => void> = [];
    m.routeStatus.mockImplementation(() => new Promise((r) => lands.push(r)));
    let first!: Promise<void>;
    let second!: Promise<void>;
    act(() => {
      first = result.current.reload();
      second = result.current.reload();
    });
    await act(async () => {
      lands[1](status(routeView([orRow("new/2")])));
      await second;
      lands[0](status(routeView([orRow("old/1")])));
      await first;
    });
    expect(result.current.chat?.entries[0].model).toBe("new/2");
  });
});
