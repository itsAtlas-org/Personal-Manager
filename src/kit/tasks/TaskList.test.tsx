// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The live task list (the kit's, mounted over a mocked `TaskData`, never `lib/ipc`): it re-reads when the
// data says a task changed (any surface's write), hides archived projects' tasks unless scoped to one,
// folds parked and done tasks away, writes through the one status call, reorders within the open list,
// asks the data to open the editor (which core draws, and a mod frame asks PM to draw), and turns to a
// plain summary below Standard depth. Mounted with no `TaskData` it throws a named error.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Task, TaskStack } from "../../lib/types";
import { TaskDataMissingError, TaskDataProvider, type TaskData } from "../taskData";
import { TaskList, type TaskListProps } from "./TaskList";

let tasksChanged: (() => void) | null = null;

let depth = "standard";
vi.mock("../theme", async (importOriginal) => ({
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

const data = {
  list: vi.fn<TaskData["list"]>(async () => ({ tasks: [], stacks: [] })),
  add: vi.fn<TaskData["add"]>(async () => 99),
  setStatus: vi.fn<TaskData["setStatus"]>(async () => {}),
  reorder: vi.fn<TaskData["reorder"]>(async () => {}),
  onChanged: vi.fn<TaskData["onChanged"]>(async (handler) => {
    tasksChanged = handler;
    return () => {
      tasksChanged = null;
    };
  }),
  edit: vi.fn<TaskData["edit"]>(),
};

/** The list over the mocked data. `data` is one stable object, as a provider's must be. */
function List(props: TaskListProps) {
  return (
    <TaskDataProvider data={data}>
      <TaskList {...props} />
    </TaskDataProvider>
  );
}

function listing(tasks: Task[], stacks: TaskStack[] = []) {
  return { tasks, stacks };
}

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
    madeBy: null,
    ...over,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  depth = "standard";
  tasksChanged = null;
});
afterEach(cleanup);

describe("TaskList", () => {
  it("lists open tasks in order and folds parked and done ones away", async () => {
    data.list.mockResolvedValue(
      listing([
        task({ id: 2, title: "Second", ordinal: 1 }),
        task({ id: 1, title: "First", ordinal: 0, why: "Because the deck is due" }),
        task({ id: 3, title: "Shelved", parked: true }),
        task({ id: 4, title: "Shipped", status: "done", completed_at: "2026-11-01T00:00:00Z" }),
      ]),
    );
    render(<List project="Atlas" />);
    await screen.findByText("Second");
    const titles = screen
      .getAllByRole("button", { name: /^Edit / })
      .map((b) => b.getAttribute("aria-label"));
    expect(titles.slice(0, 2)).toEqual(["Edit First", "Edit Second"]);
    expect(screen.getByRole("button", { name: /Parked/ }).getAttribute("aria-expanded")).toBe(
      "false",
    );
    expect(screen.getByRole("button", { name: /Done \(1\)/ }).getAttribute("aria-expanded")).toBe(
      "false",
    );
    expect(data.list).toHaveBeenCalledWith("Atlas");

    // The title toggles its note.
    expect(screen.queryByText("Because the deck is due")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "First" }));
    expect(screen.getByText("Because the deck is due")).toBeTruthy();
  });

  it("marks a task a mod made with who and its tier word, and leaves the user's own bare (K5)", async () => {
    data.list.mockResolvedValue(
      listing([
        task({ id: 1, title: "Mine" }),
        task({ id: 2, title: "Gifted", madeBy: { name: "Pinboard", tier: "first", signed: true } }),
        task({ id: 3, title: "Forged", madeBy: { name: "Atlas", tier: "first", signed: false } }),
      ]),
    );
    const { container } = render(<List project="Atlas" />);
    await screen.findByText("Gifted");
    const chips = [...container.querySelectorAll("[title^='Made by']")].map((c) => c.textContent);
    expect(chips).toEqual(["Pinboard· first party", "Atlas· unsigned"]);
  });

  it("hides archived projects' tasks in an all-tasks list, unless asked", async () => {
    data.list.mockResolvedValue(
      listing([
        task({ id: 1, title: "Live one" }),
        task({ id: 2, title: "Old one", project_name: "Old", project_archived: true }),
      ]),
    );
    const { unmount } = render(<List showProject />);
    await screen.findByText("Live one");
    expect(screen.queryByText("Old one")).toBeNull();
    expect(data.list).toHaveBeenCalledWith(null);
    expect(screen.getByText("Atlas")).toBeTruthy();
    unmount();

    render(<List includeArchived />);
    expect(await screen.findByText("Old one")).toBeTruthy();
  });

  it("cycles a task's status through the one status command", async () => {
    data.list.mockResolvedValue(listing([task({ id: 7, status: "doing" })]));
    render(<List project="Atlas" />);
    const tick = await screen.findByRole("button", { name: "Doing: mark as done" });
    fireEvent.click(tick);
    await waitFor(() => expect(data.setStatus).toHaveBeenCalledWith(7, "done"));
  });

  it("re-reads when any surface changes a task", async () => {
    data.list.mockResolvedValueOnce(listing([task({ title: "Before" })]));
    render(<List project="Atlas" />);
    await screen.findByText("Before");
    data.list.mockResolvedValueOnce(listing([task({ title: "After" })]));
    await waitFor(() => expect(tasksChanged).not.toBeNull());
    act(() => tasksChanged?.());
    expect(await screen.findByText("After")).toBeTruthy();
  });

  it("reorders within the open list", async () => {
    data.list.mockResolvedValue(
      listing([
        task({ id: 1, title: "A", ordinal: 0 }),
        task({ id: 2, title: "B", ordinal: 1 }),
        task({ id: 3, title: "C", ordinal: 2 }),
      ]),
    );
    render(<List project="Atlas" />);
    await screen.findByText("C");
    expect(screen.getByRole("button", { name: "Move A up" }).hasAttribute("disabled")).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Move B down" }));
    await waitFor(() => expect(data.reorder).toHaveBeenCalledWith([1, 3, 2]));
  });

  it("adds a task with the list's project on Enter", async () => {
    data.list.mockResolvedValue(listing([]));
    render(<List project="Atlas" />);
    expect(await screen.findByText("No tasks for this project yet. Add one below.")).toBeTruthy();
    const box = screen.getByRole("textbox", { name: "New task" });
    fireEvent.change(box, { target: { value: "Call the venue" } });
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(data.add).toHaveBeenCalled());
    expect(data.add).toHaveBeenCalledWith({
      title: "Call the venue",
      project: "Atlas",
      deadline: null,
    });
    await waitFor(() => expect((box as HTMLInputElement).value).toBe(""));
  });

  it("clears the quick-add row once More… has made the task, and keeps it if cancelled", async () => {
    data.list.mockResolvedValue(listing([]));
    render(<List project="Atlas" />);
    await screen.findByText("No tasks for this project yet. Add one below.");
    const typed = () => screen.getByRole("textbox", { name: "New task" }) as HTMLInputElement;
    fireEvent.change(typed(), { target: { value: "Call the venue" } });

    // The list hands the editor what was typed, and the project, and a way to say it saved. The
    // editor itself is the data's to draw: it is not a part of the kit.
    fireEvent.click(screen.getByRole("button", { name: "More…" }));
    expect(data.edit).toHaveBeenCalledTimes(1);
    expect(data.edit.mock.calls[0][0]).toEqual({
      defaults: { project: "Atlas", title: "Call the venue", deadline: null },
    });

    // Cancelled (the editor never says it is done): what was typed is still there.
    expect(typed().value).toBe("Call the venue");

    // Added: the editor says it saved, and the row starts empty again.
    fireEvent.click(screen.getByRole("button", { name: "More…" }));
    expect(data.edit).toHaveBeenCalledTimes(2);
    const reads = data.list.mock.calls.length;
    act(() => data.edit.mock.calls[1][1]());
    await waitFor(() => expect(typed().value).toBe(""));
    // ...and the list re-reads.
    await waitFor(() => expect(data.list.mock.calls.length).toBeGreaterThan(reads));
  });

  it("offers a sort only where the host keeps one, and applies it to open and parked tasks", async () => {
    data.list.mockResolvedValue(
      listing([
        task({ id: 1, title: "Banana", ordinal: 0 }),
        task({ id: 2, title: "apple", ordinal: 1 }),
        task({ id: 3, title: "Zebra", ordinal: 2, parked: true }),
        task({ id: 4, title: "Mango", ordinal: 3, parked: true }),
      ]),
    );
    const onSortChange = vi.fn();
    const { rerender } = render(<List project="Atlas" />);
    await screen.findByText("Banana");
    expect(screen.queryByRole("combobox", { name: "Sort tasks by" })).toBeNull();

    rerender(
      <List project="Atlas" sort={{ by: "manual", desc: false }} onSortChange={onSortChange} />,
    );
    fireEvent.change(screen.getByRole("combobox", { name: "Sort tasks by" }), {
      target: { value: "alpha" },
    });
    expect(onSortChange).toHaveBeenCalledWith({ by: "alpha", desc: false });

    rerender(
      <List project="Atlas" sort={{ by: "alpha", desc: true }} onSortChange={onSortChange} />,
    );
    const order = () =>
      screen.getAllByRole("button", { name: /^Edit / }).map((b) => b.getAttribute("aria-label"));
    expect(order()).toEqual(["Edit Banana", "Edit apple", "Edit Zebra", "Edit Mango"]);
    // Sorted by anything but My order, there are no arrows to move a task with.
    expect(screen.queryByRole("button", { name: "Move apple up" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /^Reverse the order/ }));
    expect(onSortChange).toHaveBeenLastCalledWith({ by: "alpha", desc: false });
  });

  it("reorders a reversed My order the right way up", async () => {
    data.list.mockResolvedValue(
      listing([
        task({ id: 1, title: "A", ordinal: 0 }),
        task({ id: 2, title: "B", ordinal: 1 }),
        task({ id: 3, title: "C", ordinal: 2 }),
      ]),
    );
    render(<List project="Atlas" sort={{ by: "manual", desc: true }} onSortChange={() => {}} />);
    await screen.findByText("C");
    // Shown C, B, A. Moving C down shows B, C, A: the global order A, C, B.
    fireEvent.click(screen.getByRole("button", { name: "Move C down" }));
    await waitFor(() => expect(data.reorder).toHaveBeenCalledWith([1, 3, 2]));
  });

  it("shows a refused write", async () => {
    data.list.mockResolvedValue(listing([task()]));
    data.setStatus.mockRejectedValueOnce("database is locked");
    render(<List project="Atlas" />);
    fireEvent.click(await screen.findByRole("button", { name: "To do: mark as doing" }));
    expect((await screen.findByRole("alert")).textContent).toContain("database is locked");
  });

  it("asks the data to open the editor from a row, and re-reads once it saved or deleted", async () => {
    const t = task();
    data.list.mockResolvedValue(listing([t]));
    render(<List project="Atlas" />);
    fireEvent.click(await screen.findByRole("button", { name: "Edit Write the pitch" }));
    expect(data.edit).toHaveBeenCalledTimes(1);
    expect(data.edit.mock.calls[0][0]).toEqual({ task: t });
    // An edit of an existing task leaves the quick-add row alone.
    const box = screen.getByRole("textbox", { name: "New task" }) as HTMLInputElement;
    fireEvent.change(box, { target: { value: "Half typed" } });
    const reads = data.list.mock.calls.length;
    act(() => data.edit.mock.calls[0][1]());
    await waitFor(() => expect(data.list.mock.calls.length).toBeGreaterThan(reads));
    expect((screen.getByRole("textbox", { name: "New task" }) as HTMLInputElement).value).toBe(
      "Half typed",
    );
  });

  it("shows the stacks the data lists on their tasks", async () => {
    const stack: TaskStack = {
      id: 3,
      title: "Visa paperwork",
      kind: "waiting_on",
      note: null,
      created_at: "",
      updated_at: "",
    };
    data.list.mockResolvedValue(listing([task({ stack_id: 3 })], [stack]));
    render(<List project="Atlas" />);
    expect(await screen.findByText(/Visa paperwork/)).toBeTruthy();
  });

  it("throws a named error, not an empty list, when mounted with no TaskData", () => {
    const quiet = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      expect(() => render(<TaskList project="Atlas" />)).toThrow(TaskDataMissingError);
      expect(() => render(<TaskList project="Atlas" />)).toThrow(/TaskDataMissingError/);
    } finally {
      quiet.mockRestore();
    }
  });

  it("is a plain summary below Standard depth", async () => {
    depth = "min";
    data.list.mockResolvedValue(
      listing([
        task({ id: 1, title: "Open one" }),
        task({ id: 2, title: "Closed one", status: "done" }),
      ]),
    );
    render(<List project="Atlas" />);
    expect(await screen.findByText("Open one")).toBeTruthy();
    expect(screen.getByText("Closed one").className).toContain("line-through");
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.queryByRole("textbox")).toBeNull();
  });
});
