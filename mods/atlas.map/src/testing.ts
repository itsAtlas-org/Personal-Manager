// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Shared by the engine's tests only (no page imports it, so the bundle never holds it): a seeded
// document generator, and the change-detector digest of a layout. The generator and the digest are
// written the same way in src/lib/mapLayout.test.ts, which took its literals from the core Map's code;
// the tests here hold those literals, so the port and the old code are held to one oracle.

import { inlineCompute, SemanticStopped, type Compute, type SemanticJobOptions } from "./compute";
import { reduceToUnit } from "./pca";
import type { MapDoc } from "./types";

/** mulberry32: a small seeded generator, so every run on every machine sees the same documents. */
export function seeded(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const PROJECTS = ["Atlas", "Beta", "Gamma", "Delta", "Epsilon", "Zeta", "Eta"];

/** `n` documents over `projects` projects (a blank project for every 11th), chunk counts 1..40. */
export function documents(n: number, projects: number): MapDoc[] {
  const rand = seeded(1234 + n);
  const docs: MapDoc[] = [];
  for (let i = 0; i < n; i++) {
    docs.push({
      id: i + 1,
      title: `Document ${i + 1}`,
      project: i % 11 === 10 ? "" : PROJECTS[Math.floor(rand() * projects)],
      chunkCount: 1 + Math.floor(rand() * 40),
      reviewed: true,
      sourceType: "vault",
      tags: [],
      importance: null,
      ingestedAt: "2026-01-01T00:00:00Z",
    });
  }
  return docs;
}

/** One document with the fields a test cares about. */
export function doc(id: number, project: string, chunkCount = 4, title = `T${id}`): MapDoc {
  return {
    id,
    title,
    project,
    chunkCount,
    reviewed: true,
    sourceType: "vault",
    tags: [],
    importance: null,
    ingestedAt: "2026-01-01T00:00:00Z",
  };
}

/** Two 32-bit FNV-1a hashes of the values rounded to 3 decimals: a change detector, not for security.
 *  Long enough that two different layouts never share one by accident. */
export function digest(values: number[]): string {
  let h1 = 0x811c9dc5;
  let h2 = 0x01000193;
  for (const v of values) {
    const s = `${v.toFixed(3)};`;
    for (let i = 0; i < s.length; i++) {
      h1 = Math.imul(h1 ^ s.charCodeAt(i), 0x01000193) >>> 0;
      h2 = Math.imul(h2 ^ s.charCodeAt(i), 0x85ebca6b) >>> 0;
    }
  }
  return `${h1.toString(16).padStart(8, "0")}${h2.toString(16).padStart(8, "0")}`;
}

export const flat = (positions: { x: number; y: number }[]) => positions.flatMap((p) => [p.x, p.y]);

/** A vector for each of `docs`, `dimension` wide: a seeded cloud with two clearly different spreads
 *  (so the top two principal axes are well separated, and the projection is the same on every run). */
export function vectorsFor(docs: MapDoc[], dimension: number, seed = 7): Map<number, Float32Array> {
  const rand = seeded(seed);
  const out = new Map<number, Float32Array>();
  for (const d of docs) {
    const v = new Float32Array(dimension);
    for (let j = 0; j < dimension; j++) {
      const scale = j === 0 ? 5 : j === 1 ? 2.5 : 0.3;
      v[j] = (rand() - 0.5) * scale;
    }
    out.set(d.id, v);
  }
  return out;
}

/** The last element (`Array.prototype.at` is newer than the mods' type-check library). */
export const last = <T>(items: readonly T[]): T | undefined => items[items.length - 1];

// ---- a Compute a test steers ---------------------------------------------------------------------

/** One `semantic` job the page posted, and the hands a test has on it. */
export interface ScriptedJob {
  pca: boolean;
  tsne: boolean;
  count: number;
  dimension: number;
  /** The vectors as posted (the real compute moves them to the worker). */
  vectors: Float32Array;
  cancelled: boolean;
  ended: boolean;
  /** The quick layout, as a worker posts it before t-SNE (only a job that asked for both). */
  provisional(): void;
  progress(fraction: number): void;
  /** End the job with the enhanced layout (a mirror image of PCA's, so a test can tell them apart). */
  finish(): void;
  /** End the job with the quick layout alone (what a PCA-only job, or a missing Worker, answers). */
  finishQuick(): void;
  fail(message?: string): void;
  /** The deadline passing. */
  deadline(): void;
}

export interface ScriptedCompute extends Compute {
  jobs: ScriptedJob[];
  /** Pretend no Worker could be made. */
  mode: "worker" | "inline";
}

/** The enhanced layout a scripted job answers: PCA mirrored left to right, so it is another picture. */
export function mirrored(coords: Float64Array): Float64Array {
  const out = new Float64Array(coords.length);
  for (let i = 0; i < coords.length; i += 2) {
    out[i] = 1 - coords[i];
    out[i + 1] = coords[i + 1];
  }
  return out;
}

/**
 * A Compute whose `semantic` jobs wait for the test: `provisional()`, `progress()`, `finish()`. With
 * `auto`, a job runs to its end by itself (a provisional first when both layouts were asked for).
 * `force` is the real inline simulation. Its PCA is the real one, so a test can compare coordinates.
 */
export function scriptedCompute(auto = false): ScriptedCompute {
  const real = inlineCompute();
  const jobs: ScriptedJob[] = [];
  const compute: ScriptedCompute = {
    jobs,
    mode: "worker",
    semantic(vectors, count, dimension, options: SemanticJobOptions) {
      const quick = reduceToUnit(vectors.slice(), count, dimension).coords;
      let settle!: {
        resolve: (r: { coords: Float64Array; method: "pca" | "tsne" }) => void;
        reject: (e: Error) => void;
      };
      const result = new Promise<{ coords: Float64Array; method: "pca" | "tsne" }>(
        (resolve, reject) => {
          settle = { resolve, reject };
        },
      );
      const job: ScriptedJob = {
        pca: options.pca,
        tsne: options.tsne,
        count,
        dimension,
        vectors,
        cancelled: false,
        ended: false,
        provisional() {
          if (job.ended || !options.pca || !options.tsne) return;
          options.onProvisional?.({ coords: quick.slice(), method: "pca" });
        },
        progress(fraction) {
          if (!job.ended) options.onProgress?.(fraction);
        },
        finish() {
          if (job.ended) return;
          job.ended = true;
          settle.resolve(
            options.tsne
              ? { coords: mirrored(quick), method: "tsne" }
              : { coords: quick.slice(), method: "pca" },
          );
        },
        finishQuick() {
          if (job.ended) return;
          job.ended = true;
          settle.resolve({ coords: quick.slice(), method: "pca" });
        },
        fail(message = "The layout worker failed.") {
          if (job.ended) return;
          job.ended = true;
          settle.reject(new Error(message));
        },
        deadline() {
          if (job.ended) return;
          job.ended = true;
          settle.reject(new SemanticStopped("deadline"));
        },
      };
      jobs.push(job);
      if (auto) {
        queueMicrotask(() => {
          job.provisional();
          job.progress(0.5);
          if (compute.mode === "inline") job.finishQuick();
          else job.finish();
        });
      }
      return {
        result,
        cancel() {
          if (job.ended) return;
          job.ended = true;
          job.cancelled = true;
          settle.reject(new SemanticStopped("cancelled"));
        },
      };
    },
    force: (request) => real.force(request),
    close: () => real.close(),
  };
  return compute;
}
