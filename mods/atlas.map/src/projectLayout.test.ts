// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The by-project pipeline: remembered positions used for exactly the document set they were computed
// for, a cache that is wrong or unreadable treated as no cache, a failed save costing only the next
// open a recompute. Ported behaviours are the ones src/lib/mapLayout.test.ts pins on the old
// `getProjectLayout`; what is new is where the positions live (one cache-class storage key).

import { describe, expect, it, vi } from "vitest";
import { inlineCompute, type Compute } from "./compute";
import { CODE, type MapHost } from "./hostApi";
import { assemble, buildGraph, docSetKey, MAX_COHESION } from "./layoutModel";
import { memoryHost, type MemoryHost } from "./memoryHost";
import {
  KEY_PROJECT_LAYOUT,
  MAX_LAYOUT_BYTES,
  PROJECT_LAYOUT_VERSION,
  loadProjectLayout,
  readStored,
  toStored,
  type ProjectPhase,
} from "./projectLayout";
import { doc, documents } from "./testing";
import type { MapDoc } from "./types";

const host = (docs: MapDoc[] = []) => memoryHost({ docs });
const compute = inlineCompute();

describe("loadProjectLayout", () => {
  it("is null with no documents, and asks PM for nothing", async () => {
    const pm = host();
    expect(await loadProjectLayout(pm.host, compute, [])).toBeNull();
    expect(pm.calls).toEqual([]);
  });

  it("computes on a cold open and remembers: one read, one write, at the revision it read", async () => {
    const pm = host();
    const docs = documents(30, 3);
    const phases: ProjectPhase[] = [];
    const result = (await loadProjectLayout(pm.host, compute, docs, (p) => phases.push(p)))!;
    expect(result).toMatchObject({ source: "computed", saved: true });
    expect(phases).toEqual(["checking", "computing", "saving"]);
    expect(pm.calls.map((c) => c.method)).toEqual(["storage.get", "storage.set"]);
    const write = pm.of("storage.set")[0].args as { key: string; expectedRevision: number };
    expect(write.key).toBe(KEY_PROJECT_LAYOUT);
    expect(write.expectedRevision).toBe(0);
    expect(result.layout.nodes).toHaveLength(30 + 4);
  });

  it("a second open of the same documents reads the positions back and runs no simulation", async () => {
    const pm = host();
    const docs = documents(30, 3);
    const cold = (await loadProjectLayout(pm.host, compute, docs))!;
    pm.calls.length = 0;
    const spy: Compute = {
      ...compute,
      force: vi.fn(() => Promise.reject(new Error("must not run"))),
    };
    const phases: ProjectPhase[] = [];
    const warm = (await loadProjectLayout(pm.host, spy, docs, (p) => phases.push(p)))!;
    expect(warm.source).toBe("cache");
    expect(phases).toEqual(["checking"]);
    expect(spy.force).not.toHaveBeenCalled();
    expect(pm.calls.map((c) => c.method)).toEqual(["storage.get"]);
    // A remembered layout is pixel-identical to the computed one, edges and fit box included.
    expect(warm.layout).toEqual(cold.layout);
  });

  it("positions are rounded to two decimals before the layout is built, so fresh equals remembered", async () => {
    const pm = host();
    const docs = documents(20, 2);
    const { layout } = (await loadProjectLayout(pm.host, compute, docs))!;
    for (const n of layout.nodes) {
      expect(Math.round(n.x * 100) / 100).toBe(n.x);
      expect(Math.round(n.y * 100) / 100).toBe(n.y);
    }
  });

  it("stores compact rows: [id, x, y] per document and [name, x, y] per hub, under the version and key", async () => {
    const pm = host();
    const docs = [doc(1, "A"), doc(2, "B"), doc(3, "")];
    await loadProjectLayout(pm.host, compute, docs);
    const stored = pm.kv.get(KEY_PROJECT_LAYOUT)!.value as {
      v: number;
      key: string;
      docs: unknown[][];
      hubs: unknown[][];
    };
    expect(stored.v).toBe(PROJECT_LAYOUT_VERSION);
    expect(stored.key).toBe(docSetKey(docs));
    expect(stored.docs.map((r) => r[0])).toEqual([1, 2, 3]);
    expect(stored.hubs.map((r) => r[0])).toEqual(["A", "B", "Unsorted"]);
    for (const row of [...stored.docs, ...stored.hubs]) expect(row).toHaveLength(3);
  });

  describe("what the remembered positions are good for", () => {
    const docs = [doc(1, "A", 3), doc(2, "B", 7), doc(3, "A", 2)];
    const remembered = async () => {
      const pm = host();
      await loadProjectLayout(pm.host, compute, docs);
      return pm;
    };
    const again = async (pm: MemoryHost, next: MapDoc[]) =>
      (await loadProjectLayout(pm.host, compute, next))!.source;

    it("a title change, or the order PM lists them in, is the same layout", async () => {
      const pm = await remembered();
      expect(
        await again(
          pm,
          docs.map((d) => ({ ...d, title: "renamed" })),
        ),
      ).toBe("cache");
      expect(await again(pm, [...docs].reverse())).toBe("cache");
    });

    it("a project change, a chunk-count change, a document added or removed is a new layout", async () => {
      expect(
        await again(await remembered(), [{ ...docs[0], project: "C" }, docs[1], docs[2]]),
      ).toBe("computed");
      expect(
        await again(await remembered(), [{ ...docs[0], chunkCount: 9 }, docs[1], docs[2]]),
      ).toBe("computed");
      expect(await again(await remembered(), [...docs, doc(4, "A")])).toBe("computed");
      expect(await again(await remembered(), docs.slice(1))).toBe("computed");
    });

    it("and is then remembered in its place, so the next open is a hit", async () => {
      const pm = await remembered();
      const next = [...docs, doc(4, "A")];
      expect(await again(pm, next)).toBe("computed");
      expect(await again(pm, next)).toBe("cache");
    });
  });

  describe("a stored value that is not a layout for this graph is no cache", () => {
    const docs = [doc(1, "A", 3), doc(2, "B", 7)];
    const key = docSetKey(docs);
    const good = () => ({
      v: PROJECT_LAYOUT_VERSION,
      key,
      docs: [
        [1, 10, 20],
        [2, 30, 40],
      ],
      hubs: [
        ["A", 1, 2],
        ["B", 3, 4],
      ],
    });
    const cases: [string, unknown][] = [
      ["null", null],
      ["a string", "layout"],
      ["another version", { ...good(), v: PROJECT_LAYOUT_VERSION + 1 }],
      ["another key", { ...good(), key: "2|0000000000000000" }],
      ["docs that is not a list", { ...good(), docs: "no" }],
      ["hubs that is not a list", { ...good(), hubs: {} }],
      [
        "a row with two numbers",
        {
          ...good(),
          docs: [
            [1, 10],
            [2, 30, 40],
          ],
        },
      ],
      [
        "a row with a string x",
        {
          ...good(),
          docs: [
            [1, "10", 20],
            [2, 30, 40],
          ],
        },
      ],
      [
        "a row with NaN y",
        {
          ...good(),
          docs: [
            [1, 10, NaN],
            [2, 30, 40],
          ],
        },
      ],
      [
        "a document id that is not a count",
        {
          ...good(),
          docs: [
            [-1, 10, 20],
            [2, 30, 40],
          ],
        },
      ],
      [
        "a hub name that is not text",
        {
          ...good(),
          hubs: [
            [1, 1, 2],
            ["B", 3, 4],
          ],
        },
      ],
      ["a document missing", { ...good(), docs: [[1, 10, 20]] }],
      ["a hub missing", { ...good(), hubs: [["A", 1, 2]] }],
      [
        "a repeated document",
        {
          ...good(),
          docs: [
            [1, 10, 20],
            [1, 30, 40],
            [2, 5, 5],
          ],
        },
      ],
    ];

    it.each(cases)("%s", async (_name, value) => {
      const pm = host();
      pm.kv.set(KEY_PROJECT_LAYOUT, { value, revision: 4 });
      const result = (await loadProjectLayout(pm.host, compute, docs))!;
      expect(result.source).toBe("computed");
      // Written over the bad value, at the revision it was read at.
      expect(pm.of("storage.set")[0].args).toMatchObject({ expectedRevision: 4 });
      expect((await loadProjectLayout(pm.host, compute, docs))!.source).toBe("cache");
    });

    it("a good value is used as it is, hubs and documents", async () => {
      const pm = host();
      pm.kv.set(KEY_PROJECT_LAYOUT, { value: good(), revision: 1 });
      const result = (await loadProjectLayout(pm.host, compute, docs))!;
      expect(result.source).toBe("cache");
      expect(result.layout.nodes.map((n) => [n.id, n.x, n.y])).toEqual([
        ["project:A", 1, 2],
        ["project:B", 3, 4],
        ["doc:1", 10, 20],
        ["doc:2", 30, 40],
      ]);
    });

    it("readStored and toStored are inverses over a graph's nodes", () => {
      const graph = buildGraph(docs);
      const positions = Array.from(graph.meta.keys()).map((id, i) => ({ id, x: i + 0.5, y: -i }));
      const back = readStored(toStored(key, graph, positions), key, graph)!;
      expect(new Map(back.map((p) => [p.id, p]))).toEqual(new Map(positions.map((p) => [p.id, p])));
      expect(assemble(graph, back)).toEqual(assemble(graph, positions));
    });
  });

  describe("the store misbehaving", () => {
    it("a cache that cannot be read does not stop the layout", async () => {
      const pm = host();
      pm.failAlways("storage.get", "MOD-9001");
      const result = (await loadProjectLayout(pm.host, compute, documents(10, 2)))!;
      expect(result.source).toBe("computed");
      expect(result.layout.nodes).toHaveLength(10 + 2);
      // With no revision read it writes at 0, right for an empty key; were the key not empty, PM would
      // refuse it as stale and the save would fail quietly (the test below).
      expect(result.saved).toBe(true);
    });

    it("a save that fails is silent: the person still has the layout", async () => {
      const pm = host();
      pm.failAlways("storage.set", CODE.quotaExceeded);
      const result = (await loadProjectLayout(pm.host, compute, documents(10, 2)))!;
      expect(result).toMatchObject({ source: "computed", saved: false });
      expect(result.layout.nodes.length).toBeGreaterThan(0);
    });

    it("another frame writing first (a stale revision) is the same", async () => {
      const pm = host();
      pm.failNext("storage.set", CODE.staleRevision);
      expect((await loadProjectLayout(pm.host, compute, documents(10, 2)))!.saved).toBe(false);
    });

    it("a layout too big for one storage value is shown but not remembered", async () => {
      const docs = Array.from({ length: 45_000 }, (_, i) => doc(i + 1, "A"));
      const pm = host();
      // A grid stands in for the simulation: 45,000 nodes would take the real one a long while.
      const grid: Compute = {
        mode: "inline",
        reduce: () => Promise.reject(new Error("unused")),
        force: async (req) => ({
          positions: req.nodes.map((n, i) => ({
            id: n.id,
            x: (i % 300) * 7.77,
            y: Math.floor(i / 300) * 7.77,
          })),
        }),
        close: () => undefined,
      };
      const result = (await loadProjectLayout(pm.host, grid, docs))!;
      expect(result).toMatchObject({ source: "computed", saved: false });
      expect(result.layout.nodes).toHaveLength(45_001);
      expect(pm.of("storage.set")).toHaveLength(0);
      expect(MAX_LAYOUT_BYTES).toBeLessThan(1 << 20);
    });

    it("a layout under the limit is remembered, within PM's argument cap", async () => {
      const docs = Array.from({ length: 5000 }, (_, i) => doc(i + 1, i % 2 ? "A" : "B"));
      const pm = host();
      const grid: Compute = {
        mode: "inline",
        reduce: () => Promise.reject(new Error("unused")),
        force: async (req) => ({
          positions: req.nodes.map((n, i) => ({
            id: n.id,
            x: (i % 70) * 7.77,
            y: Math.floor(i / 70) * 7.77,
          })),
        }),
        close: () => undefined,
      };
      const result = (await loadProjectLayout(pm.host, grid, docs))!;
      expect(result.saved).toBe(true);
      expect(JSON.stringify(pm.of("storage.set")[0].args).length).toBeLessThan(MAX_LAYOUT_BYTES);
    });

    it("a simulation that fails is the caller's to report, and nothing is written", async () => {
      const pm = host();
      const failing: Compute = {
        ...compute,
        force: () => Promise.reject(new Error("The layout worker failed.")),
      };
      await expect(loadProjectLayout(pm.host, failing, documents(5, 2))).rejects.toThrow(
        /layout worker failed/,
      );
      expect(pm.of("storage.set")).toHaveLength(0);
    });
  });

  it("a document with no home project is drawn under Unsorted, with its own hub", async () => {
    const pm = host();
    const { layout } = (await loadProjectLayout(pm.host, compute, [doc(1, ""), doc(2, "A")]))!;
    expect(layout.projectNames).toEqual(["Unsorted", "A"]);
    expect(layout.nodes.find((n) => n.id === "doc:1")!.project).toBe("Unsorted");
  });

  it("works through any host with just the three calls it uses", async () => {
    const pm = host();
    const narrow: MapHost = {
      call: (m, a) =>
        ["storage.get", "storage.set"].includes(m)
          ? pm.host.call(m, a)
          : Promise.reject(new Error(m)),
      documentVectors: pm.host.documentVectors,
      onEvent: pm.host.onEvent,
    };
    expect((await loadProjectLayout(narrow, compute, documents(5, 2)))!.saved).toBe(true);
    expect(MAX_COHESION).toBe(0.5);
  });
});
