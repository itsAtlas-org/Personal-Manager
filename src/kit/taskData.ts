// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The task seam of the mod kit (Mods spec D14, PR C plan K6a). The kit's `TaskList` reads and writes
// tasks only through this interface, so it holds no IPC and no Host call of its own: core's
// `CoreTaskList` (src/components/tasks/) supplies one that calls `lib/ipc` and draws core's own
// `TaskEditor`; a mod frame supplies one that calls the Host API (`tasks.*`) and asks PM to open the
// editor (`ui.openTaskEditor`). The editor, the stack editor and the event picker stay in core: they
// need calls (update, delete, stacks) a mod is not given.
//
// A list mounted with no provider throws `TaskDataMissingError` in render, loudly, rather than
// showing an empty list that looks like "no tasks".

import { createContext, createElement, useContext, type ReactNode } from "react";
import type { Task, TaskInput, TaskStack, TaskStatus } from "../lib/types";
import type { Unlisten } from "./lib/subscribe";

/** What a quick-add (and "More…") can seed: a title, a project, a deadline. */
export type NewTask = Pick<TaskInput, "title" | "project" | "deadline">;

/** What the list asks to edit: an existing task, or a new one starting from `defaults`. */
export type TaskEditTarget = { task: Task } | { defaults: Partial<NewTask> };

export interface TaskData {
  /** One project's tasks (null: every task) and every stack. A stack list that cannot be read is an
   *  empty one; a task list that cannot be read rejects. */
  list(project: string | null): Promise<{ tasks: Task[]; stacks: TaskStack[] }>;
  add(input: NewTask): Promise<unknown>;
  setStatus(id: number, status: TaskStatus): Promise<unknown>;
  /** Every open task's id in the one global order (the list sends the whole order back). */
  reorder(ids: number[]): Promise<unknown>;
  /** `cb` runs after any task write, from any surface; resolves to the way to stop listening. */
  onChanged(cb: () => void): Promise<Unlisten>;
  /** Open the editor for `target`. `done` is called once the editor SAVED or DELETED a task (the list
   *  re-reads, and a quick-add row that became a task starts empty); a cancel does not call it. */
  edit(target: TaskEditTarget, done: () => void): void;
}

/** Thrown in render by anything that needs a `TaskData` when no provider is above it. */
export class TaskDataMissingError extends Error {
  constructor() {
    super(
      "TaskDataMissingError: a TaskList needs a TaskData from a TaskDataProvider above it (core: CoreTaskList; a mod: its frame adapter).",
    );
    this.name = "TaskDataMissingError";
  }
}

const TaskDataContext = createContext<TaskData | null>(null);

/** Supplies the `TaskData` every kit task component below it reads and writes through. Pass a stable
 *  object (memoise it): a new one makes the list re-read. */
export function TaskDataProvider({ data, children }: { data: TaskData; children?: ReactNode }) {
  return createElement(TaskDataContext.Provider, { value: data }, children);
}

export function useTaskData(): TaskData {
  const data = useContext(TaskDataContext);
  if (data === null) throw new TaskDataMissingError();
  return data;
}
