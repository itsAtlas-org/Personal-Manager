// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import {
  ARRANGE_FAILED,
  COHESION_CHOICES,
  LOAD_FAILED,
  nearestCohesion,
  NO_ANSWER,
  noticeText,
  NOT_FRAMED,
  OPEN_FAILED,
  phaseText,
  SETTINGS_READ_FAILED,
  SETTINGS_SAVE_FAILED,
  subtitle,
} from "./copy";

describe("subtitle", () => {
  it("says how many documents across how many projects by project, singular and plural", () => {
    expect(subtitle("project", 1, 1, 0)).toBe(
      "1 document across 1 project · scroll to zoom, drag to pan, click a document to read it",
    );
    expect(subtitle("project", 12, 3, 0)).toBe(
      "12 documents across 3 projects · scroll to zoom, drag to pan, click a document to read it",
    );
    expect(subtitle("project", 0, 0, 0)).toBe(
      "0 documents across 0 projects · scroll to zoom, drag to pan, click a document to read it",
    );
  });

  it("says how many documents by meaning when all are placed", () => {
    expect(subtitle("semantic", 1, 1, 1)).toBe(
      "1 document by meaning · scroll to zoom, drag to pan",
    );
    expect(subtitle("semantic", 40, 5, 40)).toBe(
      "40 documents by meaning · scroll to zoom, drag to pan",
    );
  });

  it("says it is showing the top N when the cap left some out", () => {
    expect(subtitle("semantic", 1500, 5, 1000)).toBe(
      "showing the top 1000 of 1500 documents by meaning · scroll to zoom, drag to pan",
    );
  });

  it("does not claim a top N when none are placed", () => {
    expect(subtitle("semantic", 20, 2, 0)).toBe(
      "20 documents by meaning · scroll to zoom, drag to pan",
    );
  });
});

describe("phaseText", () => {
  it("is silent for the quick check of what is saved, and says each long step", () => {
    expect(phaseText("checking")).toBeNull();
    expect(phaseText("reading")).toBe("Reading what your documents are about…");
    expect(phaseText("reducing")).toBe("Placing documents by meaning…");
    expect(phaseText("computing")).toBe("Arranging your projects…");
    expect(phaseText("saving")).toBe("Saving the map for next time…");
  });
});

describe("noticeText", () => {
  it("counts what was added", () => {
    expect(noticeText({ added: 1, deleted: 0, missed: false })).toBe("1 document was added.");
    expect(noticeText({ added: 7, deleted: 0, missed: false })).toBe("7 documents were added.");
  });

  it("counts what was removed", () => {
    expect(noticeText({ added: 0, deleted: 1, missed: false })).toBe("1 document was removed.");
    expect(noticeText({ added: 0, deleted: 4, missed: false })).toBe("4 documents were removed.");
  });

  it("counts both", () => {
    expect(noticeText({ added: 3, deleted: 2, missed: false })).toBe(
      "3 documents were added and 2 removed.",
    );
    expect(noticeText({ added: 1, deleted: 1, missed: false })).toBe(
      "1 document was added and 1 removed.",
    );
  });

  it("says only that things changed when PM dropped events (the counts are a floor)", () => {
    const text = "Your documents changed while the map was open.";
    expect(noticeText({ added: 3, deleted: 2, missed: true })).toBe(text);
    expect(noticeText({ added: 0, deleted: 0, missed: true })).toBe(text);
  });

  it("says that things changed for a notice with no counts", () => {
    expect(noticeText({ added: 0, deleted: 0, missed: false })).toBe(
      "Your documents changed while the map was open.",
    );
  });
});

describe("the cohesion menu", () => {
  it("offers the core Map's four choices", () => {
    expect(COHESION_CHOICES).toEqual([
      { value: 0, label: "Off" },
      { value: 0.15, label: "Low" },
      { value: 0.3, label: "Medium" },
      { value: 0.5, label: "High" },
    ]);
  });

  it("shows the nearest choice for a stored value between them", () => {
    expect(nearestCohesion(0)).toBe(0);
    expect(nearestCohesion(0.15)).toBe(0.15);
    expect(nearestCohesion(0.07)).toBe(0);
    expect(nearestCohesion(0.08)).toBe(0.15);
    expect(nearestCohesion(0.2)).toBe(0.15);
    expect(nearestCohesion(0.25)).toBe(0.3);
    expect(nearestCohesion(0.45)).toBe(0.5);
    expect(nearestCohesion(0.5)).toBe(0.5);
  });
});

describe("the sentences for what goes wrong", () => {
  it("are the Map's own: fixed text, nothing from PM or a document in them", () => {
    for (const text of [
      LOAD_FAILED,
      ARRANGE_FAILED,
      OPEN_FAILED,
      SETTINGS_READ_FAILED,
      SETTINGS_SAVE_FAILED,
      NOT_FRAMED,
      NO_ANSWER,
    ]) {
      expect(typeof text).toBe("string");
      expect(text).not.toMatch(/MOD-\d|undefined|\[object/);
      expect(text.endsWith(".")).toBe(true);
    }
  });

  it("reassure that documents are untouched where that is the worry", () => {
    expect(LOAD_FAILED).toContain("unchanged");
    expect(ARRANGE_FAILED).toContain("unchanged");
  });
});
