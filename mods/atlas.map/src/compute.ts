// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The page's side of the layout Worker: `Compute`, the two jobs the engine needs done off the page's
// thread, behind a promise each. The engine is written against this interface, so its tests (and the
// frame's DOM tests) need no Worker, and the page chooses how the jobs run:
//  - `workerCompute(spawn)`: in a Worker built by the page (src/kit's spawnWorker over the worker's
//    source text). One worker, made on first use, jobs matched to replies by id. A worker that errors
//    ends with every job waiting on it rejected, and is replaced by a fresh one on the next job.
//  - `inlineCompute()`: the same two functions on the page's own thread. The core Map fell back to
//    this when a Worker could not be made, and so does `workerCompute` (only when `spawn` throws, never
//    after a worker has started and failed): a map that freezes the page for a second is better than
//    no map. The mode is visible, so a test (and a developer) can tell.

import type { LayoutRequest, LayoutResponse } from "./forceLayout";
import type { Reduction } from "./pca";
import { handle, type WorkerReply, type WorkerRequest } from "./workerProtocol";

export interface Compute {
  /** Project `count` vectors of `dimension` floats to [0,1]^2 coordinates. The buffer under `vectors`
   *  is moved to the worker: the caller must not read it afterwards. */
  reduce(vectors: Float32Array, count: number, dimension: number): Promise<Reduction>;
  /** The by-project force simulation. */
  force(request: LayoutRequest): Promise<LayoutResponse>;
  /** Stop the worker and fail every job still waiting. Idempotent. */
  close(): void;
  readonly mode: "worker" | "inline";
}

/** The part of a Worker `workerCompute` uses (a real Worker, or a test's stand-in). */
export interface WorkerLike {
  postMessage(message: unknown, transfer: Transferable[]): void;
  terminate(): void;
  onmessage: ((event: { data: unknown }) => void) | null;
  onerror: ((event: unknown) => void) | null;
  onmessageerror: ((event: unknown) => void) | null;
}

/** What a failed job says: one sentence of ours, never the worker's own text. */
export const WORKER_FAILED = "The layout worker failed.";
export const WORKER_CLOSED = "The layout was stopped.";

type Answer = WorkerReply;

function reduceFrom(reply: Answer, count: number): Reduction {
  if (reply.kind === "error") throw new Error(reply.message);
  if (reply.kind !== "reduce" || !(reply.coords instanceof Float64Array)) {
    throw new Error(WORKER_FAILED);
  }
  // The reply is trusted to be this mod's own worker, but a count that does not match is a bug that
  // would draw documents at the wrong place; it is not allowed to become a map.
  if (reply.coords.length !== 2 * count) throw new Error(WORKER_FAILED);
  return { coords: reply.coords, method: reply.method };
}

function forceFrom(reply: Answer, nodes: number): LayoutResponse {
  if (reply.kind === "error") throw new Error(reply.message);
  if (
    reply.kind !== "force" ||
    !Array.isArray(reply.positions) ||
    reply.positions.length !== nodes
  ) {
    throw new Error(WORKER_FAILED);
  }
  return { positions: reply.positions };
}

/** Both jobs on the page's own thread. */
export function inlineCompute(): Compute {
  let closed = false;
  const live = <T>(run: () => T): Promise<T> => {
    if (closed) return Promise.reject(new Error(WORKER_CLOSED));
    try {
      return Promise.resolve(run());
    } catch (error) {
      return Promise.reject(error instanceof Error ? error : new Error(WORKER_FAILED));
    }
  };
  return {
    mode: "inline",
    reduce: (vectors, count, dimension) =>
      live(() => reduceFrom(handle({ id: 0, kind: "reduce", vectors, count, dimension }), count)),
    force: (request) =>
      live(() => forceFrom(handle({ id: 0, kind: "force", request }), request.nodes.length)),
    close: () => {
      closed = true;
    },
  };
}

/** `spawn` threw: no worker can be made here (not a job that failed). */
class NoWorker extends Error {}

interface Waiting {
  resolve: (reply: Answer) => void;
  reject: (error: Error) => void;
}

/** The two jobs in a Worker made by `spawn`, falling back to the page's thread if it cannot be made. */
export function workerCompute(spawn: () => WorkerLike): Compute {
  let worker: WorkerLike | null = null;
  let closed = false;
  let inline: Compute | null = null;
  let nextId = 1;
  const waiting = new Map<number, Waiting>();

  const settleAll = (error: Error) => {
    const all = Array.from(waiting.values());
    waiting.clear();
    for (const w of all) w.reject(error);
  };

  const dropWorker = (error: Error) => {
    const dead = worker;
    worker = null;
    if (dead) {
      dead.onmessage = null;
      dead.onerror = null;
      dead.onmessageerror = null;
      try {
        dead.terminate();
      } catch {
        // Already gone.
      }
    }
    settleAll(error);
  };

  const start = (): WorkerLike | null => {
    if (worker) return worker;
    let made: WorkerLike;
    try {
      made = spawn();
    } catch {
      return null;
    }
    made.onmessage = (event) => {
      const reply = event.data as Answer | null;
      const id = typeof reply === "object" && reply !== null ? reply.id : undefined;
      const job = id === undefined ? undefined : waiting.get(id);
      if (!job) {
        // An error with no job to blame (a request the worker could not read) cannot be matched: the
        // page only ever posts well-formed requests, so this is a worker in an unknown state.
        if (reply && typeof reply === "object" && reply.kind === "error") {
          dropWorker(new Error(WORKER_FAILED));
        }
        return;
      }
      waiting.delete(id as number);
      job.resolve(reply as Answer);
    };
    made.onerror = () => dropWorker(new Error(WORKER_FAILED));
    made.onmessageerror = () => dropWorker(new Error(WORKER_FAILED));
    worker = made;
    return made;
  };

  const run = (request: WorkerRequest, transfer: Transferable[]): Promise<Answer> => {
    if (closed) return Promise.reject(new Error(WORKER_CLOSED));
    const w = start();
    if (!w) return Promise.reject(new NoWorker());
    return new Promise<Answer>((resolve, reject) => {
      waiting.set(request.id, { resolve, reject });
      try {
        w.postMessage(request, transfer);
      } catch {
        waiting.delete(request.id);
        dropWorker(new Error(WORKER_FAILED));
        reject(new Error(WORKER_FAILED));
      }
    });
  };

  const fallback = (): Compute => (inline ??= inlineCompute());

  return {
    get mode() {
      return inline ? "inline" : "worker";
    },
    async reduce(vectors, count, dimension) {
      if (!inline) {
        const id = nextId++;
        try {
          const request: WorkerRequest = { id, kind: "reduce", vectors, count, dimension };
          return reduceFrom(await run(request, [vectors.buffer]), count);
        } catch (error) {
          if (!(error instanceof NoWorker)) throw error;
        }
      }
      return fallback().reduce(vectors, count, dimension);
    },
    async force(request) {
      if (!inline) {
        const id = nextId++;
        try {
          return forceFrom(await run({ id, kind: "force", request }, []), request.nodes.length);
        } catch (error) {
          if (!(error instanceof NoWorker)) throw error;
        }
      }
      return fallback().force(request);
    },
    close() {
      closed = true;
      dropWorker(new Error(WORKER_CLOSED));
      inline?.close();
    },
  };
}
