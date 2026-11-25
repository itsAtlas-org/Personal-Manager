// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// PM's theme as the frame reads it: what a snapshot is taken to mean, what is left out of one, and the
// store the kit's provider is given (the first snapshot, each later one PM sends, and PM's own variables).

import { afterEach, describe, expect, it, vi } from "vitest";
import { connectPm } from "../lib/pm";
import { memoryPm } from "../testing";
import { frameThemeStore, readSnapshot } from "./theme";

afterEach(() => connectPm(null));

const SNAPSHOT = {
  vars: {
    "--bg": "oklch(0.2 0 0)",
    "--font-scale": "1.15",
    "--tap-min": "44px",
    "--ui": '"Atkinson Hyperlegible", system-ui',
  },
  mode: "light",
  system: "terminal",
  accent: "mono",
  colorblind: true,
  depth: "power",
};

describe("readSnapshot", () => {
  it("reads the axes into the blob the provider takes, and the text size and density from the variables", () => {
    const snap = readSnapshot(SNAPSHOT);
    expect(JSON.parse(snap?.blob ?? "null")).toEqual({
      system: "terminal",
      modePref: "light",
      accent: "mono",
      depth: "power",
      colorblind: true,
      fontScale: "large",
      density: "comfortable",
      legibleFont: true,
    });
    expect(snap?.vars["--bg"]).toBe("oklch(0.2 0 0)");
  });

  it("leaves out a variable that is not a plain custom property with a short text value", () => {
    const snap = readSnapshot({
      vars: {
        "--ok": "1",
        background: "red",
        "--Upper": "x",
        "--long": "x".repeat(513),
        "--num": 5,
        [`--${"n".repeat(70)}`]: "x",
      },
    });
    expect(Object.keys(snap?.vars ?? {})).toEqual(["--ok"]);
  });

  it("takes at most 200 variables", () => {
    const vars = Object.fromEntries(Array.from({ length: 300 }, (_, i) => [`--v${i}`, "1"]));
    expect(Object.keys(readSnapshot({ vars })?.vars ?? {})).toHaveLength(200);
  });

  it("is not a theme when it has no vars object, and a mode that is not light is dark", () => {
    for (const bad of [null, 5, [], {}, { vars: [] }, { vars: null }])
      expect(readSnapshot(bad)).toBeNull();
    expect(JSON.parse(readSnapshot({ vars: {}, mode: "weird" })?.blob ?? "{}").modePref).toBe(
      "dark",
    );
    expect(JSON.parse(readSnapshot({ vars: {}, depth: "weird" })?.blob ?? "{}").depth).toBe(
      "standard",
    );
  });

  it("picks the nearest text size to what PM set", () => {
    const size = (scale: string) =>
      JSON.parse(readSnapshot({ vars: { "--font-scale": scale } })?.blob ?? "{}").fontScale;
    expect([size("0.9"), size("1"), size("1.1"), size("1.3"), size("nonsense")]).toEqual([
      "small",
      "default",
      "large",
      "xlarge",
      "default",
    ]);
  });
});

describe("frameThemeStore", () => {
  it("hands the provider the first snapshot (theme.get) and PM's variables, and is ready once it has", async () => {
    const pm = await memoryPm({ answers: { "theme.get": SNAPSHOT } });
    const store = frameThemeStore(pm.host);
    await store.ready;
    expect(JSON.parse((await store.get()) ?? "null").system).toBe("terminal");
    expect(store.vars?.()?.["--bg"]).toBe("oklch(0.2 0 0)");
    expect(pm.of("theme.get")).toHaveLength(1);
  });

  it("tells a subscriber about each theme.changed PM sends, and not after it stops", async () => {
    const pm = await memoryPm({ answers: { "theme.get": SNAPSHOT } });
    const store = frameThemeStore(pm.host);
    await store.ready;
    const heard = vi.fn();
    const stop = store.subscribe?.(heard) ?? (() => {});
    pm.emit("theme.changed", { ...SNAPSHOT, mode: "dark", vars: { "--bg": "oklch(0.1 0 0)" } });
    await pm.settle();
    expect(heard).toHaveBeenCalledTimes(1);
    expect(JSON.parse(heard.mock.calls[0][0]).modePref).toBe("dark");
    expect(store.vars?.()?.["--bg"]).toBe("oklch(0.1 0 0)");
    stop();
    pm.emit("theme.changed", SNAPSHOT);
    await pm.settle();
    expect(heard).toHaveBeenCalledTimes(1);
  });

  it("keeps the theme it has when PM sends something that is not one", async () => {
    const pm = await memoryPm({ answers: { "theme.get": SNAPSHOT } });
    const store = frameThemeStore(pm.host);
    await store.ready;
    pm.emit("theme.changed", { nope: true });
    await pm.settle();
    expect(store.vars?.()?.["--bg"]).toBe("oklch(0.2 0 0)");
  });

  it("with a PM that does not answer, there is no theme and the page still starts", async () => {
    const pm = await memoryPm();
    const store = frameThemeStore(pm.host);
    await store.ready;
    expect(await store.get()).toBeNull();
    expect(store.vars?.()).toBeNull();
    await store.set("{}");
  });
});
