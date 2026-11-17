// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The header Refresh's tooltip and description. The expected clocks are built with formatClock and
// formatDateTime, never typed: CI runs en-US (12-hour) and this box en-GB.

import { describe, expect, it } from "vitest";
import { formatClock, formatDateTime } from "./format";
import { refreshDescription, refreshTip } from "./calendarRefresh";

const NOW = new Date(2026, 10, 11, 18, 0);
const TODAY = new Date(2026, 10, 11, 14, 32).toISOString();
const EARLIER = new Date(2026, 10, 10, 14, 32).toISOString();

describe("refreshTip", () => {
  it("gives the clock time for a sync today", () => {
    expect(refreshTip(TODAY, false, NOW)).toBe(
      `Refresh · last synced ${formatClock(new Date(TODAY))}`,
    );
  });

  it("gives the date and the clock for an earlier day, so the exact moment stays reachable", () => {
    expect(refreshTip(EARLIER, false, NOW)).toBe(
      `Refresh · last synced ${formatDateTime(EARLIER)}`,
    );
    // The date is the app's DD-MM, the current year dropped.
    expect(refreshTip(EARLIER, false, NOW)).toContain("10-11");
  });

  it("says never synced for no stamp, and just Refresh for one it can't read", () => {
    expect(refreshTip(null, false, NOW)).toBe("Refresh · never synced");
    expect(refreshTip("not a date", false, NOW)).toBe("Refresh");
  });

  it("says Refreshing… while a sync runs, whatever the stamp", () => {
    expect(refreshTip(TODAY, true, NOW)).toBe("Refreshing…");
    expect(refreshTip(null, true, NOW)).toBe("Refreshing…");
  });
});

describe("refreshDescription", () => {
  it("is the same fact without the button's name", () => {
    expect(refreshDescription(TODAY, false, NOW)).toBe(
      `Last synced ${formatClock(new Date(TODAY))}`,
    );
    expect(refreshDescription(EARLIER, false, NOW)).toBe(`Last synced ${formatDateTime(EARLIER)}`);
    expect(refreshDescription(null, false, NOW)).toBe("Never synced");
    expect(refreshDescription(TODAY, true, NOW)).toBe("Refreshing…");
  });

  it("says nothing for a stamp it can't read", () => {
    expect(refreshDescription("not a date", false, NOW)).toBe("");
  });
});
