// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The catalogue read behind the Available section: on mount and on every `mods://changed`, answers
// applied in the order asked, a failed read keeping what was shown and saying why, and the listener
// let go on unmount.

import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { Catalogue, ModsChanged } from "./modsTypes";

const listCatalogue = vi.fn();
const handlers = new Set<(e: ModsChanged) => void>();

vi.mock("./ipc", () => ({
  listCatalogue: () => listCatalogue(),
  onModsChanged: (h: (e: ModsChanged) => void) => {
    handlers.add(h);
    return Promise.resolve(() => handlers.delete(h));
  },
}));

import { useCatalogue } from "./useCatalogue";

const cat = (n: number): Catalogue => ({
  usable: true,
  unavailable: null,
  entries: Array.from({ length: n }, (_, i) => ({
    id: `a.m${i}`,
    name: `M${i}`,
    description: "",
    tier: "first" as const,
    author: "A",
    repo: "a/b",
    commit: "c",
    version: "1.0.0",
    permissions: [],
    installed: false,
    installedVersion: null,
    updateAvailable: false,
    needsNewerPm: false,
    apiRetired: false,
    blocked: null,
    installable: true,
    whyNot: null,
  })),
});

beforeEach(() => {
  vi.clearAllMocks();
  handlers.clear();
});
afterEach(cleanup);

describe("useCatalogue", () => {
  it("reads on mount", async () => {
    listCatalogue.mockResolvedValue(cat(2));
    const { result } = renderHook(() => useCatalogue());
    expect(result.current.catalogue).toBeNull();
    await waitFor(() => expect(result.current.catalogue?.entries.length).toBe(2));
    expect(result.current.problem).toBeNull();
  });

  it("reads again on every mods://changed", async () => {
    listCatalogue.mockResolvedValue(cat(1));
    const { result } = renderHook(() => useCatalogue());
    await waitFor(() => expect(result.current.catalogue?.entries.length).toBe(1));
    await waitFor(() => expect(handlers.size).toBe(1));
    listCatalogue.mockResolvedValue(cat(3));
    act(() => handlers.forEach((h) => h({ cause: "installed" })));
    await waitFor(() => expect(result.current.catalogue?.entries.length).toBe(3));
  });

  it("keeps what it showed when a later read fails, and says why until one works", async () => {
    listCatalogue.mockResolvedValue(cat(1));
    const { result } = renderHook(() => useCatalogue());
    await waitFor(() => expect(result.current.catalogue).not.toBeNull());
    listCatalogue.mockRejectedValue({ code: "locked", message: "PM is locked." });
    await act(async () => result.current.refresh());
    expect(result.current.catalogue?.entries.length).toBe(1);
    expect(result.current.problem).toBe("PM is locked.");
    listCatalogue.mockResolvedValue(cat(2));
    await act(async () => result.current.refresh());
    expect(result.current.problem).toBeNull();
    expect(result.current.catalogue?.entries.length).toBe(2);
  });

  it("applies answers in the order asked: a slow old answer can't undo a newer one", async () => {
    let old!: (c: Catalogue) => void;
    listCatalogue.mockReturnValueOnce(
      new Promise<Catalogue>((res) => {
        old = res;
      }),
    );
    const { result } = renderHook(() => useCatalogue());
    listCatalogue.mockResolvedValue(cat(5));
    await act(async () => result.current.refresh());
    expect(result.current.catalogue?.entries.length).toBe(5);
    await act(async () => old(cat(1)));
    expect(result.current.catalogue?.entries.length).toBe(5);
  });

  it("lets go of the listener on unmount, and an answer after that changes nothing", async () => {
    listCatalogue.mockResolvedValue(cat(1));
    const { result, unmount } = renderHook(() => useCatalogue());
    await waitFor(() => expect(handlers.size).toBe(1));
    unmount();
    await waitFor(() => expect(handlers.size).toBe(0));
    expect(result.current.catalogue?.entries.length ?? 0).toBeLessThanOrEqual(1);
  });
});
