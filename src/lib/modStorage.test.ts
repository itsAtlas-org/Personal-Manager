// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// How Settings → Storage says what a mod takes: files are measured, what a mod saved is an estimate,
// so a "~" appears exactly when an estimate is in the figure.

import { describe, expect, it } from "vitest";

import { modBytes, modSize, modsTotal } from "./modStorage";
import type { StorageMod } from "./types";

const MB = 1024 * 1024;
const m = (folder: number, data: number): StorageMod => ({
  id: "acme.tool",
  name: "Acme Tool",
  version: "1.0.0",
  folder_bytes: folder,
  data_bytes: data,
});

describe("a mod's size", () => {
  it("adds its files and what it saved", () => {
    expect(modBytes(m(3 * MB, MB))).toBe(4 * MB);
  });

  it("is an estimate (a ~) when it saved anything, measured when it saved nothing", () => {
    expect(modSize(m(3 * MB, MB))).toBe("~4 MB");
    expect(modSize(m(2 * MB, 0))).toBe("2 MB");
  });

  it("is measured, not estimated, for a mod with no files and nothing saved", () => {
    expect(modSize(m(0, 0))).toBe("0 B");
  });
});

describe("the mods together", () => {
  it("sums them, with a ~ when any of them is an estimate", () => {
    expect(modsTotal([m(3 * MB, MB), m(2 * MB, 0)])).toBe("~6 MB");
    expect(modsTotal([m(3 * MB, 0), m(2 * MB, 0)])).toBe("5 MB");
  });

  it("is zero for none", () => {
    expect(modsTotal([])).toBe("0 B");
  });
});
