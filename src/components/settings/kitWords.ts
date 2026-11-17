// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The words of Settings → Developer's kit that are the page's own: what a section says while developer
// mode is off, and each limit's label with how its number is said. The figures are PM's (`devKitInfo`).

import { formatBytes } from "../../lib/format";
import type { KitLimits } from "../../lib/modsTypes";

/** What a kit section says while developer mode is off (the backend refuses the read then). */
export const OFF_HINT = "Turn developer mode on above to see this.";

const count = (n: number) => String(n);

/** A limit's label and how its number is said. Every key of `KitLimits` has one (a compile error
 *  otherwise), so a limit the backend adds can't ship unlabelled. */
export const LIMIT_ROWS: Record<keyof KitLimits, { label: string; say: (n: number) => string }> = {
  callsPerSecond: { label: "Calls per second, sustained", say: count },
  callBurst: { label: "Calls at once after a quiet spell", say: count },
  maxInFlight: { label: "Calls running at the same time", say: count },
  maxArgsBytes: { label: "Largest call, as sent", say: formatBytes },
  maxResultBytes: { label: "Largest answer", say: formatBytes },
  maxBinaryResultBytes: { label: "Largest binary answer", say: formatBytes },
  maxDocsPerCall: { label: "Documents one call may name or return", say: count },
  storageQuotaBytes: { label: "A mod's own storage", say: formatBytes },
  storageGrowthPerSecond: { label: "Storage-growing writes per second", say: count },
  storageGrowthBurst: { label: "Storage-growing writes after a quiet spell", say: count },
  eventWindow: { label: "Events sent and not yet acknowledged", say: count },
  maxEventBytes: { label: "Largest event", say: formatBytes },
  maxIdsPerEvent: { label: "Ids in one deletion event", say: count },
  eventCoalesceMs: { label: "Wait before announcing a burst", say: (n) => `${n} ms` },
  maxPendingIds: { label: "Ids held while a burst settles", say: count },
};

/** The limits in the order the page lists them. */
export const LIMIT_KEYS = Object.keys(LIMIT_ROWS) as (keyof KitLimits)[];
