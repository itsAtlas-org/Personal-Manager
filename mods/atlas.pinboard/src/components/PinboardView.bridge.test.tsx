// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The places the copied Pinboard view meets PM, drawn over the real bridge to a fake PM (the kit's
// fakePm): which notes it asks about, how it saves one, what a timeline bound to a project reads and
// writes, and the "ask before deleting" switch that moved into its header. The view's own behaviour
// (cards, drags, menus) is held by PinboardView.test.tsx, the core tests copied.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { TaskDataProvider } from "../../../../src/kit/taskData";
import { ThemeProvider } from "../../../../src/kit/theme";
import { frameTaskData } from "../frame/taskData";
import { resetPrefs, startPrefs } from "../lib/pinboard/prefs";
import { connectPm } from "../lib/pm";
import { memoryPm, refuse, type MemoryPm } from "../testing";
import { PinboardView } from "./PinboardView";

beforeAll(() => {
  const rects = () =>
    ({
      length: 0,
      item: () => null,
      [Symbol.iterator]: [][Symbol.iterator],
    }) as unknown as DOMRectList;
  Range.prototype.getClientRects = rects;
  Range.prototype.getBoundingClientRect = () => new DOMRect();
  Element.prototype.getClientRects = rects;
  document.elementFromPoint = () => null;
  Element.prototype.scrollTo = () => {};
  Element.prototype.getBoundingClientRect = () => new DOMRect(0, 0, 1000, 800);
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});

const store = { get: async () => null, set: async () => {} };

let pm: MemoryPm;
afterEach(() => {
  cleanup();
  connectPm(null);
  resetPrefs();
});
afterAll(() => connectPm(null));
beforeEach(() => resetPrefs());

type Seed = { widgets: unknown[] };
const board = (seed: Seed) => ({ version: 1, widgets: seed.widgets });
const note = (id: string, extra: object = {}) => ({
  id,
  kind: "note",
  rect: { x: 0, y: 0, w: 7, h: 5 },
  title: id.toUpperCase(),
  text: "some words",
  ...extra,
});

async function mount(widgets: unknown[], answers: Record<string, unknown> = {}) {
  pm = await memoryPm({
    seed: { board: board({ widgets }) },
    answers: {
      "projects.list": { projects: ["Atlas"], truncated: false },
      "milestones.list": { milestones: [], next: null },
      "documents.statusOf": {},
      "tasks.list": { tasks: [], stacks: [], next: null },
      ...answers,
    },
  });
  const utils = render(
    <ThemeProvider store={store}>
      <TaskDataProvider data={frameTaskData(pm.host, window)}>
        <PinboardView />
      </TaskDataProvider>
    </ThemeProvider>,
  );
  await act(async () => void (await pm.settle()));
  return utils;
}

describe("a note's filing status", () => {
  it("asks PM about every note it can, by widget id (inside a folder too), and shows what PM answers", async () => {
    await mount(
      [
        note("n1", { ingestedAt: "2026-11-01T00:00:00Z", ingestedHash: "x" }),
        note("n2", {
          ingestedAt: "2026-11-01T00:00:00Z",
          ingestedHash: "x",
          rect: { x: 8, y: 0, w: 7, h: 5 },
        }),
        // Not a key PM could be asked about: one such id would make PM refuse the whole question.
        note("chat-1", { rect: { x: 0, y: 6, w: 7, h: 5 } }),
        note("bad id!", { rect: { x: 8, y: 6, w: 7, h: 5 } }),
        {
          id: "f1",
          kind: "folder",
          rect: { x: 16, y: 0, w: 4, h: 3 },
          title: "",
          children: [note("kid")],
        },
        { id: "t1", kind: "timeline", rect: { x: 16, y: 6, w: 7, h: 5 }, title: "", items: [] },
      ],
      {
        "documents.statusOf": (a: { keys: string[] }) =>
          Object.fromEntries(
            a.keys.map((k) => [
              k,
              k === "n1"
                ? { documentId: 1, reviewed: true, archived: false, project: "Atlas" }
                : k === "n2"
                  ? { documentId: 2, reviewed: false, archived: false }
                  : null,
            ]),
          ),
      },
    );
    await waitFor(() => expect(screen.getByText("Filed · Atlas")).toBeTruthy());
    expect(screen.getByText("In review")).toBeTruthy();
    expect(pm.of("documents.statusOf").map((c) => c.args)).toEqual([{ keys: ["n1", "n2", "kid"] }]);
  });

  it("with no note on the board it asks nothing", async () => {
    await mount([
      { id: "t1", kind: "timeline", rect: { x: 0, y: 0, w: 7, h: 5 }, title: "", items: [] },
    ]);
    expect(pm.of("documents.statusOf")).toEqual([]);
  });

  it("an ingested note whose document PM no longer has reads as plain Ingested", async () => {
    await mount([note("n1", { ingestedAt: "2026-11-01T00:00:00Z", ingestedHash: "x" })]);
    await waitFor(() => expect(pm.of("documents.statusOf")).toHaveLength(1));
    expect(screen.getByText("Ingested")).toBeTruthy();
  });
});

describe("Ingest", () => {
  it("saves the note's text as a document under its widget id, titled as the card is", async () => {
    await mount([note("n1", { title: "Plan" })], { "documents.saveNote": { saved: true } });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Ingest" })));
    await act(async () => void (await pm.settle()));
    expect(pm.of("documents.saveNote").map((c) => c.args)).toEqual([
      { key: "n1", title: "Plan", body: "some words" },
    ]);
    await waitFor(() => expect(screen.getByText("Ingested")).toBeTruthy());
  });

  it("when PM refuses (the search engine is not set up), the card says so in PM's words and is not marked ingested", async () => {
    await mount([note("n1")], { "documents.saveNote": refuse("MOD-5007", "Set up search first.") });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Ingest" })));
    await act(async () => void (await pm.settle()));
    expect(screen.getByText(/Set up search first\./)).toBeTruthy();
    expect(screen.queryByText("Ingested")).toBeNull();
  });
});

describe("a timeline bound to a project", () => {
  const bound = {
    id: "t1",
    kind: "timeline",
    rect: { x: 0, y: 0, w: 9, h: 8 },
    title: "",
    project: "Atlas",
  };
  const ms = (id: number, project: string, label: string) => ({
    id,
    project_name: project,
    label,
    due_date: "2030-12-01",
    calendar_linked: false,
    state: "unmet",
    status: "not_started",
    sort_order: id,
  });

  it("shows that project's milestones and only those", async () => {
    await mount([bound], {
      "milestones.list": {
        milestones: [ms(1, "Atlas", "Ship it"), ms(2, "Home", "Paint")],
        next: null,
      },
    });
    await waitFor(() => expect(screen.getByDisplayValue("Ship it")).toBeTruthy());
    expect(screen.queryByDisplayValue("Paint")).toBeNull();
  });

  it("+ Milestone adds a 'deadline' to the project, with no date and no event", async () => {
    await mount([bound], { "milestones.add": { id: 5 } });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "+ Milestone" })));
    await act(async () => void (await pm.settle()));
    expect(pm.of("milestones.add").map((c) => c.args)).toEqual([
      { project: "Atlas", label: "deadline", dueDate: null },
    ]);
  });

  it("when the person says no to PM's question, the card says so in PM's words and adds nothing", async () => {
    await mount([bound], { "milestones.add": refuse("MOD-2004", "You said no.") });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "+ Milestone" })));
    await act(async () => void (await pm.settle()));
    expect(screen.getByRole("alert").textContent).toBe("You said no.");
  });
});

describe("the calendar events a card names (what calendar.read is declared for)", () => {
  const task = {
    id: 1,
    title: "Prepare slides",
    why: null,
    status: "todo",
    completed_at: null,
    lane: "foreground",
    stack_id: null,
    parked: false,
    ordinal: 1,
    source: "user",
    created_at: "2026-11-01T09:00:00.000Z",
    updated_at: "2026-11-01T09:00:00.000Z",
    do_start: null,
    do_end: null,
    deadline_kind: "hard",
    deadline: "2030-12-01",
    deadline_from: "event",
    effective_deadline: "2030-12-01",
    deadline_missing: false,
    milestone_linked: false,
    event_linked: true,
    event_uid: "uid-1",
    event_title: "Board meeting",
  };

  it("a task linked to a calendar event shows the event's name on the task list card", async () => {
    await mount([{ id: "l1", kind: "tasklist", rect: { x: 0, y: 0, w: 9, h: 8 }, title: "" }], {
      "tasks.list": { tasks: [task], stacks: [], next: null },
    });
    await waitFor(() => expect(screen.getByText("Prepare slides")).toBeTruthy());
    // The list is the small kind: it names the event in the date's tooltip.
    expect(screen.getByTitle("Event's date: Board meeting")).toBeTruthy();
  });

  it("a milestone linked to an event whose event PM cannot find says so", async () => {
    await mount(
      [
        {
          id: "t1",
          kind: "timeline",
          rect: { x: 0, y: 0, w: 9, h: 8 },
          title: "",
          project: "Atlas",
        },
      ],
      {
        "milestones.list": {
          milestones: [
            {
              id: 1,
              project_name: "Atlas",
              label: "Launch",
              due_date: "2030-12-01",
              event_uid: "uid-1",
              calendar_linked: true,
              event_missing: true,
              state: "unmet",
              status: "not_started",
              sort_order: 1,
            },
          ],
          next: null,
        },
      },
    );
    await waitFor(() => expect(screen.getByDisplayValue("Launch")).toBeTruthy());
    expect(screen.getAllByTitle("Linked event not found in your calendars").length).toBeGreaterThan(
      0,
    );
  });
});

describe("ask before deleting", () => {
  const nameOf = (label: string) => screen.getByRole("checkbox", { name: new RegExp(label) });

  it("is on at first: deleting a note asks, and the header says so", async () => {
    await mount([note("n1")]);
    expect((nameOf("Ask before deleting") as HTMLInputElement).checked).toBe(true);
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Delete" })));
    expect(screen.getByRole("dialog")).toBeTruthy();
  });

  it("switched off in the header, a delete goes through at once, and the choice is saved as one preference", async () => {
    const saved: unknown[] = [];
    startPrefs(null, (p) => saved.push(p));
    await mount([note("n1")]);
    await act(async () => fireEvent.click(nameOf("Ask before deleting")));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Delete" })));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.querySelector("[data-widget-id=n1]")).toBeNull();
    expect(saved).toEqual([{ confirmDelete: false, showPast: true, showCompleted: true }]);
  });

  it("'Don't ask again' in the pop-up turns the header's switch off too", async () => {
    startPrefs(null, () => {});
    await mount([note("n1")]);
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Delete" })));
    const dialog = screen.getByRole("dialog");
    await act(async () => fireEvent.click(within(dialog).getByRole("checkbox")));
    await act(async () =>
      fireEvent.click(within(dialog).getByRole("button", { name: /^Delete|Remove/ })),
    );
    expect((nameOf("Ask before deleting") as HTMLInputElement).checked).toBe(false);
  });
});
