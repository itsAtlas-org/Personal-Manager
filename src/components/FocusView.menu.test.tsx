// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Focus's project cards and Archive folder through the real right-click menu. Each item calls the
// card's own control: Open and Triage the card's buttons, Archive the one archive path (which
// regenerates the briefing), and Merge into… and Delete… the card's own type-to-confirm dialogs, which
// the menu opens with the Triage editor closed and never confirms itself. (The dialogs are stubbed
// here: their own tests hold their wording and type-to-confirm.)

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ProjectOverview } from "../lib/types";

const data = vi.hoisted(() => ({ projects: [] as unknown[] }));
const ipc = vi.hoisted(() => ({
  setProjectArchived: vi.fn(async () => {}),
}));
vi.mock("../lib/ipc", () => {
  const known: Record<string, unknown> = {
    ...ipc,
    listProjectOverviews: vi.fn(async () => data.projects),
    calendarOverview: vi.fn(async () => ({ calendars: [] })),
    listCalendarEvents: vi.fn(async () => []),
    checkGoogleCalendars: vi.fn(async () => {}),
  };
  return new Proxy(known, {
    has: () => true,
    get(target, prop: string) {
      if (prop === "then") return undefined;
      if (!(prop in target)) {
        target[prop] = /^on[A-Z]/.test(prop) ? vi.fn(async () => () => {}) : vi.fn(async () => []);
      }
      return target[prop];
    },
  });
});

const refresh = vi.hoisted(() => vi.fn(async () => {}));
vi.mock("../lib/briefing", () => ({
  useBriefing: () => ({ briefing: null, busy: false, refresh }),
}));
vi.mock("./Briefing", () => ({ Briefing: () => null }));
vi.mock("./FocusUpcoming", () => ({ FocusUpcoming: () => null }));
vi.mock("./MergeProjectDialog", () => ({
  MergeProjectDialog: ({ project }: { project: string }) => (
    <div role="dialog">Merge {project}</div>
  ),
}));
vi.mock("./DeleteProjectDialog", () => ({
  DeleteProjectDialog: ({ project }: { project: string }) => (
    <div role="dialog">Delete {project}</div>
  ),
}));

import { FocusView } from "./FocusView";
import { ContextMenuHost } from "./ui/ContextMenuHost";
import { ThemeProvider } from "../theme";
import { closeContextMenu } from "../lib/contextMenu";

function project(name: string, over: Partial<ProjectOverview> = {}): ProjectOverview {
  return {
    name,
    status: "on_track",
    doc_count: 2,
    last_activity: null,
    deadline: null,
    size: "medium",
    blocked_by: null,
    importance: null,
    auto_importance: null,
    calendar_event: null,
    milestones: [],
    governing_milestone: null,
    archived: false,
    ...over,
  } as unknown as ProjectOverview;
}

const onOpenProject = vi.fn();

async function mount(projects: ProjectOverview[]) {
  data.projects = projects;
  render(
    <ThemeProvider>
      <FocusView onOpenProject={onOpenProject} onAsk={() => {}} />
      <ContextMenuHost />
    </ThemeProvider>,
  );
  await act(async () => {});
}

const rightClick = (el: Element) => fireEvent.contextMenu(el, { clientX: 5, clientY: 5 });

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  // jsdom has no matchMedia; the view reads the wide-screen breakpoint with it.
  window.matchMedia = ((query: string) => ({
    matches: true,
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    onchange: null,
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
});
afterEach(() => {
  act(() => closeContextMenu());
  cleanup();
});

describe("a Focus card's menu", () => {
  it("offers Open, Triage, Archive, Merge into… and Delete… last", async () => {
    await mount([project("Atlas"), project("Beta")]);
    rightClick(screen.getByRole("button", { name: /Atlas/ }));
    const menu = screen.getByRole("menu", { name: "Project Atlas" });
    expect([...menu.querySelectorAll('[role="menuitem"]')].map((i) => i.textContent)).toEqual([
      "Open",
      "Triage",
      "Archive",
      "Merge into…",
      "Delete…",
    ]);
  });

  it("Open goes to the project, as the card's own button does", async () => {
    await mount([project("Atlas")]);
    rightClick(screen.getByRole("button", { name: /Atlas/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Open" }));
    expect(onOpenProject).toHaveBeenCalledWith("Atlas");
  });

  it("Triage opens the editor, and the item then reads Close triage", async () => {
    await mount([project("Atlas"), project("Beta")]);
    rightClick(screen.getByRole("button", { name: /Atlas/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Triage" }));
    expect(screen.getAllByRole("button", { name: "Close" })).toHaveLength(1);
    rightClick(screen.getByRole("button", { name: /Atlas/ }));
    expect(screen.getByRole("menuitem", { name: "Close triage" })).toBeTruthy();
  });

  it("Delete… opens the card's delete dialog with the editor closed, and deletes nothing itself", async () => {
    await mount([project("Atlas"), project("Beta")]);
    rightClick(screen.getByRole("button", { name: /Atlas/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete…" }));
    expect(screen.getByRole("dialog").textContent).toBe("Delete Atlas");
    expect(screen.queryByRole("button", { name: "Close" })).toBeNull();
  });

  it("a right-click inside an open dialog opens no card menu", async () => {
    await mount([project("Atlas"), project("Beta")]);
    rightClick(screen.getByRole("button", { name: /Atlas/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete…" }));
    fireEvent.contextMenu(screen.getByRole("dialog"), { clientX: 5, clientY: 5 });
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("Merge into… opens the merge dialog; it is left out when there is nothing to merge into", async () => {
    await mount([project("Atlas"), project("Beta")]);
    rightClick(screen.getByRole("button", { name: /Atlas/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Merge into…" }));
    expect(screen.getByRole("dialog").textContent).toBe("Merge Atlas");
  });

  it("has no Merge into… for the only project", async () => {
    await mount([project("Atlas")]);
    rightClick(screen.getByRole("button", { name: /Atlas/ }));
    expect(screen.queryByRole("menuitem", { name: "Merge into…" })).toBeNull();
  });

  it("Archive writes through the shared path and regenerates the briefing", async () => {
    await mount([project("Atlas")]);
    rightClick(screen.getByRole("button", { name: /Atlas/ }));
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitem", { name: "Archive" }));
    });
    expect(ipc.setProjectArchived).toHaveBeenCalledWith("Atlas", true);
    expect(refresh).toHaveBeenCalledOnce();
  });

  it("opens from the keyboard on the card's Triage button", async () => {
    await mount([project("Atlas")]);
    const triage = screen.getByRole("button", { name: "Triage" });
    triage.focus();
    fireEvent.keyDown(triage, { key: "F10", shiftKey: true });
    expect(screen.getByRole("menu", { name: "Project Atlas" })).toBeTruthy();
  });
});

describe("an archived row's menu", () => {
  it("offers Open and Unarchive, and Unarchive regenerates the briefing too", async () => {
    await mount([project("Atlas"), project("Old", { archived: true })]);
    fireEvent.click(screen.getByText("Archive")); // the folder
    const row = screen.getByTitle("Open Old");
    rightClick(row);
    const menu = screen.getByRole("menu", { name: "Archived project Old" });
    expect([...menu.querySelectorAll('[role="menuitem"]')].map((i) => i.textContent)).toEqual([
      "Open",
      "Unarchive",
    ]);
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitem", { name: "Unarchive" }));
    });
    expect(ipc.setProjectArchived).toHaveBeenCalledWith("Old", false);
    expect(refresh).toHaveBeenCalledOnce();
  });
});
