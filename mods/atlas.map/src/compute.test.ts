// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The page's side of the Worker: jobs matched to replies, a failed worker replaced, a missing worker
// replaced by the page's own thread. The stand-in worker runs the real `handle` and moves buffers the
// way a Worker does (structuredClone with a transfer list), so "the vectors are moved, not copied" and
// "the worker's reply arrives as its own copy" are tested, not assumed.

import { describe, expect, it, vi } from "vitest";
import {
  inlineCompute,
  WORKER_CLOSED,
  WORKER_FAILED,
  workerCompute,
  type WorkerLike,
} from "./compute";
import { buildGraph, forceRequest } from "./layoutModel";
import { reduceToUnit } from "./pca";
import { documents } from "./testing";
import { handle, transferOf, type WorkerReply } from "./workerProtocol";

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
function fakeWorker(answer: (request: unknown) => WorkerReply | null = (r) => handle(r)): Fake {
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
          const reply = answer(request);
          if (reply === null || w.terminated > 0) return;
          const copy = structuredClone(reply, { transfer: transferOf(reply) });
          w.onmessage?.({ data: copy });
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

const vectors = () => {
  const v = new Float32Array(8 * 4);
  for (let i = 0; i < v.length; i++) v[i] = Math.cos(i * 0.9) * (1 + (i % 4));
  return v;
};

describe("workerCompute", () => {
  it("makes no worker until the first job, then one for all of them", async () => {
    const spawn = vi.fn(() => fakeWorker());
    const compute = workerCompute(spawn);
    expect(spawn).not.toHaveBeenCalled();
    await compute.reduce(vectors(), 8, 4);
    await compute.force(forceRequest(buildGraph(documents(5, 2))));
    expect(spawn).toHaveBeenCalledTimes(1);
    expect(compute.mode).toBe("worker");
  });

  it("reduce: the worker's projection, and the vectors' buffer is moved to it", async () => {
    const compute = workerCompute(() => fakeWorker());
    const v = vectors();
    const expected = Array.from(reduceToUnit(v.slice(), 8, 4).coords);
    const result = await compute.reduce(v, 8, 4);
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
    const a = compute.reduce(vectors(), 8, 4);
    const requestB = forceRequest(buildGraph(documents(4, 2)));
    const b = compute.force(requestB);
    await Promise.resolve();
    const [first, second] = w.posted as { id: number; kind: string }[];
    expect([first.kind, second.kind]).toEqual(["reduce", "force"]);
    expect(first.id).not.toBe(second.id);
    // The force job's reply arrives first, then the reduce job's.
    const order: string[] = [];
    void a.then(() => order.push("reduce"));
    void b.then(() => order.push("force"));
    w.release();
    const [ra, rb] = await Promise.all([a, b]);
    expect(order).toEqual(["force", "reduce"]);
    expect(ra.coords).toHaveLength(8 * 2);
    expect(rb.positions).toHaveLength(requestB.nodes.length);
  });

  it("a job's error reply rejects that job with our sentence, and the worker stays", async () => {
    const w = fakeWorker();
    const compute = workerCompute(() => w);
    await expect(compute.reduce(new Float32Array(3), 4, 3)).rejects.toThrow(/do not match/);
    expect(w.terminated).toBe(0);
    await expect(compute.reduce(vectors(), 8, 4)).resolves.toBeDefined();
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
    const a = compute.reduce(vectors(), 8, 4);
    const b = compute.force(forceRequest(buildGraph(documents(4, 2))));
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
    await expect(compute.reduce(vectors(), 8, 4)).resolves.toBeDefined();
    expect(workers).toHaveLength(2);
  });

  it("a message the worker's page cannot read (messageerror) is the same", async () => {
    const w = fakeWorker();
    w.hold = true;
    const compute = workerCompute(() => w);
    const job = compute.reduce(vectors(), 8, 4);
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
    await expect(compute.reduce(vectors(), 8, 4)).rejects.toThrow(WORKER_FAILED);
    expect(w.terminated).toBe(1);
  });

  it("an error reply that names no job of ours is a worker in an unknown state: dropped", async () => {
    const w = fakeWorker(() => ({ id: -1, kind: "error", message: "?" }));
    const compute = workerCompute(() => w);
    const job = compute.reduce(vectors(), 8, 4);
    await expect(job).rejects.toThrow(WORKER_FAILED);
    expect(w.terminated).toBe(1);
  });

  it("a reply for a job nobody is waiting for is ignored", async () => {
    const w = fakeWorker();
    w.hold = true;
    const compute = workerCompute(() => w);
    const job = compute.reduce(vectors(), 8, 4);
    await Promise.resolve();
    w.onmessage?.({
      data: { id: 999, kind: "reduce", coords: new Float64Array(16), method: "pca" },
    });
    w.onmessage?.({ data: null });
    w.release();
    await expect(job).resolves.toBeDefined();
    expect(w.terminated).toBe(0);
  });

  it("a reduce reply with the wrong number of coordinates is refused, not drawn", async () => {
    const w = fakeWorker((r) => {
      const reply = handle(r);
      return reply.kind === "reduce" ? { ...reply, coords: new Float64Array(3) } : reply;
    });
    const compute = workerCompute(() => w);
    await expect(compute.reduce(vectors(), 8, 4)).rejects.toThrow(WORKER_FAILED);
  });

  it("a reply of the wrong kind, or a force reply with the wrong number of positions, is refused", async () => {
    const wrongKind = workerCompute(() =>
      fakeWorker((r) => {
        const id = (r as { id: number }).id;
        return { id, kind: "force", positions: [] };
      }),
    );
    await expect(wrongKind.reduce(vectors(), 8, 4)).rejects.toThrow(WORKER_FAILED);
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
    const job = compute.reduce(vectors(), 8, 4);
    await job;
    expect(() => compute.close()).not.toThrow();
  });

  it("close stops the worker, fails what waits, refuses later jobs, and is idempotent", async () => {
    const w = fakeWorker();
    w.hold = true;
    const compute = workerCompute(() => w);
    const job = compute.reduce(vectors(), 8, 4);
    await Promise.resolve();
    compute.close();
    compute.close();
    await expect(job).rejects.toThrow(WORKER_CLOSED);
    expect(w.terminated).toBe(1);
    await expect(compute.reduce(vectors(), 8, 4)).rejects.toThrow(WORKER_CLOSED);
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
    const result = await compute.reduce(v, 8, 4);
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
    await compute.reduce(vectors(), 8, 4);
    await compute.reduce(vectors(), 8, 4);
    expect(attempt).toHaveBeenCalledTimes(1);
  });

  it("still says what is wrong with data that does not add up", async () => {
    await expect(workerCompute(spawn).reduce(new Float32Array(3), 4, 3)).rejects.toThrow(
      /do not match/,
    );
  });

  it("close ends the fallback too", async () => {
    const compute = workerCompute(spawn);
    await compute.reduce(vectors(), 8, 4);
    compute.close();
    await expect(compute.reduce(vectors(), 8, 4)).rejects.toThrow(WORKER_CLOSED);
  });

  it("a worker that STARTED and then failed is not papered over by the fallback", async () => {
    const w = fakeWorker();
    w.hold = true;
    const compute = workerCompute(() => w);
    const job = compute.reduce(vectors(), 8, 4);
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
    expect(Array.from((await compute.reduce(v, 8, 4)).coords)).toEqual(expected);
    const request = forceRequest(buildGraph(documents(6, 2)));
    expect((await compute.force(request)).positions).toHaveLength(request.nodes.length);
    expect(compute.mode).toBe("inline");
  });

  it("rejects after close", async () => {
    const compute = inlineCompute();
    compute.close();
    await expect(compute.reduce(vectors(), 8, 4)).rejects.toThrow(WORKER_CLOSED);
  });

  it("a force request d3-force cannot run becomes a rejection", async () => {
    const compute = inlineCompute();
    await expect(
      compute.force({ nodes: [], links: [{ source: "a", target: "b" }], width: 10, height: 10 }),
    ).rejects.toThrow();
  });
});
