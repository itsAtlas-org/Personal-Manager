// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Routing of the mods' views: "mods" is the Mods tab and "mod:<id>" is one mod's own tab, and App
// sends both here. A `mod:` view must never reach the branch that draws the chat (the last one in
// App's chain), whatever follows the colon.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import appSource from "../../App.tsx?raw";
import { CORE_NAV, isModsRoute } from "../../lib/nav";
import { ModRowsContext } from "../../lib/modRows";
import type { ModInfo } from "../../lib/modsTypes";

vi.mock("../../lib/ipc", () => ({
  onVaultFault: () => Promise.resolve(() => {}),
  onModsChanged: () => Promise.resolve(() => {}),
  onModsNotice: () => Promise.resolve(() => {}),
  consentMod: () => Promise.resolve(),
  // The Mods tab's own reads: the disclaimer was accepted and there is nothing on the safety list.
  modsDisclaimerState: () => Promise.resolve({ accepted: true, problem: null }),
  modsSafetyStatus: () =>
    Promise.resolve({
      checked: { kind: "checked", at: null, text: null },
      record: null,
      index: null,
      notices: [],
    }),
}));
vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

vi.mock("../../lib/capabilities", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useDevMode: () => ({ devMode: false, setDevMode: () => {} }),
}));

import { ModsRoute } from "./ModsRoute";

afterEach(cleanup);

const noop = () => {};
// Waiting for its first open, so the mod's tab draws PM's own screen and no frame starts.
const row = {
  id: "acme.tool",
  name: "Acme Tool",
  navTab: "Acme",
  tier: "first",
  signed: true,
  author: "Acme",
  version: "1.0.0",
  description: "",
  permissions: [],
  internetHosts: [],
  state: {
    kind: "waiting-for-you",
    running: false,
    fix: "turn-on",
    text: "",
    state: "active",
    reason: null,
  },
} as unknown as ModInfo;

function route(view: string, rows: ModInfo[] = [row]) {
  return render(
    <ModRowsContext.Provider
      value={{ rows, loaded: true, problem: null, refresh: () => Promise.resolve() }}
    >
      <ModsRoute view={view} onOpenMods={noop} onOpenSettings={noop} />
    </ModRowsContext.Provider>,
  );
}

describe("isModsRoute", () => {
  it("is the Mods tab and every mod's tab, and nothing of PM's own", () => {
    expect(isModsRoute("mods")).toBe(true);
    expect(isModsRoute("mod:acme.tool")).toBe(true);
    expect(isModsRoute("mod:")).toBe(true);
    for (const entry of CORE_NAV.filter((e) => e.id !== "mods")) {
      expect(isModsRoute(entry.id), entry.id).toBe(false);
    }
    expect(isModsRoute("project")).toBe(false);
    expect(isModsRoute("modx")).toBe(false);
  });
});

describe("ModsRoute", () => {
  it("draws the Mods tab for 'mods'", async () => {
    route("mods");
    expect(screen.getByRole("heading", { name: "Mods", level: 1 })).toBeTruthy();
    expect(await screen.findByRole("heading", { name: "Installed" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Available" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Install from file" })).toBeTruthy();
  });

  it("counts what is installed, from the shared rows", () => {
    route("mods", []);
    expect(screen.getByText("No mods installed")).toBeTruthy();
    cleanup();
    route("mods", [row]);
    expect(screen.getByText("1 mod installed")).toBeTruthy();
  });

  it("draws one mod's own tab for 'mod:<id>', never the Mods tab", () => {
    route("mod:acme.tool");
    expect(screen.getByRole("heading", { name: "Turn on “Acme Tool”?" })).toBeTruthy();
    expect(screen.queryByRole("heading", { name: "Installed" })).toBeNull();
  });

  it("hands a mod tab's Open Settings the Developer page, through to whoever opens Settings", () => {
    const paused = {
      ...row,
      state: { ...row.state, kind: "paused-dev-mode", fix: "none", text: "Paused." },
    } as unknown as ModInfo;
    const onOpenSettings = vi.fn();
    render(
      <ModRowsContext.Provider
        value={{ rows: [paused], loaded: true, problem: null, refresh: () => Promise.resolve() }}
      >
        <ModsRoute view="mod:acme.tool" onOpenMods={noop} onOpenSettings={onOpenSettings} />
      </ModRowsContext.Provider>,
    );
    expect(screen.getByText("Developer mode off")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Open Settings" }));
    expect(onOpenSettings).toHaveBeenCalledExactlyOnceWith("developer");
  });

  it("says so for a mod that isn't there, whatever follows the colon", () => {
    route("mod:no.such");
    expect(screen.getByRole("heading", { name: "This mod isn't installed" })).toBeTruthy();
    cleanup();
    route("mod:");
    expect(screen.getByRole("heading", { name: "This mod isn't installed" })).toBeTruthy();
  });
});

describe("App's routing", () => {
  it("sends the mods' views through ModsRoute, ahead of the chat branch they must not reach", () => {
    // App can't be rendered in a test, so the order of its view chain is read from the source: the
    // chain ends in the chat, and a `mod:` view falling into it would draw a conversation under a
    // mod's name.
    const route = appSource.indexOf("isModsRoute(view)");
    const chat = appSource.indexOf("<ChatView");
    expect(route).toBeGreaterThan(0);
    expect(chat).toBeGreaterThan(route);
    expect(appSource).toContain("<ModsRoute");
    // The old stub that drew nothing for a mod view is gone.
    expect(appSource).not.toMatch(/isModView\(view\) \? null/);
  });

  it("asks App's fallback the same question the sidebar answers: nav entries, then reachable", () => {
    // ...and tells it whether the mods have been read, so a mod tab open while the store re-opens
    // (the provider empties the rows until the new read lands) isn't sent to Focus.
    expect(appSource).toMatch(/const \{ loaded: modsLoaded \} = useModRows\(\)/);
    expect(appSource).toMatch(/isReachable\(view, navEntries, nav, modsLoaded\)/);
  });
});
