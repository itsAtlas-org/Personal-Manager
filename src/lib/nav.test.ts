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
  navigateTarget,
  navMenuItems,
  navPlacementIsDefault,
  PINBOARD_VIEW,
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
  type NavId,
  type NavPlacement,
} from "./nav";

// The Map was a core tab: hideable, in no group, shown at every Depth but Minimal. A good many rules
// below are about exactly that shape, and no core tab has it now (the Document Map is a mod's tab),
// so the list under test carries a stand-in with the same shape rather than the rules losing their
// subject. It is only ever in these lists; `CORE_NAV` itself is checked on its own where it matters.
const SPARE = "spare" as unknown as NavId;
const SPARE_TAB: NavEntryDef = {
  id: SPARE,
  label: "Spare",
  source: "core",
  helpId: "nav-teach",
  hideable: true,
  defaultShown: (depth) => depth !== "min",
};
const WITH_SPARE: readonly NavEntryDef[] = CORE_NAV.flatMap((e) =>
  e.id === "mods" ? [SPARE_TAB, e] : [e],
);

const NONE: NavPlacement = { hidden: [], pinned: [] };
const STANDARD: NavContext = { depth: "standard", devMode: false };
const MINIMAL: NavContext = { depth: "min", devMode: false };
const ids = (list: readonly NavEntryDef[]) => list.map((e) => e.id);
const shown = (p: NavPlacement, ctx: NavContext = STANDARD) =>
  ids(resolveNav(WITH_SPARE, p, ctx).shown);

// The hand-written sidebar this replaced, in its order, with its gates, then the Mods tab (spec
// §1.13), which sits after Teach and before Dev.
const BEFORE = ["focus", "chat", "calendar", "documents", "review", "teach", SPARE, "mods"];

describe("resolveNav: the default", () => {
  it("is the sidebar as it was, for Standard and Power", () => {
    expect(shown(NONE)).toEqual(BEFORE);
    expect(shown(NONE, { depth: "power", devMode: false })).toEqual(BEFORE);
  });

  it("leaves the learning tools and the Map out on Minimal, as teachVisible/mapVisible did", () => {
    const r = resolveNav(WITH_SPARE, NONE, MINIMAL);
    expect(ids(r.shown)).toEqual(["focus", "chat", "calendar", "documents", "mods"]);
    expect(ids(r.dormant)).toEqual(["review", "teach", SPARE]);
    expect(r.hidden).toEqual([]);
  });

  it("adds Dev last, and only in developer mode", () => {
    expect(shown(NONE, { depth: "standard", devMode: true })).toEqual([...BEFORE, "dev"]);
    expect(shown(NONE)).not.toContain("dev");
  });
});

describe("resolveNav: placement", () => {
  it("puts pinned entries first, in pinned order", () => {
    expect(shown({ hidden: [], pinned: ["documents", "calendar"] })).toEqual([
      "documents",
      "calendar",
      "focus",
      "chat",
      "review",
      "teach",
      SPARE,
      "mods",
    ]);
  });

  it("drops hidden entries from the sidebar but keeps them to offer", () => {
    const r = resolveNav(WITH_SPARE, { hidden: [SPARE, "chat"], pinned: [] }, STANDARD);
    expect(ids(r.shown)).toEqual(["focus", "calendar", "documents", "review", "teach", "mods"]);
    expect(ids(r.hidden)).toEqual(["chat", SPARE]);
  });

  it("lets a pin override a Depth default and a hide alike", () => {
    expect(shown({ hidden: [], pinned: [SPARE] }, MINIMAL)[0]).toBe(SPARE);
    // Both lists naming it (only a hand edit can do that): the rule says a pin shows.
    expect(shown({ hidden: [SPARE], pinned: [SPARE] })[0]).toBe(SPARE);
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
    const r = resolveNav(WITH_SPARE, { hidden: [], pinned: ["dev"] }, STANDARD);
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
    expect(ids(resolveNav([...WITH_SPARE, mod], NONE, STANDARD).shown)).toEqual([
      ...BEFORE,
      "mod:atlas.map",
    ]);
  });
});

describe("the learning tools", () => {
  it("pin, hide and show as one", () => {
    const pinned = pinEntry(NONE, "teach", WITH_SPARE);
    expect(pinned.pinned).toEqual(["review", "teach"]);
    const hidden = hideEntry(pinned, "review", WITH_SPARE);
    expect(hidden).toEqual({ hidden: ["review", "teach"], pinned: [] });
    expect(showEntry(hidden, "teach", WITH_SPARE, STANDARD)).toEqual(NONE);
    expect(unpinEntry(pinned, "review", WITH_SPARE)).toEqual(NONE);
  });

  it("keep teachVisible's meaning: shown at Standard, gone when hidden or on Minimal", () => {
    expect(learningToolsShown(resolveNav(WITH_SPARE, NONE, STANDARD))).toBe(true);
    expect(learningToolsShown(resolveNav(WITH_SPARE, NONE, MINIMAL))).toBe(false);
    const hidden = hideEntry(NONE, "teach", WITH_SPARE);
    expect(learningToolsShown(resolveNav(WITH_SPARE, hidden, STANDARD))).toBe(false);
    const shownOnMinimal = showEntry(NONE, "review", WITH_SPARE, MINIMAL);
    expect(learningToolsShown(resolveNav(WITH_SPARE, shownOnMinimal, MINIMAL))).toBe(true);
  });
});

describe("placement changes", () => {
  it("pins after what's already pinned, and pinning un-hides", () => {
    const p = pinEntry({ hidden: [SPARE], pinned: ["calendar"] }, SPARE, WITH_SPARE);
    expect(p).toEqual({ hidden: [], pinned: ["calendar", SPARE] });
    // Pinning again changes nothing.
    expect(pinEntry(p, SPARE, WITH_SPARE)).toEqual(p);
  });

  it("unpins on hide, or the pin would keep it showing", () => {
    expect(hideEntry({ hidden: [], pinned: [SPARE] }, SPARE, WITH_SPARE)).toEqual({
      hidden: [SPARE],
      pinned: [],
    });
  });

  it("refuses to hide what can't hide, and leaves unknown ids alone", () => {
    expect(hideEntry(NONE, "focus", WITH_SPARE)).toBe(NONE);
    expect(hideEntry(NONE, "mod:atlas.gone", WITH_SPARE)).toBe(NONE);
    expect(pinEntry(NONE, "nope", WITH_SPARE)).toBe(NONE);
  });

  it("shows by un-hiding where the Depth shows it, and by pinning where it doesn't", () => {
    expect(showEntry({ hidden: [SPARE], pinned: [] }, SPARE, WITH_SPARE, STANDARD)).toEqual(NONE);
    expect(showEntry({ hidden: [SPARE], pinned: [] }, SPARE, WITH_SPARE, MINIMAL)).toEqual({
      hidden: [],
      pinned: [SPARE],
    });
  });

  it("keeps ids it doesn't know, for a mod that may come back", () => {
    const p = { hidden: ["mod:atlas.gone"], pinned: [] };
    expect(hideEntry(p, SPARE, WITH_SPARE).hidden).toEqual(["mod:atlas.gone", SPARE]);
  });
});

describe("isReachable and isActiveEntry", () => {
  it("keeps a hidden tab reachable, and drops one that's gone", () => {
    const hidden = resolveNav(WITH_SPARE, { hidden: [SPARE], pinned: [] }, STANDARD);
    expect(isReachable(SPARE, WITH_SPARE, hidden, true)).toBe(true);
    expect(isReachable("teach", WITH_SPARE, resolveNav(WITH_SPARE, NONE, MINIMAL), true)).toBe(
      false,
    );
    expect(isReachable("dev", WITH_SPARE, resolveNav(WITH_SPARE, NONE, STANDARD), true)).toBe(
      false,
    );
    expect(
      isReachable(
        "dev",
        WITH_SPARE,
        resolveNav(WITH_SPARE, NONE, { depth: "min", devMode: true }),
        true,
      ),
    ).toBe(true);
  });

  it("always allows a view with no entry, and never a mod tab that isn't there", () => {
    const r = resolveNav(WITH_SPARE, NONE, MINIMAL);
    expect(isReachable("project", WITH_SPARE, r, true)).toBe(true);
    expect(isReachable("mod:atlas.map", WITH_SPARE, r, true)).toBe(false);
  });

  it("keeps a mod's tab while the mods haven't been read, but never one of PM's own that is gone", () => {
    // The store re-opening empties the rows until the new read lands; the tab is unknown, not gone.
    const r = resolveNav(WITH_SPARE, NONE, MINIMAL);
    expect(isReachable("mod:atlas.map", WITH_SPARE, r, false)).toBe(true);
    expect(isReachable("teach", WITH_SPARE, r, false)).toBe(false);
  });

  it("lights Focus for an open project", () => {
    const focus = WITH_SPARE.find((e) => e.id === "focus")!;
    expect(isActiveEntry(focus, "project")).toBe(true);
    expect(isActiveEntry(focus, "chat")).toBe(false);
  });
});

describe("the Pinboard's tab since it became a mod", () => {
  const PINBOARD_ROW = { id: "atlas.pinboard", name: "Pinboard", navTab: "Pinboard" };

  it("is a mod tab, and PM has no tab of its own by that old name", () => {
    expect(PINBOARD_VIEW).toBe("mod:atlas.pinboard");
    expect(CORE_NAV.map((e) => e.id as string)).not.toContain("pinboard");
  });

  it("can be opened from a Calendar dot whether its row is a pending placeholder or a landed one", () => {
    // `pending` is the placeholder for a mod PM is still installing. A paused or a waiting-for-you
    // mod is a landed row with the same `navTab` (its panel says why); an entry takes no standing,
    // so the placeholder, `pending: false` and a row with no `pending` field cover the shapes.
    for (const pending of [true, false, undefined]) {
      const entries = [...CORE_NAV, ...modNavEntries([{ ...PINBOARD_ROW, pending }])];
      const resolved = resolveNav(entries, NONE, STANDARD);
      expect(isReachable(PINBOARD_VIEW, entries, resolved, true), String(pending)).toBe(true);
    }
  });

  it("sends a Calendar dot to Focus when there is no row at all (a new user who never installed it)", () => {
    const resolved = resolveNav(CORE_NAV, NONE, STANDARD);
    expect(isReachable(PINBOARD_VIEW, CORE_NAV, resolved, true)).toBe(false);
  });
});

describe("navigateTarget: what the main window does with the pop-out's link", () => {
  it("opens the Calendar, a named project and a mod's tab", () => {
    expect(navigateTarget("calendar")).toEqual({ kind: "view", view: "calendar" });
    expect(navigateTarget("project", "Atlas")).toEqual({ kind: "project", project: "Atlas" });
    expect(navigateTarget("mod:atlas.pinboard")).toEqual({
      kind: "view",
      view: "mod:atlas.pinboard",
    });
  });

  it("reads the old `pinboard` view as the Pinboard's tab", () => {
    expect(navigateTarget("pinboard")).toEqual({ kind: "view", view: PINBOARD_VIEW });
  });

  it("refuses a project with no name, a bad mod id, a path and any other view", () => {
    expect(navigateTarget("project")).toBeNull();
    expect(navigateTarget("project", "")).toBeNull();
    expect(navigateTarget("mod:Atlas.pinboard")).toBeNull();
    expect(navigateTarget("mod:atlas")).toBeNull();
    expect(navigateTarget("mod:atlas.pinboard/../x")).toBeNull();
    expect(navigateTarget("/etc/passwd")).toBeNull();
    // Views that exist but are not the pop-out's to open (Dev is gated, Mods is a core route).
    for (const view of ["dev", "mods", "documents", "focus", "", "Calendar", "graph"]) {
      expect(navigateTarget(view), view).toBeNull();
    }
  });
});

describe("the Mods tab", () => {
  it("is always shown: at every Depth, whatever is stored, and it can't be hidden", () => {
    expect(shown(NONE)).toContain("mods");
    expect(shown(NONE, MINIMAL)).toContain("mods");
    expect(shown({ hidden: ["mods"], pinned: [] })).toContain("mods");
    expect(WITH_SPARE.find((e) => e.id === "mods")?.hideable).toBe(false);
    expect(hideEntry(NONE, "mods", WITH_SPARE)).toEqual(NONE);
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
    expect(ids(resolveNav([...WITH_SPARE, ...entries], NONE, STANDARD).shown)).toEqual([
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
    const entries = [...WITH_SPARE, ...modNavEntries([row("acme.tool", "Acme Tool")])];
    const hidden = resolveNav(entries, { hidden: ["mod:acme.tool"], pinned: [] }, STANDARD);
    expect(ids(hidden.hidden)).toEqual(["mod:acme.tool"]);
    expect(isReachable("mod:acme.tool", entries, hidden, true)).toBe(true);
    // Uninstalled (no row, so no entry): back to Focus.
    const gone = resolveNav(WITH_SPARE, NONE, STANDARD);
    expect(isReachable("mod:acme.tool", WITH_SPARE, gone, true)).toBe(false);
  });

  it("pins, hides and shows like a core tab", () => {
    const entries = [...WITH_SPARE, ...modNavEntries([row("acme.tool", "Acme Tool")])];
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
    const p = { hidden: [SPARE], pinned: [] };
    expect(labels(navMenuItems("calendar", WITH_SPARE, p, STANDARD, () => {}))).toEqual([
      "Pin to the top",
      "Hide",
      "—",
      "Show Spare",
    ]);
  });

  it("quotes and marks a mod's tab, so a name like PM's own can't pass for it", () => {
    // A mod may call its tab "Settings" or "Review"; the item then acts on the mod's tab, not PM's.
    const entries = [
      ...WITH_SPARE,
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
    expect(labels(navMenuItems("review", WITH_SPARE, NONE, STANDARD, () => {}))).toEqual([
      "Pin Review and Teach to the top",
      "Hide Review and Teach",
    ]);
    expect(labels(navMenuItems("focus", WITH_SPARE, NONE, MINIMAL, () => {}))).toEqual([
      "Pin to the top",
      "—",
      "Show Review and Teach",
      "Show Spare",
    ]);
  });

  it("offers Unpin on a pinned tab, and no Hide where it can't hide", () => {
    const p = { hidden: [], pinned: ["focus"] };
    expect(labels(navMenuItems("focus", WITH_SPARE, p, STANDARD, () => {}))).toEqual(["Unpin"]);
  });

  it("hands the new placement to apply", () => {
    const applied: NavPlacement[] = [];
    const apply = (next: NavPlacement) => void applied.push(next);
    pick(navMenuItems(SPARE, WITH_SPARE, NONE, STANDARD, apply), "Hide").onSelect();
    expect(applied[0]).toEqual({ hidden: [SPARE], pinned: [] });
    // From the empty part of the list, on Minimal: showing the hidden Map pins it.
    pick(navMenuItems(null, WITH_SPARE, applied[0], MINIMAL, apply), "Show Spare").onSelect();
    expect(applied[1]).toEqual({ hidden: [], pinned: [SPARE] });
  });

  it("has nothing for the empty part of the list when every tab is there", () => {
    expect(navMenuItems(null, WITH_SPARE, NONE, STANDARD, () => {})).toEqual([]);
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
    for (const raw of [null, "", "{", "[]", "null", '"x"', '{"hidden":"review"}']) {
      expect(parsePlacement(raw), String(raw)).toEqual(NONE);
    }
    expect(
      parsePlacement(
        '{"hidden":["review",3,"project","nope","review"],"pinned":["mod:atlas.map"]}',
      ),
    ).toEqual({ hidden: ["review"], pinned: ["mod:atlas.map"] });
  });

  it("forgets an uninstalled mod's tab, and nothing else", () => {
    writeNavPlacement({
      hidden: ["review", "mod:acme.tool"],
      pinned: ["mod:acme.tool", "mod:acme.other", "documents"],
    });
    forgetNavTab("mod:acme.tool");
    expect(readNavPlacement("standard")).toEqual({
      hidden: ["review"],
      pinned: ["mod:acme.other", "documents"],
    });
    // Nothing stored, or nothing about it: nothing is written.
    localStorage.clear();
    forgetNavTab("mod:acme.tool");
    expect(localStorage.getItem(NAV_PLACEMENT_KEY)).toBeNull();
    writeNavPlacement({ hidden: ["review"], pinned: [] });
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
    writeNavPlacement({ hidden: ["review"], pinned: ["documents"] });
    const read = readNavPlacement("standard");
    expect(read).toEqual({ hidden: ["review"], pinned: ["documents"] });
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
    writeNavPlacement({ hidden: ["review"], pinned: [] });
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
