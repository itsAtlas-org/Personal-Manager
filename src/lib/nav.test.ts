// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// lib/nav: the one sidebar list (Mods spec §1.17). What these pin is the rule — pinned first in
// pinned order, then what isn't hidden and shows by default at this Depth — and the promises that
// rest on it: an untouched install looks exactly as it did before the list was data, a hidden tab
// stays reachable, the learning tools move as one, and nothing a stored value says can show a tab
// whose capability is off or hide one that must always be there.

import { beforeEach, describe, expect, it } from "vitest";
import { ACUTE, MARK_ABOVE } from "../components/mods/modFixtures";
import type { MenuItem } from "./contextMenu";
import { HELP } from "./help";
import {
  adoptLegacy,
  CORE_NAV,
  forgetNavTab,
  hideEntry,
  isActiveEntry,
  isNavIdShape,
  isReachable,
  learningToolsShown,
  LEGACY_MAP_KEY,
  LEGACY_TEACH_KEY,
  modNavEntries,
  modTabLabel,
  MOD_TAB_HELP_ID,
  MOD_TAB_LABEL_MAX,
  NAV_PLACEMENT_KEY,
  navMenuItems,
  navPlacementIsDefault,
  parsePlacement,
  pinEntry,
  readNavPlacement,
  resetNavPlacement,
  resolveNav,
  showEntry,
  unpinEntry,
  writeNavPlacement,
  type NavContext,
  type NavEntryDef,
  type NavPlacement,
} from "./nav";

const NONE: NavPlacement = { hidden: [], pinned: [] };
const STANDARD: NavContext = { depth: "standard", devMode: false };
const MINIMAL: NavContext = { depth: "min", devMode: false };
const ids = (list: readonly NavEntryDef[]) => list.map((e) => e.id);
const shown = (p: NavPlacement, ctx: NavContext = STANDARD) =>
  ids(resolveNav(CORE_NAV, p, ctx).shown);

// The hand-written sidebar this replaced, in its order, with its gates, then the Mods tab (spec
// §1.13), which sits after Pinboard and before Dev.
const BEFORE = [
  "focus",
  "chat",
  "calendar",
  "documents",
  "review",
  "teach",
  "graph",
  "pinboard",
  "mods",
];

describe("resolveNav: the default", () => {
  it("is the sidebar as it was, for Standard and Power", () => {
    expect(shown(NONE)).toEqual(BEFORE);
    expect(shown(NONE, { depth: "power", devMode: false })).toEqual(BEFORE);
  });

  it("leaves the learning tools and the Map out on Minimal, as teachVisible/mapVisible did", () => {
    const r = resolveNav(CORE_NAV, NONE, MINIMAL);
    expect(ids(r.shown)).toEqual(["focus", "chat", "calendar", "documents", "pinboard", "mods"]);
    expect(ids(r.dormant)).toEqual(["review", "teach", "graph"]);
    expect(r.hidden).toEqual([]);
  });

  it("adds Dev last, and only in developer mode", () => {
    expect(shown(NONE, { depth: "standard", devMode: true })).toEqual([...BEFORE, "dev"]);
    expect(shown(NONE)).not.toContain("dev");
  });
});

describe("resolveNav: placement", () => {
  it("puts pinned entries first, in pinned order", () => {
    expect(shown({ hidden: [], pinned: ["pinboard", "calendar"] })).toEqual([
      "pinboard",
      "calendar",
      "focus",
      "chat",
      "documents",
      "review",
      "teach",
      "graph",
      "mods",
    ]);
  });

  it("drops hidden entries from the sidebar but keeps them to offer", () => {
    const r = resolveNav(CORE_NAV, { hidden: ["graph", "chat"], pinned: [] }, STANDARD);
    expect(ids(r.shown)).toEqual([
      "focus",
      "calendar",
      "documents",
      "review",
      "teach",
      "pinboard",
      "mods",
    ]);
    expect(ids(r.hidden)).toEqual(["chat", "graph"]);
  });

  it("lets a pin override a Depth default and a hide alike", () => {
    expect(shown({ hidden: [], pinned: ["graph"] }, MINIMAL)[0]).toBe("graph");
    // Both lists naming it (only a hand edit can do that): the rule says a pin shows.
    expect(shown({ hidden: ["graph"], pinned: ["graph"] })[0]).toBe("graph");
  });

  it("ignores ids it doesn't know, and repeats", () => {
    const p = {
      hidden: ["mod:atlas.gone", "nope"],
      pinned: ["mod:atlas.gone", "calendar", "calendar"],
    };
    expect(shown(p)).toEqual(["calendar", ...BEFORE.filter((id) => id !== "calendar")]);
  });

  it("never hides an entry that can't hide, whatever is stored", () => {
    expect(shown({ hidden: ["focus"], pinned: [] })[0]).toBe("focus");
    expect(shown({ hidden: ["dev"], pinned: [] }, { depth: "standard", devMode: true })).toContain(
      "dev",
    );
  });

  it("never shows an entry whose capability is off, even pinned", () => {
    const r = resolveNav(CORE_NAV, { hidden: [], pinned: ["dev"] }, STANDARD);
    expect([...r.shown, ...r.hidden, ...r.dormant].map((e) => e.id)).not.toContain("dev");
  });

  it("appends a mod's entries after PM's own", () => {
    const mod: NavEntryDef = {
      id: "mod:atlas.map",
      label: "Map",
      source: "mod",
      helpId: "mod-atlas.map",
      hideable: true,
      defaultShown: () => true,
    };
    expect(ids(resolveNav([...CORE_NAV, mod], NONE, STANDARD).shown)).toEqual([
      ...BEFORE,
      "mod:atlas.map",
    ]);
  });
});

describe("the learning tools", () => {
  it("pin, hide and show as one", () => {
    const pinned = pinEntry(NONE, "teach", CORE_NAV);
    expect(pinned.pinned).toEqual(["review", "teach"]);
    const hidden = hideEntry(pinned, "review", CORE_NAV);
    expect(hidden).toEqual({ hidden: ["review", "teach"], pinned: [] });
    expect(showEntry(hidden, "teach", CORE_NAV, STANDARD)).toEqual(NONE);
    expect(unpinEntry(pinned, "review", CORE_NAV)).toEqual(NONE);
  });

  it("keep teachVisible's meaning: shown at Standard, gone when hidden or on Minimal", () => {
    expect(learningToolsShown(resolveNav(CORE_NAV, NONE, STANDARD))).toBe(true);
    expect(learningToolsShown(resolveNav(CORE_NAV, NONE, MINIMAL))).toBe(false);
    const hidden = hideEntry(NONE, "teach", CORE_NAV);
    expect(learningToolsShown(resolveNav(CORE_NAV, hidden, STANDARD))).toBe(false);
    const shownOnMinimal = showEntry(NONE, "review", CORE_NAV, MINIMAL);
    expect(learningToolsShown(resolveNav(CORE_NAV, shownOnMinimal, MINIMAL))).toBe(true);
  });
});

describe("placement changes", () => {
  it("pins after what's already pinned, and pinning un-hides", () => {
    const p = pinEntry({ hidden: ["graph"], pinned: ["calendar"] }, "graph", CORE_NAV);
    expect(p).toEqual({ hidden: [], pinned: ["calendar", "graph"] });
    // Pinning again changes nothing.
    expect(pinEntry(p, "graph", CORE_NAV)).toEqual(p);
  });

  it("unpins on hide, or the pin would keep it showing", () => {
    expect(hideEntry({ hidden: [], pinned: ["graph"] }, "graph", CORE_NAV)).toEqual({
      hidden: ["graph"],
      pinned: [],
    });
  });

  it("refuses to hide what can't hide, and leaves unknown ids alone", () => {
    expect(hideEntry(NONE, "focus", CORE_NAV)).toBe(NONE);
    expect(hideEntry(NONE, "mod:atlas.gone", CORE_NAV)).toBe(NONE);
    expect(pinEntry(NONE, "nope", CORE_NAV)).toBe(NONE);
  });

  it("shows by un-hiding where the Depth shows it, and by pinning where it doesn't", () => {
    expect(showEntry({ hidden: ["graph"], pinned: [] }, "graph", CORE_NAV, STANDARD)).toEqual(NONE);
    expect(showEntry({ hidden: ["graph"], pinned: [] }, "graph", CORE_NAV, MINIMAL)).toEqual({
      hidden: [],
      pinned: ["graph"],
    });
  });

  it("keeps ids it doesn't know, for a mod that may come back", () => {
    const p = { hidden: ["mod:atlas.gone"], pinned: [] };
    expect(hideEntry(p, "graph", CORE_NAV).hidden).toEqual(["mod:atlas.gone", "graph"]);
  });
});

describe("isReachable and isActiveEntry", () => {
  it("keeps a hidden tab reachable, and drops one that's gone", () => {
    const hidden = resolveNav(CORE_NAV, { hidden: ["graph"], pinned: [] }, STANDARD);
    expect(isReachable("graph", CORE_NAV, hidden, true)).toBe(true);
    expect(isReachable("teach", CORE_NAV, resolveNav(CORE_NAV, NONE, MINIMAL), true)).toBe(false);
    expect(isReachable("dev", CORE_NAV, resolveNav(CORE_NAV, NONE, STANDARD), true)).toBe(false);
    expect(
      isReachable(
        "dev",
        CORE_NAV,
        resolveNav(CORE_NAV, NONE, { depth: "min", devMode: true }),
        true,
      ),
    ).toBe(true);
  });

  it("always allows a view with no entry, and never a mod tab that isn't there", () => {
    const r = resolveNav(CORE_NAV, NONE, MINIMAL);
    expect(isReachable("project", CORE_NAV, r, true)).toBe(true);
    expect(isReachable("mod:atlas.map", CORE_NAV, r, true)).toBe(false);
  });

  it("keeps a mod's tab while the mods haven't been read, but never one of PM's own that is gone", () => {
    // The store re-opening empties the rows until the new read lands; the tab is unknown, not gone.
    const r = resolveNav(CORE_NAV, NONE, MINIMAL);
    expect(isReachable("mod:atlas.map", CORE_NAV, r, false)).toBe(true);
    expect(isReachable("teach", CORE_NAV, r, false)).toBe(false);
  });

  it("lights Focus for an open project", () => {
    const focus = CORE_NAV.find((e) => e.id === "focus")!;
    expect(isActiveEntry(focus, "project")).toBe(true);
    expect(isActiveEntry(focus, "chat")).toBe(false);
  });
});

describe("the Mods tab", () => {
  it("is always shown: at every Depth, whatever is stored, and it can't be hidden", () => {
    expect(shown(NONE)).toContain("mods");
    expect(shown(NONE, MINIMAL)).toContain("mods");
    expect(shown({ hidden: ["mods"], pinned: [] })).toContain("mods");
    expect(CORE_NAV.find((e) => e.id === "mods")?.hideable).toBe(false);
    expect(hideEntry(NONE, "mods", CORE_NAV)).toEqual(NONE);
  });

  it("can be pinned, like any tab", () => {
    expect(shown({ hidden: [], pinned: ["mods"] })[0]).toBe("mods");
  });
});

describe("mods' own tabs", () => {
  const row = (id: string, navTab: string | null, name = "A Mod") => ({ id, name, navTab });

  it("gives each installed mod that names a tab an entry, after PM's own", () => {
    const entries = modNavEntries([row("acme.tool", "Acme Tool"), row("acme.quiet", null)]);
    expect(entries).toHaveLength(1);
    expect(entries[0]).toMatchObject({
      id: "mod:acme.tool",
      label: "Acme Tool",
      source: "mod",
      hideable: true,
    });
    expect(entries[0].defaultShown("min")).toBe(true);
    expect(ids(resolveNav([...CORE_NAV, ...entries], NONE, STANDARD).shown)).toEqual([
      ...BEFORE,
      "mod:acme.tool",
    ]);
  });

  it("points every one at the one help entry that exists", () => {
    for (const entry of modNavEntries([row("acme.a", "A"), row("acme.b", "B")])) {
      expect(entry.helpId).toBe(MOD_TAB_HELP_ID);
      expect(HELP[entry.helpId]).toBeDefined();
    }
  });

  it("keeps a steady order (by label, then id) and one entry per mod", () => {
    const entries = modNavEntries([
      row("acme.zed", "Beta"),
      row("acme.one", "Alpha"),
      row("acme.dup", "Alpha"),
      row("acme.dup", "Other"),
    ]);
    expect(ids(entries)).toEqual(["mod:acme.dup", "mod:acme.one", "mod:acme.zed"]);
  });

  it("leaves out an id the grammar wouldn't allow, which couldn't be pinned or hidden", () => {
    expect(modNavEntries([row("../x", "X"), row("Acme.Tool", "X"), row("nodot", "X")])).toEqual([]);
  });

  it("cleans the name a mod chose: control and format characters go, spaces collapse", () => {
    expect(modTabLabel("  Acme‮  Tool\n\t2 ", "x")).toBe("Acme Tool 2");
    expect(modTabLabel("​\u0007", "Fallback Name")).toBe("Fallback Name");
    expect(modTabLabel(null, "  Only name ")).toBe("Only name");
    expect(modTabLabel("​", "‮")).toBe("Mod");
  });

  it("holds the name to one line's length, by characters rather than code units", () => {
    const long = "x".repeat(MOD_TAB_LABEL_MAX + 10);
    const cut = modTabLabel(long, "n");
    expect(Array.from(cut)).toHaveLength(MOD_TAB_LABEL_MAX);
    expect(cut.endsWith("…")).toBe(true);
    // A name exactly at the limit is untouched, and an emoji counts as one character.
    expect(modTabLabel("y".repeat(MOD_TAB_LABEL_MAX), "n")).toBe("y".repeat(MOD_TAB_LABEL_MAX));
    const emoji = "😀".repeat(MOD_TAB_LABEL_MAX);
    expect(modTabLabel(emoji, "n")).toBe(emoji);
  });

  it("cuts a run of stacked combining marks, which would paint over the row beside the tab", () => {
    const stacked = `A${MARK_ABOVE.repeat(39)}`;
    expect(modTabLabel(stacked, "n")).toBe(`A${MARK_ABOVE.repeat(3)}`);
    // Ordinary accents, and a letter with a few of them, are left alone.
    expect(modTabLabel(`e${ACUTE}cole`, "n")).toBe(`e${ACUTE}cole`);
  });

  it("keeps a hidden mod tab reachable, and drops one that is gone", () => {
    const entries = [...CORE_NAV, ...modNavEntries([row("acme.tool", "Acme Tool")])];
    const hidden = resolveNav(entries, { hidden: ["mod:acme.tool"], pinned: [] }, STANDARD);
    expect(ids(hidden.hidden)).toEqual(["mod:acme.tool"]);
    expect(isReachable("mod:acme.tool", entries, hidden, true)).toBe(true);
    // Uninstalled (no row, so no entry): back to Focus.
    const gone = resolveNav(CORE_NAV, NONE, STANDARD);
    expect(isReachable("mod:acme.tool", CORE_NAV, gone, true)).toBe(false);
  });

  it("pins, hides and shows like a core tab", () => {
    const entries = [...CORE_NAV, ...modNavEntries([row("acme.tool", "Acme Tool")])];
    const pinned = pinEntry(NONE, "mod:acme.tool", entries);
    expect(pinned.pinned).toEqual(["mod:acme.tool"]);
    const hidden = hideEntry(pinned, "mod:acme.tool", entries);
    expect(hidden).toEqual({ hidden: ["mod:acme.tool"], pinned: [] });
    expect(showEntry(hidden, "mod:acme.tool", entries, MINIMAL)).toEqual(NONE);
  });
});

describe("navMenuItems", () => {
  const labels = (items: MenuItem[]) =>
    items.map((i) => (i.kind === "separator" ? "—" : "label" in i ? i.label : "?"));
  const pick = (items: MenuItem[], label: string) => {
    const item = items.find((i) => i.kind !== "separator" && "label" in i && i.label === label);
    if (!item || item.kind === "separator" || item.kind === "swatches") throw new Error(label);
    return item;
  };

  it("offers Pin and Hide on a tab, and Show for each tab that isn't there", () => {
    const p = { hidden: ["graph"], pinned: [] };
    expect(labels(navMenuItems("calendar", CORE_NAV, p, STANDARD, () => {}))).toEqual([
      "Pin to the top",
      "Hide",
      "—",
      "Show Map",
    ]);
  });

  it("quotes and marks a mod's tab, so a name like PM's own can't pass for it", () => {
    // A mod may call its tab "Settings" or "Review"; the item then acts on the mod's tab, not PM's.
    const entries = [
      ...CORE_NAV,
      ...modNavEntries([{ id: "acme.tool", name: "x", navTab: "Review" }]),
    ];
    // Pin and Hide act on the tab under the pointer and name nothing; Show names what comes back.
    const items = navMenuItems("mod:acme.tool", entries, NONE, STANDARD, () => {});
    expect(labels(items)).toEqual(["Pin to the top", "Hide"]);
    const hidden = { hidden: ["mod:acme.tool"], pinned: [] };
    expect(labels(navMenuItems(null, entries, hidden, STANDARD, () => {}))).toContain(
      "Show “Review” (mod)",
    );
    // PM's own tabs are named as they always were.
    expect(labels(navMenuItems("review", entries, NONE, STANDARD, () => {}))).toEqual([
      "Pin Review and Teach to the top",
      "Hide Review and Teach",
    ]);
  });

  it("names both learning tools, and offers them back as one line", () => {
    expect(labels(navMenuItems("review", CORE_NAV, NONE, STANDARD, () => {}))).toEqual([
      "Pin Review and Teach to the top",
      "Hide Review and Teach",
    ]);
    expect(labels(navMenuItems("focus", CORE_NAV, NONE, MINIMAL, () => {}))).toEqual([
      "Pin to the top",
      "—",
      "Show Review and Teach",
      "Show Map",
    ]);
  });

  it("offers Unpin on a pinned tab, and no Hide where it can't hide", () => {
    const p = { hidden: [], pinned: ["focus"] };
    expect(labels(navMenuItems("focus", CORE_NAV, p, STANDARD, () => {}))).toEqual(["Unpin"]);
  });

  it("hands the new placement to apply", () => {
    const applied: NavPlacement[] = [];
    const apply = (next: NavPlacement) => void applied.push(next);
    pick(navMenuItems("graph", CORE_NAV, NONE, STANDARD, apply), "Hide").onSelect();
    expect(applied[0]).toEqual({ hidden: ["graph"], pinned: [] });
    // From the empty part of the list, on Minimal: showing the hidden Map pins it.
    pick(navMenuItems(null, CORE_NAV, applied[0], MINIMAL, apply), "Show Map").onSelect();
    expect(applied[1]).toEqual({ hidden: [], pinned: ["graph"] });
  });

  it("has nothing for the empty part of the list when every tab is there", () => {
    expect(navMenuItems(null, CORE_NAV, NONE, STANDARD, () => {})).toEqual([]);
  });
});

describe("help", () => {
  it("has an entry for every core tab", () => {
    for (const entry of CORE_NAV) expect(HELP[entry.helpId], entry.helpId).toBeDefined();
  });
});

describe("stored placement", () => {
  beforeEach(() => localStorage.clear());

  it("reads defensively: anything malformed is nothing chosen", () => {
    for (const raw of [null, "", "{", "[]", "null", '"x"', '{"hidden":"graph"}']) {
      expect(parsePlacement(raw), String(raw)).toEqual(NONE);
    }
    expect(
      parsePlacement('{"hidden":["graph",3,"project","nope","graph"],"pinned":["mod:atlas.map"]}'),
    ).toEqual({ hidden: ["graph"], pinned: ["mod:atlas.map"] });
  });

  it("forgets an uninstalled mod's tab, and nothing else", () => {
    writeNavPlacement({
      hidden: ["graph", "mod:acme.tool"],
      pinned: ["mod:acme.tool", "mod:acme.other", "pinboard"],
    });
    forgetNavTab("mod:acme.tool");
    expect(readNavPlacement("standard")).toEqual({
      hidden: ["graph"],
      pinned: ["mod:acme.other", "pinboard"],
    });
    // Nothing stored, or nothing about it: nothing is written.
    localStorage.clear();
    forgetNavTab("mod:acme.tool");
    expect(localStorage.getItem(NAV_PLACEMENT_KEY)).toBeNull();
    writeNavPlacement({ hidden: ["graph"], pinned: [] });
    const before = localStorage.getItem(NAV_PLACEMENT_KEY);
    forgetNavTab("mod:acme.tool");
    expect(localStorage.getItem(NAV_PLACEMENT_KEY)).toBe(before);
  });

  it("keeps only strings that could name a tab", () => {
    expect(isNavIdShape("calendar")).toBe(true);
    expect(isNavIdShape("mod:atlas.map")).toBe(true);
    expect(isNavIdShape("project")).toBe(false);
    expect(isNavIdShape("mod:Atlas.map")).toBe(false);
    expect(isNavIdShape("mod:map")).toBe(false);
    expect(isNavIdShape("mod:atlas.map/../x")).toBe(false);
  });

  it("caps what a stored value can hold", () => {
    const many = Array.from({ length: 500 }, (_, i) => `mod:atlas.m${i}`);
    expect(parsePlacement(JSON.stringify({ hidden: many, pinned: [] })).hidden).toHaveLength(64);
  });

  it("writes, reads back and resets", () => {
    expect(navPlacementIsDefault(readNavPlacement("standard"))).toBe(true);
    writeNavPlacement({ hidden: ["graph"], pinned: ["pinboard"] });
    const read = readNavPlacement("standard");
    expect(read).toEqual({ hidden: ["graph"], pinned: ["pinboard"] });
    // The same object while nothing changes: useSyncExternalStore loops on a fresh one.
    expect(readNavPlacement("standard")).toBe(read);
    resetNavPlacement();
    expect(localStorage.getItem(NAV_PLACEMENT_KEY)).toBeNull();
    expect(navPlacementIsDefault(readNavPlacement("standard"))).toBe(true);
  });

  it("tells a still-mounted sidebar about every write", () => {
    let heard = 0;
    const listen = () => heard++;
    window.addEventListener("pm:settings-changed", listen);
    writeNavPlacement({ hidden: ["graph"], pinned: [] });
    resetNavPlacement();
    window.removeEventListener("pm:settings-changed", listen);
    expect(heard).toBe(2);
  });
});

describe("adopting the old Settings switches", () => {
  beforeEach(() => localStorage.clear());

  it("turns a hide into hidden, whatever the Depth", () => {
    expect(adoptLegacy("hide", "hide", "standard")).toEqual({
      hidden: ["review", "teach", "mod:atlas.map"],
      pinned: [],
    });
    expect(adoptLegacy("hide", null, "min").hidden).toEqual(["review", "teach"]);
  });

  it("pins a show only where the Depth would leave the tab out", () => {
    expect(adoptLegacy("show", "show", "standard")).toEqual(NONE);
    expect(adoptLegacy("show", "show", "min")).toEqual({
      hidden: [],
      pinned: ["review", "teach", "mod:atlas.map"],
    });
  });

  it("reads auto, and anything unknown, as the default", () => {
    expect(adoptLegacy("auto", "auto", "min")).toEqual(NONE);
    expect(adoptLegacy("sideways", null, "standard")).toEqual(NONE);
  });

  it("adopts them once, then forgets them, so a reset can't bring them back", () => {
    localStorage.setItem(LEGACY_TEACH_KEY, "hide");
    localStorage.setItem(LEGACY_MAP_KEY, "show");
    expect(readNavPlacement("min")).toEqual({
      hidden: ["review", "teach"],
      pinned: ["mod:atlas.map"],
    });
    expect(localStorage.getItem(LEGACY_TEACH_KEY)).toBeNull();
    expect(localStorage.getItem(LEGACY_MAP_KEY)).toBeNull();
    expect(parsePlacement(localStorage.getItem(NAV_PLACEMENT_KEY))).toEqual({
      hidden: ["review", "teach"],
      pinned: ["mod:atlas.map"],
    });
    resetNavPlacement();
    expect(navPlacementIsDefault(readNavPlacement("min"))).toBe(true);
  });

  it("never adopts over a placement already stored", () => {
    writeNavPlacement({ hidden: [], pinned: ["calendar"] });
    localStorage.setItem(LEGACY_TEACH_KEY, "hide");
    expect(readNavPlacement("standard")).toEqual({ hidden: [], pinned: ["calendar"] });
  });
});
