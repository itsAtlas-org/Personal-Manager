// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// PM's theme provider. The context, the hooks and every axis live in the mod kit (src/kit/theme.tsx),
// which has no `ipc`: this file gives it the one thing it cannot reach, the store that mirrors the
// theme blob into the encrypted settings table (`appearance`, through `set_pref`) so the theme travels
// with the data folder. It is the same context object a mod's frame uses; the kit's `useTheme` is
// re-exported here so every core import site (and every test that reads it) is unchanged.

import type { ReactNode } from "react";
import { getPref, setPref } from "../lib/ipc";
import { ThemeProvider as KitThemeProvider, type ThemeStore } from "../kit/theme";

export { storedContrast, storedDensity, useTheme } from "../kit/theme";
export type { FontScale, ThemeState, ThemeStore } from "../kit/theme";

// The settings-table key under which the full theme blob is mirrored.
const PREF_KEY = "appearance";

const ipcStore: ThemeStore = {
  get: () => getPref(PREF_KEY),
  set: (blob) => setPref(PREF_KEY, blob),
};

export function ThemeProvider({ children }: { children: ReactNode }) {
  return <KitThemeProvider store={ipcStore}>{children}</KitThemeProvider>;
}
