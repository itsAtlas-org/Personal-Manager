// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the main window adds to the page hooks a mod's page actions use (`modPageHooks.ts`; PR C, K5): PM's
// own task editor for `ui.openTaskEditor`, and the way to the document engine's setup for MOD-5007's
// "Set it up". It sits in App beside the document reader, which is where every surface a frame can be on
// already reaches PM's own views from.
//
// The editor is core's own `TaskEditor`, the one every task surface opens, mounted ONCE here and opened
// at the mod's request: one at a time (a second request while one is open or loading is refused, and the
// frame is told it is asking too fast), and a task made or changed in it is the person's, not the mod's
// gift. The frame lives only in the main window, so this is only mounted by App: the pop-out Calendar
// has no frames and keeps the editor its CalendarView mounts.
//
// Prefill is text and is shown as the value of the editor's own fields; the mod gets only that the
// editor opened (`ui.openTaskEditor` does not wait for it to close). The frame re-reads on
// `tasks.changed`, which core's own `tasks://changed` after a save raises.

import { useCallback, useContext, useMemo, useRef, useState, type ReactNode } from "react";

import { listTasks } from "../../lib/ipc";
import type { Task, TaskInput } from "../../lib/types";
import { TaskEditor } from "../tasks/TaskEditor";
import {
  ModPageHooksContext,
  type ModPageHooks,
  type ModSource,
  type TaskEditorRequest,
} from "./modPageHooks";

/** What is open: an existing task, or a new one starting from what the mod prefilled. */
type Editing = { task: Task } | { defaults: Partial<TaskInput> };

export function ModPageHooksHost({
  children,
  onOpenEngineSetup,
}: {
  children: ReactNode;
  onOpenEngineSetup: () => void;
}) {
  const outer = useContext(ModPageHooksContext);
  const [editing, setEditing] = useState<Editing | null>(null);
  // Set the moment a request is accepted and cleared when the editor closes (or fails to open), so two
  // requests in one tick can't both get through: the state would still say "closed" to the second.
  const busy = useRef(false);

  const open = useCallback((_from: ModSource, request: TaskEditorRequest): boolean => {
    if (busy.current) return false;
    busy.current = true;
    if ("id" in request) {
      const wanted = request.id;
      listTasks().then(
        (tasks) => {
          const task = tasks.find((t) => t.id === wanted);
          if (task) setEditing({ task });
          else busy.current = false;
        },
        () => {
          busy.current = false;
        },
      );
    } else {
      const { title, project, deadline } = request.new;
      setEditing({
        defaults: {
          ...(title !== null ? { title } : {}),
          ...(project !== null ? { project } : {}),
          ...(deadline !== null ? { deadline } : {}),
        },
      });
    }
    return true;
  }, []);

  const close = useCallback(() => {
    busy.current = false;
    setEditing(null);
  }, []);

  const hooks = useMemo<ModPageHooks>(
    () => ({ ...outer, openTaskEditor: open, openEngineSetup: onOpenEngineSetup }),
    [outer, open, onOpenEngineSetup],
  );

  return (
    <ModPageHooksContext.Provider value={hooks}>
      {children}
      <TaskEditor
        open={editing !== null}
        task={editing && "task" in editing ? editing.task : null}
        defaults={editing && "defaults" in editing ? editing.defaults : undefined}
        onClose={close}
      />
    </ModPageHooksContext.Provider>
  );
}
