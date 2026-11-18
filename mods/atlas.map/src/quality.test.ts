// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The scorer, proven before any JavaScript t-SNE is judged by it: on openTSNE's own stored pictures it must
// give the numbers scikit-learn gave (fixtures/tsne-opentsne.json), and on small shapes whose answer is
// obvious it must give that answer.

import { describe, expect, it } from "vitest";
import { scoreNeighbours, scorePicture } from "./quality";
import { tsneCases } from "./tsneFixture";

describe("the scorer against scikit-learn, on openTSNE's own pictures", () => {
  it.each(tsneCases.map((c) => c.name))(
    "%s: trustworthiness, recall and purity match to 1e-9",
    (name) => {
      const c = tsneCases.find((x) => x.name === name)!;
      const got = scorePicture(c.vectors, c.coords, c.labels, c.n, c.dimension, c.scoreNeighbours);
      expect(Math.abs(got.trustworthiness - c.scores.tsne.trustworthiness)).toBeLessThan(1e-9);
      expect(Math.abs(got.recall - c.scores.tsne.recall)).toBeLessThan(1e-9);
      expect(Math.abs(got.purity - c.scores.tsne.purity)).toBeLessThan(1e-9);
    },
    60_000,
  );

  it("uses scikit-learn's own limit for k when none is given", () => {
    for (const c of tsneCases) expect(scoreNeighbours(c.n), c.name).toBe(c.scoreNeighbours);
    const c = tsneCases.find((x) => x.name === "separated-300x64")!;
    const explicit = scorePicture(c.vectors, c.coords, c.labels, c.n, c.dimension, 10);
    expect(scorePicture(c.vectors, c.coords, c.labels, c.n, c.dimension)).toEqual(explicit);
  });
});

/**
 * Documents on a quarter circle at unevenly spaced angles, so no document has two others at the same distance:
 * cosine distance grows with the angle between two of them, so the order along the arc IS the neighbourhood.
 */
function arc(angles: number[]) {
  const x = new Float64Array(2 * angles.length);
  angles.forEach((a, i) => {
    x[2 * i] = Math.cos(a);
    x[2 * i + 1] = Math.sin(a);
  });
  return x;
}
const line = (positions: number[]) => Float64Array.from(positions.flatMap((p) => [p, 0]));

describe("the scorer on shapes with a known answer", () => {
  const n = 40;
  const angles = Array.from({ length: n }, (_, i) => (Math.PI / 2) * (i / (n - 1)) ** 1.5);
  const labels = Array.from({ length: n }, (_, i) => Math.floor(i / 10));

  it("a picture that keeps the order scores perfectly", () => {
    const s = scorePicture(arc(angles), line(angles), labels, n, 2, 5);
    expect(s.trustworthiness).toBe(1);
    expect(s.recall).toBe(1);
    // Labels run in blocks of ten along the line: a document at a block's edge has neighbours across the
    // edge, so purity is high but not 1.
    expect(s.purity).toBeGreaterThan(0.85);
    expect(s.purity).toBeLessThan(1);
  });

  it("a shuffled picture scores far lower on all three", () => {
    // 17 and 40 share no factor, so this is a permutation.
    const shuffled = angles.map((_, i) => angles[(i * 17) % n]);
    const s = scorePicture(arc(angles), line(shuffled), labels, n, 2, 5);
    expect(s.trustworthiness).toBeLessThan(0.75);
    expect(s.recall).toBeLessThan(0.3);
    expect(s.purity).toBeLessThan(0.5);
  });

  it("counts the penalty by how far out of place a document is (hand-worked, n = 6, k = 2)", () => {
    // Six documents on the arc at angles 0, .1, .25, .45, .7, 1.0 (rows of original ranks, nearest first:
    // doc 0: 1 2 3 4 5; doc 1: 0 2 3 4 5; doc 2: 1 3 0 4 5; doc 3: 2 4 1 0 5; doc 4: 3 5 2 1 0; doc 5: 4 3 2 1 0).
    // The picture puts them at x = 5.0, 1.0, 2.2, 3.5, 4.2, 0.0 and each document's two nearest there are:
    //  doc 0: 4, 3 -> original ranks 4, 3   penalty (rank - 2, if positive) 2 + 1 = 3
    //  doc 1: 5, 2 -> ranks 5, 2            3 + 0 = 3
    //  doc 2: 1, 3 -> ranks 1, 2            0
    //  doc 3: 4, 2 -> ranks 2, 1            0
    //  doc 4: 3, 0 -> ranks 1, 5            0 + 3 = 3
    //  doc 5: 1, 2 -> ranks 4, 3            2 + 1 = 3
    // Penalty 12, so T = 1 - 12 * 2 / (6 * 2 * (12 - 6 - 1)) = 1 - 24 / 60 = 0.6.
    // Recall: the true two nearest against the picture's two nearest share 0, 1, 2, 2, 1, 0 documents
    // (doc 0: {1,2} vs {4,3} none; doc 1: {0,2} vs {5,2} one; doc 2: {1,3} both; doc 3: {2,4} both;
    // doc 4: {3,5} vs {3,0} one; doc 5: {4,3} vs {1,2} none) = 6 / 12 = 0.5.
    const s = scorePicture(
      arc([0, 0.1, 0.25, 0.45, 0.7, 1.0]),
      line([5.0, 1.0, 2.2, 3.5, 4.2, 0.0]),
      [0, 0, 0, 1, 1, 1],
      6,
      2,
      2,
    );
    expect(s.trustworthiness).toBeCloseTo(0.6, 12);
    expect(s.recall).toBeCloseTo(0.5, 12);
    // Purity: labels 0 0 0 1 1 1; picture neighbours same-label: doc0 {4,3} 0 of 2; doc1 {5,2} 1; doc2 {1,3} 1;
    // doc3 {4,2} 1; doc4 {3,0} 1; doc5 {1,2} 0 -> 4 / 12.
    expect(s.purity).toBeCloseTo(4 / 12, 12);
  });

  it("measures the original space by cosine, so how long a document's vector is changes nothing", () => {
    const stretched = arc(angles).map((v, i) => v * (1 + (Math.floor(i / 2) % 7)));
    const shuffled = angles.map((_, i) => angles[(i * 17) % n]);
    for (const y of [line(angles), line(shuffled)]) {
      expect(scorePicture(stretched, y, labels, n, 2, 5)).toEqual(
        scorePicture(arc(angles), y, labels, n, 2, 5),
      );
    }
  });

  it("refuses a k that scikit-learn refuses (k must be less than n / 2)", () => {
    expect(() => scorePicture(arc(angles), line(angles), labels, n, 2, 20)).toThrow();
    expect(() => scorePicture(arc(angles), line(angles), labels, n, 2, 0)).toThrow();
  });
});
