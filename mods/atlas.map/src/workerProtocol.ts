// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the Map's layout Worker is asked and what it answers, and the one function that answers
// (`handle`), kept apart from the worker's own file so it runs, and is tested, with no Worker at all.
// The worker file (worker/layout.worker.ts) is two lines around `handle`.
//
// Two jobs: `semantic` (document vectors to 2-D coordinates: PCA, pca.ts, then the enhanced t-SNE,
// tsne.ts) and `force` (the by-project simulation, forceLayout.ts). Both are CPU work that would freeze
// the page for seconds at a few thousand documents, which is why the Map has a Worker at all.
//
// A `semantic` job is ONE job that keeps the vectors: the page moves them to the worker once, the worker
// answers with PCA as a `provisional` reply that does not end the job, reports `progress` while t-SNE
// runs on the vectors it still holds, and ends the job with the `semantic` reply. (Moving the vectors
// twice would mean reading them from PM twice, and the two reads could describe different documents.)
// Cancelling is the page ending that worker, so the worker has nothing to check.
//
// A request is read as untrusted even though only this mod's own page posts it: a malformed one is an
// `error` reply, never an exception that kills the worker and strands every job waiting on it.

import { simulate, type LayoutRequest, type LayoutResponse } from "./forceLayout";
import { reduceToUnit, type Reduction } from "./pca";
import { tsne } from "./tsne";

export interface SemanticRequest {
  id: number;
  kind: "semantic";
  /** `count` documents of `dimension` floats, row after row. Transferred, not copied. */
  vectors: Float32Array;
  count: number;
  dimension: number;
  /** Work out the PCA layout. With `tsne` as well it is posted as a provisional reply. */
  pca: boolean;
  /** Work out the enhanced (t-SNE) layout, on the vectors the worker still holds. */
  tsne: boolean;
}

/** What a finished `semantic` job's coordinates are: pca.ts's routes, or "tsne". */
export type SemanticMethod = Reduction["method"] | "tsne";

export interface ForceRequest {
  id: number;
  kind: "force";
  request: LayoutRequest;
}

export type WorkerRequest = SemanticRequest | ForceRequest;

export type WorkerReply =
  | { id: number; kind: "provisional"; method: "pca"; coords: Float64Array }
  | { id: number; kind: "progress"; fraction: number }
  | { id: number; kind: "semantic"; coords: Float64Array; method: SemanticMethod }
  | { id: number; kind: "force"; positions: LayoutResponse["positions"] }
  | { id: number; kind: "error"; message: string };

/** An id the worker could not read out of a request, so its reply cannot name a job. */
export const NO_JOB = -1;

const isRecord = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null;

/** A Float32Array by what it is, not by which realm's constructor made it: the frame SDK checks the
 *  vectors PM sends the same way (src/kit/host.ts), and a message port can hand back an array whose
 *  constructor is another realm's (a test's jsdom page over Node's channel does), which `instanceof`
 *  would refuse although it is exactly the view the SDK vouched for. */
const isFloat32Array = (v: unknown): v is Float32Array =>
  ArrayBuffer.isView(v) && Object.prototype.toString.call(v) === "[object Float32Array]";

/** A job's id, if the request has a usable one. */
function idOf(request: unknown): number {
  return isRecord(request) && Number.isSafeInteger(request.id) ? (request.id as number) : NO_JOB;
}

function fail(id: number, message: string): WorkerReply {
  return { id, kind: "error", message };
}

function readForce(request: unknown): LayoutRequest | null {
  if (!isRecord(request)) return null;
  const { nodes, links, width, height } = request;
  if (!Array.isArray(nodes) || !Array.isArray(links)) return null;
  if (typeof width !== "number" || typeof height !== "number") return null;
  if (!Number.isFinite(width) || !Number.isFinite(height) || width <= 0 || height <= 0) return null;
  for (const n of nodes) {
    if (!isRecord(n) || typeof n.id !== "string") return null;
    if (typeof n.radius !== "number" || !Number.isFinite(n.radius)) return null;
  }
  for (const l of links) {
    if (!isRecord(l) || typeof l.source !== "string" || typeof l.target !== "string") return null;
  }
  // A link to a node that is not there makes d3-force throw ("node not found"): refuse it here, with
  // the same one sentence, rather than let the throw be the answer.
  const ids = new Set((nodes as { id: string }[]).map((n) => n.id));
  for (const l of links as { source: string; target: string }[]) {
    if (!ids.has(l.source) || !ids.has(l.target)) return null;
  }
  return request as unknown as LayoutRequest;
}

/** Replies that come before a job's last one (`provisional`, `progress`) and leave it waiting. */
export type Interim = Extract<WorkerReply, { kind: "provisional" | "progress" }>;

/** Answer one request: the reply that ends the job. A `semantic` job with PCA and t-SNE first hands
 *  `emit` its provisional PCA reply and its progress (so the real worker can post them while it keeps
 *  working); with no `emit` they are dropped. Never throws. */
export function handle(request: unknown, emit?: (reply: Interim) => void): WorkerReply {
  const id = idOf(request);
  if (!isRecord(request)) return fail(id, "The layout request was not understood.");
  try {
    switch (request.kind) {
      case "semantic": {
        const { vectors, count, dimension, pca, tsne: wantTsne } = request;
        if (
          !isFloat32Array(vectors) ||
          typeof count !== "number" ||
          typeof dimension !== "number" ||
          typeof pca !== "boolean" ||
          typeof wantTsne !== "boolean" ||
          (!pca && !wantTsne)
        ) {
          return fail(id, "The layout request was not understood.");
        }
        if (!wantTsne) {
          const { coords, method } = reduceToUnit(vectors, count, dimension);
          return { id, kind: "semantic", coords, method };
        }
        if (pca) {
          const first = reduceToUnit(vectors, count, dimension);
          // Three documents or fewer, or none: there is no t-SNE of them, so PCA's answer is the answer.
          if (first.method !== "pca") {
            return { id, kind: "semantic", coords: first.coords, method: first.method };
          }
          emit?.({ id, kind: "provisional", method: "pca", coords: first.coords });
        }
        const { coords, method } = tsne(vectors, count, dimension, {
          onProgress: (fraction) => emit?.({ id, kind: "progress", fraction }),
        });
        return { id, kind: "semantic", coords, method };
      }
      case "force": {
        const layout = readForce(request.request);
        if (!layout) return fail(id, "The layout request was not understood.");
        return { id, kind: "force", positions: simulate(layout).positions };
      }
      default:
        return fail(id, "The layout request was not understood.");
    }
  } catch (error) {
    // Our own words (pca.ts says what was wrong with the data), nothing from a document.
    return fail(id, error instanceof Error ? error.message : "The layout failed.");
  }
}

/** The buffers of a reply that move to the page instead of being copied. */
export function transferOf(reply: WorkerReply): Transferable[] {
  return reply.kind === "semantic" || reply.kind === "provisional" ? [reply.coords.buffer] : [];
}
