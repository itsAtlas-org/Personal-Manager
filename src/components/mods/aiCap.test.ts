// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";

import { MAX_AI_CAP_USD } from "../../lib/modsTypes";
import { formatUsd, parseCapUsd } from "./aiCap";

describe("parseCapUsd", () => {
  it("reads plain amounts, 0 included, rounded to a whole cent", () => {
    expect(parseCapUsd("1")).toBe(1);
    expect(parseCapUsd("1.00")).toBe(1);
    expect(parseCapUsd(" 2.5 ")).toBe(2.5);
    expect(parseCapUsd("0")).toBe(0);
    expect(parseCapUsd("0.00")).toBe(0);
    expect(parseCapUsd(".5")).toBe(0.5);
    expect(parseCapUsd("3.")).toBe(3);
    expect(parseCapUsd("1.006")).toBe(1.01);
    expect(parseCapUsd("0.004")).toBe(0);
    expect(parseCapUsd(String(MAX_AI_CAP_USD))).toBe(MAX_AI_CAP_USD);
  });

  it("refuses everything that is not a plain non-negative amount", () => {
    for (const bad of [
      "",
      " ",
      "-1",
      "-0",
      "+1",
      "abc",
      "1,000",
      "$1",
      "1e3",
      "1e999",
      "Infinity",
      "NaN",
      "0x10",
      "1 2",
      "1.2.3",
      ".",
      "٣",
      String(MAX_AI_CAP_USD + 1),
      "1" + "0".repeat(400),
    ]) {
      expect(parseCapUsd(bad), JSON.stringify(bad)).toBeNull();
    }
  });
});

describe("formatUsd", () => {
  it("shows two decimals, and four for a real but tiny sum", () => {
    expect(formatUsd(1)).toBe("1.00");
    expect(formatUsd(0)).toBe("0.00");
    expect(formatUsd(12.345)).toBe("12.35");
    expect(formatUsd(0.0042)).toBe("0.0042");
    expect(formatUsd(Number.NaN)).toBe("—");
  });
});
