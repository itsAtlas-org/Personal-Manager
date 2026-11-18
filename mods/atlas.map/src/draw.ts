// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// One frame of the Map on a 2-D canvas context: edges, hubs, dots, the hovered dot, and the opt-in file
// names. Ported from GraphView.tsx's `draw` with the same numbers, split from the page so a test can run
// it against a recording context. It reads everything from its arguments (no DOM, no clock) and draws
// with the colour strings the caller resolved: a canvas cannot read `var(--...)`.

import { isDashed, type BaseLayout, type PositionedNode } from "./layoutModel";
import type { CanvasColors } from "./theme";
import type { Transform } from "./view";

export interface Scene {
  base: BaseLayout;
  view: Transform;
  /** The canvas's size in CSS pixels. */
  width: number;
  height: number;
  colors: CanvasColors;
  /** A project's colour: the palette's, by the project's place in the legend. */
  colorFor: (project: string) => string;
  /** The dot to draw lifted, if any. */
  hovered: PositionedNode | null;
  showLabels: boolean;
}

/** A dot's label is drawn only when its radius on screen is at least this: smaller is unreadable, and
 *  skipping it bounds how many labels a frame draws on a big library. */
export const MIN_LABEL_RADIUS = 12;

/** The widest prefix of `text` (with a trailing ellipsis if shortened) that fits `maxWidth` pixels in the
 *  context's current font, or "" if not even one character does. A proportional first guess keeps the
 *  trim short. */
export function fitLabel(
  ctx: Pick<CanvasRenderingContext2D, "measureText">,
  text: string,
  maxWidth: number,
): string {
  if (maxWidth <= 0 || !text) return "";
  const full = ctx.measureText(text).width;
  if (full <= maxWidth) return text;
  let keep = Math.min(
    text.length - 1,
    Math.max(0, Math.floor((text.length * maxWidth) / full) - 1),
  );
  while (keep > 0 && ctx.measureText(text.slice(0, keep) + "…").width > maxWidth) keep--;
  return keep > 0 ? text.slice(0, keep) + "…" : "";
}

export function drawScene(ctx: CanvasRenderingContext2D, scene: Scene): void {
  const { base, view: t, colors, colorFor, hovered, showLabels } = scene;
  const sx = (x: number) => x * t.scale + t.offsetX;
  const sy = (y: number) => y * t.scale + t.offsetY;

  // Edges (by project only; one path, one stroke). Dot radii scale with the fit (size is how much a
  // document holds, a real signal); stroke and edge widths stay fixed in screen pixels, as crisp chrome.
  if (base.edges.length > 0) {
    ctx.strokeStyle = colors.border;
    ctx.lineWidth = 0.75;
    ctx.beginPath();
    for (const e of base.edges) {
      ctx.moveTo(sx(e.sx), sy(e.sy));
      ctx.lineTo(sx(e.tx), sy(e.ty));
    }
    ctx.stroke();
    ctx.setLineDash([]);
  }

  for (const n of base.nodes) {
    const color = colorFor(n.project);
    const r = n.radius * t.scale;
    if (n.kind === "project") {
      ctx.globalAlpha = 0.25;
      ctx.fillStyle = color;
      ctx.beginPath();
      ctx.arc(sx(n.x), sy(n.y), r, 0, Math.PI * 2);
      ctx.fill();
      ctx.globalAlpha = 1;
      ctx.strokeStyle = color;
      ctx.lineWidth = 1.5;
      ctx.stroke();
      ctx.fillStyle = colors.ink2;
      ctx.font = `600 ${11 * t.scale}px ${colors.uiFont}`;
      ctx.textAlign = "center";
      ctx.textBaseline = "alphabetic";
      ctx.fillText(n.label, sx(n.x), sy(n.y) - r - 4);
    } else {
      const dashed = isDashed(n.doc);
      // A dot not yet projected (new since the last compute, or past the cap) sits at its project's
      // centre; it is drawn faded until a recompute places it.
      ctx.globalAlpha = n.transient ? 0.32 : 0.85;
      ctx.fillStyle = color;
      ctx.beginPath();
      ctx.arc(sx(n.x), sy(n.y), r, 0, Math.PI * 2);
      ctx.fill();
      ctx.globalAlpha = 1;
      ctx.strokeStyle = dashed ? colors.stLook : colors.bg;
      ctx.lineWidth = dashed ? 1.5 : 1;
      ctx.setLineDash(dashed ? [2, 2] : []);
      ctx.stroke();
      ctx.setLineDash([]);
    }
  }

  if (hovered) {
    ctx.globalAlpha = 1;
    ctx.fillStyle = colorFor(hovered.project);
    ctx.beginPath();
    ctx.arc(sx(hovered.x), sy(hovered.y), hovered.radius * t.scale, 0, Math.PI * 2);
    ctx.fill();
    ctx.strokeStyle = colors.ink;
    ctx.lineWidth = 2;
    ctx.setLineDash([]);
    ctx.stroke();
  }

  // File names centred inside each dot (opt-in). Sized to the dot, so about ten to fifteen characters
  // fit and the text sits inside the circle at any zoom, and cut to the dot's width, so a long name
  // shows its start (zoom into a bigger dot to read more). A halo in the background colour keeps it
  // legible on any dot colour; dots off screen or too small to read are skipped.
  if (showLabels) {
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.lineJoin = "round";
    ctx.setLineDash([]);
    for (const n of base.nodes) {
      if (n.kind !== "doc") continue;
      const r = n.radius * t.scale;
      if (r < MIN_LABEL_RADIUS) continue;
      const cx = sx(n.x);
      const cy = sy(n.y);
      if (cx < -r || cx > scene.width + r || cy < -r || cy > scene.height + r) continue;
      const fontPx = r * 0.25;
      ctx.font = `500 ${fontPx}px ${colors.uiFont}`;
      const text = fitLabel(ctx, n.label, r * 1.8);
      if (!text) continue;
      ctx.lineWidth = Math.max(1.5, fontPx * 0.14);
      ctx.strokeStyle = colors.bg;
      ctx.strokeText(text, cx, cy);
      ctx.fillStyle = colors.ink;
      ctx.fillText(text, cx, cy);
    }
  }
}
