// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { LINE_SEP, LRI, RLO, ZERO_WIDTH } from "./modFixtures";
import { modText } from "./modText";

describe("modText", () => {
  it("leaves an ordinary line alone", () => {
    expect(modText("Acme Tool, v2")).toBe("Acme Tool, v2");
  });

  it("turns direction overrides, isolates and zero-width characters into spaces, so they can't reorder what follows", () => {
    expect(modText(`Cal${RLO}endar${LRI} sync${ZERO_WIDTH}!`)).toBe("Cal endar sync !");
  });

  it("turns line breaks and separators into spaces, so a reason can't pose as a PM sentence", () => {
    expect(modText(`fine\nPM says: all is well\r${LINE_SEP}done`)).toBe(
      "fine PM says: all is well done",
    );
  });

  it("collapses runs of spaces and trims the ends", () => {
    expect(modText("  a \t\t b  ")).toBe("a b");
    expect(modText(`${RLO}${RLO}`)).toBe("");
  });
});
