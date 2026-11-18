// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// fixtures/tsne-opentsne.json, decoded: test support only (the page never imports it). The file holds,
// per case, vectors and openTSNE's coordinates as base64 little-endian float32 (exact and small).

import fixture from "./fixtures/tsne-opentsne.json";
import type { Scores } from "./quality";

export interface TsneCase {
  name: string;
  note: string;
  n: number;
  dimension: number;
  method: string;
  labels: number[];
  /** `n * dimension` numbers, row-major. */
  vectors: Float32Array;
  /** openTSNE's picture, `x0, y0, x1, y1, ...`, in [0,1]. */
  coords: Float32Array;
  /** Whether the sidecar reduced to 50 axes before t-SNE (dimension above 50). */
  pcaStep: boolean;
  requestedPerplexity: number;
  effectivePerplexity: number;
  neighbours: number;
  scoreNeighbours: number;
  scores: { tsne: Scores; pca: Scores };
}

export function decodeFloat32(base64: string): Float32Array {
  const raw = atob(base64);
  const bytes = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
  const view = new DataView(bytes.buffer);
  const out = new Float32Array(raw.length / 4);
  for (let i = 0; i < out.length; i++) out[i] = view.getFloat32(4 * i, true);
  return out;
}

export const tsneFixture = fixture;

export const tsneCases: TsneCase[] = fixture.cases.map((c) => ({
  name: c.name,
  note: c.note,
  n: c.n,
  dimension: c.dimension,
  method: c.method,
  labels: c.labels,
  vectors: decodeFloat32(c.vectors),
  coords: decodeFloat32(c.coords),
  pcaStep: c.pcaStep,
  requestedPerplexity: c.requestedPerplexity,
  effectivePerplexity: c.effectivePerplexity,
  neighbours: c.neighbours,
  scoreNeighbours: c.scoreNeighbours,
  scores: c.scores,
}));
