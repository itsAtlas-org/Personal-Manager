// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The port of the force simulation, held to the core Map's: the literals in the `it.each` below came
// from running src/lib/forceLayout.ts (see src/lib/mapLayout.test.ts, which holds the same ones and
// fails if the old code drifts). The graph is built by this mod's own buildGraph, so the test also
// proves the port builds the same graph the old code did.

import { describe, expect, it } from "vitest";
import { BIG_MAP_NODES, simulate, TICKS, TICKS_BIG } from "./forceLayout";
import { buildGraph, forceRequest, MAP_HEIGHT, MAP_WIDTH } from "./layoutModel";
import { digest, documents, flat } from "./testing";

const requestFor = (n: number, projects: number) =>
  forceRequest(buildGraph(documents(n, projects)));

describe("simulate", () => {
  it("is deterministic: the same graph twice is the same positions", () => {
    const req = requestFor(40, 3);
    expect(flat(simulate(req).positions)).toEqual(flat(simulate(req).positions));
  });

  it("returns one position per node, in node order, ids kept, all finite", () => {
    const req = requestFor(25, 3);
    const { positions } = simulate(req);
    expect(positions.map((p) => p.id)).toEqual(req.nodes.map((n) => n.id));
    for (const p of positions) {
      expect(Number.isFinite(p.x)).toBe(true);
      expect(Number.isFinite(p.y)).toBe(true);
    }
  });

  it("does not change the request it was given", () => {
    const req = requestFor(25, 3);
    const before = JSON.stringify(req);
    simulate(req);
    expect(JSON.stringify(req)).toBe(before);
  });

  it("settles around the middle of the box", () => {
    const { positions } = simulate(requestFor(60, 4));
    const mx = positions.reduce((s, p) => s + p.x, 0) / positions.length;
    const my = positions.reduce((s, p) => s + p.y, 0) / positions.length;
    expect(Math.abs(mx - MAP_WIDTH / 2)).toBeLessThan(40);
    expect(Math.abs(my - MAP_HEIGHT / 2)).toBeLessThan(40);
  });

  it("keeps a document nearer its own project's hub than a stranger's", () => {
    const docs = documents(60, 3).filter((d) => d.project !== "");
    const { positions } = simulate(forceRequest(buildGraph(docs)));
    const at = new Map(positions.map((p) => [p.id, p]));
    let nearer = 0;
    for (const d of docs) {
      const p = at.get(`doc:${d.id}`)!;
      const own = at.get(`project:${d.project}`)!;
      const others = ["Atlas", "Beta", "Gamma"].filter((n) => n !== d.project);
      const dist = (q: { x: number; y: number }) => Math.hypot(p.x - q.x, p.y - q.y);
      if (others.every((n) => dist(own) < dist(at.get(`project:${n}`)!))) nearer++;
    }
    expect(nearer / docs.length).toBeGreaterThan(0.9);
  });

  it("an empty request is an empty answer", () => {
    expect(simulate({ nodes: [], links: [], width: 10, height: 10 }).positions).toEqual([]);
  });

  it("runs 400 ticks up to 400 nodes and 250 above (the limits are the old code's)", () => {
    expect([BIG_MAP_NODES, TICKS, TICKS_BIG]).toEqual([400, 400, 250]);
  });

  // 394 documents and 6 hubs are 400 nodes (400 ticks); one more document is 401 nodes (250 ticks).
  it.each([
    [5, 2, 7, "67a7639b9fc6c5ed"],
    [60, 3, 64, "fc0dfc77e5cbe9f9"],
    [394, 5, 400, "372c7255429bb287"],
    [395, 5, 401, "cd1d3b27dfe45f4d"],
  ])("the exact layout of %i documents in %i projects (%i nodes)", (n, p, nodes, expected) => {
    const req = requestFor(n, p);
    expect(req.nodes).toHaveLength(nodes);
    expect(digest(flat(simulate(req).positions))).toBe(expected);
  });
});
