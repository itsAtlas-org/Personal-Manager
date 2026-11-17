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
