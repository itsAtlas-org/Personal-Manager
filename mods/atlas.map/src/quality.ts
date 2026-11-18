// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// How good is a 2-D picture of documents? Three scores, test support only: the page never imports
// this file, and the mod's bundle does not contain it. They are the scores the openTSNE fixture
// (fixtures/tsne-opentsne.json) records, defined the same way, so a picture made here can be held
// against a picture made by openTSNE and scikit-learn, number against number:
//
//  - trustworthiness: scikit-learn's `sklearn.manifold.trustworthiness(X, Y, n_neighbors=k,
//    metric="cosine")`. How rarely a document that is NOT among a document's k closest in the
//    original space shows up among its k closest in the picture, weighted by how far off it was.
//  - neighbour recall: of a document's k closest in the original space, the share that are also its
//    k closest in the picture, averaged over documents.
//  - label purity: of a document's k closest in the picture, the share with the same label as the
//    document, averaged over documents.
//
// The original space uses cosine distance on the vectors as given (no centring); the picture uses
// euclidean distance. A document is never its own neighbour. Ties go to the lower index, like a
// stable sort. The cost is O(n^2 log n): it is a test tool, run on up to a few thousand documents.

export interface Scores {
  trustworthiness: number;
  recall: number;
  purity: number;
}

/** scikit-learn's own limit: trustworthiness needs k < n / 2. */
export function scoreNeighbours(n: number, wanted = 10): number {
  return Math.min(wanted, Math.ceil(n / 2) - 1);
}

/** Cosine distance between every pair, row-major n x n, clipped to [0, 2] like scikit-learn's. */
function cosineDistances(x: Float64Array, n: number, d: number): Float64Array {
  const unit = new Float64Array(n * d);
  for (let i = 0; i < n; i++) {
    let sum = 0;
    for (let j = 0; j < d; j++) sum += x[i * d + j] * x[i * d + j];
    const norm = Math.sqrt(sum) || 1;
    for (let j = 0; j < d; j++) unit[i * d + j] = x[i * d + j] / norm;
  }
  const out = new Float64Array(n * n);
  for (let i = 0; i < n; i++) {
    for (let j = i + 1; j < n; j++) {
      let dot = 0;
      for (let c = 0; c < d; c++) dot += unit[i * d + c] * unit[j * d + c];
      const dist = Math.min(2, Math.max(0, 1 - dot));
      out[i * n + j] = dist;
      out[j * n + i] = dist;
    }
  }
  return out;
}

/** Squared euclidean distance between every pair of the picture's points, row-major n x n. */
function pictureDistances(y: Float64Array, n: number): Float64Array {
  const out = new Float64Array(n * n);
  for (let i = 0; i < n; i++) {
    for (let j = i + 1; j < n; j++) {
      const dx = y[2 * i] - y[2 * j];
      const dy = y[2 * i + 1] - y[2 * j + 1];
      const dist = dx * dx + dy * dy;
      out[i * n + j] = dist;
      out[j * n + i] = dist;
    }
  }
  return out;
}

/** Every other document, nearest first (ties to the lower index). */
function order(dist: Float64Array, n: number, i: number): Int32Array {
  const ids = new Int32Array(n - 1);
  let at = 0;
  for (let j = 0; j < n; j++) if (j !== i) ids[at++] = j;
  const row = i * n;
  return ids.sort((a, b) => dist[row + a] - dist[row + b] || a - b);
}

/**
 * The three scores of a picture `y` (`x0, y0, x1, y1, ...`) of `n` documents whose vectors are `x`
 * (row-major, `d` columns). `labels` is one integer per document. `k` defaults to
 * {@link scoreNeighbours}.
 */
export function scorePicture(
  x: ArrayLike<number>,
  y: ArrayLike<number>,
  labels: ArrayLike<number>,
  n: number,
  d: number,
  k = scoreNeighbours(n),
): Scores {
  if (!(k >= 1) || k >= n / 2)
    throw new Error(`k must be at least 1 and less than n / 2 (n = ${n}, k = ${k})`);
  const dx = cosineDistances(Float64Array.from(x), n, d);
  const dy = pictureDistances(Float64Array.from(y), n);
  const rank = new Int32Array(n);
  let penalty = 0;
  let recall = 0;
  let purity = 0;
  for (let i = 0; i < n; i++) {
    const original = order(dx, n, i);
    const picture = order(dy, n, i);
    // rank[j] = 1-based place of j among i's neighbours in the original space.
    for (let r = 0; r < n - 1; r++) rank[original[r]] = r + 1;
    const near = new Set<number>();
    for (let r = 0; r < k; r++) near.add(original[r]);
    let shared = 0;
    let same = 0;
    for (let r = 0; r < k; r++) {
      const j = picture[r];
      if (near.has(j)) shared++;
      if (labels[j] === labels[i]) same++;
      if (rank[j] > k) penalty += rank[j] - k;
    }
    recall += shared / k;
    purity += same / k;
  }
  return {
    trustworthiness: 1 - penalty * (2 / (n * k * (2 * n - 3 * k - 1))),
    recall: recall / n,
    purity: purity / n,
  };
}
