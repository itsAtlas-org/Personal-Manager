// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The sidebar's half of a core tab becoming a mod's (the Map, PR B plan B8): a placement stored for
// `graph` means `mod:atlas.map` once `graph` is no longer one of PM's own tabs, and not before; and
// the one-time "installed hidden" rule. Both are keyed on that fact. The core Map tab is gone now
// (Mods PR B2), so the real list is the "after" state; the "before" state is tested by passing the
// set of ids that still had `graph`.

import { describe, expect, it } from "vitest";
import autoinstallRs from "../../src-tauri/src/mods/autoinstall.rs?raw";
import {
  adoptLegacy,
  CORE_NAV,
  isNavIdShape,
  modNavEntries,
  parsePlacement,
  pilotCoreTab,
  pilotFeature,
  pilotHiddenRuleApplies,
  pilotModIds,
  PINBOARD_VIEW,
  placementAfterPilotLanded,
  renamedNavId,
  resolveNav,
  type NavContext,
  type NavPlacement,
} from "./nav";

/** PM's own tabs: the core Map is gone, so `graph` is not among them. */
const CORE = new Set(CORE_NAV.map((e) => e.id as string));
const WITHOUT_GRAPH = CORE;
/** What they were while the core Map tab existed. */
const WITH_GRAPH = new Set([...CORE, "graph"]);
const MAP = "mod:atlas.map";
const NONE: NavPlacement = { hidden: [], pinned: [] };
const STANDARD: NavContext = { depth: "standard", devMode: false };
const MINIMAL: NavContext = { depth: "min", devMode: false };

describe("a stored `graph` placement", () => {
  it("is the core Map tab for as long as one exists: nothing is renamed", () => {
    expect(WITH_GRAPH.has("graph")).toBe(true);
    expect(renamedNavId("graph", WITH_GRAPH)).toBe("graph");
    expect(parsePlacement('{"hidden":["graph"],"pinned":["documents"]}', WITH_GRAPH)).toEqual({
      hidden: ["graph"],
      pinned: ["documents"],
    });
    // A pin the user made on the mod's tab ahead of time is kept as a mod tab, as before.
    expect(parsePlacement('{"hidden":[],"pinned":["mod:atlas.map"]}', WITH_GRAPH).pinned).toEqual([
      MAP,
    ]);
  });

  it("is the Map mod's tab in PM as it ships: the core list has no `graph`", () => {
    expect(CORE.has("graph")).toBe(false);
    expect(renamedNavId("graph")).toBe(MAP);
    expect(parsePlacement('{"hidden":["graph"],"pinned":["documents"]}')).toEqual({
      hidden: [MAP],
      pinned: ["documents"],
    });
  });

  it("means the Map mod's tab once the core entry is gone, for a pin and for a hide", () => {
    expect(renamedNavId("graph", WITHOUT_GRAPH)).toBe(MAP);
    expect(
      parsePlacement('{"hidden":["graph","teach"],"pinned":["graph"]}', WITHOUT_GRAPH),
    ).toEqual({ hidden: [MAP, "teach"], pinned: [MAP] });
  });

  it("is dropped, as any id nobody knows, when the rename isn't on", () => {
    // The state the ordering hazard is about: without the translation, removing `graph` from the core
    // list would make `isNavIdShape` refuse every stored one.
    expect(isNavIdShape("graph", WITHOUT_GRAPH)).toBe(false);
    expect(isNavIdShape("graph", WITH_GRAPH)).toBe(true);
    expect(isNavIdShape(MAP, WITHOUT_GRAPH)).toBe(true);
  });

  it("doesn't list the same tab twice when both spellings were stored", () => {
    expect(
      parsePlacement('{"hidden":["graph","mod:atlas.map","graph"],"pinned":[]}', WITHOUT_GRAPH)
        .hidden,
    ).toEqual([MAP]);
  });

  it("leaves every other id alone, whichever state PM is in", () => {
    for (const core of [WITH_GRAPH, WITHOUT_GRAPH]) {
      for (const id of ["focus", "documents", "mods", "mod:acme.tool", "mod:atlas.pinboard"]) {
        expect(renamedNavId(id, core), id).toBe(id);
      }
    }
    // Inherited property names are not renames.
    expect(renamedNavId("constructor", WITHOUT_GRAPH)).toBe("constructor");
    expect(renamedNavId("__proto__", WITHOUT_GRAPH)).toBe("__proto__");
    expect(parsePlacement('{"hidden":["constructor"],"pinned":[]}', WITHOUT_GRAPH)).toEqual(NONE);
  });
});

describe("the installed-hidden rule", () => {
  it("does nothing while a core Map tab still exists", () => {
    expect(pilotHiddenRuleApplies("atlas.map", WITH_GRAPH)).toBe(false);
    expect(placementAfterPilotLanded(NONE, "atlas.map", "min", WITH_GRAPH)).toBe(NONE);
  });

  it("applies in PM as it ships, where the core Map tab is gone", () => {
    expect(pilotHiddenRuleApplies("atlas.map")).toBe(true);
    expect(placementAfterPilotLanded(NONE, "atlas.map", "min")).toEqual({
      hidden: [MAP],
      pinned: [],
    });
    expect(placementAfterPilotLanded(NONE, "atlas.map", "standard")).toBe(NONE);
  });

  it("hides the new tab on a Minimal device that wasn't showing the core tab, once the core entry is gone", () => {
    expect(pilotHiddenRuleApplies("atlas.map", WITHOUT_GRAPH)).toBe(true);
    const start: NavPlacement = { hidden: ["teach"], pinned: ["documents"] };
    expect(placementAfterPilotLanded(start, "atlas.map", "min", WITHOUT_GRAPH)).toEqual({
      hidden: ["teach", MAP],
      pinned: ["documents"],
    });
  });

  it("leaves the tab showing where the core tab was showing, or where the user pinned it", () => {
    for (const depth of ["standard", "power"] as const) {
      expect(placementAfterPilotLanded(NONE, "atlas.map", depth, WITHOUT_GRAPH)).toBe(NONE);
    }
    const pinned: NavPlacement = { hidden: [], pinned: [MAP] };
    expect(placementAfterPilotLanded(pinned, "atlas.map", "min", WITHOUT_GRAPH)).toBe(pinned);
  });

  it("doesn't add a tab twice, and knows no other mod", () => {
    const hidden: NavPlacement = { hidden: [MAP], pinned: [] };
    expect(placementAfterPilotLanded(hidden, "atlas.map", "min", WITHOUT_GRAPH)).toBe(hidden);
    expect(placementAfterPilotLanded(NONE, "acme.tool", "min", WITHOUT_GRAPH)).toBe(NONE);
    expect(pilotHiddenRuleApplies("acme.tool", WITHOUT_GRAPH)).toBe(false);
  });
});

describe("a mod PM is still installing", () => {
  it("has its tab from the synthesized entry, so the user can see the Installing screen and it survives tab switches", () => {
    const entries = modNavEntries([{ id: "atlas.map", name: "Map", navTab: "Map" }]);
    expect(entries.map((e) => [e.id, e.label])).toEqual([[MAP, "Map"]]);
  });

  it("shows by the old Map tab's rule (not at Minimal) so a Minimal user never sees it flicker in", () => {
    const [pending] = modNavEntries([
      { id: "atlas.map", name: "Map", navTab: "Map", pending: true },
    ]);
    expect(pending.defaultShown("min")).toBe(false);
    expect(pending.defaultShown("standard")).toBe(true);
    expect(pending.defaultShown("power")).toBe(true);
    expect(resolveNav([pending], NONE, STANDARD).shown.map((e) => e.id)).toEqual([MAP]);
    expect(resolveNav([pending], NONE, MINIMAL).shown).toEqual([]);
  });

  it("is shown at every Depth once landed, as any installed mod's tab is", () => {
    const [landed] = modNavEntries([
      { id: "atlas.map", name: "Map", navTab: "Map", pending: false },
    ]);
    expect(landed.defaultShown("min")).toBe(true);
    const [bare] = modNavEntries([{ id: "atlas.map", name: "Map", navTab: "Map" }]);
    expect(bare.defaultShown("min")).toBe(true);
  });
});

describe("adopting the old Map switch", () => {
  it("hides the mod's tab for a legacy hide, at every Depth", () => {
    for (const depth of ["min", "standard", "power"] as const) {
      expect(adoptLegacy(null, "hide", depth)).toEqual({ hidden: [MAP], pinned: [] });
    }
  });

  it("pins the mod's tab for a legacy show only where the Depth would leave it out", () => {
    expect(adoptLegacy(null, "show", "min")).toEqual({ hidden: [], pinned: [MAP] });
    expect(adoptLegacy(null, "show", "standard")).toEqual(NONE);
    expect(adoptLegacy(null, "show", "power")).toEqual(NONE);
  });
});

describe("what PM installs by itself", () => {
  it("names the feature the person used, and knows no other mod", () => {
    expect(pilotFeature("atlas.map")).toBe("Map");
    expect(pilotFeature("acme.tool")).toBeNull();
  });
});

describe("the sidebar's pilot tables and the backend's PILOTS", () => {
  // `src-tauri/src/mods/autoinstall.rs` is where PM names the mods it installs on its own. The
  // sidebar keeps its own two tables (the core tab each took over, and the stored id a placement
  // translates), and a pilot missing from either is a tab that appears, vanishes or loses its pin
  // for one user. Read the Rust table as text so the three cannot drift.
  const source = autoinstallRs;
  const table = source.split("pub const PILOTS")[1]?.split("\n];")[0] ?? "";
  const pilots = [
    ...table.matchAll(
      /id: "([^"]+)",\s*marker: "([^"]+)",\s*name: "([^"]+)",\s*tab_label: "([^"]+)",\s*landed: "([^"]+)"/g,
    ),
  ].map(([, id, marker, name, tabLabel, landed]) => ({ id, marker, name, tabLabel, landed }));

  it("reads the Rust table (the Map and the Pinboard)", () => {
    expect(pilots.map((p) => p.id)).toEqual(["atlas.map", "atlas.pinboard"]);
  });

  it("names the Pinboard's tab the way the Rust pilot table names the mod", () => {
    const pinboard = pilots.find((p) => p.id === "atlas.pinboard");
    expect(PINBOARD_VIEW).toBe(`mod:${pinboard?.id}`);
  });

  it("has the same pilots in the sidebar's table", () => {
    expect(pilotModIds().sort()).toEqual(pilots.map((p) => p.id).sort());
  });

  it("gives each pilot a core tab that a stored placement translates to its mod's tab", () => {
    const none = new Set<string>();
    for (const p of pilots) {
      const tab = pilotCoreTab(p.id);
      expect(tab, p.id).not.toBeNull();
      // Once PM's own list has no such tab (the Map and the Pinboard now) a pin or a hide
      // stored for it means the mod's tab; while PM's list has it, nothing is translated.
      expect(renamedNavId(tab as string, none), p.id).toBe(`mod:${p.id}`);
      expect(pilotFeature(p.id), p.id).not.toBeNull();
    }
  });

  it("names a marker and a landed key after the same word, and the tab as the core tab did", () => {
    for (const p of pilots) {
      const word = p.id.split(".")[1];
      expect(p.marker).toBe(`mods_autoinstall_${word}`);
      expect(p.landed).toBe(`mods_pilot_landed_${word}`);
      const core = CORE_NAV.find((e) => e.id === pilotCoreTab(p.id));
      if (core) expect(core.label, p.id).toBe(p.tabLabel);
    }
  });

  it("makes the Pinboard's old tab the mod's now that PM's own list has none", () => {
    // PR C2: the core Pinboard tab is gone, so a pin or a hide stored for `pinboard` is the mod's.
    expect(CORE.has("pinboard")).toBe(false);
    expect(renamedNavId("pinboard")).toBe("mod:atlas.pinboard");
    expect(parsePlacement('{"hidden":["pinboard"],"pinned":["pinboard"]}')).toEqual({
      hidden: ["mod:atlas.pinboard"],
      pinned: ["mod:atlas.pinboard"],
    });
    expect(pilotHiddenRuleApplies("atlas.pinboard")).toBe(true);
    // The tab was always shown, so landing the mod hides nothing, at any Depth.
    for (const depth of ["min", "standard", "power"] as const) {
      expect(placementAfterPilotLanded(NONE, "atlas.pinboard", depth)).toBe(NONE);
    }
    const [pending] = modNavEntries([
      { id: "atlas.pinboard", name: "Pinboard", navTab: "Pinboard", pending: true },
    ]);
    expect(pending.defaultShown("min")).toBe(true);
  });
});
