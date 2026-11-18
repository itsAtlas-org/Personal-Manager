// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The page's side of the layout Worker: `Compute`, the two jobs the engine needs done off the page's
// thread. The engine is written against this interface, so its tests (and the frame's DOM tests) need
// no Worker, and the page chooses how the jobs run:
//  - `workerCompute(spawn)`: in Workers built by the page (src/kit's spawnWorker over the worker's
//    source text), made on first use, jobs matched to replies by id. The by-project `force` job has
//    one worker; the `semantic` job (PCA, then t-SNE, which can run for many seconds) has its own, so a
//    running t-SNE never holds up a `force` job. A worker that errors ends with every job waiting on it
//    rejected, and is replaced by a fresh one on the next job.
//  - `inlineCompute()`: the same jobs on the page's own thread, except that t-SNE is never run there:
//    on some engines the frame shares the main thread, and a ten-second run would trip PM's
//    frozen-page watchdog (the mod would be paused and PM reloaded). A `semantic` job answers with PCA.
//    The core Map fell back to the page's thread when a Worker could not be made, and so does
//    `workerCompute` (only when `spawn` throws, never after a worker has started and failed): a plain
//    map is better than no map. The mode is visible, so a test (and a developer) can tell.

import type { LayoutRequest, LayoutResponse } from "./forceLayout";
import {
  handle,
  type Interim,
  type SemanticMethod,
  type WorkerReply,
  type WorkerRequest,
} from "./workerProtocol";

/** What a `semantic` job is asked for: at least one of the two layouts. */
export interface SemanticJobOptions {
  /** Work out the PCA layout. With `tsne` too, it is handed to `onProvisional` as soon as it is ready. */
  pca: boolean;
  /** Work out the enhanced (t-SNE) layout. Only a Worker does: the inline fallback answers PCA. */
  tsne: boolean;
  /** PCA, ready while t-SNE is still running. The job is not finished. */
  onProvisional?: (reduction: { coords: Float64Array; method: "pca" }) => void;
  /** The share of t-SNE done, 0 to 1, every 25 of its 750 steps. */
  onProgress?: (fraction: number) => void;
}

/** What a finished `semantic` job made: the last layout it was asked for. `method` says which. */
export interface SemanticReduction {
  coords: Float64Array;
  method: SemanticMethod;
}

/** A running `semantic` job. */
export interface SemanticJob {
  /** The final layout, or a rejection: {@link SemanticStopped} when it was cancelled or ran out of time. */
  result: Promise<SemanticReduction>;
  /** End the job now: its worker is stopped (the vectors die with it), whatever it had not posted is
   *  lost, and `result` rejects with `SemanticStopped("cancelled")`. The next job gets a fresh worker.
   *  Does nothing once the job has ended. */
  cancel(): void;
}

export interface Compute {
  /** One job for PCA and/or t-SNE of `count` vectors of `dimension` floats, to [0,1]^2 coordinates. The
   *  buffer under `vectors` is moved to the worker, once: the caller must not read it afterwards. */
  semantic(
    vectors: Float32Array,
    count: number,
    dimension: number,
    options: SemanticJobOptions,
  ): SemanticJob;
  /** The by-project force simulation. */
  force(request: LayoutRequest): Promise<LayoutResponse>;
  /** Stop the workers and fail every job still waiting. Idempotent. */
  close(): void;
  readonly mode: "worker" | "inline";
}

/** An event handler typed the way DOM handlers are (parameter bivariance), so a real `Worker`, whose
 *  `onmessage` takes a `MessageEvent`, is a `WorkerLike` as well as a test's stand-in taking `{data}`. */
type Handler<E> = { bivarianceHack(event: E): void }["bivarianceHack"];

/** The part of a Worker `workerCompute` uses (a real Worker, or a test's stand-in). */
export interface WorkerLike {
  postMessage(message: unknown, transfer: Transferable[]): void;
  terminate(): void;
  onmessage: Handler<{ data: unknown }> | null;
  onerror: Handler<unknown> | null;
  onmessageerror: Handler<unknown> | null;
}

/** What a failed job says: one sentence of ours, never the worker's own text. */
export const WORKER_FAILED = "The layout worker failed.";
export const WORKER_CLOSED = "The layout was stopped.";

/** How long t-SNE may run, from the moment it starts: the Linux numbers are 1 to 10 seconds up to 5,000
 *  documents, so two minutes is a slow machine at the top of the range, and past it the person is
 *  better served by the PCA map and a way to try again. */
export const SEMANTIC_DEADLINE_MS = 120_000;

/** A `semantic` job that was ended by the page, not by a failure. */
export class SemanticStopped extends Error {
  constructor(readonly reason: "cancelled" | "deadline") {
    super(reason === "deadline" ? "The enhanced layout took too long." : WORKER_CLOSED);
    this.name = "SemanticStopped";
  }
}

type Answer = WorkerReply;

function semanticFrom(reply: Answer, count: number): SemanticReduction {
  if (reply.kind === "error") throw new Error(reply.message);
  if (reply.kind !== "semantic" || !(reply.coords instanceof Float64Array)) {
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

/** Both jobs on the page's own thread. A `semantic` job is PCA whatever it asks for (see the top). */
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
    semantic(vectors, count, dimension) {
      let stopped = false;
      const done = live(() =>
        semanticFrom(
          handle({ id: 0, kind: "semantic", vectors, count, dimension, pca: true, tsne: false }),
          count,
        ),
      );
      return {
        result: done.then((r) => {
          if (stopped) throw new SemanticStopped("cancelled");
          return r;
        }),
        cancel: () => {
          stopped = true;
        },
      };
    },
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
  /** Replies that come before the last one; the job stays waiting. */
  interim?: (reply: Interim) => void;
}

/** One Worker, made on first use, and the jobs waiting on it. */
function channel(spawn: () => WorkerLike, isClosed: () => boolean) {
  let worker: WorkerLike | null = null;
  const waiting = new Map<number, Waiting>();

  const settleAll = (error: Error) => {
    const all = Array.from(waiting.values());
    waiting.clear();
    for (const w of all) w.reject(error);
  };

  const drop = (error: Error) => {
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
          drop(new Error(WORKER_FAILED));
        }
        return;
      }
      if (reply && (reply.kind === "provisional" || reply.kind === "progress")) {
        // Not the job's answer: pass it on and keep waiting.
        job.interim?.(reply);
        return;
      }
      waiting.delete(id as number);
      job.resolve(reply as Answer);
    };
    made.onerror = () => drop(new Error(WORKER_FAILED));
    made.onmessageerror = () => drop(new Error(WORKER_FAILED));
    worker = made;
    return made;
  };

  const run = (
    request: WorkerRequest,
    transfer: Transferable[],
    interim?: (reply: Interim) => void,
  ): Promise<Answer> => {
    if (isClosed()) return Promise.reject(new Error(WORKER_CLOSED));
    const w = start();
    if (!w) return Promise.reject(new NoWorker());
    return new Promise<Answer>((resolve, reject) => {
      waiting.set(request.id, { resolve, reject, interim });
      try {
        w.postMessage(request, transfer);
      } catch {
        waiting.delete(request.id);
        drop(new Error(WORKER_FAILED));
        reject(new Error(WORKER_FAILED));
      }
    });
  };

  /** Take one job off the line without touching the worker (its late replies are then ignored). */
  const forget = (id: number) => {
    waiting.delete(id);
  };

  return { run, drop, forget };
}

/** The two jobs in Workers made by `spawn`, falling back to the page's thread if one cannot be made. */
export function workerCompute(spawn: () => WorkerLike): Compute {
  let closed = false;
  let inline: Compute | null = null;
  let nextId = 1;
  const isClosed = () => closed;
  const shared = channel(spawn, isClosed);
  // The semantic job's own worker: a t-SNE that runs for ten seconds must not hold up a `force` job.
  const semantic = channel(spawn, isClosed);

  const fallback = (): Compute => (inline ??= inlineCompute());

  return {
    get mode() {
      return inline ? "inline" : "worker";
    },
    semantic(vectors, count, dimension, options) {
      if (inline) return inline.semantic(vectors, count, dimension, options);
      const { pca, tsne, onProvisional, onProgress } = options;
      const id = nextId++;
      let ended = false;
      let deadline: ReturnType<typeof setTimeout> | undefined;
      let reject: (error: Error) => void = () => undefined;
      let onPage: SemanticJob | null = null;

      const end = (error: Error | null, then: () => void) => {
        if (ended) return;
        ended = true;
        clearTimeout(deadline);
        then();
        if (error) reject(error);
      };
      /** Stop the job: nothing it posts later is heard, its worker is ended, and the page hears why. */
      const stop = (reason: "cancelled" | "deadline") =>
        end(new SemanticStopped(reason), () => {
          onPage?.cancel();
          semantic.forget(id);
          semantic.drop(new Error(WORKER_FAILED));
        });
      const startDeadline = () => {
        if (!tsne || ended) return;
        deadline = setTimeout(() => stop("deadline"), SEMANTIC_DEADLINE_MS);
      };
      // t-SNE starts when the job is posted, or, with PCA first, when PCA's reply comes back.
      const interim = (reply: Interim) => {
        if (ended) return;
        try {
          if (reply.kind === "provisional") {
            if (
              !pca ||
              !tsne ||
              !(reply.coords instanceof Float64Array) ||
              reply.coords.length !== 2 * count
            ) {
              return;
            }
            startDeadline();
            onProvisional?.({ coords: reply.coords, method: "pca" });
          } else if (typeof reply.fraction === "number" && Number.isFinite(reply.fraction)) {
            onProgress?.(Math.min(1, Math.max(0, reply.fraction)));
          }
        } catch {
          // A callback of the page's own is not allowed to break the worker's message handling.
        }
      };

      const result = new Promise<SemanticReduction>((resolve, rej) => {
        reject = rej;
        const request: WorkerRequest = {
          id,
          kind: "semantic",
          vectors,
          count,
          dimension,
          pca,
          tsne,
        };
        semantic.run(request, [vectors.buffer], interim).then(
          (reply) =>
            end(null, () => {
              try {
                resolve(semanticFrom(reply, count));
              } catch (error) {
                rej(error as Error);
              }
            }),
          (error: unknown) => {
            if (error instanceof NoWorker) {
              // No worker can be made here: PCA on the page's own thread, and never t-SNE.
              onPage = fallback().semantic(vectors, count, dimension, options);
              onPage.result.then(
                (r) => end(null, () => resolve(r)),
                (e: unknown) => end(e as Error, () => undefined),
              );
              return;
            }
            end(error as Error, () => undefined);
          },
        );
        // With no PCA first, t-SNE starts now.
        if (!pca) startDeadline();
      });
      return { result, cancel: () => stop("cancelled") };
    },
    async force(request) {
      if (!inline) {
        const id = nextId++;
        try {
          return forceFrom(
            await shared.run({ id, kind: "force", request }, []),
            request.nodes.length,
          );
        } catch (error) {
          if (!(error instanceof NoWorker)) throw error;
        }
      }
      return fallback().force(request);
    },
    close() {
      closed = true;
      const stopped = new Error(WORKER_CLOSED);
      shared.drop(stopped);
      semantic.drop(stopped);
      inline?.close();
    },
  };
}
