// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The sidebar's half of a core tab becoming a mod's (the Map, PR B plan B8): a placement stored for
// `graph` means `mod:atlas.map` the day `graph` is no longer one of PM's own tabs, and not before; and
// the one-time "installed hidden" rule. Both are keyed on that fact, so PM's removal of the core entry
// is what switches them on, and both are tested in either state.

import { describe, expect, it } from "vitest";
import {
  CORE_NAV,
  isNavIdShape,
  modNavEntries,
  parsePlacement,
  pilotHiddenRuleApplies,
  placementAfterPilotLanded,
  renamedNavId,
  type NavPlacement,
} from "./nav";

const CORE = new Set(CORE_NAV.map((e) => e.id as string));
/** PM's own tabs once the core Map is gone. */
const WITHOUT_GRAPH = new Set([...CORE].filter((id) => id !== "graph"));
const MAP = "mod:atlas.map";
const NONE: NavPlacement = { hidden: [], pinned: [] };

describe("a stored `graph` placement", () => {
  it("is the core Map tab for as long as it exists: nothing is renamed", () => {
    expect(CORE.has("graph")).toBe(true);
    expect(renamedNavId("graph")).toBe("graph");
    expect(renamedNavId("graph", CORE)).toBe("graph");
    expect(parsePlacement('{"hidden":["graph"],"pinned":["pinboard"]}')).toEqual({
      hidden: ["graph"],
      pinned: ["pinboard"],
    });
    // A pin the user made on the mod's tab ahead of time is kept as a mod tab, as before.
    expect(parsePlacement('{"hidden":[],"pinned":["mod:atlas.map"]}').pinned).toEqual([MAP]);
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
    expect(isNavIdShape("graph", CORE)).toBe(true);
    expect(isNavIdShape(MAP, WITHOUT_GRAPH)).toBe(true);
  });

  it("doesn't list the same tab twice when both spellings were stored", () => {
    expect(
      parsePlacement('{"hidden":["graph","mod:atlas.map","graph"],"pinned":[]}', WITHOUT_GRAPH)
        .hidden,
    ).toEqual([MAP]);
  });

  it("leaves every other id alone, whichever state PM is in", () => {
    for (const core of [CORE, WITHOUT_GRAPH]) {
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
  it("does nothing while the core Map tab still exists", () => {
    expect(pilotHiddenRuleApplies("atlas.map")).toBe(false);
    expect(pilotHiddenRuleApplies("atlas.map", CORE)).toBe(false);
    expect(placementAfterPilotLanded(NONE, "atlas.map", "min")).toBe(NONE);
    expect(placementAfterPilotLanded(NONE, "atlas.map", "min", CORE)).toBe(NONE);
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
});
