// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The openTSNE oracle, checked as a file. fixtures/tsne-opentsne.json was made by running the sidecar's own
// `do_reduce({"method": "tsne"})` (openTSNE) over seeded vectors, and scoring its picture with scikit-learn;
// its `script` field is the program that did it. After the old enhanced layout leaves PM nobody has openTSNE
// again, so this file is the only thing the JavaScript t-SNE can be held against. These tests keep the file
// honest: every case is a real openTSNE picture (not the PCA the sidecar quietly answers with when openTSNE
// fails), the perplexity each case ran at is the one openTSNE's rule gives, and the file has the cases the
// later tests lean on.

import { describe, expect, it } from "vitest";
import raw from "./fixtures/tsne-opentsne.json?raw";
import { tsneCases, tsneFixture } from "./tsneFixture";

const byName = (name: string) => tsneCases.find((c) => c.name === name)!;
const script = tsneFixture.script.join("\n");

describe("the openTSNE fixture file", () => {
  it("is small enough to live in the repository (under 1 MB)", () => {
    expect(raw.length).toBeLessThan(1_000_000);
  });

  it("records the versions it was made with, and how to make it again", () => {
    for (const name of ["python", "numpy", "scipy", "scikit-learn", "openTSNE"] as const) {
      expect(tsneFixture.versions[name]).toMatch(/^\d+\.\d+/);
    }
    expect(tsneFixture.howMade).toContain("make-tsne-fixture.py");
    expect(tsneFixture.scoring.join(" ")).toContain("sklearn.manifold.trustworthiness");
  });

  it("carries the script that made it, which calls the sidecar's own reducer and refuses a fall-back to PCA", () => {
    expect(script).toContain("mod.do_reduce");
    expect(script).toContain('"method": "tsne"');
    // The sidecar's `_reduce` swallows every error and answers PCA labelled "pca": the script must fail
    // on any case that did not report "tsne", or PCA would silently become the target.
    expect(script).toContain('assert out["method"] == "tsne"');
    expect(script).toContain("trustworthiness");
  });

  it("has the cases the t-SNE tests rely on", () => {
    expect(tsneCases.map((c) => c.name)).toEqual([
      "separated-300x64",
      "overlapping-300x32",
      "curved-300x16",
      "tiny-4x8",
      "tiny-8x8",
      "tiny-20x16",
      "wide-150x96",
      "large-1200x32",
    ]);
    // The large one is past openTSNE's switch to approximate neighbours (n >= 1,000).
    expect(byName("large-1200x32").n).toBe(1200);
    // Tiny n for the perplexity clamp.
    expect([4, 8, 20].map((n) => tsneCases.some((c) => c.n === n))).toEqual([true, true, true]);
    // Wide input, so the sidecar's PCA-50 step ran.
    expect(byName("wide-150x96").dimension).toBeGreaterThan(50);
    expect(byName("wide-150x96").pcaStep).toBe(true);
    expect(byName("overlapping-300x32").pcaStep).toBe(false);
  });

  it("holds an openTSNE picture in every case, never the PCA fall-back", () => {
    for (const c of tsneCases) expect(c.method, c.name).toBe("tsne");
  });

  it("stores vectors, labels and coordinates that agree with each case's size", () => {
    for (const c of tsneCases) {
      expect(c.vectors, c.name).toHaveLength(c.n * c.dimension);
      expect(c.coords, c.name).toHaveLength(2 * c.n);
      expect(c.labels, c.name).toHaveLength(c.n);
      expect(c.pcaStep, c.name).toBe(c.dimension > 50);
      expect(Array.from(c.vectors).every(Number.isFinite), c.name).toBe(true);
      // Coordinates are the sidecar's own: min-max scaled into the unit box.
      let low = Infinity;
      let high = -Infinity;
      for (const v of c.coords) {
        low = Math.min(low, v);
        high = Math.max(high, v);
      }
      expect(low, c.name).toBe(0);
      expect(high, c.name).toBe(1);
    }
  });

  it("records the perplexity openTSNE really used, which is NOT the requested one on a small library", () => {
    for (const c of tsneCases) {
      // The sidecar asks for min(30, max(5, floor((n-1)/3))); openTSNE 1.0.4 takes (n-1)/3 instead when
      // 3 x that is more than n-1; the neighbours searched are min(n-1, int(3 x effective)).
      const requested = Math.min(30, Math.max(5, Math.floor((c.n - 1) / 3)));
      const effective = 3 * requested > c.n - 1 ? (c.n - 1) / 3 : requested;
      expect(c.requestedPerplexity, c.name).toBe(requested);
      expect(c.effectivePerplexity, c.name).toBe(effective);
      expect(c.neighbours, c.name).toBe(Math.min(c.n - 1, Math.floor(3 * effective)));
    }
    // The clamp is real in the tiny cases, and absent from the big ones.
    expect(byName("tiny-4x8").effectivePerplexity).toBe(1);
    expect(byName("tiny-8x8").effectivePerplexity).toBeCloseTo(7 / 3, 12);
    expect(byName("tiny-20x16").effectivePerplexity).toBe(6);
    expect(byName("large-1200x32").effectivePerplexity).toBe(30);
  });

  it("scores every picture, and openTSNE beats the sidecar's PCA wherever there is structure to find", () => {
    for (const c of tsneCases) {
      expect(c.scoreNeighbours, c.name).toBe(Math.min(10, Math.ceil(c.n / 2) - 1));
      for (const m of ["tsne", "pca"] as const) {
        for (const v of Object.values(c.scores[m])) {
          expect(v, `${c.name} ${m}`).toBeGreaterThanOrEqual(0);
          expect(v, `${c.name} ${m}`).toBeLessThanOrEqual(1);
        }
      }
    }
    for (const name of ["separated-300x64", "overlapping-300x32", "wide-150x96", "large-1200x32"]) {
      const { tsne, pca } = byName(name).scores;
      expect(tsne.trustworthiness, name).toBeGreaterThan(pca.trustworthiness);
      expect(tsne.recall, name).toBeGreaterThan(pca.recall);
    }
  });

  it("includes a case where the answer is not saturated (overlapping clusters leave room to be worse)", () => {
    const { tsne } = byName("overlapping-300x32").scores;
    expect(tsne.purity).toBeLessThan(0.9);
    expect(byName("large-1200x32").scores.tsne.purity).toBeLessThan(0.95);
  });
});
