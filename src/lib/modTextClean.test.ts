// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The one short form that credits a mod's makers: people first, then the publisher.

import { describe, expect, it } from "vitest";
import { madeByLine, makerCredit } from "./modTextClean";

const RLO = "‮";

describe("makerCredit", () => {
  it("puts the people first and the publisher after them", () => {
    expect(makerCredit(["Bobby Yu"], "Atlas")).toBe("Bobby Yu · Atlas");
    expect(makerCredit(["Bobby Yu", "Ann Lee"], "Atlas")).toBe("Bobby Yu, Ann Lee · Atlas");
  });

  it("falls back to the publisher alone when no one is named", () => {
    expect(makerCredit([], "Atlas")).toBe("Atlas");
    expect(makerCredit(undefined, "Atlas")).toBe("Atlas");
  });

  it("is empty when there is neither, and skips names that clean to nothing", () => {
    expect(makerCredit([], "")).toBe("");
    expect(makerCredit([`${RLO}`, "  "], "Atlas")).toBe("Atlas");
    expect(makerCredit(["Bobby Yu"], "")).toBe("Bobby Yu");
  });

  it("cleans the mod's own words and reads at most eight names", () => {
    expect(makerCredit([`Bo${RLO}bby`], "Atlas")).toBe("Bo bby · Atlas");
    const nine = ["a", "b", "c", "d", "e", "f", "g", "h", "i"];
    expect(makerCredit(nine, "Atlas")).toBe("a, b, c, d, e, f, g, h · Atlas");
  });
});

describe("madeByLine", () => {
  it("says who made it, or that no maker is named", () => {
    expect(madeByLine(["Bobby Yu"], "Atlas")).toBe("Made by Bobby Yu · Atlas");
    expect(madeByLine([], "Atlas")).toBe("Made by Atlas");
    expect(madeByLine([], "")).toBe("Maker not named");
  });
});
