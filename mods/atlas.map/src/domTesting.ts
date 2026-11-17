// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the Map's page tests need that jsdom does not give: layout (an element has no size), a canvas
// (no 2-D context), animation frames that a test steps by hand, and a resize observer and a density
// media query a test can fire. Only tests import it, so no bundle holds it. The window it patches is
// jsdom's own; `restore` puts everything back.

import { vi } from "vitest";
import { recordingContext, type Recorder } from "./canvasTesting";

/** The size every element with the class `stage` has (the canvas's box). */
export const STAGE_SIZE = { width: 800, height: 500 };

export interface Frames {
  /** Run every frame that was asked for (and any they ask for in turn, once each). */
  run(): void;
  /** Frames waiting. */
  readonly pending: number;
}

export interface FakeObserver {
  callback: () => void;
  observed: Element[];
  disconnected: boolean;
}

export interface FakeDensity {
  query: string;
  listeners: Set<() => void>;
  fire(): void;
}

export interface DomRig {
  win: Window & typeof globalThis;
  frames: Frames;
  canvas: Recorder;
  observers: FakeObserver[];
  densities: FakeDensity[];
  /** Give an element a size (the box of any `stage` is STAGE_SIZE already). */
  setSize(el: Element, width: number, height: number): void;
  restore(): void;
}

const sizes = new WeakMap<Element, { width: number; height: number }>();

export function installDom(): DomRig {
  const win = window as Window & typeof globalThis;
  const canvas = recordingContext();
  const spy = vi
    .spyOn(HTMLCanvasElement.prototype, "getContext")
    .mockReturnValue(canvas.ctx as unknown as CanvasRenderingContext2D);

  const sizeOf = (el: Element) =>
    sizes.get(el) ?? (el.classList.contains("stage") ? STAGE_SIZE : { width: 0, height: 0 });
  Object.defineProperty(HTMLElement.prototype, "clientWidth", {
    configurable: true,
    get(this: HTMLElement) {
      return sizeOf(this).width;
    },
  });
  Object.defineProperty(HTMLElement.prototype, "clientHeight", {
    configurable: true,
    get(this: HTMLElement) {
      return sizeOf(this).height;
    },
  });

  const queue = new Map<number, () => void>();
  let nextId = 1;
  const original = {
    raf: win.requestAnimationFrame,
    caf: win.cancelAnimationFrame,
    ro: (win as unknown as { ResizeObserver?: unknown }).ResizeObserver,
    mm: win.matchMedia,
    dpr: Object.getOwnPropertyDescriptor(win, "devicePixelRatio"),
  };
  win.requestAnimationFrame = (cb: FrameRequestCallback) => {
    const id = nextId++;
    queue.set(id, () => cb(0));
    return id;
  };
  win.cancelAnimationFrame = (id: number) => {
    queue.delete(id);
  };
  const frames: Frames = {
    run() {
      const batch = Array.from(queue.entries());
      queue.clear();
      for (const [, run] of batch) run();
    },
    get pending() {
      return queue.size;
    },
  };

  const observers: FakeObserver[] = [];
  (win as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
    private readonly entry: FakeObserver;
    constructor(callback: () => void) {
      this.entry = { callback, observed: [], disconnected: false };
      observers.push(this.entry);
    }
    observe(el: Element) {
      this.entry.observed.push(el);
    }
    disconnect() {
      this.entry.disconnected = true;
    }
  };

  const densities: FakeDensity[] = [];
  win.matchMedia = ((query: string) => {
    const listeners = new Set<() => void>();
    const entry: FakeDensity = {
      query,
      listeners,
      fire: () => {
        for (const l of Array.from(listeners)) l();
      },
    };
    densities.push(entry);
    return {
      matches: false,
      media: query,
      addEventListener: (_type: string, l: () => void) => listeners.add(l),
      removeEventListener: (_type: string, l: () => void) => listeners.delete(l),
    } as unknown as MediaQueryList;
  }) as typeof win.matchMedia;

  return {
    win,
    frames,
    canvas,
    observers,
    densities,
    setSize: (el, width, height) => sizes.set(el, { width, height }),
    restore() {
      spy.mockRestore();
      delete (HTMLElement.prototype as unknown as Record<string, unknown>).clientWidth;
      delete (HTMLElement.prototype as unknown as Record<string, unknown>).clientHeight;
      win.requestAnimationFrame = original.raf;
      win.cancelAnimationFrame = original.caf;
      (win as unknown as { ResizeObserver: unknown }).ResizeObserver = original.ro;
      win.matchMedia = original.mm;
      if (original.dpr) Object.defineProperty(win, "devicePixelRatio", original.dpr);
      else delete (win as unknown as Record<string, unknown>).devicePixelRatio;
    },
  };
}

/** Set the page's pixel density. */
export function setDensity(win: Window, ratio: number): void {
  Object.defineProperty(win, "devicePixelRatio", { configurable: true, value: ratio });
}

/**
 * A rig whose lifetime a test file manages with `beforeEach(rig.install)` and `afterEach(rig.uninstall)`,
 * so each test starts with a fresh recording context, no frames waiting and no observers.
 */
export function domTesting() {
  let rig: DomRig | null = null;
  const get = (): DomRig => {
    if (!rig) throw new Error("domTesting: install() has not run");
    return rig;
  };
  return {
    install() {
      rig = installDom();
    },
    uninstall() {
      rig?.restore();
      rig = null;
    },
    canvas: () => get().canvas,
    frames: () => get().frames,
    framesPending: () => get().frames.pending,
    observers: () => get().observers,
    densities: () => get().densities,
    size: (el: Element, width: number, height: number) => get().setSize(el, width, height),
    density: (ratio: number) => setDensity(get().win, ratio),
  };
}
