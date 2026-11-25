// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Runway, board and terminal, drawn exactly as they are today: the exact DOM of each, from one
// fixture (tasks in every column, a parked one, a done one, a stack, a milestone, an event and a
// window). A behaviour-neutral move of the Tasks code, the kit's `ui` closure or the theme context
// (the Mods kit, PR C1 K6) must leave every attribute, class and node of both as it was: this file is
// the proof, and its expected DOM (src/components/calendar/__dom__/) is written once, from the code before the move. It uses the REAL
// ThemeProvider (no `useTheme` stub), so it also holds that core and the kit share one theme context.
//
// IPC is mocked whole-module, as RunwayBoard.test.tsx does; the day is fixed (Wednesday 04-11-2026)
// and the events are all-day ones, so no locale clock is in the DOM.

import { cleanup, render } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { CalendarEvent, Milestone, Task, TaskStack } from "../../lib/types";
import type { RunwayWindow } from "../../lib/calendarPrefs";

vi.mock("../../lib/ipc", () => ({
  planTask: vi.fn(async () => {}),
  reorderTasks: vi.fn(async () => {}),
  changeTaskStatus: vi.fn(async () => {}),
  addTask: vi.fn(async () => 1),
  addTaskStack: vi.fn(async () => 1),
  updateTask: vi.fn(async () => {}),
  deleteTask: vi.fn(async () => {}),
  listAllCalendarEvents: vi.fn(async () => []),
  listAllMilestones: vi.fn(async () => []),
  listProjects: vi.fn(async () => []),
  listTaskStacks: vi.fn(async () => []),
  getPref: vi.fn(async () => null),
  setPref: vi.fn(async () => {}),
}));

import { ThemeProvider } from "../../theme";
import { RunwayBoard } from "./views/RunwayBoard";
import { useRunwayModel, type RunwayProps } from "./views/runwayLogic";
import { TerminalRunway } from "./terminal/TerminalRunway";

const NOW = new Date(2026, 10, 4, 10, 0);
const OPENED = new Date(2026, 10, 4, 9, 30);

function task(over: Partial<Task> = {}): Task {
  return {
    id: 1,
    title: "Draft the deck",
    why: null,
    project_name: null,
    project_archived: false,
    milestone_id: null,
    event_uid: null,
    do_start: "2026-11-04",
    do_end: null,
    deadline: null,
    deadline_kind: "hard",
    deadline_from: "date",
    effective_deadline: null,
    deadline_missing: false,
    milestone_label: null,
    event_title: null,
    status: "todo",
    completed_at: null,
    lane: "foreground",
    stack_id: null,
    parked: false,
    ordinal: over.id ?? 1,
    source: "user",
    created_at: "2026-11-01T00:00:00Z",
    updated_at: "2026-11-01T00:00:00Z",
    madeBy: null,
    ...over,
  };
}

const TASKS: Task[] = [
  task({ id: 1, project_name: "Atlas", effective_deadline: "2026-11-06", deadline: "2026-11-06" }),
  task({ id: 2, title: "Call the bank", do_start: "2026-11-05", lane: "background" }),
  task({ id: 3, title: "Someday", do_start: null }),
  task({ id: 4, title: "On the shelf", parked: true }),
  task({ id: 5, title: "Far off", do_start: "2026-12-01" }),
  task({ id: 6, title: "Doing it", status: "doing", do_start: "2026-11-06" }),
  task({
    id: 7,
    title: "Done already",
    status: "done",
    completed_at: "2026-11-04T09:45:00Z",
    do_start: "2026-11-04",
  }),
  task({ id: 8, title: "Passport photo", do_start: "2026-11-04", stack_id: 7 }),
  task({ id: 9, title: "Visa form", do_start: "2026-11-05", stack_id: 7 }),
];

const STACKS: TaskStack[] = [
  { id: 7, title: "Visa", kind: "sequential", note: null, created_at: "", updated_at: "" },
];

const MILESTONES: Milestone[] = [
  {
    id: 1,
    project_name: "Atlas",
    label: "Pitch",
    due_date: "2026-11-05",
    event_uid: null,
    calendar_linked: false,
    event_missing: false,
    state: null,
    status: null,
    source_type: null,
    external_id: null,
    sort_order: 0,
    madeBy: null,
  },
];

const EVENTS: CalendarEvent[] = [
  {
    id: "e1",
    calendar_id: "c",
    summary: "Standup",
    description: null,
    location: null,
    start: "2026-11-04",
    end: null,
    all_day: true,
    html_link: null,
    uid: "u1",
  },
];

const WINDOWS: RunwayWindow[] = [
  { id: "w1", label: "Lisbon trip", start: "2026-11-09", end: "2026-11-12" },
];

/** The Runway as CalendarView mounts it: its model built outside it, and a slot to draw into. */
function Mounted({
  Surface,
  chrome,
}: {
  Surface: typeof RunwayBoard | typeof TerminalRunway;
  chrome: HTMLElement;
}) {
  const runway = useRunwayModel({
    tasks: TASKS,
    stacks: STACKS,
    milestones: MILESTONES,
    events: EVENTS,
    archived: new Set(),
    windows: WINDOWS,
    now: NOW,
    hideDone: false,
    doneCutoff: OPENED,
    eventKinds: "all",
    milestoneUids: new Set(),
  });
  const props: RunwayProps = {
    tasks: TASKS,
    stacks: STACKS,
    events: EVENTS,
    runway,
    chrome,
    windows: WINDOWS,
    now: NOW,
    onWindowsChange: () => {},
    announce: () => {},
  };
  return <Surface {...props} />;
}

const slots: HTMLElement[] = [];
function draw(Surface: typeof RunwayBoard | typeof TerminalRunway) {
  const chrome = document.createElement("div");
  chrome.setAttribute("data-testid", "header-slot");
  document.body.appendChild(chrome);
  slots.push(chrome);
  const { container } = render(
    <ThemeProvider>
      <Mounted Surface={Surface} chrome={chrome} />
    </ThemeProvider>,
  );
  return { container, chrome };
}

beforeAll(() => {
  if (!("ResizeObserver" in globalThis)) {
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
});

beforeEach(() => {
  localStorage.clear();
});

afterEach(() => {
  cleanup();
  for (const el of slots.splice(0)) el.remove();
  localStorage.clear();
});

describe("the Runway (exact DOM)", () => {
  it("the board, with its header slot", async () => {
    localStorage.setItem("pm:theme:system", "slate");
    const { container, chrome } = draw(RunwayBoard);
    await expect(container).toMatchFileSnapshot("./__dom__/runway.board.dom.txt");
    await expect(chrome).toMatchFileSnapshot("./__dom__/runway.boardHeader.dom.txt");
  });

  it("the terminal, with its header slot", async () => {
    localStorage.setItem("pm:theme:system", "terminal");
    const { container, chrome } = draw(TerminalRunway);
    await expect(container).toMatchFileSnapshot("./__dom__/runway.terminal.dom.txt");
    await expect(chrome).toMatchFileSnapshot("./__dom__/runway.terminalHeader.dom.txt");
  });
});
