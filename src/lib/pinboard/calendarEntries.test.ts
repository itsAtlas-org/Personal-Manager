// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import fixtures from "../../../tests/pinboard-overlay-fixtures.json";
import { clampOverlay, pinboardEntries } from "./calendarEntries";
import { BOARD_VERSION, type Board, type Widget } from "./types";

// The calendar's pinboard overlay draws freeform timeline entries. The load-bearing rules are the
// exclusions: a project-bound timeline must NOT contribute (its entries are already real milestones
// drawn by the milestone overlay, so including them would double-draw), an opted-out widget must not,
// and a board read straight out of the settings table must never throw into the calendar. These lock
// those in.

const board = (widgets: Widget[]): string => JSON.stringify({ version: BOARD_VERSION, widgets });

const timeline = (over: Partial<Widget> = {}): Widget => ({
  id: "w1",
  kind: "timeline",
  rect: { x: 0, y: 0, w: 3, h: 3 },
  items: [
    { id: "i1", date: "2026-07-20", label: "pitch" },
    { id: "i2", date: "2026-07-22", label: "demo" },
  ],
  ...over,
});

describe("pinboardEntries — freeform timeline entries for the calendar overlay", () => {
  it("lifts every dated entry of a freeform timeline", () => {
    expect(pinboardEntries(board([timeline()]))).toEqual([
      { id: "w1:i1", date: "2026-07-20", label: "pitch" },
      { id: "w1:i2", date: "2026-07-22", label: "demo" },
    ]);
  });

  it("skips a timeline bound to a project (its entries are real milestones — no double-draw)", () => {
    expect(pinboardEntries(board([timeline({ project: "Atlas" })]))).toEqual([]);
  });

  it("skips a timeline opted out, but shows one where the flag is unset or true (default on)", () => {
    expect(pinboardEntries(board([timeline({ showOnCalendar: false })]))).toEqual([]);
    expect(pinboardEntries(board([timeline({ showOnCalendar: true })]))).toHaveLength(2);
    expect(pinboardEntries(board([timeline()]))).toHaveLength(2); // unset → shown
  });

  it("reaches timelines inside a folder (folders never nest, so depth-1)", () => {
    const folder: Widget = {
      id: "f1",
      kind: "folder",
      rect: { x: 0, y: 0, w: 3, h: 3 },
      children: [timeline({ id: "w2" })],
    };
    expect(pinboardEntries(board([folder])).map((e) => e.id)).toEqual(["w2:i1", "w2:i2"]);
  });

  it("ignores notes, dateless entries, and trims a label (falling back when blank)", () => {
    const note: Widget = { id: "n1", kind: "note", rect: { x: 0, y: 0, w: 3, h: 3 }, text: "hi" };
    const mixed = timeline({
      items: [
        { id: "i1", date: "", label: "dateless" },
        { id: "i2", date: "2026-07-20", label: "  spaced  " },
        { id: "i3", date: "2026-07-21", label: "   " },
      ],
    });
    expect(pinboardEntries(board([note, mixed]))).toEqual([
      { id: "w1:i2", date: "2026-07-20", label: "spaced" },
      // Blank falls back to the same word linkProject writes, so linking can't rename the row.
      { id: "w1:i3", date: "2026-07-21", label: "deadline" },
    ]);
  });

  it("draws nothing for a task list — tasks reach the calendar through their own overlay", () => {
    // The card holds only a filter; its tasks are backend rows, drawn by the Tasks overlay. Lifting
    // anything from the card here would double-draw them, so a task list contributes nothing, at
    // the top level or (should one ever be met there) inside a folder.
    const list: Widget = {
      id: "t1",
      kind: "tasklist",
      rect: { x: 0, y: 0, w: 9, h: 8 },
      project: "Atlas",
    };
    const folder: Widget = {
      id: "f1",
      kind: "folder",
      rect: { x: 0, y: 0, w: 3, h: 3 },
      children: [{ ...list, id: "t2" }],
    };
    expect(pinboardEntries(board([list, folder]))).toEqual([]);
    expect(pinboardEntries(board([list, timeline()]))).toHaveLength(2);
  });

  it("yields nothing for absent, malformed, or wrong-version boards rather than throwing", () => {
    expect(pinboardEntries(null)).toEqual([]);
    expect(pinboardEntries("")).toEqual([]);
    expect(pinboardEntries("{not json")).toEqual([]);
    expect(
      pinboardEntries(JSON.stringify({ version: BOARD_VERSION + 1, widgets: [timeline()] })),
    ).toEqual([]);
    expect(pinboardEntries(JSON.stringify({ version: BOARD_VERSION } as Board))).toEqual([]);
  });

  it("skips malformed widgets/items without losing the good ones beside them", () => {
    const raw = JSON.stringify({
      version: BOARD_VERSION,
      widgets: [
        null,
        { id: "w9", kind: "timeline", rect: { x: 0, y: 0, w: 3, h: 3 }, items: [null, 7] },
        timeline({ id: "ok" }),
      ],
    });
    expect(pinboardEntries(raw).map((e) => e.id)).toEqual(["ok:i1", "ok:i2"]);
  });
});

// The shared fixtures (`tests/pinboard-overlay-fixtures.json`) are the contract between this
// TypeScript original and the Rust port (`src-tauri/src/pinboard_legacy/tests.rs`): each case holds a
// board and the entries the overlay must come to, and both implementations must agree on every one.
type Entry = { id: string; date: string; label: string };
interface FixtureCase {
  name: string;
  board?: unknown;
  raws?: string[];
  expected?: Entry[];
  expectedCount?: number;
  expectedFirst?: Entry;
  expectedLast?: Entry;
}

/** `{"$repeat": N, "item": {...}}` stands for N items, "{n}" in their strings read 1..N. */
function expand(value: unknown): unknown {
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
}

describe("the shared overlay fixtures", () => {
  for (const c of fixtures.cases as FixtureCase[]) {
    it(c.name, () => {
      const raws = c.raws ?? [JSON.stringify(expand(c.board))];
      for (const raw of raws) {
        const got = pinboardEntries(raw);
        if (c.expected) expect(got).toEqual(c.expected);
        if (c.expectedCount !== undefined) expect(got).toHaveLength(c.expectedCount);
        if (c.expectedFirst) expect(got[0]).toEqual(c.expectedFirst);
        if (c.expectedLast) expect(got[got.length - 1]).toEqual(c.expectedLast);
      }
    });
  }

  it("holds every case the plan names", () => {
    const names = (fixtures.cases as FixtureCase[]).map((c) => c.name).join("\n");
    for (const word of [
      "bound to a project",
      "showOnCalendar",
      "blank label",
      "folders",
      "unicode",
      "2,001-entry",
      "bad date",
      "datetime",
      "141-character",
      "repeated id",
      "undated",
    ]) {
      expect(names.toLowerCase()).toContain(word.toLowerCase());
    }
  });
});

describe("clampOverlay — the rule the reader and the move's seeding share", () => {
  const e = (id: string, date: string, label: string): Entry => ({ id, date, label });

  it("never throws on odd entries, and clamps what the table cannot hold", () => {
    expect(
      clampOverlay([
        e("a", "2026-1-5", "x"),
        e("b", "2026-12-01T10:00", "  y  "),
        e("c", "2026-12-02", ""),
        e("b", "2026-12-03", "again"),
      ]),
    ).toEqual([e("b", "2026-12-01", "y"), e("c", "2026-12-02", "deadline")]);
  });
});
