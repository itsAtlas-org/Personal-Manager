// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The installed mods, as one read every reader shares (Mods spec §1.13, §1.17). The sidebar's mod
// tabs, Ctrl K, App's fallback, each mod tab's panel and the Mods tab all need the same rows, and
// each asking `list_mods` for itself would race its own answers. `ModRowsProvider` (modRowsProvider
// .tsx, mounted once in the main window) reads them on mount and on every `mods://changed`; this
// file is the context readers use. With no provider above (a test, the pop-out Calendar), there are
// no rows, and PM's own tabs are all there is.

import { createContext, useContext } from "react";
import type { ModInfo } from "./modsTypes";

export interface ModRows {
  /** The installed mods, as the backend last listed them (kept when a later read fails, so one
   *  failed read doesn't take the tabs away). */
  rows: readonly ModInfo[];
  /** A read has succeeded. Before it, `rows` is empty because nobody has answered, not because
   *  there are no mods. */
  loaded: boolean;
  /** Why the last read failed, in the backend's words, until one succeeds. */
  problem: string | null;
  /** Read the rows again now (a command that changed one calls it so the tab doesn't wait for
   *  `mods://changed`). Never rejects. */
  refresh: () => Promise<void>;
}

const NONE: ModRows = {
  rows: [],
  loaded: false,
  problem: null,
  refresh: () => Promise.resolve(),
};

export const ModRowsContext = createContext<ModRows>(NONE);

export function useModRows(): ModRows {
  return useContext(ModRowsContext);
}
