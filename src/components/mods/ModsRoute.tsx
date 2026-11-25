// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Which of the mods' two kinds of view App is on (Mods spec §1.13, §1.17): the Mods tab ("mods"), or
// one mod's own tab ("mod:<id>"). App hands the Mods tab here; a mod's own tab is drawn by ModTabHost
// (K5), which keeps it mounted while its frame saves after a switch, and App draws nothing for a `mod:`
// view in the router. This still answers both, so a `mod:` string sent here can never fall through to
// a branch that draws something else.

import type { SettingsTab } from "../settings/registry";
import { ModTab } from "./ModTab";
import { ModsView } from "./ModsView";

const MOD_PREFIX = "mod:";

export function ModsRoute({
  view,
  onOpenMods,
  onOpenSettings,
}: {
  view: string;
  onOpenMods: () => void;
  onOpenSettings: (tab: SettingsTab) => void;
}) {
  if (view === "mods") return <ModsView />;
  return (
    <ModTab
      modId={view.slice(MOD_PREFIX.length)}
      onOpenMods={onOpenMods}
      onOpenSettings={onOpenSettings}
    />
  );
}
