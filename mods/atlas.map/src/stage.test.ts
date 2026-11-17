// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The canvas and what a hand does to it, in jsdom with a recording 2-D context. The layout's box is the
// stage's 800 x 500 and its world is 800 x 500 too, so at the fit the world IS the screen and every
// coordinate below can be read straight off.

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { domTesting } from "./domTesting";
import { createStage, type Stage } from "./stage";
import type { BaseLayout, PositionedNode } from "./layoutModel";
import { last } from "./testing";
import type { MapDoc } from "./types";

const COLORS = { border: "B", bg: "G", ink: "I", ink2: "I2", stLook: "L", uiFont: "UI" };
const look = (showLabels = false) => ({ colors: COLORS, colorFor: () => "red", showLabels });

const mapDoc = (id: number): MapDoc => ({
  id,
  title: `Doc ${id}`,
  project: "A",
  importance: null,
  chunkCount: 4,
  reviewed: true,
  sourceType: "vault",
  tags: [],
  ingestedAt: "",
});
const dot = (id: number, x: number, y: number): PositionedNode => ({
  id: `doc:${id}`,
  kind: "doc",
  label: `Doc ${id}`,
  project: "A",
  radius: 10,
  x,
  y,
  doc: mapDoc(id),
});
const layout = (nodes: PositionedNode[]): BaseLayout => ({
  nodes,
  edges: [],
  bounds: { minX: 0, minY: 0, width: 800, height: 500 },
  viewBox: "0 0 800 500",
  projectNames: ["A"],
});
const BASE = () => layout([dot(1, 200, 200), dot(2, 500, 300)]);

const rig = domTesting();

interface Harness {
  stage: Stage;
  canvas: HTMLCanvasElement;
  wrap: HTMLElement;
  hovered: (MapDoc | null)[];
  opened: MapDoc[];
}

function make(): Harness {
  const wrap = document.createElement("div");
  wrap.className = "stage";
  const canvas = document.createElement("canvas");
  wrap.append(canvas);
  document.body.append(wrap);
  const hovered: (MapDoc | null)[] = [];
  const opened: MapDoc[] = [];
  const stage = createStage({
    wrap,
    canvas,
    win: window as Window & typeof globalThis,
    onHover: (d) => hovered.push(d),
    onOpen: (d) => opened.push(d),
  });
  return { stage, canvas, wrap, hovered, opened };
}

/** Where the first dot's circle was last drawn: [x, y, r]. */
const firstDot = () => {
  const arcs = rig.canvas().of("arc");
  // The scene is cleared by clearRect; the last frame is whatever follows the last clearRect.
  const clears = rig.canvas().of("clearRect");
  const lastClear = rig.canvas().calls.indexOf(clears[clears.length - 1]);
  const after = rig
    .canvas()
    .calls.slice(lastClear)
    .filter((c) => c.op === "arc");
  return (after[0] ?? arcs[0]).args.slice(0, 3) as number[];
};

const mouse = (type: string, init: MouseEventInit = {}) =>
  new MouseEvent(type, { bubbles: true, cancelable: true, ...init });

beforeEach(() => rig.install());
afterEach(() => {
  rig.uninstall();
  document.body.replaceChildren();
});

describe("drawing", () => {
  it("sizes the canvas to the box at the screen's pixel density and draws the fit", () => {
    rig.density(2);
    const h = make();
    h.stage.setLook(look());
    h.stage.setBase(BASE());
    rig.frames().run();
    expect(h.canvas.width).toBe(1600);
    expect(h.canvas.height).toBe(1000);
    const t = rig.canvas().of("setTransform")[0];
    expect(t.args).toEqual([2, 0, 0, 2, 0, 0]);
    expect(rig.canvas().of("clearRect")[0].args).toEqual([0, 0, 800, 500]);
    expect(firstDot()).toEqual([200, 200, 10]);
  });

  it("draws nothing until it has both a layout and a look", () => {
    const h = make();
    h.stage.setBase(BASE());
    rig.frames().run();
    expect(rig.canvas().of("arc")).toHaveLength(0);
    h.stage.setLook(look());
    rig.frames().run();
    expect(rig.canvas().of("arc")).toHaveLength(2);
  });

  it("draws nothing for no layout", () => {
    const h = make();
    h.stage.setLook(look());
    h.stage.setBase(null);
    rig.frames().run();
    expect(rig.canvas().of("clearRect")).toHaveLength(0);
  });

  it("draws nothing while its box has no size, and draws once it has", () => {
    const h = make();
    rig.size(h.wrap, 0, 0);
    h.stage.setLook(look());
    h.stage.setBase(BASE());
    rig.frames().run();
    expect(rig.canvas().of("arc")).toHaveLength(0);
    rig.size(h.wrap, 800, 500);
    h.stage.redraw();
    rig.frames().run();
    expect(rig.canvas().of("arc")).toHaveLength(2);
  });

  it("asks for one animation frame however many redraws are asked for", () => {
    const h = make();
    h.stage.setLook(look());
    h.stage.setBase(BASE());
    h.stage.redraw();
    h.stage.redraw();
    h.stage.setLook(look(true));
    expect(rig.framesPending()).toBe(1);
    rig.frames().run();
    expect(rig.canvas().of("clearRect")).toHaveLength(1);
  });

  it("keeps the view when only the look changes, and fits again for a new layout", () => {
    const h = make();
    h.stage.setLook(look());
    h.stage.setBase(BASE());
    rig.frames().run();
    h.wrap.dispatchEvent(
      new WheelEvent("wheel", { deltaY: -200, clientX: 0, clientY: 0, cancelable: true }),
    );
    rig.frames().run();
    const zoomed = firstDot();
    expect(zoomed[2]).toBeGreaterThan(10);
    h.stage.setLook(look(true));
    rig.frames().run();
    expect(firstDot()).toEqual(zoomed);
    h.stage.setBase(BASE());
    rig.frames().run();
    expect(firstDot()).toEqual([200, 200, 10]);
  });
});

describe("zoom and pan", () => {
  function ready() {
    const h = make();
    h.stage.setLook(look());
    h.stage.setBase(BASE());
    rig.frames().run();
    return h;
  }

  it("zooms toward the pointer on a vertical wheel and stops the page scrolling", () => {
    const h = ready();
    const event = new WheelEvent("wheel", {
      deltaY: -200,
      clientX: 200,
      clientY: 200,
      cancelable: true,
    });
    h.wrap.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    rig.frames().run();
    const [x, y, r] = firstDot();
    // The dot under the pointer stays under it, and grows by the wheel's factor.
    expect(x).toBeCloseTo(200);
    expect(y).toBeCloseTo(200);
    expect(r).toBeCloseTo(10 * Math.exp(0.3));
  });

  it("zooms out on a wheel turned the other way, no further than half the fit", () => {
    const h = ready();
    for (let i = 0; i < 30; i++) {
      h.wrap.dispatchEvent(
        new WheelEvent("wheel", { deltaY: 400, clientX: 0, clientY: 0, cancelable: true }),
      );
    }
    rig.frames().run();
    expect(firstDot()[2]).toBeCloseTo(5);
  });

  it("zooms in no further than sixteen times the fit", () => {
    const h = ready();
    for (let i = 0; i < 30; i++) {
      h.wrap.dispatchEvent(
        new WheelEvent("wheel", { deltaY: -400, clientX: 0, clientY: 0, cancelable: true }),
      );
    }
    rig.frames().run();
    expect(firstDot()[2]).toBeCloseTo(160);
  });

  it("pans on a mostly sideways wheel, scrolling right to reveal what is on the right", () => {
    const h = ready();
    const event = new WheelEvent("wheel", { deltaX: 30, deltaY: 5, cancelable: true });
    h.wrap.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    rig.frames().run();
    expect(firstDot()).toEqual([170, 200, 10]);
  });

  it("zooms about the middle of the canvas from the buttons", () => {
    const h = ready();
    h.stage.zoom(2);
    rig.frames().run();
    // The world point at the canvas's centre (400, 250) stays there: dot 1 at (200,200) moves to (0,150).
    expect(firstDot()).toEqual([0, 150, 20]);
    h.stage.zoom(0.5);
    rig.frames().run();
    expect(firstDot()).toEqual([200, 200, 10]);
  });

  it("ignores the wheel before there is a layout", () => {
    const h = make();
    const event = new WheelEvent("wheel", { deltaY: -200, cancelable: true });
    h.wrap.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
  });

  it("pans with a drag, tracking the pointer past the canvas's edge", () => {
    const h = ready();
    h.canvas.dispatchEvent(
      mouse("mousedown", { button: 0, clientX: 100, clientY: 100, buttons: 1 }),
    );
    expect(h.canvas.classList.contains("is-grabbing")).toBe(true);
    // The moves land on the window, not the canvas.
    window.dispatchEvent(mouse("mousemove", { clientX: 130, clientY: 90, buttons: 1 }));
    window.dispatchEvent(mouse("mousemove", { clientX: 160, clientY: 120, buttons: 1 }));
    rig.frames().run();
    expect(firstDot()).toEqual([260, 220, 10]);
    window.dispatchEvent(mouse("mouseup", { clientX: 160, clientY: 120 }));
    expect(h.canvas.classList.contains("is-grabbing")).toBe(false);
    // A drag is not a click.
    expect(h.opened).toEqual([]);
  });

  it("keeps the person's framing through a resize, and fits again after a double-click", () => {
    const h = ready();
    h.canvas.dispatchEvent(mouse("mousedown", { button: 0, clientX: 0, clientY: 0, buttons: 1 }));
    window.dispatchEvent(mouse("mousemove", { clientX: 50, clientY: 0, buttons: 1 }));
    window.dispatchEvent(mouse("mouseup", { clientX: 50, clientY: 0 }));
    rig.frames().run();
    expect(firstDot()).toEqual([250, 200, 10]);
    rig.observers()[0].callback();
    rig.frames().run();
    expect(firstDot()).toEqual([250, 200, 10]);
    h.canvas.dispatchEvent(mouse("dblclick"));
    rig.frames().run();
    expect(firstDot()).toEqual([200, 200, 10]);
  });

  it("fits again on a resize when the person has not moved the map", () => {
    const h = ready();
    rig.size(h.wrap, 400, 250);
    rig.observers()[0].callback();
    rig.frames().run();
    // The 800 x 500 world in a 400 x 250 box: half scale.
    expect(firstDot()).toEqual([100, 100, 5]);
  });

  it("ignores a press of any button but the primary one", () => {
    const h = ready();
    h.canvas.dispatchEvent(
      mouse("mousedown", { button: 2, clientX: 100, clientY: 100, buttons: 2 }),
    );
    expect(h.canvas.classList.contains("is-grabbing")).toBe(false);
    window.dispatchEvent(mouse("mousemove", { clientX: 160, clientY: 120, buttons: 2 }));
    rig.frames().run();
    expect(firstDot()).toEqual([200, 200, 10]);
  });
});

describe("a drag that leaves the frame", () => {
  function dragging() {
    const h = make();
    h.stage.setLook(look());
    h.stage.setBase(BASE());
    rig.frames().run();
    h.canvas.dispatchEvent(
      mouse("mousedown", { button: 0, clientX: 200, clientY: 200, buttons: 1 }),
    );
    window.dispatchEvent(mouse("mousemove", { clientX: 230, clientY: 200, buttons: 1 }));
    return h;
  }

  it("ends when the next move reports no button held (the release was never heard)", () => {
    const h = dragging();
    window.dispatchEvent(mouse("mousemove", { clientX: 300, clientY: 200, buttons: 0 }));
    expect(h.canvas.classList.contains("is-grabbing")).toBe(false);
    rig.frames().run();
    // It moved with the first move only, not with the one that found the button up.
    expect(firstDot()).toEqual([230, 200, 10]);
    window.dispatchEvent(mouse("mousemove", { clientX: 400, clientY: 200, buttons: 0 }));
    rig.frames().run();
    expect(firstDot()).toEqual([230, 200, 10]);
    // And a later release opens nothing.
    window.dispatchEvent(mouse("mouseup", { clientX: 230, clientY: 200 }));
    expect(h.opened).toEqual([]);
  });

  it("ends when the window loses focus", () => {
    const h = dragging();
    window.dispatchEvent(new Event("blur"));
    expect(h.canvas.classList.contains("is-grabbing")).toBe(false);
    window.dispatchEvent(mouse("mousemove", { clientX: 400, clientY: 200, buttons: 1 }));
    rig.frames().run();
    expect(firstDot()).toEqual([230, 200, 10]);
  });
});

describe("hover and click", () => {
  function ready() {
    const h = make();
    h.stage.setLook(look());
    h.stage.setBase(BASE());
    rig.frames().run();
    return h;
  }

  it("reports the document under the pointer once, and none when it leaves", () => {
    const h = ready();
    h.canvas.dispatchEvent(mouse("mousemove", { clientX: 203, clientY: 200 }));
    h.canvas.dispatchEvent(mouse("mousemove", { clientX: 205, clientY: 201 }));
    expect(h.hovered.map((d) => d?.id)).toEqual([1]);
    expect(h.canvas.classList.contains("is-hover")).toBe(true);
    h.canvas.dispatchEvent(mouse("mousemove", { clientX: 500, clientY: 300 }));
    expect(h.hovered.map((d) => d?.id)).toEqual([1, 2]);
    h.canvas.dispatchEvent(mouse("mousemove", { clientX: 700, clientY: 450 }));
    expect(last(h.hovered)).toBeNull();
    expect(h.canvas.classList.contains("is-hover")).toBe(false);
  });

  it("draws the hovered dot lifted", () => {
    const h = ready();
    rig.canvas().clear();
    h.canvas.dispatchEvent(mouse("mousemove", { clientX: 200, clientY: 200 }));
    rig.frames().run();
    expect(rig.canvas().of("arc")).toHaveLength(3);
    expect(last(rig.canvas().of("stroke"))?.state).toMatchObject({
      strokeStyle: "I",
      lineWidth: 2,
    });
  });

  it("clears the hover when the pointer leaves the canvas", () => {
    const h = ready();
    h.canvas.dispatchEvent(mouse("mousemove", { clientX: 200, clientY: 200 }));
    h.canvas.dispatchEvent(mouse("mouseleave"));
    expect(last(h.hovered)).toBeNull();
  });

  it("clears the hover when the map is moved under the pointer", () => {
    const h = ready();
    h.canvas.dispatchEvent(mouse("mousemove", { clientX: 200, clientY: 200 }));
    h.wrap.dispatchEvent(
      new WheelEvent("wheel", { deltaY: -100, clientX: 0, clientY: 0, cancelable: true }),
    );
    expect(last(h.hovered)).toBeNull();
  });

  it("does not hover while dragging", () => {
    const h = ready();
    h.canvas.dispatchEvent(
      mouse("mousedown", { button: 0, clientX: 600, clientY: 400, buttons: 1 }),
    );
    h.canvas.dispatchEvent(mouse("mousemove", { clientX: 200, clientY: 200, buttons: 1 }));
    expect(h.hovered).toEqual([]);
  });

  it("opens the document under a press that did not move", () => {
    const h = ready();
    h.canvas.dispatchEvent(
      mouse("mousedown", { button: 0, clientX: 202, clientY: 202, buttons: 1 }),
    );
    window.dispatchEvent(mouse("mouseup", { clientX: 202, clientY: 202 }));
    expect(h.opened.map((d) => d.id)).toEqual([1]);
  });

  it("counts a press that moved no more than three pixels as a click, and four as a drag", () => {
    const h = ready();
    h.canvas.dispatchEvent(
      mouse("mousedown", { button: 0, clientX: 200, clientY: 200, buttons: 1 }),
    );
    window.dispatchEvent(mouse("mousemove", { clientX: 203, clientY: 200, buttons: 1 }));
    window.dispatchEvent(mouse("mouseup", { clientX: 203, clientY: 200 }));
    expect(h.opened.map((d) => d.id)).toEqual([1]);
    h.canvas.dispatchEvent(
      mouse("mousedown", { button: 0, clientX: 200, clientY: 200, buttons: 1 }),
    );
    window.dispatchEvent(mouse("mousemove", { clientX: 204, clientY: 200, buttons: 1 }));
    window.dispatchEvent(mouse("mouseup", { clientX: 204, clientY: 200 }));
    expect(h.opened.map((d) => d.id)).toEqual([1]);
  });

  it("opens nothing for a click on empty space", () => {
    const h = ready();
    h.canvas.dispatchEvent(
      mouse("mousedown", { button: 0, clientX: 700, clientY: 450, buttons: 1 }),
    );
    window.dispatchEvent(mouse("mouseup", { clientX: 700, clientY: 450 }));
    expect(h.opened).toEqual([]);
  });

  it("reports no hover for a new layout (the old dot is gone)", () => {
    const h = ready();
    h.canvas.dispatchEvent(mouse("mousemove", { clientX: 200, clientY: 200 }));
    h.stage.setBase(layout([dot(7, 600, 100)]));
    expect(last(h.hovered)).toBeNull();
    h.canvas.dispatchEvent(mouse("mousemove", { clientX: 200, clientY: 200 }));
    expect(last(h.hovered)).toBeNull();
  });
});

describe("the screen's own changes", () => {
  it("redraws when the pixel density changes and watches the new density", () => {
    const h = make();
    h.stage.setLook(look());
    h.stage.setBase(BASE());
    rig.frames().run();
    const first = rig.densities()[0];
    expect(first.query).toBe("(resolution: 1dppx)");
    rig.density(2);
    rig.canvas().clear();
    first.fire();
    rig.frames().run();
    expect(h.canvas.width).toBe(1600);
    // Re-armed at the new density; the old query is no longer listened to.
    expect(last(rig.densities())?.query).toBe("(resolution: 2dppx)");
    expect(first.listeners.size).toBe(0);
    expect(last(rig.densities())?.listeners.size).toBe(1);
  });

  it("observes its box for a size change", () => {
    const h = make();
    expect(rig.observers()[0].observed).toEqual([h.wrap]);
  });
});

describe("a hand that arrives before a new layout's first frame", () => {
  // A new layout is drawn on the next animation frame; the wheel, the pointer and a press are not
  // held back for it, and must find the layout fitted, not a camera that does not exist yet.
  function fresh() {
    const h = make();
    h.stage.setLook(look());
    h.stage.setBase(BASE());
    expect(rig.framesPending()).toBe(1);
    return h;
  }

  it("zooms from the fit", () => {
    const h = fresh();
    const event = new WheelEvent("wheel", {
      deltaY: -200,
      clientX: 200,
      clientY: 200,
      cancelable: true,
    });
    h.wrap.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    rig.frames().run();
    expect(firstDot()[2]).toBeCloseTo(10 * Math.exp(0.3));
  });

  it("pans from the fit", () => {
    const h = fresh();
    h.wrap.dispatchEvent(new WheelEvent("wheel", { deltaX: 30, deltaY: 0, cancelable: true }));
    rig.frames().run();
    expect(firstDot()).toEqual([170, 200, 10]);
  });

  it("finds the dot under the pointer", () => {
    const h = fresh();
    h.canvas.dispatchEvent(mouse("mousemove", { clientX: 200, clientY: 200 }));
    expect(h.hovered.map((d) => d?.id)).toEqual([1]);
    h.canvas.dispatchEvent(
      mouse("mousedown", { button: 0, clientX: 200, clientY: 200, buttons: 1 }),
    );
    window.dispatchEvent(mouse("mouseup", { clientX: 200, clientY: 200 }));
    expect(h.opened.map((d) => d.id)).toEqual([1]);
  });

  it("drags from the fit", () => {
    const h = fresh();
    h.canvas.dispatchEvent(mouse("mousedown", { button: 0, clientX: 0, clientY: 0, buttons: 1 }));
    window.dispatchEvent(mouse("mousemove", { clientX: 40, clientY: 10, buttons: 1 }));
    rig.frames().run();
    expect(firstDot()).toEqual([240, 210, 10]);
  });

  it("zooms about the middle from a button", () => {
    const h = fresh();
    h.stage.zoom(2);
    rig.frames().run();
    expect(firstDot()).toEqual([0, 150, 20]);
  });
});

describe("destroy", () => {
  it("stops listening, cancels the frame it asked for and lets go of what it watches", () => {
    const h = make();
    h.stage.setLook(look());
    h.stage.setBase(BASE());
    expect(rig.framesPending()).toBe(1);
    h.stage.destroy();
    expect(rig.framesPending()).toBe(0);
    expect(rig.observers()[0].disconnected).toBe(true);
    expect(last(rig.densities())?.listeners.size).toBe(0);
    rig.frames().run();
    expect(rig.canvas().of("clearRect")).toHaveLength(0);
    h.canvas.dispatchEvent(mouse("mousemove", { clientX: 200, clientY: 200 }));
    h.canvas.dispatchEvent(
      mouse("mousedown", { button: 0, clientX: 200, clientY: 200, buttons: 1 }),
    );
    window.dispatchEvent(mouse("mouseup", { clientX: 200, clientY: 200 }));
    const wheel = new WheelEvent("wheel", { deltaY: -100, cancelable: true });
    h.wrap.dispatchEvent(wheel);
    expect(wheel.defaultPrevented).toBe(false);
    expect(h.hovered).toEqual([]);
    expect(h.opened).toEqual([]);
    // Nothing it was asked to do afterwards draws or asks for a frame.
    h.stage.redraw();
    h.stage.zoom(2);
    expect(rig.framesPending()).toBe(0);
  });

  it("lets a drag that was under way go: nothing is opened or moved by what follows", () => {
    const h = make();
    h.stage.setLook(look());
    h.stage.setBase(BASE());
    rig.frames().run();
    h.canvas.dispatchEvent(mouse("mousemove", { clientX: 200, clientY: 200 }));
    h.canvas.dispatchEvent(
      mouse("mousedown", { button: 0, clientX: 200, clientY: 200, buttons: 1 }),
    );
    h.stage.destroy();
    const heard = h.hovered.length;
    window.dispatchEvent(mouse("mousemove", { clientX: 260, clientY: 200, buttons: 1 }));
    window.dispatchEvent(mouse("mouseup", { clientX: 200, clientY: 200 }));
    expect(h.opened).toEqual([]);
    expect(h.hovered).toHaveLength(heard);
  });

  it("is safe to call twice", () => {
    const h = make();
    h.stage.destroy();
    expect(() => h.stage.destroy()).not.toThrow();
  });
});
