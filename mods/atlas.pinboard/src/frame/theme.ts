// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// PM's theme as the Pinboard's frame wears it: a `ThemeStore` for the kit's `ThemeProvider` (the same
// provider core wears, so every kit component reads the theme the same way) built from `theme.get` and
// `theme.changed` instead of core's settings table. The two answer in one shape (a snapshot: every
// `--…` custom property PM set on its root, and the axes a mod may branch on).
//
// What the provider is given: a blob of the axes (system, mode, accent, depth, colour-blind palette,
// and the text size and density the variables show), so `useTheme()` and `useDepth()` read as they do
// in PM; and PM's own variables, which the provider sets on the root after its own so the page looks as
// PM does even where the axes cannot say how (the contrast level). The frame writes nothing back: `set`
// is a no-op, the theme is PM's.
//
// A snapshot is read defensively (PM is trusted, but a value about to reach the CSSOM is checked for its
// shape first): a custom property that is not a plain `--name` with a short text value is left out, and
// a snapshot that is not an object with `vars` is no theme (the one the page has stays).

import type { ModHost } from "../../../../src/kit/host";
import type { ThemeStore } from "../../../../src/kit/theme";

/** The most properties, and the longest name and value, taken from a snapshot. PM sets about thirty. */
const MAX_VARS = 200;
const MAX_NAME = 64;
const MAX_VALUE = 512;
const VAR_NAME = /^--[a-z0-9][a-z0-9-]*$/;

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

export interface Snapshot {
  vars: Readonly<Record<string, string>>;
  blob: string;
}

/** The text sizes the provider knows (`FontScale`), by the `--font-scale` PM sets for each. */
const FONT_SCALES: readonly [string, number][] = [
  ["small", 0.9],
  ["default", 1],
  ["large", 1.15],
  ["xlarge", 1.3],
];

/** The snapshot `raw` means (`theme.get`'s answer or `theme.changed`'s payload), or null. */
export function readSnapshot(raw: unknown): Snapshot | null {
  if (!isRecord(raw) || !isRecord(raw.vars)) return null;
  const vars: Record<string, string> = {};
  let count = 0;
  for (const [name, value] of Object.entries(raw.vars)) {
    if (count >= MAX_VARS) break;
    if (name.length > MAX_NAME || !VAR_NAME.test(name)) continue;
    if (typeof value !== "string" || value.length > MAX_VALUE) continue;
    vars[name] = value;
    count += 1;
  }
  const parsed = Number.parseFloat(vars["--font-scale"] ?? "1");
  const scale = Number.isFinite(parsed) ? parsed : 1;
  const fontScale = FONT_SCALES.reduce((best, cur) =>
    Math.abs(cur[1] - scale) < Math.abs(best[1] - scale) ? cur : best,
  )[0];
  const text = (v: unknown, max = 64) =>
    typeof v === "string" && v !== "" && v.length <= max ? v : undefined;
  const blob = {
    system: text(raw.system),
    // The resolved mode is the preference: the provider has nothing to resolve.
    modePref: raw.mode === "light" ? "light" : "dark",
    accent: text(raw.accent),
    depth: raw.depth === "min" || raw.depth === "power" ? raw.depth : "standard",
    colorblind: raw.colorblind === true,
    fontScale,
    density: vars["--tap-min"] === "44px" ? "comfortable" : "standard",
    legibleFont: (vars["--ui"] ?? "").includes("Atkinson"),
  };
  return { vars, blob: JSON.stringify(blob) };
}

/** A theme store for the frame, once PM has given it the theme (`ready`). */
export interface FrameThemeStore extends ThemeStore {
  /** Resolves when the first snapshot arrived (or, for a PM that never answers, when it failed). */
  ready: Promise<void>;
}

export function frameThemeStore(host: ModHost): FrameThemeStore {
  let snapshot: Snapshot | null = null;
  const subscribers = new Set<(blob: string | null) => void>();
  const take = (raw: unknown) => {
    const next = readSnapshot(raw);
    if (!next) return;
    snapshot = next;
    for (const cb of [...subscribers]) cb(next.blob);
  };
  host.onEvent((event) => {
    if (event.name === "theme.changed") take(event.payload);
  });
  const ready = host.call("theme.get").then(take, () => {
    /* no theme: the page wears the provider's defaults, as a PM that never answered would leave it */
  });
  return {
    ready,
    get: async () => {
      await ready;
      return snapshot?.blob ?? null;
    },
    set: async () => {},
    subscribe: (cb) => {
      subscribers.add(cb);
      return () => void subscribers.delete(cb);
    },
    vars: () => snapshot?.vars ?? null,
  };
}
