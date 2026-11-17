// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Map's page, whole: the real controller, shell and stage over PM's side of the calls in memory
// (memoryHost, written from the Rust handlers and holding PM's limits), in jsdom with a recording canvas.
// What is asserted is what a person sees (the strips, the card, the legend, the canvas's drawing) and
// what PM is told (the calls, the saved settings), never the controller's internals.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mountMap, type MapApp } from "./app";
import { inlineCompute, type Compute } from "./compute";
import { domTesting } from "./domTesting";
import { type MapHost } from "./hostApi";
import { memoryHost, type MemoryHost, type MemoryHostOptions } from "./memoryHost";
import { doc, vectorsFor } from "./testing";
import type { MapDoc } from "./types";
import { QUIET_MS } from "./changes";
import tableRs from "../../../src-tauri/src/mods/api/table.rs?raw";
import manifest from "../manifest.json";

const rig = domTesting();
beforeEach(() => rig.install());

const apps: MapApp[] = [];
afterEach(() => {
  for (const app of apps.splice(0)) app.destroy();
  rig.uninstall();
  document.body.replaceChildren();
  document.documentElement.removeAttribute("style");
  delete document.documentElement.dataset.mode;
  delete document.documentElement.dataset.depth;
  vi.useRealTimers();
});

const NAMES = ["Atlas", "Beta", "Gamma"];
/** Documents in three projects, in the order their colours are given out. The Map lists newest first
 *  (hostApi.ts byNewest), so each document is ingested a minute before the one with the id below it:
 *  the list then opens with document 1 and Atlas, Beta, Gamma keep their colours. */
const library = (n = 12): MapDoc[] =>
  Array.from({ length: n }, (_, i) => ({
    ...doc(i + 1, NAMES[i % 3], 2 + (i % 5), `Document ${i + 1}`),
    ingestedAt: new Date(Date.UTC(2026, 0, 1) - i * 60_000).toISOString(),
  }));

const tick = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

interface Gate {
  host: MapHost;
  /** Calls held at the gate so far. */
  readonly held: number;
  open(): void;
}

/** A host that holds every call of `method` until `open()`, then lets them through. */
function gated(pm: MemoryHost, method: string): Gate {
  let held = 0;
  let open!: () => void;
  const released = new Promise<void>((resolve) => {
    open = resolve;
  });
  const host: MapHost = {
    ...pm.host,
    async call(m, args) {
      if (m === method) {
        held += 1;
        await released;
      }
      return pm.host.call(m, args);
    },
    async documentVectors(request) {
      if (method === "embeddings.documentVectors") {
        held += 1;
        await released;
      }
      return pm.host.documentVectors(request);
    },
  };
  return {
    host,
    get held() {
      return held;
    },
    open,
  };
}

interface Opened {
  pm: MemoryHost;
  app: MapApp;
  mount: HTMLElement;
  compute: Compute;
  /** The one element matching, or fail. */
  el<T extends Element = HTMLElement>(selector: string): T;
  text(selector: string): string;
  hidden(selector: string): boolean;
}

interface OpenOptions extends Partial<MemoryHostOptions> {
  docCount?: number;
  embed?: boolean;
  /** What is stored under `settings` already. */
  settings?: unknown;
  /** Wrap the host (a gate). */
  wrap?: (pm: MemoryHost) => MapHost;
  compute?: Compute;
  /** Set up PM's side (a failure) before the page starts: it starts asking at once. */
  arm?: (pm: MemoryHost) => void;
  /** Do not wait for the first load to finish. */
  noWait?: boolean;
}

async function open(options: OpenOptions = {}): Promise<Opened> {
  const { docCount = 12, embed = true, settings, wrap, noWait } = options;
  const docs = options.docs ?? library(docCount);
  const pm = memoryHost({
    docs,
    vectors: embed ? (options.vectors ?? vectorsFor(docs, 8)) : new Map(),
    dimension: 8,
    contentKeys: ["settings"],
    ...(options.theme !== undefined ? { theme: options.theme } : {}),
  });
  if (settings !== undefined) pm.kv.set("settings", { value: settings, revision: 1 });
  options.arm?.(pm);
  const mount = document.createElement("div");
  document.body.append(mount);
  const compute = options.compute ?? inlineCompute();
  const app = mountMap({
    doc: document,
    win: window as Window & typeof globalThis,
    mount,
    host: wrap ? wrap(pm) : pm.host,
    compute,
    themeRoot: document.documentElement,
  });
  apps.push(app);
  if (!noWait) await app.ready;
  const el = <T extends Element = HTMLElement>(selector: string): T => {
    const found = mount.querySelector<T>(selector);
    if (!found) throw new Error(`no element ${selector}`);
    return found;
  };
  return {
    pm,
    app,
    mount,
    compute,
    el,
    text: (selector) => el(selector).textContent ?? "",
    hidden: (selector) => el(selector).hidden === true,
  };
}

/** Everything the canvas was asked in the last frame drawn (from its last clearRect on). */
function lastFrame() {
  const calls = rig.canvas().calls;
  let start = 0;
  for (let i = calls.length - 1; i >= 0; i--) {
    if (calls[i].op === "clearRect") {
      start = i;
      break;
    }
  }
  return calls.slice(start);
}

/** The circles of the last frame drawn: [x, y, r, ...]. */
const lastFrameArcs = (): number[][] =>
  lastFrame()
    .filter((c) => c.op === "arc")
    .map((c) => c.args as number[]);

const draw = () => {
  rig.frames().run();
  return lastFrameArcs();
};

const click = (el: Element) => el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
const mouse = (type: string, init: MouseEventInit = {}) =>
  new MouseEvent(type, { bubbles: true, cancelable: true, ...init });

const legendNames = (o: Opened) =>
  Array.from(o.mount.querySelectorAll(".legend li")).map((li) => li.textContent);
const legendColors = (o: Opened) =>
  Array.from(o.mount.querySelectorAll(".legend circle")).map((c) => c.getAttribute("fill"));
const stored = (o: Opened) => o.pm.kv.get("settings")?.value as Record<string, unknown>;
const callsOf = (o: Opened, method: string) => o.pm.of(method).length;

const DARK = ["#60a5fa", "#34d399", "#f472b6"];

describe("starting", () => {
  it("asks for the theme, then the settings, then the documents, and draws the map by project", async () => {
    const o = await open();
    const order = o.pm.calls.map((c) => c.method);
    expect(order.indexOf("theme.get")).toBe(0);
    expect(order.indexOf("storage.get")).toBe(1);
    expect(order.indexOf("documents.list")).toBeGreaterThan(1);
    // Twelve documents in three projects: twelve dots and three hubs, and edges for the hubs.
    const arcs = draw();
    expect(arcs).toHaveLength(15);
    expect(o.hidden(".canvas")).toBe(false);
    expect(o.hidden(".nav")).toBe(false);
    expect(o.hidden(".message")).toBe(true);
    expect(o.text(".subtitle")).toBe(
      "12 documents across 3 projects · scroll to zoom, drag to pan, click a document to read it",
    );
    expect(o.el(".canvas").getAttribute("aria-label")).toContain("12 documents across 3 projects");
  });

  it("lists the projects in the legend in the order their colours were given out", async () => {
    const o = await open();
    expect(legendNames(o)).toEqual(["Atlas", "Beta", "Gamma"]);
    expect(legendColors(o)).toEqual(DARK);
  });

  it("hands the colours out as the core Map does: by the newest documents' projects first", async () => {
    // Every document ingested at the same moment: newest first is the highest id, a Gamma one here.
    const docs = Array.from({ length: 12 }, (_, i) => doc(i + 1, NAMES[i % 3]));
    const o = await open({ docs });
    expect(legendNames(o)).toEqual(["Gamma", "Beta", "Atlas"]);
  });

  it("puts PM's theme on the page and the canvas", async () => {
    const o = await open({
      theme: {
        vars: { "--bg": "oklch(0.2 0 0)", "--ink": "oklch(0.9 0 0)", "--border": "BORDER" },
        mode: "light",
        colorblind: false,
        depth: "standard",
      },
    });
    const root = document.documentElement;
    expect(root.style.getPropertyValue("--bg")).toBe("oklch(0.2 0 0)");
    expect(root.dataset.mode).toBe("light");
    expect(root.style.colorScheme).toBe("light");
    // The light palette, and the canvas drawing the edges in the snapshot's own border colour.
    expect(legendColors(o)).toEqual(["#2563eb", "#059669", "#db2777"]);
    draw();
    expect(rig.canvas().of("stroke")[0].state.strokeStyle).toBe("BORDER");
    expect(o.pm.of("theme.get")).toHaveLength(1);
  });

  it("shows the shimmer and no subtitle until the documents are in, then the map", async () => {
    let gate!: Gate;
    const o = await open({
      noWait: true,
      wrap: (pm) => (gate = gated(pm, "documents.list")).host,
    });
    await tick();
    expect(gate.held).toBe(1);
    expect(o.hidden(".message")).toBe(false);
    expect(o.hidden(".skeleton")).toBe(false);
    expect(o.hidden(".canvas")).toBe(true);
    expect(o.text(".subtitle")).toBe("");
    gate.open();
    await o.app.ready;
    expect(o.hidden(".message")).toBe(true);
    expect(o.hidden(".canvas")).toBe(false);
    expect(o.text(".subtitle")).toContain("12 documents");
  });

  it("says so, and draws no canvas, for a library with no documents", async () => {
    const o = await open({ docs: [] });
    expect(o.text(".message")).toBe(
      "No documents yet. Ingest some in the Documents view and they'll appear here.",
    );
    expect(o.hidden(".canvas")).toBe(true);
    expect(o.hidden(".skeleton")).toBe(true);
    expect(o.hidden(".strip-error")).toBe(true);
    expect(callsOf(o, "embeddings.signature")).toBe(0);
  });

  it("wears the stored settings: arrangement, cohesion, names and the cap", async () => {
    const o = await open({
      settings: { layoutMode: "semantic", cohesion: 0.3, labels: true, nodeCap: 300 },
    });
    expect(o.el("button.seg-button.active").textContent).toBe("Semantic");
    expect(o.el<HTMLSelectElement>("#map-cohesion").value).toBe("0.3");
    expect(o.hidden(".field")).toBe(false);
    expect(o.el("button.chip").getAttribute("aria-pressed")).toBe("true");
    expect(o.el<HTMLInputElement>("#map-node-cap").value).toBe("300");
    // By meaning: a dot per document, no hubs.
    expect(draw()).toHaveLength(12);
    expect(o.pm.of("embeddings.documentVectors")[0].args).toEqual({ cap: 300 });
  });

  it("says it is showing the top of the documents when the cap leaves some out", async () => {
    const o = await open({
      docCount: 250,
      settings: { layoutMode: "semantic", nodeCap: 200 },
    });
    expect(o.text(".subtitle")).toBe(
      "showing the top 200 of 250 documents by meaning · scroll to zoom, drag to pan",
    );
    // The 50 left out are still on the map, gathered and faded.
    draw();
    const fills = lastFrame().filter((c) => c.op === "fill");
    expect(fills).toHaveLength(250);
    expect(fills.filter((c) => c.state.globalAlpha === 0.32)).toHaveLength(50);
  });

  it("starts with the defaults when PM has no settings for it yet", async () => {
    const o = await open();
    expect(o.el("button.seg-button.active").textContent).toBe("By project");
    expect(o.hidden(".field")).toBe(true);
    expect(o.el<HTMLInputElement>("#map-node-cap").value).toBe("1000");
    expect(o.pm.kv.has("settings")).toBe(false);
  });

  it("reads a settings value that has only a node cap (the install's handover)", async () => {
    const o = await open({ settings: { nodeCap: 2500 } });
    expect(o.el("button.seg-button.active").textContent).toBe("By project");
    expect(o.el<HTMLInputElement>("#map-node-cap").value).toBe("2500");
  });

  it("goes on with the standard settings, and says so, when PM cannot read them", async () => {
    const o = await open({ arm: (pm) => pm.failNext("storage.get", "MOD-9001") });
    expect(o.text(".strip-error .strip-text")).toBe(
      "Your map settings couldn't be read, so the standard ones are showing.",
    );
    expect(o.el(".strip-error .link").textContent).toBe("Dismiss");
    expect(draw().length).toBeGreaterThan(0);
  });

  it("goes on without a theme when PM does not answer one, or answers nonsense", async () => {
    const o = await open({ theme: "not a theme" });
    expect(document.documentElement.style.getPropertyValue("--bg")).toBe("");
    expect(draw().length).toBeGreaterThan(0);
    expect(legendColors(o)).toEqual(DARK);

    for (const app of apps.splice(0)) app.destroy();
    document.body.replaceChildren();
    const failing = await open({ arm: (pm) => pm.failAlways("theme.get", "MOD-9001") });
    expect(draw().length).toBeGreaterThan(0);
    expect(failing.hidden(".strip-error")).toBe(true);
  });
});

describe("the two arrangements", () => {
  it("works the by-meaning layout out on first use, says so while it does, and saves the choice", async () => {
    let gate!: Gate;
    const o = await open({ wrap: (pm) => (gate = gated(pm, "embeddings.documentVectors")).host });
    click(o.el("button.seg-button:last-child"));
    await tick();
    expect(gate.held).toBe(1);
    expect(o.hidden(".strip[role=status]")).toBe(false);
    expect(o.text(".strip[role=status] .strip-text")).toBe(
      "Reading what your documents are about…",
    );
    expect(o.text(".message span:last-child")).toBe("Preparing the map…");
    expect(o.hidden(".canvas")).toBe(true);
    gate.open();
    await tick();
    await tick();
    expect(o.hidden(".canvas")).toBe(false);
    expect(o.hidden(".strip[role=status]")).toBe(true);
    expect(draw()).toHaveLength(12);
    expect(o.text(".subtitle")).toBe("12 documents by meaning · scroll to zoom, drag to pan");
    expect(stored(o)).toMatchObject({ layoutMode: "semantic" });
  });

  it("does not flash a strip for a map it already has saved", async () => {
    const first = await open({ settings: { layoutMode: "semantic" } });
    const kv = new Map(first.pm.kv);
    const rows = new Map(first.pm.rows);
    for (const app of apps.splice(0)) app.destroy();
    document.body.replaceChildren();

    const docs = library(12);
    const pm = memoryHost({
      docs,
      vectors: vectorsFor(docs, 8),
      dimension: 8,
      contentKeys: ["settings"],
    });
    for (const [k, v] of kv) pm.kv.set(k, v);
    for (const [k, v] of rows) pm.rows.set(k, v);
    const seen: (string | null)[] = [];
    const mount = document.createElement("div");
    document.body.append(mount);
    const watcher = new MutationObserver(() => {
      const strip = mount.querySelector<HTMLElement>(".strip[role=status]");
      seen.push(strip && !strip.hidden ? (strip.textContent ?? "") : null);
    });
    watcher.observe(mount, {
      subtree: true,
      attributes: true,
      childList: true,
      characterData: true,
    });
    const app = mountMap({
      doc: document,
      win: window as Window & typeof globalThis,
      mount,
      host: pm.host,
      compute: inlineCompute(),
      themeRoot: document.documentElement,
    });
    apps.push(app);
    await app.ready;
    await tick();
    expect(seen.filter((s) => s !== null)).toEqual([]);
    // A saved layout: the signature, the stored fingerprint, the rows. No vector was read.
    expect(pm.of("embeddings.documentVectors")).toHaveLength(0);
    expect(draw()).toHaveLength(12);
    watcher.disconnect();
  });

  it("keeps what it has worked out: switching back and forth asks PM for nothing more", async () => {
    const o = await open();
    click(o.el("button.seg-button:last-child"));
    await tick();
    await tick();
    click(o.el("button.seg-button:first-child"));
    await tick();
    const before = o.pm.calls.length;
    click(o.el("button.seg-button:last-child"));
    await tick();
    click(o.el("button.seg-button:first-child"));
    await tick();
    // Saving the arrangement (a read of the settings and a write) is not the map's own work.
    const reads = o.pm.calls
      .slice(before)
      .filter((c) => !["storage.set"].includes(c.method))
      .filter(
        (c) => !(c.method === "storage.get" && (c.args as { key?: string }).key === "settings"),
      );
    expect(reads).toEqual([]);
    expect(callsOf(o, "documents.list")).toBe(1);
    expect(draw()).toHaveLength(15);
  });

  it("says nothing is there to place when no document has been read for meaning yet", async () => {
    const o = await open({ embed: false, settings: { layoutMode: "semantic" } });
    expect(o.text(".message span:last-child")).toBe(
      "Nothing to place by meaning yet. Documents appear here once PM has read them.",
    );
    expect(o.hidden(".canvas")).toBe(true);
    expect(o.hidden(".strip-error")).toBe(true);
  });

  it("drops the answer of an arrangement that was switched away from while it was working", async () => {
    let gate!: Gate;
    const o = await open({ wrap: (pm) => (gate = gated(pm, "embeddings.documentVectors")).host });
    click(o.el("button.seg-button:last-child"));
    await tick();
    expect(gate.held).toBe(1);
    click(o.el("button.seg-button:first-child"));
    await tick();
    expect(o.hidden(".canvas")).toBe(false);
    gate.open();
    await tick();
    await tick();
    // The late by-meaning answer did not replace the by-project map the person went back to.
    expect(draw()).toHaveLength(15);
    expect(o.text(".subtitle")).toContain("across 3 projects");
    expect(o.hidden(".strip[role=status]")).toBe(true);
    expect(stored(o)).toMatchObject({ layoutMode: "project" });
  });

  it("drops the answer of a by-project layout that was switched away from while the Worker was busy", async () => {
    const real = inlineCompute();
    let release!: () => void;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    const compute: Compute = {
      ...real,
      force: async (request) => {
        await held;
        return real.force(request);
      },
    };
    const o = await open({ compute, noWait: true });
    await tick();
    await tick();
    // The by-project layout is waiting on the Worker: choose by meaning instead.
    click(o.el("button.seg-button:last-child"));
    await tick();
    await tick();
    expect(draw()).toHaveLength(12);
    release();
    await o.app.ready;
    await tick();
    // The late by-project answer did not replace the by-meaning map that is showing.
    expect(draw()).toHaveLength(12);
    expect(o.text(".subtitle")).toContain("by meaning");
    // And it was not lost: going back to by project has it.
    click(o.el("button.seg-button:first-child"));
    await tick();
    await tick();
    expect(draw()).toHaveLength(15);
  });

  it("does not keep a layout that lands while the documents are being listed again", async () => {
    let releaseVectors!: () => void;
    const vectors = new Promise<void>((resolve) => {
      releaseVectors = resolve;
    });
    let releaseList!: () => void;
    const list = new Promise<void>((resolve) => {
      releaseList = resolve;
    });
    let lists = 0;
    const o = await open({
      settings: { layoutMode: "semantic" },
      noWait: true,
      wrap: (pm) => ({
        ...pm.host,
        async call(method, args) {
          if (method === "documents.list") {
            lists += 1;
            // The first listing goes through; the second (the refresh) is held.
            if (lists === 2) await list;
          }
          return pm.host.call(method, args);
        },
        async documentVectors(request) {
          // PM answers from the documents it has now (twelve), and the answer is slow to arrive.
          const answer = await pm.host.documentVectors(request);
          await vectors;
          return answer;
        },
      }),
    });
    await tick();
    await tick();
    // A document arrives, and the person refreshes while the first layout is still being worked out.
    o.pm.addDoc(doc(99, "Atlas", 3, "New one"), new Float32Array(8).fill(0.4));
    const reload = o.app.reload();
    await tick();
    // The first layout lands now: it was made for the old set of documents.
    releaseVectors();
    await tick();
    await tick();
    releaseList();
    await o.app.ready;
    await reload;
    await tick();
    draw();
    const fills = lastFrame().filter((c) => c.op === "fill");
    // Thirteen documents, every one placed: none is parked and faded (alpha 0.32).
    expect(fills).toHaveLength(13);
    expect(fills.filter((c) => c.state.globalAlpha === 0.32)).toHaveLength(0);
  });

  it("uses the arrangement chosen while the documents were still loading", async () => {
    let gate!: Gate;
    const o = await open({
      noWait: true,
      wrap: (pm) => (gate = gated(pm, "documents.list")).host,
    });
    await tick();
    click(o.el("button.seg-button:last-child"));
    await tick();
    // Nothing is computed before there are documents to compute it for.
    expect(callsOf(o, "embeddings.signature")).toBe(0);
    gate.open();
    await o.app.ready;
    expect(draw()).toHaveLength(12);
    expect(o.text(".subtitle")).toContain("by meaning");
  });

  it("says the layout failed, leaves the documents alone, and tries again on request", async () => {
    const o = await open({
      settings: { layoutMode: "semantic" },
      arm: (pm) => pm.failNext("embeddings.documentVectors", "MOD-9001"),
    });
    expect(o.text(".strip-error .strip-text")).toBe(
      "The map couldn't be laid out. Your documents are unchanged.",
    );
    expect(o.hidden(".canvas")).toBe(true);
    expect(o.text(".strip-error .link")).toBe("Try again");
    // PM's own words never reach the person.
    expect(o.mount.textContent).not.toMatch(/MOD-|Failed/);
    click(o.el(".strip-error .link"));
    await tick();
    await tick();
    expect(o.hidden(".strip-error")).toBe(true);
    expect(draw()).toHaveLength(12);
  });

  it("says the same when the Worker fails to arrange the projects", async () => {
    const real = inlineCompute();
    let broken = true;
    const compute: Compute = {
      ...real,
      mode: "worker",
      reduce: real.reduce,
      force: (request) =>
        broken ? Promise.reject(new Error("The layout worker failed.")) : real.force(request),
      close: () => real.close(),
    };
    const o = await open({ compute });
    expect(o.text(".strip-error .strip-text")).toBe(
      "The map couldn't be laid out. Your documents are unchanged.",
    );
    expect(o.hidden(".canvas")).toBe(true);
    broken = false;
    click(o.el(".strip-error .link"));
    await tick();
    await tick();
    expect(draw()).toHaveLength(15);
    expect(o.hidden(".strip-error")).toBe(true);
  });

  it("says the documents could not be loaded, and loads them again on request", async () => {
    const o = await open({ arm: (pm) => pm.failNext("documents.list", "MOD-9001") });
    expect(o.text(".strip-error .strip-text")).toBe(
      "The map couldn't be loaded. Your documents are unchanged.",
    );
    expect(o.hidden(".canvas")).toBe(true);
    expect(o.hidden(".skeleton")).toBe(true);
    click(o.el(".strip-error .link"));
    await tick();
    await tick();
    expect(o.hidden(".strip-error")).toBe(true);
    expect(draw()).toHaveLength(15);
  });

  it("keeps showing the old picture, the new document faded, while a changed set is worked out", async () => {
    // The vectors are held only once the first layout is made and saved.
    let hold = false;
    let release: () => void = () => undefined;
    const o = await open({
      settings: { layoutMode: "semantic" },
      wrap: (pm) => ({
        ...pm.host,
        async documentVectors(request) {
          if (hold) await new Promise<void>((resolve) => (release = resolve));
          return pm.host.documentVectors(request);
        },
      }),
    });
    expect(draw()).toHaveLength(12);

    hold = true;
    o.pm.addDoc(doc(99, "Atlas", 3, "New one"), new Float32Array(8).fill(0.4));
    // Refresh: the saved layout is for twelve documents, the signature says thirteen.
    void o.app.reload();
    await tick();
    await tick();
    // Under the strip, not behind a blank "Preparing the map…": every dot, the new one parked and faded.
    expect(o.text(".strip[role=status] .strip-text")).toBe(
      "Reading what your documents are about…",
    );
    expect(o.hidden(".message")).toBe(true);
    expect(o.hidden(".canvas")).toBe(false);
    draw();
    const fills = lastFrame().filter((c) => c.op === "fill");
    expect(fills).toHaveLength(13);
    expect(fills.filter((c) => c.state.globalAlpha === 0.32)).toHaveLength(1);

    release();
    await tick();
    await tick();
    await tick();
    expect(o.hidden(".strip[role=status]")).toBe(true);
    draw();
    const placed = lastFrame().filter((c) => c.op === "fill");
    expect(placed).toHaveLength(13);
    expect(placed.filter((c) => c.state.globalAlpha === 0.32)).toHaveLength(0);
  });
});

describe("an old picture that arrives after the person moved on", () => {
  it("is dropped: the old by-meaning picture does not replace the by-project map chosen meanwhile", async () => {
    let gate!: Gate;
    const o = await open({
      settings: { layoutMode: "semantic" },
      wrap: (pm) => (gate = gated(pm, "storage.doc.list")).host,
    });
    expect(draw()).toHaveLength(12);
    o.pm.addDoc(doc(99, "Atlas", 3, "New one"), new Float32Array(8).fill(0.4));
    // Refresh: reading the saved rows (for the placeholder) is held at the gate.
    void o.app.reload();
    await tick();
    await tick();
    expect(gate.held).toBe(1);
    click(o.el("button.seg-button:first-child"));
    await tick();
    await tick();
    await tick();
    // Thirteen documents and three hubs.
    expect(draw()).toHaveLength(16);
    gate.open();
    await tick();
    await tick();
    await tick();
    expect(draw()).toHaveLength(16);
  });
});

describe("the subtitle while a layout is being worked out", () => {
  it("counts the projects from the documents, not from a layout that is not there yet", async () => {
    let release: () => void = () => undefined;
    const compute = inlineCompute();
    const o = await open({
      noWait: true,
      compute: {
        ...compute,
        force: async (request) => {
          await new Promise<void>((resolve) => (release = resolve));
          return compute.force(request);
        },
      },
    });
    await tick();
    await tick();
    await tick();
    expect(o.text(".strip[role=status] .strip-text")).toBe("Arranging your projects…");
    expect(o.text(".subtitle")).toContain("12 documents across 3 projects");
    release();
    await o.app.ready;
    await tick();
    expect(o.text(".subtitle")).toContain("12 documents across 3 projects");
  });
});

describe("the controls", () => {
  it("shows the cohesion menu only for the by-meaning arrangement", async () => {
    const o = await open();
    expect(o.hidden(".field")).toBe(true);
    click(o.el("button.seg-button:last-child"));
    await tick();
    expect(o.hidden(".field")).toBe(false);
    click(o.el("button.seg-button:first-child"));
    await tick();
    expect(o.hidden(".field")).toBe(true);
  });

  it("marks the chosen arrangement on both buttons", async () => {
    const o = await open();
    const [byProject, semantic] = Array.from(o.mount.querySelectorAll("button.seg-button"));
    expect([byProject, semantic].map((b) => b.getAttribute("aria-pressed"))).toEqual([
      "true",
      "false",
    ]);
    click(semantic);
    await tick();
    expect([byProject, semantic].map((b) => b.getAttribute("aria-pressed"))).toEqual([
      "false",
      "true",
    ]);
  });

  it("blends by cohesion without working the layout out again, and keeps the choice", async () => {
    const o = await open({ settings: { layoutMode: "semantic" } });
    const before = draw().map((a) => a.slice(0, 2));
    const vectors = callsOf(o, "embeddings.documentVectors");
    const writes = callsOf(o, "storage.doc.set");
    const select = o.el<HTMLSelectElement>("#map-cohesion");
    select.value = "0.5";
    select.dispatchEvent(new Event("change", { bubbles: true }));
    await tick();
    const after = draw().map((a) => a.slice(0, 2));
    expect(after).toHaveLength(before.length);
    expect(after).not.toEqual(before);
    expect(callsOf(o, "embeddings.documentVectors")).toBe(vectors);
    expect(callsOf(o, "storage.doc.set")).toBe(writes);
    expect(stored(o)).toMatchObject({ cohesion: 0.5 });
  });

  it("shows the nearest choice for a stored cohesion the menu has no entry for", async () => {
    const o = await open({ settings: { layoutMode: "semantic", cohesion: 0.2 } });
    expect(o.el<HTMLSelectElement>("#map-cohesion").value).toBe("0.15");
    // Looking at it saves nothing.
    expect(stored(o)).toEqual({ layoutMode: "semantic", cohesion: 0.2 });
  });

  it("turns file names on and off and keeps the choice", async () => {
    const o = await open();
    const names = o.el("button.chip");
    expect(names.getAttribute("aria-pressed")).toBe("false");
    expect(names.getAttribute("title")).toBe("Show file names");
    click(names);
    await tick();
    expect(names.getAttribute("aria-pressed")).toBe("true");
    expect(names.getAttribute("title")).toBe("Hide file names");
    expect(stored(o)).toMatchObject({ labels: true });
    click(names);
    await tick();
    expect(names.getAttribute("aria-pressed")).toBe("false");
    expect(stored(o)).toMatchObject({ labels: false });
  });

  it("keeps changes in the order they were made, one save at a time", async () => {
    const o = await open();
    const names = o.el("button.chip");
    click(names);
    click(names);
    click(names);
    await tick();
    await tick();
    expect(stored(o)).toMatchObject({ labels: true });
    // The saves never overlapped: each read the revision the last one wrote, so none was refused.
    expect(o.pm.kv.get("settings")?.revision).toBe(3);
  });

  it("keeps the later of two changes when the first one's save is slow", async () => {
    let release!: () => void;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    let slow = true;
    const o = await open({
      wrap: (pm) => ({
        ...pm.host,
        async call(method, args) {
          // The first write of the settings is slow to land, as a busy PM's can be.
          if (method === "storage.set" && (args as { key: string }).key === "settings" && slow) {
            slow = false;
            await held;
          }
          return pm.host.call(method, args);
        },
      }),
    });
    const names = o.el("button.chip");
    click(names); // on
    click(names); // off again
    await tick();
    release();
    await tick();
    await tick();
    // Saved in the order made, so the last choice (off) is what is kept, not whichever write was last.
    expect(stored(o)).toMatchObject({ labels: false });
  });

  it("does not lose one setting to another saved just before it", async () => {
    const o = await open();
    click(o.el("button.chip"));
    click(o.el("button.seg-button:last-child"));
    await tick();
    await tick();
    expect(stored(o)).toMatchObject({ labels: true, layoutMode: "semantic" });
  });

  it("says a setting could not be saved, in the Map's words, and lets it be dismissed", async () => {
    const o = await open();
    o.pm.failAlways("storage.set", "MOD-9001");
    click(o.el("button.chip"));
    await tick();
    await tick();
    expect(o.text(".strip-error .strip-text")).toBe(
      "Your map settings couldn't be saved. They will be as they were next time.",
    );
    // The change still shows for now.
    expect(o.el("button.chip").getAttribute("aria-pressed")).toBe("true");
    expect(o.text(".strip-error .link")).toBe("Dismiss");
    click(o.el(".strip-error .link"));
    expect(o.hidden(".strip-error")).toBe(true);
  });

  it("zooms and fits from the buttons", async () => {
    const o = await open({ settings: { layoutMode: "semantic" } });
    const fit = draw()[0];
    click(o.el("button[aria-label='Zoom in']"));
    const zoomed = draw()[0];
    expect(zoomed[2]).toBeCloseTo(fit[2] * 1.3);
    click(o.el("button[aria-label='Zoom out']"));
    expect(draw()[0][2]).toBeCloseTo(fit[2]);
    click(o.el("button[aria-label='Zoom in']"));
    click(o.el("button[aria-label='Fit to view']"));
    expect(draw()[0]).toEqual(fit);
  });
});

describe("the settings popover", () => {
  const panel = (o: Opened) => o.el("#map-settings");
  const button = (o: Opened) => o.el("button[aria-controls='map-settings']");

  it("opens and closes from its button, and says which", async () => {
    const o = await open();
    expect(panel(o).hidden).toBe(true);
    click(button(o));
    expect(panel(o).hidden).toBe(false);
    expect(button(o).getAttribute("aria-expanded")).toBe("true");
    click(button(o));
    expect(panel(o).hidden).toBe(true);
    expect(button(o).getAttribute("aria-expanded")).toBe("false");
  });

  it("closes on Escape and gives the focus back to its button", async () => {
    const o = await open();
    document.body.append(o.mount);
    click(button(o));
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(panel(o).hidden).toBe(true);
    expect(document.activeElement).toBe(button(o));
    // Another key does nothing; Escape with it closed does nothing.
    click(button(o));
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "a", bubbles: true }));
    expect(panel(o).hidden).toBe(false);
  });

  it("closes on a press outside it and stays open for a press inside it", async () => {
    const o = await open();
    click(button(o));
    panel(o).dispatchEvent(mouse("mousedown"));
    expect(panel(o).hidden).toBe(false);
    button(o).dispatchEvent(mouse("mousedown"));
    expect(panel(o).hidden).toBe(false);
    o.el(".canvas").dispatchEvent(mouse("mousedown", { button: 0 }));
    expect(panel(o).hidden).toBe(true);
    expect(button(o).getAttribute("aria-expanded")).toBe("false");
  });

  const type = (o: Opened, value: string) => {
    const input = o.el<HTMLInputElement>("#map-node-cap");
    input.value = value;
    input.dispatchEvent(new Event("change", { bubbles: true }));
    return input;
  };

  it("keeps a new cap, and works the by-meaning layout out again when that is what is showing", async () => {
    const o = await open({ settings: { layoutMode: "semantic", nodeCap: 1000 } });
    type(o, "300");
    await tick();
    await tick();
    expect(stored(o)).toMatchObject({ nodeCap: 300 });
    const asked = o.pm.of("embeddings.documentVectors").map((c) => c.args);
    expect(asked).toEqual([{ cap: 1000 }, { cap: 300 }]);
    expect(draw()).toHaveLength(12);
  });

  it("only remembers a new cap while the by-project arrangement is showing", async () => {
    const o = await open();
    type(o, "400");
    await tick();
    expect(stored(o)).toMatchObject({ nodeCap: 400 });
    expect(callsOf(o, "embeddings.documentVectors")).toBe(0);
    // And uses it the first time the by-meaning arrangement is chosen.
    click(o.el("button.seg-button:last-child"));
    await tick();
    await tick();
    expect(o.pm.of("embeddings.documentVectors")[0].args).toEqual({ cap: 400 });
  });

  it.each([
    ["99999", "5000"],
    ["5", "200"],
    ["-40", "200"],
    ["250.4", "250"],
    ["250.5", "251"],
  ])("clamps %s to %s and shows what it kept", async (typed, kept) => {
    const o = await open();
    const input = type(o, typed);
    await tick();
    expect(input.value).toBe(kept);
    expect(stored(o)).toMatchObject({ nodeCap: Number(kept) });
  });

  it("shows the kept value in a box that still has the focus (a number committed with Enter)", async () => {
    const o = await open();
    click(button(o));
    const input = o.el<HTMLInputElement>("#map-node-cap");
    input.focus();
    expect(document.activeElement).toBe(input);
    type(o, "99999");
    await tick();
    expect(input.value).toBe("5000");
    expect(document.activeElement).toBe(input);
  });

  it("puts a box left empty back as it was, and saves nothing", async () => {
    const o = await open({ settings: { nodeCap: 700 } });
    const input = type(o, "");
    await tick();
    expect(input.value).toBe("700");
    expect(o.pm.kv.get("settings")?.revision).toBe(1);
  });

  it("saves nothing when the cap is typed back to what it was", async () => {
    const o = await open({ settings: { nodeCap: 700 } });
    type(o, "700");
    await tick();
    expect(o.pm.kv.get("settings")?.revision).toBe(1);
  });
});

describe("the map under the pointer", () => {
  /** A document dot of the by-meaning layout, as the screen position it was last drawn at. */
  async function semanticMap(extra: Partial<OpenOptions> = {}) {
    const o = await open({ settings: { layoutMode: "semantic" }, ...extra });
    const [x, y, r] = draw()[0];
    return { o, x, y, r };
  }

  it("shows a card for the document under the pointer, and hides it when the pointer leaves", async () => {
    const { o, x, y } = await semanticMap();
    expect(o.hidden(".card")).toBe(true);
    o.el(".canvas").dispatchEvent(mouse("mousemove", { clientX: x, clientY: y }));
    expect(o.hidden(".card")).toBe(false);
    expect(o.text(".card-title")).toMatch(/^Document \d+/);
    expect(Array.from(o.mount.querySelectorAll(".card dt")).map((t) => t.textContent)).toEqual([
      "Project",
      "Importance",
      "Chunks",
      "Reviewed",
    ]);
    o.el(".canvas").dispatchEvent(mouse("mouseleave"));
    expect(o.hidden(".card")).toBe(true);
  });

  it("adds the date ingested to the card at Power depth, and never a path", async () => {
    const o = await open({
      settings: { layoutMode: "semantic" },
      theme: { vars: {}, mode: "dark", colorblind: false, depth: "power" },
    });
    const [x, y] = draw()[0];
    o.el(".canvas").dispatchEvent(mouse("mousemove", { clientX: x, clientY: y }));
    const labels = Array.from(o.mount.querySelectorAll(".card dt")).map((t) => t.textContent);
    expect(labels).toContain("Ingested");
    expect(labels).not.toContain("Path");
  });

  it("follows a change of depth while the card is showing", async () => {
    const { o, x, y } = await semanticMap();
    o.el(".canvas").dispatchEvent(mouse("mousemove", { clientX: x, clientY: y }));
    expect(o.mount.querySelectorAll(".card dt")).toHaveLength(4);
    o.pm.emit({
      name: "theme.changed",
      seq: 1,
      payload: { vars: {}, mode: "dark", colorblind: false, depth: "power" },
    });
    expect(o.mount.querySelectorAll(".card dt")).toHaveLength(5);
  });

  it("opens the document in PM's reader when it is clicked", async () => {
    const { o, x, y } = await semanticMap();
    const canvas = o.el(".canvas");
    canvas.dispatchEvent(mouse("mousedown", { button: 0, clientX: x, clientY: y, buttons: 1 }));
    window.dispatchEvent(mouse("mouseup", { clientX: x, clientY: y }));
    await tick();
    expect(o.pm.opened).toHaveLength(1);
    expect(o.pm.of("ui.openDocument")[0].args).toEqual({ id: o.pm.opened[0] });
    expect(o.hidden(".strip-error")).toBe(true);
  });

  it("says so when the document could not be opened (it went while the map was open)", async () => {
    const { o, x, y } = await semanticMap();
    // The click lands on a document PM no longer has: its list still shows it.
    const target = Array.from({ length: 12 }, (_, i) => i + 1);
    for (const id of target) o.pm.removeDoc(id);
    const canvas = o.el(".canvas");
    canvas.dispatchEvent(mouse("mousedown", { button: 0, clientX: x, clientY: y, buttons: 1 }));
    window.dispatchEvent(mouse("mouseup", { clientX: x, clientY: y }));
    await tick();
    expect(o.text(".strip-error .strip-text")).toBe("That document couldn't be opened.");
    expect(o.text(".strip-error .link")).toBe("Dismiss");
  });
});

describe("PM's theme changing", () => {
  it("repaints with the new palette, the new vars and the new mode", async () => {
    const o = await open();
    expect(legendColors(o)).toEqual(DARK);
    o.pm.emit({
      name: "theme.changed",
      seq: 1,
      payload: {
        vars: { "--bg": "oklch(0.98 0 0)" },
        mode: "light",
        colorblind: false,
        depth: "standard",
      },
    });
    expect(legendColors(o)).toEqual(["#2563eb", "#059669", "#db2777"]);
    expect(document.documentElement.style.getPropertyValue("--bg")).toBe("oklch(0.98 0 0)");
    expect(document.documentElement.dataset.mode).toBe("light");
    // The map is redrawn, in place: no new layout work, no refit.
    expect(rig.framesPending()).toBe(1);
    expect(callsOf(o, "documents.list")).toBe(1);
  });

  it("switches to the colour-blind palette", async () => {
    const o = await open();
    o.pm.emit({
      name: "theme.changed",
      seq: 1,
      payload: { vars: {}, mode: "dark", colorblind: true, depth: "standard" },
    });
    expect(legendColors(o)).toEqual(["#56b4e9", "#e69f00", "#009e73"]);
  });

  it("takes away a property the new theme no longer has", async () => {
    const o = await open({
      theme: { vars: { "--bg": "x", "--ink": "y" }, mode: "dark", colorblind: false },
    });
    o.pm.emit({
      name: "theme.changed",
      seq: 1,
      payload: { vars: { "--bg": "z" }, mode: "dark", colorblind: false, depth: "standard" },
    });
    expect(document.documentElement.style.getPropertyValue("--ink")).toBe("");
    expect(document.documentElement.style.getPropertyValue("--bg")).toBe("z");
  });

  it("ignores a theme it cannot read and other events", async () => {
    const o = await open();
    o.pm.emit({ name: "theme.changed", seq: 1, payload: "nonsense" });
    o.pm.emit({ name: "grants.changed", seq: 2, payload: {} });
    expect(legendColors(o)).toEqual(DARK);
    expect(o.hidden(".strip-error")).toBe(true);
  });
});

describe("documents changing while the map is open", () => {
  const added = (doc: MapDoc) => ({
    name: "documents.added" as const,
    seq: 1,
    payload: { documents: [doc] },
  });

  it("says so once things go quiet, and Refresh draws the map again", async () => {
    const o = await open();
    vi.useFakeTimers();
    const fresh: MapDoc = doc(99, "Atlas", 3, "New one");
    o.pm.addDoc(fresh, new Float32Array(8).fill(0.5));
    o.pm.emit(added(fresh));
    vi.advanceTimersByTime(QUIET_MS - 1);
    expect(noticeStrip(o).hidden).toBe(true);
    vi.advanceTimersByTime(1);
    expect(noticeStrip(o).hidden).toBe(false);
    expect(noticeStrip(o).textContent).toContain("1 document was added.");
    // The map did not move under the person.
    expect(o.text(".subtitle")).toContain("12 documents");
    vi.useRealTimers();
    click(noticeStrip(o).querySelector("button.link") as Element);
    await tick();
    await tick();
    expect(noticeStrip(o).hidden).toBe(true);
    expect(o.text(".subtitle")).toContain("13 documents");
    expect(callsOf(o, "documents.list")).toBe(2);
  });

  it("can be dismissed without refreshing", async () => {
    const o = await open();
    vi.useFakeTimers();
    o.pm.emit({ name: "documents.deleted", seq: 1, payload: { ids: [1, 2] } });
    vi.advanceTimersByTime(QUIET_MS);
    expect(noticeStrip(o).textContent).toContain("2 documents were removed.");
    click(noticeStrip(o).querySelector("button.quiet") as Element);
    expect(noticeStrip(o).hidden).toBe(true);
    expect(callsOf(o, "documents.list")).toBe(1);
  });

  it("says only that things changed when PM dropped events", async () => {
    const o = await open();
    vi.useFakeTimers();
    o.pm.emit({ name: "grants.changed", seq: 5, payload: {}, missed: 3 });
    vi.advanceTimersByTime(QUIET_MS);
    expect(noticeStrip(o).textContent).toContain("Your documents changed while the map was open.");
  });

  it("is not raised for a theme event", async () => {
    const o = await open();
    vi.useFakeTimers();
    o.pm.emit({ name: "theme.changed", seq: 1, payload: { vars: {} } });
    vi.advanceTimersByTime(60_000);
    expect(noticeStrip(o).hidden).toBe(true);
  });

  it("the latest of two overlapping loads is the one that shows", async () => {
    let gate!: Gate;
    const o = await open({ wrap: (pm) => (gate = gated(pm, "documents.list")).host, noWait: true });
    await tick();
    expect(gate.held).toBe(1);
    // Refresh while the first load is still waiting on PM.
    const second = o.app.reload();
    await tick();
    expect(gate.held).toBe(2);
    gate.open();
    await o.app.ready;
    await second;
    await tick();
    expect(callsOf(o, "documents.list")).toBe(2);
    expect(draw()).toHaveLength(15);
    expect(callsOf(o, "storage.get")).toBeGreaterThan(0);
    // The stale first load did not lay the map out a second time.
    expect(callsOf(o, "storage.set")).toBeLessThanOrEqual(2);
  });
});

/** The strip that carries the "documents changed" notice: the status strip with a Refresh button. */
function noticeStrip(o: Opened): HTMLElement {
  const strips = Array.from(o.mount.querySelectorAll<HTMLElement>(".strip[role=status]"));
  const found = strips.find((s) => s.querySelector("button.link") !== null);
  if (!found) throw new Error("no notice strip");
  return found;
}

describe("going away", () => {
  it("takes the page, its listeners and its worker with it", async () => {
    const real = inlineCompute();
    let closed = 0;
    const compute: Compute = { ...real, close: () => (closed += 1) };
    const o = await open({ compute });
    expect(o.mount.children).toHaveLength(1);
    o.app.destroy();
    expect(o.mount.children).toHaveLength(0);
    expect(closed).toBe(1);
    expect(rig.framesPending()).toBe(0);
    // Nothing left listening: PM's events, the keyboard, the wheel do nothing and do not throw.
    expect(() => {
      o.pm.emit({ name: "theme.changed", seq: 1, payload: { vars: { "--zzz": "1" } } });
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    }).not.toThrow();
    expect(document.documentElement.style.getPropertyValue("--zzz")).toBe("");
    expect(o.pm.handlers.size).toBe(0);
  });

  it("is safe twice, and a load still in flight does nothing when it lands", async () => {
    let gate!: Gate;
    const o = await open({ noWait: true, wrap: (pm) => (gate = gated(pm, "documents.list")).host });
    await tick();
    o.app.destroy();
    o.app.destroy();
    gate.open();
    await o.app.ready;
    await tick();
    expect(o.mount.children).toHaveLength(0);
    expect(rig.framesPending()).toBe(0);
  });
});

describe("what the page asks PM for is what the manifest declares", () => {
  const rows = new Map(
    Array.from(tableRs.matchAll(/\bcall\(\s*"([\w.]+)",\s*"([\w.]+)"/g)).map((m) => [m[1], m[2]]),
  );
  const permissions = new Set(manifest.permissions.map((p) => p.id));
  const keys = new Map(manifest.data.keys.map((k) => [k.key, k.class]));

  it("every call of a whole session is a row of the dispatch table under a permission it asks for, on a declared key", async () => {
    const o = await open({ settings: { layoutMode: "semantic" } });
    // Everything a person can do: arrangement, cohesion, names, the cap, a click, a refresh.
    click(o.el("button.seg-button:first-child"));
    await tick();
    click(o.el("button.seg-button:last-child"));
    await tick();
    const select = o.el<HTMLSelectElement>("#map-cohesion");
    select.value = "0.3";
    select.dispatchEvent(new Event("change", { bubbles: true }));
    click(o.el("button.chip"));
    const input = o.el<HTMLInputElement>("#map-node-cap");
    input.value = "300";
    input.dispatchEvent(new Event("change", { bubbles: true }));
    await tick();
    await tick();
    const [x, y] = draw()[0];
    o.el(".canvas").dispatchEvent(mouse("mousedown", { button: 0, clientX: x, clientY: y }));
    window.dispatchEvent(mouse("mouseup", { clientX: x, clientY: y }));
    await o.app.reload();
    await tick();

    const methods = Array.from(new Set(o.pm.calls.map((c) => c.method))).sort();
    expect(methods).toEqual([
      "documents.list",
      "embeddings.documentVectors",
      "embeddings.signature",
      "storage.doc.delete",
      "storage.doc.list",
      "storage.doc.set",
      "storage.get",
      "storage.set",
      "theme.get",
      "ui.openDocument",
    ]);
    for (const method of methods) {
      const permission = rows.get(method);
      expect(permission, `${method} is not a call of the dispatch table`).toBeDefined();
      expect(permissions.has(permission as string), `${method} needs ${permission}`).toBe(true);
    }
    const usedKeys = new Set(
      o.pm.calls
        .filter((c) => c.method.startsWith("storage."))
        .map((c) => (c.args as { key: string }).key),
    );
    for (const key of usedKeys) expect(keys.has(key), `${key} is not declared`).toBe(true);
    expect(Array.from(usedKeys).sort()).toEqual([
      "project-layout",
      "semantic-coords",
      "semantic-meta",
      "settings",
    ]);
    // The one user's key is content class; per-document rows are only ever under a cache key.
    for (const c of o.pm.calls.filter((c) => c.method.startsWith("storage.doc."))) {
      expect(keys.get((c.args as { key: string }).key)).toBe("cache");
    }
  });

  it("listens only for events a permission it asks for lets it hear", () => {
    const events = new Map(
      Array.from(tableRs.matchAll(/\bevent\(\s*"([\w.]+)",\s*Some\("([\w.]+)"\)/g)).map((m) => [
        m[1],
        m[2],
      ]),
    );
    // theme.changed (the page's theme) and the two document events (the notice).
    for (const name of ["theme.changed", "documents.added", "documents.deleted"]) {
      expect(permissions.has(events.get(name) as string), name).toBe(true);
    }
  });
});
