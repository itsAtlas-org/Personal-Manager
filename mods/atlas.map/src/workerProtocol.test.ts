// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Worker's one function, `handle`, with no Worker: what it answers for each job, and that it
// answers (never throws) for anything else, because a throw in a worker strands every job waiting on it.

// @ts-expect-error no @types/node (this repo has none: the root vite.config.ts says the same)
import { runInNewContext } from "node:vm";
import { describe, expect, it } from "vitest";
import { forceRequest, buildGraph } from "./layoutModel";
import { simulate } from "./forceLayout";
import { reduceToUnit } from "./pca";
import { NO_JOB, handle, transferOf } from "./workerProtocol";
import { documents } from "./testing";

const vectors = () => {
  const out = new Float32Array(6 * 3);
  for (let i = 0; i < out.length; i++) out[i] = Math.sin(i * 1.7) + (i % 3);
  return out;
};

describe("handle: reduce", () => {
  it("answers with the projection pca.ts makes of the same vectors", () => {
    const v = vectors();
    const reply = handle({ id: 4, kind: "reduce", vectors: v, count: 6, dimension: 3 });
    expect(reply.kind).toBe("reduce");
    if (reply.kind !== "reduce") return;
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
    const reply = handle({ id: 4, kind: "reduce", vectors: foreign, count: 6, dimension: 3 });
    expect(reply.kind).toBe("reduce");
    if (reply.kind !== "reduce") return;
    expect(Array.from(reply.coords)).toEqual(Array.from(reduceToUnit(v, 6, 3).coords));
  });

  it("still refuses every other typed array, even one of another realm", () => {
    for (const make of ["new Float64Array(18)", "new Int32Array(18)", "new Uint8Array(18)"]) {
      const view = runInNewContext(make) as ArrayBufferView;
      expect(
        handle({ id: 1, kind: "reduce", vectors: view, count: 6, dimension: 3 }),
      ).toMatchObject({
        id: 1,
        kind: "error",
        message: "The layout request was not understood.",
      });
    }
    // And an object that only says it is one.
    const liar = { [Symbol.toStringTag]: "Float32Array", length: 18 };
    expect(handle({ id: 1, kind: "reduce", vectors: liar, count: 6, dimension: 3 })).toMatchObject({
      kind: "error",
    });
  });

  it("moves the coordinates' buffer to the page instead of copying it", () => {
    const reply = handle({ id: 1, kind: "reduce", vectors: vectors(), count: 6, dimension: 3 });
    if (reply.kind !== "reduce") throw new Error("not a reduce reply");
    expect(transferOf(reply)).toEqual([reply.coords.buffer]);
  });

  it("says what is wrong with data that does not add up, in the reply, with the job's id", () => {
    const reply = handle({ id: 9, kind: "reduce", vectors: vectors(), count: 7, dimension: 3 });
    expect(reply).toMatchObject({ id: 9, kind: "error" });
    expect(reply.kind === "error" && reply.message).toMatch(/do not match/);
  });

  it("says so for a value that is not a number", () => {
    const v = vectors();
    v[5] = Number.NaN;
    expect(handle({ id: 2, kind: "reduce", vectors: v, count: 6, dimension: 3 })).toMatchObject({
      id: 2,
      kind: "error",
    });
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
    ["a string", "reduce"],
    ["a number", 7],
    ["an array", []],
    ["an unknown kind", { id: 1, kind: "other" }],
    ["no kind", { id: 1 }],
    [
      "reduce with plain-array vectors",
      { id: 1, kind: "reduce", vectors: [1, 2], count: 1, dimension: 2 },
    ],
    ["reduce with no count", { id: 1, kind: "reduce", vectors: new Float32Array(2), dimension: 2 }],
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
