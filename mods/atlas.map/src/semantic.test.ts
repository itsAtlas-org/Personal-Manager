// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The by-meaning pipeline against a PM that keeps its limits (memoryHost.ts): the fingerprint hit and
// miss paths, a cache that is never half true however a write is interrupted, batching under the
// argument cap and the growth bucket, the node cap, and a run through the real SDK and its fake PM.

import { describe, expect, it, vi } from "vitest";
import { fakePm } from "../../../src/kit/fakePm";
import { connectWith, ModCallError } from "../../../src/kit/host";
import { inlineCompute, type Compute } from "./compute";
import { CODE, type MapHost } from "./hostApi";
import { memoryHost, type MemoryHost } from "./memoryHost";
import { reduceToUnit } from "./pca";
import {
  ALGO_VERSION,
  BATCH_BYTES,
  BATCH_ENTRIES,
  KEY_SEMANTIC_COORDS,
  KEY_SEMANTIC_META,
  batches,
  isFresh,
  loadSemanticCoords,
  readMeta,
  type SemanticPhase,
} from "./semantic";
import { doc, documents, vectorsFor } from "./testing";

const DIM = 8;
const noSleep = () => Promise.resolve();

function world(n = 40, dimension = DIM) {
  const docs = documents(n, 3);
  const vectors = vectorsFor(docs, dimension);
  const pm = memoryHost({ docs, vectors, dimension, contentKeys: ["settings"] });
  return { docs, vectors, pm };
}

const load = (
  host: MapHost,
  cap = 1000,
  extra: Partial<Parameters<typeof loadSemanticCoords>[2]> = {},
) => loadSemanticCoords(host, inlineCompute(), { nodeCap: cap, sleep: noSleep, ...extra });

const writes = (pm: MemoryHost) =>
  pm.calls.filter((c) =>
    ["storage.set", "storage.doc.delete", "storage.doc.set"].includes(c.method),
  );

const methods = (pm: MemoryHost) => pm.calls.map((c) => c.method);

describe("a cold open", () => {
  it("reads the signature and the cache, reads the vectors, projects, then writes: fingerprint cleared, rows replaced, fingerprint last", async () => {
    const { docs, vectors, pm } = world(40);
    const phases: SemanticPhase[] = [];
    const result = await load(pm.host, 1000, { onPhase: (p) => phases.push(p) });

    expect(result).toMatchObject({ source: "computed", total: 40, placed: 40, saved: true });
    expect(phases).toEqual(["checking", "reading", "reducing", "saving"]);
    expect(methods(pm)).toEqual([
      "embeddings.signature",
      "storage.get",
      "embeddings.documentVectors",
      "storage.set",
      "storage.doc.delete",
      "storage.doc.set",
      "storage.set",
    ]);
    expect(pm.of("embeddings.documentVectors")[0].args).toEqual({ cap: 1000 });
    const sets = pm
      .of("storage.set")
      .map((c) => c.args as { key: string; value: unknown; expectedRevision: number });
    expect(sets[0]).toEqual({ key: KEY_SEMANTIC_META, value: null, expectedRevision: 0 });
    expect(sets[1].key).toBe(KEY_SEMANTIC_META);
    expect(sets[1].expectedRevision).toBe(1);
    expect(sets[1].value).toEqual({
      v: 1,
      hash: expect.stringMatching(/^[0-9a-f]{64}$/),
      dimension: DIM,
      cap: 1000,
      algo: ALGO_VERSION,
      nodes: 40,
    });

    // The coordinates are exactly the projection of the vectors, by document.
    const flatVectors = new Float32Array(40 * DIM);
    docs.forEach((d, i) => flatVectors.set(vectors.get(d.id) as Float32Array, i * DIM));
    const direct = reduceToUnit(flatVectors, 40, DIM).coords;
    expect(result.coords.map((c) => c.id)).toEqual(docs.map((d) => d.id));
    result.coords.forEach((c, i) => expect([c.x, c.y]).toEqual([direct[2 * i], direct[2 * i + 1]]));
  });

  it("reads the vectors from PM once and gives the compute one job, whatever the run does with them", async () => {
    const { pm } = world(40);
    const real = inlineCompute();
    const jobs: { pca: boolean; tsne: boolean; vectors: Float32Array }[] = [];
    const counting: Compute = {
      ...real,
      semantic: (vectors, count, dimension, options) => {
        jobs.push({ pca: options.pca, tsne: options.tsne, vectors });
        return real.semantic(vectors, count, dimension, options);
      },
    };
    await loadSemanticCoords(pm.host, counting, { nodeCap: 1000, sleep: noSleep });
    expect(pm.of("embeddings.documentVectors")).toHaveLength(1);
    expect(jobs).toHaveLength(1);
    // Without `enhanced` the run is the quick layout alone: one job, PCA only (semanticEnhanced.test.ts
    // has the runs that ask for t-SNE).
    expect(jobs[0]).toMatchObject({ pca: true, tsne: false });
  });

  it("stores each document's coordinate as {x, y} under the cache key", async () => {
    const { pm } = world(10);
    const result = await load(pm.host);
    const rows = pm.rows.get(KEY_SEMANTIC_COORDS)!;
    expect(rows.size).toBe(10);
    for (const c of result.coords) expect(rows.get(c.id)).toEqual({ x: c.x, y: c.y });
  });

  it("is the same answer on every run: two separate worlds, same coordinates", async () => {
    const a = await load(world(60).pm.host);
    const b = await load(world(60).pm.host);
    expect(b.coords).toEqual(a.coords);
  });
});

describe("a warm open", () => {
  it("reads the coordinates back and reads no vector, runs no projection", async () => {
    const { pm } = world(40);
    const cold = await load(pm.host);
    pm.calls.length = 0;
    const phases: SemanticPhase[] = [];
    const warm = await load(pm.host, 1000, { onPhase: (p) => phases.push(p) });
    expect(warm.source).toBe("cache");
    expect(phases).toEqual(["checking"]);
    expect(methods(pm)).toEqual(["embeddings.signature", "storage.get", "storage.doc.list"]);
    // A remembered layout is identical to the one just computed.
    expect(warm.coords).toEqual(cold.coords);
    expect(warm).toMatchObject({ total: 40, placed: 40, saved: true });
  });

  it("does not use a projection function at all (the compute is never asked)", async () => {
    const { pm } = world(20);
    await load(pm.host);
    const never: Compute = {
      mode: "inline",
      semantic: vi.fn(() => {
        throw new Error("must not run");
      }),
      force: vi.fn(() => Promise.reject(new Error("must not run"))),
      close: () => undefined,
    };
    const warm = await loadSemanticCoords(pm.host, never, { nodeCap: 1000, sleep: noSleep });
    expect(warm.source).toBe("cache");
    expect(never.semantic).not.toHaveBeenCalled();
  });
});

describe("what makes the cache stale", () => {
  const stale = async (change: (pm: MemoryHost) => void) => {
    const { pm } = world(30);
    await load(pm.host);
    change(pm);
    pm.calls.length = 0;
    const next = await load(pm.host);
    expect(next.source).toBe("computed");
    expect(methods(pm)).toContain("embeddings.documentVectors");
    // It heals: the open after is a hit.
    expect((await load(pm.host)).source).toBe("cache");
    return next;
  };

  it("a document with a vector arrives", async () => {
    const { vectors, docs } = world(30);
    const next = await stale((pm) => {
      const d = { ...docs[0], id: 999 };
      pm.addDoc(d, vectors.get(1) as Float32Array);
    });
    expect(next.placed).toBe(31);
  });

  it("a document goes (its coordinate row went with it)", async () => {
    const next = await stale((pm) => pm.removeDoc(5));
    expect(next.placed).toBe(29);
    expect(next.coords.some((c) => c.id === 5)).toBe(false);
  });

  it("a re-index changes which chunks the vectors are of, though not how many", async () => {
    await stale((pm) => pm.reindex());
  });

  it("a different node cap", async () => {
    const { pm } = world(30);
    await load(pm.host, 1000);
    pm.calls.length = 0;
    expect((await load(pm.host, 200)).source).toBe("computed");
    expect(pm.of("embeddings.documentVectors")[0].args).toEqual({ cap: 200 });
    expect((await load(pm.host, 200)).source).toBe("cache");
    // And back: the cap is part of the fingerprint, so 1000 is stale again, not served from 200's rows.
    expect((await load(pm.host, 1000)).source).toBe("computed");
  });

  it("a newer algorithm version", async () => {
    await stale((pm) => {
      const meta = pm.kv.get(KEY_SEMANTIC_META)!;
      pm.kv.set(KEY_SEMANTIC_META, {
        value: { ...(meta.value as object), algo: ALGO_VERSION - 1 },
        revision: meta.revision,
      });
    });
  });

  it("a different vector width", async () => {
    await stale((pm) => {
      const meta = pm.kv.get(KEY_SEMANTIC_META)!;
      pm.kv.set(KEY_SEMANTIC_META, {
        value: { ...(meta.value as object), dimension: DIM + 1 },
        revision: meta.revision,
      });
    });
  });
});

describe("the previous layout, handed over as a placeholder while a new one is worked out", () => {
  /** Load with a listener; what it was given and what PM had been asked when it was. */
  const watched = async (pm: MemoryHost) => {
    const seen: { coords: { id: number }[]; vectorsRead: boolean }[] = [];
    const result = await load(pm.host, 1000, {
      onStale: (coords) =>
        seen.push({ coords, vectorsRead: methods(pm).includes("embeddings.documentVectors") }),
    });
    return { seen, result };
  };

  it("a changed document set hands over the old coordinates before the vectors are read", async () => {
    const { pm } = world(30);
    const first = await load(pm.host);
    pm.addDoc(doc(99, "A"), new Float32Array(DIM).fill(0.3));
    pm.calls.length = 0;
    const { seen, result } = await watched(pm);
    expect(seen).toHaveLength(1);
    expect(seen[0].vectorsRead).toBe(false);
    expect(seen[0].coords).toEqual(first.coords);
    // And the answer is still the new, full layout.
    expect(result).toMatchObject({ source: "computed", placed: 31 });
  });

  it("a changed cap does the same: the rows are the old cap's, and still a picture", async () => {
    const { pm } = world(30);
    const first = await load(pm.host, 1000);
    const seen: unknown[] = [];
    await load(pm.host, 200, { onStale: (c) => seen.push(c) });
    expect(seen).toEqual([first.coords]);
  });

  it("a layout that is still right is not a placeholder", async () => {
    const { pm } = world(30);
    await load(pm.host);
    const { seen, result } = await watched(pm);
    expect(result.source).toBe("cache");
    expect(seen).toEqual([]);
  });

  it("a first open has nothing to hand over", async () => {
    const { pm } = world(30);
    const { seen } = await watched(pm);
    expect(seen).toEqual([]);
  });

  const unusable: [string, (pm: MemoryHost) => void][] = [
    [
      "coordinates made by another algorithm",
      (pm) => {
        const m = pm.kv.get(KEY_SEMANTIC_META)!;
        pm.kv.set(KEY_SEMANTIC_META, {
          value: { ...(m.value as object), algo: ALGO_VERSION + 1 },
          revision: m.revision,
        });
      },
    ],
    [
      "a fingerprint that was cleared (a write that never finished)",
      (pm) => {
        const m = pm.kv.get(KEY_SEMANTIC_META)!;
        pm.kv.set(KEY_SEMANTIC_META, { value: null, revision: m.revision });
      },
    ],
    ["a row missing", (pm) => pm.rows.get(KEY_SEMANTIC_COORDS)!.delete(3)],
    ["a row that is not a coordinate", (pm) => pm.rows.get(KEY_SEMANTIC_COORDS)!.set(3, null)],
  ];

  it.each(unusable)("%s is not a placeholder, and the recompute still runs", async (_n, damage) => {
    const { pm } = world(30);
    await load(pm.host);
    pm.addDoc(doc(99, "A"), new Float32Array(DIM).fill(0.3));
    damage(pm);
    const { seen, result } = await watched(pm);
    expect(seen).toEqual([]);
    expect(result.source).toBe("computed");
  });

  it("a failure reading the old rows costs the placeholder and nothing else", async () => {
    const { pm } = world(30);
    await load(pm.host);
    pm.addDoc(doc(99, "A"), new Float32Array(DIM).fill(0.3));
    pm.failNext("storage.doc.list", "MOD-2001");
    const { seen, result } = await watched(pm);
    expect(seen).toEqual([]);
    expect(result).toMatchObject({ source: "computed", placed: 31 });
  });
});

describe("a cache that does not hold together is stale, never trusted", () => {
  const broken: [string, (pm: MemoryHost) => void][] = [
    ["a missing row", (pm) => pm.rows.get(KEY_SEMANTIC_COORDS)!.delete(3)],
    ["a surplus row", (pm) => pm.rows.get(KEY_SEMANTIC_COORDS)!.set(31, { x: 0.5, y: 0.5 })],
    ["a coordinate past 1", (pm) => pm.rows.get(KEY_SEMANTIC_COORDS)!.set(3, { x: 1.5, y: 0.5 })],
    [
      "a negative coordinate",
      (pm) => pm.rows.get(KEY_SEMANTIC_COORDS)!.set(3, { x: -0.1, y: 0.5 }),
    ],
    [
      "a coordinate that is not a number",
      (pm) => pm.rows.get(KEY_SEMANTIC_COORDS)!.set(3, { x: "0.5", y: 0.5 }),
    ],
    [
      "a coordinate that is NaN",
      (pm) => pm.rows.get(KEY_SEMANTIC_COORDS)!.set(3, { x: NaN, y: 0.5 }),
    ],
    ["a row that is not an object", (pm) => pm.rows.get(KEY_SEMANTIC_COORDS)!.set(3, 5)],
    ["a null row", (pm) => pm.rows.get(KEY_SEMANTIC_COORDS)!.set(3, null)],
    [
      "a meta that is a string",
      (pm) => pm.kv.set(KEY_SEMANTIC_META, { value: "fresh", revision: 3 }),
    ],
    [
      "a meta of another version",
      (pm) => {
        const m = pm.kv.get(KEY_SEMANTIC_META)!;
        pm.kv.set(KEY_SEMANTIC_META, {
          value: { ...(m.value as object), v: 2 },
          revision: m.revision,
        });
      },
    ],
    [
      "a meta with no node count",
      (pm) => {
        const m = pm.kv.get(KEY_SEMANTIC_META)!;
        const rest: Record<string, unknown> = { ...(m.value as Record<string, unknown>) };
        delete rest.nodes;
        pm.kv.set(KEY_SEMANTIC_META, { value: rest, revision: m.revision });
      },
    ],
    ["no meta at all", (pm) => pm.kv.delete(KEY_SEMANTIC_META)],
    [
      "a meta that was cleared and never written back",
      (pm) => {
        const m = pm.kv.get(KEY_SEMANTIC_META)!;
        pm.kv.set(KEY_SEMANTIC_META, { value: null, revision: m.revision });
      },
    ],
  ];

  it.each(broken)("%s", async (_name, damage) => {
    const { pm } = world(30);
    // A thirty-first document that exists but has no vector: a row for it is a surplus, and PM would
    // still list it (it is readable), unlike a row for a document that is not there at all.
    pm.addDoc(doc(31, "A"));
    const cold = await load(pm.host);
    damage(pm);
    const again = await load(pm.host);
    expect(again.source).toBe("computed");
    // The recompute replaced everything: same coordinates as the first time, and the next open is a hit.
    expect(again.coords).toEqual(cold.coords);
    expect(pm.rows.get(KEY_SEMANTIC_COORDS)!.size).toBe(30);
    expect((await load(pm.host)).source).toBe("cache");
  });
});

describe("a write that is interrupted anywhere leaves a cache that is stale, not wrong", () => {
  // 2,500 documents is three batches: the writes are meta cleared, rows deleted, three batches, meta.
  const N = 2500;
  const WRITE_METHODS = ["storage.set", "storage.doc.delete", "storage.doc.set"];

  it.each([1, 2, 3, 4, 5, 6])("the write number %i fails", async (k) => {
    const { pm } = world(N);
    let seen = 0;
    const wrapped: MapHost = {
      ...pm.host,
      call: (method, args) => {
        if (WRITE_METHODS.includes(method) && ++seen === k) {
          return Promise.reject(
            new ModCallError({ code: "MOD-9001", message: "Something went wrong." }),
          );
        }
        return pm.host.call(method, args);
      },
    };
    const first = await load(wrapped, 5000);
    // The person still gets their map; it just was not remembered.
    expect(first).toMatchObject({ source: "computed", placed: N, saved: false });
    expect(seen).toBe(k);
    // Whatever half-written state is in storage is not a layout: the next open recomputes ...
    const second = await load(pm.host, 5000);
    expect(second.source).toBe("computed");
    expect(second.coords).toEqual(first.coords);
    // ... and that heals it.
    const third = await load(pm.host, 5000);
    expect(third.source).toBe("cache");
    expect(third.coords).toEqual(first.coords);
  });

  it("the fingerprint is cleared before any row changes and written after the last", async () => {
    const { pm } = world(N);
    await load(pm.host, 5000);
    const order = writes(pm).map((c) =>
      c.method === "storage.set"
        ? (c.args as { value: unknown }).value === null
          ? "clear"
          : "fingerprint"
        : c.method === "storage.doc.delete"
          ? "delete"
          : "rows",
    );
    expect(order).toEqual(["clear", "delete", "rows", "rows", "rows", "fingerprint"]);
  });
});

describe("batching under PM's caps", () => {
  it("5,000 documents are written in batches of at most 1,000, each far under the argument cap", async () => {
    const { pm } = world(5000);
    const result = await load(pm.host, 5000);
    expect(result).toMatchObject({ placed: 5000, saved: true });
    const sets = pm.of("storage.doc.set").map((c) => c.args as { entries: unknown[] });
    expect(sets).toHaveLength(5);
    for (const s of sets) {
      expect(s.entries.length).toBeLessThanOrEqual(BATCH_ENTRIES);
      expect(JSON.stringify(s).length).toBeLessThan(BATCH_BYTES + 200);
    }
    expect(pm.rows.get(KEY_SEMANTIC_COORDS)!.size).toBe(5000);
    expect((await load(pm.host, 5000)).source).toBe("cache");
  });

  it("splits by entries, by bytes, and puts an oversized row alone; nothing is lost or reordered", () => {
    const row = (id: number, pad = 0) => ({
      documentId: id,
      value: { x: 0.5, y: 0.5, pad: "p".repeat(pad) },
    });
    const entries = Array.from({ length: 2500 }, (_, i) => row(i + 1));
    const byEntries = batches(entries);
    expect(byEntries.map((b) => b.length)).toEqual([1000, 1000, 500]);
    expect(byEntries.flat()).toEqual(entries);

    const big = Array.from({ length: 10 }, (_, i) => row(i + 1, 100_000));
    const byBytes = batches(big);
    expect(byBytes.length).toBeGreaterThan(2);
    for (const b of byBytes) {
      const size = b.reduce(
        (s, r) => s + JSON.stringify({ documentId: r.documentId, value: r.value }).length + 1,
        0,
      );
      expect(b.length === 1 || size <= BATCH_BYTES).toBe(true);
    }
    expect(byBytes.flat()).toEqual(big);

    const huge = [row(1), row(2, BATCH_BYTES * 2), row(3)];
    expect(batches(huge).map((b) => b.map((r) => r.documentId))).toEqual([[1], [2], [3]]);
    expect(batches([])).toEqual([]);
  });
});

describe("PM's write limits", () => {
  it("waits out the growth limit (MOD-3001) and carries on", async () => {
    const docs = documents(2500, 3);
    const pm = memoryHost({
      docs,
      vectors: vectorsFor(docs, DIM),
      dimension: DIM,
      growthBudget: 2,
    });
    const waits: number[] = [];
    const sleep = (ms: number) => {
      waits.push(ms);
      pm.refill(1);
      return Promise.resolve();
    };
    const result = await load(pm.host, 5000, { sleep });
    expect(result.saved).toBe(true);
    expect(waits.length).toBeGreaterThan(0);
    expect(waits.every((w) => w === 300)).toBe(true);
    expect((await load(pm.host, 5000)).source).toBe("cache");
  });

  it("gives up after a few waits and still returns the map", async () => {
    const docs = documents(30, 3);
    const pm = memoryHost({
      docs,
      vectors: vectorsFor(docs, DIM),
      dimension: DIM,
      growthBudget: 0,
    });
    const sleep = vi.fn(() => Promise.resolve());
    const result = await load(pm.host, 1000, { sleep });
    expect(result).toMatchObject({ source: "computed", placed: 30, saved: false });
    // The first write was tried once and then retried 8 times.
    expect(pm.of("storage.set")).toHaveLength(9);
    expect(sleep).toHaveBeenCalledTimes(8);
    // And nothing half-written is believed.
    expect(
      (await load(memoryHost({ docs, vectors: vectorsFor(docs, DIM), dimension: DIM }).host))
        .source,
    ).toBe("computed");
  });

  it("a full store (MOD-3003) is not retried", async () => {
    const { pm } = world(30);
    pm.failAlways("storage.doc.set", CODE.quotaExceeded);
    const sleep = vi.fn(() => Promise.resolve());
    const result = await load(pm.host, 1000, { sleep });
    expect(result.saved).toBe(false);
    expect(pm.of("storage.doc.set")).toHaveLength(1);
    expect(sleep).not.toHaveBeenCalled();
  });

  it("another frame writing the fingerprint first (a stale revision) costs only the save", async () => {
    const { pm } = world(30);
    pm.failNext("storage.set", CODE.staleRevision);
    const result = await load(pm.host);
    expect(result).toMatchObject({ source: "computed", placed: 30, saved: false });
  });
});

describe("edges", () => {
  it("no document has a vector yet: nothing to place, nothing asked, nothing written", async () => {
    const docs = documents(5, 2);
    const pm = memoryHost({ docs, dimension: DIM });
    const result = await load(pm.host);
    expect(result).toEqual({
      method: "pca",
      coords: [],
      source: "computed",
      total: 0,
      placed: 0,
      saved: true,
    });
    expect(methods(pm)).toEqual(["embeddings.signature"]);
  });

  it("past the node cap, the most important are placed and the rest are counted", async () => {
    const { pm } = world(300);
    const result = await load(pm.host, 200);
    expect(result).toMatchObject({ total: 300, placed: 200, saved: true });
    expect(pm.of("embeddings.documentVectors")[0].args).toEqual({ cap: 200 });
    expect(pm.rows.get(KEY_SEMANTIC_COORDS)!.size).toBe(200);
    expect(readMeta(pm.kv.get(KEY_SEMANTIC_META)!.value)).toMatchObject({ cap: 200, nodes: 200 });
  });

  it("a projection that fails is the caller's to report, and nothing is written", async () => {
    const { pm } = world(30);
    const failing: Compute = {
      mode: "worker",
      semantic: () => ({
        result: Promise.reject(new Error("The layout worker failed.")),
        cancel: () => undefined,
      }),
      force: () => Promise.reject(new Error("unused")),
      close: () => undefined,
    };
    await expect(
      loadSemanticCoords(pm.host, failing, { nodeCap: 1000, sleep: noSleep }),
    ).rejects.toThrow(/layout worker failed/);
    expect(writes(pm)).toHaveLength(0);
  });

  it("a PM that refuses the vectors is the caller's to report, and nothing is written", async () => {
    const { pm } = world(30);
    pm.failAlways("embeddings.documentVectors", "MOD-2001");
    await expect(load(pm.host)).rejects.toMatchObject({ code: "MOD-2001" });
    expect(writes(pm)).toHaveLength(0);
  });

  it("a signature PM will not give is the caller's to report", async () => {
    const { pm } = world(30);
    pm.failAlways("embeddings.signature", "MOD-2001");
    await expect(load(pm.host)).rejects.toMatchObject({ code: "MOD-2001" });
  });

  it("a cache that cannot be read at all is a failure to report, not a silent empty map", async () => {
    const { pm } = world(30);
    pm.failAlways("storage.get", "MOD-9001");
    await expect(load(pm.host)).rejects.toMatchObject({ code: "MOD-9001" });
  });

  it("more vectors than the cap, repeated ids, or ids that are not documents are refused, not drawn", async () => {
    const { pm } = world(300);
    const tooMany: MapHost = {
      ...pm.host,
      documentVectors: () => pm.host.documentVectors({ cap: 300 }),
    };
    await expect(load(tooMany, 200)).rejects.toThrow(/not what the Map expects/);

    const repeated: MapHost = {
      ...pm.host,
      documentVectors: async (o) => {
        const v = await pm.host.documentVectors(o);
        v.ids[1] = v.ids[0];
        return v;
      },
    };
    await expect(load(repeated, 200)).rejects.toThrow(/not what the Map expects/);

    const negative: MapHost = {
      ...pm.host,
      documentVectors: async (o) => {
        const v = await pm.host.documentVectors(o);
        v.ids[0] = -4;
        return v;
      },
    };
    await expect(load(negative, 200)).rejects.toThrow(/not what the Map expects/);
    expect(writes(pm)).toHaveLength(0);
  });
});

describe("isFresh and readMeta", () => {
  const sig = { count: 4, dimension: 8, hash: "h" };
  const meta = { v: 1 as const, hash: "h", dimension: 8, cap: 1000, algo: ALGO_VERSION, nodes: 4 };

  it("fresh only when hash, dimension, cap and algorithm all agree", () => {
    expect(isFresh(meta, sig, 1000)).toBe(true);
    expect(isFresh(null, sig, 1000)).toBe(false);
    expect(isFresh({ ...meta, hash: "x" }, sig, 1000)).toBe(false);
    expect(isFresh({ ...meta, dimension: 9 }, sig, 1000)).toBe(false);
    expect(isFresh({ ...meta, cap: 500 }, sig, 1000)).toBe(false);
    expect(isFresh({ ...meta, algo: ALGO_VERSION + 1 }, sig, 1000)).toBe(false);
  });

  it("reads only a complete meta", () => {
    expect(readMeta(meta)).toEqual(meta);
    for (const bad of [
      null,
      5,
      "x",
      [],
      { ...meta, v: 2 },
      { ...meta, hash: 1 },
      { ...meta, nodes: -1 },
      { ...meta, cap: 1.5 },
    ]) {
      expect(readMeta(bad)).toBeNull();
    }
  });
});

describe("through the real SDK and a fake PM", () => {
  it("cold then warm, the vectors arriving as PM sends them (one buffer, checked views)", async () => {
    const { pm } = world(1200);
    const fake = fakePm(pm.backend);
    const pending = connectWith(fake.env);
    await fake.settle();
    const host = await pending;

    const direct = await load(world(1200).pm.host);
    const cold = await load(host);
    expect(cold).toMatchObject({ source: "computed", placed: 1000, saved: true });
    expect(cold.coords).toEqual(direct.coords);
    expect(fake.stats.stopped).toBe(false);
    expect(fake.stats.refused).toBe(0);

    const warm = await load(host);
    expect(warm.source).toBe("cache");
    expect(warm.coords).toEqual(cold.coords);
  });

  it("5,000 documents of 1,024 floats fit the SDK's limits end to end", async () => {
    const docs = documents(5000, 4);
    const pm = memoryHost({ docs, vectors: vectorsFor(docs, 1024), dimension: 1024 });
    const fake = fakePm(pm.backend);
    const pending = connectWith(fake.env);
    await fake.settle();
    const host = await pending;
    const cold = await load(host, 5000);
    expect(cold).toMatchObject({ placed: 5000, saved: true, source: "computed" });
    expect(fake.stats.stopped).toBe(false);
    expect((await load(host, 5000)).source).toBe("cache");
  }, 60_000);
});
