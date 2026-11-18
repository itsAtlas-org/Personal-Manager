// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The JS projection against the numpy one it replaces. fixtures/pca-numpy.json holds vectors from a
// seeded numpy generator and, for each case, what the sidecar's own `do_reduce` (the "pca" path) made
// of them; its `script` field is the program that made it. The two agree up to the SIGN of each axis,
// which numpy leaves to its SVD and this code fixes (see pca.ts), so the comparison picks the better
// mirror per axis, and a separate set of tests pins the rule itself.

import { describe, expect, it } from "vitest";
import fixture from "./fixtures/pca-numpy.json";
import { MAX_ITERATIONS, reduceToUnit } from "./pca";

interface Case {
  name: string;
  note: string;
  vectors: number[][];
  expected: number[][];
  method: string;
}
const cases = fixture.cases as Case[];
const byName = (name: string) => cases.find((c) => c.name === name)!;

function flatten(rows: number[][], dimension: number): Float32Array {
  const out = new Float32Array(rows.length * dimension);
  rows.forEach((row, i) => out.set(row, i * dimension));
  return out;
}

function reduceCase(c: Case) {
  const dimension = c.vectors[0]?.length ?? 0;
  return reduceToUnit(flatten(c.vectors, dimension), c.vectors.length, dimension);
}

/** The largest difference on one axis, taking the better of the axis and its mirror image. */
function axisError(got: Float64Array, expected: number[][], axis: 0 | 1): number {
  let straight = 0;
  let mirrored = 0;
  expected.forEach((row, i) => {
    straight = Math.max(straight, Math.abs(got[2 * i + axis] - row[axis]));
    mirrored = Math.max(mirrored, Math.abs(1 - got[2 * i + axis] - row[axis]));
  });
  return Math.min(straight, mirrored);
}

/** A seeded cloud with two clearly different spreads, so the top two axes are well separated. */
function cloud(n: number, d: number, seed: number): Float32Array {
  let a = seed >>> 0;
  const rand = () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  const out = new Float32Array(n * d);
  for (let i = 0; i < n; i++) {
    for (let j = 0; j < d; j++) {
      const scale = j === 0 ? 5 : j === 1 ? 2.5 : 0.3;
      out[i * d + j] = (rand() - 0.5) * scale;
    }
  }
  return out;
}

describe("the numpy fixture", () => {
  it("has the cases the tests below rely on, made by numpy and the sidecar's own reducer", () => {
    expect(fixture.numpy).toMatch(/^\d+\.\d+/);
    expect(cases.map((c) => c.name)).toEqual([
      "empty",
      "one",
      "three",
      "four",
      "clusters-24x8",
      "clusters-60x32",
      "clusters-80x128",
      "identical-6x8",
      "line-exact-8x6",
    ]);
    expect(fixture.script.join("\n")).toContain("mod.do_reduce");
  });
});

describe("reduceToUnit against numpy's reducer", () => {
  it.each(["four", "clusters-24x8", "clusters-60x32", "clusters-80x128"])(
    "%s: both axes match numpy's coordinates up to the axis' sign",
    (name) => {
      const c = byName(name);
      const { coords, method } = reduceCase(c);
      expect(method).toBe(c.method);
      expect(coords).toHaveLength(2 * c.vectors.length);
      expect(axisError(coords, c.expected, 0)).toBeLessThan(2e-4);
      expect(axisError(coords, c.expected, 1)).toBeLessThan(2e-4);
    },
  );

  it("stays inside [0,1] and spans it on both axes", () => {
    const { coords } = reduceCase(byName("clusters-60x32"));
    for (const axis of [0, 1]) {
      const values = Array.from(coords).filter((_, i) => i % 2 === axis);
      expect(Math.min(...values)).toBe(0);
      expect(Math.max(...values)).toBe(1);
    }
  });

  it("all alike: nothing to separate, every document at the origin (as numpy)", () => {
    const c = byName("identical-6x8");
    expect(c.expected.flat().every((v) => v === 0)).toBe(true);
    const { coords, method } = reduceCase(c);
    expect(method).toBe("pca");
    expect(Array.from(coords)).toEqual(new Array(12).fill(0));
  });

  it("on exactly one line: the first axis is numpy's, the second is a real zero", () => {
    const c = byName("line-exact-8x6");
    const { coords } = reduceCase(c);
    expect(axisError(coords, c.expected, 0)).toBeLessThan(2e-4);
    expect(c.expected.every((row) => row[1] === 0)).toBe(true);
    for (let i = 0; i < 8; i++) expect(coords[2 * i + 1]).toBe(0);
  });

  it("a second axis that is only noise beside the first is a real zero, not noise stretched to fill [0,1]", () => {
    // Twenty documents spread over a thousand units on one column; the next column holds a rounding-
    // sized wobble (1e-4), a hundred-millionth of the first axis' size. numpy's absolute test
    // (a span over 1e-9) would stretch that wobble over the whole second axis.
    const n = 20;
    const d = 4;
    const v = new Float32Array(n * d);
    for (let i = 0; i < n; i++) {
      v[i * d] = (i * 1000) / (n - 1);
      v[i * d + 1] = (i % 2 === 0 ? 1 : -1) * 1e-4 * (1 + (i % 3));
    }
    const { coords } = reduceToUnit(v, n, d);
    for (let i = 0; i < n; i++) expect(coords[2 * i + 1]).toBe(0);
    // The first axis is still the spread: in order, end to end.
    expect(coords[0]).toBe(0);
    expect(coords[2 * (n - 1)]).toBe(1);
  });

  it("differences under a billionth are no layout at all: every document at the origin", () => {
    // The old code's own rule (a span of 1e-9 or less is not scaled): vectors that differ by float
    // dust must not be stretched into a scatter. The dust here is 1e-12.
    const n = 10;
    const d = 4;
    const v = new Float32Array(n * d);
    for (let i = 0; i < n; i++) {
      v[i * d] = 1;
      v[i * d + 1] = (i % 2 === 0 ? 1 : -1) * 1e-12;
      v[i * d + 2] = (i % 3 === 0 ? 1 : -1) * 1e-12;
    }
    expect(Array.from(reduceToUnit(v, n, d).coords)).toEqual(new Array(2 * n).fill(0));
  });

  it("a small but real spread is scaled to fill the box: the layout does not depend on the vectors' scale", () => {
    const d = 6;
    const n = 40;
    const big = cloud(n, d, 13);
    const small = big.map((x) => x * 1e-5);
    const a = reduceToUnit(big, n, d).coords;
    const b = reduceToUnit(small, n, d).coords;
    for (let i = 0; i < a.length; i++) expect(Math.abs(a[i] - b[i])).toBeLessThan(1e-3);
  });

  it("nothing: no coordinates, method none (numpy's answer)", () => {
    const c = byName("empty");
    expect(c.method).toBe("none");
    const { coords, method } = reduceToUnit(new Float32Array(0), 0, 8);
    expect(method).toBe("none");
    expect(coords).toHaveLength(0);
  });
});

describe("one to three documents", () => {
  it("numpy's sidecar spread them 0, 1, 2 on the x axis (outside the range it promises)", () => {
    expect(byName("one").expected).toEqual([[0, 0]]);
    expect(byName("three").expected).toEqual([
      [0, 0],
      [1, 0],
      [2, 0],
    ]);
    expect(byName("three").method).toBe("trivial");
  });

  it("the port spreads them along the middle of the unit box instead, in order", () => {
    const three = reduceCase(byName("three"));
    expect(three.method).toBe("trivial");
    expect(Array.from(three.coords)).toEqual([0, 0.5, 0.5, 0.5, 1, 0.5]);
    const two = reduceToUnit(new Float32Array([1, 2, 3, 4]), 2, 2);
    expect(Array.from(two.coords)).toEqual([0, 0.5, 1, 0.5]);
    const one = reduceCase(byName("one"));
    expect(Array.from(one.coords)).toEqual([0.5, 0.5]);
  });

  it("four is the first real projection", () => {
    expect(reduceCase(byName("four")).method).toBe("pca");
  });
});

describe("the sign of an axis", () => {
  it("follows the data's own orientation: column 0 is the strong axis, so x rises with it, and y with column 1", () => {
    const d = 6;
    const n = 80;
    const v = cloud(n, d, 11);
    const { coords } = reduceToUnit(v, n, d);
    const correlation = (xs: number[], ys: number[]) => {
      const mean = (a: number[]) => a.reduce((s, x) => s + x, 0) / a.length;
      const [mx, my] = [mean(xs), mean(ys)];
      let sxy = 0;
      let sxx = 0;
      let syy = 0;
      xs.forEach((x, i) => {
        sxy += (x - mx) * (ys[i] - my);
        sxx += (x - mx) ** 2;
        syy += (ys[i] - my) ** 2;
      });
      return sxy / Math.sqrt(sxx * syy);
    };
    const column = (j: number) => Array.from({ length: n }, (_, i) => v[i * d + j]);
    const axis = (k: number) => Array.from({ length: n }, (_, i) => coords[2 * i + k]);
    expect(correlation(axis(0), column(0))).toBeGreaterThan(0.95);
    expect(correlation(axis(1), column(1))).toBeGreaterThan(0.95);
  });

  it("is a rule about the loadings, so reflecting the data mirrors the map (x becomes 1 - x)", () => {
    const d = 6;
    const a = cloud(40, d, 7);
    const b = new Float32Array(a);
    for (let i = 0; i < 40; i++) b[i * d] = -b[i * d];
    const ra = reduceToUnit(a, 40, d);
    const rb = reduceToUnit(b, 40, d);
    for (let i = 0; i < 40; i++) {
      expect(rb.coords[2 * i]).toBeCloseTo(1 - ra.coords[2 * i], 4);
      expect(rb.coords[2 * i + 1]).toBeCloseTo(ra.coords[2 * i + 1], 4);
    }
  });

  it("does not flip when a few documents are added: the shared documents keep their sides", () => {
    const d = 12;
    const all = cloud(210, d, 21);
    const first = reduceToUnit(all.subarray(0, 200 * d), 200, d);
    const more = reduceToUnit(all, 210, d);
    // The coordinates of the 200 shared documents correlate positively on both axes.
    for (const axis of [0, 1]) {
      let sa = 0;
      let sb = 0;
      let sab = 0;
      let saa = 0;
      let sbb = 0;
      for (let i = 0; i < 200; i++) {
        const x = first.coords[2 * i + axis];
        const y = more.coords[2 * i + axis];
        sa += x;
        sb += y;
        sab += x * y;
        saa += x * x;
        sbb += y * y;
      }
      const n = 200;
      const r = (sab - (sa * sb) / n) / Math.sqrt((saa - (sa * sa) / n) * (sbb - (sb * sb) / n));
      expect(r).toBeGreaterThan(0.9);
    }
  });
});

describe("determinism", () => {
  it("the same vectors give the same coordinates, to the last bit, run after run", () => {
    const v = cloud(150, 16, 3);
    const a = reduceToUnit(v, 150, 16).coords;
    const b = reduceToUnit(v.slice(), 150, 16).coords;
    expect(Array.from(b)).toEqual(Array.from(a));
  });

  it("stops as soon as the axes settle: well-separated data takes a handful of steps, not the bound", () => {
    const probe = { iterations: 0 };
    reduceToUnit(cloud(300, 16, 17), 300, 16, probe);
    expect(probe.iterations).toBeGreaterThan(1);
    expect(probe.iterations).toBeLessThan(40);
  });

  it("never takes more than the bound", () => {
    // Two columns of equal variance: no gap to converge on.
    const n = 100;
    const v = new Float32Array(n * 3);
    for (let i = 0; i < n; i++) {
      v[i * 3] = Math.cos(i);
      v[i * 3 + 1] = Math.sin(i);
    }
    const probe = { iterations: 0 };
    reduceToUnit(v, n, 3, probe);
    expect(probe.iterations).toBeLessThanOrEqual(MAX_ITERATIONS);
  });

  it("does not change the vectors it was given", () => {
    const v = cloud(50, 8, 5);
    const copy = v.slice();
    reduceToUnit(v, 50, 8);
    expect(Array.from(v)).toEqual(Array.from(copy));
  });

  it("rounds to 5 decimals, so a stored coordinate reads back as the number it was", () => {
    const { coords } = reduceToUnit(cloud(80, 8, 9), 80, 8);
    for (const v of coords) expect(Number(JSON.stringify(v))).toBe(v);
    for (const v of coords) expect(Math.round(v * 1e5) / 1e5).toBe(v);
  });

  it("is bounded: it stops after MAX_ITERATIONS even when the two top axes are nearly equal", () => {
    // Equal variance on two columns: the plane is determined, the axes inside it are not. It must
    // still finish (and finish the same way twice).
    const d = 4;
    const n = 100;
    const v = new Float32Array(n * d);
    for (let i = 0; i < n; i++) {
      v[i * d] = Math.cos(i);
      v[i * d + 1] = Math.sin(i);
    }
    const a = reduceToUnit(v, n, d).coords;
    expect(a.every((x) => x >= 0 && x <= 1)).toBe(true);
    expect(Array.from(reduceToUnit(v.slice(), n, d).coords)).toEqual(Array.from(a));
    expect(MAX_ITERATIONS).toBeGreaterThan(50);
  });
});

describe("a shape that does not add up, or a value that is not a number, is refused", () => {
  it.each([
    ["too few floats", () => reduceToUnit(new Float32Array(10), 4, 3)],
    ["too many floats", () => reduceToUnit(new Float32Array(20), 4, 3)],
    ["a negative count", () => reduceToUnit(new Float32Array(0), -1, 3)],
    ["a fractional count", () => reduceToUnit(new Float32Array(0), 1.5, 3)],
    ["a NaN", () => reduceToUnit(new Float32Array([1, 2, 3, 4, 5, NaN, 7, 8]), 4, 2)],
    ["an Infinity", () => reduceToUnit(new Float32Array([1, 2, 3, 4, 5, Infinity, 7, 8]), 4, 2)],
  ])("%s", (_name, run) => {
    expect(run).toThrow();
  });

  it("one dimension is a line: x is the document's place on it, y is zero", () => {
    const { coords } = reduceToUnit(new Float32Array([4, 1, 3, 2, 5]), 5, 1);
    expect(Array.from(coords).filter((_, i) => i % 2 === 1)).toEqual([0, 0, 0, 0, 0]);
    const xs = Array.from(coords).filter((_, i) => i % 2 === 0);
    expect(Math.min(...xs)).toBe(0);
    expect(Math.max(...xs)).toBe(1);
    // The order along the line is the order of the values (or its mirror: 4,1,3,2,5 -> 3/4 0 1/2 1/4 1).
    const ranks = xs.map((x) => Math.round(x * 4));
    expect(ranks).toEqual([3, 0, 2, 1, 4]);
  });
});
