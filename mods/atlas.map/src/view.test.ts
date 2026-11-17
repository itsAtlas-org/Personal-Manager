// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import type { PositionedNode } from "./layoutModel";
import {
  fitTransform,
  hitTester,
  isSidewaysWheel,
  MAX_ZOOM,
  MIN_ZOOM,
  panBy,
  wheelFactor,
  zoomAbout,
  type Transform,
} from "./view";

const node = (id: number, x: number, y: number, radius = 8, kind: "doc" | "project" = "doc") =>
  ({
    id: kind === "doc" ? `doc:${id}` : `project:${id}`,
    kind,
    label: `n${id}`,
    project: "P",
    radius,
    x,
    y,
  }) as PositionedNode;

describe("fitTransform", () => {
  it("scales the world to the smaller side and centres it on the other", () => {
    const bounds = { minX: -50, minY: -50, width: 200, height: 100 };
    const t = fitTransform(bounds, 400, 400);
    expect(t.scale).toBe(2);
    // The world's 200 x 100 becomes 400 x 200: flush left and right, centred vertically.
    expect(t.offsetX).toBe(100);
    expect(t.offsetY).toBe(200);
    expect(bounds.minX * t.scale + t.offsetX).toBe(0);
    expect(bounds.minY * t.scale + t.offsetY).toBe(100);
  });

  it("maps the box's corners inside the canvas", () => {
    const bounds = { minX: 10, minY: 20, width: 300, height: 600 };
    const t = fitTransform(bounds, 800, 300);
    const left = bounds.minX * t.scale + t.offsetX;
    const right = (bounds.minX + bounds.width) * t.scale + t.offsetX;
    const top = bounds.minY * t.scale + t.offsetY;
    const bottom = (bounds.minY + bounds.height) * t.scale + t.offsetY;
    expect(top).toBeCloseTo(0);
    expect(bottom).toBeCloseTo(300);
    expect(left).toBeCloseTo((800 - 150) / 2);
    expect(right).toBeCloseTo((800 + 150) / 2);
  });
});

describe("zoomAbout", () => {
  const t: Transform = { scale: 1, offsetX: 30, offsetY: 40 };

  it("keeps the world point under the pointer where it was", () => {
    const next = zoomAbout(t, 1, 200, 150, 2);
    const before = [(200 - t.offsetX) / t.scale, (150 - t.offsetY) / t.scale];
    const after = [(200 - next.offsetX) / next.scale, (150 - next.offsetY) / next.scale];
    expect(next.scale).toBe(2);
    expect(after[0]).toBeCloseTo(before[0]);
    expect(after[1]).toBeCloseTo(before[1]);
  });

  it("stops at sixteen times the fit when zooming in", () => {
    expect(zoomAbout({ ...t, scale: 15 }, 1, 0, 0, 3).scale).toBe(MAX_ZOOM);
    expect(MAX_ZOOM).toBe(16);
  });

  it("stops at half the fit when zooming out", () => {
    expect(zoomAbout({ ...t, scale: 0.6 }, 1, 0, 0, 0.1).scale).toBe(MIN_ZOOM);
    expect(MIN_ZOOM).toBe(0.5);
  });

  it("measures the limits against the fit's scale, not against 1", () => {
    expect(zoomAbout({ ...t, scale: 0.1 }, 0.1, 0, 0, 1000).scale).toBeCloseTo(1.6);
    expect(zoomAbout({ ...t, scale: 0.1 }, 0.1, 0, 0, 0.0001).scale).toBeCloseTo(0.05);
  });
});

describe("panBy and the wheel", () => {
  it("moves the offset and nothing else", () => {
    expect(panBy({ scale: 2, offsetX: 1, offsetY: 2 }, 10, -5)).toEqual({
      scale: 2,
      offsetX: 11,
      offsetY: -3,
    });
  });

  it("zooms in for a turn up and out for a turn down, by the same amount each way", () => {
    expect(wheelFactor(0)).toBe(1);
    expect(wheelFactor(-100)).toBeGreaterThan(1);
    expect(wheelFactor(100)).toBeLessThan(1);
    expect(wheelFactor(-100) * wheelFactor(100)).toBeCloseTo(1, 12);
    expect(wheelFactor(-100)).toBeCloseTo(Math.exp(0.15), 12);
  });

  it("pans only when the turn is mostly sideways", () => {
    expect(isSidewaysWheel(10, 3)).toBe(true);
    expect(isSidewaysWheel(-10, 3)).toBe(true);
    expect(isSidewaysWheel(3, 10)).toBe(false);
    expect(isSidewaysWheel(5, 5)).toBe(false);
    expect(isSidewaysWheel(0, 0)).toBe(false);
  });
});

describe("hitTester", () => {
  const nodes = [node(1, 100, 100, 8), node(2, 300, 100, 16), node(9, 100, 300, 14, "project")];
  const t: Transform = { scale: 1, offsetX: 0, offsetY: 0 };
  const hits = hitTester(nodes);

  it("finds the dot under the pointer", () => {
    expect(hits.at(100, 100, t)?.id).toBe("doc:1");
    expect(hits.at(300, 100, t)?.id).toBe("doc:2");
  });

  it("allows six screen pixels of slop beyond a dot's own radius", () => {
    expect(hits.at(100 + 8 + 5.9, 100, t)?.id).toBe("doc:1");
    expect(hits.at(100 + 8 + 6.1, 100, t)).toBeNull();
  });

  it("keeps the slop in screen pixels as the map zooms", () => {
    const zoomed: Transform = { scale: 4, offsetX: 0, offsetY: 0 };
    // At 4x the dot is 32 px wide on screen: its edge is at 100*4+32, and the slop is 6 more pixels.
    expect(hits.at(400 + 32 + 5.9, 400, zoomed)?.id).toBe("doc:1");
    expect(hits.at(400 + 32 + 6.1, 400, zoomed)).toBeNull();
    const out: Transform = { scale: 0.25, offsetX: 0, offsetY: 0 };
    // Zoomed out, the dot is 2 px; the 6 px of slop is 24 world units.
    expect(hits.at(25 + 2 + 5.9, 25, out)?.id).toBe("doc:1");
    expect(hits.at(25 + 2 + 6.1, 25, out)).toBeNull();
  });

  it("follows the camera's offset", () => {
    const moved: Transform = { scale: 1, offsetX: 50, offsetY: -20 };
    expect(hits.at(150, 80, moved)?.id).toBe("doc:1");
    expect(hits.at(100, 100, moved)).toBeNull();
  });

  it("never hits a project hub, only documents", () => {
    expect(hits.at(100, 300, t)).toBeNull();
  });

  it("finds nothing in empty space or in an empty layout", () => {
    expect(hits.at(700, 700, t)).toBeNull();
    expect(hitTester([]).at(0, 0, t)).toBeNull();
  });

  it("finds the nearest of two dots that both reach the pointer", () => {
    const close = hitTester([node(1, 100, 100, 12), node(2, 110, 100, 12)]);
    expect(close.at(103, 100, t)?.id).toBe("doc:1");
    expect(close.at(107, 100, t)?.id).toBe("doc:2");
  });
});
