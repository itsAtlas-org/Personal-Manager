// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Help mode on the sidebar's small chrome. The tabs are covered by SidebarNav.test.tsx; this is the
// rest — What's New, Settings, the resize grip, a chat row's move and delete, the project view's own
// New, and the slim tab that reopens a collapsed panel. Each must carry an id that HELP really
// defines, because an id with no entry shows nothing and nothing else would notice.

import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Conversation } from "../lib/types";

const ipc = vi.hoisted(() => ({
  listProjects: vi.fn(async () => ["Atlas"]),
  listArchivedProjects: vi.fn(async () => [] as string[]),
  calendarOverview: vi.fn(async () => null),
  onCalendarSynced: vi.fn(async () => () => {}),
}));
vi.mock(
  "../lib/ipc",
  () =>
    new Proxy(ipc, {
      has: () => true,
      get(target, prop: string) {
        if (prop === "then") return undefined;
        if (!(prop in target)) {
          (target as Record<string, unknown>)[prop] = /^on[A-Z]/.test(prop)
            ? vi.fn(async () => () => {})
            : vi.fn(async () => []);
        }
        return (target as Record<string, unknown>)[prop];
      },
    }),
);
vi.mock("../lib/briefing", () => ({
  useBriefing: () => ({ briefing: null, busy: false, refresh: vi.fn(async () => {}) }),
}));
vi.mock("./Briefing", () => ({ Briefing: () => null }));
vi.mock("./SidebarNav", () => ({ SidebarNav: () => null }));

import { HELP } from "../lib/help";
import { ThemeProvider } from "../theme";
import { CollapseTab } from "./CollapseTab";
import { Sidebar } from "./Sidebar";

const conv = (id: number, title: string): Conversation => ({
  id,
  title,
  created_at: "",
  updated_at: "",
  project: null,
});

function mount(view: "chat" | "project", conversations: Conversation[]) {
  const noop = vi.fn();
  return render(
    <ThemeProvider>
      <Sidebar
        view={view}
        onNavigate={noop}
        conversations={conversations}
        activeId={null}
        reviewCount={0}
        onSelect={noop}
        onDelete={noop}
        onMove={noop}
        onNew={noop}
        onNewProjectChat={noop}
        onOpenProject={noop}
        onOpenSettings={noop}
        onOpenWhatsNew={noop}
        onOpenPalette={noop}
        onStartResize={noop}
        betterFit={false}
        sheetsNudge={false}
        chatModel={null}
        backgroundModel={null}
        chatFallbacks={0}
        backgroundFallbacks={0}
        localAi={null}
        width={240}
        resizing={false}
      />
    </ThemeProvider>,
  );
}

/** The help id an element answers to, the way help mode finds it: the nearest `[data-help]`. */
const helpOf = (el: Element) => el.closest("[data-help]")?.getAttribute("data-help");

beforeEach(() => {
  ipc.listProjects.mockResolvedValue(["Atlas"]);
  ipc.listArchivedProjects.mockResolvedValue([]);
});
afterEach(cleanup);

describe("help mode on the sidebar's small chrome", () => {
  it("What's New, Settings and the resize grip each have their own entry", async () => {
    mount("chat", []);
    await act(async () => {});
    const found = {
      "sidebar-whats-new": screen.getByRole("button", { name: "What's New" }),
      "sidebar-settings": screen.getByRole("button", { name: "Settings" }),
      "sidebar-resize": screen.getByRole("separator", { name: "Resize sidebar" }),
    };
    for (const [id, el] of Object.entries(found)) {
      expect(helpOf(el), id).toBe(id);
      expect(HELP[id], id).toBeDefined();
    }
  });

  it("a chat row's move and delete buttons answer to their own entries, not the list's", async () => {
    mount("chat", [conv(1, "Trip plan")]);
    await act(async () => {});
    const move = screen.getByRole("button", { name: /Move conversation/ });
    const del = screen.getByRole("button", { name: /Delete conversation/ });
    expect(helpOf(move)).toBe("sidebar-chat-move");
    expect(helpOf(del)).toBe("sidebar-chat-delete");
    expect(HELP["sidebar-chat-move"]).toBeDefined();
    expect(HELP["sidebar-chat-delete"]).toBeDefined();
  });

  it("the project view's own New is told apart from the Chats row's", async () => {
    mount("project", []);
    await act(async () => {});
    const plus = screen.getByRole("button", { name: "New conversation in this project" });
    expect(helpOf(plus)).toBe("sidebar-new-project-chat");
    expect(HELP["sidebar-new-project-chat"]).toBeDefined();
  });

  it("the reopen tab for a collapsed panel has one generic entry", () => {
    render(<CollapseTab side="left" onExpand={() => {}} />);
    const tab = screen.getByRole("button", { name: "Show panel" });
    expect(helpOf(tab)).toBe("panel-reopen");
    expect(HELP["panel-reopen"]).toBeDefined();
  });
});
