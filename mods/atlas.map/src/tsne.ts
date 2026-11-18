// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Document Map's enhanced layout: t-SNE in plain JavaScript, so it runs in the mod's own Worker with
// no Python, no openTSNE and no library. It does what the old core Map did through the sidecar's
// `do_reduce` (sidecar/pm_sidecar.py, "tsne" path, openTSNE 1.0.4), recipe for recipe:
//
//  1. Mean-centre the vectors; when there are more than 50 dimensions, keep the top 50 principal axes
//     (pca.ts `principalScores`, subspace iteration).
//  2. The perplexity the sidecar asks for is min(30, max(5, floor((n - 1) / 3))). openTSNE lowers it to
//     (n - 1) / 3 when three times it would be more than the n - 1 other documents there are; each
//     document's neighbours are its k = min(n - 1, floor(3 x perplexity)) closest by cosine distance
//     (exact, for every n: openTSNE is exact below 1,000 documents and approximate, with Annoy, above).
//  3. Each document's neighbour weights are Gaussian on the SQUARED cosine distance, the Gaussian's width
//     found by bisection so the row's entropy is log(perplexity); the rows are symmetrised, (P + P') / 2,
//     and scaled to sum to 1.
//  4. The start is PCA: the top two axes, scaled so axis 0 has standard deviation 1e-4 (one factor for
//     both axes) plus 1 per cent jitter, from a fixed seed (42).
//  5. 250 steps with the attraction exaggerated 12 times, then 500 without; learning rate n /
//     exaggeration; momentum 0.8; per-coordinate gains (+0.2 when the step and the gradient disagree in
//     sign, else x0.8 + 0.01) carried from the first phase into the second; no step longer than 5; the
//     picture re-centred on zero after every step. The repulsion is Barnes-Hut at theta 0.5 on a
//     quadtree held in typed arrays (openTSNE does the same below 10,000 points).
//  6. Each axis is min-max scaled into [0,1] and rounded to 5 decimals, like pca.ts.
//
// What is different, on purpose: the numbers a seeded JavaScript generator makes are not numpy's, the
// neighbours are exact above 1,000 documents, and the gradient's sums run in another order, so the
// COORDINATES are not openTSNE's. The pictures are held to the same QUALITY (tsne.test.ts scores them
// against the fixture openTSNE made), never to the same coordinates.
//
// Determinism: uniform numbers from a seeded integer generator and + - * / sqrt, plus Math.exp and
// Math.log in the bisection. Two runs on the same input in one engine give the same bits. THE COORDINATES
// ARE A PER-VAULT CACHE (cache class, stripped from backups): mod storage lives in the vault's own store
// (mod_kv, mod_doc_data), not on the device, so every device that opens a vault reads the picture the
// vault holds, whichever device computed it, until the documents change. A recompute on another engine
// can differ in the last bit of Math.exp, so the arrangement can change after a recompute, with equal
// quality. That is accepted.

import { principalScores, reduceToUnit, seededRandom } from "./pca";

/** Which route made the picture ("none" and "trivial" are pca.ts's: no documents, three or fewer). */
export interface TsneReduction {
  coords: Float64Array;
  method: "none" | "trivial" | "tsne";
}

/** Raised from a cancel check: the run stopped, and nothing it made is kept. */
export class TsneCancelled extends Error {
  constructor() {
    super("the layout was cancelled");
    this.name = "TsneCancelled";
  }
}

/** The steps with exaggerated attraction, and the steps after them. */
export const EARLY_ITERATIONS = 250;
export const MAIN_ITERATIONS = 500;
export const TOTAL_ITERATIONS = EARLY_ITERATIONS + MAIN_ITERATIONS;
export const EARLY_EXAGGERATION = 12;
export const THETA = 0.5;
export const MOMENTUM = 0.8;
/** No point moves further than this in one step (openTSNE's `max_step_norm`). */
export const MAX_STEP = 5;
const MIN_GAIN = 0.01;
/** How many axes the pre-step keeps when the vectors are wider than this. */
export const PCA_AXES = 50;
/** The start: axis 0 gets this standard deviation, then jitter of this share of it. */
export const INIT_STD = 1e-4;
export const JITTER = 0.01;
/** The seed of the jitter (the sidecar's `random_state=42`). */
export const SEED = 42;
/** Progress is reported, and a cancel checked, every this many steps. */
export const REPORT_EVERY = 25;
/** The deepest the quadtree goes: points closer than the box / 2^40 are one leaf. */
const MAX_DEPTH = 40;
/** A span narrower than this is no axis at all (pca.ts `MIN_SPAN`). */
const MIN_SPAN = 1e-9;

const round5 = (v: number) => Math.round(v * 1e5) / 1e5;

// ---------------------------------------------------------------------------------------------------
// The plan: perplexity and neighbours
// ---------------------------------------------------------------------------------------------------

export interface Plan {
  /** What the sidecar asks openTSNE for: min(30, max(5, floor((n - 1) / 3))). */
  requested: number;
  /** What openTSNE uses: `requested`, or (n - 1) / 3 when 3 x requested is more than n - 1. */
  perplexity: number;
  /** Neighbours per document: min(n - 1, floor(3 x perplexity)). */
  neighbours: number;
}

export function plan(n: number): Plan {
  const requested = Math.min(30, Math.max(5, Math.floor((n - 1) / 3)));
  const perplexity = 3 * requested > n - 1 ? (n - 1) / 3 : requested;
  return { requested, perplexity, neighbours: Math.min(n - 1, Math.floor(3 * perplexity)) };
}

// ---------------------------------------------------------------------------------------------------
// Neighbours: exact, by cosine distance
// ---------------------------------------------------------------------------------------------------

export interface Neighbours {
  /** `n x k`: each row's neighbours, nearest first (ties to the lower index). */
  indices: Int32Array;
  /** `n x k`: the cosine distances (1 - cosine, clipped to [0, 2]), same order. */
  distances: Float64Array;
  k: number;
}

/** Exact k nearest neighbours of every row of `x` (`n x d`, row-major) by cosine distance. A row of
 *  zeros has cosine 0 with everything, so distance 1. A document is never its own neighbour. */
export function nearestNeighbours(
  x: Float64Array,
  n: number,
  d: number,
  k: number,
  isCancelled?: () => boolean,
): Neighbours {
  const unit = new Float64Array(n * d);
  for (let i = 0; i < n; i++) {
    let s = 0;
    for (let j = 0; j < d; j++) s += x[i * d + j] * x[i * d + j];
    const len = Math.sqrt(s) || 1;
    for (let j = 0; j < d; j++) unit[i * d + j] = x[i * d + j] / len;
  }
  const indices = new Int32Array(n * k);
  const distances = new Float64Array(n * k);
  for (let i = 0; i < n; i++) {
    if (isCancelled && i % 256 === 0 && isCancelled()) throw new TsneCancelled();
    const at = i * k;
    const a = i * d;
    let have = 0;
    for (let j = 0; j < n; j++) {
      if (j === i) continue;
      const b = j * d;
      let dot = 0;
      for (let c = 0; c < d; c++) dot += unit[a + c] * unit[b + c];
      const dist = Math.min(2, Math.max(0, 1 - dot));
      // Insertion into the sorted list; a later (higher) j never displaces an equal distance, so ties
      // keep the lower index, as a stable sort would.
      if (have === k && !(dist < distances[at + k - 1])) continue;
      let slot = have < k ? have : k - 1;
      while (slot > 0 && dist < distances[at + slot - 1]) {
        distances[at + slot] = distances[at + slot - 1];
        indices[at + slot] = indices[at + slot - 1];
        slot--;
      }
      distances[at + slot] = dist;
      indices[at + slot] = j;
      if (have < k) have++;
    }
  }
  return { indices, distances, k };
}

// ---------------------------------------------------------------------------------------------------
// Affinities
// ---------------------------------------------------------------------------------------------------

/** The entropy a row of probabilities is searched to have. */
const ENTROPY_TOLERANCE = 1e-8;

/**
 * Each row's conditional probabilities: Gaussian in the SQUARED distance to its neighbours, the width
 * found by bisection so the row's entropy (in nats) is log(perplexity). `squared` is `n x k`, each row
 * ascending. Returns `n x k`, each row summing to 1. When the entropy cannot be reached (all neighbours
 * equally far: it is log k whatever the width) the nearest the search gets is returned.
 */
export function conditionalAffinities(
  squared: Float64Array,
  n: number,
  k: number,
  perplexity: number,
): Float64Array {
  const out = new Float64Array(n * k);
  const target = Math.log(perplexity);
  for (let i = 0; i < n; i++) {
    const at = i * k;
    // Measured from the nearest neighbour, so the nearest always weighs 1 and nothing underflows to 0/0.
    const base = squared[at];
    let beta = 1;
    let lo = 0;
    let hi = Infinity;
    for (let step = 0; step < 200; step++) {
      let sum = 0;
      let moment = 0;
      for (let j = 0; j < k; j++) {
        const dd = squared[at + j] - base;
        const w = Math.exp(-beta * dd);
        out[at + j] = w;
        sum += w;
        moment += w * dd;
      }
      const entropy = Math.log(sum) + (beta * moment) / sum;
      const gap = entropy - target;
      if (Math.abs(gap) < ENTROPY_TOLERANCE) break;
      if (gap > 0) {
        lo = beta;
        beta = hi === Infinity ? beta * 2 : (beta + hi) / 2;
      } else {
        hi = beta;
        beta = (beta + lo) / 2;
      }
    }
    let sum = 0;
    for (let j = 0; j < k; j++) sum += out[at + j];
    for (let j = 0; j < k; j++) out[at + j] /= sum;
  }
  return out;
}

/** A sparse symmetric matrix (compressed rows): row i's entries are `indices[indptr[i]..indptr[i+1]]`. */
export interface Sparse {
  indptr: Int32Array;
  indices: Int32Array;
  values: Float64Array;
}

/** The joint probabilities: (P + P') / 2 of the conditional rows, scaled to sum to 1. */
export function symmetrise(
  neighbours: Int32Array,
  conditional: Float64Array,
  n: number,
  k: number,
): Sparse {
  // Every (row, column, value) both ways, grouped by row, then equal columns in a row merged.
  const count = new Int32Array(n + 1);
  for (let i = 0; i < n; i++) {
    count[i + 1] += k;
    for (let j = 0; j < k; j++) count[neighbours[i * k + j] + 1]++;
  }
  for (let i = 0; i < n; i++) count[i + 1] += count[i];
  const total = count[n];
  const col = new Int32Array(total);
  const val = new Float64Array(total);
  const fill = count.slice(0, n);
  for (let i = 0; i < n; i++) {
    for (let j = 0; j < k; j++) {
      const other = neighbours[i * k + j];
      const v = conditional[i * k + j] / 2;
      col[fill[i]] = other;
      val[fill[i]++] = v;
      col[fill[other]] = i;
      val[fill[other]++] = v;
    }
  }
  const indptr = new Int32Array(n + 1);
  const indices = new Int32Array(total);
  const values = new Float64Array(total);
  const seenIn = new Int32Array(n).fill(-1);
  const slotOf = new Int32Array(n);
  let out = 0;
  let sum = 0;
  for (let i = 0; i < n; i++) {
    for (let e = count[i]; e < count[i + 1]; e++) {
      const c = col[e];
      if (seenIn[c] !== i) {
        seenIn[c] = i;
        slotOf[c] = out;
        indices[out] = c;
        values[out++] = val[e];
      } else {
        values[slotOf[c]] += val[e];
      }
    }
    indptr[i + 1] = out;
  }
  for (let e = 0; e < out; e++) sum += values[e];
  const scale = 1 / sum;
  for (let e = 0; e < out; e++) values[e] *= scale;
  return { indptr, indices: indices.slice(0, out), values: values.slice(0, out) };
}

/** The joint probabilities of a neighbour graph: Gaussian weights on the SQUARED cosine distance at
 *  `perplexity`, symmetrised and scaled to sum to 1. */
export function jointProbabilities(near: Neighbours, n: number, perplexity: number): Sparse {
  const { indices, distances, k } = near;
  const squared = new Float64Array(n * k);
  for (let i = 0; i < n * k; i++) squared[i] = distances[i] * distances[i];
  return symmetrise(indices, conditionalAffinities(squared, n, k, perplexity), n, k);
}

// ---------------------------------------------------------------------------------------------------
// Barnes-Hut repulsion on a quadtree in typed arrays
// ---------------------------------------------------------------------------------------------------

/**
 * A quadtree over the points of a 2-D picture. Cell `c` has centre (cx, cy), half-width `hw`, `cnt`
 * points below it, their sum (then, after `finish`, their centre of mass) in (mx, my). A leaf holds one
 * point's index in `pt` (points at exactly the same place, and points too close to tell apart at
 * MAX_DEPTH, share a leaf, counted in `cnt`); an internal cell's four children are `child[c]` to
 * `child[c] + 3`, quadrant q = (x >= cx ? 1 : 0) | (y >= cy ? 2 : 0). No recursion, no allocation per
 * step once the arrays have grown to fit.
 */
export class QuadTree {
  private cap = 0;
  private cx = new Float64Array(0);
  private cy = new Float64Array(0);
  private hw = new Float64Array(0);
  private mx = new Float64Array(0);
  private my = new Float64Array(0);
  private cnt = new Int32Array(0);
  private child = new Int32Array(0);
  private pt = new Int32Array(0);
  private used = 0;
  private stack = new Int32Array(4 * MAX_DEPTH + 8);

  private grow(to: number): void {
    const f64 = (a: Float64Array) => {
      const b = new Float64Array(to);
      b.set(a);
      return b;
    };
    const i32 = (a: Int32Array) => {
      const b = new Int32Array(to);
      b.set(a);
      return b;
    };
    this.cx = f64(this.cx);
    this.cy = f64(this.cy);
    this.hw = f64(this.hw);
    this.mx = f64(this.mx);
    this.my = f64(this.my);
    this.cnt = i32(this.cnt);
    this.child = i32(this.child);
    this.pt = i32(this.pt);
    this.cap = to;
  }

  private newCell(cx: number, cy: number, hw: number): number {
    if (this.used === this.cap) this.grow(Math.max(64, this.cap * 2));
    const c = this.used++;
    this.cx[c] = cx;
    this.cy[c] = cy;
    this.hw[c] = hw;
    this.mx[c] = 0;
    this.my[c] = 0;
    this.cnt[c] = 0;
    this.child[c] = -1;
    this.pt[c] = -1;
    return c;
  }

  /** Build the tree over `n` points `y` (`x0, y0, x1, y1, ...`). */
  build(y: Float64Array, n: number): void {
    this.used = 0;
    let lox = Infinity;
    let loy = Infinity;
    let hix = -Infinity;
    let hiy = -Infinity;
    for (let i = 0; i < n; i++) {
      const x = y[2 * i];
      const v = y[2 * i + 1];
      if (x < lox) lox = x;
      if (x > hix) hix = x;
      if (v < loy) loy = v;
      if (v > hiy) hiy = v;
    }
    const half = Math.max(hix - lox, hiy - loy) / 2;
    // A little room so the largest point is strictly inside; a unit box when all points coincide.
    this.newCell((lox + hix) / 2, (loy + hiy) / 2, half > 0 ? half * (1 + 1e-9) : 1);
    for (let i = 0; i < n; i++) this.insert(y, i);
    for (let c = 0; c < this.used; c++) {
      if (this.cnt[c] > 0) {
        this.mx[c] /= this.cnt[c];
        this.my[c] /= this.cnt[c];
      }
    }
  }

  private insert(y: Float64Array, i: number): void {
    const x = y[2 * i];
    const v = y[2 * i + 1];
    let node = 0;
    let depth = 0;
    for (;;) {
      if (this.child[node] < 0) {
        if (this.cnt[node] === 0) {
          this.pt[node] = i;
          this.cnt[node] = 1;
          this.mx[node] = x;
          this.my[node] = v;
          return;
        }
        const p = this.pt[node];
        if ((y[2 * p] === x && y[2 * p + 1] === v) || depth >= MAX_DEPTH) {
          this.cnt[node]++;
          this.mx[node] += x;
          this.my[node] += v;
          return;
        }
        // Split: four children, and the point that was here moves into the one it belongs in.
        const h = this.hw[node] / 2;
        const ncx = this.cx[node];
        const ncy = this.cy[node];
        const first = this.used;
        for (let q = 0; q < 4; q++) {
          this.newCell(ncx + (q & 1 ? h : -h), ncy + (q & 2 ? h : -h), h);
        }
        this.child[node] = first;
        const into = first + ((y[2 * p] >= ncx ? 1 : 0) | (y[2 * p + 1] >= ncy ? 2 : 0));
        this.pt[into] = p;
        this.cnt[into] = this.cnt[node];
        this.mx[into] = this.mx[node];
        this.my[into] = this.my[node];
        // This cell now only routes; its own `pt` is spent.
        this.pt[node] = -1;
      }
      this.cnt[node]++;
      this.mx[node] += x;
      this.my[node] += v;
      node = this.child[node] + ((x >= this.cx[node] ? 1 : 0) | (v >= this.cy[node] ? 2 : 0));
      depth++;
    }
  }

  /**
   * For every point: `neg[2i], neg[2i+1]` = the sum over the other points of w^2 (y_i - y_j), with
   * w = 1 / (1 + |y_i - y_j|^2); returns Z, the sum over all ordered pairs of w. A cell is taken as
   * one point at its centre of mass when its width is under `theta` times its distance; theta 0 never
   * does, so it is the exact sum.
   */
  repulsion(y: Float64Array, n: number, theta: number, neg: Float64Array): number {
    const theta2 = theta * theta;
    const { cnt, child, pt, mx, my, hw, stack } = this;
    let z = 0;
    for (let i = 0; i < n; i++) {
      const xi = y[2 * i];
      const yi = y[2 * i + 1];
      let fx = 0;
      let fy = 0;
      let sp = 0;
      stack[sp++] = 0;
      while (sp > 0) {
        const node = stack[--sp];
        let c = cnt[node];
        if (c === 0) continue;
        let dx = xi - mx[node];
        let dy = yi - my[node];
        let d2 = dx * dx + dy * dy;
        if (child[node] < 0) {
          const p = pt[node];
          if (y[2 * p] === xi && y[2 * p + 1] === yi) {
            // The point's own leaf (and any twins at exactly its place): twins sit at distance 0.
            c -= 1;
            if (c === 0) continue;
            dx = 0;
            dy = 0;
            d2 = 0;
          }
        } else {
          const width = 2 * hw[node];
          if (!(width * width < theta2 * d2)) {
            const first = child[node];
            stack[sp++] = first;
            stack[sp++] = first + 1;
            stack[sp++] = first + 2;
            stack[sp++] = first + 3;
            continue;
          }
        }
        const w = 1 / (1 + d2);
        z += c * w;
        const w2 = c * w * w;
        fx += w2 * dx;
        fy += w2 * dy;
      }
      neg[2 * i] = fx;
      neg[2 * i + 1] = fy;
    }
    return z;
  }
}

/**
 * The gradient of the KL divergence for the picture `y`, in openTSNE's scale (no factor 4):
 * g_i = sum_j (E p_ij - w_ij / Z) w_ij (y_i - y_j), the attraction from the sparse `p` exaggerated by
 * `exaggeration` and the repulsion from the tree at `theta`.
 */
export function gradient(
  y: Float64Array,
  n: number,
  p: Sparse,
  exaggeration: number,
  theta: number,
  tree: QuadTree,
  out: Float64Array,
): void {
  tree.build(y, n);
  const z = tree.repulsion(y, n, theta, out);
  const invZ = 1 / z;
  for (let i = 0; i < n; i++) {
    const xi = y[2 * i];
    const yi = y[2 * i + 1];
    let ax = 0;
    let ay = 0;
    for (let e = p.indptr[i]; e < p.indptr[i + 1]; e++) {
      const j = p.indices[e];
      const dx = xi - y[2 * j];
      const dy = yi - y[2 * j + 1];
      const w = 1 / (1 + dx * dx + dy * dy);
      const pw = p.values[e] * w;
      ax += pw * dx;
      ay += pw * dy;
    }
    out[2 * i] = exaggeration * ax - out[2 * i] * invZ;
    out[2 * i + 1] = exaggeration * ay - out[2 * i + 1] * invZ;
  }
}

// ---------------------------------------------------------------------------------------------------
// The whole run
// ---------------------------------------------------------------------------------------------------

export interface TsneOptions {
  /** Called every REPORT_EVERY steps with the share of the steps done, 0 to 1. */
  onProgress?: (fraction: number) => void;
  /** Checked at the same points (and between the neighbour search's rows); true stops the run with
   *  {@link TsneCancelled}. */
  isCancelled?: () => boolean;
}

/** The exaggeration of the attraction and the learning rate (n / exaggeration) at step `iter`. */
export function schedule(iter: number, n: number): { exaggeration: number; rate: number } {
  const exaggeration = iter < EARLY_ITERATIONS ? EARLY_EXAGGERATION : 1;
  return { exaggeration, rate: n / exaggeration };
}

/**
 * One descent step, in place: per-coordinate gains (+0.2 where the last step and this gradient disagree
 * in sign, else x0.8 + 0.01), momentum, no point moving further than MAX_STEP, then the picture
 * re-centred on zero. `update` and `gains` carry over from one step to the next (and from the
 * exaggerated phase into the plain one).
 */
export function step(
  y: Float64Array,
  n: number,
  grad: Float64Array,
  update: Float64Array,
  gains: Float64Array,
  rate: number,
): void {
  for (let c = 0; c < 2 * n; c++) {
    const flipped = Math.sign(update[c]) !== Math.sign(grad[c]);
    gains[c] = flipped ? gains[c] + 0.2 : gains[c] * 0.8 + MIN_GAIN;
    update[c] = MOMENTUM * update[c] - rate * gains[c] * grad[c];
  }
  for (let i = 0; i < n; i++) {
    const ux = update[2 * i];
    const uy = update[2 * i + 1];
    const len = Math.sqrt(ux * ux + uy * uy);
    if (len > MAX_STEP) {
      update[2 * i] = (ux / len) * MAX_STEP;
      update[2 * i + 1] = (uy / len) * MAX_STEP;
    }
  }
  let sx = 0;
  let sy = 0;
  for (let i = 0; i < n; i++) {
    y[2 * i] += update[2 * i];
    y[2 * i + 1] += update[2 * i + 1];
    sx += y[2 * i];
    sy += y[2 * i + 1];
  }
  sx /= n;
  sy /= n;
  for (let i = 0; i < n; i++) {
    y[2 * i] -= sx;
    y[2 * i + 1] -= sy;
  }
}

/** The start: the top two axes scaled to axis 0 having standard deviation INIT_STD, plus jitter. */
export function initialPicture(x: Float64Array, n: number, d: number): Float64Array {
  const rand = seededRandom(SEED);
  const axes = principalScores(x, n, d, 2);
  const y = new Float64Array(2 * n);
  for (let i = 0; i < n; i++) {
    for (let c = 0; c < 2; c++) y[2 * i + c] = c < axes.axes ? axes.scores[i * axes.axes + c] : 0;
  }
  let sumSq = 0;
  for (let i = 0; i < n; i++) sumSq += y[2 * i] * y[2 * i];
  const std = Math.sqrt(sumSq / n);
  // Uniform noise of the standard deviation a normal one would have: width sqrt(3) x std.
  const spread = Math.sqrt(3);
  if (std > 0) {
    const factor = INIT_STD / std;
    for (let i = 0; i < 2 * n; i++) y[i] *= factor;
    const noise = INIT_STD * JITTER * spread;
    for (let i = 0; i < 2 * n; i++) y[i] += (2 * rand() - 1) * noise;
  } else {
    // Every document alike (or one on a line of zero width): no axis to start from.
    for (let i = 0; i < 2 * n; i++) y[i] = (2 * rand() - 1) * INIT_STD * spread;
  }
  return y;
}

/** Min-max each axis of `y` into [0,1], rounded to 5 decimals (a span under MIN_SPAN is all zeros). */
function fitUnit(y: Float64Array, n: number): Float64Array {
  const out = new Float64Array(2 * n);
  for (let axis = 0; axis < 2; axis++) {
    let mn = Infinity;
    let mx = -Infinity;
    for (let i = 0; i < n; i++) {
      const v = y[2 * i + axis];
      if (v < mn) mn = v;
      if (v > mx) mx = v;
    }
    const span = mx - mn;
    for (let i = 0; i < n; i++) {
      out[2 * i + axis] = span > MIN_SPAN ? round5((y[2 * i + axis] - mn) / span) : 0;
    }
  }
  return out;
}

/**
 * Lay `count` vectors of `dimension` floats (row after row in `vectors`) out in 2-D by t-SNE, each axis
 * in [0,1]. Three documents or fewer take pca.ts's spread. Throws on a shape that does not add up or a
 * value that is not a finite number: a map is never drawn from half-read data. The input is not changed.
 */
export function tsne(
  vectors: Float32Array,
  count: number,
  dimension: number,
  options: TsneOptions = {},
): TsneReduction {
  if (
    !Number.isSafeInteger(count) ||
    !Number.isSafeInteger(dimension) ||
    count < 0 ||
    dimension < 0 ||
    vectors.length !== count * dimension
  ) {
    throw new Error("the vectors do not match their count and dimension");
  }
  for (let i = 0; i < vectors.length; i++) {
    if (!Number.isFinite(vectors[i]))
      throw new Error("the vectors hold a value that is not a number");
  }
  if (count <= 3) {
    const small = reduceToUnit(vectors, count, dimension);
    return { coords: small.coords, method: small.method as "none" | "trivial" };
  }
  const { onProgress, isCancelled } = options;
  const cancelCheck = () => {
    if (isCancelled && isCancelled()) throw new TsneCancelled();
  };
  cancelCheck();
  const n = count;

  // 1. Mean-centre; above 50 dimensions keep the top 50 axes (their scores are already centred).
  let x: Float64Array;
  let d: number;
  if (dimension > PCA_AXES) {
    const reduced = principalScores(vectors, n, dimension, PCA_AXES);
    x = reduced.scores;
    d = reduced.axes;
  } else {
    d = dimension;
    x = new Float64Array(n * d);
    const mean = new Float64Array(d);
    for (let i = 0; i < n; i++) for (let j = 0; j < d; j++) mean[j] += vectors[i * d + j];
    for (let j = 0; j < d; j++) mean[j] /= n;
    for (let i = 0; i < n; i++)
      for (let j = 0; j < d; j++) x[i * d + j] = vectors[i * d + j] - mean[j];
  }
  cancelCheck();

  // 2 and 3. Neighbours, affinities.
  const { perplexity, neighbours: k } = plan(n);
  const near = nearestNeighbours(x, n, d, k, isCancelled);
  const p = jointProbabilities(near, n, perplexity);
  cancelCheck();

  // 4. The start.
  const y = initialPicture(x, n, d);

  // 5. The descent.
  const tree = new QuadTree();
  const grad = new Float64Array(2 * n);
  const update = new Float64Array(2 * n);
  const gains = new Float64Array(2 * n).fill(1);
  onProgress?.(0);
  for (let iter = 0; iter < TOTAL_ITERATIONS; iter++) {
    const { exaggeration, rate } = schedule(iter, n);
    gradient(y, n, p, exaggeration, THETA, tree, grad);
    step(y, n, grad, update, gains, rate);
    if ((iter + 1) % REPORT_EVERY === 0) {
      onProgress?.((iter + 1) / TOTAL_ITERATIONS);
      cancelCheck();
    }
  }

  // 6. Into the box.
  return { coords: fitUnit(y, n), method: "tsne" };
}
