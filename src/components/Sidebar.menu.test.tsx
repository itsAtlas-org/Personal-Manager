// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The sidebar's conversation rows and project entries, through the real right-click menu: each item
// calls what the row's own button calls, Delete… opens the existing confirm and deletes nothing, a
// project chat has no Rename, and Archive goes through the shared hook (which regenerates the briefing).

import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Conversation } from "../lib/types";

const ipc = vi.hoisted(() => ({
  listProjects: vi.fn(async () => ["Atlas"]),
  listArchivedProjects: vi.fn(async () => [] as string[]),
  setProjectArchived: vi.fn(async () => {}),
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

const refresh = vi.hoisted(() => vi.fn(async () => {}));
vi.mock("../lib/briefing", () => ({
  useBriefing: () => ({ briefing: null, busy: false, refresh }),
}));
vi.mock("./Briefing", () => ({ Briefing: () => null }));
vi.mock("./SidebarNav", () => ({ SidebarNav: () => null }));

import { Sidebar } from "./Sidebar";
import { ContextMenuHost } from "./ui/ContextMenuHost";
import { ThemeProvider } from "../theme";
import { closeContextMenu } from "../lib/contextMenu";

function conv(id: number, title: string, project: string | null = null): Conversation {
  return { id, title, created_at: "", updated_at: "", project };
}

const handlers = {
  onNavigate: vi.fn(),
  onSelect: vi.fn(),
  onDelete: vi.fn(),
  onMove: vi.fn(),
  onRename: vi.fn(),
  onNew: vi.fn(),
  onNewProjectChat: vi.fn(),
  onOpenProject: vi.fn(),
  onOpenSettings: vi.fn(),
  onOpenWhatsNew: vi.fn(),
  onOpenPalette: vi.fn(),
  onStartResize: vi.fn(),
};

function mount(view: "chat" | "project", conversations: Conversation[]) {
  return render(
    <ThemeProvider>
      <Sidebar
        {...handlers}
        view={view}
        conversations={conversations}
        activeId={null}
        reviewCount={0}
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
      <ContextMenuHost />
    </ThemeProvider>,
  );
}

const rightClick = (el: Element) => fireEvent.contextMenu(el, { clientX: 5, clientY: 5 });

beforeEach(() => {
  vi.clearAllMocks();
  ipc.listProjects.mockResolvedValue(["Atlas"]);
  ipc.listArchivedProjects.mockResolvedValue([]);
});
afterEach(() => {
  act(() => closeContextMenu());
  cleanup();
});

describe("a conversation row's menu", () => {
  it("a global chat: Open, Rename, Move to project…, Delete…", async () => {
    mount("chat", [conv(1, "Trip plan")]);
    await act(async () => {});
    rightClick(screen.getByRole("button", { name: "Trip plan" }));
    const menu = screen.getByRole("menu", { name: "Conversation Trip plan" });
    expect(
      within(menu)
        .getAllByRole("menuitem")
        .map((i) => i.textContent),
    ).toEqual(["Open", "Rename", "Move to project…", "Delete…"]);
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Rename" }));
    expect(handlers.onRename).toHaveBeenCalledWith(1);
  });

  it("Open selects it, Move opens the existing picker, and Delete… opens the confirm without deleting", async () => {
    mount("chat", [conv(1, "Trip plan")]);
    await act(async () => {});
    const row = screen.getByRole("button", { name: "Trip plan" });
    rightClick(row);
    fireEvent.click(screen.getByRole("menuitem", { name: "Open" }));
    expect(handlers.onSelect).toHaveBeenCalledWith(1);

    rightClick(row);
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete…" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Delete this conversation?")).toBeTruthy();
    expect(handlers.onDelete).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(handlers.onDelete).toHaveBeenCalledWith(1);
  });

  it("Move to project… opens the picker the 📁 button opens", async () => {
    mount("chat", [conv(1, "Trip plan")]);
    await act(async () => {});
    rightClick(screen.getByRole("button", { name: "Trip plan" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Move to project…" }));
    expect(await screen.findByRole("dialog")).toBeTruthy();
    expect(handlers.onMove).not.toHaveBeenCalled();
  });

  it("a project chat has no Rename", async () => {
    mount("project", [conv(2, "Atlas chat", "Atlas")]);
    await act(async () => {});
    rightClick(screen.getByRole("button", { name: "Atlas chat" }));
    expect(screen.getByRole("menuitem", { name: "Open" })).toBeTruthy();
    expect(screen.queryByRole("menuitem", { name: "Rename" })).toBeNull();
  });

  it("opens from the keyboard on the row's button", async () => {
    mount("chat", [conv(1, "Trip plan")]);
    await act(async () => {});
    const row = screen.getByRole("button", { name: "Trip plan" });
    row.focus();
    fireEvent.keyDown(row, { key: "ContextMenu" });
    expect(screen.getByRole("menu", { name: "Conversation Trip plan" })).toBeTruthy();
  });
});

describe("a project entry's menu", () => {
  it("Open goes to the project", async () => {
    mount("chat", []);
    await act(async () => {});
    rightClick(screen.getByTitle("Atlas"));
    fireEvent.click(screen.getByRole("menuitem", { name: "Open" }));
    expect(handlers.onOpenProject).toHaveBeenCalledWith("Atlas");
  });

  it("Archive goes through the shared hook: writes, regenerates the briefing, and the entry leaves", async () => {
    mount("chat", []);
    await act(async () => {});
    rightClick(screen.getByTitle("Atlas"));
    ipc.listArchivedProjects.mockResolvedValue(["Atlas"]);
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitem", { name: "Archive" }));
    });
    expect(ipc.setProjectArchived).toHaveBeenCalledWith("Atlas", true);
    expect(refresh).toHaveBeenCalledOnce();
    expect(screen.queryByTitle("Atlas")).toBeNull();
  });

  it("says so when the archive write fails, and keeps the entry", async () => {
    ipc.setProjectArchived.mockRejectedValueOnce(new Error("db locked"));
    mount("chat", []);
    await act(async () => {});
    rightClick(screen.getByTitle("Atlas"));
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitem", { name: "Archive" }));
    });
    expect(screen.getByRole("alert").textContent).toBe("Couldn't archive Atlas: db locked");
    expect(screen.getByTitle("Atlas")).toBeTruthy();
  });

  // The archived entry leaves the list with keyboard focus on it; focus moves to a neighbour instead
  // of falling to the page.
  async function archiveFromTheKeyboard(name: string) {
    const button = screen.getByTitle(name).closest("button") as HTMLButtonElement;
    button.focus();
    rightClick(button);
    ipc.listArchivedProjects.mockResolvedValue([name]);
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitem", { name: "Archive" }));
    });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });
  }

  it("puts focus on the next project once the archived one has gone", async () => {
    ipc.listProjects.mockResolvedValue(["Atlas", "Beta"]);
    mount("chat", []);
    await act(async () => {});
    await archiveFromTheKeyboard("Atlas");
    expect(screen.queryByTitle("Atlas")).toBeNull();
    expect(document.activeElement).toBe(screen.getByTitle("Beta").closest("button"));
  });

  it("puts focus on the project before it when the last one goes", async () => {
    ipc.listProjects.mockResolvedValue(["Atlas", "Beta"]);
    mount("chat", []);
    await act(async () => {});
    await archiveFromTheKeyboard("Beta");
    expect(document.activeElement).toBe(screen.getByTitle("Atlas").closest("button"));
  });
});
