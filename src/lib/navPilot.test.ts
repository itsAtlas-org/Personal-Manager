// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The sidebar's half of a core tab becoming a mod's (the Map, PR B plan B8): a placement stored for
// `graph` means `mod:atlas.map` once `graph` is no longer one of PM's own tabs, and not before; and
// the one-time "installed hidden" rule. Both are keyed on that fact. The core Map tab is gone now
// (Mods PR B2), so the real list is the "after" state; the "before" state is tested by passing the
// set of ids that still had `graph`.

import { describe, expect, it } from "vitest";
import {
  adoptLegacy,
  CORE_NAV,
  isNavIdShape,
  modNavEntries,
  parsePlacement,
  pilotFeature,
  pilotHiddenRuleApplies,
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
    expect(parsePlacement('{"hidden":["graph"],"pinned":["pinboard"]}', WITH_GRAPH)).toEqual({
      hidden: ["graph"],
      pinned: ["pinboard"],
    });
    // A pin the user made on the mod's tab ahead of time is kept as a mod tab, as before.
    expect(parsePlacement('{"hidden":[],"pinned":["mod:atlas.map"]}', WITH_GRAPH).pinned).toEqual([
      MAP,
    ]);
  });

  it("is the Map mod's tab in PM as it ships: the core list has no `graph`", () => {
    expect(CORE.has("graph")).toBe(false);
    expect(renamedNavId("graph")).toBe(MAP);
    expect(parsePlacement('{"hidden":["graph"],"pinned":["pinboard"]}')).toEqual({
      hidden: [MAP],
      pinned: ["pinboard"],
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
      for (const id of ["focus", "pinboard", "mods", "mod:acme.tool", "mod:atlas.pinboard"]) {
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
    const start: NavPlacement = { hidden: ["teach"], pinned: ["pinboard"] };
    expect(placementAfterPilotLanded(start, "atlas.map", "min", WITHOUT_GRAPH)).toEqual({
      hidden: ["teach", MAP],
      pinned: ["pinboard"],
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
