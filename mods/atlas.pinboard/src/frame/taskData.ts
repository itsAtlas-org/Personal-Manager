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
// deleted, and the `done` it was given runs then (a quick-add row that became a task starts empty).
// PM never says when an editor closes, so a cancel cannot be heard; what keeps a cancelled editor from
// calling `done` is what `done` waits for. `tasks.changed` is a bare signal, so two things are told
// apart: this frame's own writes (PM announces those too, and a task ticked or added while the editor
// is open is not the editor saving), and, for a NEW task, whether a task that was not there when the
// editor opened is there now. A foreign change with no new task (a calendar sync, another window, the
// editor cancelled and something else ticked) leaves `done` waiting, so it never wipes the quick-add
// row the person is typing in; nor does a task this frame's own quick-add made after a cancel. A later edit replaces a `done` still waiting; one PM refuses puts the
// earlier one back.

import type { HostEvent, ModHost } from "../../../../src/kit/host";
import { fromModTask, type ModTaskDto } from "../../../../src/kit/lib/modTask";
import type { TaskData } from "../../../../src/kit/taskData";
import { tidyLine } from "../lib/pinboard/calendarEntries";
import {
  addTask,
  listTasksPage,
  openTaskEditor,
  reorderTasks,
  setTaskStatus,
  tidyProjectName,
  type TaskPage,
} from "../lib/pm";

type Listed = Awaited<ReturnType<TaskData["list"]>>;

/** The most characters of a task title PM takes (`tasks::TITLE_MAX`). */
const TITLE_MAX = 500;

/** An editor being waited on: what to run when it saved, and, for a new task, the ids that existed
 *  when it opened (null: not known, or an existing task, whose `done` only reads again). */
interface Waiting {
  done: () => void;
  known: Set<number> | null;
  /** True for a new-task editor, whose `done` must wait for a task that was not there before. */
  isNew: boolean;
  /** Tasks this frame's own quick-add made while the editor was awaited: not the editor's save. */
  ownAdded: Set<number>;
}

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
  let waiting: Waiting | null = null;
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
  /** A change this frame did not cause while an editor is awaited: a new-task editor's `done` runs only
   *  once a task that was not there before is (a cancel and an unrelated change leave it waiting). */
  const heard = (entry: Waiting) => {
    if (!entry.isNew || entry.known === null) {
      waiting = null;
      entry.done();
      return;
    }
    const known = entry.known;
    void listAll(null).then(
      (now) => {
        if (
          waiting !== entry ||
          !now.tasks.some((t) => !known.has(t.id) && !entry.ownAdded.has(t.id))
        )
          return;
        waiting = null;
        entry.done();
      },
      () => {},
    );
  };
  host.onEvent((event: HostEvent) => {
    if (event.name !== "tasks.changed") return;
    if (ownWrites > 0) {
      // PM coalesces announcements: one stands for every write of ours made since the last.
      ownWrites = 0;
    } else if (waiting !== null) {
      heard(waiting);
    }
    for (const cb of [...changed]) cb();
  });
  return {
    list: listAll,
    add: async (input) => {
      // An editor that was cancelled stays awaited, so a task this frame adds meanwhile is not a
      // task "the editor made": the next foreign change would otherwise run `done` and wipe the row.
      const entry = waiting;
      const made = await own(() => addTask(input));
      if (made !== null) entry?.ownAdded.add(made);
      return made;
    },
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
      const previous = waiting;
      const entry: Waiting = { done, known: null, isNew: !("task" in target), ownAdded: new Set() };
      waiting = entry;
      ownWrites = 0;
      let open: Promise<void>;
      if ("task" in target) {
        open = openTaskEditor({ id: target.task.id });
      } else {
        // What PM takes as a prefill (one plain line, a title of at most 500 characters): anything
        // else is refused and the editor would not open, so the text is made to fit first.
        const title = tidyLine(target.defaults.title ?? "", TITLE_MAX);
        const project = tidyProjectName(target.defaults.project ?? "");
        open = openTaskEditor({
          ...(title ? { title } : {}),
          ...(project ? { project } : {}),
          ...(target.defaults.deadline ? { deadline: target.defaults.deadline } : {}),
        });
        // The tasks there are now, to tell the editor's new one from any other change. Unread, the
        // first change heard stands for a save (done only starts a fresh quick-add row).
        void listAll(null).then(
          (now) => {
            if (waiting === entry) entry.known = new Set(now.tasks.map((t) => t.id));
          },
          () => {},
        );
      }
      // A refusal (an editor is already open, say) means no new editor: the one already open keeps
      // its own `done`.
      void open.catch(() => {
        if (waiting === entry) waiting = previous;
      });
    },
  };
}
