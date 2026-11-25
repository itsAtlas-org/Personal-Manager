// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Project view's Tasks panel, drawn exactly as it is today: the exact DOM of the panel (the
// `data-help="project-tasks"` block of the REAL ProjectView, mounted whole), in three states. A
// behaviour-neutral move of the Tasks code, the kit's `ui` closure or the theme context (the Mods kit,
// PR C1 K6) must leave every attribute, class and node of it as it was: this file is the proof, and
// its expected DOM (src/components/__dom__/) is written once, from the code before the move. It uses the REAL ThemeProvider (no
// `useTheme` stub), so it also holds that core and the kit share one theme context.
//
// IPC is mocked whole-module: every export a view may call answers with an empty list or nothing,
// and the few the Tasks panel reads answer with the fixtures below.

import { cleanup, render, screen } from "@testing-library/react";
import type { ReactElement } from "react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { Task, TaskStack } from "../lib/types";

const data = vi.hoisted(() => ({ tasks: [] as unknown[], stacks: [] as unknown[] }));

vi.mock("../lib/ipc", () => {
  const known: Record<string, unknown> = {
    listTasks: vi.fn(async () => data.tasks),
    listTaskStacks: vi.fn(async () => data.stacks),
    onTasksChanged: vi.fn(async () => () => {}),
    getPref: vi.fn(async () => null),
    setPref: vi.fn(async () => {}),
    vaultStatus: vi.fn(async () => null),
  };
  // Any other command a mounted view asks for answers with an empty list; a subscription (`onX`)
  // answers with an unlisten function.
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

import { ProjectView } from "./ProjectView";
import { ReaderProvider } from "../lib/reader";
import { CapabilityProvider } from "../lib/capabilities";
import { ThemeProvider } from "../theme";
import type { ProjectChat } from "../lib/useProjectChat";

// Wednesday 04-11-2026, 10:00 local: the deadlines below read the same on every run.
const NOW = new Date(2026, 10, 4, 10, 0);

function task(over: Partial<Task> = {}): Task {
  return {
    id: 1,
    title: "Write the pitch",
    why: null,
    project_name: "Atlas",
    project_archived: false,
    milestone_id: null,
    event_uid: null,
    do_start: null,
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
    ordinal: 0,
    source: "user",
    created_at: "2026-11-01T00:00:00Z",
    updated_at: "2026-11-01T00:00:00Z",
    madeBy: null,
    ...over,
  };
}

const stack: TaskStack = {
  id: 7,
  title: "Visa",
  kind: "sequential",
  note: null,
  created_at: "",
  updated_at: "",
};

const FIXTURE: Task[] = [
  task({
    id: 1,
    title: "Write the pitch",
    why: "Because the deck is due",
    deadline: "2026-11-06",
    effective_deadline: "2026-11-06",
    ordinal: 0,
  }),
  task({ id: 2, title: "Call the bank", status: "doing", ordinal: 1, lane: "background" }),
  task({
    id: 3,
    title: "Book the venue",
    deadline: "2026-11-02",
    effective_deadline: "2026-11-02",
    deadline_kind: "soft",
    ordinal: 2,
  }),
  task({ id: 4, title: "Passport photo", stack_id: 7, ordinal: 3 }),
  task({ id: 5, title: "Shelved idea", parked: true, ordinal: 4 }),
  task({
    id: 6,
    title: "Send the invoice",
    status: "done",
    completed_at: "2026-11-03T09:00:00Z",
    ordinal: 5,
  }),
];

// The scoped chat session App owns, as an empty thread: only what ProjectView reads.
const chat = {
  convId: null,
  messages: [],
  prompts: [],
  confidences: [],
  providers: [],
  thoughts: [],
  thoughtFold: {},
  streaming: null,
  streamingThought: null,
  error: null,
  fallback: null,
  sending: false,
  dismissedIdleFor: null,
  queue: {
    queued: [],
    stalled: false,
    failedId: null,
    full: false,
    remove: () => {},
    edit: () => {},
    hold: () => {},
    resume: () => {},
    enqueue: () => {},
  },
  setError: () => {},
  setDismissedIdleFor: () => {},
  dismissFallback: () => {},
  newChat: () => {},
  noteLiveFold: () => {},
  noteThoughtFold: () => {},
} as unknown as ProjectChat;

function mount(ui: ReactElement) {
  return render(
    <ThemeProvider>
      <CapabilityProvider>
        <ReaderProvider view="project:Atlas">{ui}</ReaderProvider>
      </CapabilityProvider>
    </ThemeProvider>,
  );
}

async function panel(): Promise<HTMLElement> {
  // `findByText("Tasks")` is the panel's own label; the panel is its parent block.
  const label = await screen.findByText("Tasks");
  const block = label.closest('[data-help="project-tasks"]');
  if (!(block instanceof HTMLElement)) throw new Error("the Tasks panel did not draw");
  return block;
}

beforeAll(() => {
  if (!("ResizeObserver" in globalThis)) {
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
  Element.prototype.scrollIntoView ??= () => {};
});

beforeEach(() => {
  localStorage.clear();
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(NOW);
  data.tasks = [];
  data.stacks = [];
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

function view() {
  return mount(
    <ProjectView
      project="Atlas"
      chat={chat}
      localAi={null}
      onUpgrade={() => {}}
      onBack={() => {}}
    />,
  );
}

describe("the Project view's Tasks panel (exact DOM)", () => {
  it("a project with tasks: open, parked, done, a stack, a deadline and a why", async () => {
    data.tasks = FIXTURE;
    data.stacks = [stack];
    view();
    await screen.findByText("Write the pitch");
    await screen.findByText("Call the bank");
    await expect(await panel()).toMatchFileSnapshot(
      "./__dom__/projectTasksPanel.withTasks.dom.txt",
    );
  });

  it("a project with no tasks yet", async () => {
    view();
    await screen.findByText(/No tasks/i);
    await expect(await panel()).toMatchFileSnapshot("./__dom__/projectTasksPanel.empty.dom.txt");
  });
});
