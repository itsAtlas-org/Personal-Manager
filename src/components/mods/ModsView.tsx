// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Mods tab (Mods spec §1.13). This is its shell: the page header and the three sections the spec
// names, in order, each an element of its own for the next parts of the slice to fill:
//
//   Installed         the installed mods, their standing, permissions, pause and uninstall
//   Available         the signed catalogue
//   Install from file the one home of file installs (an unsigned one needs developer mode)
//
// The installed rows are `useModRows()` (lib/modRows): the same read the sidebar's mod tabs use, so
// the two never disagree about what is installed.

import { useModRows } from "../../lib/modRows";

export function ModsView() {
  const { rows, loaded, problem } = useModRows();
  return (
    <div className="flex h-full flex-col">
      <header className="border-b border-border px-6 py-3">
        <h1 className="font-head text-sm font-semibold text-ink">Mods</h1>
        <p className="text-xs text-ink3">
          {!loaded
            ? (problem ?? "Loading…")
            : rows.length === 0
              ? "No mods installed"
              : `${rows.length} mod${rows.length === 1 ? "" : "s"} installed`}
        </p>
      </header>
      <div className="flex-1 overflow-y-auto" data-help="mods-view">
        <div className="mx-auto max-w-3xl space-y-8 px-6 py-6">
          <section aria-labelledby="mods-installed" data-mods-section="installed">
            <h2 id="mods-installed" className="font-head text-sm font-semibold text-ink">
              Installed
            </h2>
          </section>
          <section aria-labelledby="mods-available" data-mods-section="available">
            <h2 id="mods-available" className="font-head text-sm font-semibold text-ink">
              Available
            </h2>
          </section>
          <section aria-labelledby="mods-from-file" data-mods-section="from-file">
            <h2 id="mods-from-file" className="font-head text-sm font-semibold text-ink">
              Install from file
            </h2>
          </section>
        </div>
      </div>
    </div>
  );
}
