// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Map's camera and its hit test, as pure functions: where the world sits on the screen, how a zoom
// or a drag moves it, and which dot is under a point. Ported from GraphView.tsx (the core Map), whose
// numbers they keep: the zoom range (half the fit out to sixteen times it), the six pixels of slop
// round a dot, the quadtree search radius.

import { quadtree } from "d3-quadtree";
import type { Bounds, PositionedNode } from "./layoutModel";

export interface Transform {
  scale: number;
  offsetX: number;
  offsetY: number;
}

/** Zoom out to no less than this fraction of the fit, in to no more than this multiple of it. */
export const MIN_ZOOM = 0.5;
export const MAX_ZOOM = 16;
/** A press that moves no further than this many pixels is a click, not a drag. */
export const CLICK_SLOP = 3;
/** Screen pixels of forgiveness round a dot, so a small dot is still easy to hit when zoomed out. */
export const HIT_SLOP = 6;

/** Fit the world `bounds` into a `w` x `h` pixel canvas, centred. */
export function fitTransform(bounds: Bounds, w: number, h: number): Transform {
  const scale = Math.min(w / bounds.width, h / bounds.height);
  return {
    scale,
    offsetX: (w - bounds.width * scale) / 2 - bounds.minX * scale,
    offsetY: (h - bounds.height * scale) / 2 - bounds.minY * scale,
  };
}

/** Zoom about a screen point by `factor`, kept between MIN_ZOOM and MAX_ZOOM times the fit's scale. */
export function zoomAbout(
  t: Transform,
  fitScale: number,
  px: number,
  py: number,
  factor: number,
): Transform {
  const wx = (px - t.offsetX) / t.scale;
  const wy = (py - t.offsetY) / t.scale;
  const scale = Math.min(fitScale * MAX_ZOOM, Math.max(fitScale * MIN_ZOOM, t.scale * factor));
  return { scale, offsetX: px - wx * scale, offsetY: py - wy * scale };
}

/** The camera moved by a screen distance. */
export const panBy = (t: Transform, dx: number, dy: number): Transform => ({
  ...t,
  offsetX: t.offsetX + dx,
  offsetY: t.offsetY + dy,
});

/** A wheel turn's zoom factor: smooth, and the same for a turn up as its reverse turn down. */
export const wheelFactor = (deltaY: number): number => Math.exp(-deltaY * 0.0015);

/** Whether a wheel turn is mostly sideways (a trackpad side-swipe, Shift+wheel): that pans. */
export const isSidewaysWheel = (deltaX: number, deltaY: number): boolean =>
  Math.abs(deltaX) > Math.abs(deltaY);

/** The dot under a screen point, or null. Built once per layout over its document dots. */
export interface HitTester {
  at(screenX: number, screenY: number, t: Transform): PositionedNode | null;
}

export function hitTester(nodes: readonly PositionedNode[]): HitTester {
  const tree = quadtree<PositionedNode>()
    .x((n) => n.x)
    .y((n) => n.y)
    .addAll(nodes.filter((n) => n.kind === "doc"));
  return {
    at(screenX, screenY, t) {
      const wx = (screenX - t.offsetX) / t.scale;
      const wy = (screenY - t.offsetY) / t.scale;
      // A constant few screen pixels of slop on top of the dot's own radius, in world units.
      const tol = HIT_SLOP / t.scale;
      // 16 is the largest a dot's radius gets (docRadius caps there); 2 covers its stroke.
      const hit = tree.find(wx, wy, 16 + tol + 2);
      if (!hit) return null;
      return Math.hypot(hit.x - wx, hit.y - wy) <= hit.radius + tol ? hit : null;
    },
  };
}
