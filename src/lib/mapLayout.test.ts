// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The core Map's layout code had no tests. These pin what it does TODAY, because the Map is becoming a
// mod (Mods PR B) and the port in mods/atlas.map/src is only as good as its oracle. The numbers below
// were taken from THIS code before the port was written; the mod's tests hold the same literals
// (layoutModel.test.ts, forceLayout.test.ts), so a drift on either side fails a test. This file goes
// when the core Map does (B9); the mod's tests keep the literals.
//
// What is pinned: the force simulation (d3-force settings, the tick count that drops from 400 to 250
// above 400 nodes), the by-project graph (hub per project, "Unsorted" for a blank, radius from chunk
// count), the semantic layout (scale, transient parking, centroid blend and its clamp) and the cache
// key and its use. Positions are compared rounded to 3 decimals so a last-bit difference in a maths
// library can never fail a test.

import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Document } from "./types";

vi.mock("./ipc", () => ({
  listDocuments: vi.fn(),
  projectLayout: vi.fn(),
  setProjectLayout: vi.fn(),
}));

import { projectLayout, setProjectLayout } from "./ipc";
import { simulate } from "./forceLayout";
import { buildSemanticLayout, getProjectLayout, MAP_HEIGHT, MAP_WIDTH } from "./mapLayout";

// ---- the shared inputs (the mod's tests spell the same generator) ---------------------------------

/** mulberry32: a small seeded generator, so every run and every machine sees the same documents. */
function seeded(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const PROJECTS = ["Atlas", "Beta", "Gamma", "Delta", "Epsilon", "Zeta", "Eta"];

/** `n` documents over `projects` projects (a blank project for every 11th), chunk counts 1..40. */
function documents(n: number, projects: number): Document[] {
  const rand = seeded(1234 + n);
  const docs: Document[] = [];
  for (let i = 0; i < n; i++) {
    docs.push({
      id: i + 1,
      title: `Document ${i + 1}`,
      project: i % 11 === 10 ? "" : PROJECTS[Math.floor(rand() * projects)],
      chunk_count: 1 + Math.floor(rand() * 40),
      reviewed: true,
      source_type: "vault",
      tags: [],
      importance: null,
      ingested_at: "2026-01-01T00:00:00Z",
    } as unknown as Document);
  }
  return docs;
}

/** Two 32-bit FNV-1a hashes of the positions rounded to 3 decimals: a change detector, not a hash for
 *  security. Long enough that two different layouts never share one by accident. */
function digest(values: number[]): string {
  let h1 = 0x811c9dc5;
  let h2 = 0x01000193;
  for (const v of values) {
    const s = `${v.toFixed(3)};`;
    for (let i = 0; i < s.length; i++) {
      h1 = Math.imul(h1 ^ s.charCodeAt(i), 0x01000193) >>> 0;
      h2 = Math.imul(h2 ^ s.charCodeAt(i), 0x85ebca6b) >>> 0;
    }
  }
  return `${h1.toString(16).padStart(8, "0")}${h2.toString(16).padStart(8, "0")}`;
}

/** The request `simulateFor` builds for a document set, as plain data. */
function request(docs: Document[]) {
  const names = Array.from(new Set(docs.map((d) => d.project || "Unsorted")));
  const radius = (c: number) => Math.min(16, 4.5 + Math.sqrt(Math.max(1, c)) * 1.6);
  return {
    nodes: [
      ...names.map((name) => ({ id: `project:${name}`, radius: 14 })),
      ...docs.map((d) => ({ id: `doc:${d.id}`, radius: radius(d.chunk_count) })),
    ],
    links: docs.map((d) => ({
      source: `doc:${d.id}`,
      target: `project:${d.project || "Unsorted"}`,
    })),
    width: MAP_WIDTH,
    height: MAP_HEIGHT,
  };
}

const flat = (positions: { x: number; y: number }[]) => positions.flatMap((p) => [p.x, p.y]);

// ---- the force simulation -----------------------------------------------------------------------

describe("core Map: simulate (the by-project force layout)", () => {
  it("is deterministic: the same graph twice is the same positions", () => {
    const req = request(documents(40, 3));
    expect(flat(simulate(req).positions)).toEqual(flat(simulate(req).positions));
  });

  it("returns one position per node, in node order, ids kept", () => {
    const req = request(documents(25, 3));
    const { positions } = simulate(req);
    expect(positions.map((p) => p.id)).toEqual(req.nodes.map((n) => n.id));
    for (const p of positions) {
      expect(Number.isFinite(p.x)).toBe(true);
      expect(Number.isFinite(p.y)).toBe(true);
    }
  });

  it("does not change the request it was given", () => {
    const req = request(documents(25, 3));
    const before = JSON.stringify(req);
    simulate(req);
    expect(JSON.stringify(req)).toBe(before);
  });

  it("settles around the middle of the box", () => {
    const { positions } = simulate(request(documents(60, 4)));
    const mx = positions.reduce((s, p) => s + p.x, 0) / positions.length;
    const my = positions.reduce((s, p) => s + p.y, 0) / positions.length;
    expect(Math.abs(mx - MAP_WIDTH / 2)).toBeLessThan(40);
    expect(Math.abs(my - MAP_HEIGHT / 2)).toBeLessThan(40);
  });

  it("keeps a document nearer its own project's hub than a stranger's", () => {
    const docs = documents(60, 3).filter((d) => d.project !== "");
    const { positions } = simulate(request(docs));
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

  // The literals: taken from this code. The tick count is part of them: 400 up to 400 nodes, 250 above.
  // 394 documents and 6 hubs are 400 nodes (400 ticks); one more document is 401 nodes (250 ticks).
  it.each([
    [5, 2, 7, "67a7639b9fc6c5ed"],
    [60, 3, 64, "fc0dfc77e5cbe9f9"],
    [394, 5, 400, "372c7255429bb287"],
    [395, 5, 401, "cd1d3b27dfe45f4d"],
  ])("the exact layout of %i documents in %i projects (%i nodes)", (n, p, nodes, expected) => {
    const req = request(documents(n, p));
    expect(req.nodes).toHaveLength(nodes);
    expect(digest(flat(simulate(req).positions))).toBe(expected);
  });
});

// ---- the semantic layout ------------------------------------------------------------------------

const doc = (id: number, project: string, chunks = 4, title = `T${id}`) =>
  ({ id, title, project, chunk_count: chunks }) as unknown as Document;

describe("core Map: buildSemanticLayout", () => {
  it("is null with no documents", () => {
    expect(buildSemanticLayout([], [{ id: 1, x: 0, y: 0 }])).toBeNull();
  });

  it("scales [0,1] coordinates by 900 and names nodes doc:<id>", () => {
    const l = buildSemanticLayout([doc(1, "A")], [{ id: 1, x: 0.25, y: 0.5 }])!;
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

  it("radius is 4.5 + 1.6*sqrt(chunks), at least 1 chunk, at most 16", () => {
    const l = buildSemanticLayout(
      [doc(1, "A", 0), doc(2, "A", 1), doc(3, "A", 16), doc(4, "A", 10000)],
      [],
    )!;
    expect(l.nodes.map((n) => n.radius)).toEqual([6.1, 6.1, 4.5 + 1.6 * 4, 16]);
  });

  it("parks a document with no coordinate at its project's centroid, transient", () => {
    const l = buildSemanticLayout(
      [doc(1, "A"), doc(2, "A"), doc(3, "A")],
      [
        { id: 1, x: 0.2, y: 0.2 },
        { id: 2, x: 0.6, y: 0.4 },
      ],
    )!;
    const third = l.nodes[2];
    expect(third.transient).toBe(true);
    expect(third.x).toBeCloseTo(0.4 * 900, 6);
    expect(third.y).toBeCloseTo(0.3 * 900, 6);
  });

  it("parks at the overall centroid when its own project has no coordinates, and at the middle when none has", () => {
    const some = buildSemanticLayout([doc(1, "A"), doc(2, "B")], [{ id: 1, x: 0.2, y: 0.8 }])!;
    expect([some.nodes[1].x, some.nodes[1].y]).toEqual([0.2 * 900, 0.8 * 900]);
    const none = buildSemanticLayout([doc(1, "A")], [])!;
    expect([none.nodes[0].x, none.nodes[0].y]).toEqual([450, 450]);
    expect(none.nodes[0].transient).toBe(true);
  });

  // A quirk, pinned so the port's choice is a decision and not an accident: the overall centroid sums
  // the coordinates of the LISTED documents but divides by ALL coordinates, so a coordinate for a
  // document that is not in the list drags it toward the corner. It cannot happen in practice (the
  // coordinates were computed for the document set), and the mod's port divides by the count it
  // summed (layoutModel.test.ts says so).
  it("the overall centroid divides by every coordinate, even ones for documents not in the list", () => {
    const l = buildSemanticLayout(
      [doc(1, "B"), doc(2, "C")],
      [
        { id: 2, x: 0.8, y: 0.8 },
        { id: 99, x: 0, y: 0 },
      ],
    )!;
    // doc 1 (project B, no coordinate of its own) goes to (0.8 / 2, 0.8 / 2), not (0.8, 0.8).
    expect([l.nodes[0].x, l.nodes[0].y]).toEqual([0.4 * 900, 0.4 * 900]);
  });

  it("cohesion pulls each document that way toward its project centroid, clamped to 0..0.5", () => {
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
    const docs = [doc(1, "A"), doc(2, "A")];
    const l = buildSemanticLayout(docs, [{ id: 1, x: 0.5, y: 0.5 }], 0.5)!;
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
    expect(l.bounds).toEqual({ minX: -50, minY: -50, width: 900 + 50 + 50, height: 450 + 50 + 50 });
    expect(l.viewBox).toBe("-50 -50 1000 550");
  });
});

// ---- the by-project cache -----------------------------------------------------------------------

describe("core Map: getProjectLayout and its cache key", () => {
  beforeEach(() => {
    vi.mocked(projectLayout).mockReset();
    vi.mocked(setProjectLayout).mockReset();
    vi.mocked(projectLayout).mockResolvedValue(null);
    vi.mocked(setProjectLayout).mockResolvedValue(undefined);
  });

  it("is null for no documents", () => {
    expect(getProjectLayout([])).toBeNull();
  });

  it("builds a hub per project plus a node per document, and an edge per document", async () => {
    const docs = [doc(1, "A"), doc(2, "A"), doc(3, "")];
    const layout = (await getProjectLayout(docs))!;
    expect(layout.projectNames).toEqual(["A", "Unsorted"]);
    expect(layout.nodes.map((n) => n.id)).toEqual([
      "project:A",
      "project:Unsorted",
      "doc:1",
      "doc:2",
      "doc:3",
    ]);
    expect(layout.nodes.filter((n) => n.kind === "project").every((n) => n.radius === 14)).toBe(
      true,
    );
    expect(layout.edges).toHaveLength(3);
    // An edge runs from the document to its hub's computed position.
    const byId = new Map(layout.nodes.map((n) => [n.id, n]));
    expect(layout.edges[2]).toEqual({
      sx: byId.get("doc:3")!.x,
      sy: byId.get("doc:3")!.y,
      tx: byId.get("project:Unsorted")!.x,
      ty: byId.get("project:Unsorted")!.y,
    });
  });

  it("remembers positions under a key of the sorted id:project:chunks parts, version 1", async () => {
    await getProjectLayout([doc(2, "B", 7), doc(1, "A", 3), doc(3, "", 1)]);
    await Promise.resolve();
    const saved = JSON.parse(vi.mocked(setProjectLayout).mock.calls[0][0] ?? "null");
    expect(saved.v).toBe(1);
    expect(saved.hash).toBe("3|1:A:3,2:B:7,3:Unsorted:1");
    expect(saved.positions).toHaveLength(3 + 3);
  });

  it("the key ignores the title and the order, and notices a project or chunk-count change", async () => {
    // Observed through the in-memory tier, which is keyed the same way: the same key is the same
    // answer object.
    const base = getProjectLayout([doc(1, "A", 3, "x"), doc(2, "B", 7)]);
    expect(getProjectLayout([doc(2, "B", 7), doc(1, "A", 3, "y")])).toBe(base);
    expect(getProjectLayout([doc(1, "A", 4), doc(2, "B", 7)])).not.toBe(base);
    expect(getProjectLayout([doc(1, "C", 3), doc(2, "B", 7)])).not.toBe(base);
    await base;
  });

  it("uses a stored layout for exactly that key and does not simulate or save again", async () => {
    const hash = "1|1:A:4";
    vi.mocked(projectLayout).mockResolvedValue(
      JSON.stringify({
        v: 1,
        hash,
        positions: [
          { id: "project:A", x: 10, y: 20 },
          { id: "doc:1", x: 30, y: 40 },
        ],
      }),
    );
    const layout = (await getProjectLayout([doc(1, "A")]))!;
    expect(layout.nodes.map((n) => [n.id, n.x, n.y])).toEqual([
      ["project:A", 10, 20],
      ["doc:1", 30, 40],
    ]);
    expect(setProjectLayout).not.toHaveBeenCalled();
  });

  it("a stored layout for another key, another version, or garbage is simply not used", async () => {
    const cases = (hash: string) => [
      JSON.stringify({ v: 1, hash: "other", positions: [{ id: "doc:1", x: 1, y: 1 }] }),
      JSON.stringify({ v: 2, hash, positions: [{ id: "doc:1", x: 1, y: 1 }] }),
      JSON.stringify({ v: 1, hash, positions: "no" }),
      "not json",
    ];
    for (let i = 0; i < 4; i++) {
      // A fresh document each time so the in-memory tier cannot answer.
      const id = 100 + i;
      vi.mocked(projectLayout).mockResolvedValue(cases(`1|${id}:A:4`)[i]);
      vi.mocked(setProjectLayout).mockClear();
      const layout = (await getProjectLayout([doc(id, "A")]))!;
      await Promise.resolve();
      // Computed (not the stored positions) and remembered again.
      expect(layout.nodes.find((n) => n.id === `doc:${id}`)!.x).not.toBe(1);
      expect(setProjectLayout).toHaveBeenCalledTimes(1);
    }
  });

  it("a stored layout that fails to read does not stop the layout being computed", async () => {
    vi.mocked(projectLayout).mockRejectedValue(new Error("db"));
    const layout = (await getProjectLayout([doc(50, "A")]))!;
    expect(layout.nodes).toHaveLength(2);
  });

  it("a failure to save is silent", async () => {
    vi.mocked(setProjectLayout).mockRejectedValue(new Error("db"));
    await expect(getProjectLayout([doc(60, "A")])).resolves.not.toBeNull();
  });

  it("the same document set is answered from memory the second time", async () => {
    const docs = [doc(70, "A")];
    const first = getProjectLayout(docs);
    expect(getProjectLayout(docs)).toBe(first);
  });
});
