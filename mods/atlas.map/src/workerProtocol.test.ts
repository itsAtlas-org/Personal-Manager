// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Worker's one function, `handle`, with no Worker: what it answers for each job, and that it
// answers (never throws) for anything else, because a throw in a worker strands every job waiting on it.

// @ts-expect-error no @types/node (this repo has none: the root vite.config.ts says the same)
import { runInNewContext } from "node:vm";
import { describe, expect, it } from "vitest";
import { forceRequest, buildGraph } from "./layoutModel";
import { simulate } from "./forceLayout";
import fixture from "./fixtures/pca-numpy.json";
import { reduceToUnit } from "./pca";
import { tsne } from "./tsne";
import { NO_JOB, handle, transferOf, type Interim } from "./workerProtocol";
import { documents } from "./testing";

const vectors = () => {
  const out = new Float32Array(6 * 3);
  for (let i = 0; i < out.length; i++) out[i] = Math.sin(i * 1.7) + (i % 3);
  return out;
};

describe("handle: semantic, PCA only", () => {
  it("answers with the projection pca.ts makes of the same vectors", () => {
    const v = vectors();
    const reply = handle({
      id: 4,
      kind: "semantic",
      pca: true,
      tsne: false,
      vectors: v,
      count: 6,
      dimension: 3,
    });
    expect(reply.kind).toBe("semantic");
    if (reply.kind !== "semantic") return;
    const direct = reduceToUnit(v, 6, 3);
    expect(reply.id).toBe(4);
    expect(reply.method).toBe("pca");
    expect(Array.from(reply.coords)).toEqual(Array.from(direct.coords));
  });

  it("takes vectors made by another realm's Float32Array (a message port's, in a test's jsdom page)", () => {
    const v = vectors();
    // A view whose constructor is another realm's: `instanceof Float32Array` is false for it.
    const foreign = runInNewContext("(b) => new Float32Array(b)")(v.buffer) as Float32Array;
    expect(foreign instanceof Float32Array).toBe(false);
    const reply = handle({
      id: 4,
      kind: "semantic",
      pca: true,
      tsne: false,
      vectors: foreign,
      count: 6,
      dimension: 3,
    });
    expect(reply.kind).toBe("semantic");
    if (reply.kind !== "semantic") return;
    expect(Array.from(reply.coords)).toEqual(Array.from(reduceToUnit(v, 6, 3).coords));
  });

  it("still refuses every other typed array, even one of another realm", () => {
    for (const make of ["new Float64Array(18)", "new Int32Array(18)", "new Uint8Array(18)"]) {
      const view = runInNewContext(make) as ArrayBufferView;
      expect(
        handle({
          id: 1,
          kind: "semantic",
          pca: true,
          tsne: false,
          vectors: view,
          count: 6,
          dimension: 3,
        }),
      ).toMatchObject({
        id: 1,
        kind: "error",
        message: "The layout request was not understood.",
      });
    }
    // And an object that only says it is one.
    const liar = { [Symbol.toStringTag]: "Float32Array", length: 18 };
    expect(
      handle({
        id: 1,
        kind: "semantic",
        pca: true,
        tsne: false,
        vectors: liar,
        count: 6,
        dimension: 3,
      }),
    ).toMatchObject({
      kind: "error",
    });
  });

  it("moves the coordinates' buffer to the page instead of copying it", () => {
    const reply = handle({
      id: 1,
      kind: "semantic",
      pca: true,
      tsne: false,
      vectors: vectors(),
      count: 6,
      dimension: 3,
    });
    if (reply.kind !== "semantic") throw new Error("not a reduce reply");
    expect(transferOf(reply)).toEqual([reply.coords.buffer]);
  });

  it("says what is wrong with data that does not add up, in the reply, with the job's id", () => {
    const reply = handle({
      id: 9,
      kind: "semantic",
      pca: true,
      tsne: false,
      vectors: vectors(),
      count: 7,
      dimension: 3,
    });
    expect(reply).toMatchObject({ id: 9, kind: "error" });
    expect(reply.kind === "error" && reply.message).toMatch(/do not match/);
  });

  it("says so for a value that is not a number", () => {
    const v = vectors();
    v[5] = Number.NaN;
    expect(
      handle({
        id: 2,
        kind: "semantic",
        pca: true,
        tsne: false,
        vectors: v,
        count: 6,
        dimension: 3,
      }),
    ).toMatchObject({
      id: 2,
      kind: "error",
    });
  });
});

/** Four tight groups in 6 dimensions, 12 documents each, seeded: something t-SNE has a picture of. */
function groups(): Float32Array {
  let a = 7;
  const rand = () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  const out = new Float32Array(48 * 6);
  for (let i = 0; i < 48; i++) {
    const g = i % 4;
    for (let j = 0; j < 6; j++) out[i * 6 + j] = (j === g ? 5 : 0) + (rand() - 0.5) * 0.4;
  }
  return out;
}

const semanticRequest = (pca: boolean, wantTsne: boolean, v = groups(), n = 48, d = 6) => ({
  id: 5,
  kind: "semantic" as const,
  vectors: v,
  count: n,
  dimension: d,
  pca,
  tsne: wantTsne,
});

describe("handle: semantic, PCA then t-SNE in one job", () => {
  it("posts PCA as a provisional reply, then progress, then ends with t-SNE on the same vectors", () => {
    const v = groups();
    const seen: Interim[] = [];
    const reply = handle(semanticRequest(true, true, v), (r) => seen.push(r));
    expect(seen[0]).toMatchObject({ id: 5, kind: "provisional", method: "pca" });
    expect(Array.from((seen[0] as { coords: Float64Array }).coords)).toEqual(
      Array.from(reduceToUnit(v, 48, 6).coords),
    );
    const progress = seen.slice(1);
    expect(progress.length).toBeGreaterThan(5);
    const fractions = progress.map((r) => (r as { fraction: number }).fraction);
    expect(progress.every((r) => r.kind === "progress" && r.id === 5)).toBe(true);
    expect(fractions).toEqual([...fractions].sort((x, y) => x - y));
    expect(fractions[0]).toBe(0);
    expect(fractions[fractions.length - 1]).toBe(1);
    // The job's last reply is t-SNE, of the very vectors it was sent (still there after PCA).
    expect(reply.kind).toBe("semantic");
    if (reply.kind !== "semantic") return;
    expect(reply.method).toBe("tsne");
    expect(Array.from(reply.coords)).toEqual(Array.from(tsne(v, 48, 6).coords));
  });

  it("with t-SNE only, posts no provisional reply", () => {
    const seen: Interim[] = [];
    const reply = handle(semanticRequest(false, true), (r) => seen.push(r));
    expect(seen.some((r) => r.kind === "provisional")).toBe(false);
    expect(seen.some((r) => r.kind === "progress")).toBe(true);
    expect(reply).toMatchObject({ kind: "semantic", method: "tsne" });
  });

  it("with PCA only, posts nothing before the answer, and the answer is the old projection to the bit", () => {
    const c = fixture.cases.find((x) => x.name === "clusters-60x32")!;
    const d = c.vectors[0].length;
    const flat = new Float32Array(c.vectors.length * d);
    c.vectors.forEach((row, i) => flat.set(row, i * d));
    const seen: Interim[] = [];
    const reply = handle(semanticRequest(true, false, flat, c.vectors.length, d), (r) =>
      seen.push(r),
    );
    expect(seen).toEqual([]);
    const direct = reduceToUnit(flat, c.vectors.length, d);
    expect(reply).toMatchObject({ id: 5, kind: "semantic", method: direct.method });
    if (reply.kind !== "semantic") return;
    expect(Array.from(reply.coords)).toEqual(Array.from(direct.coords));
  });

  it("three documents or fewer have no t-SNE: PCA's answer ends the job, with no provisional reply", () => {
    const seen: Interim[] = [];
    const v = new Float32Array([1, 2, 3, 4, 5, 6, 9, 8, 7]);
    const reply = handle(semanticRequest(true, true, v, 3, 3), (r) => seen.push(r));
    expect(seen).toEqual([]);
    expect(reply).toMatchObject({ kind: "semantic", method: "trivial" });
    const alone = handle(semanticRequest(false, true, v, 3, 3), (r) => seen.push(r));
    expect(alone).toMatchObject({ kind: "semantic", method: "trivial" });
  });

  it("does not change the vectors it was sent", () => {
    const v = groups();
    const before = Array.from(v);
    handle(semanticRequest(true, true, v));
    expect(Array.from(v)).toEqual(before);
  });

  it("refuses a request that asks for neither layout, or says it with something that is not a boolean", () => {
    for (const [pca, wantTsne] of [
      [false, false],
      ["yes", true],
      [true, 1],
      [undefined, undefined],
    ]) {
      expect(handle({ ...semanticRequest(true, true), pca, tsne: wantTsne })).toMatchObject({
        id: 5,
        kind: "error",
        message: "The layout request was not understood.",
      });
    }
  });

  it("says what is wrong with data that does not add up, before any provisional reply", () => {
    const seen: Interim[] = [];
    const reply = handle(semanticRequest(true, true, new Float32Array(10), 48, 6), (r) =>
      seen.push(r),
    );
    expect(seen).toEqual([]);
    expect(reply).toMatchObject({ kind: "error" });
    expect(reply.kind === "error" && reply.message).toMatch(/do not match/);
  });

  it("an error while t-SNE runs is the job's error reply (a throw would strand the worker)", () => {
    const reply = handle(semanticRequest(false, true), () => {
      throw new Error("post failed");
    });
    expect(reply).toMatchObject({ id: 5, kind: "error", message: "post failed" });
  });

  it("moves the provisional and the final coordinates' buffers, and nothing for progress", () => {
    const seen: Interim[] = [];
    const reply = handle(semanticRequest(true, true), (r) => seen.push(r));
    const provisional = seen[0];
    if (provisional.kind !== "provisional" || reply.kind !== "semantic") throw new Error("shape");
    expect(transferOf(provisional)).toEqual([provisional.coords.buffer]);
    expect(transferOf(reply)).toEqual([reply.coords.buffer]);
    expect(transferOf({ id: 5, kind: "progress", fraction: 0.5 })).toEqual([]);
  });
});

describe("handle: force", () => {
  it("answers with the simulation of the same graph", () => {
    const request = forceRequest(buildGraph(documents(12, 2)));
    const reply = handle({ id: 3, kind: "force", request });
    expect(reply).toEqual({ id: 3, kind: "force", positions: simulate(request).positions });
    expect(transferOf(reply)).toEqual([]);
  });

  it("refuses a link to a node that is not in the request (d3-force would throw on it)", () => {
    const request = {
      nodes: [{ id: "a", radius: 1 }],
      links: [{ source: "a", target: "b" }],
      width: 100,
      height: 100,
    };
    expect(() => simulate(request)).toThrow(/node not found: b/);
    // Our one sentence, not d3's (which carries the id that was asked for).
    expect(handle({ id: 5, kind: "force", request })).toEqual({
      id: 5,
      kind: "error",
      message: "The layout request was not understood.",
    });
  });
});

describe("handle: anything else is an error reply, never an exception", () => {
  const good = { nodes: [{ id: "a", radius: 1 }], links: [], width: 10, height: 10 };
  it.each([
    ["undefined", undefined],
    ["null", null],
    ["a string", "semantic"],
    ["a number", 7],
    ["an array", []],
    ["an unknown kind", { id: 1, kind: "other" }],
    ["no kind", { id: 1 }],
    [
      "reduce with plain-array vectors",
      { id: 1, kind: "semantic", pca: true, tsne: false, vectors: [1, 2], count: 1, dimension: 2 },
    ],
    [
      "reduce with no count",
      {
        id: 1,
        kind: "semantic",
        pca: true,
        tsne: false,
        vectors: new Float32Array(2),
        dimension: 2,
      },
    ],
    ["force with no request", { id: 1, kind: "force" }],
    ["force with a non-array nodes", { id: 1, kind: "force", request: { ...good, nodes: 1 } }],
    [
      "force with a non-string id",
      { id: 1, kind: "force", request: { ...good, nodes: [{ id: 1, radius: 1 }] } },
    ],
    [
      "force with a NaN radius",
      { id: 1, kind: "force", request: { ...good, nodes: [{ id: "a", radius: NaN }] } },
    ],
    ["force with a zero-size box", { id: 1, kind: "force", request: { ...good, width: 0 } }],
    [
      "force with an infinite box",
      { id: 1, kind: "force", request: { ...good, height: Infinity } },
    ],
    ["force with a non-object link", { id: 1, kind: "force", request: { ...good, links: [1] } }],
  ])("%s", (_name, request) => {
    const reply = handle(request);
    expect(reply.kind).toBe("error");
  });

  it("names the job when it can, and says there is none when it cannot", () => {
    expect(handle({ id: 12, kind: "nope" }).id).toBe(12);
    expect(handle({ id: "12", kind: "nope" }).id).toBe(NO_JOB);
    expect(handle(null).id).toBe(NO_JOB);
    expect(handle({ id: 1.5, kind: "nope" }).id).toBe(NO_JOB);
  });

  it("does not repeat anything from the request in its message", () => {
    const reply = handle({ id: 1, kind: "secret-kind-text" });
    expect(reply.kind === "error" && reply.message).not.toContain("secret-kind-text");
  });
});
