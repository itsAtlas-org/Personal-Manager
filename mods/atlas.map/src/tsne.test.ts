// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The JavaScript t-SNE against openTSNE, the layout it replaces. Never coordinates (openTSNE's are not
// reproducible outside it): QUALITY SCORES, from fixtures/tsne-opentsne.json, which holds what openTSNE and
// scikit-learn made of eight seeded cases. quality.test.ts proves the scorer matches scikit-learn first.
//
// The margins below were set from the first real run (every number is in the table under MARGINS); a margin
// is a bound on how far BELOW openTSNE the port may fall, never a licence to be above it.

import { describe, expect, it } from "vitest";
import { principalScores, seededRandom } from "./pca";
import { scorePicture, type Scores } from "./quality";
import {
  EARLY_EXAGGERATION,
  EARLY_ITERATIONS,
  INIT_STD,
  JITTER,
  MAIN_ITERATIONS,
  MAX_STEP,
  MOMENTUM,
  QuadTree,
  REPORT_EVERY,
  SEED,
  THETA,
  TOTAL_ITERATIONS,
  TsneCancelled,
  conditionalAffinities,
  gradient,
  initialPicture,
  jointProbabilities,
  nearestNeighbours,
  plan,
  schedule,
  step,
  symmetrise,
  tsne,
} from "./tsne";
import { reduceToUnit } from "./pca";
import { tsneCases } from "./tsneFixture";

const byName = (name: string) => tsneCases.find((c) => c.name === name)!;

/**
 * How far the port may fall below openTSNE, set from the first run. The port's numbers against openTSNE's
 * (port minus openTSNE), trustworthiness / recall / purity:
 *   separated-300x64    -0.0001 / -0.0007 /  0       overlapping-300x32  +0.0014 / +0.0037 / -0.0273
 *   curved-300x16       -0.0018 / -0.0013 / -0.0046  tiny-4x8             0      /  0      /  0
 *   tiny-8x8            -0.0278 / +0.0417 / +0.0417  tiny-20x16          -0.0250 / -0.0222 / -0.0222
 *   wide-150x96         +0.0010 / +0.0047 /  0       large-1200x32       +0.0025 / +0.0052 / +0.0084
 * A picture of 20 documents moves by whole neighbours (one document is 0.05), so the three tiny cases get
 * a wider margin than the ones with hundreds. Purity is the noisiest of the three on the overlapping case.
 */
const MARGINS = {
  large: { trustworthiness: 0.01, recall: 0.02, purity: 0.04 },
  tiny: { trustworthiness: 0.05, recall: 0.06, purity: 0.06 },
};
const marginsFor = (n: number) => (n < 30 ? MARGINS.tiny : MARGINS.large);

describe("the layout against openTSNE's quality, case by case", () => {
  it.each(tsneCases.map((c) => c.name))(
    "%s: no worse than openTSNE by more than the margin, and its coordinates are in [0,1] at 5 decimals",
    (name) => {
      const c = byName(name);
      const result = tsne(c.vectors, c.n, c.dimension);
      expect(result.method).toBe("tsne");
      expect(result.coords.length).toBe(2 * c.n);
      for (const v of result.coords) {
        expect(Number.isFinite(v)).toBe(true);
        expect(v).toBeGreaterThanOrEqual(0);
        expect(v).toBeLessThanOrEqual(1);
        expect(Math.abs(v * 1e5 - Math.round(v * 1e5))).toBeLessThan(1e-6);
      }
      const got = scorePicture(
        c.vectors,
        result.coords,
        c.labels,
        c.n,
        c.dimension,
        c.scoreNeighbours,
      );
      const want: Scores = c.scores.tsne;
      const m = marginsFor(c.n);
      expect(got.trustworthiness, "trustworthiness").toBeGreaterThanOrEqual(
        want.trustworthiness - m.trustworthiness,
      );
      expect(got.recall, "recall").toBeGreaterThanOrEqual(want.recall - m.recall);
      expect(got.purity, "purity").toBeGreaterThanOrEqual(want.purity - m.purity);
    },
    120_000,
  );

  it("is clearly above PCA wherever openTSNE is (trustworthiness by 0.02, recall by 0.1)", () => {
    let compared = 0;
    for (const c of tsneCases) {
      if (c.scores.tsne.trustworthiness - c.scores.pca.trustworthiness < 0.02) continue;
      compared++;
      const result = tsne(c.vectors, c.n, c.dimension);
      const got = scorePicture(
        c.vectors,
        result.coords,
        c.labels,
        c.n,
        c.dimension,
        c.scoreNeighbours,
      );
      expect(got.trustworthiness, c.name).toBeGreaterThan(c.scores.pca.trustworthiness + 0.02);
      expect(got.recall, c.name).toBeGreaterThan(c.scores.pca.recall + 0.1);
    }
    // The cases where openTSNE beats PCA clearly: separated, overlapping, wide, large, and n = 4.
    expect(compared).toBeGreaterThanOrEqual(5);
  }, 120_000);

  it("the n = 1,200 case (past openTSNE's switch to approximate neighbours) meets the same margin", () => {
    const c = byName("large-1200x32");
    expect(c.n).toBeGreaterThanOrEqual(1000);
    const result = tsne(c.vectors, c.n, c.dimension);
    const got = scorePicture(
      c.vectors,
      result.coords,
      c.labels,
      c.n,
      c.dimension,
      c.scoreNeighbours,
    );
    expect(got.trustworthiness).toBeGreaterThanOrEqual(
      c.scores.tsne.trustworthiness - MARGINS.large.trustworthiness,
    );
  }, 120_000);
});

describe("the perplexity and neighbour rule", () => {
  it("gives, for every fixture case, the perplexity and neighbours openTSNE really used", () => {
    for (const c of tsneCases) {
      const p = plan(c.n);
      expect(p.requested, c.name).toBe(c.requestedPerplexity);
      expect(p.perplexity, c.name).toBeCloseTo(c.effectivePerplexity, 12);
      expect(p.neighbours, c.name).toBe(c.neighbours);
    }
  });

  it("asks for min(30, max(5, floor((n - 1) / 3))) and lowers it to (n - 1) / 3 when 3x is over n - 1", () => {
    // The 5 floor (n = 4 and n = 8 ask for 5 though (n - 1) / 3 is 1 and 2), the 30 ceiling (n = 100 would
    // give 33), the middle (n = 50 gives 16), and the k / 3 rule at the edge: 3 x 5 = 15 is not over n - 1 =
    // 15 (n = 16), but is over 14 (n = 15).
    expect(plan(100)).toEqual({ requested: 30, perplexity: 30, neighbours: 90 });
    expect(plan(50)).toEqual({ requested: 16, perplexity: 16, neighbours: 48 });
    expect(plan(16)).toEqual({ requested: 5, perplexity: 5, neighbours: 15 });
    const fifteen = plan(15);
    expect(fifteen.requested).toBe(5);
    expect(fifteen.perplexity).toBeCloseTo(14 / 3, 12);
    expect(fifteen.neighbours).toBe(14);
    expect(plan(4)).toEqual({ requested: 5, perplexity: 1, neighbours: 3 });
    expect(plan(1000)).toEqual({ requested: 30, perplexity: 30, neighbours: 90 });
    expect(plan(5000)).toEqual({ requested: 30, perplexity: 30, neighbours: 90 });
  });
});

// ---- the three pieces that need no openTSNE ----

/** A seeded cloud of n vectors of d numbers (roughly normal). */
function cloud(n: number, d: number, seed: number): Float64Array {
  const rand = seededRandom(seed);
  const out = new Float64Array(n * d);
  for (let i = 0; i < out.length; i++) out[i] = rand() + rand() + rand() - 1.5;
  return out;
}

describe("exact nearest neighbours", () => {
  it("equal a brute-force sort by cosine distance, nearest first, never the document itself", () => {
    const n = 120;
    const d = 12;
    const k = 17;
    const x = cloud(n, d, 7);
    const got = nearestNeighbours(x, n, d, k);
    for (let i = 0; i < n; i++) {
      const dist = (a: number, b: number) => {
        let dot = 0;
        let na = 0;
        let nb = 0;
        for (let c = 0; c < d; c++) {
          dot += x[a * d + c] * x[b * d + c];
          na += x[a * d + c] ** 2;
          nb += x[b * d + c] ** 2;
        }
        return 1 - dot / Math.sqrt(na * nb);
      };
      const order = Array.from({ length: n }, (_, j) => j)
        .filter((j) => j !== i)
        .sort((a, b) => dist(i, a) - dist(i, b) || a - b)
        .slice(0, k);
      expect(Array.from(got.indices.subarray(i * k, (i + 1) * k)), `row ${i}`).toEqual(order);
      order.forEach((j, r) => {
        expect(Math.abs(got.distances[i * k + r] - dist(i, j))).toBeLessThan(1e-12);
      });
    }
  });

  it("break a tie by the lower index, and put a zero vector at distance 1 from everything", () => {
    // Documents 1, 2 and 3 are the same vector: from document 0 they are equally far (cosine 0), so the
    // order is 1, 2, 3; from document 1 the twins 2 and 3 are at distance 0, and 0 comes last.
    const x = Float64Array.from([1, 0, 0, 0, 1, 1, 0, 1, 1, 0, 1, 1]);
    const near = nearestNeighbours(x, 4, 3, 3);
    expect(Array.from(near.indices.subarray(0, 3))).toEqual([1, 2, 3]);
    expect(Array.from(near.indices.subarray(3, 6))).toEqual([2, 3, 0]);
    const withZero = nearestNeighbours(Float64Array.from([1, 0, 0, 0, 0, 1]), 3, 2, 2);
    // Row 1 is the zero vector: cosine 0 with the others, distance 1, ties to the lower index.
    expect(Array.from(withZero.distances.subarray(2, 4))).toEqual([1, 1]);
    expect(Array.from(withZero.indices.subarray(2, 4))).toEqual([0, 2]);
  });
});

describe("the affinities", () => {
  const n = 200;
  const d = 10;
  const { perplexity, neighbours: k } = plan(n);
  const x = cloud(n, d, 11);
  const near = nearestNeighbours(x, n, d, k);
  const squared = near.distances.map((v) => v * v);
  const conditional = conditionalAffinities(squared, n, k, perplexity);

  it("give every row an entropy of log(perplexity)", () => {
    for (let i = 0; i < n; i++) {
      let sum = 0;
      let entropy = 0;
      for (let j = 0; j < k; j++) {
        const p = conditional[i * k + j];
        sum += p;
        entropy -= p * Math.log(p);
      }
      expect(Math.abs(sum - 1), `row ${i} sums to 1`).toBeLessThan(1e-12);
      expect(Math.abs(entropy - Math.log(perplexity)), `row ${i} entropy`).toBeLessThan(1e-6);
    }
  });

  it("weigh a neighbour by a Gaussian in the SQUARED distance (the log-ratio is linear in it)", () => {
    // log p_j - log p_0 = -beta (d_j^2 - d_0^2): the same beta for every neighbour of a row.
    for (const i of [0, 57, 199]) {
      const beta = (a: number) =>
        -(Math.log(conditional[i * k + a]) - Math.log(conditional[i * k])) /
        (squared[i * k + a] - squared[i * k]);
      expect(Math.abs(beta(5) - beta(60))).toBeLessThan(1e-6 * Math.abs(beta(5)));
      expect(beta(5)).toBeGreaterThan(0);
    }
  });

  it("are built from the squared distance by jointProbabilities (the step tsne() takes)", () => {
    const got = jointProbabilities(near, n, perplexity);
    const want = symmetrise(near.indices, conditional, n, k);
    expect(got.indptr).toEqual(want.indptr);
    expect(got.indices).toEqual(want.indices);
    for (let e = 0; e < want.values.length; e++) {
      expect(Math.abs(got.values[e] - want.values[e])).toBeLessThan(1e-15);
    }
  });

  it("are symmetrised to (P + P') / 2 and scaled to sum to 1", () => {
    const sparse = symmetrise(near.indices, conditional, n, k);
    const dense = new Float64Array(n * n);
    for (let i = 0; i < n; i++) {
      for (let j = 0; j < k; j++) {
        dense[i * n + near.indices[i * k + j]] += conditional[i * k + j] / 2;
        dense[near.indices[i * k + j] * n + i] += conditional[i * k + j] / 2;
      }
    }
    let total = 0;
    for (const v of dense) total += v;
    const got = new Float64Array(n * n);
    for (let i = 0; i < n; i++) {
      const seen = new Set<number>();
      for (let e = sparse.indptr[i]; e < sparse.indptr[i + 1]; e++) {
        expect(seen.has(sparse.indices[e]), "a column once per row").toBe(false);
        seen.add(sparse.indices[e]);
        got[i * n + sparse.indices[e]] = sparse.values[e];
      }
    }
    let sum = 0;
    for (let i = 0; i < n * n; i++) {
      sum += got[i];
      expect(Math.abs(got[i] - dense[i] / total)).toBeLessThan(1e-14);
      expect(Math.abs(got[i] - got[(i % n) * n + Math.floor(i / n)])).toBeLessThan(1e-15);
    }
    expect(Math.abs(sum - 1)).toBeLessThan(1e-12);
  });
});

/** The exact O(n^2) gradient, in openTSNE's scale: sum_j (E p_ij - w_ij / Z) w_ij (y_i - y_j). */
function exactGradient(
  y: Float64Array,
  n: number,
  p: ReturnType<typeof symmetrise>,
  exaggeration: number,
): Float64Array {
  const dense = new Float64Array(n * n);
  for (let i = 0; i < n; i++) {
    for (let e = p.indptr[i]; e < p.indptr[i + 1]; e++) dense[i * n + p.indices[e]] = p.values[e];
  }
  let z = 0;
  const w = new Float64Array(n * n);
  for (let i = 0; i < n; i++) {
    for (let j = 0; j < n; j++) {
      if (i === j) continue;
      const dx = y[2 * i] - y[2 * j];
      const dy = y[2 * i + 1] - y[2 * j + 1];
      w[i * n + j] = 1 / (1 + dx * dx + dy * dy);
      z += w[i * n + j];
    }
  }
  const g = new Float64Array(2 * n);
  for (let i = 0; i < n; i++) {
    for (let j = 0; j < n; j++) {
      if (i === j) continue;
      const f = (exaggeration * dense[i * n + j] - w[i * n + j] / z) * w[i * n + j];
      g[2 * i] += f * (y[2 * i] - y[2 * j]);
      g[2 * i + 1] += f * (y[2 * i + 1] - y[2 * j + 1]);
    }
  }
  return g;
}

function problem(n: number, seed: number, duplicates = 0) {
  const d = 6;
  const { perplexity, neighbours: k } = plan(n);
  const x = cloud(n, d, seed);
  const near = nearestNeighbours(x, n, d, k);
  const p = symmetrise(
    near.indices,
    conditionalAffinities(
      near.distances.map((v) => v * v),
      n,
      k,
      perplexity,
    ),
    n,
    k,
  );
  const rand = seededRandom(seed + 1);
  const y = new Float64Array(2 * n);
  for (let i = 0; i < 2 * n; i++) y[i] = (rand() - 0.5) * 20;
  // Points at exactly the same place as an earlier one.
  for (let t = 0; t < duplicates; t++) {
    y[2 * (n - 1 - t)] = y[2 * t];
    y[2 * (n - 1 - t) + 1] = y[2 * t + 1];
  }
  return { n, p, y };
}

function relativeError(a: Float64Array, b: Float64Array): number {
  let diff = 0;
  let norm = 0;
  for (let i = 0; i < a.length; i++) {
    diff += (a[i] - b[i]) ** 2;
    norm += b[i] ** 2;
  }
  return Math.sqrt(diff / norm);
}

describe("the Barnes-Hut gradient against the exact one (50 points)", () => {
  const { n, p, y } = problem(50, 21);
  const tree = new QuadTree();

  it("at theta 0 equals the exact gradient (the tree visits every point)", () => {
    for (const exaggeration of [1, 12]) {
      const got = new Float64Array(2 * n);
      gradient(y, n, p, exaggeration, 0, tree, got);
      expect(relativeError(got, exactGradient(y, n, p, exaggeration))).toBeLessThan(1e-12);
    }
  });

  it("at theta 0.5 is within about 2 per cent", () => {
    const got = new Float64Array(2 * n);
    gradient(y, n, p, 1, THETA, tree, got);
    const error = relativeError(got, exactGradient(y, n, p, 1));
    expect(error).toBeLessThan(0.02);
    // ...and it really is an approximation (a tree that never summarised would pass the line above).
    expect(error).toBeGreaterThan(1e-9);
  });

  it("at theta 2 is visibly off, so a loose theta is not mistaken for the right one", () => {
    const got = new Float64Array(2 * n);
    gradient(y, n, p, 1, 2, tree, got);
    expect(relativeError(got, exactGradient(y, n, p, 1))).toBeGreaterThan(0.03);
  });

  it("is exact at theta 0 and close at 0.5 with points at exactly the same place (twins)", () => {
    const twins = problem(60, 33, 8);
    const exact = exactGradient(twins.y, twins.n, twins.p, 1);
    const got = new Float64Array(2 * twins.n);
    gradient(twins.y, twins.n, twins.p, 1, 0, tree, got);
    expect(relativeError(got, exact)).toBeLessThan(1e-12);
    gradient(twins.y, twins.n, twins.p, 1, THETA, tree, got);
    expect(relativeError(got, exact)).toBeLessThan(0.03);
  });

  it("holds when every point is in one place (no division by zero)", () => {
    const same = new Float64Array(2 * n);
    const got = new Float64Array(2 * n);
    gradient(same, n, p, 12, THETA, tree, got);
    for (const v of got) expect(Number.isFinite(v)).toBe(true);
  });
});

describe("one descent step", () => {
  it("raises a gain by 0.2 where the last step and the gradient disagree in sign, else x0.8 + 0.01", () => {
    // n = 2: coordinates (x0, y0, x1, y1). Last steps +, +, -, 0; gradients +, -, -, +.
    const y = new Float64Array(4);
    const update = Float64Array.from([1, 1, -1, 0]);
    const gains = Float64Array.from([2, 2, 2, 2]);
    step(y, 2, Float64Array.from([1, -1, -1, 1]), update, gains, 0);
    expect(Array.from(gains)).toEqual([2 * 0.8 + 0.01, 2 + 0.2, 2 * 0.8 + 0.01, 2 + 0.2]);
  });

  it("is momentum 0.8 of the last step minus rate x gain x gradient", () => {
    const y = new Float64Array(4);
    const update = Float64Array.from([1, 0, 0, 0]);
    const gains = Float64Array.from([1, 1, 1, 1]);
    step(y, 2, Float64Array.from([0.5, 0.25, 0, 0]), update, gains, 2);
    // Coordinate 0: sign(+1) = sign(+0.5), gain 0.81; 0.8 * 1 - 2 * 0.81 * 0.5 = -0.01. Coordinate 1: the
    // last step was 0 and the gradient is not, so the gain rises to 1.2: -2 * 1.2 * 0.25 = -0.6.
    expect(update[0]).toBeCloseTo(MOMENTUM * 1 - 2 * 0.81 * 0.5, 12);
    expect(update[1]).toBeCloseTo(-2 * 1.2 * 0.25, 12);
  });

  it("never moves a point further than 5, keeping its direction", () => {
    const y = new Float64Array(4);
    const update = new Float64Array(4);
    const gains = new Float64Array(4).fill(1);
    // A gradient of (-300, 400) at rate 1 would be a step of length about 600 x gain.
    step(y, 2, Float64Array.from([-300, 400, 0.001, 0]), update, gains, 1);
    expect(Math.hypot(update[0], update[1])).toBeCloseTo(MAX_STEP, 12);
    expect(update[0] / update[1]).toBeCloseTo(300 / -400, 12);
    // A step under the limit is left alone.
    expect(Math.hypot(update[2], update[3])).toBeLessThan(MAX_STEP);
    expect(update[2]).toBeCloseTo(-1.2 * 0.001, 12);
  });

  it("leaves the picture centred on zero", () => {
    const n = 5;
    const y = Float64Array.from([3, 4, -1, 2, 7, 7, 0, -9, 2, 2]);
    const update = new Float64Array(2 * n);
    const gains = new Float64Array(2 * n).fill(1);
    step(y, n, Float64Array.from([0.1, 0, 0.3, 0, -0.2, 0, 0, 0.5, 0, 0]), update, gains, 1);
    let sx = 0;
    let sy = 0;
    for (let i = 0; i < n; i++) {
      sx += y[2 * i];
      sy += y[2 * i + 1];
    }
    expect(Math.abs(sx)).toBeLessThan(1e-12);
    expect(Math.abs(sy)).toBeLessThan(1e-12);
  });
});

describe("the schedule and the start", () => {
  it("exaggerates the attraction 12 times for 250 steps, then 500 steps plain; rate is n / exaggeration", () => {
    expect(EARLY_ITERATIONS).toBe(250);
    expect(MAIN_ITERATIONS).toBe(500);
    expect(TOTAL_ITERATIONS).toBe(750);
    expect(schedule(0, 1200)).toEqual({ exaggeration: 12, rate: 100 });
    expect(schedule(249, 1200)).toEqual({ exaggeration: 12, rate: 100 });
    expect(schedule(250, 1200)).toEqual({ exaggeration: 1, rate: 1200 });
    expect(schedule(749, 1200)).toEqual({ exaggeration: 1, rate: 1200 });
  });

  it("pins openTSNE 1.0.4's other constants (theta, momentum, the 12, the seed, the 1e-4 start, the 1% jitter)", () => {
    expect(THETA).toBe(0.5);
    expect(MOMENTUM).toBe(0.8);
    expect(EARLY_EXAGGERATION).toBe(12);
    expect(MAX_STEP).toBe(5);
    expect(SEED).toBe(42);
    expect(INIT_STD).toBe(1e-4);
    expect(JITTER).toBe(0.01);
    expect(REPORT_EVERY).toBe(25);
  });

  it("starts from the top two axes, scaled by ONE factor so axis 0 has standard deviation 1e-4, plus 1% jitter", () => {
    const c = byName("overlapping-300x32");
    // The centred vectors, as tsne() hands them to the start.
    const mean = new Float64Array(c.dimension);
    for (let i = 0; i < c.n; i++)
      for (let j = 0; j < c.dimension; j++) mean[j] += c.vectors[i * c.dimension + j] / c.n;
    const x = new Float64Array(c.n * c.dimension);
    for (let i = 0; i < c.n; i++)
      for (let j = 0; j < c.dimension; j++)
        x[i * c.dimension + j] = c.vectors[i * c.dimension + j] - mean[j];
    const y = initialPicture(x, c.n, c.dimension);
    const axes = principalScores(x, c.n, c.dimension, 2);
    const std0 = Math.sqrt(axes.variance[0] / c.n);
    const factor = 1e-4 / std0;
    let residual = 0;
    let own = 0;
    for (let i = 0; i < c.n; i++) {
      for (let a = 0; a < 2; a++) {
        const clean = axes.scores[i * 2 + a] * factor;
        residual += (y[2 * i + a] - clean) ** 2;
        own += clean * clean;
      }
    }
    // The jitter has the standard deviation of 1% of axis 0's: 1e-6 per coordinate.
    expect(Math.sqrt(residual / (2 * c.n))).toBeGreaterThan(0.7e-6);
    expect(Math.sqrt(residual / (2 * c.n))).toBeLessThan(1.3e-6);
    // Axis 0 at 1e-4 (give or take the jitter), axis 1 smaller by the same ratio the axes had.
    let s0 = 0;
    let s1 = 0;
    for (let i = 0; i < c.n; i++) {
      s0 += y[2 * i] ** 2;
      s1 += y[2 * i + 1] ** 2;
    }
    expect(Math.sqrt(s0 / c.n)).toBeGreaterThan(0.99e-4);
    expect(Math.sqrt(s0 / c.n)).toBeLessThan(1.02e-4);
    const ratio = Math.sqrt(axes.variance[1] / axes.variance[0]);
    expect(Math.abs(Math.sqrt(s1 / s0) - ratio)).toBeLessThan(0.02 * ratio);
    expect(own).toBeGreaterThan(0);
  });

  it("starts the same way twice, and still gives a finite start when every document is alike", () => {
    const x = cloud(30, 5, 3);
    expect(initialPicture(x, 30, 5)).toEqual(initialPicture(x, 30, 5));
    const flat = initialPicture(new Float64Array(30 * 5), 30, 5);
    for (const v of flat) expect(Number.isFinite(v)).toBe(true);
    expect(Math.max(...flat.map(Math.abs))).toBeGreaterThan(0);
  });
});

describe("a whole run", () => {
  const c = byName("separated-300x64");

  it("is bit-identical on the same input, and leaves its input alone", () => {
    const copy = c.vectors.slice();
    const a = tsne(c.vectors, c.n, c.dimension);
    const b = tsne(c.vectors, c.n, c.dimension);
    expect(a.coords).toEqual(b.coords);
    expect(c.vectors).toEqual(copy);
  });

  it("gives finite output for identical vectors, for repeated vectors, and for zero vectors", () => {
    const same = new Float32Array(40 * 8).fill(0.25);
    const repeats = new Float32Array(90 * 8);
    const rand = seededRandom(5);
    for (let i = 0; i < 60; i++) for (let j = 0; j < 8; j++) repeats[i * 8 + j] = rand() - 0.5;
    for (let i = 60; i < 90; i++) repeats.copyWithin(i * 8, (i - 60) * 8, (i - 60) * 8 + 8);
    const zeros = new Float32Array(30 * 8);
    for (const vectors of [same, repeats, zeros]) {
      const result = tsne(vectors, vectors.length / 8, 8);
      expect(result.method).toBe("tsne");
      for (const v of result.coords) {
        expect(Number.isFinite(v)).toBe(true);
        expect(v).toBeGreaterThanOrEqual(0);
        expect(v).toBeLessThanOrEqual(1);
      }
    }
  });

  it("refuses a value that is not a finite number, and a shape that does not add up", () => {
    for (const bad of [NaN, Infinity, -Infinity]) {
      const v = c.vectors.slice();
      v[1234] = bad;
      expect(() => tsne(v, c.n, c.dimension)).toThrow(/not a number/);
    }
    expect(() => tsne(c.vectors, c.n + 1, c.dimension)).toThrow(/count and dimension/);
    expect(() => tsne(new Float32Array(10), 2.5, 4)).toThrow(/count and dimension/);
    expect(() => tsne(new Float32Array(0), -1, 0)).toThrow();
  });

  it("gives three documents or fewer the same spread pca.ts gives, and no documents none", () => {
    for (const n of [0, 1, 2, 3]) {
      const v = Float32Array.from({ length: n * 4 }, (_, i) => i * 0.37 - 1);
      const got = tsne(v, n, 4);
      const want = reduceToUnit(v, n, 4);
      expect(got.coords).toEqual(want.coords);
      expect(got.method).toBe(want.method);
    }
  });

  it("gives finite, in-range output at the smallest real sizes (4, 5 and 8 documents)", () => {
    for (const n of [4, 5, 8]) {
      const v = Float32Array.from(cloud(n, 6, 40 + n));
      const got = tsne(v, n, 6);
      expect(got.method).toBe("tsne");
      for (const x of got.coords) {
        expect(Number.isFinite(x)).toBe(true);
        expect(x).toBeGreaterThanOrEqual(0);
        expect(x).toBeLessThanOrEqual(1);
      }
    }
  });

  it("reports progress every 25 steps (0, then 25/750 to 1) and stops when cancelled", () => {
    const seen: number[] = [];
    tsne(c.vectors, c.n, c.dimension, { onProgress: (f) => seen.push(f) });
    expect(seen.length).toBe(1 + TOTAL_ITERATIONS / REPORT_EVERY);
    expect(seen[0]).toBe(0);
    expect(seen[1]).toBeCloseTo(25 / 750, 12);
    expect(seen[seen.length - 1]).toBe(1);
    for (let i = 1; i < seen.length; i++) expect(seen[i]).toBeGreaterThan(seen[i - 1]);

    // Cancelled once 10 per cent is reported: it stops there, with nothing returned.
    const reached: number[] = [];
    expect(() =>
      tsne(c.vectors, c.n, c.dimension, {
        onProgress: (f) => reached.push(f),
        isCancelled: () => reached.length > 0 && reached[reached.length - 1] >= 0.1,
      }),
    ).toThrow(TsneCancelled);
    expect(reached[reached.length - 1]).toBeCloseTo(0.1, 12);
  });

  it("is cancelled before it starts when asked at once", () => {
    expect(() => tsne(c.vectors, c.n, c.dimension, { isCancelled: () => true })).toThrow(
      TsneCancelled,
    );
  });
});
