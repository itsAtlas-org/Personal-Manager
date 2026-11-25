// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import packageRs from "../../../src-tauri/src/mods/package.rs?raw";
import {
  CONFIRM_LINE_CHARS,
  CONFIRM_MAX_ADDRESS_LINES,
  CONFIRM_MAX_MARKS,
  cleanUserLine,
  readSummary,
} from "./confirmSummary";

describe("readSummary", () => {
  it("takes line 1 as the question and the rest as the address lines", () => {
    expect(readSummary("Open this address in your browser?\nhttps://a.example/x")).toEqual({
      question: "Open this address in your browser?",
      lines: ["https://a.example/x"],
      hidden: 0,
    });
    expect(readSummary("Delete it?")).toEqual({ question: "Delete it?", lines: [], hidden: 0 });
    expect(readSummary("")).toEqual({ question: "", lines: [], hidden: 0 });
  });

  it("keeps the 'and N more characters' note as a line of its own, and drops blank lines", () => {
    expect(readSummary("Q?\r\nhttps://a.example\n\n...and 12 more characters")).toEqual({
      question: "Q?",
      lines: ["https://a.example", "...and 12 more characters"],
      hidden: 0,
    });
  });

  it("bounds what crossed IPC: each line, and how many follow the question", () => {
    const long = "x".repeat(CONFIRM_LINE_CHARS + 50);
    const out = readSummary(`${long}\n${long}`);
    expect(out.question.length).toBe(CONFIRM_LINE_CHARS + 1);
    expect(out.lines[0].length).toBe(CONFIRM_LINE_CHARS + 1);
    const many = readSummary(
      ["Q?", ...Array.from({ length: 40 }, (_, i) => `line ${i}`)].join("\n"),
    );
    expect(many.lines).toHaveLength(CONFIRM_MAX_ADDRESS_LINES);
    // The 34 not drawn are counted, never dropped without a word.
    expect(many.hidden).toBe(40 - CONFIRM_MAX_ADDRESS_LINES);
  });

  it("turns control and direction-changing characters into spaces on every line", () => {
    const out = readSummary("Delete \u202e\u0007it?\nhttps://a.example/\u2066x\u200b");
    expect(out.question).toBe("Delete   it?");
    expect(out.lines).toEqual(["https://a.example/ x "]);
  });
});

describe("what the dialog draws for a line is what the shared rule keeps", () => {
  const FAMILY = "\u{1F468}\u200D\u{1F469}\u200D\u{1F467} Family";
  const ACUTE = "\u0301";

  it("draws an emoji sequence whole: the joiner and the variation selector stay", () => {
    expect(cleanUserLine(FAMILY)).toBe(FAMILY);
    expect(readSummary(`Create a new project with this name?\n${FAMILY}`).lines).toEqual([FAMILY]);
    expect(cleanUserLine("\u2764\uFE0F\u00A0ok")).toBe("\u2764\uFE0F\u00A0ok");
  });

  it("keeps four stacked marks and cuts the fifth", () => {
    expect(CONFIRM_MAX_MARKS).toBe(4);
    expect(cleanUserLine(`e${ACUTE.repeat(4)}`)).toBe(`e${ACUTE.repeat(4)}`);
    expect(cleanUserLine(`e${ACUTE.repeat(5)}x`)).toBe(`e${ACUTE.repeat(4)}x`);
    expect(readSummary(`Q?\ne${ACUTE.repeat(9)}`).lines).toEqual([`e${ACUTE.repeat(4)}`]);
  });

  it("still turns a bidi control, a zero-width space, a line separator and a control into a space", () => {
    expect(cleanUserLine("a\u202Eb\u200Bc\u2028d\u0007e\u0085f\uFEFFg")).toBe("a b c d e f g");
    expect(readSummary("Q?\nx\u2066y").lines).toEqual(["x y"]);
  });

  it("leaves the address and the hidden-line count as they were", () => {
    const out = readSummary(
      [
        "Open this address in your browser?",
        "https://a.example/x?q=1#f",
        "...and 12 more characters",
      ].join("\n"),
    );
    expect(out.lines).toEqual(["https://a.example/x?q=1#f", "...and 12 more characters"]);
    expect(out.hidden).toBe(0);
    const many = readSummary(["Q?", ...Array.from({ length: 9 }, (_, i) => `l${i}`)].join("\n"));
    expect(many.hidden).toBe(9 - CONFIRM_MAX_ADDRESS_LINES);
  });

  // The drawn rule is pinned to the Rust one by reading the Rust source, so a character added to
  // `is_invisible` (or a change of the mark limit) fails here until the page follows.
  describe("parity with src-tauri/src/mods/package.rs", () => {
    const fnBody = (name: string): string => {
      const start = packageRs.indexOf(`fn ${name}(`);
      expect(start, name).toBeGreaterThan(-1);
      return packageRs.slice(start, packageRs.indexOf("\n}\n", start));
    };
    /** The code points a Rust `matches!` pattern list names (`0xAB | 0xA..=0xB`, or `'\u{a}'..='\u{b}'`). */
    const named = (body: string): Set<number> => {
      const set = new Set<number>();
      const num = String.raw`(?:0x([0-9A-Fa-f]+)|'\\u\{([0-9A-Fa-f]+)\}')`;
      const re = new RegExp(`${num}(?:\\s*\\.\\.=\\s*${num})?`, "g");
      for (const m of body.matchAll(re)) {
        const lo = parseInt(m[1] ?? m[2], 16);
        const hi = m[3] || m[4] ? parseInt(m[3] ?? m[4], 16) : lo;
        for (let c = lo; c <= hi; c++) set.add(c);
      }
      return set;
    };

    it("reads the glue and the invisible lists from the Rust source", () => {
      const glue = named(fnBody("is_emoji_glue"));
      expect([...glue].sort((a, b) => a - b)).toEqual([
        0x200d,
        ...Array.from({ length: 16 }, (_, i) => 0xfe00 + i),
      ]);
      expect(named(fnBody("is_invisible")).size).toBeGreaterThan(4000);
    });

    it("hides, for every code point, exactly what tidy_user_line removes or refuses", () => {
      const glue = named(fnBody("is_emoji_glue"));
      const invisible = named(fnBody("is_invisible"));
      const cc = /^\p{Cc}$/u;
      const wrong: string[] = [];
      for (let cp = 0; cp <= 0x10ffff; cp++) {
        if (cp >= 0xd800 && cp <= 0xdfff) continue;
        const ch = String.fromCodePoint(cp);
        const rustRemoves = !glue.has(cp) && (invisible.has(cp) || cc.test(ch));
        const drawn = cleanUserLine(ch) === " " && ch !== " ";
        if (rustRemoves !== drawn) wrong.push(`U+${cp.toString(16).toUpperCase()}`);
      }
      expect(wrong).toEqual([]);
    });

    it("keeps the Rust mark limit", () => {
      const m = /const MAX_STACKED_MARKS: usize = (\d+);/.exec(packageRs);
      expect(Number(m?.[1])).toBe(CONFIRM_MAX_MARKS);
    });
  });
});
