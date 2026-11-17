// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A small mark beside the name of a mod PM paused because developer mode is off (Bobby, 2026-11-17: "on
// the top maybe next to the mod name, put a dev mode turned off tab"). The mod's own tab and its row
// in the Mods tab both wear it, in the same pill the row's other marks use, in the warning tone the
// badge for a mod nobody has checked uses. It is shown only for the standing the backend names
// `paused-dev-mode` (state reason dev-mode): the page never works out a standing, so a mod paused for
// any other reason, or running, never wears it.

import { cn } from "../ui/cn";
import { toneSurface } from "../ui/tone";

export function DevModeOffMark({ className }: { className?: string }) {
  return (
    <span
      className={cn("flex-none rounded-full border px-2 py-px text-xs font-normal", className)}
      style={toneSurface("warning")}
      data-help="mod-dev-mode-off"
    >
      Developer mode off
    </span>
  );
}
