// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The "by project" arrangement: a hub per project, every document linked to its hub, positioned by the
// force simulation (forceLayout.ts, in the Worker). Ported from the core Map's src/lib/mapLayout.ts
// (`getProjectLayout`, `loadOrCompute`, the persisted-positions cache). What it keeps from the old
// code: only node POSITIONS are remembered, keyed on a fingerprint of the document set; everything else
// is rebuilt from the list in microseconds; a stale, malformed or unreadable cache is silently "no
// cache"; failing to remember costs the next open a recompute and nothing else. What differs: the
// positions live in one cache-class storage key (`project-layout`) instead of a database setting,
// and a layout too big for one storage value is simply not remembered (a vault that large recomputes).
//
// Positions are rounded to 2 decimals (0.01 px) BEFORE the layout is built, so a layout that was
// computed and one read back from storage are made from the same numbers: a remembered layout is
// pixel-identical to a fresh one, which the old code's design relied on too.

import type { Compute } from "./compute";
import { isCount, MAX_ARGS_BYTES, readKey, writeKey, type MapHost } from "./hostApi";
import {
  assemble,
  buildGraph,
  docSetKey,
  forceRequest,
  type BaseLayout,
  type Graph,
  type NodePosition,
} from "./layoutModel";
import type { MapDoc } from "./types";

/** Bump after a change to the simulation or to how the graph is built (the old code's
 *  PERSISTED_VERSION), so a remembered layout is never replayed under different rules. */
export const PROJECT_LAYOUT_VERSION = 1;

/** The storage key. Cache class (see semantic.ts). */
export const KEY_PROJECT_LAYOUT = "project-layout";

/** The most JSON a remembered layout may be: well under PM's 1 MiB argument cap. */
export const MAX_LAYOUT_BYTES = (MAX_ARGS_BYTES / 8) * 7;

export type ProjectPhase = "checking" | "computing" | "saving";

export interface ProjectResult {
  layout: BaseLayout;
  source: "cache" | "computed";
  /** Computed, but not remembered. */
  saved: boolean;
}

/** What is stored: the key it was computed for, then `[documentId, x, y]` rows and `[projectName, x, y]`
 *  rows (no repeated field names: 5,000 documents are about 100 KB). */
interface StoredLayout {
  v: number;
  key: string;
  docs: [number, number, number][];
  hubs: [string, number, number][];
}

const round2 = (v: number) => Math.round(v * 100) / 100;

const isRow = (
  row: unknown,
  first: "number" | "string",
): row is [number | string, number, number] =>
  Array.isArray(row) &&
  row.length === 3 &&
  (first === "number" ? isCount(row[0]) : typeof row[0] === "string") &&
  Number.isFinite(row[1]) &&
  Number.isFinite(row[2]);

/** The positions a stored value holds for exactly this graph, or null: the wrong version or key, a row
 *  that is not a position, or a node of the graph with no position all mean "no cache". */
export function readStored(value: unknown, key: string, graph: Graph): NodePosition[] | null {
  if (typeof value !== "object" || value === null) return null;
  const s = value as Partial<StoredLayout>;
  if (s.v !== PROJECT_LAYOUT_VERSION || s.key !== key) return null;
  if (!Array.isArray(s.docs) || !Array.isArray(s.hubs)) return null;
  const positions: NodePosition[] = [];
  for (const row of s.docs) {
    if (!isRow(row, "number")) return null;
    positions.push({ id: `doc:${row[0]}`, x: row[1], y: row[2] });
  }
  for (const row of s.hubs) {
    if (!isRow(row, "string")) return null;
    positions.push({ id: `project:${row[0]}`, x: row[1], y: row[2] });
  }
  const have = new Set(positions.map((p) => p.id));
  if (have.size !== positions.length) return null;
  for (const id of graph.meta.keys()) if (!have.has(id)) return null;
  return positions;
}

/** A layout's positions as the value to store. */
export function toStored(key: string, graph: Graph, positions: NodePosition[]): StoredLayout {
  const docs: StoredLayout["docs"] = [];
  const hubs: StoredLayout["hubs"] = [];
  for (const p of positions) {
    const node = graph.meta.get(p.id);
    if (!node) continue;
    if (node.kind === "doc" && node.doc) docs.push([node.doc.id, p.x, p.y]);
    else if (node.kind === "project") hubs.push([node.project, p.x, p.y]);
  }
  return { v: PROJECT_LAYOUT_VERSION, key, docs, hubs };
}

/**
 * The by-project layout of `documents`: remembered positions when the document set is unchanged,
 * otherwise simulated in the Worker and remembered. Null for no documents. Rejects when the simulation
 * cannot run (the Worker failed): the caller says so and offers to try again.
 */
export async function loadProjectLayout(
  host: MapHost,
  compute: Compute,
  documents: MapDoc[],
  onPhase?: (phase: ProjectPhase) => void,
): Promise<ProjectResult | null> {
  if (documents.length === 0) return null;
  onPhase?.("checking");
  const graph = buildGraph(documents);
  const key = docSetKey(documents);

  // A cache that cannot be read is no cache (the old code's rule): the layout is still computed.
  let revision = 0;
  try {
    const stored = await readKey(host, KEY_PROJECT_LAYOUT);
    revision = stored.revision;
    const positions = readStored(stored.value, key, graph);
    if (positions) {
      return { layout: assemble(graph, positions), source: "cache", saved: true };
    }
  } catch {
    // Fall through to computing. The write below will likely fail too and say so as `saved: false`.
  }

  onPhase?.("computing");
  const { positions: raw } = await compute.force(forceRequest(graph));
  const positions: NodePosition[] = raw.map((p) => ({ id: p.id, x: round2(p.x), y: round2(p.y) }));
  const layout = assemble(graph, positions);

  onPhase?.("saving");
  let saved = false;
  try {
    const value = toStored(key, graph, positions);
    if (JSON.stringify(value).length <= MAX_LAYOUT_BYTES) {
      await writeKey(host, KEY_PROJECT_LAYOUT, value, revision);
      saved = true;
    }
  } catch {
    // Stale (another frame wrote), quota, rate: all cost the next open a recompute and nothing else.
  }
  return { layout, source: "computed", saved };
}
