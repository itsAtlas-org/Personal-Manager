// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import {
  ACUTE,
  CIRCUMFLEX,
  LINE_SEP,
  LRI,
  MARK_ABOVE,
  MARK_BELOW,
  RLO,
  ZERO_WIDTH,
} from "./modFixtures";
import { modBody, modText, quoteName } from "./modText";

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

  it("cuts a run of combining marks to what a real script stacks, so one letter can't smear over the badges beside it", () => {
    const stacked = `A${MARK_ABOVE.repeat(39)}`;
    expect(modText(stacked)).toBe(`A${MARK_ABOVE.repeat(3)}`);
    // Marks above and below, mixed, are one run.
    expect(modText(`A${(MARK_ABOVE + MARK_BELOW).repeat(20)}`)).toBe(
      `A${MARK_ABOVE}${MARK_BELOW}${MARK_ABOVE}`,
    );
  });

  it("keeps the marks real text carries: an accent, two on one letter, Hebrew's pointing", () => {
    const acute = `e${ACUTE}cole`;
    expect(modText(acute)).toBe(acute);
    const vietnamese = `a${CIRCUMFLEX}${MARK_BELOW}ge`;
    expect(modText(vietnamese)).toBe(vietnamese);
    // A letter with up to three marks (Hebrew pointing is the most that real text stacks).
    const hebrew = ["ב", "ְ", "ּ", "ר", "ֵ"].join("");
    expect(modText(hebrew)).toBe(hebrew);
  });

  it("collapses runs of spaces and trims the ends", () => {
    expect(modText("  a \t\t b  ")).toBe("a b");
    expect(modText(`${RLO}${RLO}`)).toBe("");
  });
});

describe("quoteName", () => {
  it("puts the name in quotes, cleaned like every other mod text", () => {
    expect(quoteName(`Acme${RLO}  Tool\n`)).toBe("“Acme Tool”");
  });

  it("can't be closed from inside: a double quote in the name becomes a single one", () => {
    // Without this a name could end its own quote and carry on in PM's voice.
    expect(quoteName("Calendar” was removed. “Settings")).toBe(
      "“Calendar' was removed. 'Settings”",
    );
    expect(quoteName('say "hi"')).toBe("“say 'hi'”");
  });
});

describe("modBody", () => {
  it("keeps line breaks (a carriage return is one) and the spaces as written", () => {
    expect(modBody("One.\r\nTwo.\rThree.  Four.")).toBe("One.\nTwo.\nThree.  Four.");
  });

  it("turns invisible, direction and separator characters into spaces, like a plain line", () => {
    expect(modBody(`a${RLO}b${ZERO_WIDTH}c${LINE_SEP}d`)).toBe("a b c d");
  });

  it("cuts a run of combining marks, and a run of blank lines to one", () => {
    expect(modBody(`A${MARK_ABOVE.repeat(39)}`)).toBe(`A${MARK_ABOVE.repeat(3)}`);
    expect(modBody("a\n\n\n\n\nb")).toBe("a\n\nb");
  });

  it("trims the ends and has nothing left of nothing", () => {
    expect(modBody("\n  text \n")).toBe("text");
    expect(modBody(`${ZERO_WIDTH}\n `)).toBe("");
  });
});
