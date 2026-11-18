// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the benchmark runs (tsne.bench.ts under node, and the same file bundled for JavaScriptCore): seeded,
// clustered, overlapping input of a given size, and a timed run of the whole enhanced layout over it.
// Benchmark support only: the page never imports this file.

import { seededRandom } from "./pca";
import { tsne } from "./tsne";

/** Roughly normal numbers (a sum of four uniforms, rescaled) from a seeded uniform generator. */
function normal(rand: () => number): number {
  return (rand() + rand() + rand() + rand() - 2) * Math.sqrt(3);
}

/**
 * `count` unit-length vectors of `dimension` floats in `clusters` overlapping clusters: a centre per
 * cluster, a document is its cluster's centre plus noise of the same size, so neighbouring clusters
 * blur into each other as real libraries do. The same call gives the same numbers on every run.
 */
export function clusteredVectors(count: number, dimension: number, clusters = 20): Float32Array {
  const rand = seededRandom(20261118);
  const centres = new Float64Array(clusters * dimension);
  for (let i = 0; i < centres.length; i++) centres[i] = normal(rand);
  const out = new Float32Array(count * dimension);
  const row = new Float64Array(dimension);
  for (let i = 0; i < count; i++) {
    const c = (i % clusters) * dimension;
    let sum = 0;
    for (let j = 0; j < dimension; j++) {
      row[j] = centres[c + j] + normal(rand);
      sum += row[j] * row[j];
    }
    const len = Math.sqrt(sum);
    for (let j = 0; j < dimension; j++) out[i * dimension + j] = row[j] / len;
  }
  return out;
}

export interface Timing {
  count: number;
  dimension: number;
  /** Wall-clock milliseconds for one whole layout (PCA-50, neighbours, affinities, 750 steps). */
  milliseconds: number;
}

/** One timed layout of `count` x `dimension` seeded vectors. `now` is the engine's clock. */
export function measure(count: number, dimension: number, now: () => number): Timing {
  const vectors = clusteredVectors(count, dimension);
  const start = now();
  const result = tsne(vectors, count, dimension);
  const milliseconds = now() - start;
  if (result.method !== "tsne" || result.coords.length !== 2 * count) throw new Error("bad run");
  return { count, dimension, milliseconds };
}
