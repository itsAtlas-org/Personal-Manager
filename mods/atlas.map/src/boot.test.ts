// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Starting the page: the connection, what is said before a map exists, and the whole page run through
// the REAL frame SDK against a fake PM (the page's own reading rules and flood stop, over a real
// MessageChannel), so what the Map does is held to what the SDK lets a mod do.

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { fakePm } from "../../../src/kit/fakePm";
import { connectWith, HostError } from "../../../src/kit/host";
import { boot, type BootEnv } from "./boot";
import { domTesting } from "./domTesting";
import type { WorkerLike } from "./compute";
import type { MapHost } from "./hostApi";
import { memoryHost } from "./memoryHost";
import { doc, vectorsFor } from "./testing";
import type { MapApp } from "./app";

const rig = domTesting();
beforeEach(() => {
  rig.install();
  document.body.replaceChildren();
  const app = document.createElement("div");
  app.id = "app";
  document.body.append(app);
});
const booted: MapApp[] = [];
afterEach(() => {
  for (const app of booted.splice(0)) app.destroy();
  rig.uninstall();
  document.body.replaceChildren();
  document.documentElement.removeAttribute("style");
});

const NAMES = ["Atlas", "Beta", "Gamma"];
const docs = Array.from({ length: 9 }, (_, i) => doc(i + 1, NAMES[i % 3], 3 + i, `Doc ${i + 1}`));
const tick = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

const noWorker = (): WorkerLike => {
  throw new Error("no worker in this test");
};
const env = (connect: BootEnv["connect"], spawn: BootEnv["spawn"] = noWorker): BootEnv => ({
  doc: document,
  win: window as Window & typeof globalThis,
  connect,
  spawn,
});
const text = () => document.getElementById("app")?.textContent ?? "";

describe("boot", () => {
  it("says it is starting while it waits for PM", async () => {
    let resolve!: (h: MapHost) => void;
    const pending = new Promise<MapHost>((r) => (resolve = r));
    const started = boot(env(() => pending));
    expect(text()).toBe("Starting the map…");
    resolve(memoryHost({ docs: [] }).host);
    const app = await started;
    booted.push(app as MapApp);
    expect(text()).not.toContain("Starting the map");
  });

  it("says the Map opens inside PM when the page is not framed, and builds nothing", async () => {
    const app = await boot(
      env(() => Promise.reject(new HostError("not-framed", "This page is not inside PM's frame."))),
    );
    expect(app).toBeNull();
    expect(text()).toBe("The Map opens inside PM.");
    expect(document.querySelector(".map")).toBeNull();
  });

  it.each([
    ["no port", new HostError("no-port", "PM did not answer.")],
    ["a closed host", new HostError("closed", "closed")],
    ["anything else", new Error("boom: secret detail")],
  ])("says PM did not answer for %s, in the Map's words only", async (_name, error) => {
    const app = await boot(env(() => Promise.reject(error)));
    expect(app).toBeNull();
    expect(text()).toBe("PM didn't answer. Close this tab and open it again.");
    expect(text()).not.toContain("secret");
  });

  it("builds into the page's own body when there is no #app", async () => {
    document.body.replaceChildren();
    const app = await boot(env(() => Promise.resolve(memoryHost({ docs: [] }).host)));
    booted.push(app as MapApp);
    expect(document.body.querySelector(".map")).not.toBeNull();
  });

  it("builds the Map in #app and replaces the placeholder", async () => {
    const pm = memoryHost({ docs, vectors: vectorsFor(docs, 8), dimension: 8 });
    const app = (await boot(env(() => Promise.resolve(pm.host)))) as MapApp;
    booted.push(app);
    await app.ready;
    expect(document.querySelector("#app > .map")).not.toBeNull();
    expect(document.querySelectorAll("#app .plain")).toHaveLength(0);
    expect(document.querySelector(".subtitle")?.textContent).toContain("9 documents across 3");
  });

  it("falls back to the page's own thread when no Worker can be made, and still draws", async () => {
    const pm = memoryHost({ docs, vectors: vectorsFor(docs, 8), dimension: 8 });
    const app = (await boot(env(() => Promise.resolve(pm.host), noWorker))) as MapApp;
    booted.push(app);
    await app.ready;
    rig.frames().run();
    expect(rig.canvas().of("arc").length).toBe(9 + 3);
    expect(document.querySelector(".strip-error")?.hasAttribute("hidden")).toBe(true);
  });

  it("stops the Map when the page goes away", async () => {
    const pm = memoryHost({ docs, vectors: vectorsFor(docs, 8), dimension: 8 });
    const app = (await boot(env(() => Promise.resolve(pm.host)))) as MapApp;
    booted.push(app);
    await app.ready;
    window.dispatchEvent(new Event("pagehide"));
    expect(document.querySelector(".map")).toBeNull();
    expect(pm.handlers.size).toBe(0);
  });
});

describe("the whole page over the real frame SDK", () => {
  async function overSdk(settings?: unknown) {
    const pm = memoryHost({
      docs,
      vectors: vectorsFor(docs, 8),
      dimension: 8,
      contentKeys: ["settings"],
    });
    if (settings !== undefined) pm.kv.set("settings", { value: settings, revision: 1 });
    const fake = fakePm(pm.backend);
    const app = (await boot(env(() => connectWith(fake.env)))) as MapApp;
    booted.push(app);
    await app.ready;
    await fake.settle();
    return { pm, fake, app };
  }

  it("draws the map by project, themed, with PM's flood stop untouched", async () => {
    const { pm, fake } = await overSdk();
    expect(fake.stats.stopped).toBe(false);
    expect(fake.stats.refused).toBe(0);
    expect(pm.calls.map((c) => c.method).slice(0, 3)).toEqual([
      "theme.get",
      "storage.get",
      "documents.list",
    ]);
    expect(document.querySelector(".subtitle")?.textContent).toContain("9 documents across 3");
    expect(document.documentElement.style.getPropertyValue("--bg")).toBe("oklch(0.16 0.01 270)");
    rig.frames().run();
    expect(rig.canvas().of("arc").length).toBe(12);
  });

  it("lays the map out by meaning through the binary vectors answer", async () => {
    const { fake } = await overSdk({ layoutMode: "semantic" });
    expect(fake.stats.stopped).toBe(false);
    rig.frames().run();
    expect(rig.canvas().of("arc").length).toBe(9);
    expect(document.querySelector(".strip-error")?.hasAttribute("hidden")).toBe(true);
  });

  it("follows PM's theme.changed over the port", async () => {
    const { fake } = await overSdk();
    fake.emit("theme.changed", {
      vars: { "--bg": "oklch(0.98 0 0)" },
      mode: "light",
      colorblind: false,
      depth: "standard",
    });
    await fake.settle();
    await tick();
    expect(document.documentElement.dataset.mode).toBe("light");
    expect(document.querySelector(".legend circle")?.getAttribute("fill")).toBe("#2563eb");
  });

  it("opens a document through the page action", async () => {
    const { pm, fake } = await overSdk({ layoutMode: "semantic" });
    rig.frames().run();
    const arc = rig.canvas().of("arc")[0].args as number[];
    const canvas = document.querySelector(".canvas") as HTMLElement;
    canvas.dispatchEvent(
      new MouseEvent("mousedown", { bubbles: true, button: 0, clientX: arc[0], clientY: arc[1] }),
    );
    window.dispatchEvent(new MouseEvent("mouseup", { clientX: arc[0], clientY: arc[1] }));
    await fake.settle();
    await tick();
    expect(pm.opened).toHaveLength(1);
  });
});
