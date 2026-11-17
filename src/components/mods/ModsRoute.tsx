// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Which of the mods' two kinds of view App is on (Mods spec §1.13, §1.17): the Mods tab ("mods"), or
// one mod's own tab ("mod:<id>"). App hands both here so the routing has a home a test can reach
// (App itself can't be rendered), and so a `mod:` view can never fall through to a branch that draws
// something else: every string App sends here gets one of the two.

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
