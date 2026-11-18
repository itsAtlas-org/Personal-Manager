// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The page's side of the Worker: jobs matched to replies, a failed worker replaced, a missing worker
// replaced by the page's own thread. The stand-in worker runs the real `handle` and moves buffers the
// way a Worker does (structuredClone with a transfer list), so "the vectors are moved, not copied" and
// "the worker's reply arrives as its own copy" are tested, not assumed.

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  inlineCompute,
  SEMANTIC_DEADLINE_MS,
  SemanticStopped,
  type Compute,
  WORKER_CLOSED,
  WORKER_FAILED,
  workerCompute,
  type WorkerLike,
} from "./compute";
import { buildGraph, forceRequest } from "./layoutModel";
import { reduceToUnit } from "./pca";
import { documents } from "./testing";
import { handle, transferOf, type Interim, type WorkerReply } from "./workerProtocol";

interface Fake extends WorkerLike {
  terminated: number;
  posted: unknown[];
  /** Answer the next message(s) as the real worker would, but only when released. */
  hold: boolean;
  /** Answer the held jobs last-in first-out. */
  reversed: boolean;
  release(): void;
  fail(): void;
}

/** A Worker stand-in that answers with the real `handle`, a macrotask after the request. */
function fakeWorker(
  answer: (request: unknown, emit: (reply: Interim) => void) => WorkerReply | null = (r, emit) =>
    handle(r, emit),
): Fake {
  const queue: unknown[] = [];
  const w: Fake = {
    terminated: 0,
    posted: [],
    hold: false,
    reversed: false,
    onmessage: null,
    onerror: null,
    onmessageerror: null,
    postMessage(message, transfer) {
      const cloned = structuredClone(message, { transfer });
      w.posted.push(cloned);
      queue.push(cloned);
      if (!w.hold) w.release();
    },
    release() {
      const batch = queue.splice(0);
      if (w.reversed) batch.reverse();
      batch.forEach((request, i) => {
        setTimeout(() => {
          const sent: WorkerReply[] = [];
          const reply = answer(request, (interim) => sent.push(interim));
          if (reply === null || w.terminated > 0) return;
          for (const r of [...sent, reply]) {
            const copy = structuredClone(r, { transfer: transferOf(r) });
            w.onmessage?.({ data: copy });
          }
        }, i);
      });
    },
    terminate() {
      w.terminated += 1;
    },
    fail() {
      w.onerror?.({ message: "boom with a document title in it" });
    },
  };
  return w;
}

/** The PCA-only job, the old `reduce`. */
const reduce = (compute: Compute, v: Float32Array, count: number, dimension: number) =>
  compute.semantic(v, count, dimension, { pca: true, tsne: false }).result;

const vectors = () => {
  const v = new Float32Array(8 * 4);
  for (let i = 0; i < v.length; i++) v[i] = Math.cos(i * 0.9) * (1 + (i % 4));
  return v;
};

describe("workerCompute", () => {
  it("makes no worker until the first job, then one for the semantic jobs and one for the force jobs", async () => {
    const spawn = vi.fn(() => fakeWorker());
    const compute = workerCompute(spawn);
    expect(spawn).not.toHaveBeenCalled();
    await reduce(compute, vectors(), 8, 4);
    expect(spawn).toHaveBeenCalledTimes(1);
    await reduce(compute, vectors(), 8, 4);
    expect(spawn).toHaveBeenCalledTimes(1);
    await compute.force(forceRequest(buildGraph(documents(5, 2))));
    await compute.force(forceRequest(buildGraph(documents(5, 2))));
    expect(spawn).toHaveBeenCalledTimes(2);
    expect(compute.mode).toBe("worker");
  });

  it("reduce: the worker's projection, and the vectors' buffer is moved to it", async () => {
    const compute = workerCompute(() => fakeWorker());
    const v = vectors();
    const expected = Array.from(reduceToUnit(v.slice(), 8, 4).coords);
    const result = await reduce(compute, v, 8, 4);
    expect(result.method).toBe("pca");
    expect(Array.from(result.coords)).toEqual(expected);
    // Moved, not copied: the page's view is detached.
    expect(v.byteLength).toBe(0);
  });

  it("force: the worker's simulation", async () => {
    const compute = workerCompute(() => fakeWorker());
    const request = forceRequest(buildGraph(documents(9, 2)));
    const result = await compute.force(request);
    expect(result.positions.map((p) => p.id)).toEqual(request.nodes.map((n) => n.id));
  });

  it("matches replies to jobs by id, whatever order they come back in", async () => {
    const w = fakeWorker();
    w.hold = true;
    w.reversed = true;
    const compute = workerCompute(() => w);
    const a = reduce(compute, vectors(), 8, 4);
    const b = reduce(compute, vectors().slice(0, 6 * 4), 6, 4);
    await Promise.resolve();
    const [first, second] = w.posted as { id: number; kind: string }[];
    expect([first.kind, second.kind]).toEqual(["semantic", "semantic"]);
    expect(first.id).not.toBe(second.id);
    // The second job's reply arrives first, then the first job's.
    const order: string[] = [];
    void a.then(() => order.push("a"));
    void b.then(() => order.push("b"));
    w.release();
    const [ra, rb] = await Promise.all([a, b]);
    expect(order).toEqual(["b", "a"]);
    expect(ra.coords).toHaveLength(8 * 2);
    expect(rb.coords).toHaveLength(6 * 2);
  });

  it("a job's error reply rejects that job with our sentence, and the worker stays", async () => {
    const w = fakeWorker();
    const compute = workerCompute(() => w);
    await expect(reduce(compute, new Float32Array(3), 4, 3)).rejects.toThrow(/do not match/);
    expect(w.terminated).toBe(0);
    await expect(reduce(compute, vectors(), 8, 4)).resolves.toBeDefined();
  });

  it("a worker error fails every job waiting on it, without repeating its text, and the next job gets a fresh worker", async () => {
    const workers: Fake[] = [];
    const spawn = () => {
      const w = fakeWorker();
      w.hold = workers.length === 0;
      workers.push(w);
      return w;
    };
    const compute = workerCompute(spawn);
    const a = reduce(compute, vectors(), 8, 4);
    const b = reduce(compute, vectors(), 8, 4);
    const settled = Promise.allSettled([a, b]);
    await Promise.resolve();
    workers[0].fail();
    const results = await settled;
    for (const r of results) {
      expect(r.status).toBe("rejected");
      expect((r as PromiseRejectedResult).reason.message).toBe(WORKER_FAILED);
      expect((r as PromiseRejectedResult).reason.message).not.toContain("document title");
    }
    expect(workers[0].terminated).toBe(1);
    await expect(reduce(compute, vectors(), 8, 4)).resolves.toBeDefined();
    expect(workers).toHaveLength(2);
  });

  it("a message the worker's page cannot read (messageerror) is the same", async () => {
    const w = fakeWorker();
    w.hold = true;
    const compute = workerCompute(() => w);
    const job = reduce(compute, vectors(), 8, 4);
    await Promise.resolve();
    w.onmessageerror?.({});
    await expect(job).rejects.toThrow(WORKER_FAILED);
    expect(w.terminated).toBe(1);
  });

  it("postMessage throwing (a buffer that cannot be moved) rejects and drops the worker", async () => {
    const w = fakeWorker();
    w.postMessage = () => {
      throw new Error("DataCloneError");
    };
    const compute = workerCompute(() => w);
    await expect(reduce(compute, vectors(), 8, 4)).rejects.toThrow(WORKER_FAILED);
    expect(w.terminated).toBe(1);
  });

  it("an error reply that names no job of ours is a worker in an unknown state: dropped", async () => {
    const w = fakeWorker(() => ({ id: -1, kind: "error", message: "?" }));
    const compute = workerCompute(() => w);
    const job = reduce(compute, vectors(), 8, 4);
    await expect(job).rejects.toThrow(WORKER_FAILED);
    expect(w.terminated).toBe(1);
  });

  it("a reply for a job nobody is waiting for is ignored", async () => {
    const w = fakeWorker();
    w.hold = true;
    const compute = workerCompute(() => w);
    const job = reduce(compute, vectors(), 8, 4);
    await Promise.resolve();
    w.onmessage?.({
      data: { id: 999, kind: "semantic", coords: new Float64Array(16), method: "pca" },
    });
    w.onmessage?.({ data: null });
    w.release();
    await expect(job).resolves.toBeDefined();
    expect(w.terminated).toBe(0);
  });

  it("a reduce reply with the wrong number of coordinates is refused, not drawn", async () => {
    const w = fakeWorker((r) => {
      const reply = handle(r);
      return reply.kind === "semantic" ? { ...reply, coords: new Float64Array(3) } : reply;
    });
    const compute = workerCompute(() => w);
    await expect(reduce(compute, vectors(), 8, 4)).rejects.toThrow(WORKER_FAILED);
  });

  it("a reply of the wrong kind, or a force reply with the wrong number of positions, is refused", async () => {
    const wrongKind = workerCompute(() =>
      fakeWorker((r) => {
        const id = (r as { id: number }).id;
        return { id, kind: "force", positions: [] };
      }),
    );
    await expect(reduce(wrongKind, vectors(), 8, 4)).rejects.toThrow(WORKER_FAILED);
    const short = workerCompute(() =>
      fakeWorker((r) => {
        const id = (r as { id: number }).id;
        return { id, kind: "force", positions: [{ id: "x", x: 0, y: 0 }] };
      }),
    );
    await expect(short.force(forceRequest(buildGraph(documents(4, 2))))).rejects.toThrow(
      WORKER_FAILED,
    );
  });

  it("terminate throwing is swallowed", async () => {
    const w = fakeWorker();
    w.terminate = () => {
      throw new Error("gone");
    };
    const compute = workerCompute(() => w);
    const job = reduce(compute, vectors(), 8, 4);
    await job;
    expect(() => compute.close()).not.toThrow();
  });

  it("close stops the worker, fails what waits, refuses later jobs, and is idempotent", async () => {
    const w = fakeWorker();
    w.hold = true;
    const compute = workerCompute(() => w);
    const job = reduce(compute, vectors(), 8, 4);
    await Promise.resolve();
    compute.close();
    compute.close();
    await expect(job).rejects.toThrow(WORKER_CLOSED);
    expect(w.terminated).toBe(1);
    await expect(reduce(compute, vectors(), 8, 4)).rejects.toThrow(WORKER_CLOSED);
    await expect(compute.force(forceRequest(buildGraph(documents(3, 1))))).rejects.toThrow(
      WORKER_CLOSED,
    );
  });
});

describe("workerCompute when no worker can be made", () => {
  const spawn = () => {
    throw new Error("worker-src blocked");
  };

  it("does the job on the page's own thread, and says so", async () => {
    const compute = workerCompute(spawn);
    const v = vectors();
    const expected = Array.from(reduceToUnit(v.slice(), 8, 4).coords);
    const result = await reduce(compute, v, 8, 4);
    expect(Array.from(result.coords)).toEqual(expected);
    // Nothing was moved anywhere.
    expect(v.byteLength).toBe(32 * 4);
    expect(compute.mode).toBe("inline");
    const forced = await compute.force(forceRequest(buildGraph(documents(5, 2))));
    expect(forced.positions).toHaveLength(7);
  });

  it("does not try to spawn again after it has fallen back", async () => {
    const attempt = vi.fn(spawn);
    const compute = workerCompute(attempt);
    await reduce(compute, vectors(), 8, 4);
    await reduce(compute, vectors(), 8, 4);
    expect(attempt).toHaveBeenCalledTimes(1);
  });

  it("still says what is wrong with data that does not add up", async () => {
    await expect(reduce(workerCompute(spawn), new Float32Array(3), 4, 3)).rejects.toThrow(
      /do not match/,
    );
  });

  it("close ends the fallback too", async () => {
    const compute = workerCompute(spawn);
    await reduce(compute, vectors(), 8, 4);
    compute.close();
    await expect(reduce(compute, vectors(), 8, 4)).rejects.toThrow(WORKER_CLOSED);
  });

  it("a worker that STARTED and then failed is not papered over by the fallback", async () => {
    const w = fakeWorker();
    w.hold = true;
    const compute = workerCompute(() => w);
    const job = reduce(compute, vectors(), 8, 4);
    await Promise.resolve();
    w.fail();
    await expect(job).rejects.toThrow(WORKER_FAILED);
    expect(compute.mode).toBe("worker");
  });
});

describe("inlineCompute", () => {
  it("runs both jobs and gives the same answers as the worker would", async () => {
    const compute = inlineCompute();
    const v = vectors();
    const expected = Array.from(reduceToUnit(v.slice(), 8, 4).coords);
    expect(Array.from((await reduce(compute, v, 8, 4)).coords)).toEqual(expected);
    const request = forceRequest(buildGraph(documents(6, 2)));
    expect((await compute.force(request)).positions).toHaveLength(request.nodes.length);
    expect(compute.mode).toBe("inline");
  });

  it("rejects after close", async () => {
    const compute = inlineCompute();
    compute.close();
    await expect(reduce(compute, vectors(), 8, 4)).rejects.toThrow(WORKER_CLOSED);
  });

  it("a force request d3-force cannot run becomes a rejection", async () => {
    const compute = inlineCompute();
    await expect(
      compute.force({ nodes: [], links: [{ source: "a", target: "b" }], width: 10, height: 10 }),
    ).rejects.toThrow();
  });
});

/** Four tight groups, seeded: enough documents (48) for t-SNE to run, small enough to run in a blink. */
function groups(): Float32Array {
  const out = new Float32Array(48 * 6);
  for (let i = 0; i < 48; i++) {
    for (let j = 0; j < 6; j++) {
      out[i * 6 + j] = (j === i % 4 ? 5 : 0) + Math.sin(i * 12.9898 + j * 78.233) * 0.2;
    }
  }
  return out;
}
const both = { pca: true, tsne: true };

describe("a semantic job: PCA, then t-SNE, in a worker of its own", () => {
  afterEach(() => vi.useRealTimers());

  it("hands PCA over as it is ready, reports progress, and settles only on the last reply", async () => {
    const w = fakeWorker();
    w.hold = true;
    const events: string[] = [];
    const compute = workerCompute(() => w);
    const job = compute.semantic(groups(), 48, 6, {
      ...both,
      onProvisional: (r) => events.push(`provisional:${r.method}`),
      onProgress: (f) => events.push(`progress:${f}`),
    });
    let settled = false;
    void job.result.then(() => (settled = true));
    await Promise.resolve();
    const { id } = w.posted[0] as { id: number };
    const deliver = async (data: unknown) => {
      w.onmessage?.({ data });
      await Promise.resolve();
      await Promise.resolve();
    };
    await deliver({ id, kind: "provisional", method: "pca", coords: new Float64Array(96) });
    await deliver({ id, kind: "progress", fraction: 0.5 });
    expect(settled).toBe(false);
    expect(events).toEqual(["provisional:pca", "progress:0.5"]);
    await deliver({ id, kind: "progress", fraction: 0.75 });
    expect(settled).toBe(false);
    await deliver({ id, kind: "semantic", method: "tsne", coords: new Float64Array(96) });
    expect(settled).toBe(true);
    await expect(job.result).resolves.toMatchObject({ method: "tsne" });
  });

  it("end to end through the real handler: PCA first, then t-SNE, and the vectors cross once", async () => {
    const w = fakeWorker();
    const compute = workerCompute(() => w);
    const v = groups();
    const direct = reduceToUnit(v.slice(), 48, 6).coords;
    const order: string[] = [];
    const job = compute.semantic(v, 48, 6, {
      ...both,
      onProvisional: (r) => {
        order.push("provisional");
        expect(Array.from(r.coords)).toEqual(Array.from(direct));
      },
      onProgress: () => order.push("progress"),
    });
    const result = await job.result;
    expect(order[0]).toBe("provisional");
    expect(order.filter((o) => o === "provisional")).toHaveLength(1);
    expect(order.filter((o) => o === "progress").length).toBeGreaterThan(5);
    expect(result.method).toBe("tsne");
    expect(result.coords).toHaveLength(96);
    // One request, one transfer: the page's view is detached once and the worker was told once.
    expect(w.posted).toHaveLength(1);
    expect(v.byteLength).toBe(0);
  });

  it("t-SNE only: no provisional reply reaches the page", async () => {
    const onProvisional = vi.fn();
    const onProgress = vi.fn();
    const compute = workerCompute(() => fakeWorker());
    const result = await compute.semantic(groups(), 48, 6, {
      pca: false,
      tsne: true,
      onProvisional,
      onProgress,
    }).result;
    expect(result.method).toBe("tsne");
    expect(onProvisional).not.toHaveBeenCalled();
    expect(onProgress).toHaveBeenCalled();
  });

  it("PCA only: the old projection of the numpy-checked kind, no callbacks", async () => {
    const onProvisional = vi.fn();
    const onProgress = vi.fn();
    const compute = workerCompute(() => fakeWorker());
    const v = groups();
    const expected = Array.from(reduceToUnit(v.slice(), 48, 6).coords);
    const result = await compute.semantic(v, 48, 6, {
      pca: true,
      tsne: false,
      onProvisional,
      onProgress,
    }).result;
    expect(result.method).toBe("pca");
    expect(Array.from(result.coords)).toEqual(expected);
    expect(onProvisional).not.toHaveBeenCalled();
    expect(onProgress).not.toHaveBeenCalled();
  });

  it("a callback that throws does not break the job", async () => {
    const compute = workerCompute(() => fakeWorker());
    const result = await compute.semantic(groups(), 48, 6, {
      ...both,
      onProvisional: () => {
        throw new Error("page bug");
      },
      onProgress: () => {
        throw new Error("page bug");
      },
    }).result;
    expect(result.method).toBe("tsne");
  });

  it("ignores a provisional reply of the wrong size, and a progress that is not a number", async () => {
    const w = fakeWorker();
    w.hold = true;
    const onProvisional = vi.fn();
    const onProgress = vi.fn();
    const compute = workerCompute(() => w);
    compute.semantic(groups(), 48, 6, { ...both, onProvisional, onProgress });
    await Promise.resolve();
    const { id } = w.posted[0] as { id: number };
    w.onmessage?.({
      data: { id, kind: "provisional", method: "pca", coords: new Float64Array(3) },
    });
    w.onmessage?.({ data: { id, kind: "progress", fraction: Number.NaN } });
    w.onmessage?.({ data: { id, kind: "progress", fraction: 7 } });
    expect(onProvisional).not.toHaveBeenCalled();
    expect(onProgress).toHaveBeenCalledTimes(1);
    expect(onProgress).toHaveBeenCalledWith(1);
  });

  it("has a worker of its own: a force job completes while a semantic job is running", async () => {
    const workers: Fake[] = [];
    const spawn = () => {
      const w = fakeWorker();
      w.hold = workers.length === 0; // the first worker made (the semantic job's) never answers
      workers.push(w);
      return w;
    };
    const compute = workerCompute(spawn);
    void compute.semantic(groups(), 48, 6, both).result.catch(() => undefined);
    const forced = compute.force(forceRequest(buildGraph(documents(4, 2))));
    const outcome = await Promise.race([
      forced.then(() => "done"),
      new Promise((resolve) => setTimeout(() => resolve("blocked"), 150)),
    ]);
    expect(outcome).toBe("done");
    expect(workers).toHaveLength(2);
  });

  describe("cancel", () => {
    it("ends the job's worker, rejects with SemanticStopped, and the next job gets a fresh worker", async () => {
      const workers: Fake[] = [];
      const spawn = () => {
        const w = fakeWorker();
        w.hold = workers.length === 0;
        workers.push(w);
        return w;
      };
      const compute = workerCompute(spawn);
      const job = compute.semantic(groups(), 48, 6, both);
      const outcome = job.result.then(
        () => null,
        (e: unknown) => e,
      );
      await Promise.resolve();
      job.cancel();
      const error = await outcome;
      expect(error).toBeInstanceOf(SemanticStopped);
      expect((error as SemanticStopped).reason).toBe("cancelled");
      expect(workers[0].terminated).toBe(1);
      // Not closed: the next job works, on a new worker.
      const next = await compute.semantic(groups(), 48, 6, { pca: true, tsne: false }).result;
      expect(next.method).toBe("pca");
      expect(workers).toHaveLength(2);
      expect(workers[1].terminated).toBe(0);
    });

    it("leaves the by-project worker alone", async () => {
      const workers: Fake[] = [];
      const compute = workerCompute(() => {
        const w = fakeWorker();
        w.hold = workers.length === 1; // the second worker (the semantic job's) never answers
        workers.push(w);
        return w;
      });
      await compute.force(forceRequest(buildGraph(documents(4, 2))));
      const job = compute.semantic(groups(), 48, 6, both);
      void job.result.catch(() => undefined);
      await Promise.resolve();
      job.cancel();
      expect(workers).toHaveLength(2);
      expect(workers[0].terminated).toBe(0);
      expect(workers[1].terminated).toBe(1);
      await compute.force(forceRequest(buildGraph(documents(4, 2))));
      expect(workers).toHaveLength(2);
    });

    it("drops a late reply: nothing is heard from a cancelled job", async () => {
      const w = fakeWorker();
      w.hold = true;
      const onProvisional = vi.fn();
      const onProgress = vi.fn();
      const compute = workerCompute(() => w);
      const job = compute.semantic(groups(), 48, 6, { ...both, onProvisional, onProgress });
      void job.result.catch(() => undefined);
      await Promise.resolve();
      const late = w.onmessage; // the handler the worker had, kept past the cancel
      const { id } = w.posted[0] as { id: number };
      job.cancel();
      late?.({ data: { id, kind: "provisional", method: "pca", coords: new Float64Array(96) } });
      late?.({ data: { id, kind: "progress", fraction: 0.5 } });
      late?.({ data: { id, kind: "semantic", method: "tsne", coords: new Float64Array(96) } });
      expect(onProvisional).not.toHaveBeenCalled();
      expect(onProgress).not.toHaveBeenCalled();
      await expect(job.result).rejects.toBeInstanceOf(SemanticStopped);
    });

    it("does nothing once the job has ended", async () => {
      const w = fakeWorker();
      const compute = workerCompute(() => w);
      const job = compute.semantic(groups(), 48, 6, both);
      const result = await job.result;
      job.cancel();
      expect(w.terminated).toBe(0);
      expect(result.method).toBe("tsne");
    });
  });

  describe("the deadline", () => {
    it("ends a t-SNE that runs past it, reporting the deadline, with its worker ended", async () => {
      vi.useFakeTimers();
      const w = fakeWorker();
      w.hold = true;
      const compute = workerCompute(() => w);
      const job = compute.semantic(groups(), 48, 6, { pca: false, tsne: true });
      const outcome = job.result.then(
        () => null,
        (e: unknown) => e,
      );
      await vi.advanceTimersByTimeAsync(SEMANTIC_DEADLINE_MS - 1);
      expect(w.terminated).toBe(0);
      await vi.advanceTimersByTimeAsync(2);
      const error = await outcome;
      expect(error).toBeInstanceOf(SemanticStopped);
      expect((error as SemanticStopped).reason).toBe("deadline");
      expect(w.terminated).toBe(1);
    });

    it("counts from the moment t-SNE starts, which with PCA first is the provisional reply", async () => {
      vi.useFakeTimers();
      const w = fakeWorker();
      w.hold = true;
      const compute = workerCompute(() => w);
      const job = compute.semantic(groups(), 48, 6, both);
      const outcome = job.result.then(
        () => null,
        (e: unknown) => e,
      );
      await vi.advanceTimersByTimeAsync(SEMANTIC_DEADLINE_MS * 2); // PCA is slow: no deadline yet
      expect(w.terminated).toBe(0);
      const { id } = w.posted[0] as { id: number };
      w.onmessage?.({
        data: { id, kind: "provisional", method: "pca", coords: new Float64Array(96) },
      });
      await vi.advanceTimersByTimeAsync(SEMANTIC_DEADLINE_MS - 1);
      expect(w.terminated).toBe(0);
      await vi.advanceTimersByTimeAsync(2);
      expect(((await outcome) as SemanticStopped).reason).toBe("deadline");
    });

    it("is cleared when the job ends, and never applies to PCA alone", async () => {
      vi.useFakeTimers();
      const done = fakeWorker();
      const compute = workerCompute(() => done);
      const first = compute.semantic(groups(), 48, 6, both).result;
      await vi.advanceTimersByTimeAsync(10);
      await first;
      expect(vi.getTimerCount()).toBe(0);
      await vi.advanceTimersByTimeAsync(SEMANTIC_DEADLINE_MS * 2);
      expect(done.terminated).toBe(0);
      const held = fakeWorker();
      held.hold = true;
      const pcaOnly = workerCompute(() => held);
      void pcaOnly.semantic(groups(), 48, 6, { pca: true, tsne: false }).result;
      await vi.advanceTimersByTimeAsync(SEMANTIC_DEADLINE_MS * 2);
      expect(held.terminated).toBe(0);
    });
  });

  it("a worker that fails ends the job with our sentence, and the next job gets a fresh worker", async () => {
    const workers: Fake[] = [];
    const compute = workerCompute(() => {
      const w = fakeWorker();
      w.hold = workers.length === 0;
      workers.push(w);
      return w;
    });
    const job = compute.semantic(groups(), 48, 6, both);
    const outcome = job.result.then(
      () => null,
      (e: unknown) => e,
    );
    await Promise.resolve();
    workers[0].fail();
    expect(((await outcome) as Error).message).toBe(WORKER_FAILED);
    await expect(compute.semantic(groups(), 48, 6, both).result).resolves.toMatchObject({
      method: "tsne",
    });
    expect(workers).toHaveLength(2);
  });

  it("close fails a running semantic job and refuses the next", async () => {
    const w = fakeWorker();
    w.hold = true;
    const compute = workerCompute(() => w);
    const job = compute.semantic(groups(), 48, 6, both);
    const outcome = job.result.then(
      () => null,
      (e: unknown) => e,
    );
    await Promise.resolve();
    compute.close();
    expect(((await outcome) as Error).message).toBe(WORKER_CLOSED);
    await expect(compute.semantic(groups(), 48, 6, both).result).rejects.toThrow(WORKER_CLOSED);
  });

  it("two jobs in a row each move their own vectors, once each", async () => {
    const w = fakeWorker();
    const compute = workerCompute(() => w);
    const a = groups();
    const b = groups();
    await compute.semantic(a, 48, 6, { pca: true, tsne: false }).result;
    await compute.semantic(b, 48, 6, both).result;
    expect(w.posted).toHaveLength(2);
    expect(a.byteLength).toBe(0);
    expect(b.byteLength).toBe(0);
  });
});

describe("a semantic job with no worker to run it", () => {
  const noWorker = () => {
    throw new Error("worker-src blocked");
  };

  it("answers PCA for a t-SNE request, labelled PCA, and never runs t-SNE", async () => {
    const compute = workerCompute(noWorker);
    const v = groups();
    const expected = Array.from(reduceToUnit(v.slice(), 48, 6).coords);
    const onProvisional = vi.fn();
    const onProgress = vi.fn();
    const result = await compute.semantic(v, 48, 6, { ...both, onProvisional, onProgress }).result;
    expect(result.method).toBe("pca");
    expect(Array.from(result.coords)).toEqual(expected);
    expect(onProvisional).not.toHaveBeenCalled();
    expect(onProgress).not.toHaveBeenCalled();
    expect(compute.mode).toBe("inline");
    // Nothing was moved: there was no worker to move it to.
    expect(v.byteLength).toBe(48 * 6 * 4);
  });

  it("does the same for a t-SNE-only request", async () => {
    const result = await workerCompute(noWorker).semantic(groups(), 48, 6, {
      pca: false,
      tsne: true,
    }).result;
    expect(result.method).toBe("pca");
  });

  it("the inline compute itself never runs t-SNE", async () => {
    const result = await inlineCompute().semantic(groups(), 48, 6, both).result;
    expect(result.method).toBe("pca");
  });

  it("can still be cancelled", async () => {
    const compute = inlineCompute();
    const job = compute.semantic(groups(), 48, 6, both);
    job.cancel();
    await expect(job.result).rejects.toBeInstanceOf(SemanticStopped);
  });
});
