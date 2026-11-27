// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the import and export dialogs say: one line per count and warning, plurals that read
// naturally, nothing for a zero. Invented names only.

import { describe, expect, it } from "vitest";
import {
  exportTitle,
  exportedLine,
  headerExportChoices,
  ignoredLine,
  importedLine,
  isStalePreview,
  keptLine,
  peopleCount,
  previewHeading,
  previewLines,
  restoredLine,
  selectionExportChoice,
} from "./peopleTransfer";
import type { PeopleImportPreview } from "./types";

function preview(over: Partial<PeopleImportPreview> = {}): PeopleImportPreview {
  return {
    preview_id: "p1",
    file_name: "friends.vcf",
    people: 5,
    already: 0,
    from_trash: 0,
    look_alike: 0,
    no_name: 0,
    repeated: 0,
    photos_skipped: false,
    complete: true,
    warnings: [],
    ignored: [],
    ...over,
  };
}

const texts = (p: PeopleImportPreview) => previewLines(p).map((l) => l.text);

describe("plurals", () => {
  it("says 1 person and 2 people", () => {
    expect(peopleCount(1)).toBe("1 person");
    expect(peopleCount(0)).toBe("0 people");
    expect(peopleCount(2)).toBe("2 people");
    expect(exportTitle(1)).toBe("Export 1 person");
    expect(exportTitle(12)).toBe("Export 12 people");
    expect(exportTitle(null)).toBe("Export people");
  });
});

describe("the preview", () => {
  it("heads with the count and the file's name", () => {
    expect(previewHeading(preview())).toBe("5 people in friends.vcf");
    expect(previewHeading(preview({ people: 1 }))).toBe("1 person in friends.vcf");
    expect(previewHeading(preview({ file_name: "" }))).toBe("5 people in this file");
  });

  it("says nothing for a file with nothing to remark on", () => {
    expect(texts(preview())).toEqual([]);
  });

  it("words the already-in-PM and look-alike counts", () => {
    expect(texts(preview({ already: 3, look_alike: 2 }))).toEqual([
      "3 are already in PM. PM fills in what's missing and keeps what you have.",
      "2 look like people you have. You can merge them after the import.",
    ]);
    expect(texts(preview({ already: 1, look_alike: 1 }))).toEqual([
      "1 is already in PM. PM fills in what's missing and keeps what you have.",
      "1 looks like someone you have. You can merge them after the import.",
    ]);
  });

  it("says who comes back from Trash", () => {
    expect(texts(preview({ already: 2, from_trash: 1 }))[1]).toBe(
      "1 of them is in Trash, and comes back.",
    );
    expect(texts(preview({ already: 3, from_trash: 2 }))[1]).toBe(
      "2 of them are in Trash, and come back.",
    );
  });

  it("words the skipped cards, the repeated id and photos", () => {
    expect(texts(preview({ no_name: 12 }))).toEqual([
      "12 cards had no name, email or phone and were skipped.",
    ]);
    expect(texts(preview({ no_name: 1 }))).toEqual([
      "1 card had no name, email or phone and was skipped.",
    ]);
    expect(texts(preview({ repeated: 1 }))).toEqual([
      "1 card repeated an earlier card's id, so it was added as a new person.",
    ]);
    expect(texts(preview({ repeated: 3 }))).toEqual([
      "3 cards repeated an earlier card's id, so they were added as new people.",
    ]);
    expect(texts(preview({ photos_skipped: true }))).toEqual(["Photos are skipped."]);
  });

  it("does not say the no-name warning twice", () => {
    const lines = texts(preview({ no_name: 2, warnings: [{ kind: "no_name", count: 2 }] }));
    expect(lines).toHaveLength(1);
  });

  it("words every other warning in plain words, never as a raw kind", () => {
    const kinds = [
      "bad_encoding",
      "value_cut",
      "bad_date",
      "unfinished_card",
      "too_many_properties",
      "too_many_details",
      "unreadable_line",
    ] as const;
    for (const kind of kinds) {
      const [one] = texts(preview({ warnings: [{ kind, count: 1 }] }));
      const [many] = texts(preview({ warnings: [{ kind, count: 4 }] }));
      expect(one, kind).toMatch(/^1 /);
      expect(many, kind).toMatch(/^4 /);
      expect(one).not.toContain("_");
    }
  });

  it("skips a warning kind it doesn't know and a zero count", () => {
    const warnings = [
      { kind: "from_the_future" as never, count: 3 },
      { kind: "value_cut" as const, count: 0 },
    ];
    expect(texts(preview({ warnings }))).toEqual([]);
  });

  it("says a file that ended early may be missing people", () => {
    const [line] = previewLines(preview({ complete: false }));
    expect(line.text).toMatch(/some people may be missing/);
    expect(line.tone).toBe("warn");
  });

  it("counts what was left out by name, photos aside", () => {
    expect(
      ignoredLine([
        { name: "PHOTO", count: 9 },
        { name: "GEO", count: 3 },
        { name: "X-CUSTOM", count: 1 },
      ]),
    ).toBe("Left out, because PM doesn't keep them: map positions (3), X-CUSTOM fields (1).");
    expect(ignoredLine([{ name: "PHOTO", count: 9 }])).toBeNull();
    expect(ignoredLine([])).toBeNull();
  });

  it("caps the left-out list and says how many more", () => {
    const many = ["LOGO", "SOUND", "KEY", "SOURCE", "GEO", "TZ", "PRODID", "FBURL"].map((name) => ({
      name,
      count: 1,
    }));
    expect(ignoredLine(many)).toMatch(/, and 2 more kinds\.$/);
  });

  it("keeps a hostile file name as plain text in the heading", () => {
    const name = "<img src=x onerror=alert(1)>.vcf";
    expect(previewHeading(preview({ file_name: name }))).toBe(`5 people in ${name}`);
  });
});

describe("the result", () => {
  const result = { added: 0, updated: 0, unchanged: 0, restored: [], kept: [], review: 0 };

  it("says how many were imported and how they split", () => {
    expect(importedLine({ ...result, added: 3 })).toBe("Imported 3 people.");
    expect(importedLine({ ...result, added: 1 })).toBe("Imported 1 person.");
    expect(importedLine({ ...result, added: 2, updated: 1, unchanged: 4 })).toBe(
      "Imported 7 people. 2 new, 1 filled in, 4 already complete.",
    );
  });

  it("names the people who came back from Trash", () => {
    expect(restoredLine([])).toBeNull();
    expect(restoredLine(["Ada Example"])).toBe("1 person came back from Trash: Ada Example.");
    expect(restoredLine(["Ada Example", "Ben Sample"])).toBe(
      "2 people came back from Trash: Ada Example, Ben Sample.",
    );
  });

  it("caps the people named as back from Trash", () => {
    const names = Array.from({ length: 15 }, (_, i) => `Person ${i + 1}`);
    const line = restoredLine(names) ?? "";
    expect(line.startsWith("15 people came back from Trash: Person 1, Person 2,")).toBe(true);
    expect(line).toContain("Person 12, and 3 more.");
    expect(line).not.toContain("Person 13");
    expect(restoredLine(names.slice(0, 12))?.endsWith("Person 12.")).toBe(true);
  });

  it("lists what PM kept for a person", () => {
    expect(keptLine({ name: "Ada Example", fields: ["birthday", "notes"] })).toBe(
      "Ada Example: birthday, notes",
    );
  });

  it("recognises the backend's refusal of an old preview", () => {
    expect(
      isStalePreview(
        "That file was read a while ago, or PM has changed since. Pick the file again.",
      ),
    ).toBe(true);
    expect(isStalePreview("PM couldn't import that file.")).toBe(false);
    expect(isStalePreview(new Error("x"))).toBe(false);
  });
});

describe("who an export covers", () => {
  it("offers everyone from the full list, with its count", () => {
    const [all, ...rest] = headerExportChoices("All", null, { count: 7, searching: false });
    expect(rest).toEqual([]);
    expect(all).toMatchObject({ scope: "All", count: 7 });
  });

  it("does not take a search's or a filter's count for everyone", () => {
    expect(headerExportChoices("All", null, { count: 2, searching: true })[0].count).toBeNull();
    expect(headerExportChoices("Starred", null, { count: 2, searching: false })[0]).toMatchObject({
      scope: "All",
      count: null,
    });
  });

  it("offers a label's people first, then everyone", () => {
    const choices = headerExportChoices({ Label: 4 }, "Friends", { count: 3, searching: false });
    expect(choices.map((c) => c.scope)).toEqual([{ Label: 4 }, "All"]);
    expect(choices[0]).toMatchObject({ label: "People labelled Friends", count: 3 });
    expect(choices[1].count).toBeNull();
  });

  it("exports exactly the people ticked", () => {
    const c = selectionExportChoice([4, 9]);
    expect(c).toMatchObject({ scope: { Ids: [4, 9] }, count: 2 });
  });

  it("says what was written", () => {
    expect(exportedLine({ people: 1 })).toBe("Exported 1 person.");
    expect(exportedLine({ people: 3 })).toBe("Exported 3 people.");
  });
});
