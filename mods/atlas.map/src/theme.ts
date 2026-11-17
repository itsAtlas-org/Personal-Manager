// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// PM's theme as the Map wears it. `theme.get` (and the `theme.changed` event, the same shape) hands the
// frame the live theme: every CSS custom property PM sets on its root, and the axes a mod may branch on
// (the resolved mode, the colour-blind palette, the depth). Two uses:
//  - the page's own chrome (header, buttons, strips, the card) is hand-written CSS over those vars, so
//    the vars are set on this page's root element. That is `style.setProperty` on the CSSOM, which a
//    `style-src` policy does not block (only the `style` attribute and `<style>` text are), and no
//    `style` attribute or `<style>` is ever written. Until the theme arrives, and if it never does, the
//    stylesheet's own fallbacks (system colours) keep the page legible.
//  - the canvas draws with concrete colour strings (it cannot read `var(--...)`), so they are taken from
//    the snapshot itself, not from the page's computed style: the canvas is themed even where the
//    page's chrome is not.
//
// Everything read here is read defensively and applied narrowly: PM is trusted, but a value the page
// is about to hand to the CSSOM or the canvas is first checked for its shape, so a bad snapshot is no
// theme (the current one stays), never a half-applied one.

import { THEME_FALLBACK, type Mode } from "./palette";

export type Depth = "min" | "standard" | "power";

export interface MapTheme {
  /** PM's custom properties, name to value, exactly as PM applied them. */
  vars: Readonly<Record<string, string>>;
  mode: Mode;
  /** The colour-blind-safe project palette is on. */
  colorblind: boolean;
  depth: Depth;
}

/** What the page wears until PM answers (and wears for good if it never does). */
export const DEFAULT_THEME: MapTheme = {
  vars: {},
  mode: "dark",
  colorblind: false,
  depth: "standard",
};

/** The most properties and the longest name and value taken from a snapshot. PM sets about thirty. */
const MAX_VARS = 200;
const MAX_NAME = 64;
const MAX_VALUE = 512;
const VAR_NAME = /^--[a-z0-9][a-z0-9-]*$/;

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

/** The theme a `theme.get` answer or a `theme.changed` payload means, or null when it is not one. A
 *  property that is not a plain custom property with a short text value is left out; a mode that is
 *  not `light` is dark (PM resolves it, so only these two exist); an unknown depth is the middle one. */
export function readTheme(raw: unknown): MapTheme | null {
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
  return {
    vars,
    mode: raw.mode === "light" ? "light" : "dark",
    colorblind: raw.colorblind === true,
    depth: raw.depth === "min" || raw.depth === "power" ? raw.depth : "standard",
  };
}

/** The part of an element the theme is applied to (a test's stand-in has just these). */
export interface ThemeTarget {
  style: {
    setProperty(name: string, value: string): void;
    removeProperty(name: string): string;
    colorScheme: string;
  };
  dataset: DOMStringMap;
}

/**
 * Put `theme` on `root`: its custom properties (and none of an earlier theme's that this one no longer
 * has), the mode for the browser's own controls and scrollbars, and `data-mode` for the stylesheet.
 * Returns the names now set, to pass back as `previous` next time.
 */
export function applyTheme(
  root: ThemeTarget,
  theme: MapTheme,
  previous: ReadonlySet<string> = new Set(),
): Set<string> {
  const now = new Set<string>();
  for (const [name, value] of Object.entries(theme.vars)) {
    root.style.setProperty(name, value);
    now.add(name);
  }
  for (const name of previous) if (!now.has(name)) root.style.removeProperty(name);
  root.style.colorScheme = theme.mode;
  root.dataset.mode = theme.mode;
  root.dataset.depth = theme.depth;
  return now;
}

/** The theme colours the canvas needs as concrete strings. */
export interface CanvasColors {
  border: string;
  bg: string;
  ink: string;
  ink2: string;
  stLook: string;
  uiFont: string;
}

/** The same six the core Map snapshotted from its computed style, taken from the snapshot instead. */
export function canvasColors(theme: MapTheme): CanvasColors {
  const v = (name: string) => theme.vars[name]?.trim() || THEME_FALLBACK;
  return {
    border: v("--border"),
    bg: v("--bg"),
    ink: v("--ink"),
    ink2: v("--ink2"),
    stLook: v("--st-look"),
    uiFont: theme.vars["--ui"]?.trim() || "system-ui, sans-serif",
  };
}
