// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The one short form that credits a mod's makers: the publisher first, then the people the mod
// names, under a label that says they are the mod's own claim.

import { describe, expect, it } from "vitest";
import { madeByLine, makerCredit } from "./modTextClean";

const RLO = "‮";

describe("makerCredit", () => {
  it("puts the publisher first and the people after it, as the mod's own claim", () => {
    expect(makerCredit(["Bobby Yu"], "Atlas")).toBe("Atlas · author: Bobby Yu");
    expect(makerCredit(["Bobby Yu", "Ann Lee"], "Atlas")).toBe(
      "Atlas · authors: Bobby Yu, Ann Lee",
    );
  });

  it("never lets a credited name stand where the publisher does", () => {
    // A mod can credit anyone, "Atlas" included, and put the separator in a name.
    expect(makerCredit(["Atlas"], "Evil Co")).toBe("Evil Co · author: Atlas");
    expect(makerCredit(["Bobby Yu · Atlas"], "Evil Co")).toBe("Evil Co · author: Bobby Yu · Atlas");
    expect(madeByLine(["Atlas"], "Evil Co")).toBe("Made by Evil Co · author: Atlas");
    for (const credit of [
      makerCredit(["Atlas"], "Evil Co"),
      makerCredit(["x, Atlas"], "Evil Co"),
    ]) {
      expect(credit.startsWith("Evil Co · author: ")).toBe(true);
    }
  });

  it("falls back to the publisher alone when no one is named", () => {
    expect(makerCredit([], "Atlas")).toBe("Atlas");
    expect(makerCredit(undefined, "Atlas")).toBe("Atlas");
  });

  it("is empty when there is neither, and skips names that clean to nothing", () => {
    expect(makerCredit([], "")).toBe("");
    expect(makerCredit([`${RLO}`, "  "], "Atlas")).toBe("Atlas");
    expect(makerCredit(["Bobby Yu"], "")).toBe("an unnamed publisher · author: Bobby Yu");
  });

  it("cleans the mod's own words and reads at most eight names", () => {
    expect(makerCredit([`Bo${RLO}bby`], "Atlas")).toBe("Atlas · author: Bo bby");
    const nine = ["a", "b", "c", "d", "e", "f", "g", "h", "i"];
    expect(makerCredit(nine, "Atlas")).toBe("Atlas · authors: a, b, c, d, e, f, g, h");
  });
});

describe("madeByLine", () => {
  it("says who made it, or that no maker is named", () => {
    expect(madeByLine(["Bobby Yu"], "Atlas")).toBe("Made by Atlas · author: Bobby Yu");
    expect(madeByLine([], "Atlas")).toBe("Made by Atlas");
    expect(madeByLine([], "")).toBe("Maker not named");
  });
});
