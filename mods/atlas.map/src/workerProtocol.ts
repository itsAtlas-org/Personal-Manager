// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the Map's layout Worker is asked and what it answers, and the one function that answers
// (`handle`), kept apart from the worker's own file so it runs, and is tested, with no Worker at all.
// The worker file (worker/layout.worker.ts) is two lines around `handle`.
//
// Two jobs: `reduce` (document vectors to 2-D coordinates, pca.ts) and `force` (the by-project
// simulation, forceLayout.ts). Both are CPU work that would freeze the page for seconds at a few
// thousand documents, which is why the Map has a Worker at all.
//
// A request is read as untrusted even though only this mod's own page posts it: a malformed one is an
// `error` reply, never an exception that kills the worker and strands every job waiting on it.

import { simulate, type LayoutRequest, type LayoutResponse } from "./forceLayout";
import { reduceToUnit, type Reduction } from "./pca";

export interface ReduceRequest {
  id: number;
  kind: "reduce";
  /** `count` documents of `dimension` floats, row after row. Transferred, not copied. */
  vectors: Float32Array;
  count: number;
  dimension: number;
}

export interface ForceRequest {
  id: number;
  kind: "force";
  request: LayoutRequest;
}

export type WorkerRequest = ReduceRequest | ForceRequest;

export type WorkerReply =
  | { id: number; kind: "reduce"; coords: Float64Array; method: Reduction["method"] }
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

/** Answer one request. Never throws. */
export function handle(request: unknown): WorkerReply {
  const id = idOf(request);
  if (!isRecord(request)) return fail(id, "The layout request was not understood.");
  try {
    switch (request.kind) {
      case "reduce": {
        const { vectors, count, dimension } = request;
        if (
          !isFloat32Array(vectors) ||
          typeof count !== "number" ||
          typeof dimension !== "number"
        ) {
          return fail(id, "The layout request was not understood.");
        }
        const { coords, method } = reduceToUnit(vectors, count, dimension);
        return { id, kind: "reduce", coords, method };
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
  return reply.kind === "reduce" ? [reply.coords.buffer] : [];
}
