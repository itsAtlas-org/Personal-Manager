// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The partial-date helpers against the Rust encoding's own examples (`people/partial_date.rs` tests):
// what the backend stores, this reads and writes back exactly, and what it refuses this refuses too.

import { describe, expect, it } from "vitest";
import {
  ageOn,
  birthdayIn,
  describeBirthday,
  describeBirthdayChoice,
  fieldsFromIso,
  formatPartial,
  isLeapDayBirthday,
  isoFromFields,
  parsePartial,
  toDisplay,
  toIso,
} from "./partialDate";

describe("parsePartial", () => {
  it("round-trips every stored shape, as the Rust test does", () => {
    for (const s of ["2001-04-15", "--04-15", "2001-04", "2001", "--02-29"]) {
      const d = parsePartial(s);
      expect(d, s).not.toBeNull();
      expect(toIso(d!)).toBe(s);
    }
  });

  it("refuses what is not a real date, the Rust list verbatim", () => {
    for (const s of [
      "2023-02-29",
      "--02-30",
      "--13-01",
      "2001-04-31",
      "2001-00",
      "0000",
      "01-04-2001",
      "2001-4-5",
      "",
      "--",
      "--04",
    ]) {
      expect(parsePartial(s), JSON.stringify(s)).toBeNull();
    }
    expect(parsePartial("2024-02-29")).not.toBeNull();
    expect(parsePartial("2000-02-29")).not.toBeNull();
    expect(parsePartial("1900-02-29")).toBeNull();
  });

  it("reads nothing from a missing or padded-with-junk value", () => {
    expect(parsePartial(null)).toBeNull();
    expect(parsePartial(undefined)).toBeNull();
    expect(parsePartial("2001-04-05-06")).toBeNull();
    expect(parsePartial("--04-15-01")).toBeNull();
    expect(parsePartial("2001-04-0x")).toBeNull();
    expect(parsePartial(" 2001-04-05 ")).not.toBeNull();
  });
});

describe("toDisplay", () => {
  it("follows the house format, as the Rust test does", () => {
    expect(toDisplay(parsePartial("2001-04-05")!)).toBe("05-04-2001");
    expect(toDisplay(parsePartial("--04-15")!)).toBe("15 April");
    expect(toDisplay(parsePartial("2001")!)).toBe("2001");
    expect(toDisplay(parsePartial("2001-04")!)).toBe("04-2001");
  });
});

describe("formatPartial", () => {
  it("shows a part-date as it is and leaves a non-date as text", () => {
    expect(formatPartial("2001")).toBe("2001");
    expect(formatPartial("2001-04")).toBe("04-2001");
    expect(formatPartial("--04-15")).toBe("15 April");
    expect(formatPartial("not a date")).toBe("not a date");
    expect(formatPartial(null)).toBe("");
  });

  it("keeps a full day's year, in the current year too", () => {
    const year = new Date().getFullYear();
    expect(formatPartial(`${year}-04-05`)).toBe(`05-04-${year}`);
    expect(formatPartial("2001-04-05")).toBe("05-04-2001");
  });
});

describe("ages and birthdays", () => {
  const today = new Date(2026, 10, 26); // 26 November 2026

  it("counts the age as of today, the day itself counting as had", () => {
    expect(ageOn(parsePartial("1990-04-15")!, today)).toBe(36);
    expect(ageOn(parsePartial("1990-12-25")!, today)).toBe(35);
    expect(ageOn(parsePartial("1990-11-26")!, today)).toBe(36);
  });

  it("has no age without a year, or for a year still ahead", () => {
    expect(ageOn(parsePartial("--04-15")!, today)).toBeNull();
    expect(ageOn(parsePartial("2999-04-15")!, today)).toBeNull();
  });

  it("words a year-less birthday as day and month, and a full one with its year and age", () => {
    expect(describeBirthday("--04-15", today)).toBe("15 April");
    expect(describeBirthday("--02-29", today)).toBe("29 February");
    // Always the year, even in the current one.
    expect(describeBirthday("2026-01-02", today)).toBe("02-01-2026 (0)");
    expect(describeBirthday("1990-04-15", today)).toBe("15-04-1990 (36)");
    expect(describeBirthday("2999-04-15", today)).toBe("15-04-2999");
  });

  it("says nothing for no birthday or one that is not a day", () => {
    expect(describeBirthday(null, today)).toBeNull();
    expect(describeBirthday("1990-04", today)).toBeNull();
    expect(describeBirthday("1990", today)).toBeNull();
    expect(describeBirthday("1991-02-29", today)).toBeNull();
  });
});

describe("a 29 February birthday in other years (E22)", () => {
  const born = parsePartial("2000-02-29")!;
  const dayIn = (leap: "feb28" | "mar1" | null, year: number) => birthdayIn(born, leap, year);

  it("lands on the chosen day in a common year, and on itself in a leap one", () => {
    // The same table as Rust's `a_29_february_birthday_lands_on_the_chosen_day_in_common_years`.
    expect(dayIn("feb28", 2027)).toEqual({ month: 2, day: 28 });
    expect(dayIn("mar1", 2027)).toEqual({ month: 3, day: 1 });
    expect(dayIn(null, 2027)).toEqual({ month: 2, day: 28 });
    for (const leap of ["feb28", "mar1", null] as const) {
      expect(dayIn(leap, 2028)).toEqual({ month: 2, day: 29 });
    }
    // 1900 is not a leap year (divisible by 100, not 400); 2000 is.
    expect(dayIn("mar1", 1900)).toEqual({ month: 3, day: 1 });
    expect(dayIn("mar1", 2000)).toEqual({ month: 2, day: 29 });
  });

  it("leaves every other birthday alone, whatever answer is passed", () => {
    expect(birthdayIn(parsePartial("--04-15")!, "mar1", 2027)).toEqual({ month: 4, day: 15 });
    expect(birthdayIn(parsePartial("1990")!, null, 2027)).toBeNull();
  });

  it("turns a leapling a year older on the chosen day", () => {
    const eve = new Date(2027, 1, 27);
    const feb28 = new Date(2027, 1, 28);
    const mar1 = new Date(2027, 2, 1);
    expect(ageOn(born, eve, "feb28")).toBe(26);
    expect(ageOn(born, feb28, "feb28")).toBe(27);
    expect(ageOn(born, feb28, null)).toBe(27);
    expect(ageOn(born, feb28, "mar1")).toBe(26);
    expect(ageOn(born, mar1, "mar1")).toBe(27);
    expect(describeBirthday("2000-02-29", feb28, "mar1")).toBe("29-02-2000 (26)");
    expect(describeBirthday("2000-02-29", mar1, "mar1")).toBe("29-02-2000 (27)");
  });

  it("says which birthdays are 29 February", () => {
    expect(isLeapDayBirthday("--02-29")).toBe(true);
    expect(isLeapDayBirthday("2000-02-29")).toBe(true);
    expect(isLeapDayBirthday("1990-02-28")).toBe(false);
    expect(isLeapDayBirthday("--03-01")).toBe(false);
    expect(isLeapDayBirthday(null)).toBe(false);
  });

  it("shows the answer beside a merge's birthday, only for a 29 February one", () => {
    const today = new Date(2026, 10, 26);
    expect(describeBirthdayChoice("--02-29", today, "mar1")).toBe(
      "29 February, marked on 1 March in other years",
    );
    expect(describeBirthdayChoice("--02-29", today, null)).toBe("29 February");
    expect(describeBirthdayChoice("--04-15", today, "mar1")).toBe("15 April");
  });
});

describe("the three fields", () => {
  it("fills them from a stored date, and leaves them empty for none", () => {
    expect(fieldsFromIso("2001-04-05")).toEqual({ day: "05", month: "04", year: "2001" });
    expect(fieldsFromIso("--04-15")).toEqual({ day: "15", month: "04", year: "" });
    expect(fieldsFromIso("2001-04")).toEqual({ day: "", month: "04", year: "2001" });
    expect(fieldsFromIso(null)).toEqual({ day: "", month: "", year: "" });
    expect(fieldsFromIso("junk")).toEqual({ day: "", month: "", year: "" });
  });

  it("turns typed fields into the stored text, padding single digits", () => {
    expect(isoFromFields({ day: "5", month: "4", year: "2001" }, "born")).toEqual({
      iso: "2001-04-05",
      error: null,
    });
    expect(isoFromFields({ day: "15", month: "4", year: "" }, "born")).toEqual({
      iso: "--04-15",
      error: null,
    });
    expect(isoFromFields({ day: "", month: "", year: "2019" }, "dated")).toEqual({
      iso: "2019",
      error: null,
    });
    expect(isoFromFields({ day: "", month: "3", year: "2019" }, "dated")).toEqual({
      iso: "2019-03",
      error: null,
    });
    expect(isoFromFields({ day: "29", month: "2", year: "" }, "any")).toEqual({
      iso: "--02-29",
      error: null,
    });
  });

  it("takes empty fields as no date, never an error", () => {
    for (const mode of ["born", "dated", "any"] as const) {
      expect(isoFromFields({ day: " ", month: "", year: "" }, mode)).toEqual({
        iso: null,
        error: null,
      });
    }
  });

  it("says in words what is wrong", () => {
    expect(isoFromFields({ day: "30", month: "2", year: "2001" }, "born").error).toBe(
      "That isn't a real date.",
    );
    expect(isoFromFields({ day: "1", month: "1", year: "99" }, "born").error).toBe(
      "Use a four-digit year.",
    );
    expect(isoFromFields({ day: "x", month: "1", year: "" }, "born").error).toBe(
      "Use numbers only.",
    );
    // A birthday needs its day and month; a met-on needs its year; either shape does for "any".
    expect(isoFromFields({ day: "", month: "", year: "1990" }, "born").error).toMatch(
      /day and a month/,
    );
    expect(isoFromFields({ day: "15", month: "4", year: "" }, "dated").error).toMatch(/year first/);
    expect(isoFromFields({ day: "15", month: "", year: "" }, "any").error).toMatch(
      /year, or a day/,
    );
    expect(isoFromFields({ day: "15", month: "", year: "2001" }, "dated").error).toMatch(
      /year first/,
    );
  });
});
