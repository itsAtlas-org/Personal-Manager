// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The by-meaning pipeline with the enhanced (t-SNE) layout: the six rows of the table at the top of
// `loadSemanticCoords`, each with the host calls it makes, the job it posts, what it draws in order and
// what it writes; the two cache pairs; Cancel, the deadline, a failing worker and a refused save; and one
// run through the real Worker protocol. The job is a steered stand-in (testing.ts scriptedCompute): its
// PCA is the real one and its "t-SNE" is PCA mirrored, so a test can tell the two pictures apart.

import { describe, expect, it } from "vitest";
import { workerCompute, type WorkerLike } from "./compute";
import { ModCallError } from "../../../src/kit/host";
import { type MapHost } from "./hostApi";
import { memoryHost, type MemoryHost } from "./memoryHost";
import {
  ALGO_VERSION,
  KEY_SEMANTIC_COORDS,
  KEY_SEMANTIC_COORDS_TSNE,
  KEY_SEMANTIC_META,
  KEY_SEMANTIC_META_TSNE,
  loadSemanticCoords,
  readMeta,
  SemanticControl,
  TSNE_ALGO_VERSION,
  type SemanticOptions,
  type SemanticResult,
} from "./semantic";
import { documents, scriptedCompute, vectorsFor } from "./testing";
import { handle, type Interim, type WorkerReply } from "./workerProtocol";

const DIM = 8;
const noSleep = () => Promise.resolve();
const tick = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

function world(n = 40) {
  const docs = documents(n, 3);
  const pm = memoryHost({
    docs,
    vectors: vectorsFor(docs, DIM),
    dimension: DIM,
    contentKeys: ["settings"],
  });
  return { docs, pm };
}

/** What a run did, in order: the placeholder, the quick layout, and the phases. */
interface Seen {
  stale: { id: number; x: number; y: number }[][];
  provisional: SemanticResult[];
  phases: string[];
  progress: number[];
}

function watch(extra: Partial<SemanticOptions> = {}): { options: SemanticOptions; seen: Seen } {
  const seen: Seen = { stale: [], provisional: [], phases: [], progress: [] };
  return {
    seen,
    options: {
      nodeCap: 1000,
      sleep: noSleep,
      onStale: (c) => seen.stale.push(c),
      onProvisional: (r) => seen.provisional.push(r),
      onPhase: (p) => seen.phases.push(p),
      onProgress: (f) => seen.progress.push(f),
      ...extra,
    },
  };
}

/** The calls PM got, as "method" or "method:key". */
const trace = (pm: MemoryHost) =>
  pm.calls.map((c) => {
    const key = (c.args as { key?: string } | undefined)?.key;
    return key ? `${c.method}:${key}` : c.method;
  });

const PCA_KEYS = [KEY_SEMANTIC_META, KEY_SEMANTIC_COORDS];
const TSNE_KEYS = [KEY_SEMANTIC_META_TSNE, KEY_SEMANTIC_COORDS_TSNE];

/** The write calls, as "method:key". */
const writesOf = (pm: MemoryHost) =>
  trace(pm).filter((t) => /^storage\.(set|doc\.set|doc\.delete):/.test(t));
const keysOf = (calls: string[]) => Array.from(new Set(calls.map((t) => t.split(":")[1])));

/** A run to its end by itself, both layouts: the cache is warm for both pairs afterwards. */
async function prime(pm: MemoryHost, n = 1000) {
  const compute = scriptedCompute(true);
  const result = await loadSemanticCoords(pm.host, compute, {
    nodeCap: n,
    enhanced: true,
    sleep: noSleep,
  });
  expect(result).toMatchObject({ method: "tsne", saved: true });
  pm.calls.length = 0;
  return result;
}

/** Break one stored fingerprint without touching its rows. */
function staleMeta(pm: MemoryHost, key: string, change: Record<string, unknown>) {
  const m = pm.kv.get(key)!;
  pm.kv.set(key, { value: { ...(m.value as object), ...change }, revision: m.revision });
}

describe("row 1: enhanced on, both caches fresh", () => {
  it("draws the enhanced layout at once: no vectors, no job, no write; the quick cache is not even read", async () => {
    const { pm } = world();
    const first = await prime(pm);
    const compute = scriptedCompute();
    const { options, seen } = watch({ enhanced: true });
    const result = await loadSemanticCoords(pm.host, compute, options);
    expect(trace(pm)).toEqual([
      "embeddings.signature",
      `storage.get:${KEY_SEMANTIC_META_TSNE}`,
      `storage.doc.list:${KEY_SEMANTIC_COORDS_TSNE}`,
    ]);
    expect(compute.jobs).toHaveLength(0);
    expect(result).toMatchObject({ method: "tsne", source: "cache", saved: true });
    expect(result.stopped).toBeUndefined();
    expect(result.coords).toEqual(first.coords);
    expect(seen.stale).toEqual([]);
    expect(seen.provisional).toEqual([]);
    expect(seen.phases).toEqual(["checking"]);
  });
});

describe("row 2: enhanced on, quick cache stale, enhanced cache fresh", () => {
  it("is the same as row 1: the enhanced picture at once, nothing redone, the quick layout left stale", async () => {
    const { pm } = world();
    const first = await prime(pm);
    staleMeta(pm, KEY_SEMANTIC_META, { algo: ALGO_VERSION - 1 });
    const compute = scriptedCompute();
    const result = await loadSemanticCoords(pm.host, compute, watch({ enhanced: true }).options);
    expect(trace(pm)).toEqual([
      "embeddings.signature",
      `storage.get:${KEY_SEMANTIC_META_TSNE}`,
      `storage.doc.list:${KEY_SEMANTIC_COORDS_TSNE}`,
    ]);
    expect(compute.jobs).toHaveLength(0);
    expect(result.coords).toEqual(first.coords);
    expect(writesOf(pm)).toEqual([]);
  });
});

describe("row 3: enhanced on, quick cache fresh, enhanced cache stale", () => {
  it("reads the vectors once, posts a t-SNE only job, shows the quick layout, then writes only the enhanced pair", async () => {
    const { pm } = world();
    await prime(pm);
    const quickCoords = (
      await loadSemanticCoords(pm.host, scriptedCompute(), { nodeCap: 1000, sleep: noSleep })
    ).coords;
    pm.calls.length = 0;
    // The enhanced fingerprint is gone (a write that never finished, or the algorithm moved on).
    pm.kv.set(KEY_SEMANTIC_META_TSNE, {
      value: null,
      revision: pm.kv.get(KEY_SEMANTIC_META_TSNE)!.revision,
    });
    const compute = scriptedCompute();
    const { options, seen } = watch({ enhanced: true });
    const run = loadSemanticCoords(pm.host, compute, options);
    await tick();
    await tick();

    // Calls so far: both fingerprints, the quick rows (whole, so fresh), the vectors once.
    expect(trace(pm)).toEqual([
      "embeddings.signature",
      `storage.get:${KEY_SEMANTIC_META_TSNE}`,
      `storage.get:${KEY_SEMANTIC_META}`,
      `storage.doc.list:${KEY_SEMANTIC_COORDS}`,
      "embeddings.documentVectors",
    ]);
    expect(compute.jobs).toHaveLength(1);
    expect(compute.jobs[0]).toMatchObject({ pca: false, tsne: true });
    // Drawn so far: the saved quick layout as the placeholder (there is no old enhanced picture).
    expect(seen.stale).toEqual([quickCoords]);
    expect(seen.provisional).toEqual([]);
    expect(seen.phases).toEqual(["checking", "reading", "sharpening"]);

    pm.calls.length = 0;
    compute.jobs[0].progress(0.5);
    compute.jobs[0].finish();
    const result = await run;
    expect(result).toMatchObject({ method: "tsne", source: "computed", saved: true });
    // Only the enhanced pair was written, fingerprint cleared first and written last.
    expect(keysOf(writesOf(pm))).toEqual(TSNE_KEYS);
    expect(writesOf(pm).map((t) => t.split(":")[0])).toEqual([
      "storage.set",
      "storage.doc.delete",
      "storage.doc.set",
      "storage.set",
    ]);
    expect(pm.of("embeddings.documentVectors")).toHaveLength(0);
    expect(seen.progress).toEqual([0.5]);
  });

  it("an old enhanced picture whose rows are whole is the placeholder, not the quick layout", async () => {
    const { pm } = world();
    const first = await prime(pm);
    // The documents' fingerprint moved on for the enhanced pair only: its rows are old but whole.
    staleMeta(pm, KEY_SEMANTIC_META_TSNE, { hash: "an older set of documents" });
    const compute = scriptedCompute();
    const { options, seen } = watch({ enhanced: true });
    const run = loadSemanticCoords(pm.host, compute, options);
    await tick();
    await tick();
    expect(seen.stale).toEqual([first.coords]);
    expect(compute.jobs[0]).toMatchObject({ pca: false, tsne: true });
    compute.jobs[0].finish();
    await run;
  });
});

describe("row 4: enhanced on, both caches stale", () => {
  it("reads the vectors once, posts one job for both layouts, draws the quick one first and keeps it while t-SNE runs", async () => {
    const { pm } = world();
    const compute = scriptedCompute();
    const { options, seen } = watch({ enhanced: true });
    const run = loadSemanticCoords(pm.host, compute, options);
    await tick();
    await tick();

    expect(trace(pm)).toEqual([
      "embeddings.signature",
      `storage.get:${KEY_SEMANTIC_META_TSNE}`,
      `storage.get:${KEY_SEMANTIC_META}`,
      "embeddings.documentVectors",
    ]);
    expect(compute.jobs).toHaveLength(1);
    expect(compute.jobs[0]).toMatchObject({ pca: true, tsne: true, count: 40, dimension: DIM });
    // Nothing old to show, and the quick layout is not there yet.
    expect(seen.stale).toEqual([]);
    expect(seen.provisional).toEqual([]);
    expect(seen.phases).toEqual(["checking", "reading", "reducing"]);

    // The quick layout lands: drawn at once, and written while t-SNE is still running.
    compute.jobs[0].provisional();
    await tick();
    await tick();
    await tick();
    expect(seen.provisional).toHaveLength(1);
    expect(seen.provisional[0]).toMatchObject({ method: "pca", source: "computed", placed: 40 });
    expect(seen.phases).toEqual(["checking", "reading", "reducing", "sharpening"]);
    expect(compute.jobs[0].ended).toBe(false);
    expect(keysOf(writesOf(pm))).toEqual(PCA_KEYS);
    expect(readMeta(pm.kv.get(KEY_SEMANTIC_META)!.value)).toMatchObject({ nodes: 40 });
    // The enhanced pair is not touched yet.
    expect(pm.kv.has(KEY_SEMANTIC_META_TSNE)).toBe(false);

    compute.jobs[0].finish();
    const result = await run;
    expect(result).toMatchObject({ method: "tsne", source: "computed", placed: 40, saved: true });
    expect(keysOf(writesOf(pm))).toEqual([...PCA_KEYS, ...TSNE_KEYS]);
    expect(pm.of("embeddings.documentVectors")).toHaveLength(1);
    // The enhanced coordinates are the ones written, not the quick ones.
    const stored = pm.rows.get(KEY_SEMANTIC_COORDS_TSNE)!;
    for (const c of result.coords) expect(stored.get(c.id)).toEqual({ x: c.x, y: c.y });
    expect(result.coords).not.toEqual(seen.provisional[0].coords);
  });

  it("an old enhanced picture is the placeholder and the quick layout is then not drawn (it would jump twice), but still saved", async () => {
    const { pm } = world();
    const first = await prime(pm);
    pm.addDoc({ ...documents(1, 1)[0], id: 500 }, new Float32Array(DIM).fill(0.4));
    const compute = scriptedCompute();
    const { options, seen } = watch({ enhanced: true });
    const run = loadSemanticCoords(pm.host, compute, options);
    await tick();
    await tick();
    expect(seen.stale).toEqual([first.coords]);
    compute.jobs[0].provisional();
    await tick();
    await tick();
    await tick();
    expect(seen.provisional).toEqual([]);
    expect(keysOf(writesOf(pm))).toContain(KEY_SEMANTIC_COORDS);
    compute.jobs[0].finish();
    expect(await run).toMatchObject({ method: "tsne", placed: 41 });
  });

  it("with no old enhanced picture the old quick one is the placeholder, then the new quick layout is drawn", async () => {
    const { pm } = world();
    const quick = await loadSemanticCoords(pm.host, scriptedCompute(true), {
      nodeCap: 1000,
      sleep: noSleep,
    });
    pm.addDoc({ ...documents(1, 1)[0], id: 500 }, new Float32Array(DIM).fill(0.4));
    const compute = scriptedCompute();
    const { options, seen } = watch({ enhanced: true });
    const run = loadSemanticCoords(pm.host, compute, options);
    await tick();
    await tick();
    expect(seen.stale).toEqual([quick.coords]);
    compute.jobs[0].provisional();
    await tick();
    expect(seen.provisional).toHaveLength(1);
    compute.jobs[0].finish();
    await run;
  });
});

describe("rows 5 and 6: enhanced off", () => {
  it("row 5, the quick cache fresh: the quick layout at once, the enhanced cache never read, no vectors, no job, no write", async () => {
    const { pm } = world();
    const first = await prime(pm);
    const compute = scriptedCompute();
    const result = await loadSemanticCoords(pm.host, compute, watch({ enhanced: false }).options);
    expect(trace(pm)).toEqual([
      "embeddings.signature",
      `storage.get:${KEY_SEMANTIC_META}`,
      `storage.doc.list:${KEY_SEMANTIC_COORDS}`,
    ]);
    expect(compute.jobs).toHaveLength(0);
    expect(result).toMatchObject({ method: "pca", source: "cache" });
    expect(result.coords).not.toEqual(first.coords);
    expect(writesOf(pm)).toEqual([]);
  });

  it("row 6, the quick cache stale: the old quick picture, one vector read, a PCA only job, the quick pair written and nothing else", async () => {
    const { pm } = world();
    await prime(pm);
    pm.addDoc({ ...documents(1, 1)[0], id: 500 }, new Float32Array(DIM).fill(0.4));
    const before = new Map(pm.kv);
    const compute = scriptedCompute(true);
    const { options, seen } = watch({ enhanced: false });
    const result = await loadSemanticCoords(pm.host, compute, options);
    expect(compute.jobs).toHaveLength(1);
    expect(compute.jobs[0]).toMatchObject({ pca: true, tsne: false });
    expect(pm.of("embeddings.documentVectors")).toHaveLength(1);
    expect(seen.stale).toHaveLength(1);
    expect(seen.provisional).toEqual([]);
    expect(result).toMatchObject({ method: "pca", placed: 41, saved: true });
    expect(keysOf(writesOf(pm))).toEqual(PCA_KEYS);
    // The enhanced pair is exactly as it was.
    expect(pm.kv.get(KEY_SEMANTIC_META_TSNE)).toEqual(before.get(KEY_SEMANTIC_META_TSNE));
    expect(pm.rows.get(KEY_SEMANTIC_COORDS_TSNE)!.size).toBe(40);
  });
});

describe("the vectors are read from PM once per run, and only where a job is posted", () => {
  it("rows 3, 4 and 6 read them once; rows 1, 2 and 5 never do", async () => {
    const reads = async (setup: (pm: MemoryHost) => void, enhanced: boolean) => {
      const { pm } = world();
      await prime(pm);
      setup(pm);
      const compute = scriptedCompute(true);
      await loadSemanticCoords(pm.host, compute, { nodeCap: 1000, enhanced, sleep: noSleep });
      return pm.of("embeddings.documentVectors").length;
    };
    const dropTsne = (pm: MemoryHost) =>
      pm.kv.set(KEY_SEMANTIC_META_TSNE, {
        value: null,
        revision: pm.kv.get(KEY_SEMANTIC_META_TSNE)!.revision,
      });
    const addDoc = (pm: MemoryHost) =>
      pm.addDoc({ ...documents(1, 1)[0], id: 500 }, new Float32Array(DIM).fill(0.4));
    expect(await reads(() => undefined, true)).toBe(0); // 1
    expect(await reads((pm) => staleMeta(pm, KEY_SEMANTIC_META, { algo: 0 }), true)).toBe(0); // 2
    expect(await reads(dropTsne, true)).toBe(1); // 3
    expect(await reads(addDoc, true)).toBe(1); // 4
    expect(await reads(() => undefined, false)).toBe(0); // 5
    expect(await reads(addDoc, false)).toBe(1); // 6
  });

  it("a t-SNE job reads no second time, whatever happens to it", async () => {
    const { pm } = world();
    const compute = scriptedCompute();
    const run = loadSemanticCoords(pm.host, compute, watch({ enhanced: true }).options);
    await tick();
    await tick();
    compute.jobs[0].provisional();
    compute.jobs[0].progress(0.3);
    compute.jobs[0].finish();
    await run;
    expect(pm.of("embeddings.documentVectors")).toHaveLength(1);
    expect(pm.of("embeddings.signature")).toHaveLength(1);
  });
});

describe("toggling Enhanced uses the other cache pair with no recompute", () => {
  it("off then on again: the second open of each is a cache hit, and no job runs after the first", async () => {
    const { pm } = world();
    const first = await prime(pm);
    const compute = scriptedCompute();
    const off = await loadSemanticCoords(pm.host, compute, {
      nodeCap: 1000,
      enhanced: false,
      sleep: noSleep,
    });
    const on = await loadSemanticCoords(pm.host, compute, {
      nodeCap: 1000,
      enhanced: true,
      sleep: noSleep,
    });
    expect(off).toMatchObject({ method: "pca", source: "cache" });
    expect(on).toMatchObject({ method: "tsne", source: "cache" });
    expect(on.coords).toEqual(first.coords);
    expect(off.coords).not.toEqual(on.coords);
    expect(compute.jobs).toHaveLength(0);
    expect(pm.of("embeddings.documentVectors")).toHaveLength(0);
  });

  it("the enhanced pair never overwrites the quick pair, and the quick pair keeps its own algorithm number", async () => {
    const { pm } = world();
    await prime(pm);
    expect(readMeta(pm.kv.get(KEY_SEMANTIC_META)!.value)).toMatchObject({ algo: ALGO_VERSION });
    expect(readMeta(pm.kv.get(KEY_SEMANTIC_META_TSNE)!.value)).toMatchObject({
      algo: TSNE_ALGO_VERSION,
    });
    const quick = pm.rows.get(KEY_SEMANTIC_COORDS)!;
    const sharp = pm.rows.get(KEY_SEMANTIC_COORDS_TSNE)!;
    expect(quick.size).toBe(40);
    expect(sharp.size).toBe(40);
    expect(Array.from(quick.values())).not.toEqual(Array.from(sharp.values()));
    // The existing algorithm version is the one the quick caches have always had.
    expect(ALGO_VERSION).toBe(1);
  });
});

describe("the enhanced pair is never half true", () => {
  const WRITES = ["storage.set", "storage.doc.delete", "storage.doc.set"];
  /** A host that fails the k-th write to the enhanced pair. */
  const failing = (pm: MemoryHost, k: number): MapHost => {
    let seen = 0;
    return {
      ...pm.host,
      call: (method, args) => {
        const key = (args as { key?: string } | undefined)?.key;
        if (WRITES.includes(method) && key && TSNE_KEYS.includes(key) && ++seen === k) {
          return Promise.reject(
            new ModCallError({ code: "MOD-9001", message: "Something went wrong." }),
          );
        }
        return pm.host.call(method, args);
      },
    };
  };

  it.each([1, 2, 3, 4])(
    "the enhanced write number %i fails: the picture stays, it is not remembered, the next open redoes it",
    async (k) => {
      const { pm } = world();
      const compute = scriptedCompute(true);
      const result = await loadSemanticCoords(failing(pm, k), compute, {
        nodeCap: 1000,
        enhanced: true,
        sleep: noSleep,
      });
      expect(result).toMatchObject({ method: "tsne", saved: false });
      // Whatever half-written state is there is not a layout.
      const next = scriptedCompute(true);
      pm.calls.length = 0;
      const again = await loadSemanticCoords(pm.host, next, {
        nodeCap: 1000,
        enhanced: true,
        sleep: noSleep,
      });
      expect(again).toMatchObject({ method: "tsne", source: "computed", saved: true });
      // The quick pair was whole, so this was a t-SNE only job (row 3).
      expect(next.jobs[0]).toMatchObject({ pca: false, tsne: true });
      expect(
        (await loadSemanticCoords(pm.host, next, { nodeCap: 1000, enhanced: true, sleep: noSleep }))
          .source,
      ).toBe("cache");
    },
  );

  it("the fingerprint is cleared before the rows change and written after the last row", async () => {
    const { pm } = world(2500);
    const compute = scriptedCompute(true);
    await loadSemanticCoords(pm.host, compute, { nodeCap: 5000, enhanced: true, sleep: noSleep });
    const order = pm.calls
      .filter((c) => TSNE_KEYS.includes((c.args as { key?: string }).key ?? ""))
      .filter((c) => WRITES.includes(c.method))
      .map((c) =>
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

describe("Cancel", () => {
  it("after the quick layout exists: the job ends, the quick layout is the answer and is saved, the enhanced pair is not written", async () => {
    const { pm } = world();
    const compute = scriptedCompute();
    const control = new SemanticControl();
    const { options, seen } = watch({ enhanced: true, control });
    const run = loadSemanticCoords(pm.host, compute, options);
    await tick();
    await tick();
    compute.jobs[0].provisional();
    await tick();
    await tick();
    control.cancel();
    const result = await run;
    expect(compute.jobs[0].cancelled).toBe(true);
    expect(result).toMatchObject({ method: "pca", stopped: "cancelled", saved: true, placed: 40 });
    expect(result.coords).toEqual(seen.provisional[0].coords);
    expect(keysOf(writesOf(pm))).toEqual(PCA_KEYS);
    expect(pm.kv.has(KEY_SEMANTIC_META_TSNE)).toBe(false);
  });

  it("before the quick layout exists: it waits for it (the vectors die with the worker), then ends the job", async () => {
    const { pm } = world();
    const compute = scriptedCompute();
    const control = new SemanticControl();
    const run = loadSemanticCoords(pm.host, compute, watch({ enhanced: true, control }).options);
    await tick();
    await tick();
    control.cancel();
    expect(compute.jobs[0].cancelled).toBe(false);
    compute.jobs[0].provisional();
    const result = await run;
    expect(compute.jobs[0].cancelled).toBe(true);
    expect(result).toMatchObject({ method: "pca", stopped: "cancelled" });
  });

  it("while the vectors are being read: only the quick layout is worked out", async () => {
    const { pm } = world();
    const compute = scriptedCompute(true);
    const control = new SemanticControl();
    const host: MapHost = {
      ...pm.host,
      documentVectors: async (o) => {
        control.cancel();
        return pm.host.documentVectors(o);
      },
    };
    const result = await loadSemanticCoords(
      host,
      compute,
      watch({ enhanced: true, control }).options,
    );
    expect(compute.jobs).toHaveLength(1);
    expect(compute.jobs[0]).toMatchObject({ pca: true, tsne: false });
    expect(result).toMatchObject({ method: "pca", stopped: "cancelled", saved: true });
  });

  it("in row 3 the quick layout is in hand: the job ends at once and the saved quick layout is the answer, nothing written", async () => {
    const { pm } = world();
    const first = await prime(pm);
    pm.kv.set(KEY_SEMANTIC_META_TSNE, {
      value: null,
      revision: pm.kv.get(KEY_SEMANTIC_META_TSNE)!.revision,
    });
    const compute = scriptedCompute();
    const control = new SemanticControl();
    const run = loadSemanticCoords(pm.host, compute, watch({ enhanced: true, control }).options);
    await tick();
    await tick();
    pm.calls.length = 0;
    control.cancel();
    const result = await run;
    expect(compute.jobs[0].cancelled).toBe(true);
    expect(result).toMatchObject({ method: "pca", source: "cache", stopped: "cancelled" });
    expect(result.coords).not.toEqual(first.coords);
    expect(writesOf(pm)).toEqual([]);
  });

  it("in row 3 before the vectors are read: no vectors are read and no job is posted", async () => {
    const { pm } = world();
    await prime(pm);
    pm.kv.set(KEY_SEMANTIC_META_TSNE, {
      value: null,
      revision: pm.kv.get(KEY_SEMANTIC_META_TSNE)!.revision,
    });
    const compute = scriptedCompute();
    const control = new SemanticControl();
    control.cancel();
    const result = await loadSemanticCoords(
      pm.host,
      compute,
      watch({ enhanced: true, control }).options,
    );
    expect(result).toMatchObject({ method: "pca", stopped: "cancelled" });
    expect(compute.jobs).toHaveLength(0);
    expect(pm.of("embeddings.documentVectors")).toHaveLength(0);
  });

  it("while the vectors are being read in row 3: still no job", async () => {
    const { pm } = world();
    await prime(pm);
    pm.kv.set(KEY_SEMANTIC_META_TSNE, {
      value: null,
      revision: pm.kv.get(KEY_SEMANTIC_META_TSNE)!.revision,
    });
    const compute = scriptedCompute();
    const control = new SemanticControl();
    const host: MapHost = {
      ...pm.host,
      documentVectors: async (o) => {
        control.cancel();
        return pm.host.documentVectors(o);
      },
    };
    const result = await loadSemanticCoords(
      host,
      compute,
      watch({ enhanced: true, control }).options,
    );
    expect(result).toMatchObject({ method: "pca", stopped: "cancelled" });
    expect(compute.jobs).toHaveLength(0);
  });

  it("once the job has ended a Cancel does nothing", async () => {
    const { pm } = world();
    const compute = scriptedCompute(true);
    const control = new SemanticControl();
    const result = await loadSemanticCoords(
      pm.host,
      compute,
      watch({ enhanced: true, control }).options,
    );
    control.cancel();
    expect(result).toMatchObject({ method: "tsne", saved: true });
    expect(result.stopped).toBeUndefined();
  });
});

describe("a superseded run (abandon)", () => {
  it("ends the job at once, rejects, and writes nothing more", async () => {
    const { pm } = world();
    const compute = scriptedCompute();
    const control = new SemanticControl();
    const run = loadSemanticCoords(pm.host, compute, watch({ enhanced: true, control }).options);
    await tick();
    await tick();
    control.abandon();
    await expect(run).rejects.toMatchObject({ reason: "cancelled" });
    expect(compute.jobs[0].cancelled).toBe(true);
    expect(writesOf(pm)).toEqual([]);
  });

  it("after the quick layout was posted, its save is not started", async () => {
    const { pm } = world();
    const compute = scriptedCompute();
    const control = new SemanticControl();
    const run = loadSemanticCoords(pm.host, compute, watch({ enhanced: true, control }).options);
    await tick();
    await tick();
    control.abandon();
    compute.jobs[0].provisional();
    await expect(run).rejects.toBeDefined();
    expect(writesOf(pm)).toEqual([]);
  });
});

describe("the deadline, a failing worker, and no worker", () => {
  it("the deadline ends with the saved quick layout and says why", async () => {
    const { pm } = world();
    const compute = scriptedCompute();
    const run = loadSemanticCoords(pm.host, compute, watch({ enhanced: true }).options);
    await tick();
    await tick();
    compute.jobs[0].provisional();
    await tick();
    compute.jobs[0].deadline();
    const result = await run;
    expect(result).toMatchObject({ method: "pca", stopped: "deadline", saved: true });
    expect(keysOf(writesOf(pm))).toEqual(PCA_KEYS);
  });

  it("a worker that fails after the quick layout is in hand is not a failed map: the quick layout stands", async () => {
    const { pm } = world();
    const compute = scriptedCompute();
    const run = loadSemanticCoords(pm.host, compute, watch({ enhanced: true }).options);
    await tick();
    await tick();
    compute.jobs[0].provisional();
    await tick();
    compute.jobs[0].fail();
    expect(await run).toMatchObject({ method: "pca", stopped: "failed", saved: true });
  });

  it("a worker that fails with no quick layout is the caller's to report", async () => {
    const { pm } = world();
    const compute = scriptedCompute();
    const run = loadSemanticCoords(pm.host, compute, watch({ enhanced: true }).options);
    await tick();
    await tick();
    compute.jobs[0].fail("The layout worker failed.");
    await expect(run).rejects.toThrow(/layout worker failed/);
    expect(writesOf(pm)).toEqual([]);
  });

  it("no Worker (the page's own thread): the quick layout is saved and the run says the enhanced one needs a worker", async () => {
    const { pm } = world();
    const compute = scriptedCompute(true);
    compute.mode = "inline";
    const result = await loadSemanticCoords(pm.host, compute, watch({ enhanced: true }).options);
    expect(result).toMatchObject({ method: "pca", stopped: "no-worker", saved: true });
    expect(keysOf(writesOf(pm))).toEqual(PCA_KEYS);
    expect(pm.kv.has(KEY_SEMANTIC_META_TSNE)).toBe(false);
  });

  it("no Worker with the quick layout already saved: nothing is written, the saved quick layout is shown", async () => {
    const { pm } = world();
    await loadSemanticCoords(pm.host, scriptedCompute(true), { nodeCap: 1000, sleep: noSleep });
    pm.calls.length = 0;
    const compute = scriptedCompute(true);
    compute.mode = "inline";
    const result = await loadSemanticCoords(pm.host, compute, watch({ enhanced: true }).options);
    expect(result).toMatchObject({ method: "pca", source: "cache", stopped: "no-worker" });
    expect(writesOf(pm)).toEqual([]);
    // t-SNE cannot run here and the quick layout is in hand: the vectors are not even read.
    expect(pm.of("embeddings.documentVectors")).toHaveLength(0);
  });

  it("a Worker that cannot be made, found out on the job: the saved quick layout is shown, nothing is written, no PCA is worked out", async () => {
    const { pm } = world();
    await loadSemanticCoords(pm.host, scriptedCompute(true), { nodeCap: 1000, sleep: noSleep });
    pm.calls.length = 0;
    const compute = workerCompute(() => {
      throw new Error("worker-src blocked");
    });
    const result = await loadSemanticCoords(pm.host, compute, watch({ enhanced: true }).options);
    expect(result).toMatchObject({ method: "pca", source: "cache", stopped: "no-worker" });
    expect(writesOf(pm)).toEqual([]);
    expect(compute.mode).toBe("inline");
    // The next open does not even read the vectors.
    pm.calls.length = 0;
    await loadSemanticCoords(pm.host, compute, watch({ enhanced: true }).options);
    expect(pm.of("embeddings.documentVectors")).toHaveLength(0);
  });
});

describe("an enhanced layout made by an older algorithm is stale", () => {
  it.each([TSNE_ALGO_VERSION - 1, TSNE_ALGO_VERSION + 1])(
    "an enhanced fingerprint of algorithm %i is not trusted, and its rows are not a placeholder",
    async (algo) => {
      const { pm } = world();
      await prime(pm);
      staleMeta(pm, KEY_SEMANTIC_META_TSNE, { algo });
      const compute = scriptedCompute();
      const { options, seen } = watch({ enhanced: true });
      const run = loadSemanticCoords(pm.host, compute, options);
      await tick();
      await tick();
      expect(compute.jobs).toHaveLength(1);
      expect(compute.jobs[0]).toMatchObject({ pca: false, tsne: true });
      // The quick layout is what is shown meanwhile, not the old enhanced picture.
      expect(seen.stale).toHaveLength(1);
      compute.jobs[0].finish();
      const result = await run;
      expect(result.source).toBe("computed");
      expect(readMeta(pm.kv.get(KEY_SEMANTIC_META_TSNE)!.value)).toMatchObject({
        algo: TSNE_ALGO_VERSION,
      });
    },
  );
});

describe("when a save is refused", () => {
  it("a quick layout whose save fails after t-SNE already finished: the enhanced picture stays, and is not written either", async () => {
    const { pm } = world();
    pm.failNext("storage.set", "MOD-3003");
    const compute = scriptedCompute();
    const run = loadSemanticCoords(pm.host, compute, watch({ enhanced: true }).options);
    await tick();
    await tick();
    // Both land before the refused write is heard.
    compute.jobs[0].provisional();
    compute.jobs[0].finish();
    const result = await run;
    expect(result).toMatchObject({ method: "tsne", saved: false });
    expect(pm.kv.has(KEY_SEMANTIC_META_TSNE)).toBe(false);
    expect(writesOf(pm).filter((t) => TSNE_KEYS.some((k) => t.endsWith(k)))).toEqual([]);
  });

  it("the quick layout cannot be saved: t-SNE is not carried on with, and the run says so", async () => {
    const { pm } = world();
    // The first write (clearing the quick fingerprint) is refused.
    pm.failNext("storage.set", "MOD-3003");
    const compute = scriptedCompute();
    const { options, seen } = watch({ enhanced: true });
    const run = loadSemanticCoords(pm.host, compute, options);
    await tick();
    await tick();
    compute.jobs[0].provisional();
    await tick();
    await tick();
    await tick();
    expect(compute.jobs[0].cancelled).toBe(true);
    const result = await run;
    expect(result).toMatchObject({ method: "pca", stopped: "unsaved", saved: false });
    expect(result.coords).toEqual(seen.provisional[0].coords);
    expect(pm.kv.has(KEY_SEMANTIC_META_TSNE)).toBe(false);
    expect(writesOf(pm).filter((t) => TSNE_KEYS.some((k) => t.endsWith(k)))).toEqual([]);
  });
});

describe("a few documents have no t-SNE", () => {
  it("three documents: a PCA only job, the quick layout, no enhanced pair, and the next open is a hit", async () => {
    const { pm } = world(3);
    const compute = scriptedCompute(true);
    const result = await loadSemanticCoords(pm.host, compute, watch({ enhanced: true }).options);
    expect(compute.jobs).toHaveLength(1);
    expect(compute.jobs[0]).toMatchObject({ pca: true, tsne: false });
    expect(result).toMatchObject({ method: "pca", placed: 3, saved: true });
    expect(result.stopped).toBeUndefined();
    expect(pm.kv.has(KEY_SEMANTIC_META_TSNE)).toBe(false);
    pm.calls.length = 0;
    const again = await loadSemanticCoords(pm.host, compute, {
      nodeCap: 1000,
      enhanced: true,
      sleep: noSleep,
    });
    expect(again).toMatchObject({ method: "pca", source: "cache" });
    expect(pm.of("embeddings.documentVectors")).toHaveLength(0);
    expect(trace(pm)).not.toContain(`storage.get:${KEY_SEMANTIC_META_TSNE}`);
  });

  it("four documents are the smallest that run t-SNE", async () => {
    const { pm } = world(4);
    const compute = scriptedCompute(true);
    await loadSemanticCoords(pm.host, compute, { nodeCap: 1000, enhanced: true, sleep: noSleep });
    expect(compute.jobs[0]).toMatchObject({ pca: true, tsne: true });
  });
});

describe("a run left to the old defaults is the quick layout only", () => {
  it("without `enhanced` no t-SNE job is asked for and the method is pca", async () => {
    const { pm } = world();
    const compute = scriptedCompute(true);
    const result = await loadSemanticCoords(pm.host, compute, { nodeCap: 1000, sleep: noSleep });
    expect(compute.jobs[0]).toMatchObject({ pca: true, tsne: false });
    expect(result.method).toBe("pca");
  });
});

describe("through the real Worker protocol", () => {
  /** A Worker stand-in that answers with the real `handle`, moving nothing. */
  function syncWorker(): WorkerLike {
    const w: WorkerLike = {
      onmessage: null,
      onerror: null,
      onmessageerror: null,
      terminate: () => undefined,
      postMessage(message) {
        setTimeout(() => {
          const sent: Interim[] = [];
          const reply: WorkerReply = handle(message, (r) => sent.push(r));
          for (const r of [...sent, reply]) w.onmessage?.({ data: r });
        }, 0);
      },
    };
    return w;
  }

  it("a cold run is both layouts; the next is the enhanced picture from the cache, and the quick layout is the one stored beside it", async () => {
    const { pm } = world(60);
    const compute = workerCompute(syncWorker);
    const { options, seen } = watch({ enhanced: true });
    const first = await loadSemanticCoords(pm.host, compute, options);
    expect(first).toMatchObject({ method: "tsne", placed: 60, saved: true });
    expect(seen.provisional).toHaveLength(1);
    expect(seen.progress.length).toBeGreaterThan(5);
    expect(first.coords.every((c) => c.x >= 0 && c.x <= 1 && c.y >= 0 && c.y <= 1)).toBe(true);
    pm.calls.length = 0;
    const second = await loadSemanticCoords(pm.host, compute, {
      nodeCap: 1000,
      enhanced: true,
      sleep: noSleep,
    });
    expect(second).toMatchObject({ method: "tsne", source: "cache" });
    expect(second.coords).toEqual(first.coords);
    const quick = await loadSemanticCoords(pm.host, compute, { nodeCap: 1000, sleep: noSleep });
    expect(quick).toMatchObject({ method: "pca", source: "cache" });
    expect(quick.coords).toEqual(seen.provisional[0].coords);
  });
});
