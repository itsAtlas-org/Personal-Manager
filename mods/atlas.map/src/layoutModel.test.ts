// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Map's data model against the core Map's behaviour. Most cases are the ones src/lib/mapLayout.test.ts
// pinned on the old code, here run on the port; the few that differ say so and why.

import { describe, expect, it } from "vitest";
import { simulate } from "./forceLayout";
import {
  assemble,
  buildGraph,
  buildSemanticLayout,
  docRadius,
  docSetKey,
  forceRequest,
  isDashed,
  MAP_HEIGHT,
  MAP_WIDTH,
  projectOf,
  SEMANTIC_SPAN,
} from "./layoutModel";
import { doc, documents } from "./testing";
import type { MapDoc } from "./types";

describe("projectOf", () => {
  it("is the home project, or Unsorted when there is none", () => {
    expect(projectOf(doc(1, "Atlas"))).toBe("Atlas");
    expect(projectOf(doc(1, ""))).toBe("Unsorted");
  });
});

describe("isDashed", () => {
  const base = doc(1, "A");
  it("a reviewed vault document is solid", () => {
    expect(isDashed(base)).toBe(false);
  });
  it("awaiting review is dashed", () => {
    expect(isDashed({ ...base, reviewed: false })).toBe(true);
  });
  it("an index-only document is dashed even when reviewed", () => {
    expect(isDashed({ ...base, sourceType: "index_only" })).toBe(true);
  });
  it("chat, photo and spreadsheet documents are solid", () => {
    for (const sourceType of ["chat", "photo", "spreadsheet"]) {
      expect(isDashed({ ...base, sourceType })).toBe(false);
    }
  });
  it("a hub (no document) is never dashed", () => {
    expect(isDashed(undefined)).toBe(false);
  });
});

describe("docRadius", () => {
  it("is 4.5 + 1.6*sqrt(chunks), at least one chunk, at most 16", () => {
    expect([0, 1, 16, 10000].map(docRadius)).toEqual([6.1, 6.1, 4.5 + 1.6 * 4, 16]);
  });
});

describe("the by-project graph", () => {
  it("is a hub per project (first-seen order, blank as Unsorted) then a node per document", () => {
    const g = buildGraph([doc(1, "B"), doc(2, ""), doc(3, "A"), doc(4, "B")]);
    expect(g.projectNames).toEqual(["B", "Unsorted", "A"]);
    expect(Array.from(g.meta.keys())).toEqual([
      "project:B",
      "project:Unsorted",
      "project:A",
      "doc:1",
      "doc:2",
      "doc:3",
      "doc:4",
    ]);
    expect(g.links).toEqual([
      { source: "doc:1", target: "project:B" },
      { source: "doc:2", target: "project:Unsorted" },
      { source: "doc:3", target: "project:A" },
      { source: "doc:4", target: "project:B" },
    ]);
  });

  it("hubs have radius 14 and document nodes keep their document", () => {
    const d = doc(1, "A", 9, "Title");
    const g = buildGraph([d]);
    expect(g.meta.get("project:A")).toMatchObject({ kind: "project", radius: 14, label: "A" });
    expect(g.meta.get("doc:1")).toMatchObject({
      kind: "doc",
      label: "Title",
      project: "A",
      radius: 4.5 + 1.6 * 3,
      doc: d,
    });
  });

  it("asks the Worker for the whole graph in the 960 x 640 box", () => {
    const req = forceRequest(buildGraph([doc(1, "A"), doc(2, "A")]));
    expect([req.width, req.height]).toEqual([MAP_WIDTH, MAP_HEIGHT]);
    expect(req.nodes.map((n) => n.id)).toEqual(["project:A", "doc:1", "doc:2"]);
    expect(req.links).toHaveLength(2);
  });

  it("assembles nodes, one edge per document to its hub, and a fit box", () => {
    const docs = [doc(1, "A"), doc(2, "A"), doc(3, "")];
    const layout = assemble(buildGraph(docs), simulate(forceRequest(buildGraph(docs))).positions);
    expect(layout.projectNames).toEqual(["A", "Unsorted"]);
    expect(layout.nodes.map((n) => n.id)).toEqual([
      "project:A",
      "project:Unsorted",
      "doc:1",
      "doc:2",
      "doc:3",
    ]);
    expect(layout.edges).toHaveLength(3);
    const byId = new Map(layout.nodes.map((n) => [n.id, n]));
    expect(layout.edges[2]).toEqual({
      sx: byId.get("doc:3")!.x,
      sy: byId.get("doc:3")!.y,
      tx: byId.get("project:Unsorted")!.x,
      ty: byId.get("project:Unsorted")!.y,
    });
    const xs = layout.nodes.map((n) => n.x);
    expect(layout.bounds.minX).toBe(Math.min(...xs) - 50);
    expect(layout.bounds.width).toBe(Math.max(...xs) - layout.bounds.minX + 50);
    expect(layout.viewBox).toBe(
      `${layout.bounds.minX} ${layout.bounds.minY} ${layout.bounds.width} ${layout.bounds.height}`,
    );
  });

  it("a node with no remembered position sits at the origin, so a half-empty cache still draws", () => {
    const layout = assemble(buildGraph([doc(1, "A")]), [{ id: "doc:1", x: 5, y: 6 }]);
    expect(layout.nodes.map((n) => [n.id, n.x, n.y])).toEqual([
      ["project:A", 0, 0],
      ["doc:1", 5, 6],
    ]);
  });

  it("a remembered layout assembles to exactly the same thing as a fresh one", () => {
    const docs = documents(30, 3);
    const graph = buildGraph(docs);
    const fresh = assemble(graph, simulate(forceRequest(graph)).positions);
    const remembered = assemble(
      graph,
      JSON.parse(JSON.stringify(fresh.nodes.map((n) => ({ id: n.id, x: n.x, y: n.y })))),
    );
    expect(remembered).toEqual(fresh);
  });

  it("fits a very large map without a call-stack overflow (no Math.min(...spread))", () => {
    const docs: MapDoc[] = Array.from({ length: 200_000 }, (_, i) => doc(i + 1, "A"));
    const graph = buildGraph(docs);
    const positions = Array.from(graph.meta.keys()).map((id, i) => ({
      id,
      x: i % 977,
      y: i % 311,
    }));
    const layout = assemble(graph, positions);
    expect(layout.bounds.minX).toBe(-50);
    expect(layout.bounds.width).toBe(976 + 50 + 50);
  });
});

describe("docSetKey (the by-project cache key)", () => {
  const key = docSetKey;
  it("ignores the order and the title", () => {
    const base = key([doc(1, "A", 3, "x"), doc(2, "B", 7)]);
    expect(key([doc(2, "B", 7), doc(1, "A", 3, "y")])).toBe(base);
  });
  it("notices a project change, a chunk-count change, a document added, a document gone", () => {
    const base = key([doc(1, "A", 3), doc(2, "B", 7)]);
    expect(key([doc(1, "C", 3), doc(2, "B", 7)])).not.toBe(base);
    expect(key([doc(1, "A", 4), doc(2, "B", 7)])).not.toBe(base);
    expect(key([doc(1, "A", 3), doc(2, "B", 7), doc(3, "B", 1)])).not.toBe(base);
    expect(key([doc(1, "A", 3)])).not.toBe(base);
  });
  it("treats a blank project and Unsorted as one, as the old key did", () => {
    expect(key([doc(1, "", 3)])).toBe(key([doc(1, "Unsorted", 3)]));
  });
  it("is short however many documents there are, and leads with the count", () => {
    const k = key(documents(5000, 5));
    expect(k.length).toBeLessThan(30);
    expect(k.startsWith("5000|")).toBe(true);
  });
  it("tells apart sets that differ only in how the parts split", () => {
    // "1:AB:3" against "1:A:3" + "B..." style joins: a separator-free join could collide.
    expect(key([doc(1, "A", 12)])).not.toBe(key([doc(11, "A", 2)]));
  });
});

describe("the by-meaning layout", () => {
  it("is null with no documents", () => {
    expect(buildSemanticLayout([], [{ id: 1, x: 0, y: 0 }])).toBeNull();
  });

  it("scales [0,1] coordinates by 900 and names nodes doc:<id>", () => {
    const l = buildSemanticLayout([doc(1, "A")], [{ id: 1, x: 0.25, y: 0.5 }])!;
    expect(SEMANTIC_SPAN).toBe(900);
    expect(l.nodes).toHaveLength(1);
    expect(l.nodes[0]).toMatchObject({ id: "doc:1", kind: "doc", x: 225, y: 450, project: "A" });
    expect(l.nodes[0].transient).toBe(false);
    expect(l.edges).toEqual([]);
  });

  it("lists projects in the order their first document appears, a blank one as Unsorted", () => {
    const l = buildSemanticLayout(
      [doc(1, "B"), doc(2, ""), doc(3, "A"), doc(4, "B")],
      [{ id: 1, x: 0, y: 0 }],
    )!;
    expect(l.projectNames).toEqual(["B", "Unsorted", "A"]);
    expect(l.nodes[1].project).toBe("Unsorted");
  });

  it("parks a document with no coordinate at its project's centroid, transient", () => {
    const l = buildSemanticLayout(
      [doc(1, "A"), doc(2, "A"), doc(3, "A")],
      [
        { id: 1, x: 0.2, y: 0.2 },
        { id: 2, x: 0.6, y: 0.4 },
      ],
    )!;
    expect(l.nodes[2].transient).toBe(true);
    expect(l.nodes[2].x).toBeCloseTo(0.4 * 900, 6);
    expect(l.nodes[2].y).toBeCloseTo(0.3 * 900, 6);
  });

  it("parks at the overall centroid when its own project has none, and at the middle when no one has", () => {
    const some = buildSemanticLayout([doc(1, "A"), doc(2, "B")], [{ id: 1, x: 0.2, y: 0.8 }])!;
    expect([some.nodes[1].x, some.nodes[1].y]).toEqual([0.2 * 900, 0.8 * 900]);
    const none = buildSemanticLayout([doc(1, "A")], [])!;
    expect([none.nodes[0].x, none.nodes[0].y]).toEqual([450, 450]);
    expect(none.nodes[0].transient).toBe(true);
  });

  // The one difference from the old code, on purpose. It summed the listed documents' coordinates but
  // divided by ALL coordinates, so a coordinate for a document that is not in the list (never in
  // practice: the coordinates were computed for the document set) dragged parked documents toward the
  // corner. The port divides by what it summed: doc 1 is parked at (0.8, 0.8), not at (0.4, 0.4).
  it("the overall centroid averages only the coordinates of listed documents", () => {
    const l = buildSemanticLayout(
      [doc(1, "B"), doc(2, "C")],
      [
        { id: 2, x: 0.8, y: 0.8 },
        { id: 99, x: 0, y: 0 },
      ],
    )!;
    expect([l.nodes[0].x, l.nodes[0].y]).toEqual([0.8 * 900, 0.8 * 900]);
  });

  it("cohesion pulls each placed document toward its project centroid, clamped to 0..0.5", () => {
    const docs = [doc(1, "A"), doc(2, "A")];
    const coords = [
      { id: 1, x: 0, y: 0 },
      { id: 2, x: 1, y: 1 },
    ];
    const at = (cohesion: number) => buildSemanticLayout(docs, coords, cohesion)!.nodes[0].x;
    expect(at(0)).toBe(0);
    expect(at(0.3)).toBeCloseTo(0.3 * 0.5 * 900, 6);
    expect(at(0.5)).toBeCloseTo(0.25 * 900, 6);
    expect(at(0.9)).toBe(at(0.5));
    expect(at(-1)).toBe(0);
  });

  it("cohesion never moves a transient document (it is already at its centroid)", () => {
    const l = buildSemanticLayout([doc(1, "A"), doc(2, "A")], [{ id: 1, x: 0.5, y: 0.5 }], 0.5)!;
    expect(l.nodes[1].x).toBe(450);
  });

  it("fits a box of the nodes with 50 padding, and the viewBox is the same box", () => {
    const l = buildSemanticLayout(
      [doc(1, "A"), doc(2, "A")],
      [
        { id: 1, x: 0, y: 0 },
        { id: 2, x: 1, y: 0.5 },
      ],
    )!;
    expect(l.bounds).toEqual({ minX: -50, minY: -50, width: 1000, height: 550 });
    expect(l.viewBox).toBe("-50 -50 1000 550");
  });

  it("uses each node's radius from its chunk count", () => {
    const l = buildSemanticLayout([doc(1, "A", 0), doc(2, "A", 16), doc(3, "A", 10000)], [])!;
    expect(l.nodes.map((n) => n.radius)).toEqual([6.1, 4.5 + 1.6 * 4, 16]);
  });

  it("handles a large map without a call-stack overflow", () => {
    const docs: MapDoc[] = Array.from({ length: 200_000 }, (_, i) => doc(i + 1, "A"));
    const l = buildSemanticLayout(docs, [{ id: 1, x: 0.5, y: 0.5 }])!;
    expect(l.nodes).toHaveLength(200_000);
  });
});
