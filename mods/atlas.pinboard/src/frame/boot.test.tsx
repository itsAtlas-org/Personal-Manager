// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The page starts: it connects, reads its preferences and PM's theme before it draws, and draws the
// board it was given. A frame has NO `localStorage` or `sessionStorage` (the sandbox gives the page an
// opaque origin, so reading either throws), so everything here runs with both replaced by throwing
// getters, as the kit's own components are held to (src/kit/kitStorage.test.tsx).

import { act, cleanup } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { HostError } from "../../../../src/kit/host";
import { resetPrefs } from "../lib/pinboard/prefs";
import { connectPm } from "../lib/pm";
import { memoryPm, refuse, type MemoryPm } from "../testing";
import { MIRROR_LAYER } from "./cspStyles";
import { boot, NO_ANSWER, NOT_FRAMED, STARTING, type Running } from "./boot";

const BOARD = {
  version: 1,
  widgets: [
    { id: "n1", kind: "note", rect: { x: 0, y: 0, w: 7, h: 5 }, title: "Plan", text: "hi" },
    { id: "l1", kind: "tasklist", rect: { x: 8, y: 0, w: 9, h: 8 }, title: "", project: "Atlas" },
  ],
};

const THEME = {
  vars: { "--bg": "oklch(0.2 0 0)" },
  mode: "dark",
  system: "slate",
  accent: "mono",
  colorblind: false,
  depth: "standard",
};

const TASK = {
  id: 1,
  title: "Ship the deck",
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
  deadline: null,
  deadline_kind: "hard",
  deadline_from: "date",
  effective_deadline: null,
  deadline_missing: false,
  milestone_linked: false,
  event_linked: false,
};

// jsdom lays nothing out; ProseMirror measures when it scrolls a selection into view, and the board
// measures its scroller.
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

/** Replaces both storages with getters that throw, as a frame's do; counts how often they were asked. */
const saved = new Map<string, PropertyDescriptor | undefined>();
const asked = { count: 0 };
beforeEach(() => {
  asked.count = 0;
  for (const name of ["localStorage", "sessionStorage"]) {
    saved.set(name, Object.getOwnPropertyDescriptor(window, name));
    Object.defineProperty(window, name, {
      configurable: true,
      get() {
        asked.count += 1;
        throw new DOMException("The operation is insecure.", "SecurityError");
      },
    });
  }
});
afterEach(() => {
  for (const [name, was] of saved) {
    if (was) Object.defineProperty(window, name, was);
    else delete (window as unknown as Record<string, unknown>)[name];
  }
  cleanup();
  document.body.innerHTML = '<div id="root"></div>';
  connectPm(null);
  resetPrefs();
});
afterAll(() => connectPm(null));

let running: Running | null = null;
afterEach(() => {
  act(() => running?.root.unmount());
  running = null;
});

async function start(pm: MemoryPm): Promise<Running | null> {
  document.body.innerHTML = '<div id="root"></div>';
  let result: Running | null = null;
  await act(async () => {
    result = await boot({ doc: document, win: window, connect: async () => pm.host });
    await pm.settle();
  });
  running = result;
  return result;
}

const answers = {
  "theme.get": THEME,
  "projects.list": { projects: ["Atlas"], truncated: false },
  "milestones.list": { milestones: [], next: null },
  "documents.statusOf": {},
  "tasks.list": { tasks: [TASK], stacks: [], next: null },
};

describe("boot", () => {
  it("starts the style mirror before it draws, so a library's <style> is kept under the frame's policy", async () => {
    // jsdom has no constructable stylesheets: stand-ins, taken away after.
    const win = window as unknown as Record<string, unknown>;
    const savedSheet = win.CSSStyleSheet;
    class Sheet {
      text = "";
      replaceSync(text: string) {
        this.text = text;
      }
    }
    win.CSSStyleSheet = Sheet;
    Object.defineProperty(document, "adoptedStyleSheets", {
      configurable: true,
      writable: true,
      value: [],
    });
    try {
      const pm = await memoryPm({ seed: { board: BOARD }, answers });
      await start(pm);
      const el = document.createElement("style");
      el.textContent = ".cm-editor { display: flex }";
      document.head.appendChild(el);
      await new Promise<void>((resolve) => setTimeout(resolve, 0));
      const sheets = (document as unknown as { adoptedStyleSheets: Sheet[] }).adoptedStyleSheets;
      expect(sheets).toHaveLength(1);
      expect(sheets[0].text).toContain(`@layer ${MIRROR_LAYER}`);
      expect(sheets[0].text).toContain(".cm-editor { display: flex }");
    } finally {
      document.head.querySelectorAll("style").forEach((n) => n.remove());
      win.CSSStyleSheet = savedSheet;
      delete (document as unknown as Record<string, unknown>).adoptedStyleSheets;
    }
  });

  it("draws the stored board with both storages throwing", async () => {
    const pm = await memoryPm({ seed: { board: BOARD }, answers });
    const up = await start(pm);
    expect(up).not.toBeNull();
    await act(async () => void (await pm.settle()));
    expect(document.querySelector("[data-widget-id=n1]")).not.toBeNull();
    // The task-list card read its tasks through the frame's task data.
    expect(document.body.textContent).toContain("Ship the deck");
    expect(pm.of("tasks.list")[0].args).toEqual({ project: "Atlas" });
  });

  it("touches no storage of its own: the page's code never names localStorage or sessionStorage", () => {
    // The kit's providers read them inside a try/catch (so `asked` counts those and the page still
    // draws, above); the Pinboard's own code must not name either, as the frame has neither.
    const own = Object.entries(
      import.meta.glob("../**/*.{ts,tsx}", {
        query: "?raw",
        import: "default",
        eager: true,
      }) as Record<string, string>,
    ).filter(([path]) => !/\.test\.tsx?$/.test(path) && !path.endsWith("/testing.ts"));
    expect(own.length).toBeGreaterThan(20);
    const naming = own
      .filter(([, text]) =>
        text
          .split("\n")
          .some((l) => !/^\s*(\/\/|\*|\/\*)/.test(l) && /\b(local|session)Storage\b/.test(l)),
      )
      .map(([path]) => path);
    expect(naming).toEqual([]);
  });

  it("wears PM's theme before it draws: the variables PM gave are on the root", async () => {
    const pm = await memoryPm({ seed: { board: BOARD }, answers });
    await start(pm);
    expect(document.documentElement.style.getPropertyValue("--bg")).toBe("oklch(0.2 0 0)");
  });

  it("reads its preferences before the first draw: a turned-off nag is off at once, and flipping it saves the key", async () => {
    const pm = await memoryPm({ seed: { board: BOARD, prefs: { confirmDelete: false } }, answers });
    await start(pm);
    await act(async () => void (await pm.settle()));
    const toggle = [...document.querySelectorAll("label")].find((l) =>
      l.textContent?.includes("Ask before deleting"),
    );
    const box = toggle?.querySelector("input") as HTMLInputElement;
    expect(box.checked).toBe(false);
    await act(async () => box.click());
    await act(async () => void (await pm.settle()));
    expect(box.checked).toBe(true);
    const sets = pm.of("storage.set").filter((c) => (c.args as { key: string }).key === "prefs");
    expect(sets.map((c) => c.args)).toEqual([
      {
        key: "prefs",
        value: { confirmDelete: true, showPast: true, showCompleted: true },
        expectedRevision: 1,
      },
    ]);
  });

  it("a preference write over a revision that moved on is read again and tried once more, never forced", async () => {
    let first = true;
    const pm = await memoryPm({
      seed: { board: BOARD, prefs: { confirmDelete: true } },
      answers: {
        ...answers,
        "storage.set": (a: { key: string; value: unknown; expectedRevision: number }) => {
          if (a.key !== "prefs") return { revision: 2 };
          if (first) {
            first = false;
            return refuse("MOD-4001", "Read it again.", 4);
          }
          // The retry names the revision the re-read gave.
          return a.expectedRevision === 1 ? { revision: 2 } : refuse("MOD-4001", "stale", 9);
        },
      },
    });
    await start(pm);
    await act(async () => void (await pm.settle()));
    const box = [...document.querySelectorAll("label")]
      .find((l) => l.textContent?.includes("Ask before deleting"))
      ?.querySelector("input") as HTMLInputElement;
    await act(async () => box.click());
    await act(async () => void (await pm.settle()));
    const prefs = (m: string) =>
      pm.of(m).filter((c) => (c.args as { key: string }).key === "prefs");
    expect(prefs("storage.set")).toHaveLength(2);
    // One read at the start and one before the retry: the retry is over what PM holds now.
    expect(prefs("storage.get")).toHaveLength(2);
  });

  it("a key PM cannot read leaves the defaults, and the board still draws", async () => {
    const pm = await memoryPm({
      seed: { board: BOARD },
      answers: {
        ...answers,
        "storage.get": (a: { key: string }) =>
          a.key === "prefs" ? { unexpected: 1 } : { value: BOARD, revision: 1 },
      },
    });
    await start(pm);
    await act(async () => void (await pm.settle()));
    expect(document.querySelector("[data-widget-id=n1]")).not.toBeNull();
    const box = [...document.querySelectorAll("label")]
      .find((l) => l.textContent?.includes("Ask before deleting"))
      ?.querySelector("input");
    expect(box?.checked).toBe(true);
  });

  it("answers lifecycle.suspend with {flushed: true} only after the board and the preferences are written", async () => {
    const pm = await memoryPm({ seed: { board: BOARD }, answers });
    await start(pm);
    await act(async () => void (await pm.settle()));
    const press = (label: string) =>
      [...document.querySelectorAll("button")].find((b) => b.textContent === label)?.click();
    await act(async () => press("+ Note"));
    const box = [...document.querySelectorAll("label")]
      .find((l) => l.textContent?.includes("Ask before deleting"))
      ?.querySelector("input") as HTMLInputElement;
    await act(async () => box.click());
    // Neither the board's half-second debounce nor anything else has written yet.
    const board = () =>
      pm.of("storage.set").filter((c) => (c.args as { key: string }).key === "board");
    expect(board()).toHaveLength(0);
    expect(pm.pm.stats.flushes).toBe(0);
    await act(async () => {
      pm.emit("lifecycle.suspend", { reason: "tab" });
      await pm.settle();
    });
    await act(async () => void (await pm.settle()));
    expect(board()).toHaveLength(1);
    expect(pm.storage.get("prefs")).toEqual({
      value: { confirmDelete: false, showPast: true, showCompleted: true },
      revision: 1,
    });
    expect(pm.pm.stats.flushes).toBe(1);
    // The board PM now holds has the note that was added.
    const widgets = (pm.storage.get("board")?.value as { widgets: unknown[] }).widgets;
    expect(widgets).toHaveLength(BOARD.widgets.length + 1);
  });

  it("answers lifecycle.suspend even with nothing to write, and even when the write fails", async () => {
    const pm = await memoryPm({
      seed: { board: BOARD },
      answers: { ...answers, "storage.set": () => refuse("MOD-9001", "PM could not.") },
    });
    await start(pm);
    await act(async () => void (await pm.settle()));
    await act(async () => {
      pm.emit("lifecycle.suspend", { reason: "tab" });
      await pm.settle();
    });
    await act(async () => void (await pm.settle()));
    expect(pm.pm.stats.flushes).toBe(1);
    expect(pm.of("storage.set")).toHaveLength(0);
    await act(async () =>
      [...document.querySelectorAll("button")].find((b) => b.textContent === "+ Note")?.click(),
    );
    await act(async () => {
      pm.emit("lifecycle.suspend", { reason: "tab" });
      await pm.settle();
    });
    await act(async () => void (await pm.settle()));
    expect(pm.of("storage.set").length).toBeGreaterThan(0);
    expect(pm.pm.stats.flushes).toBe(2);
  });

  it("a project renamed while the page is open: the board is read again and shows the new name", async () => {
    const pm = await memoryPm({ seed: { board: BOARD }, answers });
    await start(pm);
    await act(async () => void (await pm.settle()));
    pm.storage.set("board", {
      value: {
        version: 1,
        widgets: [BOARD.widgets[0], { ...BOARD.widgets[1], project: "Atlas 2" }],
      },
      revision: 2,
    });
    await act(async () => {
      pm.emit("projects.renamed", {});
      await pm.settle();
    });
    await act(async () => void (await pm.settle()));
    expect(
      pm.of("storage.get").filter((c) => (c.args as { key: string }).key === "board"),
    ).toHaveLength(2);
    expect(document.body.textContent).toContain("Atlas 2");
  });

  it("says why when it is not inside PM", async () => {
    document.body.innerHTML = '<div id="root"></div>';
    const up = await boot({
      doc: document,
      win: window,
      connect: () => Promise.reject(new HostError("not-framed", "x")),
    });
    expect(up).toBeNull();
    expect(document.body.textContent).toBe(NOT_FRAMED);
  });

  it("says so when PM never answers", async () => {
    document.body.innerHTML = '<div id="root"></div>';
    const up = await boot({
      doc: document,
      win: window,
      connect: () => Promise.reject(new HostError("no-port", "x")),
    });
    expect(up).toBeNull();
    expect(document.body.textContent).toBe(NO_ANSWER);
    expect(STARTING).toMatch(/Starting/);
  });
});
