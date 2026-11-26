// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the duplicate review and the merge dialog say: the reason sentences, the grid both cards
// share, the "Nothing differs" group and its preview line, the hidden-pairs line and the notes about
// a scan that did not look at everything.

import { describe, expect, it } from "vitest";
import {
  cardRows,
  duplicatesButtonLabel,
  groupPairs,
  hiddenLine,
  incompleteLine,
  mergeBody,
  mergeExtrasLine,
  mergedNotice,
  nothingDiffersLine,
  reasonLabels,
  reasonSentence,
} from "./peopleMerge";
import type { DupeCard, DupePair, DupesReport } from "./types";

const NO_DIFF = { name: false, birthday: false, met: false, notes: false };
const TODAY = new Date(2026, 10, 26);

function card(id: number, over: Partial<DupeCard> = {}): DupeCard {
  return {
    id,
    uid: String(id).padStart(32, "0"),
    display_name: `Person ${id}`,
    birthday: null,
    met_where: "",
    met_on: null,
    note: "",
    details: [],
    ...over,
  };
}

function pair(a: number, b: number, over: Partial<DupePair> = {}): DupePair {
  return { a, b, reasons: ["Name"], differs: NO_DIFF, nothing_differs: false, ...over };
}

function report(over: Partial<DupesReport> = {}): DupesReport {
  return { pairs: [], cards: [], hidden: 0, skipped: [], cut_short: false, scanned: 0, ...over };
}

describe("the button", () => {
  it("is absent at zero and counts the pairs otherwise", () => {
    expect(duplicatesButtonLabel(null)).toBeNull();
    expect(duplicatesButtonLabel(report())).toBeNull();
    expect(duplicatesButtonLabel(report({ pairs: [pair(1, 2), pair(3, 4), pair(5, 6)] }))).toBe(
      "Possible duplicates (3)",
    );
  });

  it("stays, without a count, when none were found but the scan was incomplete or pairs are hidden", () => {
    expect(duplicatesButtonLabel(report({ skipped: [{ reason: "Email", people: 300 }] }))).toBe(
      "Possible duplicates",
    );
    expect(duplicatesButtonLabel(report({ cut_short: true }))).toBe("Possible duplicates");
    expect(duplicatesButtonLabel(report({ hidden: 2 }))).toBe("Possible duplicates");
  });
});

describe("reasons", () => {
  it("are said as short labels and as sentences, each apart", () => {
    expect(reasonLabels(["Email", "Name"])).toBe("Same email · Same name");
    expect(reasonSentence(["Phone"])).toBe("They have the same phone number.");
    expect(reasonLabels(["Similar"])).toBe("Similar name");
  });
});

describe("the Nothing differs group", () => {
  it("takes the pairs that lose nothing and never a Similar name", () => {
    const r = report({
      pairs: [
        pair(1, 2, { nothing_differs: true }),
        pair(3, 4, { nothing_differs: true, reasons: ["Similar"] }),
        pair(5, 6),
      ],
    });
    const { identical, rest } = groupPairs(r);
    expect(identical.map((p) => p.a)).toEqual([1]);
    expect(rest.map((p) => p.a)).toEqual([3, 5]);
  });

  it("words its preview line in the spec's words", () => {
    expect(nothingDiffersLine(12, 4)).toBe("12 pairs merge with nothing lost. 4 are left for you.");
    expect(nothingDiffersLine(1, 1)).toBe("1 pair merges with nothing lost. 1 is left for you.");
  });
});

describe("hidden pairs and an incomplete scan", () => {
  it("says how many are hidden", () => {
    expect(hiddenLine(2)).toBe("2 pairs you said aren't the same person are hidden.");
    expect(hiddenLine(1)).toBe("1 pair you said isn't the same person is hidden.");
  });

  it("says what was not looked at, and says nothing for a full scan", () => {
    expect(incompleteLine(report())).toBeNull();
    expect(incompleteLine(report({ skipped: [{ reason: "Email", people: 300 }] }))).toMatch(
      /didn't compare 1 very large group/,
    );
    expect(incompleteLine(report({ cut_short: true }))).toMatch(/may be more/);
  });
});

describe("the grid", () => {
  it("draws every row on both sides, in the same order, with an empty list where one side has none", () => {
    const a = card(1, { details: [{ kind: "email", label: "", value: "a@example.com" }] });
    const b = card(2, {
      birthday: "--04-15",
      details: [
        { kind: "email", label: "work", value: "a@example.com" },
        { kind: "phone", label: "", value: "+44 20 7946 0001" },
      ],
    });
    const rows = cardRows(a, b, TODAY);
    expect(rows.map((r) => r.key)).toEqual(["birthday", "met", "email", "phone", "notes"]);
    const birthday = rows.find((r) => r.key === "birthday")!;
    expect(birthday.left).toEqual([]);
    expect(birthday.right).toEqual(["15 April"]);
    expect(rows.find((r) => r.key === "email")!.right).toEqual(["a@example.com (work)"]);
    expect(rows.find((r) => r.key === "phone")!.left).toEqual([]);
  });

  it("leaves out a kind neither has", () => {
    expect(cardRows(card(1), card(2), TODAY).map((r) => r.key)).toEqual([
      "birthday",
      "met",
      "notes",
    ]);
  });
});

describe("the dialog's words", () => {
  const p = {
    details: 7,
    details_dropped: 1,
    labels: 2,
    timeline: 3,
    projects: 0,
    sources: 1,
    lossless: false,
  };
  it("says what moves and asks only where they differ", () => {
    expect(mergeBody({ ...p, differs: { ...NO_DIFF, notes: true } })).toBe(
      "Everything from both moves to one record: 7 details, 3 timeline entries, 2 labels. Where they differ, choose which to keep. The one you don't choose is dropped.",
    );
    expect(
      mergeBody({ ...p, details: 1, timeline: 1, labels: 1, lossless: true, differs: NO_DIFF }),
    ).toBe(
      "Everything from both moves to one record: 1 detail, 1 timeline entry, 1 label. Nothing differs, so nothing is dropped.",
    );
  });

  it("never promises nothing is dropped when a duplicate detail is kept once", () => {
    expect(mergeBody({ ...p, differs: NO_DIFF })).toBe(
      "Everything from both moves to one record: 7 details, 3 timeline entries, 2 labels. Nothing needs a choice. 1 detail is the same on both and kept once.",
    );
    expect(mergeBody({ ...p, details_dropped: 3, differs: NO_DIFF })).toContain(
      "3 details are the same on both and kept once.",
    );
  });

  it("says the project links and sources that move too, only when there are some", () => {
    expect(mergeExtrasLine({ ...p, projects: 0, sources: 0, differs: NO_DIFF })).toBeNull();
    expect(mergeExtrasLine({ ...p, projects: 1, sources: 0, differs: NO_DIFF })).toBe(
      "Also 1 project link, all kept.",
    );
    expect(mergeExtrasLine({ ...p, projects: 2, sources: 3, differs: NO_DIFF })).toBe(
      "Also 2 project links and 3 places they came from, all kept.",
    );
  });

  it("words the strip", () => {
    expect(mergedNotice({ display_name: "Sam Reed", merged: 1 })).toBe("Merged into Sam Reed.");
    expect(mergedNotice({ display_name: "Sam Reed", merged: 3 })).toBe("Merged 3 duplicates.");
  });
});
