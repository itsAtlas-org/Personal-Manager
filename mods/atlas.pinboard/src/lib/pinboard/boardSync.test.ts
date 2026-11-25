// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The pure half of how the Pinboard saves (boardSync.ts): what a stored board is, and how the
// person's changes are applied to it without touching anything they did not change. The rules that
// protect a live board are tested here as properties of the two functions; the hook that uses them is
// held by usePinboard.save.test.tsx.

import { describe, expect, it } from "vitest";
import {
  applyBoard,
  applyList,
  diffBoards,
  diffList,
  freshStored,
  fullness,
  MAX_ARGS_BYTES,
  readStored,
  sameJson,
  saveBytes,
} from "./boardSync";
import limitsRs from "../../../../../src-tauri/src/mods/api/limits.rs?raw";

const note = (id: string, extra: object = {}) => ({
  id,
  kind: "note",
  rect: { x: 0, y: 0, w: 7, h: 5 },
  text: id,
  ...extra,
});

describe("readStored", () => {
  it("nothing stored is an empty board, a real answer", () => {
    expect(readStored(null)).toEqual({ kind: "empty" });
    expect(readStored("")).toEqual({ kind: "empty" });
  });

  it("reads a board of this version, whatever else it holds", () => {
    const read = readStored(JSON.stringify({ version: 1, widgets: [note("a")], future: { x: 1 } }));
    expect(read.kind).toBe("ok");
    if (read.kind === "ok") expect(read.board.future).toEqual({ x: 1 });
  });

  it("refuses a version it does not know, in either direction, and says which refusal it is", () => {
    for (const version of [0, 2, 99, "1", null]) {
      expect(readStored(JSON.stringify({ version, widgets: [] }))).toEqual({
        kind: "unreadable",
        why: "version",
      });
    }
    expect(readStored(JSON.stringify({ widgets: [] }))).toEqual({
      kind: "unreadable",
      why: "version",
    });
  });

  it("refuses what is not a board at all, without throwing", () => {
    for (const raw of [
      "not json",
      "[]",
      "3",
      '"a string"',
      '{"version":1}',
      '{"version":1,"widgets":{}}',
    ]) {
      expect(readStored(raw), raw).toEqual({ kind: "unreadable", why: "shape" });
    }
  });
});

describe("a save is the stored board with only the person's changes applied", () => {
  type Card = Record<string, unknown> & { children?: Record<string, unknown>[] };
  const stored: { version: number; widgets: Card[] } = {
    version: 1,
    widgets: [
      note("a", { x_future: { deep: [1, 2, 3] } }),
      { id: "z", kind: "hologram", rect: { x: 4, y: 4, w: 2, h: 2 }, payload: "kept" },
      note("b", { rect: { x: 30, y: 0, w: 7, h: 5 } }),
      {
        id: "f",
        kind: "folder",
        rect: { x: 10, y: 10, w: 3, h: 3 },
        children: [note("c"), { id: "k", kind: "hologram", rect: { x: 0, y: 0, w: 1, h: 1 } }],
      },
    ],
  };

  /** What the view draws: the readable cards, the first of them moved off the right edge. */
  const drawn = () => ({
    widgets: [
      { ...stored.widgets[0] },
      { ...stored.widgets[2], rect: { x: 13, y: 0, w: 7, h: 5 } },
      { ...stored.widgets[3], children: [stored.widgets[3].children![0]] },
    ],
  });

  it("an unchanged display is no change at all", () => {
    const base = drawn();
    expect(diffBoards(base, { widgets: [...base.widgets] })).toBeNull();
    expect(applyBoard(stored, null)).toBe(stored);
  });

  it("a note-only edit changes that note's text and nothing else, so no rect moves", () => {
    const base = drawn();
    const now = { widgets: [{ ...base.widgets[0], text: "edited" }, ...base.widgets.slice(1)] };
    const patch = diffBoards(base, now);
    const out = applyBoard(stored, patch);
    expect(out.widgets[0]).toEqual({ ...stored.widgets[0], text: "edited" });
    // Every other card is the very same stored object, byte for byte, including the one the view
    // reflowed (its stored rect is still x: 30) and the one this build cannot read.
    expect(out.widgets.slice(1)).toEqual(stored.widgets.slice(1));
    for (let i = 1; i < 4; i += 1) expect(out.widgets[i]).toBe(stored.widgets[i]);
    expect(JSON.stringify((out.widgets[0] as { rect: unknown }).rect)).toBe(
      JSON.stringify(stored.widgets[0].rect),
    );
  });

  it("moving one card writes that card's new rect, and no other card's", () => {
    const base = drawn();
    const moved = { x: 2, y: 9, w: 7, h: 5 };
    const now = {
      widgets: [base.widgets[0], { ...base.widgets[1], rect: moved }, base.widgets[2]],
    };
    const out = applyBoard(stored, diffBoards(base, now));
    expect((out.widgets[2] as { rect: unknown }).rect).toEqual(moved);
    expect(out.widgets[0]).toBe(stored.widgets[0]);
    expect(out.widgets[1]).toBe(stored.widgets[1]);
    expect(out.widgets[3]).toBe(stored.widgets[3]);
  });

  it("keeps cards and fields this build cannot read, and a folder's unreadable child", () => {
    const base = drawn();
    const now = {
      widgets: [
        base.widgets[0],
        base.widgets[1],
        {
          ...base.widgets[2],
          children: [{ ...stored.widgets[3].children![0], text: "in the folder" }],
        },
      ],
    };
    const out = applyBoard(stored, diffBoards(base, now));
    expect(out.widgets[1]).toEqual(stored.widgets[1]);
    const folder = out.widgets[3] as { children: unknown[] };
    expect(folder.children).toHaveLength(2);
    expect(folder.children[1]).toEqual(stored.widgets[3].children![1]);
    expect(folder.children[0]).toEqual({
      ...stored.widgets[3].children![0],
      text: "in the folder",
    });
    expect((out.widgets[0] as { x_future: unknown }).x_future).toEqual({ deep: [1, 2, 3] });
  });

  it("ungrouping a folder spills the drawn children and keeps the ones this build never drew", () => {
    const base = drawn();
    // The person ungroups "f": its drawn child "c" goes onto the board, the folder is gone.
    const spilled = { ...stored.widgets[3].children![0], rect: { x: 0, y: 20, w: 7, h: 5 } };
    const now = { widgets: [base.widgets[0], base.widgets[1], spilled] };
    const out = applyBoard(stored, diffBoards(base, now));
    const ids = out.widgets.map((w) => (w as { id: string }).id);
    expect(ids).not.toContain("f");
    expect(ids).toContain("c");
    // The unreadable child "k" is still in the board, byte for byte, and "c" is there exactly once.
    expect(out.widgets).toContainEqual(stored.widgets[3].children![1]);
    expect(ids.filter((id) => id === "c")).toHaveLength(1);
  });

  it("a child the person deleted before ungrouping stays deleted, and the unreadable one stays", () => {
    const base = drawn();
    const emptied = { ...base.widgets[2], children: [] as unknown[] };
    // First the drawn child is deleted from the folder (saved), then the folder is ungrouped.
    const afterDelete = applyBoard(
      stored,
      diffBoards(base, { widgets: [base.widgets[0], base.widgets[1], emptied] }),
    );
    const out = applyBoard(
      afterDelete,
      diffBoards(
        { widgets: [base.widgets[0], base.widgets[1], emptied] },
        { widgets: [base.widgets[0], base.widgets[1]] },
      ),
    );
    const ids = out.widgets.map((w) => (w as { id: string }).id);
    expect(ids).not.toContain("c");
    expect(out.widgets).toContainEqual(stored.widgets[3].children![1]);
  });

  it("removing a plain card with a list of its own removes the whole card", () => {
    const base = drawn();
    const out = applyBoard(
      stored,
      diffBoards(base, { widgets: [base.widgets[1], base.widgets[2]] }),
    );
    expect(out.widgets.map((w) => (w as { id: string }).id)).toEqual(["z", "b", "f"]);
  });

  it("adds a new card, removes a deleted one, and leaves the unreadable one between them", () => {
    const base = drawn();
    const fresh = note("new", { text: "mine" });
    const now = { widgets: [base.widgets[1], base.widgets[2], fresh] };
    const out = applyBoard(stored, diffBoards(base, now));
    expect(out.widgets.map((w) => (w as { id: string }).id)).toEqual(["z", "b", "f", "new"]);
    expect(out.widgets[0]).toBe(stored.widgets[1]);
  });

  it("a card raised to the top is a reorder: the known cards go in the new order and the unreadable one stays after the card it followed", () => {
    const base = drawn();
    const now = { widgets: [base.widgets[1], base.widgets[2], base.widgets[0]] };
    const out = applyBoard(stored, diffBoards(base, now));
    // "z" followed "a" in the stored order, so it travels with "a" to the top.
    expect(out.widgets.map((w) => (w as { id: string }).id)).toEqual(["b", "f", "a", "z"]);
    expect(out.widgets[3]).toBe(stored.widgets[1]);
  });

  it("an unreadable card keeps its place beside the known card before it when the order is unchanged", () => {
    const base = drawn();
    const now = { widgets: [base.widgets[0], { ...base.widgets[1], title: "t" }, base.widgets[2]] };
    const out = applyBoard(stored, diffBoards(base, now));
    expect(out.widgets.map((w) => (w as { id: string }).id)).toEqual(["a", "z", "b", "f"]);
  });

  it("removing a field is written as removing it", () => {
    const base = { widgets: [note("a", { color: "st-due" })] };
    const now = { widgets: [{ ...base.widgets[0], color: undefined }] };
    const out = applyList(
      [note("a", { color: "st-due", other: 1 })],
      diffList(base.widgets, now.widgets)!,
    );
    expect(out[0]).toEqual(note("a", { other: 1 }));
  });

  it("an entry of a timeline is diffed on its own: the others, and ones this build cannot read, stay", () => {
    const tl = (items: unknown[]) => ({
      id: "t",
      kind: "timeline",
      rect: { x: 0, y: 0, w: 9, h: 8 },
      items,
    });
    const storedItems = [
      { id: "i1", date: "2026-01-01", label: "one", ext: 1 },
      "junk",
      { id: "i2", date: "2026-01-02", label: "two" },
    ];
    const base = { widgets: [tl([storedItems[0], storedItems[2]])] };
    const now = {
      widgets: [tl([storedItems[0], { id: "i2", date: "2026-01-02", label: "TWO" }])],
    };
    const out = applyList([tl(storedItems)], diffList(base.widgets, now.widgets)!);
    expect((out[0] as { items: unknown[] }).items).toEqual([
      storedItems[0],
      "junk",
      { id: "i2", date: "2026-01-02", label: "TWO" },
    ]);
  });

  it("a card that two cards share an id with is addressed by its first, and the second is left alone", () => {
    const raw = [note("d", { text: "first" }), note("d", { text: "second" })];
    const base = { widgets: [raw[0]] };
    const now = { widgets: [{ ...raw[0], text: "changed" }] };
    const out = applyList(raw, diffList(base.widgets, now.widgets)!);
    expect(out).toEqual([note("d", { text: "changed" }), raw[1]]);
  });
});

describe("merging into a board someone else changed", () => {
  it("applies our difference to THEIR board: their additions stay, ours join them, and a card they deleted that we changed comes back", () => {
    const base = { widgets: [note("a"), note("b"), note("c")] };
    const now = {
      widgets: [
        { ...base.widgets[0], text: "ours" },
        base.widgets[1],
        base.widgets[2],
        note("mine"),
      ],
    };
    const patch = diffBoards(base, now)!;
    const theirs = [note("a", { project: "Renamed" }), note("theirs"), note("c")];
    const out = applyList(theirs, patch) as { id: string; text?: string; project?: string }[];
    expect(out.map((w) => w.id)).toEqual(["a", "theirs", "c", "mine"]);
    expect(out[0]).toMatchObject({ text: "ours", project: "Renamed" });
    // "b" was deleted by them and untouched by us: it stays deleted.
    expect(out.find((w) => w.id === "b")).toBeUndefined();
    const deleted = applyList([note("c")], patch) as { id: string }[];
    expect(deleted.map((w) => w.id)).toEqual(["c", "a", "mine"]);
  });
});

describe("sameJson", () => {
  it("compares by value, ignores undefined fields, and gives up on absurd depth", () => {
    expect(sameJson({ a: [1, { b: 2 }] }, { a: [1, { b: 2 }] })).toBe(true);
    expect(sameJson({ a: 1, b: undefined }, { a: 1 })).toBe(true);
    expect(sameJson({ a: 1 }, { a: 2 })).toBe(false);
    expect(sameJson([1], { 0: 1 })).toBe(false);
    let deep: unknown = 1;
    let other: unknown = 1;
    for (let i = 0; i < 200; i += 1) {
      deep = { d: deep };
      other = { d: other };
    }
    expect(sameJson(deep, other)).toBe(false);
  });
});

describe("the size of a save", () => {
  it("counts the bytes storage.set would send, in UTF-8, at the longest revision", () => {
    const board = JSON.stringify({ version: 1, widgets: [note("é", { text: "😀 ünïcode" })] });
    const sent = JSON.stringify({
      key: "board",
      value: JSON.parse(board),
      expectedRevision: Number.MAX_SAFE_INTEGER,
    });
    expect(saveBytes("board", board)).toBe(new TextEncoder().encode(sent).length);
  });

  it("the cap is PM's: the backend's number (modBridge.ts holds the page's to it, in its own test)", () => {
    expect(MAX_ARGS_BYTES).toBe(1_048_576);
    expect(limitsRs).toMatch(/MAX_ARGS_BYTES: usize = 1 << 20/);
  });

  it("says how full a board is, never zero for a board with anything and never over a hundred", () => {
    expect(fullness(0)).toBe(0);
    expect(fullness(13_329)).toBe(1);
    expect(fullness(MAX_ARGS_BYTES / 2)).toBe(50);
    expect(fullness(MAX_ARGS_BYTES * 3)).toBe(100);
  });

  it("a fresh board is a version-1 board with no cards", () => {
    expect(freshStored()).toEqual({ version: 1, widgets: [] });
  });
});
