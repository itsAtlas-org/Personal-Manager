// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { recordingContext } from "./canvasTesting";
import { drawScene, fitLabel, MIN_LABEL_RADIUS, type Scene } from "./draw";
import type { BaseLayout, PositionedNode } from "./layoutModel";
import { last } from "./testing";
import type { MapDoc } from "./types";

const COLORS = {
  border: "BORDER",
  bg: "BG",
  ink: "INK",
  ink2: "INK2",
  stLook: "LOOK",
  uiFont: "UI",
};
const colorFor = (project: string) => `c:${project}`;

const mapDoc = (id: number, over: Partial<MapDoc> = {}): MapDoc => ({
  id,
  title: `Doc ${id}`,
  project: "A",
  importance: null,
  chunkCount: 4,
  reviewed: true,
  sourceType: "vault",
  tags: [],
  ingestedAt: "2026-01-01T00:00:00Z",
  ...over,
});

const docNode = (
  id: number,
  x: number,
  y: number,
  over: Omit<Partial<PositionedNode>, "doc"> & { doc?: Partial<MapDoc> } = {},
): PositionedNode => {
  const { doc, ...rest } = over;
  return {
    id: `doc:${id}`,
    kind: "doc",
    label: `Doc ${id}`,
    project: "A",
    radius: 10,
    x,
    y,
    doc: mapDoc(id, doc),
    ...rest,
  };
};

const hub = (name: string, x: number, y: number): PositionedNode => ({
  id: `project:${name}`,
  kind: "project",
  label: name,
  project: name,
  radius: 14,
  x,
  y,
});

const layout = (nodes: PositionedNode[], edges: BaseLayout["edges"] = []): BaseLayout => ({
  nodes,
  edges,
  bounds: { minX: 0, minY: 0, width: 100, height: 100 },
  viewBox: "0 0 100 100",
  projectNames: ["A"],
});

function scene(base: BaseLayout, over: Partial<Scene> = {}): Scene {
  return {
    base,
    view: { scale: 2, offsetX: 10, offsetY: 20 },
    width: 800,
    height: 600,
    colors: COLORS,
    colorFor,
    hovered: null,
    showLabels: false,
    ...over,
  };
}

function draw(s: Scene) {
  const rec = recordingContext();
  drawScene(rec.ctx, s);
  return rec;
}

describe("fitLabel", () => {
  const ctx = recordingContext().ctx; // six pixels a character

  it("returns the whole text when it fits", () => {
    expect(fitLabel(ctx, "hello", 30)).toBe("hello");
    expect(fitLabel(ctx, "hello", 100)).toBe("hello");
  });

  it("cuts to the longest prefix that fits with an ellipsis", () => {
    const out = fitLabel(ctx, "abcdefghijklmnop", 45);
    expect(out.endsWith("…")).toBe(true);
    expect(out.length * 6).toBeLessThanOrEqual(45);
    // One more letter would not have fit.
    const longer = "abcdefghijklmnop".slice(0, out.length) + "…";
    expect(longer.length * 6).toBeGreaterThan(45);
  });

  it("returns nothing when not even one letter and the ellipsis fit", () => {
    expect(fitLabel(ctx, "abcdef", 10)).toBe("");
  });

  it("returns nothing for no room or no text", () => {
    expect(fitLabel(ctx, "abc", 0)).toBe("");
    expect(fitLabel(ctx, "abc", -5)).toBe("");
    expect(fitLabel(ctx, "", 100)).toBe("");
  });

  it("matches the core Map's trimming on a spread of widths", () => {
    // The core's fitLabel, copied here as the oracle (src/components/GraphView.tsx).
    const oracle = (text: string, maxWidth: number): string => {
      if (maxWidth <= 0 || !text) return "";
      const full = text.length * 6;
      if (full <= maxWidth) return text;
      let keep = Math.min(
        text.length - 1,
        Math.max(0, Math.floor((text.length * maxWidth) / full) - 1),
      );
      while (keep > 0 && (keep + 1) * 6 > maxWidth) keep--;
      return keep > 0 ? text.slice(0, keep) + "…" : "";
    };
    for (const text of ["a", "ab", "a long file name.pdf", "x".repeat(60)]) {
      for (let w = -2; w < 400; w += 7) expect(fitLabel(ctx, text, w)).toBe(oracle(text, w));
    }
  });

  it("matches the core Map's trimming in a font whose letters differ in width", () => {
    // Proportional text: an 'i' is narrow and a 'W' wide, so the first proportional guess is rarely
    // the answer and the trimming loop (and its starting point) really decide it.
    const width = (s: string) => Array.from(s).reduce((n, c) => n + 2 + (c.charCodeAt(0) % 7), 0);
    const proportional = recordingContext(width).ctx;
    const oracle = (text: string, maxWidth: number): string => {
      if (maxWidth <= 0 || !text) return "";
      const full = width(text);
      if (full <= maxWidth) return text;
      let keep = Math.min(
        text.length - 1,
        Math.max(0, Math.floor((text.length * maxWidth) / full) - 1),
      );
      while (keep > 0 && width(text.slice(0, keep) + "…") > maxWidth) keep--;
      return keep > 0 ? text.slice(0, keep) + "…" : "";
    };
    for (const text of [
      "iiiiiiiiiiWWWWWWWWWW",
      "WWWWWWWWWWiiiiiiiiii",
      "Quarterly plan 2026.pdf",
    ]) {
      for (let w = 1; w < 200; w += 3) {
        expect(fitLabel(proportional, text, w), `${text} in ${w}`).toBe(oracle(text, w));
      }
    }
  });
});

describe("drawScene: edges", () => {
  it("draws every edge as one path and one stroke in the border colour", () => {
    const base = layout(
      [docNode(1, 10, 10)],
      [
        { sx: 1, sy: 2, tx: 3, ty: 4 },
        { sx: 5, sy: 6, tx: 7, ty: 8 },
      ],
    );
    const rec = draw(scene(base));
    const strokes = rec.of("stroke");
    // The edges' stroke is the first one, before any dot.
    expect(strokes[0].state.strokeStyle).toBe("BORDER");
    expect(strokes[0].state.lineWidth).toBe(0.75);
    expect(rec.of("beginPath")).toHaveLength(2); // the edge path, then the one dot
    expect(rec.of("moveTo").map((c) => c.args)).toEqual([
      [12, 24],
      [20, 32],
    ]);
    expect(rec.of("lineTo").map((c) => c.args)).toEqual([
      [16, 28],
      [24, 36],
    ]);
  });

  it("draws none when the layout has none (by meaning)", () => {
    const rec = draw(scene(layout([docNode(1, 10, 10)])));
    expect(rec.of("moveTo")).toHaveLength(0);
    expect(rec.of("lineTo")).toHaveLength(0);
  });
});

describe("drawScene: dots and hubs", () => {
  it("places a dot at its world position through the camera, with its radius scaled", () => {
    const rec = draw(scene(layout([docNode(1, 30, 40)])));
    const arc = rec.of("arc")[0];
    expect(arc.args).toEqual([30 * 2 + 10, 40 * 2 + 20, 10 * 2, 0, Math.PI * 2]);
  });

  it("fills a dot with its project's colour at 85%, ringed in the background colour", () => {
    const rec = draw(scene(layout([docNode(1, 0, 0, { project: "Zed" })])));
    expect(rec.of("fill")[0].state).toMatchObject({ fillStyle: "c:Zed", globalAlpha: 0.85 });
    const ring = rec.of("stroke")[0];
    expect(ring.state).toMatchObject({ strokeStyle: "BG", lineWidth: 1, lineDash: [] });
  });

  it("rings a document that is awaiting review with a dashed ring in the look colour", () => {
    const rec = draw(scene(layout([docNode(1, 0, 0, { doc: { reviewed: false } })])));
    expect(rec.of("stroke")[0].state).toMatchObject({
      strokeStyle: "LOOK",
      lineWidth: 1.5,
      lineDash: [2, 2],
    });
    // And the dash is cleared for whatever is drawn next.
    expect(last(rec.of("setLineDash"))?.args[0]).toEqual([]);
  });

  it("rings an index-only document dashed even when it is reviewed", () => {
    const rec = draw(
      scene(layout([docNode(1, 0, 0, { doc: { reviewed: true, sourceType: "index_only" } })])),
    );
    expect(rec.of("stroke")[0].state.lineDash).toEqual([2, 2]);
  });

  it("draws a dot that has no place yet faded to 32%", () => {
    const rec = draw(scene(layout([docNode(1, 0, 0, { transient: true })])));
    expect(rec.of("fill")[0].state.globalAlpha).toBe(0.32);
    // Its ring is at full strength again.
    expect(rec.of("stroke")[0].state.globalAlpha).toBe(1);
  });

  it("draws a project hub as a 25% disc with a project-coloured ring and its name above", () => {
    const rec = draw(scene(layout([hub("Atlas", 50, 60)])));
    const fill = rec.of("fill")[0];
    expect(fill.state).toMatchObject({ fillStyle: "c:Atlas", globalAlpha: 0.25 });
    const ring = rec.of("stroke")[0];
    expect(ring.state).toMatchObject({ strokeStyle: "c:Atlas", lineWidth: 1.5, globalAlpha: 1 });
    const text = rec.of("fillText")[0];
    // 14 * 2 is the hub's radius on screen; the name sits 4 px above it.
    expect(text.args).toEqual(["Atlas", 50 * 2 + 10, 60 * 2 + 20 - 14 * 2 - 4]);
    expect(text.state).toMatchObject({
      fillStyle: "INK2",
      font: "600 22px UI",
      textAlign: "center",
      textBaseline: "alphabetic",
    });
  });

  it("draws every node in order", () => {
    const base = layout([hub("A", 0, 0), docNode(1, 5, 5), docNode(2, 9, 9)]);
    expect(draw(scene(base)).of("arc")).toHaveLength(3);
  });
});

describe("drawScene: the hovered dot", () => {
  it("is drawn again on top, solid, with an ink ring two pixels wide", () => {
    const a = docNode(1, 0, 0);
    const b = docNode(2, 40, 0, { project: "B" });
    const rec = draw(scene(layout([a, b]), { hovered: b }));
    const arcs = rec.of("arc");
    expect(arcs).toHaveLength(3);
    expect(arcs[2].args).toEqual(arcs[1].args);
    const lastFill = last(rec.of("fill"));
    expect(lastFill?.state).toMatchObject({ fillStyle: "c:B", globalAlpha: 1 });
    const lastStroke = last(rec.of("stroke"));
    expect(lastStroke?.state).toMatchObject({ strokeStyle: "INK", lineWidth: 2, lineDash: [] });
  });

  it("draws nothing extra with no hover", () => {
    expect(draw(scene(layout([docNode(1, 0, 0)]))).of("arc")).toHaveLength(1);
  });
});

describe("drawScene: file names", () => {
  // A dot whose radius on screen is 20 (10 world x scale 2): big enough to carry a name.
  const named = (over: Partial<PositionedNode> = {}) =>
    layout([docNode(1, 100, 100, { label: "Quarterly plan", ...over })]);

  it("draws none unless asked", () => {
    const rec = draw(scene(named(), { showLabels: false }));
    expect(rec.of("strokeText")).toHaveLength(0);
    expect(rec.of("fillText")).toHaveLength(0);
  });

  it("draws a name with a background halo under ink, centred in the dot", () => {
    const rec = draw(scene(named(), { showLabels: true }));
    const halo = rec.of("strokeText")[0];
    const ink = rec.of("fillText")[0];
    expect(halo.args[1]).toBe(100 * 2 + 10);
    expect(halo.args[2]).toBe(100 * 2 + 20);
    expect(halo.state).toMatchObject({
      strokeStyle: "BG",
      textAlign: "center",
      textBaseline: "middle",
    });
    expect(ink.state.fillStyle).toBe("INK");
    expect(rec.calls.indexOf(halo)).toBeLessThan(rec.calls.indexOf(ink));
  });

  it("sizes the text to the dot and cuts a long name to the dot's width", () => {
    // Radius on screen 20: font 5 px, room 36 px, six pixels a character.
    const rec = draw(
      scene(named({ label: "A very long document name.pdf" }), { showLabels: true }),
    );
    const text = rec.of("fillText")[0].args[0] as string;
    expect(text.endsWith("…")).toBe(true);
    expect(text.length * 6).toBeLessThanOrEqual(36);
    expect(rec.of("fillText")[0].state.font).toBe("500 5px UI");
    expect(rec.of("strokeText")[0].state.lineWidth).toBe(1.5);
  });

  it("skips a dot too small on screen to read (and draws one at the limit)", () => {
    // The scene's scale is 2: a radius of 5.5 is 11 px on screen, 6 is 12 px.
    expect(MIN_LABEL_RADIUS).toBe(12);
    const small = layout([docNode(1, 100, 100, { radius: 5.5 })]);
    expect(draw(scene(small, { showLabels: true })).of("fillText")).toHaveLength(0);
    const edge = layout([docNode(1, 100, 100, { radius: 6 })]);
    expect(draw(scene(edge, { showLabels: true })).of("fillText")).toHaveLength(1);
  });

  it("skips a dot wholly off the canvas", () => {
    for (const [x, y] of [
      [-400, 100],
      [900, 100],
      [100, -400],
      [100, 800],
    ]) {
      const off = layout([docNode(1, x, y)]);
      expect(draw(scene(off, { showLabels: true })).of("fillText")).toHaveLength(0);
    }
  });

  it("draws no name for a project hub", () => {
    const rec = draw(scene(layout([hub("Atlas", 100, 100)]), { showLabels: true }));
    // The hub's own name above it is the only text, and it is the same with or without names.
    expect(rec.of("fillText")).toHaveLength(1);
    expect(rec.of("strokeText")).toHaveLength(0);
  });

  it("skips a name that has no room for a letter", () => {
    const rec = draw(scene(named({ label: "" }), { showLabels: true }));
    expect(rec.of("fillText")).toHaveLength(0);
  });
});
