// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the Map draws, as data: the node and edge lists for both arrangements, with no colour and no
// canvas. Ported from the cheap, pure half of the core Map's src/lib/mapLayout.ts (graph construction,
// the semantic layout, the fit box, the document-set cache key); the expensive half, the force run, is
// forceLayout.ts in a Worker. The old code's tests (src/lib/mapLayout.test.ts) and this file's
// (layoutModel.test.ts) hold the same literals. Two deliberate differences, each pinned by a test:
//  - the fit box and the centroids are computed with loops, not `Math.min(...xs)`: a spread of tens of
//    thousands of arguments overflows the call stack, and by-project mode draws every document;
//  - the overall centroid divides by the coordinates it summed (the old code divided by ALL of them, so
//    a coordinate for a document that was not in the list pulled it toward the corner).

import type { LayoutRequest } from "./forceLayout";
import { UNSORTED, type MapDoc, type SemanticCoord } from "./types";

/** The by-project simulation's box, and the margin the fit box leaves round the nodes. */
export const MAP_WIDTH = 960;
export const MAP_HEIGHT = 640;
const PAD = 50;
/** Semantic coordinates arrive in [0,1]^2; spread them over a square this wide before the fit. */
export const SEMANTIC_SPAN = 900;
/** A project hub's radius. */
const HUB_RADIUS = 14;
/** The most the cohesion blend may pull a document toward its project's centre. */
export const MAX_COHESION = 0.5;

/** A node with its final position, ready to colour and draw. Theme-independent. */
export interface PositionedNode {
  id: string;
  kind: "project" | "doc";
  label: string;
  project: string;
  radius: number;
  x: number;
  y: number;
  doc?: MapDoc;
  /** Semantic mode: this document has no coordinate yet (new since the last compute, or past the node
   *  cap), so it is parked at its project's centroid and drawn faded until a recompute lands. */
  transient?: boolean;
}

export interface Edge {
  sx: number;
  sy: number;
  tx: number;
  ty: number;
}

/** The fit box the canvas transform reads. */
export interface Bounds {
  minX: number;
  minY: number;
  width: number;
  height: number;
}

export interface BaseLayout {
  nodes: PositionedNode[];
  edges: Edge[];
  bounds: Bounds;
  viewBox: string;
  projectNames: string[];
}

/** A node's final coordinate: all of a by-project layout worth keeping, the rest is rebuilt cheaply. */
export interface NodePosition {
  id: string;
  x: number;
  y: number;
}

/** The project a document is drawn under: its home project, or Unsorted for none. */
export const projectOf = (doc: MapDoc): string => doc.project || UNSORTED;

/** A dot is dashed when its document is not fully there: awaiting review, or only indexed. */
export const isDashed = (doc?: MapDoc): boolean =>
  !!doc && (!doc.reviewed || doc.sourceType === "index_only");

export function docRadius(chunkCount: number): number {
  return Math.min(16, 4.5 + Math.sqrt(Math.max(1, chunkCount)) * 1.6);
}

/** Project names in the order their first document appears. */
function projectNamesOf(documents: MapDoc[]): string[] {
  return Array.from(new Set(documents.map(projectOf)));
}

/** The fit box of `nodes`, with the margin. Loops, not a spread (see the header). */
function fit(nodes: { x: number; y: number }[]): Bounds {
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const n of nodes) {
    if (n.x < minX) minX = n.x;
    if (n.x > maxX) maxX = n.x;
    if (n.y < minY) minY = n.y;
    if (n.y > maxY) maxY = n.y;
  }
  minX -= PAD;
  minY -= PAD;
  return { minX, minY, width: maxX - minX + PAD, height: maxY - minY + PAD };
}

const viewBoxOf = (b: Bounds) => `${b.minX} ${b.minY} ${b.width} ${b.height}`;

// ---- by project -----------------------------------------------------------------------------------

export interface Graph {
  meta: Map<string, Omit<PositionedNode, "x" | "y">>;
  links: { source: string; target: string }[];
  projectNames: string[];
}

/** The node and link graph for a document set: a hub per project, a node per document, a link from
 *  each document to its hub. Pure and cheap; positioning it is the expensive part. */
export function buildGraph(documents: MapDoc[]): Graph {
  const projectNames = projectNamesOf(documents);
  const meta = new Map<string, Omit<PositionedNode, "x" | "y">>();
  for (const name of projectNames) {
    meta.set(`project:${name}`, {
      id: `project:${name}`,
      kind: "project",
      label: name,
      project: name,
      radius: HUB_RADIUS,
    });
  }
  for (const d of documents) {
    meta.set(`doc:${d.id}`, {
      id: `doc:${d.id}`,
      kind: "doc",
      label: d.title,
      project: projectOf(d),
      radius: docRadius(d.chunkCount),
      doc: d,
    });
  }
  const links = documents.map((d) => ({
    source: `doc:${d.id}`,
    target: `project:${projectOf(d)}`,
  }));
  return { meta, links, projectNames };
}

/** What the force Worker is asked: every node's radius, the links, and the box. */
export function forceRequest(graph: Graph): LayoutRequest {
  return {
    nodes: Array.from(graph.meta.values()).map((n) => ({ id: n.id, radius: n.radius })),
    links: graph.links,
    width: MAP_WIDTH,
    height: MAP_HEIGHT,
  };
}

/** Positions into the drawable layout: nodes, edges, the fit box. Shared by a fresh simulation and a
 *  remembered one, so a remembered layout is pixel-identical to a computed one. A node with no position
 *  is at the origin. */
export function assemble(graph: Graph, positions: NodePosition[]): BaseLayout {
  const pos = new Map(positions.map((p) => [p.id, p]));
  const nodes: PositionedNode[] = Array.from(graph.meta.values()).map((n) => {
    const p = pos.get(n.id);
    return { ...n, x: p?.x ?? 0, y: p?.y ?? 0 };
  });
  const at = (id: string) => pos.get(id) ?? { x: 0, y: 0 };
  const edges: Edge[] = graph.links.map((l) => {
    const s = at(l.source);
    const t = at(l.target);
    return { sx: s.x, sy: s.y, tx: t.x, ty: t.y };
  });
  const bounds = fit(nodes);
  return { nodes, edges, bounds, viewBox: viewBoxOf(bounds), projectNames: graph.projectNames };
}

/** Two 32-bit FNV-1a hashes of `text`, as 16 hex characters. Only ever a cache key: a collision
 *  would show a stale layout until the next change, never anything worse. */
function hash64(text: string): string {
  let h1 = 0x811c9dc5;
  let h2 = 0x01000193;
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    h1 = Math.imul(h1 ^ c, 0x01000193) >>> 0;
    h2 = Math.imul(h2 ^ c, 0x85ebca6b) >>> 0;
  }
  return h1.toString(16).padStart(8, "0") + h2.toString(16).padStart(8, "0");
}

/**
 * The cache key of a by-project layout: the inputs that change it (each document's id, project and
 * chunk count; the count of documents), in an order that does not matter, and nothing else (a title
 * edit moves no dot). The core Map stored the whole sorted list as the key; this stores its hash, so a
 * 5,000-document key is 20 characters, not 100 KB.
 */
export function docSetKey(documents: MapDoc[]): string {
  const parts = documents.map((d) => `${d.id}:${projectOf(d)}:${d.chunkCount}`).sort();
  return `${parts.length}|${hash64(parts.join(","))}`;
}

// ---- by meaning -----------------------------------------------------------------------------------

/**
 * A "by meaning" layout from 2-D coordinates in [0,1]^2: each document sits where its embedding
 * projects, so nearby = similar. No hubs and no edges (proximity is the signal), but dots are still
 * coloured by project. A document with no coordinate yet (added since the last compute, or past the
 * node cap) is parked at its project's centroid and marked transient: it still appears, grouped, and
 * snaps into place when a recompute lands.
 *
 * `cohesion` (0 = off) pulls each placed document that fraction of the way toward its project's centre
 * of mass, so same-project documents read as looser clusters without abandoning the meaning layout. It is
 * a render-time blend over the cached coordinates (clamped to 0..MAX_COHESION): instant, no recompute,
 * no effect on the cache. It assumes one project per document, the Map's home-project rule.
 */
export function buildSemanticLayout(
  documents: MapDoc[],
  coords: SemanticCoord[],
  cohesion = 0,
): BaseLayout | null {
  if (documents.length === 0) return null;
  const pull = Math.max(0, Math.min(MAX_COHESION, cohesion));
  const projectNames = projectNamesOf(documents);
  const coordById = new Map(coords.map((c) => [c.id, c]));

  // Project centroids, from the listed documents that do have coordinates, and an overall fallback.
  const sums = new Map<string, { x: number; y: number; n: number }>();
  let ox = 0;
  let oy = 0;
  let placed = 0;
  for (const d of documents) {
    const c = coordById.get(d.id);
    if (!c) continue;
    const proj = projectOf(d);
    const s = sums.get(proj) ?? { x: 0, y: 0, n: 0 };
    s.x += c.x;
    s.y += c.y;
    s.n += 1;
    sums.set(proj, s);
    ox += c.x;
    oy += c.y;
    placed += 1;
  }
  const overall = placed > 0 ? { x: ox / placed, y: oy / placed } : { x: 0.5, y: 0.5 };
  const centroidFor = (proj: string) => {
    const s = sums.get(proj);
    return s && s.n > 0 ? { x: s.x / s.n, y: s.y / s.n } : overall;
  };

  const nodes: PositionedNode[] = documents.map((d) => {
    const proj = projectOf(d);
    const c = coordById.get(d.id);
    // No coordinate: park (faded) at the project centroid. Otherwise the meaning-driven place, blended
    // a fraction (`pull`) toward that centroid when cohesion is on.
    let p = c ?? centroidFor(proj);
    if (c && pull > 0) {
      const cen = centroidFor(proj);
      p = { x: c.x + (cen.x - c.x) * pull, y: c.y + (cen.y - c.y) * pull };
    }
    return {
      id: `doc:${d.id}`,
      kind: "doc",
      label: d.title,
      project: proj,
      radius: docRadius(d.chunkCount),
      x: p.x * SEMANTIC_SPAN,
      y: p.y * SEMANTIC_SPAN,
      doc: d,
      transient: !c,
    };
  });

  const bounds = fit(nodes);
  return { nodes, edges: [], bounds, viewBox: viewBoxOf(bounds), projectNames };
}
