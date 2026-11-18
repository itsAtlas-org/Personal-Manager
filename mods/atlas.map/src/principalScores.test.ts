// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The top-K axes (pca.ts `principalScores`), the 50-axis pre-step of the enhanced layout, against numpy's SVD
// (fixtures/pca50-numpy.json: how much variance numpy puts in its top 50 axes of the fixture's wide cases) and
// against data whose singular values are known exactly.

import { describe, expect, it } from "vitest";
import pca50 from "./fixtures/pca50-numpy.json";
import { principalScores, seededRandom } from "./pca";
import { tsneCases } from "./tsneFixture";

const sum = (v: ArrayLike<number>) => Array.from(v as ArrayLike<number>).reduce((s, x) => s + x, 0);

describe("the top 50 axes against numpy", () => {
  it.each(pca50.cases.map((c) => c.name))(
    "%s: captures at least 99 per cent of numpy's top-50 variance",
    (name) => {
      const want = pca50.cases.find((c) => c.name === name)!;
      const c = tsneCases.find((x) => x.name === name)!;
      expect(c.n).toBe(want.n);
      expect(c.dimension).toBe(want.dimension);
      const got = principalScores(c.vectors, c.n, c.dimension, 50);
      expect(got.axes).toBe(50);
      const captured = sum(got.variance);
      expect(captured).toBeGreaterThanOrEqual(0.99 * want.top50Variance);
      // No axis set can hold more than the best one (the small allowance is float32 -> float64 rounding).
      expect(captured).toBeLessThanOrEqual(want.top50Variance * (1 + 1e-9));
      // The first two axes are the ones the start of the layout uses: they must be numpy's, not near.
      expect(got.variance[0] + got.variance[1]).toBeGreaterThanOrEqual(0.999 * want.top2Variance);
    },
  );

  it("covers every fixture case wider than 50 dimensions", () => {
    const wide = tsneCases.filter((c) => c.dimension > 50).map((c) => c.name);
    expect(pca50.cases.map((c) => c.name).sort()).toEqual(wide.sort());
  });
});

/** `cols` orthonormal vectors of length `rows`, each also orthogonal to `avoid` (when given). */
function orthonormalColumns(
  rows: number,
  cols: number,
  rand: () => number,
  avoid?: Float64Array,
): Float64Array[] {
  const out: Float64Array[] = avoid ? [avoid] : [];
  const first = out.length;
  while (out.length < first + cols) {
    const v = Float64Array.from({ length: rows }, () => rand() - 0.5);
    for (let pass = 0; pass < 2; pass++) {
      for (const u of out) {
        let along = 0;
        for (let i = 0; i < rows; i++) along += v[i] * u[i];
        for (let i = 0; i < rows; i++) v[i] -= along * u[i];
      }
    }
    const len = Math.hypot(...v);
    out.push(v.map((x) => x / len));
  }
  return out.slice(first);
}

describe("the top axes of data whose singular values are known", () => {
  // X = U diag(s) V': U has orthonormal columns, every one orthogonal to the constant vector, and V is a
  // random rotation, so X is mean-free and its singular values are exactly s. A slowly falling spectrum
  // (s_j = 4 / sqrt(j + 1)) is the hard case for subspace iteration.
  const n = 300;
  const d = 120;
  const rand = seededRandom(99);
  const s = Array.from({ length: d }, (_, j) => 4 / Math.sqrt(j + 1));
  const constant = new Float64Array(n).fill(1 / Math.sqrt(n));
  const u = orthonormalColumns(n, d, rand, constant);
  const v = orthonormalColumns(d, d, rand);
  const x = new Float32Array(n * d);
  for (let i = 0; i < n; i++) {
    for (let j = 0; j < d; j++) {
      let acc = 0;
      for (let a = 0; a < d; a++) acc += u[a][i] * s[a] * v[a][j];
      x[i * d + j] = acc;
    }
  }

  it("captures at least 99 per cent of the exact top-50 variance, and never more than there is", () => {
    const exact = s.slice(0, 50).reduce((t, a) => t + a * a, 0);
    const got = principalScores(x, n, d, 50);
    // (float32 storage of x costs about 1e-7 of relative accuracy, hence the small allowance above 1.)
    expect(sum(got.variance)).toBeGreaterThanOrEqual(0.99 * exact);
    expect(sum(got.variance)).toBeLessThanOrEqual(exact * (1 + 1e-5));
  });
});

describe("the shape of what it returns", () => {
  const c = tsneCases.find((x) => x.name === "wide-150x96")!;
  const got = principalScores(c.vectors, c.n, c.dimension, 10);

  it("gives centred, uncorrelated columns, largest variance first", () => {
    expect(got.axes).toBe(10);
    for (let a = 0; a < 10; a++) {
      let mean = 0;
      for (let i = 0; i < c.n; i++) mean += got.scores[i * 10 + a];
      expect(Math.abs(mean / c.n)).toBeLessThan(1e-9);
      if (a > 0) expect(got.variance[a]).toBeLessThanOrEqual(got.variance[a - 1]);
      for (let b = a + 1; b < 10; b++) {
        let dot = 0;
        for (let i = 0; i < c.n; i++) dot += got.scores[i * 10 + a] * got.scores[i * 10 + b];
        expect(Math.abs(dot) / Math.sqrt(got.variance[a] * got.variance[b])).toBeLessThan(1e-6);
      }
    }
  });

  it("fixes each column's sign by its largest score, and is the same twice", () => {
    for (let a = 0; a < 10; a++) {
      let at = 0;
      for (let i = 1; i < c.n; i++) {
        if (Math.abs(got.scores[i * 10 + a]) > Math.abs(got.scores[at * 10 + a])) at = i;
      }
      expect(got.scores[at * 10 + a]).toBeGreaterThan(0);
    }
    expect(principalScores(c.vectors, c.n, c.dimension, 10).scores).toEqual(got.scores);
  });

  it("returns as many axes as there are, when asked for more than the data has", () => {
    const few = principalScores(
      Float32Array.from({ length: 5 * 100 }, (_, i) => Math.sin(i * 1.7)),
      5,
      100,
      50,
    );
    expect(few.axes).toBe(5);
    for (const v of few.scores) expect(Number.isFinite(v)).toBe(true);
    expect(principalScores(new Float32Array(0), 0, 0, 50).axes).toBe(0);
    const flat = principalScores(new Float32Array(20 * 6).fill(2), 20, 6, 3);
    for (const v of flat.scores) expect(v).toBe(0);
  });

  it("refuses a value that is not a number and a shape that does not add up", () => {
    const v = c.vectors.slice();
    v[7] = NaN;
    expect(() => principalScores(v, c.n, c.dimension, 10)).toThrow(/not a number/);
    expect(() => principalScores(c.vectors, c.n + 1, c.dimension, 10)).toThrow(
      /count and dimension/,
    );
  });
});
