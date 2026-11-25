// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The kit runs in a mod's sealed frame, which has NO `localStorage` or `sessionStorage` (the sandbox
// gives the page an opaque origin, so reading either one throws). Every kit component has to draw with
// its defaults there: this mounts each one the kit exports with both replaced by throwing getters, and
// asserts it renders. A list of what the kit exports is checked against the list mounted here, so a
// component added to src/kit/index.ts and not to this test fails it. (Mods K7 repeats it for the
// Pinboard mod itself.)

import { cleanup, render, screen } from "@testing-library/react";
import type { ReactElement } from "react";
import type { Task, TaskStack } from "../lib/types";
import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import * as kit from "./index";
import {
  BulletIcon,
  Button,
  Callout,
  ClearFormatIcon,
  Collapsible,
  ConfirmDialog,
  ContextMenuHost,
  DateField,
  DeadlineChip,
  Dialog,
  Input,
  LinkIcon,
  Modal,
  MonthPicker,
  NumberIcon,
  Popover,
  ProjectChip,
  SegmentedControl,
  Select,
  StackChip,
  TaskDataProvider,
  TaskList,
  TaskStatusButton,
  ThemeProvider,
  Toggle,
  Tooltip,
  VisuallyHidden,
  scrollBehavior,
  prefersReducedMotion,
  useContextMenu,
  useDepth,
  useTheme,
  type TaskData,
  type ThemeStore,
} from "./index";

/** Replaces both storages with getters that throw, as a frame's does; counts how often they were asked. */
function throwingStorage() {
  const saved = new Map<object, Map<string, PropertyDescriptor | undefined>>();
  const asked = { count: 0 };
  for (const target of [globalThis, window] as object[]) {
    const was = new Map<string, PropertyDescriptor | undefined>();
    for (const name of ["localStorage", "sessionStorage"]) {
      was.set(name, Object.getOwnPropertyDescriptor(target, name));
      Object.defineProperty(target, name, {
        configurable: true,
        get() {
          asked.count++;
          throw new DOMException("The operation is insecure.", "SecurityError");
        },
      });
    }
    saved.set(target, was);
  }
  return {
    asked,
    restore() {
      for (const [target, was] of saved) {
        for (const [name, descriptor] of was) {
          if (descriptor) Object.defineProperty(target, name, descriptor);
          else delete (target as Record<string, unknown>)[name];
        }
      }
    },
  };
}

// No stored theme anywhere: what a frame's store answers before the host's theme is known.
const emptyStore: ThemeStore = { get: async () => null, set: async () => {} };

const aTask = {
  id: 1,
  title: "task title one",
  why: null,
  project_name: "Atlas",
  project_archived: false,
  milestone_id: null,
  event_uid: null,
  do_start: null,
  do_end: null,
  deadline: "2026-11-30",
  deadline_kind: "hard",
  deadline_from: "date",
  effective_deadline: "2026-11-30",
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
  created_at: "",
  updated_at: "",
} as Task;
const aStack: TaskStack = {
  id: 3,
  title: "stack title",
  kind: "waiting_on",
  note: null,
  created_at: "",
  updated_at: "",
};
const taskData: TaskData = {
  list: async () => ({ tasks: [aTask], stacks: [aStack] }),
  add: async () => 1,
  setStatus: async () => {},
  reorder: async () => {},
  onChanged: async () => () => {},
  edit: () => {},
};

/** The hooks, read the way a component reads them. */
function Probe() {
  const { system, mode, depth } = useTheme();
  const { showMeta } = useDepth();
  const menu = useContextMenu();
  return (
    <p>
      probe:{system}:{mode}:{depth}:{String(showMeta)}:{String(menu)}:
      {String(typeof scrollBehavior())}:{String(typeof prefersReducedMotion())}
    </p>
  );
}

/** Every component the kit exports, drawn the plainest way it can be, and the text that proves it drew. */
const COMPONENTS: Record<string, [ReactElement, string | RegExp]> = {
  Button: [<Button key="b">button text</Button>, "button text"],
  Callout: [<Callout key="c">callout text</Callout>, "callout text"],
  Input: [<Input key="i" aria-label="input name" />, /./],
  Select: [
    <Select key="s" aria-label="select name">
      <option>option text</option>
    </Select>,
    "option text",
  ],
  SegmentedControl: [
    <SegmentedControl
      key="sc"
      ariaLabel="seg name"
      value="a"
      onChange={() => {}}
      options={[{ value: "a", label: "alpha text" }]}
    />,
    "alpha text",
  ],
  Toggle: [<Toggle key="tg" ariaLabel="toggle name" checked onChange={() => {}} />, /./],
  Collapsible: [
    <Collapsible key="co" title="fold title">
      fold body
    </Collapsible>,
    "fold body",
  ],
  Modal: [
    <Modal key="m" open onClose={() => {}} label="modal name">
      modal body
    </Modal>,
    "modal body",
  ],
  Dialog: [
    <Dialog key="d" open onClose={() => {}} title="dialog title">
      dialog body
    </Dialog>,
    "dialog body",
  ],
  ConfirmDialog: [
    <ConfirmDialog key="cd" open title="confirm title" onConfirm={() => {}} onClose={() => {}}>
      confirm body
    </ConfirmDialog>,
    "confirm body",
  ],
  Tooltip: [
    <Tooltip key="t" label="tip">
      <button>tooltip trigger</button>
    </Tooltip>,
    "tooltip trigger",
  ],
  Popover: [
    <Popover
      key="p"
      open
      ariaLabel="pop name"
      trigger={({ toggle }) => <button onClick={toggle}>popover trigger</button>}
    >
      popover body
    </Popover>,
    "popover body",
  ],
  VisuallyHidden: [<VisuallyHidden key="v">hidden text</VisuallyHidden>, "hidden text"],
  BulletIcon: [<BulletIcon key="bi" />, /./],
  NumberIcon: [<NumberIcon key="ni" />, /./],
  LinkIcon: [<LinkIcon key="li" />, /./],
  ClearFormatIcon: [<ClearFormatIcon key="ci" />, /./],
  DateField: [<DateField key="df" value="" onCommit={() => {}} ariaLabel="date name" />, /./],
  MonthPicker: [
    <MonthPicker
      key="mp"
      selected={new Date(2026, 10, 4)}
      onPick={() => {}}
      isDisabled={() => false}
    />,
    /./,
  ],
  ContextMenuHost: [<ContextMenuHost key="cm" />, /./],
  TaskDataProvider: [
    <TaskDataProvider key="tp" data={taskData}>
      provider child
    </TaskDataProvider>,
    "provider child",
  ],
  TaskList: [
    <TaskDataProvider key="tl" data={taskData}>
      <TaskList project="Atlas" />
    </TaskDataProvider>,
    /./,
  ],
  TaskStatusButton: [<TaskStatusButton key="ts" status="todo" onCycle={() => {}} />, /./],
  DeadlineChip: [<DeadlineChip key="dc" task={aTask} today={new Date(2026, 10, 24)} />, /./],
  StackChip: [<StackChip key="sk" stack={aStack} />, /stack title/],
  ProjectChip: [<ProjectChip key="pc" project="Atlas" />, "Atlas"],
  Probe: [<Probe key="pr" />, "probe:slate:dark:standard:true:null:string:boolean"],
};

/** The components among the kit's exports: a capitalised function. */
const exported = Object.entries(kit)
  .filter(([name, value]) => typeof value === "function" && /^[A-Z]/.test(name))
  .map(([name]) => name);

let storage: ReturnType<typeof throwingStorage>;

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
  storage = throwingStorage();
});
afterEach(() => {
  cleanup();
  storage.restore();
});

describe("the kit in a frame with no localStorage or sessionStorage", () => {
  it("mounts every component the kit exports (and this list is that list)", () => {
    // ThemeProvider wraps every mount below, and has its own test at the end.
    expect([...Object.keys(COMPONENTS), "ThemeProvider"].sort()).toEqual(
      [...exported, "Probe"].sort(),
    );
  });

  it("really has throwing storage", () => {
    expect(() => localStorage.getItem("x")).toThrow();
    expect(() => sessionStorage.getItem("x")).toThrow();
    expect(() => window.localStorage).toThrow();
  });

  it.each(Object.entries(COMPONENTS))("%s renders with its defaults", (_name, [element, text]) => {
    render(<ThemeProvider store={emptyStore}>{element}</ThemeProvider>);
    if (typeof text === "string") expect(screen.getByText(text)).toBeTruthy();
    else expect(document.body.innerHTML.length).toBeGreaterThan(0);
  });

  it("ThemeProvider itself reads and writes nothing it cannot, and themes the page with the defaults", () => {
    render(
      <ThemeProvider store={emptyStore}>
        <Probe />
      </ThemeProvider>,
    );
    // It did ask for the storage (the theme's read goes through it), and survived the refusal.
    expect(storage.asked.count).toBeGreaterThan(0);
    expect(document.documentElement.style.length).toBeGreaterThan(0);
    expect(screen.getByText(/probe:slate:dark:standard/)).toBeTruthy();
  });
});
