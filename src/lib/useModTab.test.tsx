// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// `useModTab`: what the Mods tab's row calls for "Show tab" on a mod whose tab the user hid (spec
// §1.17: a hidden mod tab stays reachable from its row), and the nav a mod's tab joins.

import { act, cleanup, renderHook } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));
vi.mock("./capabilities", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useDevMode: () => ({ devMode: false, setDevMode: () => {} }),
}));

import { useModTab, useNav, writeNavPlacement } from "./nav";
import { ModRowsContext } from "./modRows";
import type { ModInfo } from "./modsTypes";

const row = (id: string, navTab: string | null) => ({ id, name: "A Mod", navTab }) as ModInfo;

function wrap(rows: ModInfo[]) {
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <ModRowsContext.Provider
        value={{ rows, loaded: true, problem: null, refresh: () => Promise.resolve() }}
      >
        {children}
      </ModRowsContext.Provider>
    );
  };
}

afterEach(() => {
  cleanup();
  localStorage.clear();
});

describe("useModTab", () => {
  it("says where a mod's tab stands on this device", () => {
    const wrapper = wrap([row("acme.tool", "Acme Tool"), row("acme.quiet", null)]);
    expect(renderHook(() => useModTab("acme.tool"), { wrapper }).result.current.state).toBe(
      "shown",
    );
    expect(renderHook(() => useModTab("acme.quiet"), { wrapper }).result.current.state).toBe(
      "none",
    );
    expect(renderHook(() => useModTab("acme.absent"), { wrapper }).result.current.state).toBe(
      "none",
    );
    writeNavPlacement({ hidden: ["mod:acme.tool"], pinned: [] });
    expect(renderHook(() => useModTab("acme.tool"), { wrapper }).result.current.state).toBe(
      "hidden",
    );
  });

  it("shows a hidden tab again, and hides a shown one, on this device", () => {
    writeNavPlacement({ hidden: ["mod:acme.tool"], pinned: [] });
    const { result } = renderHook(() => useModTab("acme.tool"), {
      wrapper: wrap([row("acme.tool", "Acme Tool")]),
    });
    expect(result.current.state).toBe("hidden");
    act(() => result.current.show());
    expect(result.current.state).toBe("shown");
    expect(JSON.parse(localStorage.getItem("pm.nav.placement") ?? "{}")).toEqual({
      hidden: [],
      pinned: [],
    });
    act(() => result.current.hide());
    expect(result.current.state).toBe("hidden");
  });

  it("does nothing for a mod with no tab", () => {
    const { result } = renderHook(() => useModTab("acme.quiet"), {
      wrapper: wrap([row("acme.quiet", null)]),
    });
    act(() => result.current.show());
    act(() => result.current.hide());
    expect(localStorage.getItem("pm.nav.placement")).toBeNull();
  });
});

describe("useNav", () => {
  it("adds the installed mods' tabs to PM's own, one per mod that names a tab", () => {
    const { result } = renderHook(() => useNav(), {
      wrapper: wrap([row("acme.tool", "Acme Tool"), row("acme.quiet", null)]),
    });
    expect(result.current.resolved.shown.map((e) => e.id)).toContain("mod:acme.tool");
    expect(result.current.entries.filter((e) => e.source === "mod")).toHaveLength(1);
  });

  it("has PM's own tabs alone with no provider above", () => {
    const { result } = renderHook(() => useNav());
    expect(result.current.entries.some((e) => e.source === "mod")).toBe(false);
  });
});
