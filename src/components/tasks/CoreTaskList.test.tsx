// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// core's wiring of the kit's task list: `CoreTaskList` supplies the `TaskData` that reads through
// `lib/ipc` (tasks and stacks together, a failed stack read tolerated), writes through the one status
// command, the reorder command and `addTask(emptyTaskInput(..))`, re-reads on `tasks://changed`, and
// draws core's own `TaskEditor` for "edit" and "More…". (The list's own behaviour is the kit's test,
// src/kit/tasks/TaskList.test.tsx, over a mocked TaskData.)

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Task } from "../../lib/types";
import { CoreTaskList } from "./CoreTaskList";

let tasksChanged: (() => void) | null = null;

vi.mock("../../lib/ipc", () => ({
  addTask: vi.fn(async () => 99),
  addTaskStack: vi.fn(async () => 1),
  changeTaskStatus: vi.fn(async () => {}),
  deleteTask: vi.fn(async () => {}),
  listAllCalendarEvents: vi.fn(async () => []),
  listAllMilestones: vi.fn(async () => []),
  listProjects: vi.fn(async () => []),
  listTasks: vi.fn(async () => []),
  listTaskStacks: vi.fn(async () => []),
  onTasksChanged: vi.fn(async (handler: () => void) => {
    tasksChanged = handler;
    return () => {
      tasksChanged = null;
    };
  }),
  reorderTasks: vi.fn(async () => {}),
  updateTask: vi.fn(async () => {}),
}));

let depth = "standard";
vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal()),
  useTheme: () => ({
    system: "slate",
    mode: "dark",
    modePref: "system",
    modeSource: "system",
    accent: "mono",
    depth,
    autoLocation: "",
    teachVisible: true,
    setSystem: () => {},
    setModePref: () => {},
    setAccent: () => {},
    setDepth: () => {},
    setAutoLocation: () => {},
    setTeachVisible: () => {},
  }),
}));

import * as ipc from "../../lib/ipc";

const mocked = vi.mocked(ipc);

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
    created_at: "",
    updated_at: "",
    ...over,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  depth = "standard";
  tasksChanged = null;
  // clearAllMocks keeps implementations: start each test from an empty store.
  mocked.listTasks.mockResolvedValue([]);
  mocked.listTaskStacks.mockResolvedValue([]);
});
afterEach(cleanup);

describe("CoreTaskList", () => {
  it("lists the tasks and stacks ipc returns, scoped to the project", async () => {
    mocked.listTasks.mockResolvedValue([
      task({ id: 2, title: "Second" }),
      task({ id: 1, title: "First" }),
    ]);
    mocked.listTaskStacks.mockResolvedValue([]);
    render(<CoreTaskList project="Atlas" />);
    await screen.findByText("Second");
    expect(mocked.listTasks).toHaveBeenCalledWith("Atlas");
    expect(mocked.listTaskStacks).toHaveBeenCalled();
  });

  it("lists every task for a list with no project", async () => {
    mocked.listTasks.mockResolvedValue([task({ title: "Loose" })]);
    render(<CoreTaskList showProject />);
    await screen.findByText("Loose");
    expect(mocked.listTasks).toHaveBeenCalledWith(null);
  });

  it("puts the stacks ipc returns on their tasks", async () => {
    mocked.listTasks.mockResolvedValue([task({ stack_id: 3 })]);
    mocked.listTaskStacks.mockResolvedValue([
      {
        id: 3,
        title: "Visa paperwork",
        kind: "waiting_on",
        note: null,
        created_at: "",
        updated_at: "",
      },
    ]);
    render(<CoreTaskList project="Atlas" />);
    expect(await screen.findByText(/Visa paperwork/)).toBeTruthy();
  });

  it("still lists the tasks when the stacks cannot be read", async () => {
    mocked.listTasks.mockResolvedValue([task({ title: "Still here" })]);
    mocked.listTaskStacks.mockRejectedValueOnce("no stacks table");
    render(<CoreTaskList project="Atlas" />);
    expect(await screen.findByText("Still here")).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("shows a refused task read", async () => {
    mocked.listTasks.mockRejectedValueOnce("database is locked");
    render(<CoreTaskList project="Atlas" />);
    expect((await screen.findByRole("alert")).textContent).toContain("database is locked");
  });

  it("cycles a task's status through the one status command", async () => {
    mocked.listTasks.mockResolvedValue([task({ id: 7, status: "doing" })]);
    render(<CoreTaskList project="Atlas" />);
    fireEvent.click(await screen.findByRole("button", { name: "Doing: mark as done" }));
    await waitFor(() => expect(mocked.changeTaskStatus).toHaveBeenCalledWith(7, "done"));
  });

  it("reorders through the reorder command", async () => {
    mocked.listTasks.mockResolvedValue([
      task({ id: 1, title: "A", ordinal: 0 }),
      task({ id: 2, title: "B", ordinal: 1 }),
      task({ id: 3, title: "C", ordinal: 2 }),
    ]);
    render(<CoreTaskList project="Atlas" />);
    await screen.findByText("C");
    fireEvent.click(screen.getByRole("button", { name: "Move B down" }));
    await waitFor(() => expect(mocked.reorderTasks).toHaveBeenCalledWith([1, 3, 2]));
  });

  it("re-reads when any surface changes a task", async () => {
    mocked.listTasks.mockResolvedValueOnce([task({ title: "Before" })]);
    render(<CoreTaskList project="Atlas" />);
    await screen.findByText("Before");
    mocked.listTasks.mockResolvedValueOnce([task({ title: "After" })]);
    await waitFor(() => expect(tasksChanged).not.toBeNull());
    act(() => tasksChanged?.());
    expect(await screen.findByText("After")).toBeTruthy();
  });

  it("adds a task as a whole editor input with the list's project", async () => {
    mocked.listTasks.mockResolvedValue([]);
    render(<CoreTaskList project="Atlas" />);
    await screen.findByText("No tasks for this project yet. Add one below.");
    const box = screen.getByRole("textbox", { name: "New task" });
    fireEvent.change(box, { target: { value: "Call the venue" } });
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(mocked.addTask).toHaveBeenCalled());
    // `addTask` takes the full input: the list's three fields laid over a blank one.
    expect(mocked.addTask).toHaveBeenCalledWith({
      title: "Call the venue",
      why: null,
      project: "Atlas",
      milestoneId: null,
      eventUid: null,
      doStart: null,
      doEnd: null,
      deadline: null,
      deadlineKind: "hard",
      deadlineFrom: "date",
      lane: "foreground",
      stackId: null,
      parked: false,
    });
    await waitFor(() => expect((box as HTMLInputElement).value).toBe(""));
  });

  it("clears the quick-add row once More… has made the task, and keeps it if cancelled", async () => {
    mocked.listTasks.mockResolvedValue([]);
    render(<CoreTaskList project="Atlas" />);
    await screen.findByText("No tasks for this project yet. Add one below.");
    const typed = () => screen.getByRole("textbox", { name: "New task" }) as HTMLInputElement;
    fireEvent.change(typed(), { target: { value: "Call the venue" } });

    // Cancelled: what was typed is still there.
    fireEvent.click(screen.getByRole("button", { name: "More…" }));
    fireEvent.click(
      within(screen.getByRole("dialog", { name: "New task" })).getByRole("button", {
        name: "Cancel",
      }),
    );
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "New task" })).toBeNull());
    expect(typed().value).toBe("Call the venue");

    // Added: the row starts empty again.
    fireEvent.click(screen.getByRole("button", { name: "More…" }));
    const dialog = screen.getByRole("dialog", { name: "New task" });
    expect((within(dialog).getByLabelText("Title") as HTMLInputElement).value).toBe(
      "Call the venue",
    );
    fireEvent.click(within(dialog).getByRole("button", { name: "Add task" }));
    await waitFor(() => expect(mocked.addTask).toHaveBeenCalled());
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "New task" })).toBeNull());
    expect(typed().value).toBe("");
  });

  it("opens core's editor from a row, and re-reads once it saved", async () => {
    mocked.listTasks.mockResolvedValue([task()]);
    render(<CoreTaskList project="Atlas" />);
    fireEvent.click(await screen.findByRole("button", { name: "Edit Write the pitch" }));
    const dialog = await screen.findByRole("dialog", { name: "Edit task" });
    const reads = mocked.listTasks.mock.calls.length;
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(mocked.updateTask).toHaveBeenCalled());
    await waitFor(() => expect(mocked.listTasks.mock.calls.length).toBeGreaterThan(reads));
  });
});
