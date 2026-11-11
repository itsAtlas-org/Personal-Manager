// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What a mod gets for `theme.get` (Mods spec §1.5, §3's theme kit): PM's live theme, so a mod's own
// page can wear PM's tokens. The backend authorises the call; the page answers, because the theme is
// the page's (ThemeContext, applied to the root by tokens.ts's applyTheme). `theme.changed` (part 4)
// posts the same snapshot.
//
// - `vars`: every CSS custom property PM's theme sets on the root element (`--bg`, `--ink`, `--accent`,
//   `--head`, `--radius`, the density sizes…), exactly as applied, so the mod sees what PM shows.
// - The axes a mod may branch on: the resolved mode (never `system`/`auto`), the system, the accent,
//   the colour-blind palette and the depth.
//
// Nothing else on the root (its other styles, its classes, its data attributes) and nothing from the
// rest of the page reaches a mod.

import type { ThemeState } from "../../theme/ThemeContext";

export type ModThemeAxes = Pick<ThemeState, "mode" | "system" | "accent" | "colorblind" | "depth">;

export interface ModTheme extends ModThemeAxes {
  vars: Record<string, string>;
}

/** PM's theme as a mod receives it: `root`'s custom properties, and `theme`'s axes. */
export function themeSnapshot(root: HTMLElement, theme: ModThemeAxes): ModTheme {
  const vars: Record<string, string> = {};
  const style = root.style;
  for (let i = 0; i < style.length; i += 1) {
    const name = style.item(i);
    if (name.startsWith("--")) vars[name] = style.getPropertyValue(name).trim();
  }
  return {
    vars,
    mode: theme.mode,
    system: theme.system,
    accent: theme.accent,
    colorblind: theme.colorblind,
    depth: theme.depth,
  };
}
