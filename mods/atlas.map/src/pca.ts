// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Map's 2-D projection of document embeddings: plain JavaScript, so it runs in the mod's Worker
// with no numpy and no sidecar. It replaces the sidecar's `do_reduce` (sidecar/pm_sidecar.py, "pca"
// path) and keeps its contract: documents in, coordinates in [0,1]^2 out, nearby = similar. What is
// the same: mean-centre, project onto the top two principal axes, min-max each axis into [0,1].
// What is different, on purpose:
//
//  - The axes come from block power iteration (two vectors at once, re-orthogonalised every step), not
//    an SVD. It needs one pass over the vectors per step and no d x d matrix, so 5,000 documents of 1,024
//    dimensions is a few seconds of one worker thread and no extra memory (measured: build log, B5).
//  - A FIXED SIGN. A principal axis has no sign of its own (numpy's SVD picks one by an implementation
//    detail), so without a rule the same library could draw mirror images from one recompute to the
//    next. Each axis is flipped so its largest-magnitude component is positive. The rule is a function
//    of the data alone, so the same documents always give the same map on every machine: nothing
//    here uses Math.random, a clock, or any function whose last bit can differ between engines (the
//    start vectors come from a seeded integer generator; the arithmetic is + - * / and sqrt only).
//  - A real zero. When the second axis carries no variance (every document on one line, or all alike)
//    the old code scaled rounding noise to fill [0,1]; here it is 0, so a one-dimensional library
//    draws a line and not a random scatter.
//  - The one-to-three-document case spreads over [0,1]^2 (the old code returned 0, 1, 2 on the x axis,
//    outside the range its own contract states; the Map fits the box either way, so nothing visible).
//
// Output is rounded to 5 decimals (at 900 pixels a span, 0.01 px) so a coordinate that is stored and
// read back is the SAME number as the one just computed: a remembered layout is identical to a fresh one.

/** The result: `x0, y0, x1, y1, ...` in [0,1], and which route produced it (the old `method`). */
export interface Reduction {
  coords: Float64Array;
  method: "none" | "trivial" | "pca";
}

/**
 * Iteration bound. Each step is one pass over the vectors (about 7 ms at 5,000 x 1,024), so the bound
 * is about 2 seconds of the worker at the largest library; the loop stops early on convergence, which
 * well-separated data reaches in a handful of steps. A library whose top axes have nearly EQUAL
 * variance (a vault of a dozen equally-sized topics) converges slowly and can stop at the bound. What
 * is then slightly off is the choice of axes inside that near-equal subspace, not the subspace: measured
 * on such synthetic data, the variance the two axes carry at 300 steps matches a 3,000-step run to the
 * fifth digit, and the coordinates differ by at most 0.01 of the box (0.00003 with a dozen topics).
 * Any such layout is exactly reproducible, which is what a cache needs.
 */
export const MAX_ITERATIONS = 300;
/** Converged when neither axis turns by more than this (as a vector length) in one step. */
const TURN_TOLERANCE = 1e-8;
/** A projection narrower than this (the old code's own span test) is no axis at all. */
const MIN_SPAN = 1e-9;
/** A second axis this much weaker than the first (singular values) is rounding noise, not structure. */
const NOISE_RATIO = 1e-6;
/** The most documents the one- to three-document spread covers (the old `n <= 3`). */
const TRIVIAL_MAX = 3;

const round5 = (v: number) => Math.round(v * 1e5) / 1e5;

/** A small seeded generator (mulberry32): start vectors that are the same on every run and machine. */
function seeded(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function norm(v: Float64Array): number {
  let s = 0;
  for (let j = 0; j < v.length; j++) s += v[j] * v[j];
  return Math.sqrt(s);
}

function dot(a: Float64Array, b: Float64Array): number {
  let s = 0;
  for (let j = 0; j < a.length; j++) s += a[j] * b[j];
  return s;
}

/** Make `a` and `b` an orthonormal pair (Gram-Schmidt). False when `b` has nothing left of it. */
function orthonormalise(a: Float64Array, b: Float64Array): boolean {
  const na = norm(a);
  if (!(na > 0)) return false;
  for (let j = 0; j < a.length; j++) a[j] /= na;
  const along = dot(b, a);
  for (let j = 0; j < b.length; j++) b[j] -= along * a[j];
  const nb = norm(b);
  if (!(nb > 0)) return false;
  for (let j = 0; j < b.length; j++) b[j] /= nb;
  return true;
}

/**
 * One pass over the rows: for each axis `q`, the centred projection of every row (`t = (x - mean) . q`)
 * and, from those, `Xc' Xc q` (the covariance times q, unnormalised). Rows are read once for both axes.
 */
function pass(
  vectors: Float32Array,
  count: number,
  dimension: number,
  mean: Float64Array,
  q1: Float64Array,
  q2: Float64Array,
  t1: Float64Array,
  t2: Float64Array,
  w1: Float64Array,
  w2: Float64Array,
): void {
  const mq1 = dot(mean, q1);
  const mq2 = dot(mean, q2);
  w1.fill(0);
  w2.fill(0);
  for (let i = 0; i < count; i++) {
    const row = i * dimension;
    let a = 0;
    let b = 0;
    for (let j = 0; j < dimension; j++) {
      const x = vectors[row + j];
      a += x * q1[j];
      b += x * q2[j];
    }
    a -= mq1;
    b -= mq2;
    t1[i] = a;
    t2[i] = b;
    for (let j = 0; j < dimension; j++) {
      const x = vectors[row + j];
      w1[j] += a * x;
      w2[j] += b * x;
    }
  }
  // Xc' t = X' t - mean * sum(t), and sum(t) is zero because t is centred (the mean's own projection
  // was taken off above), so the rows never need the mean subtracted: the sum of products is the answer.
}

/** Flip `axis` and its projections so the axis' largest-magnitude component is positive. */
function fixSign(axis: Float64Array, projections: Float64Array): void {
  let at = 0;
  for (let j = 1; j < axis.length; j++) if (Math.abs(axis[j]) > Math.abs(axis[at])) at = j;
  if (axis[at] < 0) {
    for (let j = 0; j < axis.length; j++) axis[j] = -axis[j];
    for (let i = 0; i < projections.length; i++) projections[i] = -projections[i];
  }
}

/** Min-max `values` into [0,1] (a span under `MIN_SPAN` is all zeros), written into `out` at `slot`. */
function fitUnit(values: Float64Array, out: Float64Array, slot: 0 | 1): void {
  let mn = Infinity;
  let mx = -Infinity;
  for (let i = 0; i < values.length; i++) {
    if (values[i] < mn) mn = values[i];
    if (values[i] > mx) mx = values[i];
  }
  const span = mx - mn;
  for (let i = 0; i < values.length; i++) {
    out[2 * i + slot] = span > MIN_SPAN ? round5((values[i] - mn) / span) : 0;
  }
}

/**
 * Project `count` vectors of `dimension` floats (row after row in `vectors`) to 2-D, each axis in
 * [0,1]. Throws on a shape that does not add up or a value that is not a finite number: a map is
 * never drawn from half-read data.
 */
export function reduceToUnit(
  vectors: Float32Array,
  count: number,
  dimension: number,
  probe?: { iterations: number },
): Reduction {
  if (probe) probe.iterations = 0;
  if (
    !Number.isSafeInteger(count) ||
    !Number.isSafeInteger(dimension) ||
    count < 0 ||
    dimension < 0 ||
    vectors.length !== count * dimension
  ) {
    throw new Error("the vectors do not match their count and dimension");
  }
  if (count === 0) return { coords: new Float64Array(0), method: "none" };
  if (count <= TRIVIAL_MAX) {
    // A deterministic spread along the middle of the box: three points have no principal axes to find.
    const coords = new Float64Array(2 * count);
    for (let i = 0; i < count; i++) {
      coords[2 * i] = count === 1 ? 0.5 : round5(i / (count - 1));
      coords[2 * i + 1] = 0.5;
    }
    return { coords, method: "trivial" };
  }

  const mean = new Float64Array(dimension);
  for (let i = 0; i < count; i++) {
    const row = i * dimension;
    for (let j = 0; j < dimension; j++) mean[j] += vectors[row + j];
  }
  for (let j = 0; j < dimension; j++) mean[j] /= count;
  if (!mean.every(Number.isFinite))
    throw new Error("the vectors hold a value that is not a number");

  const q1 = new Float64Array(dimension);
  const q2 = new Float64Array(dimension);
  const w1 = new Float64Array(dimension);
  const w2 = new Float64Array(dimension);
  const t1 = new Float64Array(count);
  const t2 = new Float64Array(count);
  const rand = seeded(0x4d41505a);
  for (let j = 0; j < dimension; j++) {
    q1[j] = rand() - 0.5;
    q2[j] = rand() - 0.5;
  }
  // A one-dimensional library has one axis and nothing to be orthogonal to.
  let twoAxes = dimension >= 2 && orthonormalise(q1, q2);
  if (!twoAxes) {
    const n = norm(q1);
    for (let j = 0; j < dimension; j++) q1[j] /= n;
    q2.fill(0);
  }

  for (let step = 0; step < MAX_ITERATIONS; step++) {
    if (probe) probe.iterations = step + 1;
    pass(vectors, count, dimension, mean, q1, q2, t1, t2, w1, w2);
    const n1 = norm(w1);
    // No variance at all (every document alike): any axis is as good as another, and there is none.
    if (!(n1 > 0)) break;
    for (let j = 0; j < dimension; j++) w1[j] /= n1;
    if (twoAxes) {
      const along = dot(w2, w1);
      for (let j = 0; j < dimension; j++) w2[j] -= along * w1[j];
      const n2 = norm(w2);
      if (n2 > 0) {
        for (let j = 0; j < dimension; j++) w2[j] /= n2;
      } else {
        // Nothing left once the first axis is taken out: one axis only.
        twoAxes = false;
        w2.fill(0);
      }
    }
    let turn1 = 0;
    let turn2 = 0;
    for (let j = 0; j < dimension; j++) {
      turn1 += (w1[j] - q1[j]) * (w1[j] - q1[j]);
      turn2 += (w2[j] - q2[j]) * (w2[j] - q2[j]);
    }
    q1.set(w1);
    q2.set(w2);
    // A covariance has no negative eigenvalue, so an axis never flips between steps and a plain
    // distance between successive axes is the turn.
    if (turn1 < TURN_TOLERANCE ** 2 && (!twoAxes || turn2 < TURN_TOLERANCE ** 2)) break;
  }

  // The projections for the axes as they ended: one more pass so `t` matches `q` exactly.
  pass(vectors, count, dimension, mean, q1, q2, t1, t2, w1, w2);
  // The size of each axis (its singular value): the second is only an axis if it carries real
  // variance next to the first. (A projection narrower than MIN_SPAN is zeros whatever its size:
  // fitUnit, the old code's own rule.)
  const s1 = Math.sqrt(t1.reduce((s, v) => s + v * v, 0));
  const s2 = Math.sqrt(t2.reduce((s, v) => s + v * v, 0));

  const coords = new Float64Array(2 * count);
  fixSign(q1, t1);
  fitUnit(t1, coords, 0);
  if (twoAxes && s2 >= NOISE_RATIO * s1) {
    fixSign(q2, t2);
    fitUnit(t2, coords, 1);
  }
  return { coords, method: "pca" };
}

// ---------------------------------------------------------------------------------------------------
// The top K axes, for the enhanced layout's 50-axis pre-step (tsne.ts). Everything above this line is
// the two-axis path and is unchanged, so every cached PCA picture stays fresh (ALGO_VERSION stays 1).
//
// Randomised subspace iteration (Halko, Martinsson and Tropp): start from a seeded random set of
// `K + OVERSAMPLE` directions, then a few times multiply by the covariance and re-orthonormalise, so
// the set drifts onto the top axes; finally take the exact top K inside that set (Rayleigh-Ritz: the
// eigenvectors of the small projected covariance, by Jacobi rotations). One pass over the vectors per
// multiplication, no d x d matrix, so 5,000 documents of 1,024 dimensions needs a few seconds of one
// worker thread and a few hundred kilobytes beyond the vectors. Like the two-axis path it uses only
// + - * / and sqrt, and a seeded integer generator, so the same documents give the same axes on every
// machine; a column's sign is fixed by its largest score.
// ---------------------------------------------------------------------------------------------------

/** A seeded uniform generator in [0, 1) (mulberry32): the same numbers on every run and machine. */
export function seededRandom(seed: number): () => number {
  return seeded(seed);
}

/** Extra directions carried beside the K wanted: they make the iteration converge on the K-th axis. */
export const OVERSAMPLE = 10;
/** Multiplications by the covariance. Captured variance is what t-SNE needs, and it converges fast. */
export const POWER_STEPS = 4;

/** The top axes of a set of vectors, as the projections of every row onto them. */
export interface PrincipalScores {
  /** `count x axes`, row-major: row i's coordinates on axis 0, 1, ... (the rows are mean-centred). */
  scores: Float64Array;
  /** How many axes: `min(k, dimension, count)`. */
  axes: number;
  /** The variance (sum of squared scores) each axis carries, largest first. */
  variance: Float64Array;
}

/** Orthonormalise the `rows` rows of `m` (each `width` long) in place, twice for accuracy. A row with
 *  nothing left of it becomes zero, so a rank-deficient set stays finite. */
function orthonormaliseRows(m: Float64Array, rows: number, width: number): void {
  for (let pass = 0; pass < 2; pass++) {
    for (let r = 0; r < rows; r++) {
      const at = r * width;
      for (let s = 0; s < r; s++) {
        const other = s * width;
        let along = 0;
        for (let j = 0; j < width; j++) along += m[at + j] * m[other + j];
        for (let j = 0; j < width; j++) m[at + j] -= along * m[other + j];
      }
      let n2 = 0;
      for (let j = 0; j < width; j++) n2 += m[at + j] * m[at + j];
      const len = Math.sqrt(n2);
      if (len > 1e-12) for (let j = 0; j < width; j++) m[at + j] /= len;
      else for (let j = 0; j < width; j++) m[at + j] = 0;
    }
  }
}

/** Eigenvalues (descending) and eigenvectors (`vectors[c * p + r]` is component r of eigenvector c) of
 *  the symmetric p x p matrix `a` (destroyed), by cyclic Jacobi rotations. */
function jacobiEigen(a: Float64Array, p: number): { values: Float64Array; vectors: Float64Array } {
  const v = new Float64Array(p * p);
  for (let i = 0; i < p; i++) v[i * p + i] = 1;
  for (let sweep = 0; sweep < 60; sweep++) {
    let off = 0;
    let diag = 0;
    for (let i = 0; i < p; i++) {
      diag += a[i * p + i] * a[i * p + i];
      for (let j = i + 1; j < p; j++) off += a[i * p + j] * a[i * p + j];
    }
    if (!(off > 1e-26 * (diag + off))) break;
    for (let i = 0; i < p - 1; i++) {
      for (let j = i + 1; j < p; j++) {
        const apq = a[i * p + j];
        if (apq === 0) continue;
        const theta = (a[j * p + j] - a[i * p + i]) / (2 * apq);
        const t = Math.sign(theta || 1) / (Math.abs(theta) + Math.sqrt(theta * theta + 1));
        const c = 1 / Math.sqrt(t * t + 1);
        const s = t * c;
        for (let k = 0; k < p; k++) {
          const aki = a[k * p + i];
          const akj = a[k * p + j];
          a[k * p + i] = c * aki - s * akj;
          a[k * p + j] = s * aki + c * akj;
        }
        for (let k = 0; k < p; k++) {
          const aik = a[i * p + k];
          const ajk = a[j * p + k];
          a[i * p + k] = c * aik - s * ajk;
          a[j * p + k] = s * aik + c * ajk;
        }
        for (let k = 0; k < p; k++) {
          const vki = v[i * p + k];
          const vkj = v[j * p + k];
          v[i * p + k] = c * vki - s * vkj;
          v[j * p + k] = s * vki + c * vkj;
        }
      }
    }
  }
  const order = Array.from({ length: p }, (_, i) => i).sort(
    (x, y) => a[y * p + y] - a[x * p + x] || x - y,
  );
  const values = new Float64Array(p);
  const vectors = new Float64Array(p * p);
  order.forEach((from, to) => {
    values[to] = a[from * p + from];
    vectors.set(v.subarray(from * p, from * p + p), to * p);
  });
  return { values, vectors };
}

/** `y[i * p + c] = (row i - mean) . q[c]` for every row and axis. */
function projectRows(
  data: Float32Array | Float64Array,
  count: number,
  dimension: number,
  mean: Float64Array,
  q: Float64Array,
  p: number,
  y: Float64Array,
): void {
  const centre = new Float64Array(p);
  for (let c = 0; c < p; c++) centre[c] = dot(mean, q.subarray(c * dimension, (c + 1) * dimension));
  for (let i = 0; i < count; i++) {
    const row = i * dimension;
    for (let c = 0; c < p; c++) {
      const at = c * dimension;
      let s = 0;
      for (let j = 0; j < dimension; j++) s += data[row + j] * q[at + j];
      y[i * p + c] = s - centre[c];
    }
  }
}

/**
 * The top `k` principal axes of `count` vectors of `dimension` numbers (row after row in `data`) and
 * every row's projection onto them. Throws on a shape that does not add up or a value that is not a
 * finite number. `seed` fixes the start directions (the result is a function of the data and seed).
 */
export function principalScores(
  data: Float32Array | Float64Array,
  count: number,
  dimension: number,
  k: number,
  seed = 0x50434135,
): PrincipalScores {
  if (
    !Number.isSafeInteger(count) ||
    !Number.isSafeInteger(dimension) ||
    !Number.isSafeInteger(k) ||
    count < 0 ||
    dimension < 0 ||
    k < 0 ||
    data.length !== count * dimension
  ) {
    throw new Error("the vectors do not match their count and dimension");
  }
  const axes = Math.min(k, dimension, count);
  if (axes === 0) return { scores: new Float64Array(0), axes: 0, variance: new Float64Array(0) };
  const p = Math.min(axes + OVERSAMPLE, dimension, count);

  const mean = new Float64Array(dimension);
  for (let i = 0; i < count; i++) {
    const row = i * dimension;
    for (let j = 0; j < dimension; j++) mean[j] += data[row + j];
  }
  for (let j = 0; j < dimension; j++) mean[j] /= count;
  if (!mean.every(Number.isFinite))
    throw new Error("the vectors hold a value that is not a number");

  const rand = seeded(seed);
  const q = new Float64Array(p * dimension);
  for (let j = 0; j < q.length; j++) q[j] = rand() - 0.5;
  orthonormaliseRows(q, p, dimension);

  const y = new Float64Array(count * p);
  const z = new Float64Array(p * dimension);
  for (let step = 0; step < POWER_STEPS; step++) {
    projectRows(data, count, dimension, mean, q, p, y);
    // z = Xc' y: the sum of y over the rows is zero (the mean's own projection was taken off), so the
    // rows never need the mean subtracted here.
    z.fill(0);
    for (let i = 0; i < count; i++) {
      const row = i * dimension;
      for (let c = 0; c < p; c++) {
        const w = y[i * p + c];
        const at = c * dimension;
        for (let j = 0; j < dimension; j++) z[at + j] += w * data[row + j];
      }
    }
    q.set(z);
    orthonormaliseRows(q, p, dimension);
  }
  projectRows(data, count, dimension, mean, q, p, y);

  // Rayleigh-Ritz: the projected covariance's eigenvectors rotate the p directions into the top axes.
  const small = new Float64Array(p * p);
  for (let i = 0; i < count; i++) {
    for (let a = 0; a < p; a++) {
      const ya = y[i * p + a];
      for (let b = a; b < p; b++) small[a * p + b] += ya * y[i * p + b];
    }
  }
  for (let a = 0; a < p; a++) for (let b = 0; b < a; b++) small[a * p + b] = small[b * p + a];
  const eigen = jacobiEigen(small, p);

  const scores = new Float64Array(count * axes);
  for (let i = 0; i < count; i++) {
    for (let c = 0; c < axes; c++) {
      let s = 0;
      for (let a = 0; a < p; a++) s += y[i * p + a] * eigen.vectors[c * p + a];
      scores[i * axes + c] = s;
    }
  }
  const variance = new Float64Array(axes);
  for (let c = 0; c < axes; c++) {
    // An axis has no sign of its own: its largest score is positive, ties to the lower row.
    let at = 0;
    for (let i = 1; i < count; i++) {
      if (Math.abs(scores[i * axes + c]) > Math.abs(scores[at * axes + c])) at = i;
    }
    if (scores[at * axes + c] < 0) for (let i = 0; i < count; i++) scores[i * axes + c] *= -1;
    let v = 0;
    for (let i = 0; i < count; i++) v += scores[i * axes + c] * scores[i * axes + c];
    variance[c] = v;
  }
  return { scores, axes, variance };
}
