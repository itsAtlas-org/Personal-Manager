// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import { zoneOption, allZoneOptions, cityOf, gutterZoneCode } from "./zoneLabel";

// A code is either a real abbreviation (EDT) or a normalised UTC offset — never assert the exact
// value since it is DST/season dependent.
const CODE_RE = /^(UTC[+-]\d{2}:\d{2}|[A-Za-z]{2,5})$/;

describe("zoneOption", () => {
  it("formats as Continent / Country / City with a code", () => {
    const o = zoneOption("Europe/London");
    expect(o.label).toBe("Europe / United Kingdom / London");
    expect(o.code).toMatch(CODE_RE);
  });

  it("is searchable by continent, country and city", () => {
    const o = zoneOption("Asia/Kolkata");
    expect(o.label).toBe("Asia / India / Kolkata");
    for (const term of ["asia", "india", "kolkata"]) expect(o.search).toContain(term);
  });

  it("un-underscores multi-word cities and countries", () => {
    const o = zoneOption("America/New_York");
    expect(o.label).toBe("America / United States / New York");
    expect(o.search).toContain("new york");
    expect(o.search).toContain("united states");
  });

  it("drops the country segment when it just repeats the continent", () => {
    expect(zoneOption("Australia/Sydney").label).toBe("Australia / Sydney");
  });

  it("falls back to Continent / City for a zone with no country", () => {
    const o = zoneOption("Etc/UTC");
    expect(o.label).toBe("Etc / UTC");
    expect(o.code).toMatch(CODE_RE);
  });

  it("maps a deprecated alias to its country too", () => {
    // Asia/Calcutta is the old spelling of Asia/Kolkata — still exposed by the runtime.
    expect(zoneOption("Asia/Calcutta").search).toContain("india");
  });

  it("names a legacy id by its modern city, and finds it by that name", () => {
    expect(cityOf("Asia/Calcutta")).toBe("Kolkata");
    expect(cityOf("Europe/Kiev")).toBe("Kyiv");
    expect(cityOf("America/New_York")).toBe("New York");
    expect(zoneOption("Asia/Calcutta").label).toBe("Asia / India / Kolkata");
    expect(zoneOption("Asia/Calcutta").search).toContain("kolkata");
    // The id's own spelling still finds it.
    expect(zoneOption("Asia/Calcutta").search).toContain("calcutta");
  });

  // Every legacy spelling in the map, so a typo in any one shows up here.
  it.each([
    ["Asia/Calcutta", "Kolkata"],
    ["Asia/Katmandu", "Kathmandu"],
    ["Asia/Saigon", "Ho Chi Minh"],
    ["Asia/Rangoon", "Yangon"],
    ["Europe/Kiev", "Kyiv"],
    ["America/Godthab", "Nuuk"],
  ])("names %s %s", (id, city) => {
    expect(cityOf(id)).toBe(city);
    expect(zoneOption(id).label.endsWith(city)).toBe(true);
  });
});

describe("gutterZoneCode", () => {
  const jan = new Date(2026, 0, 15, 12);
  const jul = new Date(2026, 6, 15, 12);

  it("is the runtime's offset where the zone has no letter abbreviation", () => {
    expect(gutterZoneCode("Asia/Kolkata", jan)).toBe("GMT+5:30");
    expect(gutterZoneCode("Asia/Kathmandu", jan)).toBe("GMT+5:45");
  });

  it("follows the date given, so a week across a clock change is right", () => {
    expect(gutterZoneCode("Europe/London", jan)).toBe("GMT");
    expect(gutterZoneCode("Europe/London", jul)).toBe("GMT+1");
  });

  it("uses a real abbreviation when the platform has one", () => {
    expect(gutterZoneCode("America/New_York", jan)).toBe("EST");
    expect(gutterZoneCode("America/New_York", jul)).toBe("EDT");
  });

  it("is empty for an id the runtime rejects, without throwing", () => {
    expect(gutterZoneCode("Not/AZone", jan)).toBe("");
  });
});

describe("allZoneOptions", () => {
  it("returns one memoised option per runtime zone, including London", () => {
    const all = allZoneOptions();
    expect(all.length).toBeGreaterThan(100);
    expect(all.find((o) => o.id === "Europe/London")).toBeTruthy();
    expect(allZoneOptions()).toBe(all); // cached, stable reference
  });
});
