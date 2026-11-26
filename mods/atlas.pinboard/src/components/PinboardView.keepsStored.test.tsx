// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The view half of the same promise (see usePinboard.keepsStored.test.tsx): in a frame narrower than
// the board, opening and closing every kind of folder writes nothing and a later edit writes only
// itself; and a real press and release on a card's header, with no movement, leaves every card where
// it is stored, even where the narrow frame draws that card on top of a twin.

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { TaskDataProvider } from "../../../../src/kit/taskData";
import { ThemeProvider } from "../../../../src/kit/theme";
import { frameTaskData } from "../frame/taskData";
import { resetPrefs } from "../lib/pinboard/prefs";
import { connectPm } from "../lib/pm";
import { memoryPm, type MemoryPm } from "../testing";
import { PinboardView } from "./PinboardView";

const FRAME_W = 480; // 20 cells: narrower than the board below
let savedInnerWidth = 0;

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
  Element.prototype.getBoundingClientRect = () => new DOMRect(0, 0, FRAME_W, 800);
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
  savedInnerWidth = window.innerWidth;
  Object.defineProperty(window, "innerWidth", { value: FRAME_W, configurable: true });
});
afterAll(() => {
  Object.defineProperty(window, "innerWidth", { value: savedInnerWidth, configurable: true });
  connectPm(null);
});

const store = { get: async () => null, set: async () => {} };
let pm: MemoryPm;
beforeEach(() => {
  resetPrefs();
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
  connectPm(null);
  resetPrefs();
});

// Keys in orders the mod never writes, cards past the frame's right edge, folders of every kind with
// children larger than the folder's own board.
const LEGACY = `{"widgets":[
{"kind":"note","id":"n1","title":"N1","text":"far right","rect":{"w":7,"h":5,"x":30,"y":2}},
{"rect":{"h":5,"w":7,"y":0,"x":0},"id":"n2","kind":"note","title":"N2","text":"top left"},
{"id":"n3","kind":"note","title":"N3","rect":{"y":12,"x":4,"w":40,"h":5},"text":"wide"},
{"title":"Over","children":[
  {"text":"kid one","rect":{"w":30,"h":6,"x":0,"y":0},"kind":"note","id":"c1","title":"C1"},
  {"id":"c2","kind":"note","rect":{"y":0,"x":31,"h":5,"w":7},"text":"kid two","title":"C2"}
 ],"rect":{"w":4,"h":3,"x":25,"y":20},"kind":"folder","id":"f1","expandMode":"overlay"},
{"id":"f2","kind":"folder","rect":{"x":0,"y":30,"w":3,"h":3},"title":"Inline","children":[
  {"rect":{"x":2,"y":1,"w":7,"h":5},"kind":"note","id":"c3","text":"inline kid","title":"C3"}]},
{"game":"coin","gameOn":true,"spent":["c4","gone"],"id":"f3","kind":"folder","rect":{"x":10,"y":30,"w":3,"h":3},"title":"Game","children":[
  {"rect":{"x":0,"y":0,"w":7,"h":5},"kind":"note","id":"c4","text":"heads","title":"C4"},
  {"rect":{"x":8,"y":0,"w":7,"h":5},"kind":"note","id":"c5","text":"tails","title":"C5"}]}
],"version":1}`;

type W = Record<string, unknown> & { id: string };
const legacy = () => JSON.parse(LEGACY) as { widgets: W[] };

async function mount(seed: unknown) {
  pm = await memoryPm({
    seed: { board: seed },
    answers: {
      "projects.list": { projects: [], truncated: false },
      "milestones.list": { milestones: [], next: null },
      "documents.statusOf": {},
      "tasks.list": { tasks: [], stacks: [], next: null },
      "calendar.overlay.set": (a: { entries: unknown[] }) => ({ set: a.entries.length }),
    },
  });
  render(
    <ThemeProvider store={store}>
      <TaskDataProvider data={frameTaskData(pm.host, window)}>
        <PinboardView />
      </TaskDataProvider>
    </ThemeProvider>,
  );
  await settle();
}
const settle = () => act(async () => void (await pm.settle()));
const wait = async () => {
  await act(async () => void vi.advanceTimersByTime(700));
  await settle();
};
const sets = () => pm.of("storage.set");
const storedBoard = () => pm.storage.get("board")!.value as { widgets: W[] };
const byId = (ws: W[], id: string) => ws.find((w) => w.id === id);

describe("a narrow frame, folders opened and closed", () => {
  it("opening and closing every folder (overlay, inline, a game and its cards) writes nothing; the next edit writes only itself", async () => {
    await mount(legacy());
    const click = async (name: RegExp | string) => {
      const found = screen.getAllByRole("button", { name });
      await act(async () => void fireEvent.click(found[0]));
      await settle();
    };
    // The overlay folder, then close.
    await click(/^Open folder — 2 items/);
    expect(screen.getByRole("dialog")).toBeTruthy();
    await click("Close folder");
    // The inline folder, then close.
    await click(/^Open folder — 1 item/);
    await click("Close folder");
    // The game folder: its game face, then its cards, then close.
    await click(/^Play /);
    await wait();
    const cards = screen.getByRole("button", { name: "Cards" });
    await act(async () => void fireEvent.click(cards));
    await settle();
    // The game folder's own board is drawn now: its cards are there.
    expect(
      screen
        .getAllByRole("textbox", { name: "Note title" })
        .some((el) => (el as HTMLInputElement).value === "C5"),
    ).toBe(true);
    await wait();
    await click("Close folder");
    await wait();
    expect(sets()).toHaveLength(0);
    expect(JSON.stringify(storedBoard())).toBe(JSON.stringify(legacy()));

    // A real edit: a note's title.
    const title = screen
      .getAllByRole("textbox", { name: "Note title" })
      .find((el) => (el as HTMLInputElement).value === "N2")!;
    await act(async () => void fireEvent.change(title, { target: { value: "N2 renamed" } }));
    await wait();
    expect(sets()).toHaveLength(1);
    const before = legacy();
    const out = storedBoard();
    for (const id of ["n1", "n3", "f1", "f2", "f3"]) {
      expect(JSON.stringify(byId(out.widgets, id)), id).toBe(
        JSON.stringify(byId(before.widgets, id)),
      );
    }
    expect(byId(out.widgets, "n2")).toEqual({ ...byId(before.widgets, "n2"), title: "N2 renamed" });
  });
});

describe("a click on a card's header (pressed and let go without moving)", () => {
  /** Press and release the header of the note titled `title`, at the same point. */
  async function clickHeader(title: string, at: { x: number; y: number }) {
    const input = screen
      .getAllByRole("textbox", { name: "Note title" })
      .find((el) => (el as HTMLInputElement).value === title)!;
    const header = input.parentElement!;
    await act(async () => {
      fireEvent.pointerDown(header, { clientX: at.x, clientY: at.y, pointerId: 1, button: 0 });
    });
    await act(async () => {
      window.dispatchEvent(new MouseEvent("pointerup", { clientX: at.x, clientY: at.y }));
    });
    await wait();
  }

  it("on a card the narrow frame drew exactly on top of another of the same size: no folder is made, nothing moves", async () => {
    // N1 is stored at x 30 (off a 20-cell frame), so it is drawn in the first free slot among the cards
    // BEFORE it: (0,0), exactly where N2 (later in the list) sits at the same 7x5 size.
    await mount(legacy());
    await clickHeader("N1", { x: 30, y: 6 }); // cell (1,0): N1's header
    const out = storedBoard();
    // Nothing but paint order may change: no new folder, N1 and N2 still on the board where stored.
    expect(
      out.widgets
        .filter((w) => w.kind === "folder")
        .map((w) => w.id)
        .sort(),
    ).toEqual(["f1", "f2", "f3"]);
    expect(byId(out.widgets, "n2")?.rect).toEqual({ x: 0, y: 0, w: 7, h: 5 });
    expect(byId(out.widgets, "n1")?.rect).toEqual({ x: 30, y: 2, w: 7, h: 5 });
  });

  it("on a card the narrow frame drew over a folder: nothing is filed, nothing moves", async () => {
    // The note is stored at x 30 (off a 20-cell frame), so it is drawn at (0,0), over the folder that
    // sits there. The pointer is on the note's header, which is also over the folder's tile.
    const seed = {
      widgets: [
        { kind: "note", id: "nf", text: "far", title: "NF", rect: { x: 30, y: 2, w: 7, h: 5 } },
        { id: "fz", kind: "folder", rect: { x: 0, y: 0, w: 3, h: 3 }, title: "", children: [] },
      ],
      version: 1,
    };
    await mount(seed);
    await clickHeader("NF", { x: 30, y: 6 });
    const out = storedBoard();
    expect(byId(out.widgets, "fz")?.children).toEqual([]);
    expect(byId(out.widgets, "nf")?.rect).toEqual({ x: 30, y: 2, w: 7, h: 5 });
  });
});
