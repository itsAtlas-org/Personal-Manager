// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Pinboard's calls to PM, over the real bridge to a fake PM: that each sends what PM's handlers
// accept, reads what PM answers defensively, and that what PM refuses reaches the page as PM's own
// sentence. The board and the preferences are what must never be overwritten blind, so storage is held
// to PM's own revision rule here.

import { afterEach, describe, expect, it } from "vitest";
import errorRs from "../../../../src-tauri/src/mods/api/error.rs?raw";
import {
  CODE,
  PmAnswerError,
  PmError,
  addMilestone,
  addTask,
  connectPm,
  deleteMilestone,
  deriveTitle,
  getPref,
  importChecklistTasks,
  ingestNote,
  listMilestones,
  listProjects,
  listTasksPage,
  noteStatuses,
  openTaskEditor,
  openUrl,
  reorderTasks,
  setMilestoneStatus,
  setOverlay,
  setPref,
  setTaskStatus,
  updateMilestone,
} from "./pm";
import { memoryPm, refuse } from "../testing";

afterEach(() => connectPm(null));

describe("pinned to PM", () => {
  const code = (name: string) => new RegExp(`Self::${name} => "(MOD-\\d{4})"`).exec(errorRs)?.[1];

  it("the codes the page acts on are the Rust codes of the same name", () => {
    expect(code("StaleRevision")).toBe(CODE.staleRevision);
    expect(code("Declined")).toBe(CODE.declined);
    expect(code("TooLarge")).toBe(CODE.tooLarge);
    expect(code("QuotaExceeded")).toBe(CODE.quotaExceeded);
  });
});

describe("storage", () => {
  it("reads nothing stored as null, and what is stored as the JSON text core's getPref gave", async () => {
    const pm = await memoryPm({ seed: { board: { version: 1, widgets: [] } } });
    expect(await getPref("board")).toBe('{"version":1,"widgets":[]}');
    expect(await getPref("prefs")).toBeNull();
    expect(pm.of("storage.get").map((c) => c.args)).toEqual([{ key: "board" }, { key: "prefs" }]);
  });

  it("writes over the revision it read at, and the next write over the one that write made", async () => {
    const pm = await memoryPm({ seed: { board: { version: 1, widgets: [] } } });
    await getPref("board");
    await setPref("board", '{"version":1,"widgets":[1]}');
    await setPref("board", '{"version":1,"widgets":[2]}');
    expect(pm.of("storage.set").map((c) => c.args)).toEqual([
      { key: "board", value: { version: 1, widgets: [1] }, expectedRevision: 1 },
      { key: "board", value: { version: 1, widgets: [2] }, expectedRevision: 2 },
    ]);
    expect(pm.storage.get("board")?.value).toEqual({ version: 1, widgets: [2] });
  });

  it("a key never read is never written: nothing is sent", async () => {
    const pm = await memoryPm();
    await expect(setPref("board", "{}")).rejects.toBeInstanceOf(PmError);
    expect(pm.of("storage.set")).toEqual([]);
  });

  it("a write over a revision that moved on is PM's MOD-4001, and the value PM holds is not touched", async () => {
    const pm = await memoryPm({ seed: { board: { mine: false } } });
    await getPref("board");
    // Another window (or PM's own rename) wrote in between.
    pm.storage.set("board", { value: { theirs: true }, revision: 2 });
    const error = await setPref("board", '{"mine":true}').catch((e: unknown) => e);
    expect(error).toBeInstanceOf(PmError);
    expect((error as PmError).code).toBe("MOD-4001");
    expect((error as PmError).revision).toBe(2);
    expect(pm.storage.get("board")).toEqual({ value: { theirs: true }, revision: 2 });
  });

  it("reading what is not the shape PM sends is refused, not drawn from", async () => {
    await memoryPm({ answers: { "storage.get": { unexpected: true } } });
    await expect(getPref("board")).rejects.toBeInstanceOf(PmAnswerError);
  });

  it("PM's refusal comes through as its own sentence, and nothing else", async () => {
    await memoryPm({ answers: { "storage.get": refuse("MOD-2001", "No storage for you.") } });
    const error = await getPref("board").catch((e: unknown) => e);
    expect(String(error)).toBe("No storage for you.");
    expect((error as PmError).code).toBe("MOD-2001");
  });
});

describe("projects and milestones", () => {
  const ms = (id: number, project: string, extra: object = {}) => ({
    id,
    project_name: project,
    label: `m${id}`,
    due_date: "2026-12-01",
    calendar_linked: false,
    state: "unmet",
    status: "in_progress",
    sort_order: id,
    ...extra,
  });

  it("lists project names", async () => {
    await memoryPm({
      answers: { "projects.list": { projects: ["Atlas", "Home", 3], truncated: false } },
    });
    expect(await listProjects()).toEqual(["Atlas", "Home"]);
  });

  it("lists one project's milestones out of every page, by name and case-blind, and reads them defensively", async () => {
    const pm = await memoryPm({
      answers: {
        "milestones.list": (a: { after?: number }) =>
          a.after === undefined
            ? { milestones: [ms(1, "Atlas"), ms(2, "Home"), { nope: 1 }], next: 2 }
            : {
                milestones: [
                  ms(3, "atlas ", { event_uid: "u1", event_missing: true, status: "weird" }),
                ],
                next: null,
              },
      },
    });
    const got = await listMilestones("Atlas");
    expect(got.map((m) => m.id)).toEqual([1, 3]);
    expect(got[1]).toMatchObject({
      event_uid: "u1",
      event_missing: true,
      status: null,
      source_type: null,
    });
    expect(pm.of("milestones.list").map((c) => c.args)).toEqual([{}, { after: 2 }]);
  });

  it("a cursor that does not move forward is refused, not paged forever", async () => {
    await memoryPm({ answers: { "milestones.list": { milestones: [], next: 5 } } });
    await expect(listMilestones("Atlas")).rejects.toBeInstanceOf(PmAnswerError);
  });

  it("sends the milestone calls the shapes the handlers take", async () => {
    const pm = await memoryPm({
      answers: {
        "milestones.add": { id: 9 },
        "milestones.update": { id: 9 },
        "milestones.setStatus": { id: 9, status: "done" },
        "milestones.delete": { deleted: true },
      },
    });
    expect(await addMilestone("Atlas", "Ship", "2026-12-01")).toBe(9);
    await addMilestone("Atlas", "deadline", null);
    await updateMilestone(9, "Ship it", null);
    await setMilestoneStatus(9, "done");
    await deleteMilestone(9);
    expect(pm.calls.map((c) => [c.method, c.args])).toEqual([
      ["milestones.add", { project: "Atlas", label: "Ship", dueDate: "2026-12-01" }],
      ["milestones.add", { project: "Atlas", label: "deadline", dueDate: null }],
      ["milestones.update", { id: 9, label: "Ship it", dueDate: null }],
      ["milestones.setStatus", { id: 9, status: "done" }],
      ["milestones.delete", { id: 9 }],
    ]);
  });

  it("a milestone the person declined to create is PM's own sentence", async () => {
    await memoryPm({ answers: { "milestones.add": refuse("MOD-2004", "You said no.") } });
    const error = await addMilestone("New", "x", null).catch((e: unknown) => e);
    expect((error as PmError).code).toBe("MOD-2004");
    expect(String(error)).toBe("You said no.");
  });
});

describe("tasks", () => {
  it("pages tasks with the project and the cursor, and checks the answer's shape", async () => {
    const pm = await memoryPm({
      answers: { "tasks.list": { tasks: [{ id: 1 }], stacks: [], next: "3:1" } },
    });
    expect(await listTasksPage("Atlas", null)).toEqual({
      tasks: [{ id: 1 }],
      stacks: [],
      next: "3:1",
    });
    await listTasksPage(null, "3:1");
    expect(pm.of("tasks.list").map((c) => c.args)).toEqual([
      { project: "Atlas" },
      { after: "3:1" },
    ]);
    pm.answer("tasks.list", { tasks: [], stacks: [], next: 4 });
    await expect(listTasksPage(null, null)).rejects.toBeInstanceOf(PmAnswerError);
  });

  it("sends the task calls the shapes the handlers take, leaving out what is blank", async () => {
    const pm = await memoryPm({
      answers: {
        "tasks.add": { id: 1, status: "todo" },
        "tasks.setStatus": { id: 1, status: "doing" },
        "tasks.reorder": { reordered: 2 },
        "ui.openTaskEditor": { opened: true },
      },
    });
    await addTask({ title: "A", project: null, deadline: "" });
    await addTask({ title: "B", project: "Atlas", deadline: "2026-12-01" });
    await setTaskStatus(1, "doing");
    await reorderTasks([2, 1]);
    await openTaskEditor({ id: 7 });
    await openTaskEditor({ title: "T", project: "Atlas" });
    expect(pm.calls.map((c) => [c.method, c.args])).toEqual([
      ["tasks.add", { title: "A" }],
      ["tasks.add", { title: "B", project: "Atlas", deadline: "2026-12-01" }],
      ["tasks.setStatus", { id: 1, status: "doing" }],
      ["tasks.reorder", { ids: [2, 1] }],
      ["ui.openTaskEditor", { id: 7 }],
      ["ui.openTaskEditor", { new: { title: "T", project: "Atlas" } }],
    ]);
  });

  it("imports a checklist, naming the project only when there is one", async () => {
    const pm = await memoryPm({
      answers: { "tasks.importChecklist": { created: [4, 5], skipped: 1 } },
    });
    const items = [{ title: "a", done: true }];
    expect(await importChecklistTasks(null, items)).toEqual({ created: [4, 5], skipped: 1 });
    await importChecklistTasks("Atlas", items);
    expect(pm.of("tasks.importChecklist").map((c) => c.args)).toEqual([
      { items },
      { project: "Atlas", items },
    ]);
    pm.answer("tasks.importChecklist", { created: "no", skipped: 0 });
    await expect(importChecklistTasks(null, items)).rejects.toBeInstanceOf(PmAnswerError);
  });
});

describe("notes", () => {
  it("derives an untitled note's title as core did: its first non-blank line, cut at 80, else 'Untitled note'", () => {
    expect(deriveTitle("  Buy milk\nand eggs")).toBe("Buy milk");
    expect(deriveTitle("\n\n   Second para")).toBe("Second para");
    expect(deriveTitle("   \n ")).toBe("Untitled note");
    const long = deriveTitle("x".repeat(100));
    expect(Array.from(long)).toHaveLength(81);
    expect(long.endsWith("…")).toBe(true);
  });

  it("saves a note under its widget id, with a title PM accepts (one plain line, no ---)", async () => {
    const pm = await memoryPm({ answers: { "documents.saveNote": { saved: true } } });
    await ingestNote("w-1", "  A\ttitle ​ with --- dashes ", "body text ");
    await ingestNote("w-2", "", "First line\nsecond");
    await ingestNote("w-3", "   ", "   ");
    expect(pm.of("documents.saveNote").map((c) => c.args)).toEqual([
      { key: "w-1", title: "A title  with - dashes", body: "body text" },
      { key: "w-2", title: "First line", body: "First line\nsecond" },
      { key: "w-3", title: "Untitled note", body: "" },
    ]);
  });

  it("a title longer than PM allows is cut to 200 characters", async () => {
    const pm = await memoryPm({ answers: { "documents.saveNote": { saved: true } } });
    await ingestNote("w", "é".repeat(300), "x");
    expect(
      Array.from((pm.of("documents.saveNote")[0].args as { title: string }).title),
    ).toHaveLength(200);
  });

  it("an engine that is not set up is PM's own sentence, shown as the ingest error", async () => {
    await memoryPm({
      answers: { "documents.saveNote": refuse("MOD-5007", "Set up search first.") },
    });
    expect(String(await ingestNote("w", "t", "b").catch((e: unknown) => e))).toBe(
      "Set up search first.",
    );
  });

  it("asks about notes by key in chunks of 500, and leaves out one with no document", async () => {
    const keys = Array.from({ length: 1001 }, (_, i) => `k${i}`);
    const pm = await memoryPm({
      answers: {
        "documents.statusOf": (a: { keys: string[] }) =>
          Object.fromEntries(
            a.keys.map((k) => [
              k,
              k === "k1"
                ? { documentId: 1, reviewed: true, archived: false, project: "Atlas" }
                : k === "k2"
                  ? { documentId: 2, reviewed: false, archived: false }
                  : null,
            ]),
          ),
      },
    });
    const got = await noteStatuses(keys);
    expect(pm.of("documents.statusOf")).toHaveLength(3);
    expect([...got]).toEqual([
      ["k1", { reviewed: true, project: "Atlas" }],
      ["k2", { reviewed: false, project: "" }],
    ]);
  });
});

describe("links and the Calendar", () => {
  it("opens an address and publishes entries through the calls of those names", async () => {
    const pm = await memoryPm({
      answers: { "shell.openLink": { opened: true }, "calendar.overlay.set": { set: 1 } },
    });
    await openUrl("https://example.org/a");
    await setOverlay([{ id: "w:1", date: "2026-12-01", label: "x" }]);
    expect(pm.calls.map((c) => [c.method, c.args])).toEqual([
      ["shell.openLink", { url: "https://example.org/a" }],
      ["calendar.overlay.set", { entries: [{ id: "w:1", date: "2026-12-01", label: "x" }] }],
    ]);
  });

  it("a call before the page is connected says so", async () => {
    await expect(openUrl("https://example.org")).rejects.toThrow("isn't connected");
  });
});
