// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What keeps the live board safe (Mods plan K7's must-fix list), held against the real `lib/pm` over
// the real bridge to a fake PM that keeps revisions as PM does. The persistence contract of the copied
// hook is in usePinboard.test.tsx; this is what the mod adds: nothing written on load, a board it
// cannot read left alone, cards it cannot read kept, reflow never saved, one save over the cap kept in
// memory, a stale revision merged and never overwritten, the overlay told, a flush on suspend.

import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import fixtures from "../../../../../tests/pinboard-overlay-fixtures.json";
import { connectPm, PmError } from "../pm";
import { memoryPm, refuse, type MemoryPm } from "../../testing";
import { MAX_ARGS_BYTES } from "./boardSync";
import { flushBoard, refreshBoard, usePinboard } from "./usePinboard";

const NARROW = { cols: 20, rows: 40 };

const note = (id: string, extra: object = {}) => ({
  id,
  kind: "note",
  rect: { x: 0, y: 0, w: 7, h: 5 },
  text: `note ${id}`,
  ...extra,
});
const timeline = (id: string, items: unknown[], extra: object = {}) => ({
  id,
  kind: "timeline",
  rect: { x: 0, y: 6, w: 9, h: 8 },
  title: "",
  items,
  ...extra,
});
const board = (widgets: unknown[], extra: object = {}) => ({ version: 1, widgets, ...extra });

let mem: MemoryPm;
let overlays: unknown[][];

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  overlays = [];
});
afterEach(() => {
  vi.useRealTimers();
  connectPm(null);
});

/** Open the hook on a stored value (a board, or anything else) and let the load land. */
async function open(stored: unknown, space = NARROW, answers: Record<string, unknown> = {}) {
  mem = await memoryPm({
    seed: stored === undefined ? {} : { board: stored },
    answers: {
      "calendar.overlay.set": (a: { entries: unknown[] }) => {
        overlays.push(a.entries);
        return { set: a.entries.length };
      },
      ...answers,
    },
  });
  const hook = renderHook(() => usePinboard(space));
  await settle();
  return hook;
}

/** Let calls and answers cross the bridge. */
const settle = () => act(async () => void (await mem.settle()));
/** Run out the save debounce, then let the write cross. */
const debounce = async () => {
  act(() => void vi.advanceTimersByTime(600));
  await settle();
};
const sets = () => mem.of("storage.set").filter((c) => (c.args as { key: string }).key === "board");
const stored = () => mem.storage.get("board")?.value as { widgets: Record<string, any>[] }; // eslint-disable-line @typescript-eslint/no-explicit-any

describe("never writes on load", () => {
  it("a board that arrives is not written back: not after the debounce, not on unmount, whatever it holds", async () => {
    // A board a newer build wrote: an unknown card, an unknown field, a card that reflows, a rect below
    // the minimum, a folder holding a child this build cannot read. None of it may cause a write.
    const original = board(
      [
        note("a", { x_future: [1, 2] }),
        { id: "z", kind: "hologram", rect: { x: 1, y: 1, w: 2, h: 2 } },
        note("far", { rect: { x: 30, y: 0, w: 7, h: 5 } }),
        note("tiny", { rect: { x: 2, y: 20, w: 1, h: 1 } }),
        {
          id: "f",
          kind: "folder",
          rect: { x: 10, y: 10, w: 3, h: 3 },
          children: [note("c"), { id: "k", kind: "hologram", rect: { x: 0, y: 0, w: 1, h: 1 } }],
        },
      ],
      { more: "kept" },
    );
    const { result, unmount } = await open(original);
    expect(result.current.load).toBe("ready");
    await debounce();
    unmount();
    await settle();
    expect(sets()).toHaveLength(0);
    expect(mem.storage.get("board")).toEqual({ value: original, revision: 1 });
  });

  it("a fresh install writes nothing until the person adds something", async () => {
    const { result } = await open(undefined);
    expect(result.current.load).toBe("ready");
    await debounce();
    expect(sets()).toHaveLength(0);
    act(() => void result.current.addNote());
    await debounce();
    expect(sets()).toHaveLength(1);
    expect(stored().widgets).toHaveLength(1);
  });

  it("an edit that is undone back to what was stored writes nothing", async () => {
    const { result } = await open(board([note("a")]));
    act(() => void result.current.updateWidget("a", { color: "st-due" }));
    act(() => void result.current.undo());
    await debounce();
    expect(sets()).toHaveLength(0);
  });
});

describe("refuses to write a board it cannot read", () => {
  it.each([
    ["a newer version", { version: 2, widgets: [note("a")] }, "version"],
    ["no version", { widgets: [note("a")] }, "version"],
    ["something that is not a board", { hello: "world" }, "shape"],
    ["a list", [1, 2, 3], "shape"],
  ])(
    "%s is shown as unreadable and never written, not on a change and not on unmount",
    async (_n, value, why) => {
      const { result, unmount } = await open(value);
      expect(result.current.load).toBe("unreadable");
      expect(result.current.unreadable).toBe(why);
      expect(result.current.board.widgets).toHaveLength(0);
      act(() => void result.current.addNote());
      await debounce();
      unmount();
      await settle();
      expect(sets()).toHaveLength(0);
      expect(mem.storage.get("board")?.value).toEqual(value);
      // And the Calendar is not told an empty list over dates this build could not read.
      expect(overlays).toHaveLength(0);
    },
  );

  it("a folder inside a folder is not drawn (folders never nest) and is kept, however deep the chain goes", async () => {
    let deep: unknown = { id: "x", kind: "note", rect: { x: 0, y: 0, w: 7, h: 5 } };
    for (let i = 0; i < 100; i += 1) {
      deep = { id: `f${i}`, kind: "folder", rect: { x: 0, y: 0, w: 3, h: 3 }, children: [deep] };
    }
    const { result } = await open(board([deep, note("a")]));
    expect(result.current.load).toBe("ready");
    expect(result.current.board.widgets.map((w) => w.id)).toEqual(["a"]);
    act(() => void result.current.updateWidget("a", { text: "edited" }));
    await debounce();
    expect(stored().widgets[0]).toEqual(deep);
    expect(stored().widgets[1].text).toBe("edited");
  });
});

describe("keeps what it cannot read", () => {
  it("writes unknown cards, unknown fields of known cards and unknown top-level fields back as they were", async () => {
    const hologram = {
      id: "z",
      kind: "hologram",
      rect: { x: 1, y: 1, w: 2, h: 2 },
      payload: ["x"],
    };
    const { result } = await open(
      board([note("a", { x_future: { deep: 1 } }), hologram, note("b")], { top: "level" }),
    );
    act(() => void result.current.updateWidget("b", { text: "edited" }));
    await debounce();
    const out = stored();
    expect(out).toMatchObject({ version: 1, top: "level" });
    expect(out.widgets.map((w) => w.id)).toEqual(["a", "z", "b"]);
    expect(out.widgets[0]).toEqual(note("a", { x_future: { deep: 1 } }));
    expect(out.widgets[1]).toEqual(hologram);
    expect(out.widgets[2].text).toBe("edited");
  });

  it("ungrouping a folder keeps the children this build could not draw, as they were", async () => {
    const hologram = { id: "k", kind: "hologram", rect: { x: 0, y: 0, w: 1, h: 1 }, p: [1] };
    const badRect = { id: "m", kind: "note", rect: { x: "no" }, text: "kept" };
    const inner = { id: "g", kind: "folder", rect: { x: 0, y: 0, w: 3, h: 3 }, children: [] };
    const folder = {
      id: "f",
      kind: "folder",
      rect: { x: 0, y: 0, w: 5, h: 5 },
      children: [note("c"), hologram, badRect, inner],
    };
    const { result } = await open(board([folder]));
    act(() => void result.current.removeWidget("f"));
    await debounce();
    const out = stored().widgets;
    expect(out.map((w) => w.id).sort()).toEqual(["c", "g", "k", "m"]);
    expect(out.find((w) => w.id === "k")).toEqual(hologram);
    expect(out.find((w) => w.id === "m")).toEqual(badRect);
    expect(out.find((w) => w.id === "g")).toEqual(inner);
    expect(out.find((w) => w.id === "c")).toMatchObject({ kind: "note", text: "note c" });
  });

  it("an edit to a card does not drop what a field's wrong type kept the view from drawing", async () => {
    // `text` here is a number: the view does not draw it, and an edit to the card's colour leaves it.
    const { result } = await open(board([note("a", { text: 12345 })]));
    expect((result.current.board.widgets[0] as { text?: unknown }).text).toBeUndefined();
    act(() => void result.current.updateWidget("a", { color: "st-due" }));
    await debounce();
    expect(stored().widgets[0]).toEqual(note("a", { text: 12345, color: "st-due" }));
  });
});

describe("reflow is display-only", () => {
  const crowded = () =>
    board([
      note("a", { rect: { x: 0, y: 0, w: 7, h: 5 } }),
      note("far", { rect: { x: 30, y: 2, w: 7, h: 5 } }),
      note("wide", { rect: { x: 4, y: 12, w: 40, h: 5 } }),
      note("b", { rect: { x: 8, y: 0, w: 7, h: 5 } }),
    ]);

  it("a board that reflows, with one card moved: every other stored rect is sent exactly as it was", async () => {
    const before = crowded();
    const { result } = await open(before);
    // The view really did reflow them (otherwise this proves nothing).
    const drawn = (id: string) => result.current.board.widgets.find((w) => w.id === id)!.rect;
    expect(drawn("far")).not.toEqual({ x: 30, y: 2, w: 7, h: 5 });
    expect(drawn("wide")).not.toEqual({ x: 4, y: 12, w: 40, h: 5 });
    const moved = { x: 2, y: 20, w: 7, h: 5 };
    act(() => void result.current.moveWidget("a", moved));
    await debounce();
    const out = stored();
    const rect = (w: Record<string, any>) => JSON.stringify(w.rect); // eslint-disable-line @typescript-eslint/no-explicit-any
    expect(out.widgets.find((w) => w.id === "a")!.rect).toEqual(moved);
    for (const id of ["far", "wide", "b"]) {
      expect(rect(out.widgets.find((w) => w.id === id)!)).toBe(
        rect(before.widgets.find((w) => (w as { id: string }).id === id) as Record<string, any>), // eslint-disable-line @typescript-eslint/no-explicit-any
      );
    }
  });

  it("a note-only edit changes no rect at all", async () => {
    const before = crowded();
    const { result } = await open(before);
    act(() => void result.current.updateWidget("far", { text: "typed" }));
    await debounce();
    const out = stored();
    expect(out.widgets.map((w) => JSON.stringify(w.rect))).toEqual(
      before.widgets.map((w) => JSON.stringify((w as { rect: unknown }).rect)),
    );
    expect(out.widgets.find((w) => w.id === "far")!.text).toBe("typed");
  });
});

describe("the size guard", () => {
  const bulky = (bytes: number) => note("big", { text: "x".repeat(bytes) });

  it("measures what storage.set would send, and a board under the cap is saved with the meter showing it", async () => {
    const { result } = await open(board([note("a")]));
    act(() => void result.current.updateWidget("a", { text: "hello" }));
    await debounce();
    const call = sets()[0].args;
    const sent = new TextEncoder().encode(JSON.stringify(call)).length;
    // Counted at the longest revision, so never less than what was sent, and only a few bytes more.
    expect(result.current.sizeBytes).toBeGreaterThanOrEqual(sent);
    expect(result.current.sizeBytes - sent).toBeLessThan(20);
    expect(result.current.tooBig).toBe(false);
    expect(result.current.fullness).toBe(1);
  });

  it("over the cap: not sent, shown as too big, and the edit stays in memory; making room saves it all", async () => {
    const { result } = await open(board([note("a"), bulky(MAX_ARGS_BYTES - 2000)]));
    expect(result.current.tooBig).toBe(false);
    const attempts = vi.spyOn(mem.host, "call");
    act(() => void result.current.updateWidget("a", { text: "y".repeat(5000) }));
    await debounce();
    expect(result.current.tooBig).toBe(true);
    expect(result.current.sizeBytes).toBeGreaterThan(MAX_ARGS_BYTES);
    expect(result.current.fullness).toBe(100);
    // Nothing was even attempted, and the edit is still on the board.
    expect(attempts.mock.calls.filter(([m]) => m === "storage.set")).toHaveLength(0);
    expect(sets()).toHaveLength(0);
    expect((result.current.board.widgets[0] as { text: string }).text).toHaveLength(5000);
    // Making room: the whole edit goes with the next save.
    act(() => void result.current.removeWidget("big"));
    await debounce();
    expect(result.current.tooBig).toBe(false);
    expect(sets()).toHaveLength(1);
    expect(stored().widgets).toHaveLength(1);
    expect(stored().widgets[0].text).toHaveLength(5000);
  });

  it("PM refusing the size anyway (or a full store) is the same message, not a plain failure", async () => {
    const { result } = await open(board([note("a")]), NARROW, {
      "storage.set": () => refuse("MOD-3003", "That is more than this mod may keep."),
    });
    act(() => void result.current.updateWidget("a", { text: "hello" }));
    await debounce();
    expect(result.current.tooBig).toBe(true);
    expect(result.current.saveFailed).toBe(false);
  });
});

describe("revision conflicts", () => {
  it("a stale revision is read again and merged, never overwritten: their changes stay, ours are added", async () => {
    const { result } = await open(
      board([
        { id: "l", kind: "tasklist", rect: { x: 0, y: 10, w: 9, h: 8 }, title: "", project: "Old" },
        note("a"),
      ]),
    );
    // Another writer (a project rename, PM's own rewrite of the names in the board) changes the board
    // while our frame is open, and the revision moves on.
    mem.storage.set("board", {
      value: board([
        { id: "l", kind: "tasklist", rect: { x: 0, y: 10, w: 9, h: 8 }, title: "", project: "New" },
        note("a"),
        note("theirs"),
      ]),
      revision: 2,
    });
    act(() => void result.current.updateWidget("a", { text: "ours" }));
    await debounce();
    const out = stored();
    expect(out.widgets.map((w) => w.id)).toEqual(["l", "a", "theirs"]);
    expect(out.widgets[0].project).toBe("New");
    expect(out.widgets[1].text).toBe("ours");
    expect(mem.storage.get("board")?.revision).toBe(3);
    // The first attempt was refused, then the board was read again, then written over THAT revision.
    const writes = sets().map((c) => (c.args as { expectedRevision: number }).expectedRevision);
    expect(writes).toEqual([1, 2]);
    // And what the person sees is the merge.
    expect(result.current.board.widgets.map((w) => w.id)).toEqual(["l", "a", "theirs"]);
    expect(result.current.saveFailed).toBe(false);
  });

  it("gives up after a few conflicts in a row, says so, and keeps the edit", async () => {
    const { result } = await open(board([note("a")]), NARROW, {
      "storage.set": () => refuse("MOD-4001", "Read it again.", 9),
    });
    act(() => void result.current.updateWidget("a", { text: "ours" }));
    await debounce();
    for (let i = 0; i < 6; i += 1) await settle();
    expect(result.current.saveFailed).toBe(true);
    expect((result.current.board.widgets[0] as { text: string }).text).toBe("ours");
  });

  it("PM's copy turning into a board this build cannot read stops the saving, and writes nothing over it", async () => {
    const { result } = await open(board([note("a")]));
    mem.storage.set("board", { value: { version: 3, widgets: [] }, revision: 2 });
    act(() => void result.current.updateWidget("a", { text: "ours" }));
    await debounce();
    expect(result.current.saveFailed).toBe(true);
    expect(mem.storage.get("board")).toEqual({ value: { version: 3, widgets: [] }, revision: 2 });
    act(() => void result.current.updateWidget("a", { text: "ours again" }));
    await debounce();
    expect(sets()).toHaveLength(1);
  });

  it("once PM's copy can be read again the next change saves over it, merged, and the warning goes", async () => {
    const { result } = await open(
      board([note("a"), note("b", { rect: { x: 8, y: 0, w: 7, h: 5 } })]),
    );
    mem.storage.set("board", { value: { version: 3, widgets: [] }, revision: 2 });
    act(() => void result.current.updateWidget("a", { text: "ours" }));
    await debounce();
    expect(result.current.blocked).toBe(true);
    expect(sets()).toHaveLength(1);
    // The board becomes readable again (somebody restored it), with a card of theirs.
    mem.storage.set("board", { value: board([note("a"), note("theirs")]), revision: 3 });
    act(() => void result.current.updateWidget("a", { text: "ours again" }));
    await debounce();
    expect(result.current.blocked).toBe(false);
    expect(result.current.saveFailed).toBe(false);
    expect(stored().widgets.map((w) => w.id)).toEqual(["a", "theirs"]);
    expect(stored().widgets[0].text).toBe("ours again");
  });

  it("a project renamed while the frame is open: refreshing reads the board again and shows the new name, keeping an edit not yet written", async () => {
    const { result } = await open(
      board([
        { id: "l", kind: "tasklist", rect: { x: 0, y: 10, w: 9, h: 8 }, title: "", project: "Old" },
        note("a"),
      ]),
    );
    act(() => void result.current.updateWidget("a", { text: "typing" }));
    mem.storage.set("board", {
      value: board([
        { id: "l", kind: "tasklist", rect: { x: 0, y: 10, w: 9, h: 8 }, title: "", project: "New" },
        note("a"),
      ]),
      revision: 2,
    });
    await act(async () => {
      const done = refreshBoard();
      await mem.settle();
      await done;
    });
    const drawn = result.current.board.widgets as { id: string; project?: string; text?: string }[];
    expect(drawn[0].project).toBe("New");
    expect(drawn[1].text).toBe("typing");
    expect(sets()).toHaveLength(0);
    await debounce();
    expect(stored().widgets[0].project).toBe("New");
    expect(stored().widgets[1].text).toBe("typing");
  });
});

describe("the Calendar's dates (the overlay)", () => {
  const dated = timeline("t", [
    { id: "i1", date: "2026-12-01", label: "pitch" },
    { id: "i2", date: "2026-12-02", label: "demo" },
  ]);

  it("is told once when the frame starts, and again after a saved change that touches a dated entry, not after one that does not", async () => {
    const { result } = await open(board([dated, note("a")]));
    expect(overlays).toEqual([
      [
        { id: "t:i1", date: "2026-12-01", label: "pitch" },
        { id: "t:i2", date: "2026-12-02", label: "demo" },
      ],
    ]);
    act(() => void result.current.updateWidget("a", { text: "no dates here" }));
    await debounce();
    expect(overlays).toHaveLength(1);
    act(() => void result.current.updateTimelineItem("t", "i2", { label: "demo day" }));
    await debounce();
    expect(overlays).toHaveLength(2);
    expect(overlays[1]).toEqual([
      { id: "t:i1", date: "2026-12-01", label: "pitch" },
      { id: "t:i2", date: "2026-12-02", label: "demo day" },
    ]);
    act(() => void result.current.removeWidget("t"));
    await debounce();
    expect(overlays[2]).toEqual([]);
  });

  it("is told again at every frame start: a pause (a blacklist entry) deletes the mod's dates, and the frame that starts on resume puts them back as the fixtures say", async () => {
    // PM's table of the mod's Calendar dates, as the real one is kept: replaced by each
    // `calendar.overlay.set`, and emptied when the mod is paused (`ledger/blacklist.rs`).
    let rows: unknown[] = [];
    const answers = {
      "calendar.overlay.set": (a: { entries: unknown[] }) => {
        rows = a.entries;
        return { set: a.entries.length };
      },
    };
    const fixture = (fixtures.cases as { board?: unknown; expected?: unknown[] }[]).find(
      (c) => c.board && c.expected && c.expected.length > 0,
    )!;
    const first = await open(fixture.board, NARROW, answers);
    expect(rows).toEqual(fixture.expected);
    // Paused: the frame ends, and PM drops what it was told. Nothing the mod can do about that.
    first.unmount();
    rows = [];
    connectPm(null);
    // Resumed: a new frame starts on the same stored board, and nobody has edited anything.
    const second = await open(fixture.board, NARROW, answers);
    expect(rows).toEqual(fixture.expected);
    second.unmount();
  });

  it("a failed overlay call is not remembered as told: the next saved change tells it again", async () => {
    let fail = true;
    const { result } = await open(board([dated, note("a")]), NARROW, {
      "calendar.overlay.set": (a: { entries: unknown[] }) => {
        if (fail) return refuse("MOD-9001", "PM could not.");
        overlays.push(a.entries);
        return { set: a.entries.length };
      },
    });
    expect(overlays).toHaveLength(0);
    fail = false;
    act(() => void result.current.updateWidget("a", { text: "x" }));
    await debounce();
    expect(overlays).toHaveLength(1);
  });

  it("equals the shared fixtures: for every fixture board, what the frame tells PM is what the Rust reader makes of the same bytes", async () => {
    type Case = { name: string; board?: unknown; expected?: unknown[] };
    const cases = (fixtures.cases as Case[]).filter(
      (c) => c.board !== undefined && c.expected !== undefined,
    );
    expect(cases.length).toBeGreaterThan(5);
    const expand = (value: unknown): unknown => {
      if (Array.isArray(value)) {
        return value.flatMap((v) => {
          if (v && typeof v === "object" && "$repeat" in v) {
            const { $repeat, item } = v as { $repeat: number; item: Record<string, unknown> };
            return Array.from({ length: $repeat }, (_, i) =>
              Object.fromEntries(
                Object.entries(item).map(([k, x]) => [
                  k,
                  typeof x === "string" ? x.replace(/\{n\}/g, String(i + 1)) : x,
                ]),
              ),
            );
          }
          return [expand(v)];
        });
      }
      if (value && typeof value === "object") {
        return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, expand(v)]));
      }
      return value;
    };
    for (const c of cases) {
      overlays.length = 0;
      const { unmount } = await open(expand(c.board));
      // A board this build cannot read is not told to the Calendar at all (the fixtures' empty answer
      // for a version it does not know is the Rust reader's; the frame leaves the dates as PM has them).
      const told = overlays.length > 0 ? overlays[overlays.length - 1] : [];
      expect(told, c.name).toEqual(c.expected);
      unmount();
      connectPm(null);
    }
  });
});

describe("the flush on suspend", () => {
  it("writes the edit the debounce has not carried yet, and waits for the write", async () => {
    const { result } = await open(board([note("a")]));
    act(() => void result.current.updateWidget("a", { text: "just typed" }));
    expect(sets()).toHaveLength(0);
    await act(async () => {
      const done = flushBoard();
      await mem.settle();
      await done;
    });
    expect(sets()).toHaveLength(1);
    expect(stored().widgets[0].text).toBe("just typed");
  });

  it("with nothing unwritten, writes nothing; with no board on screen, resolves at once", async () => {
    const { unmount } = await open(board([note("a")]));
    await act(async () => {
      const done = flushBoard();
      await mem.settle();
      await done;
    });
    expect(sets()).toHaveLength(0);
    unmount();
    await expect(flushBoard()).resolves.toBeUndefined();
  });

  it("an edit while a write is in flight is not marked as written", async () => {
    let release: (v: unknown) => void = () => {};
    const gate = new Promise((resolve) => (release = resolve));
    const { result } = await open(board([note("a")]), NARROW, {
      "storage.set": async (a: { value: unknown }) => {
        await gate;
        mem.storage.set("board", { value: a.value, revision: 2 });
        return { revision: 2 };
      },
    });
    act(() => void result.current.updateWidget("a", { text: "one" }));
    act(() => void vi.advanceTimersByTime(600));
    await settle();
    act(() => void result.current.updateWidget("a", { text: "two" }));
    release(null);
    await settle();
    await debounce();
    await settle();
    expect(stored().widgets[0].text).toBe("two");
  });
});

describe("what PM refuses stays PM's sentence", () => {
  it("a save PM refuses leaves the edit on screen and says the save failed, never losing it", async () => {
    const { result } = await open(board([note("a")]), NARROW, {
      "storage.set": () => refuse("MOD-9001", "PM could not keep that."),
    });
    act(() => void result.current.updateWidget("a", { text: "kept in memory" }));
    await debounce();
    expect(result.current.saveFailed).toBe(true);
    expect((result.current.board.widgets[0] as { text: string }).text).toBe("kept in memory");
    expect(new PmError("x").toString()).toBe("x");
  });
});
