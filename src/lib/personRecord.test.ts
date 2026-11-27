// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What a record may open, and how it words last met, a job's span and a source. Invented values only.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  agoText,
  copiedWords,
  linkTarget,
  lastMetLine,
  mailTarget,
  rangeText,
  sourceLine,
} from "./personRecord";

describe("linkTarget", () => {
  it("opens only http, https and mailto", () => {
    expect(linkTarget("https://example.com/ada")).toBe("https://example.com/ada");
    expect(linkTarget("http://example.com")).toBe("http://example.com");
    expect(linkTarget("HTTPS://example.com")).toBe("HTTPS://example.com");
    expect(linkTarget("mailto:ada@example.com")).toBe("mailto:ada@example.com");
  });

  it("refuses every other scheme, however it is written", () => {
    for (const v of [
      "javascript:alert(1)",
      "JaVaScRiPt:alert(1)",
      " javascript:alert(1)",
      "data:text/html,<b>x</b>",
      "file:///etc/hosts",
      "tel:5550100",
      "ftp://example.com",
      "vbscript:x",
      "//example.com",
      "http:example.com",
      "https:///",
      "https://",
      "mailto:",
    ]) {
      expect(linkTarget(v), v).toBeNull();
    }
  });

  it("leaves a handle, a bare domain and text with a space as text", () => {
    expect(linkTarget("@ada_example")).toBeNull();
    expect(linkTarget("example.com/ada")).toBeNull();
    expect(linkTarget("https://example.com/a b")).toBeNull();
    expect(linkTarget("https://example.com/\u0000")).toBeNull();
    expect(linkTarget("")).toBeNull();
  });
});

describe("mailTarget", () => {
  it("makes a mail link only from a plain address", () => {
    expect(mailTarget("ada@example.com")).toBe("mailto:ada@example.com");
    expect(mailTarget("ada@example.com?cc=ben@example.com")).toBeNull();
    expect(mailTarget("ada@example.com&bcc=x")).toBeNull();
    expect(mailTarget("not an address")).toBeNull();
    expect(mailTarget("ada")).toBeNull();
  });
});

describe("words", () => {
  const today = new Date(2026, 10, 26);

  // format.ts drops the year in the current year, so the clock is pinned to the test's own "today".
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2026, 10, 26, 12));
  });
  afterEach(() => vi.useRealTimers());

  it("says how long ago", () => {
    expect(agoText(new Date(2026, 10, 26), today)).toBe("today");
    expect(agoText(new Date(2026, 10, 25), today)).toBe("yesterday");
    expect(agoText(new Date(2026, 10, 16), today)).toBe("10 days ago");
    expect(agoText(new Date(2026, 10, 5), today)).toBe("3 weeks ago");
    // Day 59 and day 60 around the weeks-to-months switch: never "1 months", never backwards.
    expect(agoText(new Date(2026, 8, 28), today)).toBe("8 weeks ago");
    expect(agoText(new Date(2026, 8, 27), today)).toBe("8 weeks ago");
    expect(agoText(new Date(2026, 8, 26), today)).toBe("2 months ago");
    expect(agoText(new Date(2026, 7, 26), today)).toBe("3 months ago");
    expect(agoText(new Date(2023, 10, 26), today)).toBe("3 years ago");
    expect(agoText(new Date(2026, 10, 27), today)).toBeNull();
  });

  it("words last met, or says they haven't been met", () => {
    expect(lastMetLine(null, today)).toBe("Not met yet");
    expect(lastMetLine("2025-11-12", today)).toBe("Last met 12-11-2025 (a year ago)");
    // This year: format.ts drops it.
    expect(lastMetLine("2026-11-12", today)).toBe("Last met 12-11 (2 weeks ago)");
    expect(lastMetLine("2026-11-12T09:30:00Z", today)).toBe("Last met 12-11 (2 weeks ago)");
  });

  it("words a job's span", () => {
    const base = { kind: "work" as const, from_date: null, to_date: null, is_current: false };
    expect(rangeText({ ...base, from_date: "2019-03", is_current: true })).toBe("03-2019 to now");
    expect(rangeText({ ...base, from_date: "2008", to_date: "2012" })).toBe("2008 to 2012");
    expect(rangeText({ ...base, from_date: "2019" })).toBe("from 2019");
    expect(rangeText({ ...base, to_date: "2012" })).toBe("until 2012");
    expect(rangeText({ ...base, is_current: true })).toBe("current");
    expect(rangeText({ ...base, to_date: "2012", is_current: true })).toBe("until 2012");
    expect(rangeText(base)).toBe("");
    expect(rangeText({ ...base, kind: "location", from_date: "2026-03" })).toBe("since 03-2026");
  });

  it("words a source, saying nothing invented about a provider it doesn't know", () => {
    expect(sourceLine({ provider: "vcard", created_at: "2025-11-12T12:00:00" })).toBe(
      "Imported from a file on 12-11-2025",
    );
    expect(sourceLine({ provider: "google", created_at: "2025-11-12T12:00:00" })).toBe(
      "Linked from another address book on 12-11-2025",
    );
  });

  it("keeps the year of an import made this year", () => {
    const now = new Date();
    const iso = `${now.getFullYear()}-01-02T12:00:00`;
    expect(sourceLine({ provider: "vcard", created_at: iso })).toBe(
      `Imported from a file on 02-01-${now.getFullYear()}`,
    );
  });

  it("says what Copy copied", () => {
    expect(copiedWords("phone")).toBe("Phone number copied");
    expect(copiedWords("email")).toBe("Email address copied");
    expect(copiedWords("web")).toBe("Link copied");
    expect(copiedWords("custom")).toBe("Copied");
  });
});
