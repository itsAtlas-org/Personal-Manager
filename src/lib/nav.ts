// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// PM's top-level navigation, in one place (Mods spec §1.17). The list used to be hand-written four
// times, each copy with its own gating: the `View` union and the NavItem block in Sidebar, the
// palette's NAV_DESTS (with the Teach/Map gate copied in), and App's "fall back to Focus" effect.
// Every one of them now reads the entries and the resolver here, so a tab added once (a mod's, in a
// later slice) appears in the sidebar, in Ctrl K and in the fallback together.
//
// PLACEMENT is the user's say over that list: right-click a tab to pin it to the top or hide it.
// It is chrome, not content (a closed set of tab ids), so it lives in this device's localStorage
// like the folds and the Focus panels, never in the encrypted store (storedPrefs says why). Settings
// keeps only its Reset: the controls live on the tabs ("one control, one home").
//
// THE RULE (spec §1.17, verbatim in `resolveNav`): an entry shows when it is pinned, or when it isn't
// hidden and `defaultShown(depth)` is true; pinned entries come first, in pinned order. Two things
// sit outside it on purpose:
//   - `requires` is a capability, not a default. Dev exists only in developer mode, and no pin can
//     bring back a tab whose capability is off.
//   - A HIDDEN tab stays reachable from Ctrl K, but a tab its Depth leaves out by default does not.
//     That is today's Minimal: no Teach, Review or Map anywhere until you ask for them, and asking
//     ("Show" in the right-click menu) pins them, the only thing the rule lets override a default.
//
// Review and Teach are one feature, the "learning tools", and have been since they shared a single
// Settings switch: `group` makes them pin, hide and show together, so hiding either keeps that
// switch's meaning, including the inline review controls (`useLearningToolsVisible`).

import { useMemo, useSyncExternalStore } from "react";
import { useTheme, type Depth } from "../theme";
import { useDevMode } from "./capabilities";
import type { MenuItem } from "./contextMenu";
import { useModRows } from "./modRows";
import { cleanModChars, quoteName } from "./modTextClean";
import type { ModInfo } from "./modsTypes";
import { isTearingDown } from "./teardown";

/** PM's own views. "project" is a view with no sidebar entry: it's reached from Focus (and the
 *  palette), and Focus stays lit while it's open. */
export type CoreView =
  | "focus"
  | "project"
  | "chat"
  | "calendar"
  | "documents"
  | "review"
  | "teach"
  | "pinboard"
  | "mods"
  | "dev";

/** A mod's own tab, `mod:<mod id>`. An installed mod that names a tab contributes one
 *  (`modNavEntries`, from the rows `lib/modRows` reads); App draws it through ModTab, which puts
 *  the mod's frame in it when the mod can run and PM's own panel when it can't. */
export type ModView = `mod:${string}`;

export type View = CoreView | ModView;

/** The views that have a sidebar entry. */
export type NavId = Exclude<CoreView, "project"> | ModView;

export interface NavEntryDef {
  id: NavId;
  label: string;
  /** Who contributed the entry. Mods' entries are appended after the core ones. */
  source: "core" | "mod";
  /** Help mode's registry id. Core ids stay literals (`help.ts` defines each one); a mod's is
   *  computed from its id. */
  helpId: string;
  /** Whether the user may hide it. Focus can't (it's home, and where every fallback lands), nor can
   *  Dev (developer mode is its switch), nor the Mods tab (spec §1.13: it is where a mod is paused,
   *  removed or shown again, so it must always be there). */
  hideable: boolean;
  /** Shown at this Depth when the user has said nothing. */
  defaultShown: (depth: Depth) => boolean;
  /** A capability the entry needs to exist at all. Off means nowhere: sidebar, Ctrl K and menus. */
  requires?: "devMode";
  /** Entries in one group pin, hide and show as one. */
  group?: string;
  /** Other views that light this entry, as an open project lights Focus. */
  alsoActiveOn?: readonly View[];
}

/** The user's placement on this device. Ids PM doesn't know (a mod that isn't installed, a tab a
 *  later build dropped) are kept in storage and ignored by the resolver. */
export interface NavPlacement {
  readonly hidden: readonly string[];
  readonly pinned: readonly string[];
}

export interface NavContext {
  depth: Depth;
  devMode: boolean;
}

export interface ResolvedNav {
  /** In the sidebar, in order. */
  shown: NavEntryDef[];
  /** Hidden by the user: not in the sidebar, still in Ctrl K and the menu's "Show". */
  hidden: NavEntryDef[];
  /** Left out by default at this Depth: only the menu's "Show" (which pins) brings one back. */
  dormant: NavEntryDef[];
}

/** The learning tools' group: Review and Teach. */
export const LEARNING_GROUP = "learning";

const always = () => true;
// The optional reveals: hidden on Minimal, shown on Standard and Power (what `teachVisible` and
// `mapVisible` computed for "auto").
const notMinimal = (depth: Depth) => depth !== "min";

/** PM's own entries, in canonical order (the order the sidebar has always had). */
export const CORE_NAV: readonly NavEntryDef[] = [
  {
    id: "focus",
    label: "Focus",
    source: "core",
    helpId: "nav-focus",
    hideable: false,
    defaultShown: always,
    alsoActiveOn: ["project"],
  },
  {
    id: "chat",
    label: "Chats",
    source: "core",
    helpId: "nav-chat",
    hideable: true,
    defaultShown: always,
  },
  {
    id: "calendar",
    label: "Calendar",
    source: "core",
    helpId: "nav-calendar",
    hideable: true,
    defaultShown: always,
  },
  {
    id: "documents",
    label: "Documents",
    source: "core",
    helpId: "nav-documents",
    hideable: true,
    defaultShown: always,
  },
  {
    id: "review",
    label: "Review",
    source: "core",
    helpId: "nav-review",
    hideable: true,
    defaultShown: notMinimal,
    group: LEARNING_GROUP,
  },
  {
    id: "teach",
    label: "Teach",
    source: "core",
    helpId: "nav-teach",
    hideable: true,
    defaultShown: notMinimal,
    group: LEARNING_GROUP,
  },
  {
    id: "pinboard",
    label: "Pinboard",
    source: "core",
    helpId: "nav-pinboard",
    hideable: true,
    defaultShown: always,
  },
  // The Mods tab (spec §1.13): a core entry, never hideable, shown at every Depth. Mods' own tabs
  // are not listed here; `useNav` appends them (`source: "mod"`, `id: "mod:<id>"`).
  {
    id: "mods",
    label: "Mods",
    source: "core",
    helpId: "nav-mods",
    hideable: false,
    defaultShown: always,
  },
  {
    id: "dev",
    label: "Dev",
    source: "core",
    helpId: "nav-dev",
    hideable: false,
    defaultShown: always,
    requires: "devMode",
  },
];

// --- mods' own tabs (pure) -----------------------------------------------------------------------

/** The most characters a mod's tab name shows: a sidebar row, a menu line and a palette row are all
 *  one line, and the name is the mod's to choose, so PM holds it to a length and a kind of text. */
export const MOD_TAB_LABEL_MAX = 40;

/** One literal help id for every mod's tab: help mode can't see inside a frame, and an id computed
 *  from the mod's id would point at no entry (the registry is static). */
export const MOD_TAB_HELP_ID = "nav-mod-tab";

/** What a mod's tab is called in PM's own chrome. The mod chose the words, so they are treated as
 *  untrusted text: control and format characters go (a right-to-left override could reorder the
 *  whole row), whitespace collapses, and the length is capped. A name that comes out empty falls
 *  back to the mod's own name, cleaned the same way, then to "Mod". */
export function modTabLabel(navTab: string | null, name: string): string {
  for (const raw of [navTab, name]) {
    if (raw === null) continue;
    const clean = cleanModChars(raw).replace(/\s+/g, " ").trim();
    if (clean === "") continue;
    const chars = Array.from(clean);
    return chars.length > MOD_TAB_LABEL_MAX
      ? `${chars.slice(0, MOD_TAB_LABEL_MAX - 1).join("")}…`
      : clean;
  }
  return "Mod";
}

/** The tabs installed mods contribute, in a steady order (by label, then id), after PM's own. A mod
 *  that names no tab contributes none. Every installed mod with a tab has one, whatever its
 *  standing: a paused or waiting mod's tab says so and offers the way back (ModTab). */
export function modNavEntries(
  rows: readonly (Pick<ModInfo, "id" | "name" | "navTab"> & Partial<Pick<ModInfo, "pending">>)[],
): NavEntryDef[] {
  const seen = new Set<string>();
  const out: NavEntryDef[] = [];
  for (const row of rows) {
    // An id the grammar wouldn't allow can't be pinned or hidden (the placement drops it), so it
    // gets no tab at all rather than one that behaves differently from every other.
    if (row.navTab === null || !MOD_ID.test(row.id) || seen.has(row.id)) continue;
    seen.add(row.id);
    out.push({
      id: `mod:${row.id}`,
      label: modTabLabel(row.navTab, row.name),
      source: "mod",
      helpId: MOD_TAB_HELP_ID,
      hideable: true,
      // A placeholder (PM is still installing the mod) shows by the old tab's rule, so a Minimal-Depth
      // user doesn't see the tab flicker in and then hide once the mod lands: the installed-hidden rule
      // (`placementAfterPilotLanded`) runs only after landing. A landed mod's tab is "always".
      defaultShown:
        row.pending === true ? (PILOT_CORE_TAB.get(row.id)?.shown ?? notMinimal) : always,
    });
  }
  return out.sort(
    (a, b) => a.label.localeCompare(b.label) || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0),
  );
}

// --- the resolver (pure) -------------------------------------------------------------------------

function available(entry: NavEntryDef, ctx: NavContext): boolean {
  return entry.requires === undefined || ctx[entry.requires];
}

/** Split `entries` into what the sidebar shows (in order), what the user hid, and what this Depth
 *  leaves out. Entries whose capability is off appear in none of them. */
export function resolveNav(
  entries: readonly NavEntryDef[],
  placement: NavPlacement,
  ctx: NavContext,
): ResolvedNav {
  const usable = entries.filter((e) => available(e, ctx));
  const byId = new Map(usable.map((e) => [e.id as string, e]));
  const pinned: NavEntryDef[] = [];
  for (const id of placement.pinned) {
    const entry = byId.get(id);
    if (entry && !pinned.includes(entry)) pinned.push(entry);
  }
  const hiddenIds = new Set(placement.hidden);
  const rest: NavEntryDef[] = [];
  const hidden: NavEntryDef[] = [];
  const dormant: NavEntryDef[] = [];
  for (const entry of usable) {
    if (pinned.includes(entry)) continue;
    // A stored "hidden" on an entry that can't hide (Focus, say, from a hand-edited value) is
    // ignored rather than obeyed: those entries' whole point is that they're always there.
    if (entry.hideable && hiddenIds.has(entry.id)) hidden.push(entry);
    else if (entry.defaultShown(ctx.depth)) rest.push(entry);
    else dormant.push(entry);
  }
  return { shown: [...pinned, ...rest], hidden, dormant };
}

/** Whether `entry` is the one to light for `view`. */
export function isActiveEntry(entry: NavEntryDef, view: View): boolean {
  return entry.id === view || (entry.alsoActiveOn?.includes(view) ?? false);
}

/** Whether App may stay on `view`. A view with no entry of its own ("project") always may; an
 *  entry's view may while it's shown or hidden, since a hidden tab is still a Ctrl K away. One that
 *  is gone (developer mode off, Minimal's default, a mod that isn't there) sends App to Focus.
 *  `modsKnown` is whether the installed mods have been read: while they haven't (the store has just
 *  re-opened and the list is being read again) a mod's tab is unknown, not gone, so it stays. */
export function isReachable(
  view: View,
  entries: readonly NavEntryDef[],
  resolved: ResolvedNav,
  modsKnown: boolean,
): boolean {
  if (isModView(view) && !modsKnown) return true;
  if (!isModView(view) && !entries.some((e) => e.id === view)) return true;
  return [...resolved.shown, ...resolved.hidden].some((e) => e.id === view);
}

export function isModView(view: View): view is ModView {
  return view.startsWith("mod:");
}

/** Whether `view` is drawn by the mods' code: the Mods tab or a mod's own tab. App sends exactly
 *  these to ModsRoute, so a `mod:` view can't fall through to a branch that draws something else. */
export function isModsRoute(view: string): boolean {
  return view === "mods" || view.startsWith("mod:");
}

/** Whether the learning tools show: what `teachVisible` meant. It also governs the review controls
 *  inline in Documents and a project's files, and Documents' jump to Review. */
export function learningToolsShown(resolved: ResolvedNav): boolean {
  return resolved.shown.some((e) => e.group === LEARNING_GROUP);
}

// --- placement changes (pure) --------------------------------------------------------------------

/** `id`'s entry and its group mates, in canonical order; empty for an unknown id. */
function unitOf(entries: readonly NavEntryDef[], id: string): NavEntryDef[] {
  const entry = entries.find((e) => e.id === id);
  if (!entry) return [];
  return entry.group ? entries.filter((e) => e.group === entry.group) : [entry];
}

const without = (list: readonly string[], ids: readonly string[]) =>
  list.filter((x) => !ids.includes(x));

/** Pin `id` (and its group) after anything already pinned. Pinning un-hides. */
export function pinEntry(
  placement: NavPlacement,
  id: string,
  entries: readonly NavEntryDef[],
): NavPlacement {
  const ids = unitOf(entries, id).map((e) => e.id as string);
  if (ids.length === 0) return placement;
  return {
    hidden: without(placement.hidden, ids),
    pinned: [...placement.pinned, ...ids.filter((x) => !placement.pinned.includes(x))],
  };
}

export function unpinEntry(
  placement: NavPlacement,
  id: string,
  entries: readonly NavEntryDef[],
): NavPlacement {
  const ids = unitOf(entries, id).map((e) => e.id as string);
  if (ids.length === 0) return placement;
  return { hidden: placement.hidden, pinned: without(placement.pinned, ids) };
}

/** Hide `id` (and its group). Hiding unpins too, or the pin would keep it showing. An entry that
 *  can't hide is left as it is. */
export function hideEntry(
  placement: NavPlacement,
  id: string,
  entries: readonly NavEntryDef[],
): NavPlacement {
  const unit = unitOf(entries, id).filter((e) => e.hideable);
  if (unit.length === 0) return placement;
  const ids = unit.map((e) => e.id as string);
  return {
    hidden: [...placement.hidden, ...ids.filter((x) => !placement.hidden.includes(x))],
    pinned: without(placement.pinned, ids),
  };
}

/** Bring `id` (and its group) back into the sidebar: un-hide it, and pin whatever this Depth would
 *  still leave out, because a pin is the only thing that overrides a default. */
export function showEntry(
  placement: NavPlacement,
  id: string,
  entries: readonly NavEntryDef[],
  ctx: NavContext,
): NavPlacement {
  const unit = unitOf(entries, id);
  if (unit.length === 0) return placement;
  const ids = unit.map((e) => e.id as string);
  const pinned = [...placement.pinned];
  for (const e of unit) {
    if (!e.defaultShown(ctx.depth) && !pinned.includes(e.id)) pinned.push(e.id);
  }
  return { hidden: without(placement.hidden, ids), pinned };
}

/** "Review and Teach"; "Map". */
function names(unit: readonly NavEntryDef[]): string {
  // A mod chose its tab's name, so in a menu item it is quoted and marked (the sidebar and Ctrl K
  // mark it too): "Hide “Review” (mod)" can't be taken for PM's own Review tab.
  const labels = unit.map((e) => (e.source === "mod" ? `${quoteName(e.label)} (mod)` : e.label));
  return labels.length <= 1
    ? (labels[0] ?? "")
    : `${labels.slice(0, -1).join(", ")} and ${labels[labels.length - 1]}`;
}

/** The right-click menu over the sidebar's tabs: Pin / Unpin and Hide for the tab under the pointer
 *  (`targetId`, null on the empty part of the list), then a Show for every tab you can't see there.
 *  `apply` receives the new placement; the caller persists it. */
export function navMenuItems(
  targetId: string | null,
  entries: readonly NavEntryDef[],
  placement: NavPlacement,
  ctx: NavContext,
  apply: (next: NavPlacement) => void,
): MenuItem[] {
  const resolved = resolveNav(entries, placement, ctx);
  const items: MenuItem[] = [];
  const target = resolved.shown.find((e) => e.id === targetId);
  if (target) {
    // Only the mates that exist here: a group never names a tab the user can't have.
    const unit = unitOf(entries, target.id).filter((e) => available(e, ctx));
    const both = unit.length > 1 ? ` ${names(unit)}` : "";
    if (placement.pinned.includes(target.id)) {
      items.push({
        label: `Unpin${both}`,
        onSelect: () => apply(unpinEntry(placement, target.id, entries)),
      });
    } else {
      items.push({
        label: `Pin${both} to the top`,
        onSelect: () => apply(pinEntry(placement, target.id, entries)),
      });
    }
    if (target.hideable) {
      items.push({
        label: `Hide${both}`,
        onSelect: () => apply(hideEntry(placement, target.id, entries)),
      });
    }
  }
  // One Show per unit, in canonical order, so the learning tools come back as one line.
  const away = new Set([...resolved.hidden, ...resolved.dormant]);
  const offered = new Set<string>();
  const shows: MenuItem[] = [];
  for (const entry of entries) {
    if (!away.has(entry)) continue;
    const key = entry.group ?? entry.id;
    if (offered.has(key)) continue;
    offered.add(key);
    const unit = unitOf(entries, entry.id).filter((e) => away.has(e));
    shows.push({
      label: `Show ${names(unit)}`,
      onSelect: () => apply(showEntry(placement, entry.id, entries, ctx)),
    });
  }
  if (shows.length > 0) items.push({ kind: "separator" }, ...shows);
  return items;
}

// --- storage (this device) -----------------------------------------------------------------------

export const NAV_PLACEMENT_KEY = "pm.nav.placement";
// The two Settings switches this replaces ("Review & Teach tabs", "Map tab"), each "auto" | "show"
// | "hide", which ThemeContext no longer reads. Adopted once into the placement, then removed.
export const LEGACY_TEACH_KEY = "pm:theme:teach";
export const LEGACY_MAP_KEY = "pm:theme:map";

const CHANGED_EVENT = "pm:settings-changed";
const EMPTY: NavPlacement = Object.freeze({ hidden: Object.freeze([]), pinned: Object.freeze([]) });
// More than any real sidebar holds: a corrupt or hostile value can't grow the work per render.
const MAX_IDS = 64;
// The mod id grammar (spec §1.2). Rust is the authority over ids; this only decides which stored
// strings are worth keeping for a mod that may be installed later.
const MOD_ID = /^[a-z][a-z0-9-]{0,30}(\.[a-z][a-z0-9-]{0,30}){1,4}$/;
const CORE_IDS: ReadonlySet<string> = new Set(CORE_NAV.map((e) => e.id));

/** Whether a stored string could name a tab: one of PM's, or a well-formed `mod:<id>`. */
export function isNavIdShape(id: string, core: ReadonlySet<string> = CORE_IDS): boolean {
  return core.has(id) || (id.startsWith("mod:") && MOD_ID.test(id.slice(4)));
}

/** Core tabs that became a mod's tab, by the id a placement stored for them (the Map: `graph`
 *  became `mod:atlas.map`). A pin or a hide the user made on the old tab belongs to the new one. */
const RENAMED_NAV_IDS: ReadonlyMap<string, string> = new Map([["graph", "mod:atlas.map"]]);

/** The id a stored placement entry means now. The translation takes effect only while the old id is
 *  not one of PM's own entries (`core`), which is the case for `graph` now that the core Map tab is
 *  gone, so the pins and hides stored for it are carried over and not silently dropped as an id
 *  nobody knows. */
export function renamedNavId(id: string, core: ReadonlySet<string> = CORE_IDS): string {
  const to = RENAMED_NAV_IDS.get(id);
  return to !== undefined && !core.has(id) ? to : id;
}

function cleanIds(value: unknown, core: ReadonlySet<string>): string[] {
  if (!Array.isArray(value)) return [];
  const out: string[] = [];
  for (const stored of value) {
    if (out.length >= MAX_IDS) break;
    if (typeof stored !== "string") continue;
    const id = renamedNavId(stored, core);
    if (isNavIdShape(id, core) && !out.includes(id)) out.push(id);
  }
  return out;
}

/** A stored value, read defensively: anything malformed degrades to "nothing chosen". `core` is the
 *  set of PM's own tab ids (a test may pass a set that still has `graph`, to read the state before the core Map tab went). */
export function parsePlacement(
  raw: string | null,
  core: ReadonlySet<string> = CORE_IDS,
): NavPlacement {
  if (raw === null) return EMPTY;
  try {
    const value: unknown = JSON.parse(raw);
    if (!value || typeof value !== "object" || Array.isArray(value)) return EMPTY;
    const v = value as Record<string, unknown>;
    return { hidden: cleanIds(v.hidden, core), pinned: cleanIds(v.pinned, core) };
  } catch {
    return EMPTY;
  }
}

/** The mods that took over a core tab: which tab, and whether it showed at a Depth when the user had
 *  said nothing. What "installed hidden" (below) preserves. */
const PILOT_CORE_TAB: ReadonlyMap<
  string,
  { tab: string; shown: (depth: Depth) => boolean; feature: string }
> = new Map([["atlas.map", { tab: "graph", shown: notMinimal, feature: "Map" }]]);

/** What the person used that made PM install `modId` for them ("Map"), or null when it isn't one of
 *  the mods PM installs on its own. Worded on the mod's first-open screen. */
export function pilotFeature(modId: string): string | null {
  return PILOT_CORE_TAB.get(modId)?.feature ?? null;
}

/** Whether the installed-hidden rule applies to `modId` at all: only once the core tab it replaced
 *  is gone from PM's own entries (`core`), which it is for the Map. A set that still names the tab
 *  says the core tab is the user's Map and the mod's arrival changes nothing they see. */
export function pilotHiddenRuleApplies(
  modId: string,
  core: ReadonlySet<string> = CORE_IDS,
): boolean {
  const pilot = PILOT_CORE_TAB.get(modId);
  return pilot !== undefined && !core.has(pilot.tab);
}

/** The placement after a pilot mod landed in this vault, on a device where its core tab wasn't
 *  showing (Minimal, and not pinned): the new tab is added to the hidden set, so installing the mod
 *  doesn't make a tab appear that the user's sidebar had left out. A tab the user had pinned stays
 *  pinned, one already hidden is left as it is, and nothing changes while the core tab still exists
 *  ([`pilotHiddenRuleApplies`]). Pure. */
export function placementAfterPilotLanded(
  placement: NavPlacement,
  modId: string,
  depth: Depth,
  core: ReadonlySet<string> = CORE_IDS,
): NavPlacement {
  const id = `mod:${modId}`;
  if (!pilotHiddenRuleApplies(modId, core) || placement.pinned.includes(id)) return placement;
  if (PILOT_CORE_TAB.get(modId)?.shown(depth) || placement.hidden.includes(id)) return placement;
  return { hidden: [...placement.hidden, id], pinned: placement.pinned };
}

/** The old Settings switches as a placement. "hide" hides. "show" meant shown whatever the Depth, so
 *  it pins only where `depth` would otherwise leave the tab out; elsewhere it is the default already
 *  and pinning would move the tab for nothing. */
export function adoptLegacy(teach: string | null, map: string | null, depth: Depth): NavPlacement {
  const hidden: string[] = [];
  const pinned: string[] = [];
  const adopt = (pref: string | null, ids: NavId[], shown: (id: NavId) => boolean) => {
    if (pref === "hide") hidden.push(...ids);
    if (pref === "show") {
      for (const id of ids) {
        if (!shown(id)) pinned.push(id);
      }
    }
  };
  adopt(
    teach,
    ["review", "teach"],
    (id) => CORE_NAV.find((e) => e.id === id)?.defaultShown(depth) ?? false,
  );
  // The Map is a mod now: its old switch lands on the mod's tab, by the old tab's Depth rule (kept in
  // PILOT_CORE_TAB), not by a `graph` entry that is no longer in CORE_NAV.
  const mapMod = PILOT_CORE_TAB.get("atlas.map");
  adopt(map, ["mod:atlas.map"], () => mapMod?.shown(depth) ?? false);
  return { hidden, pinned };
}

function lsGet(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}
function lsSet(key: string, value: string): boolean {
  if (isTearingDown()) return false;
  try {
    localStorage.setItem(key, value);
    return true;
  } catch {
    return false;
  }
}
function lsRemove(key: string): void {
  try {
    localStorage.removeItem(key);
  } catch {
    /* best-effort */
  }
}
function announce(): void {
  try {
    window.dispatchEvent(new Event(CHANGED_EVENT));
  } catch {
    /* non-browser context (tests) */
  }
}

/** The stored value, adopting the old switches the first time there's none. The adoption writes, so
 *  it runs once; if the write fails it is simply worked out again on the next read. */
function storedRaw(depth: Depth): string | null {
  const raw = lsGet(NAV_PLACEMENT_KEY);
  if (raw !== null) return raw;
  const teach = lsGet(LEGACY_TEACH_KEY);
  const map = lsGet(LEGACY_MAP_KEY);
  if (teach === null && map === null) return null;
  const adopted = JSON.stringify(adoptLegacy(teach, map, depth));
  if (lsSet(NAV_PLACEMENT_KEY, adopted)) {
    lsRemove(LEGACY_TEACH_KEY);
    lsRemove(LEGACY_MAP_KEY);
  }
  return adopted;
}

// Keyed on the raw string, so an unchanged value returns the same object: `useSyncExternalStore`
// re-renders forever on a snapshot that is new on every read.
let cache: { raw: string | null; value: NavPlacement } = { raw: null, value: EMPTY };

/** This device's placement. `depth` is used only to adopt the old switches. */
export function readNavPlacement(depth: Depth): NavPlacement {
  const raw = storedRaw(depth);
  if (raw !== cache.raw) cache = { raw, value: parsePlacement(raw) };
  return cache.value;
}

export function writeNavPlacement(placement: NavPlacement): void {
  if (
    lsSet(NAV_PLACEMENT_KEY, JSON.stringify({ hidden: placement.hidden, pinned: placement.pinned }))
  )
    announce();
}

/** Back to the shipped sidebar (Settings' one control for this). The old switches go too, or the
 *  next read would adopt them again. */
export function resetNavPlacement(): void {
  lsRemove(NAV_PLACEMENT_KEY);
  lsRemove(LEGACY_TEACH_KEY);
  lsRemove(LEGACY_MAP_KEY);
  announce();
}

export function navPlacementIsDefault(placement: NavPlacement): boolean {
  return placement.hidden.length === 0 && placement.pinned.length === 0;
}

// Settings opens over a still-mounted sidebar, so both follow every write as it lands.
function subscribe(onChange: () => void): () => void {
  window.addEventListener(CHANGED_EVENT, onChange);
  window.addEventListener("storage", onChange);
  return () => {
    window.removeEventListener(CHANGED_EVENT, onChange);
    window.removeEventListener("storage", onChange);
  };
}

// --- hooks ---------------------------------------------------------------------------------------

/** This device's placement, live. */
export function useNavPlacement(): NavPlacement {
  const { depth } = useTheme();
  return useSyncExternalStore(subscribe, () => readNavPlacement(depth));
}

/** The navigation as this user has it: PM's own entries, then the tabs installed mods contribute
 *  (from `lib/modRows`'s provider; with none above it, PM's own alone). The sidebar, Ctrl K and App's
 *  fallback all call this, so a mod's tab appears in all three together. `entries` replaces the
 *  list outright (the tests). */
export function useNav(entries?: readonly NavEntryDef[]) {
  const { depth } = useTheme();
  const { devMode } = useDevMode();
  const placement = useNavPlacement();
  const { rows } = useModRows();
  const all = useMemo(() => entries ?? [...CORE_NAV, ...modNavEntries(rows)], [entries, rows]);
  const ctx = useMemo<NavContext>(() => ({ depth, devMode }), [depth, devMode]);
  const resolved = useMemo(() => resolveNav(all, placement, ctx), [all, placement, ctx]);
  return { entries: all, placement, ctx, resolved };
}

/** Where one mod's tab stands on this device, and the two things the Mods tab's row can do about it:
 *  `show` is the "Show tab" a hidden mod tab's row offers, `hide` its opposite (both are the
 *  right-click menu's own changes, so the two can't disagree). A mod that names no tab has none to
 *  show or hide, and both do nothing. */
export function useModTab(modId: string): {
  state: "shown" | "hidden" | "none";
  show: () => void;
  hide: () => void;
} {
  const { entries, placement, ctx, resolved } = useNav();
  const id = `mod:${modId}`;
  const state = resolved.shown.some((e) => e.id === id)
    ? "shown"
    : resolved.hidden.some((e) => e.id === id)
      ? "hidden"
      : "none";
  return {
    state,
    show: () => {
      if (state !== "none") writeNavPlacement(showEntry(placement, id, entries, ctx));
    },
    hide: () => {
      if (state !== "none") writeNavPlacement(hideEntry(placement, id, entries));
    },
  };
}

/** After an uninstall: forget `tab` (a `mod:<id>`) in this device's placement, so a mod installed
 *  again later starts as the shipped sidebar would have it, not as it was left. */
export function forgetNavTab(tab: string): void {
  const raw = lsGet(NAV_PLACEMENT_KEY);
  if (raw === null) return;
  const placement = parsePlacement(raw);
  if (!placement.hidden.includes(tab) && !placement.pinned.includes(tab)) return;
  writeNavPlacement({
    hidden: placement.hidden.filter((x) => x !== tab),
    pinned: placement.pinned.filter((x) => x !== tab),
  });
}

/** `learningToolsShown` for a component that needs nothing else, and no developer mode: neither
 *  learning tool has a capability gate. */
export function useLearningToolsVisible(): boolean {
  const { depth } = useTheme();
  const placement = useNavPlacement();
  return useMemo(
    () => learningToolsShown(resolveNav(CORE_NAV, placement, { depth, devMode: false })),
    [placement, depth],
  );
}
