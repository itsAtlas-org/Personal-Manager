// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Map's one settings key: what a stored value means, and saving without losing another frame's
// change. The clamps and defaults are the ones the core Map applied (src/lib/mapPrefs.ts for the
// arrangement and cohesion, layout.rs `read_node_cap` for the node cap).

import { describe, expect, it } from "vitest";
import { ModCallError } from "../../../src/kit/host";
import { CODE, type MapHost } from "./hostApi";
import { memoryHost } from "./memoryHost";
import {
  COHESION_MAX,
  DEFAULT_SETTINGS,
  KEY_SETTINGS,
  NODE_CAP_DEFAULT,
  NODE_CAP_MAX,
  NODE_CAP_MIN,
  loadSettings,
  parseSettings,
  saveSettings,
} from "./settings";

describe("parseSettings", () => {
  it("the defaults are the core Map's: by project, no cohesion, no names, a cap of 1000", () => {
    expect(DEFAULT_SETTINGS).toEqual({
      layoutMode: "project",
      cohesion: 0,
      labels: false,
      nodeCap: 1000,
      enhanced: true,
    });
    expect([NODE_CAP_MIN, NODE_CAP_DEFAULT, NODE_CAP_MAX, COHESION_MAX]).toEqual([
      200, 1000, 5000, 0.5,
    ]);
    expect(parseSettings(undefined)).toEqual(DEFAULT_SETTINGS);
  });

  it.each([null, undefined, 5, "x", true, [], [1, 2]])("%j means the defaults", (value) => {
    expect(parseSettings(value)).toEqual(DEFAULT_SETTINGS);
  });

  it("reads a complete value", () => {
    expect(
      parseSettings({ layoutMode: "semantic", cohesion: 0.3, labels: true, nodeCap: 2500 }),
    ).toEqual({
      layoutMode: "semantic",
      cohesion: 0.3,
      labels: true,
      nodeCap: 2500,
      enhanced: true,
    });
  });

  it("a partial value (the install-time handover writes only nodeCap) takes defaults for the rest", () => {
    expect(parseSettings({ nodeCap: 1500 })).toEqual({ ...DEFAULT_SETTINGS, nodeCap: 1500 });
  });

  it("anything but 'semantic' is the by-project default", () => {
    for (const layoutMode of ["project", "other", "", 1, null, "SEMANTIC"]) {
      expect(parseSettings({ layoutMode }).layoutMode).toBe("project");
    }
  });

  it("cohesion is clamped to 0..0.5, and not a number is 0", () => {
    expect(parseSettings({ cohesion: 0.9 }).cohesion).toBe(0.5);
    expect(parseSettings({ cohesion: -3 }).cohesion).toBe(0);
    expect(parseSettings({ cohesion: 0.15 }).cohesion).toBe(0.15);
    for (const cohesion of ["0.3", NaN, Infinity, null, {}]) {
      expect(parseSettings({ cohesion }).cohesion).toBe(0);
    }
  });

  it("labels is on only for true", () => {
    expect(parseSettings({ labels: true }).labels).toBe(true);
    for (const labels of [1, "1", "true", null, 0])
      expect(parseSettings({ labels }).labels).toBe(false);
  });

  it("the node cap is a whole number clamped to 200..5000; anything else is the default (as the core read it)", () => {
    expect(parseSettings({ nodeCap: 0 }).nodeCap).toBe(200);
    expect(parseSettings({ nodeCap: 50 }).nodeCap).toBe(200);
    expect(parseSettings({ nodeCap: 99999 }).nodeCap).toBe(5000);
    expect(parseSettings({ nodeCap: 200 }).nodeCap).toBe(200);
    expect(parseSettings({ nodeCap: 5000 }).nodeCap).toBe(5000);
    for (const nodeCap of [-5, 1500.5, "1500", NaN, Infinity, null]) {
      expect(parseSettings({ nodeCap }).nodeCap).toBe(1000);
    }
  });

  it("enhanced is on by default, and a stored boolean is kept", () => {
    expect(DEFAULT_SETTINGS.enhanced).toBe(true);
    expect(parseSettings({}).enhanced).toBe(true);
    expect(parseSettings(null).enhanced).toBe(true);
    expect(parseSettings({ enhanced: true }).enhanced).toBe(true);
    expect(parseSettings({ enhanced: false }).enhanced).toBe(false);
  });

  it("enhanced that is not a boolean is the default, not coerced", () => {
    for (const enhanced of ["false", "true", 0, 1, null, [], {}, undefined, NaN]) {
      expect(parseSettings({ enhanced }).enhanced, JSON.stringify(enhanced)).toBe(true);
    }
  });

  it("drops a field it does not know", () => {
    expect(parseSettings({ ...DEFAULT_SETTINGS, tsneEnabled: true, extra: 1 })).toEqual(
      DEFAULT_SETTINGS,
    );
  });
});

describe("loadSettings", () => {
  it("defaults and revision 0 for a Map that has never saved", async () => {
    const pm = memoryHost({ docs: [] });
    expect(await loadSettings(pm.host)).toEqual({ settings: DEFAULT_SETTINGS, revision: 0 });
  });

  it("reads the key and the revision it is at", async () => {
    const pm = memoryHost({ docs: [] });
    pm.kv.set(KEY_SETTINGS, { value: { layoutMode: "semantic" }, revision: 6 });
    expect(await loadSettings(pm.host)).toEqual({
      settings: { ...DEFAULT_SETTINGS, layoutMode: "semantic" },
      revision: 6,
    });
  });

  it("a PM that cannot be read is an error, not silently the defaults", async () => {
    const pm = memoryHost({ docs: [] });
    pm.failAlways("storage.get", "MOD-9001");
    await expect(loadSettings(pm.host)).rejects.toMatchObject({ code: "MOD-9001" });
  });
});

describe("saveSettings", () => {
  it("writes the whole object, at the revision it read, and returns it", async () => {
    const pm = memoryHost({ docs: [] });
    const saved = await saveSettings(pm.host, { layoutMode: "semantic" });
    expect(saved).toEqual({ ...DEFAULT_SETTINGS, layoutMode: "semantic" });
    expect(pm.of("storage.set")).toEqual([
      { method: "storage.set", args: { key: KEY_SETTINGS, value: saved, expectedRevision: 0 } },
    ]);
    expect(pm.kv.get(KEY_SETTINGS)).toEqual({ value: saved, revision: 1 });
  });

  it("changes only what the patch names", async () => {
    const pm = memoryHost({ docs: [] });
    await saveSettings(pm.host, { layoutMode: "semantic", labels: true });
    const next = await saveSettings(pm.host, { cohesion: 0.3 });
    expect(next).toEqual({
      layoutMode: "semantic",
      cohesion: 0.3,
      labels: true,
      nodeCap: 1000,
      enhanced: true,
    });
    expect(pm.kv.get(KEY_SETTINGS)!.revision).toBe(2);
  });

  it("clamps a patch the way it clamps a stored value", async () => {
    const pm = memoryHost({ docs: [] });
    const next = await saveSettings(pm.host, { nodeCap: 99999, cohesion: 7 });
    expect(next.nodeCap).toBe(5000);
    expect(next.cohesion).toBe(0.5);
  });

  it("saves the Enhanced switch like any other setting, and keeps the rest", async () => {
    const pm = memoryHost({ docs: [] });
    await saveSettings(pm.host, { labels: true });
    const next = await saveSettings(pm.host, { enhanced: false });
    expect(next).toEqual({ ...DEFAULT_SETTINGS, labels: true, enhanced: false });
    expect(pm.kv.get(KEY_SETTINGS)?.value).toEqual(next);
    // And junk in storage is the default again on the next read.
    pm.kv.set(KEY_SETTINGS, { value: { enhanced: "no" }, revision: 5 });
    expect((await loadSettings(pm.host)).settings.enhanced).toBe(true);
  });

  it("completes a handed-over value: only nodeCap was there, now all five are", async () => {
    const pm = memoryHost({ docs: [] });
    pm.kv.set(KEY_SETTINGS, { value: { nodeCap: 1500 }, revision: 1 });
    await saveSettings(pm.host, { labels: true });
    expect(pm.kv.get(KEY_SETTINGS)!.value).toEqual({
      ...DEFAULT_SETTINGS,
      nodeCap: 1500,
      labels: true,
    });
  });

  it("when another frame saved first, re-reads and applies the patch on top, so neither change is lost", async () => {
    const pm = memoryHost({ docs: [] });
    let interfered = false;
    const racing: MapHost = {
      ...pm.host,
      call: (method, args) => {
        if (method === "storage.set" && !interfered) {
          interfered = true;
          // The other frame's save lands between this frame's read and write.
          pm.kv.set(KEY_SETTINGS, { value: { ...DEFAULT_SETTINGS, labels: true }, revision: 1 });
        }
        return pm.host.call(method, args);
      },
    };
    const saved = await saveSettings(racing, { cohesion: 0.3 });
    expect(saved).toEqual({ ...DEFAULT_SETTINGS, labels: true, cohesion: 0.3 });
    expect(pm.kv.get(KEY_SETTINGS)).toEqual({ value: saved, revision: 2 });
    expect(pm.of("storage.set")).toHaveLength(2);
  });

  it("gives up after three attempts that are all stale, with PM's error", async () => {
    const pm = memoryHost({ docs: [] });
    pm.failAlways("storage.set", CODE.staleRevision);
    await expect(saveSettings(pm.host, { labels: true })).rejects.toMatchObject({
      code: CODE.staleRevision,
    });
    expect(pm.of("storage.set")).toHaveLength(3);
    expect(pm.of("storage.get")).toHaveLength(3);
  });

  it("any other refusal is not retried", async () => {
    const pm = memoryHost({ docs: [] });
    pm.failAlways("storage.set", CODE.quotaExceeded);
    await expect(saveSettings(pm.host, { labels: true })).rejects.toBeInstanceOf(ModCallError);
    expect(pm.of("storage.set")).toHaveLength(1);
  });
});
