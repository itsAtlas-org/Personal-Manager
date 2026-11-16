// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A mod's trust tier as PM's own badge (Mods spec §1.1): the band over a mod's frame and the panels
// PM draws in its place both wear it. The tier is the backend's word for the mod's row, never the
// mod's claim about itself; one PM doesn't know is shown as the least trusted.

import { cn } from "../ui/cn";
import { toneSurface } from "../ui/tone";

const TIER_LABEL = {
  first: "First party",
  second: "Second party",
  third: "Third party",
} as const;

export function TierBadge({ tier }: { tier: string }) {
  const known = Object.prototype.hasOwnProperty.call(TIER_LABEL, tier);
  const shown: keyof typeof TIER_LABEL = known ? (tier as keyof typeof TIER_LABEL) : "third";
  return (
    <span
      className={cn(
        "flex-none rounded-full border px-2 py-px",
        shown === "third" ? undefined : "border-border2 text-ink2",
      )}
      style={shown === "third" ? toneSurface("warning") : undefined}
    >
      {TIER_LABEL[shown]}
    </span>
  );
}
