// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A save writes only what the person changed. Held against the real `lib/pm` over the real bridge to
// the fake PM, on a board whose objects are in an order the mod never writes itself, in a frame
// narrower than the board (so the drawing reflows): a card nobody edited is written back exactly as it
// was stored (its position, its size, every field, the order of its keys) through typing into one
// note, adding and deleting a card, undo and redo, every gesture that ends in a commit without
// moving anything, a width change, a stale-revision re-read and a refresh. A press on a card's
// header that does not move it writes nothing, even where the narrow frame draws the card on top of
// a twin or a folder. And a board PM hands back with its keys in another order is the same board.

import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { connectPm } from "../pm";
import { memoryPm, type MemoryPm } from "../../testing";
import type { Rect } from "./types";
import { flushBoard, refreshBoard, usePinboard } from "./usePinboard";

const NARROW = { cols: 20, rows: 40 };
const WIDE = { cols: 60, rows: 40 };

// Every object in an order the mod would not write: rect keys shuffled, `kind` before `id`, the
// top-level `widgets` before `version`, fields the build does not know, a folder whose round names a
// card that left, timeline items missing fields the drawing fills in, a rect below the minimum and a
// fractional one, a card kind from a newer build.
const LEGACY = `{"widgets":[
{"kind":"note","id":"n1","text":"far right","color":"st-due","rect":{"w":7,"h":5,"x":30,"y":2}},
{"rect":{"h":5,"w":7,"y":0,"x":0},"id":"n2","kind":"note","text":"- [ ] \\nhello"},
{"id":"n3","kind":"note","rect":{"y":12,"x":4,"w":40,"h":5},"text":"wider than the frame"},
{"x_future":{"a":[1,{"b":2}]},"text":"slides","rect":{"x":15,"y":30,"w":7,"h":5},"kind":"note","id":"n4"},
{"title":"T","items":[{"label":"pitch","date":"2026-12-01","id":"i1"},{"id":"i2"}],"rect":{"x":0,"y":40,"w":9,"h":8},"id":"t1","kind":"timeline"},
{"title":"F","spent":["gone","c1"],"game":"roulette","children":[
  {"text":"kid one","rect":{"w":30,"h":6,"x":0,"y":0},"kind":"note","id":"c1"},
  {"id":"c2","kind":"note","rect":{"y":0,"x":31,"h":5,"w":7},"text":"kid two","weight":2},
  {"id":"c3","kind":"note","rect":{"x":0,"y":7,"w":2,"h":1},"text":"kid three, too small"}
 ],"rect":{"w":4,"h":3,"x":25,"y":5},"kind":"folder","id":"f1","expandMode":"overlay"},
{"id":"f2","kind":"folder","rect":{"x":0,"y":50,"w":3,"h":3},"title":"","children":[]},
{"project":null,"title":"","rect":{"h":8,"w":9,"x":10,"y":40},"kind":"tasklist","id":"l1"},
{"id":"tiny","kind":"note","rect":{"x":2.4,"y":60,"w":1,"h":1},"text":"tiny"},
{"kind":"hologram","id":"z","rect":{"w":2,"h":2,"x":1,"y":1}}
],"version":1,"zzz":"kept"}`;

type W = Record<string, unknown> & { id: string };
const legacy = () => JSON.parse(LEGACY) as { widgets: W[] };
const KEPT = ["n1", "n3", "n4", "t1", "f1", "f2", "l1", "tiny", "z"];

let mem: MemoryPm;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
});
afterEach(() => {
  vi.useRealTimers();
  connectPm(null);
});

async function open(seed: unknown, space = NARROW) {
  mem = await memoryPm({
    seed: { board: seed },
    answers: { "calendar.overlay.set": (a: { entries: unknown[] }) => ({ set: a.entries.length }) },
  });
  const hook = renderHook(({ s }) => usePinboard(s), { initialProps: { s: space } });
  await settle();
  return hook;
}
const settle = () => act(async () => void (await mem.settle()));
const debounce = async () => {
  act(() => void vi.advanceTimersByTime(600));
  await settle();
};
const flush = () =>
  act(async () => {
    const done = flushBoard();
    await mem.settle();
    await done;
  });
const sets = () => mem.of("storage.set").filter((c) => (c.args as { key: string }).key === "board");
/** The board each write SENT, as JSON text exactly as the mod built it (key order included). */
const sent = () => sets().map((c) => (c.args as { value: { widgets: W[] } }).value);
const byId = (ws: W[], id: string) => ws.find((w) => w.id === id);

/** Every card in `ids` is, as the mod sends it, exactly the card in `from` (key order too). */
function expectKept(board: { widgets: W[] }, from: { widgets: W[] }, ids = KEPT) {
  for (const id of ids) {
    expect(JSON.stringify(byId(board.widgets, id)), id).toBe(
      JSON.stringify(byId(from.widgets, id)),
    );
  }
}

describe("a card nobody edited crosses every save unchanged: values, rects and key order", () => {
  it("an editing session in a narrow frame, plus every gesture that commits without moving", async () => {
    const before = legacy();
    const { result } = await open(legacy(), NARROW);
    const drawn = (id: string) =>
      byId(result.current.board.widgets as unknown as W[], id)! as W & { rect: Rect };
    // The drawing really reflowed (else this proves nothing): far, wide, sliding and folder cards.
    expect(drawn("n1").rect).not.toEqual({ x: 30, y: 2, w: 7, h: 5 });
    expect(drawn("n3").rect).not.toEqual({ x: 4, y: 12, w: 40, h: 5 });
    expect(drawn("n4").rect).toEqual({ x: 13, y: 30, w: 7, h: 5 });
    expect(drawn("f1").rect).not.toEqual({ x: 25, y: 5, w: 4, h: 3 });
    expect(drawn("tiny").rect).toEqual({ x: 2, y: 60, w: 4, h: 3 });

    // 1. Typing into one note.
    act(() => void result.current.updateWidget("n2", { text: "- [ ] \nhello w" }));
    act(() => void result.current.updateWidget("n2", { text: "- [ ] \nhello world" }));
    await debounce();
    // 2. A test note added, saved, then deleted, saved.
    let added = "";
    act(() => void (added = result.current.addNote()));
    await debounce();
    act(() => void result.current.removeWidget(added));
    await debounce();
    // 3. A press on a reflowed card's header with no movement (raise + drop where it is drawn), and
    //    on its resize handle (resize to the size it is drawn at).
    act(() => void result.current.raiseWidget("n4"));
    act(
      () =>
        void result.current.dropWidget("n4", { ...drawn("n4").rect } as never, { x: 14, y: 31 }),
    );
    act(() => void result.current.moveWidget("n3", { ...drawn("n3").rect } as never));
    // 4. Inside the open folder: a press on a child's header with no movement (raise in the folder,
    //    then the folder board commits the rect the child is drawn at).
    const kid = (drawn("f1").children as W[]).find((c) => c.id === "c1")!;
    act(() => void result.current.raiseChild("f1", "c1"));
    act(
      () => void result.current.updateWidget("c1", { rect: { ...(kid.rect as object) } } as never),
    );
    await debounce();
    // 5. The empty checkbox line deleted by hand.
    act(() => void result.current.updateWidget("n2", { text: "hello world" }));
    await debounce();
    // 6. Undo it and redo it.
    act(() => void result.current.undo());
    await debounce();
    act(() => void result.current.redo());
    await flush();

    const writes = sent();
    expect(writes.length).toBeGreaterThanOrEqual(4);
    const kids = (b: { widgets: W[] }) => byId(b.widgets, "f1")!.children as W[];
    for (const board of writes) {
      // The folder's child that was pressed was raised in its folder (paint order, a real silent
      // change); every child and every other field of the folder exactly as stored.
      const without = (o: object, key: string) =>
        JSON.stringify(Object.fromEntries(Object.entries(o).filter(([k]) => k !== key)));
      expect(without(byId(board.widgets, "f1")!, "children")).toBe(
        without(byId(before.widgets, "f1")!, "children"),
      );
      for (const c of kids(before)) {
        expect(JSON.stringify(byId(kids(board), c.id))).toBe(JSON.stringify(c));
      }
      // The top level as it was (its own key order, the unknown field) apart from the widgets.
      expect(without(board, "widgets")).toBe(JSON.stringify({ version: 1, zzz: "kept" }));
      expect(Object.keys(board)).toEqual(["widgets", "version", "zzz"]);
      // Every card nobody touched, exactly as stored.
      expectKept(
        board,
        before,
        KEPT.filter((id) => id !== "f1"),
      );
    }
    expect(kids(writes[writes.length - 1]).map((c) => c.id)).toEqual(["c2", "c3", "c1"]);
    // The one real change, and nothing else about that card.
    const last = mem.storage.get("board")!.value as { widgets: W[] };
    expect(JSON.stringify(byId(last.widgets, "n2"))).toBe(
      JSON.stringify({
        rect: { h: 5, w: 7, y: 0, x: 0 },
        id: "n2",
        kind: "note",
        text: "hello world",
      }),
    );
    // The test note is gone; the grab raised n4 (array order only, a real silent change).
    expect(last.widgets.map((w) => w.id)).toEqual([
      "n1",
      "n2",
      "n3",
      "t1",
      "f1",
      "f2",
      "l1",
      "tiny",
      "z",
      "n4",
    ]);
  });

  it("the frame gets WIDER, then a stale revision is read again and merged: cards nobody edited are the other writer's, exactly", async () => {
    const { result, rerender } = await open(legacy(), NARROW);
    rerender({ s: WIDE });
    await settle();
    // The other writer (a project rename in PM's Rust) rewrites the board and the revision moves on.
    const theirs = legacy();
    (byId(theirs.widgets, "l1") as W).project = "New";
    mem.storage.set("board", { value: theirs, revision: 2 });
    act(() => void result.current.updateWidget("n2", { text: "ours" }));
    await debounce();
    expect(sets().map((c) => (c.args as { expectedRevision: number }).expectedRevision)).toEqual([
      1, 2,
    ]);
    const out = mem.storage.get("board")!.value as { widgets: W[] };
    expectKept(out, theirs);
    expect(byId(out.widgets, "n2")!.text).toBe("ours");
    // A later edit after the re-read: still only that edit.
    act(() => void result.current.updateWidget("n4", { text: "slides, typed" }));
    await debounce();
    const again = mem.storage.get("board")!.value as { widgets: W[] };
    expectKept(
      again,
      theirs,
      KEPT.filter((id) => id !== "n4"),
    );
    expect(byId(again.widgets, "n4")!.rect).toEqual({ x: 15, y: 30, w: 7, h: 5 });
  });

  it("the frame gets NARROWER, then a stale revision: the same", async () => {
    const { result, rerender } = await open(legacy(), WIDE);
    rerender({ s: NARROW });
    await settle();
    const theirs = legacy();
    (byId(theirs.widgets, "l1") as W).project = "New";
    mem.storage.set("board", { value: theirs, revision: 2 });
    act(() => void result.current.updateWidget("n2", { text: "ours" }));
    await debounce();
    const out = mem.storage.get("board")!.value as { widgets: W[] };
    expectKept(out, theirs);
    expect(byId(out.widgets, "n2")!.text).toBe("ours");
  });

  it("a refresh after a width change writes nothing, and the next edit writes only itself", async () => {
    const before = legacy();
    const { result, rerender } = await open(legacy(), NARROW);
    rerender({ s: WIDE });
    await act(async () => {
      const done = refreshBoard();
      await mem.settle();
      await done;
    });
    await debounce();
    expect(sets()).toHaveLength(0);
    act(() => void result.current.updateWidget("n2", { text: "after refresh" }));
    await debounce();
    expect(sets()).toHaveLength(1);
    expectKept(mem.storage.get("board")!.value as { widgets: W[] }, before);
  });
});

describe("a click that does not move a card (a press and release on its header) changes nothing", () => {
  it("on a card the narrow frame drew exactly on top of a same-size card: no folder, both stay where stored", async () => {
    // n1 is stored at x 30, past a 20-cell frame: it is drawn in the first slot free of the cards
    // BEFORE it, (0,0), exactly where n2 sits at the same 7x5. The person never stacked them.
    const before = legacy();
    const { result } = await open(legacy(), NARROW);
    const drawn = (id: string) =>
      byId(result.current.board.widgets as unknown as W[], id)! as W & { rect: Rect };
    expect(drawn("n1").rect).toEqual(drawn("n2").rect);
    // What useBoardDrag commits for a press and release with no movement: raise, then a drop at the
    // rect it is drawn at, with the pointer on its header.
    act(() => void result.current.raiseWidget("n1"));
    act(
      () =>
        void result.current.dropWidget("n1", { ...(drawn("n1").rect as object) } as never, {
          x: 1,
          y: 0,
        }),
    );
    await debounce();
    const out = mem.storage.get("board")!.value as { widgets: W[] };
    expect(out.widgets.filter((w) => w.kind === "folder").map((w) => w.id)).toEqual(["f1", "f2"]);
    expectKept(out, before, ["n1", "n2"]);
  });

  it("on a card the narrow frame drew over a folder, pressed where the folder lies under it: not filed", async () => {
    const stored = JSON.parse(`{"widgets":[
      {"kind":"note","id":"nf","text":"far","rect":{"w":7,"h":5,"x":30,"y":2}},
      {"id":"fz","kind":"folder","rect":{"x":0,"y":0,"w":3,"h":3},"title":"","children":[]}
    ],"version":1}`) as { widgets: W[] };
    const { result } = await open(JSON.parse(JSON.stringify(stored)), NARROW);
    const drawn = (id: string) =>
      byId(result.current.board.widgets as unknown as W[], id)! as W & { rect: Rect };
    expect(drawn("nf").rect).toEqual({ x: 0, y: 0, w: 7, h: 5 }); // drawn over the folder
    act(() => void result.current.raiseWidget("nf"));
    act(
      () =>
        void result.current.dropWidget("nf", { ...(drawn("nf").rect as object) } as never, {
          x: 1,
          y: 0,
        }),
    );
    await debounce();
    const out = mem.storage.get("board")!.value as { widgets: W[] };
    expect(byId(out.widgets, "fz")!.children).toEqual([]);
    expect(JSON.stringify(byId(out.widgets, "nf"))).toBe(
      JSON.stringify(byId(stored.widgets, "nf")),
    );
  });
});

// --- a store that hands the board back with its keys in another order -------------------------------

/** What PM's store does to a board it keeps as JSON: every object's keys come back in byte order (a
 *  sorted map), arrays as they are. The values never change, only the order of the keys. */
function sortKeys(v: unknown): unknown {
  if (Array.isArray(v)) return v.map(sortKeys);
  if (v && typeof v === "object") {
    const o = v as Record<string, unknown>;
    return Object.fromEntries(
      Object.keys(o)
        .sort((a, b) => (a < b ? -1 : a > b ? 1 : 0))
        .map((k) => [k, sortKeys(o[k])]),
    );
  }
  return v;
}

describe("a board PM hands back with its keys in another order is the same board", () => {
  it("opening and refreshing write nothing, and one edit sends the board with only that edit in it", async () => {
    let row = { text: LEGACY, revision: 1 };
    mem = await memoryPm({
      answers: {
        "calendar.overlay.set": (a: { entries: unknown[] }) => ({ set: a.entries.length }),
        "storage.get": () => ({ value: sortKeys(JSON.parse(row.text)), revision: row.revision }),
        "storage.set": (a: { value: unknown; expectedRevision: number }) => {
          row = { text: JSON.stringify(sortKeys(a.value)), revision: row.revision + 1 };
          return { revision: row.revision };
        },
      },
    });
    const { result } = renderHook(() => usePinboard(NARROW));
    await settle();
    // Reading it is not a change: the first save is the first write.
    await act(async () => {
      const done = refreshBoard();
      await mem.settle();
      await done;
    });
    await debounce();
    expect(sets()).toHaveLength(0);
    expect(row.text).toBe(LEGACY);

    act(() => void result.current.updateWidget("n2", { text: "- [ ] \nhello world" }));
    await debounce();
    expect(sets()).toHaveLength(1);
    // The board that was stored, with that one note's text changed and nothing else: the same values
    // everywhere (the order of an object's keys is the store's to choose, so it is not compared).
    const expected = legacy();
    (byId(expected.widgets, "n2") as W).text = "- [ ] \nhello world";
    expect(JSON.stringify(sortKeys(JSON.parse(row.text)))).toBe(JSON.stringify(sortKeys(expected)));
  });
});
