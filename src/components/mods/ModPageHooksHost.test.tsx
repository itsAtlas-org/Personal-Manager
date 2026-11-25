// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// PM's own task editor for `ui.openTaskEditor` (K5): core's TaskEditor, mounted once, one at a time.

import { act, cleanup, render, screen } from "@testing-library/react";
import { useContext } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ModPageHooksContext, type ModPageHooks, type ModSource } from "./modPageHooks";
import { ModPageHooksHost } from "./ModPageHooksHost";
import type { Task, TaskInput } from "../../lib/types";

const listTasks = vi.fn();
vi.mock("../../lib/ipc", () => ({
  listTasks: () => listTasks(),
}));

interface EditorProps {
  open: boolean;
  task?: Task | null;
  defaults?: Partial<TaskInput>;
  onClose: () => void;
}
const editors: EditorProps[] = [];
vi.mock("../tasks/TaskEditor", () => ({
  TaskEditor: (props: EditorProps) => {
    editors.push(props);
    return props.open ? <div role="dialog" aria-label="task editor" /> : null;
  },
}));

const SOURCE: ModSource = { modId: "acme.tool", name: "Acme Tool", tier: "second" };
let hooks: ModPageHooks = {};
function Probe() {
  hooks = useContext(ModPageHooksContext);
  return null;
}
const last = () => editors[editors.length - 1];

beforeEach(() => {
  editors.length = 0;
  listTasks.mockReset();
  render(
    <ModPageHooksContext.Provider value={{ showNotice: vi.fn() }}>
      <ModPageHooksHost onOpenEngineSetup={vi.fn()}>
        <Probe />
      </ModPageHooksHost>
    </ModPageHooksContext.Provider>,
  );
});
afterEach(cleanup);

const task = (id: number) => ({ id, title: `task ${id}` }) as Task;

describe("ModPageHooksHost", () => {
  it("keeps what the app above it provides, and adds the editor and the engine's way", () => {
    expect(hooks.showNotice).toBeTypeOf("function");
    expect(hooks.openTaskEditor).toBeTypeOf("function");
    expect(hooks.openEngineSetup).toBeTypeOf("function");
  });

  it("opens a new task on the text the mod prefilled, and only on what it gave", () => {
    act(() => {
      expect(
        hooks.openTaskEditor!(SOURCE, {
          new: { title: "Send the deck", project: "Atlas", deadline: "2026-12-01" },
        }),
      ).toBe(true);
    });
    expect(screen.getByRole("dialog", { name: "task editor" })).toBeTruthy();
    expect(last().open).toBe(true);
    expect(last().task).toBeNull();
    expect(last().defaults).toEqual({
      title: "Send the deck",
      project: "Atlas",
      deadline: "2026-12-01",
    });
    act(() => {
      last().onClose();
    });
    act(() => {
      hooks.openTaskEditor!(SOURCE, { new: { title: null, project: null, deadline: null } });
    });
    expect(last().defaults).toEqual({});
  });

  it("opens an existing task by id once it is read", async () => {
    listTasks.mockResolvedValue([task(3), task(7)]);
    await act(async () => {
      expect(hooks.openTaskEditor!(SOURCE, { id: 7 })).toBe(true);
    });
    expect(last().open).toBe(true);
    expect(last().task?.id).toBe(7);
  });

  it("is one at a time: a second request while one is open or loading is refused", async () => {
    let finish: (t: Task[]) => void = () => {};
    listTasks.mockReturnValue(new Promise<Task[]>((resolve) => (finish = resolve)));
    let second = true;
    let third = true;
    act(() => {
      hooks.openTaskEditor!(SOURCE, { id: 7 });
      // Still loading: the state says closed, the guard says busy.
      second = hooks.openTaskEditor!(SOURCE, { id: 8 });
    });
    expect(second).toBe(false);
    await act(async () => finish([task(7)]));
    act(() => {
      third = hooks.openTaskEditor!(SOURCE, { new: { title: "x", project: null, deadline: null } });
    });
    expect(third).toBe(false);
    // Closed, a request is taken again.
    act(() => {
      last().onClose();
    });
    let again = false;
    act(() => {
      again = hooks.openTaskEditor!(SOURCE, { new: { title: "y", project: null, deadline: null } });
    });
    expect(again).toBe(true);
  });

  it.each([
    ["an id that is no longer a task", () => listTasks.mockResolvedValue([task(1)])],
    ["a list that can't be read", () => listTasks.mockRejectedValue(new Error("locked"))],
  ])("opens nothing for %s, and lets the next request through", async (_name, arrange) => {
    arrange();
    await act(async () => {
      hooks.openTaskEditor!(SOURCE, { id: 99 });
    });
    expect(editors.every((e) => !e.open)).toBe(true);
    listTasks.mockResolvedValue([task(5)]);
    let again = false;
    await act(async () => {
      again = hooks.openTaskEditor!(SOURCE, { id: 5 });
    });
    expect(again).toBe(true);
    expect(last().task?.id).toBe(5);
  });
});
