// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The kit's task list (src/kit/tasks/TaskList.tsx) wired to core: its `TaskData` is the `lib/ipc` task
// calls, and "edit" opens core's own `TaskEditor` (which stays in core: it needs update, delete and
// stack calls a mod is not given). The Project view draws this; the Pinboard mod
// draws the kit's list with its own `TaskData` (Host API calls, and `ui.openTaskEditor` for edit).

import { useMemo, useState } from "react";
import { TaskDataProvider, type TaskData, type TaskEditTarget } from "../../kit/taskData";
import { TaskList, type TaskListProps } from "../../kit/tasks/TaskList";
import { emptyTaskInput } from "../../lib/tasks";
import {
  addTask,
  changeTaskStatus,
  listTasks,
  listTaskStacks,
  onTasksChanged,
  reorderTasks,
} from "../../lib/ipc";
import { TaskEditor } from "./TaskEditor";

/** What the open editor is for, and what to tell the list once it saved or deleted. */
interface Editing {
  target: TaskEditTarget;
  done: () => void;
}

export function CoreTaskList(props: TaskListProps) {
  const [editing, setEditing] = useState<Editing | null>(null);
  // One stable object: a new one would make the list re-read (it keys its subscription on it).
  const data = useMemo<TaskData>(
    () => ({
      list: async (project) => {
        // A stack list that cannot be read is an empty one; the tasks' own failure still shows.
        const stacks = listTaskStacks().catch(() => []);
        const tasks = await listTasks(project);
        return { tasks, stacks: await stacks };
      },
      add: (input) => addTask(emptyTaskInput(input)),
      setStatus: changeTaskStatus,
      reorder: reorderTasks,
      onChanged: onTasksChanged,
      edit: (target, done) => setEditing({ target, done }),
    }),
    [],
  );
  const target = editing?.target;
  return (
    <TaskDataProvider data={data}>
      <TaskList {...props} />
      <TaskEditor
        open={editing !== null}
        task={target && "task" in target ? target.task : null}
        defaults={target && "defaults" in target ? target.defaults : undefined}
        onClose={() => setEditing(null)}
        onSaved={() => editing?.done()}
        onDeleted={() => editing?.done()}
      />
    </TaskDataProvider>
  );
}
