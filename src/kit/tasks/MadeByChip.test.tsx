// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Made by X" (PR C, K5): a mod's name is the mod's own word, so the chip draws it as clamped text and
// ALWAYS with the tier word the ledger recorded. An unsigned mod called "Atlas" can't read as Atlas.

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { MadeBy } from "../../lib/types";
import { MADE_BY_NAME_MAX, madeByName, madeByText, tierWord } from "../lib/madeBy";
import { MadeByChip } from "./TaskBits";

afterEach(cleanup);

const made = (over: Partial<MadeBy> = {}): MadeBy => ({
  name: "Pinboard",
  tier: "first",
  signed: true,
  ...over,
});

describe("MadeByChip", () => {
  it("draws nothing for what the user made", () => {
    const { container } = render(<MadeByChip made={null} />);
    expect(container.firstChild).toBeNull();
  });

  it("names the maker and ALWAYS the tier word, whichever tier it is", () => {
    for (const [tier, word] of [
      ["first", "first party"],
      ["second", "reviewed by Atlas"],
      ["third", "third party"],
    ] as const) {
      const { container, unmount } = render(<MadeByChip made={made({ tier })} />);
      expect(container.textContent).toBe(`Pinboard· ${word}`);
      unmount();
    }
  });

  it("a mod that is not signed reads as unsigned, whatever name or tier it claims", () => {
    const { container } = render(
      <MadeByChip made={made({ name: "Atlas", tier: "first", signed: false })} />,
    );
    expect(container.textContent).toBe("Atlas· unsigned");
    expect(container.textContent).not.toMatch(/first party|reviewed by Atlas/);
    expect(container.firstElementChild?.getAttribute("title")).toBe("Made by Atlas (unsigned)");
  });

  it("a forged tier word in the name is only text beside the real one", () => {
    const { container } = render(
      <MadeByChip
        made={made({ name: "Atlas (reviewed by Atlas)", tier: "third", signed: true })}
      />,
    );
    expect(container.textContent?.endsWith("· third party")).toBe(true);
  });

  it("is React text: markup in a name is shown as written", () => {
    render(<MadeByChip made={made({ name: '<img src=x onerror="alert(1)">', signed: false })} />);
    expect(document.querySelector("img")).toBeNull();
    expect(screen.getByText('<img src=x onerror="alert(1)">')).toBeTruthy();
  });

  it("clamps a long name but never the tier word", () => {
    const { container } = render(<MadeByChip made={made({ name: "x".repeat(500) })} />);
    const [name, tier] = [...container.querySelectorAll("span > span")].map((s) => s.textContent);
    expect([...(name ?? "")].length).toBeLessThanOrEqual(MADE_BY_NAME_MAX);
    expect(tier).toBe("· first party");
  });
});

describe("madeBy text", () => {
  it("strips control and direction-changing characters and tidies space", () => {
    expect(madeByName(made({ name: "  A‮B\u0007\n C​ " }))).toBe("A B C");
  });

  it("holds the bidirectional Arabic letter mark, soft hyphen, tag characters and separators too", () => {
    // U+061C, U+00AD, U+180E, U+E0041 (a tag letter), U+2028 and U+2029: format or separator
    // characters the old hand-written list let through.
    expect(madeByName(made({ name: "A\u061cB\u00adC\u180eD\u{e0041}E\u2028F\u2029G" }))).toBe(
      "A B C D E F G",
    );
  });

  it("cuts a run of combining marks, so a name can't stack accents over what is beside it", () => {
    expect(madeByName(made({ name: "e" + "\u0301".repeat(40) }))).toBe("e\u0301\u0301\u0301");
  });

  it("says 'a mod' when nothing printable is left, and still names the tier", () => {
    expect(madeByName(made({ name: "‮​" }))).toBe("a mod");
    expect(madeByText(made({ name: "", signed: false }))).toBe("a mod (unsigned)");
  });

  it("counts a character, not a UTF-16 unit, when it cuts", () => {
    const name = madeByName(made({ name: "😀".repeat(100) }));
    expect([...name]).toHaveLength(MADE_BY_NAME_MAX);
    expect(name.endsWith("…")).toBe(true);
  });

  it("treats a tier PM does not know as third party", () => {
    expect(tierWord({ name: "x", tier: "root" as MadeBy["tier"], signed: true })).toBe(
      "third party",
    );
  });
});
