// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Map's project colours, moved in from the core (src/theme/graphPalette.ts, which goes with the core
// Map in B9). DOCUMENTED EXCEPTION to PM's "no hex literals" rule, the same one the core had: the Map
// colours projects categorically, which needs as many distinct hues as there are projects, and the
// accent-driven token system (one accent and tinted neutrals) cannot express that. The theme's own
// colours (edges, halos, strokes, text) come from the vars `theme.get` hands over, never from here.
//
// A mod cannot import the core's copy (the lint fence), so this is a copy; palette.test.ts spells out
// every colour of the four sets, so an edit here is a visible test change, and a drift from the core's
// copy (which goes in B9) shows up in the live comparison with the core Map.

/** The resolved mode: PM's `theme.get` never says `system` or `auto`. */
export type Mode = "dark" | "light";

// The dark set is the original V1 palette; the light set is deepened for contrast on the near-white
// light-mode background. Indexed by (project index % length).
const DARK: readonly string[] = [
  "#60a5fa",
  "#34d399",
  "#f472b6",
  "#fbbf24",
  "#a78bfa",
  "#22d3ee",
  "#fb923c",
  "#4ade80",
  "#f87171",
  "#c084fc",
];
const LIGHT: readonly string[] = [
  "#2563eb",
  "#059669",
  "#db2777",
  "#d97706",
  "#7c3aed",
  "#0891b2",
  "#ea580c",
  "#16a34a",
  "#dc2626",
  "#9333ea",
];

// Colour-blind-safe (Okabe-Ito) sets, used when the colour-blind axis is on. Mode-tuned at the two
// extremes the base palette cannot share: yellow is invisible on the light background (a dark gold) and
// pure black is invisible on the dark one (a light neutral); the six chromatic hues carry unchanged.
const CVD_DARK: readonly string[] = [
  "#56b4e9",
  "#e69f00",
  "#009e73",
  "#f0e442",
  "#0072b2",
  "#d55e00",
  "#cc79a7",
  "#bbbbbb",
];
const CVD_LIGHT: readonly string[] = [
  "#0072b2",
  "#e69f00",
  "#009e73",
  "#cc79a7",
  "#d55e00",
  "#56b4e9",
  "#8a6d00",
  "#000000",
];

/** The colour of the project at `index` (its place in the legend). */
export function graphColor(index: number, mode: Mode, colorblind = false): string {
  const palette = colorblind
    ? mode === "light"
      ? CVD_LIGHT
      : CVD_DARK
    : mode === "light"
      ? LIGHT
      : DARK;
  const i = ((index % palette.length) + palette.length) % palette.length;
  return palette[i];
}

/** A legible mid-grey on either background, for a theme colour the snapshot does not carry (the core's
 *  own fallback for a token that read empty). */
export const THEME_FALLBACK = "#888";
