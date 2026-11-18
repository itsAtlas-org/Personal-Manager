// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The card's date, held to the core's `formatDate` (src/lib/format.test.ts pins that one): DD-MM-YYYY,
// the year dropped in the current year, junk left as it is. Dates go through a local Date at noon so
// the assertions hold in any time zone (the same trick the core's test uses).

import { describe, expect, it } from "vitest";
import { formatDate } from "./format";

const localIso = (y: number, m0: number, d: number) => new Date(y, m0, d, 12, 0, 0).toISOString();
const NOW = new Date(2026, 10, 17, 12);

describe("formatDate", () => {
  it("a past-year date is DD-MM-YYYY", () => {
    expect(formatDate(localIso(2024, 2, 5), NOW)).toBe("05-03-2024");
  });

  it("a future-year date keeps its year too", () => {
    expect(formatDate(localIso(2027, 0, 31), NOW)).toBe("31-01-2027");
  });

  it("a date in the current year drops the year", () => {
    expect(formatDate(localIso(2026, 0, 9), NOW)).toBe("09-01");
    expect(formatDate(localIso(2026, 11, 31), NOW)).toBe("31-12");
  });

  it("pads the day and the month", () => {
    expect(formatDate(localIso(2020, 8, 3), NOW)).toBe("03-09-2020");
  });

  it("the current year is the clock's, not a fixed one", () => {
    expect(formatDate(localIso(2026, 5, 1), new Date(2027, 0, 1))).toBe("01-06-2026");
  });

  it("a value that is not a date is shown as it is", () => {
    expect(formatDate("not a date", NOW)).toBe("not a date");
    expect(formatDate("", NOW)).toBe("");
  });

  it("defaults to the real clock", () => {
    const year = new Date().getFullYear();
    expect(formatDate(localIso(year, 0, 15))).toBe("15-01");
    expect(formatDate(localIso(year - 1, 0, 15))).toBe(`15-01-${year - 1}`);
  });
});
