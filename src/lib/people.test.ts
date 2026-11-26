// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The People tab's pure helpers: initials, the circle's token colour, the strip's next-30-days pick
// (across the year turn and 29 February), the sort preference. Invented people only.

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  birthdayText,
  errorWords,
  foldName,
  idsBetween,
  labelCoverage,
  parseBornOn,
  PERSON_TOKENS,
  personCircleBackground,
  personInitials,
  personToken,
  readPeopleSort,
  secondaryLine,
  upcomingBirthdays,
  visibleLabels,
  writePeopleSort,
} from "./people";
import type { PersonRow } from "./types";

function row(id: number, name: string, birthday: string | null, over: Partial<PersonRow> = {}) {
  return {
    id,
    uid: `uid${id}`.padEnd(32, "0"),
    display_name: name,
    primary: null,
    company: null,
    labels: [],
    starred: false,
    last_met: null,
    google_state: null,
    trashed_at: null,
    birthday,
    ...over,
  } satisfies PersonRow;
}

afterEach(() => localStorage.clear());

describe("personInitials", () => {
  it("takes the first and last words' letters", () => {
    expect(personInitials("Ada Lovelace")).toBe("AL");
    expect(personInitials("ada byron lovelace")).toBe("AL");
  });

  it("gives one letter for a one-word name", () => {
    expect(personInitials("Cher")).toBe("C");
    expect(personInitials("Example Ltd")).toBe("EL");
  });

  it("keeps a whole character of a script without spaces or outside the BMP", () => {
    expect(personInitials("山田太郎")).toBe("山");
    expect(personInitials("Éloïse Ñandú")).toBe("ÉÑ");
    expect(personInitials("𠮷野")).toBe("𠮷");
  });

  it("skips leading punctuation and falls back to ? with no letter at all", () => {
    expect(personInitials('"Bea" Quinn')).toBe("BQ");
    expect(personInitials("  ")).toBe("?");
    expect(personInitials("+++")).toBe("?");
  });
});

describe("the circle's colour", () => {
  it("is a token from the fixed palette, the same for the same uid", () => {
    const a = personToken("0123456789abcdef0123456789abcdef");
    expect(PERSON_TOKENS).toContain(a);
    expect(personToken("0123456789abcdef0123456789abcdef")).toBe(a);
  });

  it("spreads people over the palette", () => {
    const seen = new Set<string>();
    for (let i = 0; i < 60; i++) seen.add(personToken(`uid-${i}`));
    expect(seen.size).toBeGreaterThan(3);
  });

  it("holds no hex anywhere: every token is a custom property and the background a token mix", () => {
    for (const t of PERSON_TOKENS) expect(t).toMatch(/^--[a-z-]+$/);
    expect(personCircleBackground("uid-1")).toMatch(
      /^color-mix\(in oklab, var\(--[a-z-]+\) \d+%, var\(--bg\)\)$/,
    );
    expect(personCircleBackground("uid-1")).not.toMatch(/#[0-9a-f]{3,8}/i);
  });
});

describe("what a row says", () => {
  it("prefers the company, then the primary email or phone", () => {
    expect(secondaryLine({ company: "Example Ltd", primary: "ada@example.com" })).toBe(
      "Example Ltd",
    );
    expect(secondaryLine({ company: null, primary: "ada@example.com" })).toBe("ada@example.com");
    expect(secondaryLine({ company: "  ", primary: null })).toBe("");
  });

  it("shows three chips and counts the rest", () => {
    const labels = [1, 2, 3, 4, 5].map((id) => ({ id, name: `L${id}` }));
    expect(visibleLabels(labels)).toEqual({ shown: labels.slice(0, 3), more: 2 });
    expect(visibleLabels(labels.slice(0, 2))).toEqual({ shown: labels.slice(0, 2), more: 0 });
  });
});

describe("birthdayText", () => {
  const today = new Date(2026, 10, 26);
  it("words a year-less birthday as day and month", () => {
    expect(birthdayText("--04-15", today)).toBe("Birthday 15 April");
    expect(birthdayText("--02-29", today)).toBe("Birthday 29 February");
  });

  it("words a full one as DD-MM-YYYY with the age", () => {
    expect(birthdayText("1990-04-15", today)).toBe("Birthday 15-04-1990 (36)");
    // Not yet had this year: one younger.
    expect(birthdayText("1990-12-25", today)).toBe("Birthday 25-12-1990 (35)");
    // The day itself counts as had.
    expect(birthdayText("1990-11-26", today)).toBe("Birthday 26-11-1990 (36)");
  });

  it("says nothing for no birthday or one that isn't a date", () => {
    expect(birthdayText(null, today)).toBeNull();
    expect(birthdayText("1990-04", today)).toBeNull();
    expect(birthdayText("--13-01", today)).toBeNull();
    expect(birthdayText("1991-02-29", today)).toBeNull();
  });
});

describe("parseBornOn", () => {
  it("accepts a year-less 29 February and refuses a common year's", () => {
    expect(parseBornOn("--02-29")).toEqual({ year: null, month: 2, day: 29 });
    expect(parseBornOn("2000-02-29")).toEqual({ year: 2000, month: 2, day: 29 });
    expect(parseBornOn("2001-02-29")).toBeNull();
    expect(parseBornOn("--04-31")).toBeNull();
  });
});

describe("upcomingBirthdays", () => {
  const names = (list: ReturnType<typeof upcomingBirthdays>) => list.map((u) => u.row.display_name);

  it("picks the next 30 days, today included, soonest first", () => {
    const today = new Date(2026, 3, 1);
    const list = upcomingBirthdays(
      [
        row(1, "Cara", "--05-01"), // 30 days: in
        row(2, "Ben", "--04-01"), // today: in
        row(3, "Dee", "--05-02"), // 31 days: out
        row(4, "Abe", "--04-10"),
        row(5, "Eli", "--03-31"), // yesterday: nearly a year off
        row(6, "Fay", null),
      ],
      today,
    );
    expect(names(list)).toEqual(["Ben", "Abe", "Cara"]);
    expect(list.map((u) => u.inDays)).toEqual([0, 9, 30]);
    expect(list[1].text).toBe("Birthday 10 April");
  });

  it("carries over the year turn", () => {
    const today = new Date(2026, 11, 20);
    const list = upcomingBirthdays(
      [row(1, "Gus", "--01-05"), row(2, "Hal", "1985-12-31"), row(3, "Ivy", "--01-25")],
      today,
    );
    expect(names(list)).toEqual(["Hal", "Gus"]);
    expect(list[0].text).toBe("Birthday 31 December, turns 41");
    expect(list[1].inDays).toBe(16);
  });

  it("says no age for a birth year that has not happened yet", () => {
    const list = upcomingBirthdays([row(1, "Kit", "2999-04-15")], new Date(2026, 3, 1));
    expect(list.map((u) => u.text)).toEqual(["Birthday 15 April"]);
  });

  it("holds a 29 February on 1 March in a common year and on the day in a leap one", () => {
    const common = upcomingBirthdays([row(1, "Jo", "--02-29")], new Date(2027, 1, 20));
    expect(common.map((u) => u.inDays)).toEqual([9]); // 1 March 2027
    const leap = upcomingBirthdays([row(1, "Jo", "--02-29")], new Date(2028, 1, 20));
    expect(leap.map((u) => u.inDays)).toEqual([9]); // 29 February 2028
    // After 1 March in a common year it is a year off.
    expect(upcomingBirthdays([row(1, "Jo", "--02-29")], new Date(2027, 2, 2))).toEqual([]);
    // Across the year turn from a leap year's 29 February.
    expect(upcomingBirthdays([row(1, "Jo", "2000-02-29")], new Date(2026, 1, 10))[0].text).toBe(
      "Birthday 29 February, turns 26",
    );
  });

  it("never lists anyone in Trash", () => {
    const today = new Date(2026, 3, 1);
    expect(
      upcomingBirthdays([row(1, "Kit", "--04-02", { trashed_at: "2026-03-30" })], today),
    ).toEqual([]);
  });
});

describe("the sort preference", () => {
  it("defaults to first name and remembers a choice", () => {
    expect(readPeopleSort()).toBe("First");
    writePeopleSort("RecentlyMet");
    expect(readPeopleSort()).toBe("RecentlyMet");
  });

  it("ignores a stored value it doesn't know", () => {
    localStorage.setItem("pm.people.sort", "Shoe size");
    expect(readPeopleSort()).toBe("First");
  });

  it("survives storage that throws", () => {
    const get = vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    const set = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(readPeopleSort()).toBe("First");
    expect(() => writePeopleSort("Last")).not.toThrow();
    get.mockRestore();
    set.mockRestore();
  });
});

describe("selection helpers", () => {
  const rows = [1, 2, 3, 4, 5].map((id) => ({ id }));
  it("takes a range in either direction, ends included", () => {
    expect(idsBetween(rows, 2, 4)).toEqual([2, 3, 4]);
    expect(idsBetween(rows, 4, 2)).toEqual([2, 3, 4]);
  });

  it("falls back to the one clicked with no anchor or a vanished one", () => {
    expect(idsBetween(rows, null, 3)).toEqual([3]);
    expect(idsBetween(rows, 99, 3)).toEqual([3]);
  });

  it("splits labels into those all carry and those only some do", () => {
    const a = row(1, "A", null, {
      labels: [
        { id: 1, name: "x" },
        { id: 2, name: "y" },
      ],
    });
    const b = row(2, "B", null, { labels: [{ id: 1, name: "x" }] });
    const { all, some } = labelCoverage([a, b]);
    expect([...all]).toEqual([1]);
    expect([...some]).toEqual([2]);
  });
});

describe("errorWords", () => {
  it("shows the backend's sentence as it is, and never an Error's own text", () => {
    expect(errorWords("PM couldn't move them just now.")).toBe("PM couldn't move them just now.");
    // An Error could carry anything, a contact's value included: it never reaches the screen.
    expect(errorWords(new Error("ada@example.com"))).toBe("PM couldn't do that. Try again.");
    expect(errorWords("", "PM couldn't make that label. Try again.")).toBe(
      "PM couldn't make that label. Try again.",
    );
    expect(errorWords(undefined)).toBe("PM couldn't do that. Try again.");
  });
});

describe("foldName", () => {
  it("folds accents, case and spacing as the backend's fold_key does", () => {
    expect(foldName("Café")).toBe("cafe");
    expect(foldName("  Close   Friends ")).toBe(foldName("close friends"));
    expect(foldName("Cafe")).toBe(foldName("Café"));
    // A mark that changes the letter is kept, as in Rust: ば never folds into は.
    expect(foldName("ば")).not.toBe(foldName("は"));
  });
});
