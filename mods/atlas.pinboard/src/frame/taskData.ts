// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The kit's `TaskData` (src/kit/taskData.ts), over the bridge: what the Pinboard's task-list cards read
// and write tasks through. `list` reads every page `tasks.list` gives, `add`, `setStatus` and `reorder`
// are the calls of those names, and `onChanged` is PM's `tasks.changed` (a bare signal: the list reads
// again) plus the window regaining focus, which is how core's list heard of a write made elsewhere.
//
// `edit` asks PM to open ITS task editor (`ui.openTaskEditor`): the editor, with its stacks, its
// deletes and its calendar links, stays in core. PM answers at once and the editor stays open on its
// own, so there is no "saved" answer to wait for: the list hears `tasks.changed` when a task is saved or
// deleted, and the `done` it was given runs then (a quick-add row that became a task starts empty). A
// cancelled editor changes nothing and never calls it; a later edit replaces a `done` still waiting.
// `tasks.changed` is a bare signal, so the one thing told apart is this frame's own writes: PM announces
// those too, and a task ticked or added while the editor is open is not the editor saving.

import type { HostEvent, ModHost } from "../../../../src/kit/host";
import { fromModTask, type ModTaskDto } from "../../../../src/kit/lib/modTask";
import type { TaskData } from "../../../../src/kit/taskData";
import {
  addTask,
  listTasksPage,
  openTaskEditor,
  reorderTasks,
  setTaskStatus,
  type TaskPage,
} from "../lib/pm";

type Listed = Awaited<ReturnType<TaskData["list"]>>;

/** The most pages of tasks read: PM pages under a byte budget, and nobody has this many tasks. */
const MAX_PAGES = 200;

/** Every task of `project` (all, for null) and the stacks, from every page. */
async function listAll(project: string | null): Promise<Listed> {
  const tasks: Listed["tasks"] = [];
  let stacks: Listed["stacks"] = [];
  let after: string | null = null;
  for (let page = 0; page < MAX_PAGES; page += 1) {
    const got: TaskPage = await listTasksPage(project, after);
    for (const t of got.tasks) tasks.push(fromModTask(t as ModTaskDto));
    if (after === null) stacks = got.stacks as Listed["stacks"];
    if (got.next === null) return { tasks, stacks };
    // A cursor that does not move is PM not being what the list expects: stop rather than page forever.
    if (got.next === after) break;
    after = got.next;
  }
  throw new Error("PM's task list never ended.");
}

/** The task data a frame gives its lists. One object per page, built once: a new one would make every
 *  list read again. */
export function frameTaskData(
  host: ModHost,
  win: Pick<Window, "addEventListener" | "removeEventListener">,
): TaskData {
  let waiting: (() => void) | null = null;
  /** Writes this frame made while an editor was awaited, whose announcement has not come yet. */
  let ownWrites = 0;
  /** A write of this frame's own: its announcement is not the editor's. A write that fails announces
   *  nothing, so it takes its place back. */
  const own = <T>(write: () => Promise<T>): Promise<T> => {
    const counted = waiting !== null;
    if (counted) ownWrites += 1;
    return write().catch((error: unknown) => {
      if (counted && ownWrites > 0) ownWrites -= 1;
      throw error;
    });
  };
  const changed = new Set<() => void>();
  host.onEvent((event: HostEvent) => {
    if (event.name !== "tasks.changed") return;
    if (ownWrites > 0) {
      // PM coalesces announcements: one stands for every write of ours made since the last.
      ownWrites = 0;
    } else {
      const done = waiting;
      waiting = null;
      done?.();
    }
    for (const cb of [...changed]) cb();
  });
  return {
    list: listAll,
    add: (input) => own(() => addTask(input)),
    setStatus: (id, status) => own(() => setTaskStatus(id, status)),
    reorder: (ids) => own(() => reorderTasks(ids)),
    onChanged: (cb) => {
      changed.add(cb);
      const onFocus = () => cb();
      win.addEventListener("focus", onFocus);
      return Promise.resolve(() => {
        changed.delete(cb);
        win.removeEventListener("focus", onFocus);
      });
    },
    edit: (target, done) => {
      waiting = done;
      ownWrites = 0;
      const open =
        "task" in target
          ? openTaskEditor({ id: target.task.id })
          : openTaskEditor({
              ...(target.defaults.title ? { title: target.defaults.title } : {}),
              ...(target.defaults.project ? { project: target.defaults.project } : {}),
              ...(target.defaults.deadline ? { deadline: target.defaults.deadline } : {}),
            });
      // A refusal (an editor is already open, say) means no editor, so nothing to wait for.
      void open.catch(() => {
        if (waiting === done) waiting = null;
      });
    },
  };
}
