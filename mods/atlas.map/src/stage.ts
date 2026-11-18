// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The canvas and everything a hand does to it: sizing for the screen's pixel density, the fit, zoom
// (wheel, buttons), pan (drag, sideways wheel), the hover, and the click that opens a document. The page
// (app.ts) gives it a layout and a look and hears of two things: which document is under the pointer,
// and which one was clicked. Behaviour is the core Map's (GraphView.tsx): the same gestures and numbers.
//
// Two differences, both because this runs in a frame. A drag that leaves the frame never delivers its
// mouse-up here, so the next mouse move that reports no button down (and the window losing focus) ends
// the drag instead of leaving the map stuck to the pointer; and drawing is coalesced into one frame per
// animation frame however many events arrive.

import { drawScene } from "./draw";
import {
  hitTester,
  fitTransform,
  isSidewaysWheel,
  panBy,
  wheelFactor,
  zoomAbout,
  CLICK_SLOP,
} from "./view";
import type { HitTester, Transform } from "./view";
import type { BaseLayout, PositionedNode } from "./layoutModel";
import type { CanvasColors } from "./theme";
import type { MapDoc } from "./types";

/** What the canvas is drawn in: the theme's colours, the project palette, the names switch. */
export interface Look {
  colors: CanvasColors;
  colorFor: (project: string) => string;
  showLabels: boolean;
}

export interface StageOptions {
  /** The box the canvas fills; its size is the canvas's. */
  wrap: HTMLElement;
  canvas: HTMLCanvasElement;
  win: Window & typeof globalThis;
  /** The document under the pointer changed (null: none). */
  onHover(doc: MapDoc | null): void;
  /** A document was clicked (a press that did not move). */
  onOpen(doc: MapDoc): void;
}

export interface Stage {
  /** A new layout: fit to it. Null: nothing to draw. */
  setBase(base: BaseLayout | null): void;
  /** A new look: redraw, keeping the view. */
  setLook(look: Look): void;
  /** Zoom about the middle of the canvas (the buttons). */
  zoom(factor: number): void;
  /** Fit the whole map into view again. */
  fit(): void;
  /** Draw now if a frame is not already coming (a resize, a changed look). */
  redraw(): void;
  destroy(): void;
}

interface Drag {
  startX: number;
  startY: number;
  lastX: number;
  lastY: number;
  moved: boolean;
}

export function createStage(options: StageOptions): Stage {
  const { wrap, canvas, win } = options;
  let base: BaseLayout | null = null;
  let look: Look | null = null;
  let tester: HitTester | null = null;
  let hovered: PositionedNode | null = null;
  let view: Transform | null = null;
  // The most recent fit, for clamping the zoom and for resetting.
  let fitView: Transform | null = null;
  let needFit = true;
  // The person has panned or zoomed: a resize keeps their framing instead of fitting again.
  let interacted = false;
  let drag: Drag | null = null;
  let frame: number | null = null;
  let dead = false;

  /** The camera, fitted first if the layout has not been fitted since it changed (or the person asked
   *  for the fit again). Taken by everything that reads the camera, not only by drawing: a wheel or a
   *  press that lands between a new layout and its first frame must find a camera, not be dropped. Null
   *  with no layout; the old camera (if any) while the box has no size to fit to. */
  const currentView = (): Transform | null => {
    if (!base) return null;
    if (needFit || !view) {
      const width = wrap.clientWidth;
      const height = wrap.clientHeight;
      if (width === 0 || height === 0) return view;
      fitView = fitTransform(base.bounds, width, height);
      view = fitView;
      needFit = false;
    }
    return view;
  };

  const draw = () => {
    frame = null;
    if (dead || !base || !look) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const dpr = win.devicePixelRatio || 1;
    const width = wrap.clientWidth;
    const height = wrap.clientHeight;
    if (width === 0 || height === 0) return;
    if (canvas.width !== Math.round(width * dpr) || canvas.height !== Math.round(height * dpr)) {
      canvas.width = Math.round(width * dpr);
      canvas.height = Math.round(height * dpr);
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, width, height);
    const current = currentView();
    if (!current) return;
    drawScene(ctx, {
      base,
      view: current,
      width,
      height,
      colors: look.colors,
      colorFor: look.colorFor,
      hovered,
      showLabels: look.showLabels,
    });
  };

  const schedule = () => {
    if (dead || frame !== null) return;
    frame = win.requestAnimationFrame(draw);
  };

  const setHovered = (next: PositionedNode | null) => {
    if ((next?.doc?.id ?? null) === (hovered?.doc?.id ?? null)) return;
    hovered = next;
    canvas.classList.toggle("is-hover", next !== null && drag === null);
    options.onHover(next?.doc ?? null);
    schedule();
  };

  const moved = () => {
    interacted = true;
    setHovered(null);
    schedule();
  };

  const nodeAt = (clientX: number, clientY: number): PositionedNode | null => {
    const current = currentView();
    if (!current || !tester) return null;
    const rect = canvas.getBoundingClientRect();
    return tester.at(clientX - rect.left, clientY - rect.top, current);
  };

  const zoomAt = (px: number, py: number, factor: number) => {
    const current = currentView();
    if (!current) return;
    view = zoomAbout(current, fitView?.scale ?? current.scale, px, py, factor);
    moved();
  };

  const endDrag = () => {
    drag = null;
    canvas.classList.remove("is-grabbing");
  };

  const onWheel = (e: WheelEvent) => {
    const current = currentView();
    if (!current) return;
    e.preventDefault();
    // A mostly sideways wheel pans left and right; an up-and-down one zooms toward the pointer.
    if (isSidewaysWheel(e.deltaX, e.deltaY)) {
      view = panBy(current, -e.deltaX, 0);
      moved();
      return;
    }
    const rect = canvas.getBoundingClientRect();
    zoomAt(e.clientX - rect.left, e.clientY - rect.top, wheelFactor(e.deltaY));
  };

  const onMouseDown = (e: MouseEvent) => {
    if (e.button !== 0) return;
    drag = {
      startX: e.clientX,
      startY: e.clientY,
      lastX: e.clientX,
      lastY: e.clientY,
      moved: false,
    };
    canvas.classList.add("is-grabbing");
    canvas.classList.remove("is-hover");
  };

  // Pan with a window-level drag, so it keeps tracking past the canvas's edge.
  const onWindowMove = (e: MouseEvent) => {
    const current = currentView();
    if (!drag || !current) return;
    // No button held: the mouse-up happened outside this frame, where it is never heard.
    if (e.buttons === 0) {
      endDrag();
      return;
    }
    const dx = e.clientX - drag.lastX;
    const dy = e.clientY - drag.lastY;
    drag.lastX = e.clientX;
    drag.lastY = e.clientY;
    if (Math.hypot(e.clientX - drag.startX, e.clientY - drag.startY) > CLICK_SLOP)
      drag.moved = true;
    view = panBy(current, dx, dy);
    moved();
  };

  const onWindowUp = (e: MouseEvent) => {
    const finished = drag;
    if (!finished) return;
    endDrag();
    if (!finished.moved) {
      const hit = nodeAt(e.clientX, e.clientY);
      if (hit?.doc) options.onOpen(hit.doc);
    }
  };

  const onCanvasMove = (e: MouseEvent) => {
    if (drag) return; // panning: no hover
    setHovered(nodeAt(e.clientX, e.clientY));
  };

  const onLeave = () => setHovered(null);

  const onDoubleClick = () => stage.fit();

  wrap.addEventListener("wheel", onWheel, { passive: false });
  canvas.addEventListener("mousedown", onMouseDown);
  canvas.addEventListener("mousemove", onCanvasMove);
  canvas.addEventListener("mouseleave", onLeave);
  canvas.addEventListener("dblclick", onDoubleClick);
  win.addEventListener("mousemove", onWindowMove);
  win.addEventListener("mouseup", onWindowUp);
  win.addEventListener("blur", endDrag);

  // A resize keeps the person's framing if they have navigated, else fits to the new size.
  const observer =
    typeof win.ResizeObserver === "function"
      ? new win.ResizeObserver(() => {
          if (!interacted) needFit = true;
          schedule();
        })
      : null;
  observer?.observe(wrap);

  // The pixel density can change with no change of size (the window dragged to another screen, an OS
  // zoom step), and then the observer is silent: a media query on the current density fires exactly on
  // that change. It is re-armed at the new density each time.
  let density: MediaQueryList | null = null;
  const onDensity = () => {
    schedule();
    watchDensity();
  };
  const watchDensity = () => {
    density?.removeEventListener("change", onDensity);
    density =
      typeof win.matchMedia === "function"
        ? win.matchMedia(`(resolution: ${win.devicePixelRatio || 1}dppx)`)
        : null;
    density?.addEventListener("change", onDensity);
  };
  watchDensity();

  const stage: Stage = {
    setBase(next) {
      base = next;
      tester = next ? hitTester(next.nodes) : null;
      // The dot that was under the pointer belongs to the old layout.
      setHovered(null);
      // A new layout refits; a pan and zoom only apply within one layout.
      needFit = true;
      interacted = false;
      view = null;
      schedule();
    },
    setLook(next) {
      look = next;
      schedule();
    },
    zoom(factor) {
      zoomAt(wrap.clientWidth / 2, wrap.clientHeight / 2, factor);
    },
    fit() {
      needFit = true;
      interacted = false;
      schedule();
    },
    redraw: schedule,
    destroy() {
      dead = true;
      if (frame !== null) win.cancelAnimationFrame(frame);
      frame = null;
      wrap.removeEventListener("wheel", onWheel);
      canvas.removeEventListener("mousedown", onMouseDown);
      canvas.removeEventListener("mousemove", onCanvasMove);
      canvas.removeEventListener("mouseleave", onLeave);
      canvas.removeEventListener("dblclick", onDoubleClick);
      win.removeEventListener("mousemove", onWindowMove);
      win.removeEventListener("mouseup", onWindowUp);
      win.removeEventListener("blur", endDrag);
      observer?.disconnect();
      density?.removeEventListener("change", onDensity);
    },
  };
  return stage;
}
