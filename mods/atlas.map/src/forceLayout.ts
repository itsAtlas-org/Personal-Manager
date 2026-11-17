// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The "by project" force simulation, ported from the core Map (src/lib/forceLayout.ts) with no change
// to what it computes: the same d3-force forces, the same numbers, the same tick count. Pure (no DOM, no
// worker global), so it runs in the layout Worker and is tested directly. The core's tests of the old
// code (src/lib/mapLayout.test.ts) and this port's (forceLayout.test.ts) hold the same literals.

import {
  forceCenter,
  forceCollide,
  forceLink,
  forceManyBody,
  forceSimulation,
  forceX,
  forceY,
  type SimulationLinkDatum,
  type SimulationNodeDatum,
} from "d3-force";

interface SimNode extends SimulationNodeDatum {
  id: string;
  radius: number;
}
type SimLink = SimulationLinkDatum<SimNode>;

export interface LayoutRequest {
  nodes: { id: string; radius: number }[];
  links: { source: string; target: string }[];
  width: number;
  height: number;
}

export interface LayoutResponse {
  positions: { id: string; x: number; y: number }[];
}

/** Nodes above which the run is shortened (it is visually converged well before 400 ticks). */
export const BIG_MAP_NODES = 400;
export const TICKS = 400;
export const TICKS_BIG = 250;

/** Run the force layout to a converged set of positions. The heavy step: run it in a Worker. */
export function simulate(req: LayoutRequest): LayoutResponse {
  const { nodes: rawNodes, links: rawLinks, width, height } = req;

  // Fresh mutable copies: d3-force writes x/y onto these objects in place.
  const nodes: SimNode[] = rawNodes.map((n) => ({ id: n.id, radius: n.radius }));
  const links: SimLink[] = rawLinks.map((l) => ({ source: l.source, target: l.target }));

  const sim = forceSimulation(nodes)
    .force(
      "link",
      forceLink<SimNode, SimLink>(links)
        .id((n) => n.id)
        .distance(34)
        .strength(0.8),
    )
    // Repulsion gives the cluster air, but its many-body strength is *additive*, so a project with
    // hundreds of documents (a fresh bulk import) otherwise inflates into a vast, sparse disc.
    // `distanceMax` caps how far a node's charge reaches: nodes only push their near neighbours apart,
    // so a big cluster packs to roughly the collide limit instead of ballooning, while small clusters
    // are unchanged. `collide` (radius + a small pad) is what actually prevents overlap.
    .force("charge", forceManyBody().strength(-80).distanceMax(140))
    .force("center", forceCenter(width / 2, height / 2))
    .force(
      "collide",
      forceCollide<SimNode>().radius((n) => n.radius + 2),
    )
    .force("x", forceX(width / 2).strength(0.06))
    .force("y", forceY(height / 2).strength(0.06))
    .stop();

  const ticks = nodes.length > BIG_MAP_NODES ? TICKS_BIG : TICKS;
  for (let i = 0; i < ticks; i++) sim.tick();

  return { positions: nodes.map((n) => ({ id: n.id, x: n.x ?? 0, y: n.y ?? 0 })) };
}
