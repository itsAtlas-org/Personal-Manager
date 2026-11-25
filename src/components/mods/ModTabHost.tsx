// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Where the mods' own tabs (`mod:<id>`) live while the app is on one, and for the moment after it
// leaves (Mods spec §1.5, §4; PR C, K5).
//
// PM's tab router unmounts a view on every switch, and a frame's life is its mount: the unmount closed
// the port and the session with no chance to flush, so a mod that debounces its saves (the Pinboard
// waits 500 ms) lost its last edit to a quick tab switch. So a mod's tab is NOT drawn inside the router's
// keyed view. It is drawn here, beside it, and when the app leaves it the tab is kept mounted, hidden,
// while PM tells the frame (`lifecycle.suspend`, reason `hidden`) and waits for its `flushed` or for PM's
// own deadline (`suspendModFrames`: never past 2.5 s, whatever the backend does). Only then does it
// unmount and end the session. The same wait runs before a pause, an uninstall, an update, a vault
// change, the lock and a quit, from the places that do those (`ipc.ts`, `mods::tab::commands`,
// `mods::suspend`).
//
// Coming back to a tab that is still waiting keeps the frame it has: it was only told PM might take it
// down. Coming back after it went opens a new frame and a new session, as before.
//
// What the router decides stays the router's: the Mods tab and every core view are still drawn inside it.
// This draws mod tabs and nothing else.

import { useCallback, useEffect, useState } from "react";

import { suspendModFrames } from "../../lib/ipc";
import { isModView } from "../../lib/nav";
import type { View } from "../../lib/nav";
import type { SettingsTab } from "../settings/registry";
import { ErrorBoundary } from "../ui";
import { ModTab } from "./ModTab";

const MOD_PREFIX = "mod:";

export function ModTabHost({
  view,
  onOpenMods,
  onOpenSettings,
}: {
  view: View;
  onOpenMods: () => void;
  onOpenSettings: (tab: SettingsTab) => void;
}) {
  const active = isModView(view) ? view.slice(MOD_PREFIX.length) : null;
  // Every mod tab with a mount, the one on screen and the ones waiting to retire. Adjusted while
  // rendering (React's way to derive state from props) so the tab the app just left is never unmounted
  // for a render in between.
  const [mounted, setMounted] = useState<string[]>([]);
  if (active !== null && !mounted.includes(active)) setMounted([...mounted, active]);
  const retire = useCallback((id: string) => setMounted((all) => all.filter((m) => m !== id)), []);
  return (
    <>
      {mounted.map((id) => (
        <ModSlot
          key={id}
          modId={id}
          active={id === active}
          onRetired={retire}
          onOpenMods={onOpenMods}
          onOpenSettings={onOpenSettings}
        />
      ))}
    </>
  );
}

function ModSlot({
  modId,
  active,
  onRetired,
  onOpenMods,
  onOpenSettings,
}: {
  modId: string;
  active: boolean;
  onRetired: (id: string) => void;
  onOpenMods: () => void;
  onOpenSettings: (tab: SettingsTab) => void;
}) {
  // Not on screen any more: tell the frame, wait for its answer or the deadline, then let it go. A
  // return to the tab before that cancels the retirement (the cleanup), and the wait's end does nothing.
  useEffect(() => {
    if (active) return;
    let cancelled = false;
    void suspendModFrames("hidden", modId).then(() => {
      if (!cancelled) onRetired(modId);
    });
    return () => {
      cancelled = true;
    };
  }, [active, modId, onRetired]);
  return (
    // `display: none` takes it off the screen, out of the tab order and out of the accessibility tree
    // while the frame inside keeps running; a class would fight `flex` (cn() is not tailwind-merge).
    <main
      className="flex h-full min-w-0 flex-1 flex-col"
      style={active ? undefined : { display: "none" }}
      data-mod-tab={modId}
    >
      <ErrorBoundary what="This mod">
        <ModTab modId={modId} onOpenMods={onOpenMods} onOpenSettings={onOpenSettings} />
      </ErrorBoundary>
    </main>
  );
}
