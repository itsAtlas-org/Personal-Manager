// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The task data a frame gives its list cards: the calls of `tasks.*` it makes, what it does with what
// PM answers (a task whose link the mod may not look through never reads as removed), and what
// `edit` promises: PM opens its own editor, and `done` runs when the list hears a task was saved.

import { afterEach, describe, expect, it, vi } from "vitest";
import { REDACTED_EVENT_UID } from "../../../../src/kit/lib/modTask";
import { connectPm } from "../lib/pm";
import { memoryPm, refuse } from "../testing";
import { frameTaskData } from "./taskData";

afterEach(() => connectPm(null));

const dto = (id: number, extra: object = {}) => ({
  id,
  title: `t${id}`,
  why: null,
  status: "todo",
  completed_at: null,
  lane: "foreground",
  stack_id: null,
  parked: false,
  ordinal: id,
  source: "user",
  created_at: "2026-11-01T09:00:00.000Z",
  updated_at: "2026-11-01T09:00:00.000Z",
  do_start: null,
  do_end: null,
  deadline: null,
  deadline_kind: "hard",
  deadline_from: "date",
  effective_deadline: null,
  deadline_missing: false,
  milestone_linked: false,
  event_linked: false,
  ...extra,
});
const stack = {
  id: 1,
  title: "S",
  kind: "sequential",
  note: null,
  created_at: "x",
  updated_at: "x",
};

describe("list", () => {
  it("reads every page of one project's tasks, the stacks from the first, and fills a hidden link as a link", async () => {
    const pm = await memoryPm({
      answers: {
        "tasks.list": (a: { after?: string }) =>
          a.after === undefined
            ? {
                tasks: [dto(1), dto(2, { deadline_from: "event", event_linked: true })],
                stacks: [stack],
                next: "2:2",
              }
            : { tasks: [dto(3)], stacks: [], next: null },
      },
    });
    const data = frameTaskData(pm.host, window);
    const got = await data.list("Atlas");
    expect(got.tasks.map((t) => t.id)).toEqual([1, 2, 3]);
    expect(got.stacks).toEqual([stack]);
    // PM named the link and hid the event: the list must say "Event's date", not "removed".
    expect(got.tasks[1].event_uid).toBe(REDACTED_EVENT_UID);
    expect(got.tasks[0].event_uid).toBeNull();
    expect(pm.of("tasks.list").map((c) => c.args)).toEqual([
      { project: "Atlas" },
      { project: "Atlas", after: "2:2" },
    ]);
  });

  it("every task is a null project (no project in the call)", async () => {
    const pm = await memoryPm({ answers: { "tasks.list": { tasks: [], stacks: [], next: null } } });
    await frameTaskData(pm.host, window).list(null);
    expect(pm.of("tasks.list")[0].args).toEqual({});
  });

  it("a cursor that does not move is refused rather than paged forever", async () => {
    const pm = await memoryPm({
      answers: { "tasks.list": { tasks: [], stacks: [], next: "1:1" } },
    });
    await expect(frameTaskData(pm.host, window).list(null)).rejects.toThrow();
    expect(pm.of("tasks.list").length).toBeLessThanOrEqual(3);
  });
});

describe("writes", () => {
  it("add, setStatus and reorder are the calls of those names", async () => {
    const pm = await memoryPm({
      answers: {
        "tasks.add": { id: 1, status: "todo" },
        "tasks.setStatus": { id: 1, status: "done" },
        "tasks.reorder": { reordered: 2 },
      },
    });
    const data = frameTaskData(pm.host, window);
    await data.add({ title: "x", project: "Atlas", deadline: "2026-12-01" });
    await data.setStatus(1, "done");
    await data.reorder([2, 1]);
    expect(pm.calls.map((c) => [c.method, c.args])).toEqual([
      ["tasks.add", { title: "x", project: "Atlas", deadline: "2026-12-01" }],
      ["tasks.setStatus", { id: 1, status: "done" }],
      ["tasks.reorder", { ids: [2, 1] }],
    ]);
  });
});

describe("onChanged", () => {
  it("runs on PM's tasks.changed and on the window regaining focus, and not after it stops", async () => {
    const pm = await memoryPm();
    const data = frameTaskData(pm.host, window);
    const cb = vi.fn();
    const stop = await data.onChanged(cb);
    pm.emit("tasks.changed", {});
    await pm.settle();
    window.dispatchEvent(new Event("focus"));
    expect(cb).toHaveBeenCalledTimes(2);
    stop();
    pm.emit("tasks.changed", {});
    await pm.settle();
    window.dispatchEvent(new Event("focus"));
    expect(cb).toHaveBeenCalledTimes(2);
  });

  it("an event of another name is not a change", async () => {
    const pm = await memoryPm();
    const cb = vi.fn();
    await frameTaskData(pm.host, window).onChanged(cb);
    pm.emit("theme.changed", {});
    await pm.settle();
    expect(cb).not.toHaveBeenCalled();
  });
});

describe("edit", () => {
  it("asks PM to open its editor on a task by id, and runs done when a task is next saved, once", async () => {
    const pm = await memoryPm({ answers: { "ui.openTaskEditor": { opened: true } } });
    const data = frameTaskData(pm.host, window);
    const done = vi.fn();
    data.edit({ task: { id: 7 } as never }, done);
    await pm.settle();
    expect(pm.of("ui.openTaskEditor")[0].args).toEqual({ id: 7 });
    // Opening the editor changed nothing, so nothing has run.
    expect(done).not.toHaveBeenCalled();
    pm.emit("tasks.changed", {});
    await pm.settle();
    expect(done).toHaveBeenCalledTimes(1);
    pm.emit("tasks.changed", {});
    await pm.settle();
    expect(done).toHaveBeenCalledTimes(1);
  });

  it("a task added or ticked from the list while the editor is open is not the editor saving: done waits for the editor's own save", async () => {
    const pm = await memoryPm({
      answers: {
        "ui.openTaskEditor": { opened: true },
        "tasks.add": { id: 9 },
        "tasks.setStatus": { id: 9, status: "done" },
      },
    });
    const data = frameTaskData(pm.host, window);
    const done = vi.fn();
    data.edit({ defaults: { title: "draft" } }, done);
    await pm.settle();
    await data.add({ title: "other", project: null } as never);
    await data.setStatus(9, "done" as never);
    await pm.settle();
    // PM announces both writes as one coalesced signal: ours, so the editor is still open.
    pm.emit("tasks.changed", {});
    await pm.settle();
    expect(done).not.toHaveBeenCalled();
    // The editor's own save is the next announcement.
    pm.emit("tasks.changed", {});
    await pm.settle();
    expect(done).toHaveBeenCalledTimes(1);
  });

  it("a write of ours that PM refuses announces nothing, so it does not hide the editor's save", async () => {
    const pm = await memoryPm({
      answers: {
        "ui.openTaskEditor": { opened: true },
        "tasks.add": refuse("MOD-3001", "No."),
      },
    });
    const data = frameTaskData(pm.host, window);
    const done = vi.fn();
    data.edit({ defaults: { title: "draft" } }, done);
    await pm.settle();
    await data.add({ title: "other", project: null } as never).catch(() => {});
    pm.emit("tasks.changed", {});
    await pm.settle();
    expect(done).toHaveBeenCalledTimes(1);
  });

  it("a new task starts from what was typed, leaving out what is blank", async () => {
    const pm = await memoryPm({ answers: { "ui.openTaskEditor": { opened: true } } });
    const data = frameTaskData(pm.host, window);
    data.edit({ defaults: { title: "Call", project: null, deadline: "2026-12-01" } }, () => {});
    data.edit({ defaults: { title: "", project: "Atlas" } }, () => {});
    await pm.settle();
    expect(pm.of("ui.openTaskEditor").map((c) => c.args)).toEqual([
      { new: { title: "Call", deadline: "2026-12-01" } },
      { new: { project: "Atlas" } },
    ]);
  });

  it("a cancelled editor never calls done, and a refused one is not waited for", async () => {
    const pm = await memoryPm({
      answers: { "ui.openTaskEditor": refuse("MOD-3001", "An editor is already open.") },
    });
    const data = frameTaskData(pm.host, window);
    const done = vi.fn();
    data.edit({ defaults: { title: "x" } }, done);
    await pm.settle();
    pm.emit("tasks.changed", {});
    await pm.settle();
    expect(done).not.toHaveBeenCalled();
  });
});
