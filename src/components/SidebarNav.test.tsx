// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The sidebar's tab list, end to end through the real right-click menu: what a right-click offers,
// that a choice lands in the sidebar at once, and that the tabs with something extra (Chats' New,
// Review's count) keep it. The rule itself is unit-tested in lib/nav.test.ts.

import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { closeContextMenu } from "../lib/contextMenu";

const gates = vi.hoisted(() => ({ depth: "standard", devMode: false }));

vi.mock("../lib/ipc", () => ({ openUrl: vi.fn(() => Promise.resolve()) }));
vi.mock("../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: gates.depth }),
}));
vi.mock("../lib/capabilities", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useDevMode: () => ({ devMode: gates.devMode, setDevMode: () => {} }),
}));

import { HELP } from "../lib/help";
import { ModRowsContext } from "../lib/modRows";
import type { ModInfo } from "../lib/modsTypes";
import { SidebarNav } from "./SidebarNav";
import { ContextMenuHost } from "./ui/ContextMenuHost";

afterEach(() => {
  act(() => closeContextMenu());
  cleanup();
  localStorage.clear();
  gates.depth = "standard";
  gates.devMode = false;
});

function renderNav(props: Partial<Parameters<typeof SidebarNav>[0]> = {}) {
  return render(
    <>
      <SidebarNav view="focus" onNavigate={() => {}} reviewCount={0} onNew={() => {}} {...props} />
      <ContextMenuHost />
    </>,
  );
}

/** The tab names, in the order the sidebar shows them. */
const tabs = () =>
  [...document.querySelectorAll("nav [data-nav-id]")].map((row) => row.getAttribute("data-nav-id"));
const last = <T,>(list: T[]) => list[list.length - 1];

function rightClick(el: Element, init: MouseEventInit = {}) {
  fireEvent.contextMenu(el, { clientX: 5, clientY: 5, ...init });
}

describe("SidebarNav", () => {
  it("gives the Chats row's New its own help entry, since it is not inside the tab's button", () => {
    renderNav();
    const plus = screen.getByRole("button", { name: "New conversation" });
    expect(plus.closest("[data-help]")?.getAttribute("data-help")).toBe("sidebar-new-chat");
    expect(HELP["sidebar-new-chat"]).toBeDefined();
  });

  it("draws the tabs it always had, in order", () => {
    renderNav();
    expect(tabs()).toEqual([
      "focus",
      "chat",
      "calendar",
      "people",
      "documents",
      "review",
      "teach",
      "mods",
    ]);
  });

  it("lights Focus while a project is open, and keeps Chats' New and Review's count", () => {
    const onNew = vi.fn();
    renderNav({ view: "project", reviewCount: 3, onNew });
    expect(screen.getByRole("button", { name: "Focus" }).getAttribute("aria-current")).toBe("page");
    fireEvent.click(screen.getByRole("button", { name: "New conversation" }));
    expect(onNew).toHaveBeenCalledOnce();
    expect(screen.getByRole("button", { name: /Review/ }).textContent).toContain("3");
  });

  it("navigates to a tab's own view", () => {
    const onNavigate = vi.fn();
    renderNav({ onNavigate });
    fireEvent.click(screen.getByRole("button", { name: "Documents" }));
    expect(onNavigate).toHaveBeenCalledWith("documents");
  });

  it("hides a tab from its right-click menu, and shows it again from another tab's", () => {
    renderNav();
    rightClick(screen.getByRole("button", { name: "Documents" }));
    const menu = screen.getByRole("menu", { name: "Documents tab" });
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Hide" }));
    expect(tabs()).not.toContain("documents");

    rightClick(screen.getByRole("button", { name: "Focus" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Show Documents" }));
    expect(tabs()).toContain("documents");
  });

  it("pins a tab to the top, and unpins it", () => {
    renderNav();
    rightClick(screen.getByRole("button", { name: "Documents" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Pin to the top" }));
    expect(tabs()[0]).toBe("documents");

    rightClick(screen.getByRole("button", { name: "Documents" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Unpin" }));
    // Back in its own place, after People and before the learning tools.
    expect(tabs().slice(0, 6)).toEqual([
      "focus",
      "chat",
      "calendar",
      "people",
      "documents",
      "review",
    ]);
  });

  it("hides the learning tools together", () => {
    renderNav();
    rightClick(screen.getByRole("button", { name: "Teach" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Hide Review and Teach" }));
    expect(tabs()).not.toContain("review");
    expect(tabs()).not.toContain("teach");
  });

  it("gives Chats' New the Chats menu, and offers no Hide on Focus", () => {
    renderNav();
    rightClick(screen.getByRole("button", { name: "New conversation" }));
    expect(screen.getByRole("menu", { name: "Chats tab" })).toBeTruthy();
    act(() => closeContextMenu());

    rightClick(screen.getByRole("button", { name: "Focus" }));
    const menu = screen.getByRole("menu", { name: "Focus tab" });
    expect(within(menu).queryByRole("menuitem", { name: "Hide" })).toBeNull();
  });

  it("on Minimal, leaves the learning tools out until shown, which pins", () => {
    gates.depth = "min";
    renderNav();
    expect(tabs()).toEqual(["focus", "chat", "calendar", "people", "documents", "mods"]);
    rightClick(screen.getByRole("button", { name: "Focus" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Show Review and Teach" }));
    expect(tabs().slice(0, 2)).toEqual(["review", "teach"]);
  });

  it("adds Dev only in developer mode, and never offers to hide it", () => {
    renderNav();
    expect(tabs()).not.toContain("dev");
    cleanup();
    gates.devMode = true;
    renderNav();
    expect(last(tabs())).toBe("dev");
    rightClick(screen.getByRole("button", { name: "Dev" }));
    expect(screen.queryByRole("menuitem", { name: "Hide" })).toBeNull();
  });

  it("leaves Shift+right-click to the system menu", () => {
    renderNav();
    rightClick(screen.getByRole("button", { name: "Documents" }), { shiftKey: true });
    expect(screen.queryByRole("menu")).toBeNull();
  });
});

// The mod tabs: installed mods' own entries, appended after PM's (spec §1.17) from the rows
// `lib/modRows` shares, reachable from the same right-click menu as every other tab.
describe("SidebarNav: mod tabs", () => {
  const CORE = ["focus", "chat", "calendar", "people", "documents", "review", "teach", "mods"];
  const row = (id: string, navTab: string | null, name = "Acme Tool"): ModInfo =>
    ({ id, name, navTab }) as ModInfo;

  function renderWithMods(rows: ModInfo[], onNavigate: (view: string) => void = () => {}) {
    return render(
      <ModRowsContext.Provider
        value={{ rows, loaded: true, problem: null, refresh: () => Promise.resolve() }}
      >
        <SidebarNav view="focus" onNavigate={onNavigate} reviewCount={0} onNew={() => {}} />
        <ContextMenuHost />
      </ModRowsContext.Provider>,
    );
  }

  it("appends an installed mod's tab after PM's own, marked as a mod's, and opens it", () => {
    const onNavigate = vi.fn();
    renderWithMods([row("acme.tool", "Acme Tool")], onNavigate);
    expect(tabs()).toEqual([...CORE, "mod:acme.tool"]);
    const tab = screen.getByRole("button", { name: /Acme Tool/ });
    expect(tab.textContent).toContain("mod");
    // The help id is the one literal every mod tab shares (help mode can't see inside a frame).
    expect(tab.getAttribute("data-help")).toBe("nav-mod-tab");
    fireEvent.click(tab);
    expect(onNavigate).toHaveBeenCalledWith("mod:acme.tool");
  });

  it("gives a mod that names no tab none", () => {
    renderWithMods([row("acme.quiet", null)]);
    expect(tabs()).toEqual(CORE);
  });

  it("draws the name a mod chose as plain, cleaned text, so it can't pass for one of PM's", () => {
    // A tab called "Settings" is still marked "mod"; a right-to-left override and a line break in the
    // name can't reorder or split the row.
    renderWithMods([row("acme.tool", "Sett‮ings\nnow")]);
    const tab = document.querySelector('[data-nav-id="mod:acme.tool"] button') as HTMLElement;
    expect(tab.textContent).toContain("Sett ings now");
    expect(tab.textContent).not.toContain("‮");
    expect(tab.textContent).toContain("mod");
  });

  it("pins and hides a mod's tab from its right-click menu, and shows it again from another's", () => {
    renderWithMods([row("acme.tool", "Acme Tool")]);
    rightClick(screen.getByRole("button", { name: /Acme Tool/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Pin to the top" }));
    expect(tabs()[0]).toBe("mod:acme.tool");

    rightClick(screen.getByRole("button", { name: /Acme Tool/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Hide" }));
    expect(tabs()).not.toContain("mod:acme.tool");

    rightClick(screen.getByRole("button", { name: "Focus" }));
    // Named as a mod's tab (quoted and marked), so it can't pass for one of PM's own.
    fireEvent.click(screen.getByRole("menuitem", { name: "Show “Acme Tool” (mod)" }));
    expect(tabs()).toContain("mod:acme.tool");
  });

  it("never offers to hide the Mods tab", () => {
    renderWithMods([row("acme.tool", "Acme Tool")]);
    rightClick(screen.getByRole("button", { name: "Mods" }));
    const menu = screen.getByRole("menu", { name: "Mods tab" });
    expect(within(menu).queryByRole("menuitem", { name: "Hide" })).toBeNull();
    expect(within(menu).getByRole("menuitem", { name: "Pin to the top" })).toBeTruthy();
  });

  it("follows the rows as they change, with no provider drawing PM's own tabs alone", () => {
    const { rerender } = renderWithMods([row("acme.tool", "Acme Tool")]);
    expect(tabs()).toContain("mod:acme.tool");
    rerender(
      <ModRowsContext.Provider
        value={{ rows: [], loaded: true, problem: null, refresh: () => Promise.resolve() }}
      >
        <SidebarNav view="focus" onNavigate={() => {}} reviewCount={0} onNew={() => {}} />
        <ContextMenuHost />
      </ModRowsContext.Provider>,
    );
    expect(tabs()).toEqual(CORE);
  });
});
