// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The project palette, colour by colour. The core Map's copy (src/theme/graphPalette.ts) is held by its
// own test only at a few entries, and a mod cannot import it, so this file is where the whole palette is
// written down: a change to any colour is a visible change here, and the live comparison with the core
// Map (while both exist) is what proves the two agree.

import { describe, expect, it } from "vitest";
import { graphColor, THEME_FALLBACK } from "./palette";

const DARK = [
  "#60a5fa",
  "#34d399",
  "#f472b6",
  "#fbbf24",
  "#a78bfa",
  "#22d3ee",
  "#fb923c",
  "#4ade80",
  "#f87171",
  "#c084fc",
];
const LIGHT = [
  "#2563eb",
  "#059669",
  "#db2777",
  "#d97706",
  "#7c3aed",
  "#0891b2",
  "#ea580c",
  "#16a34a",
  "#dc2626",
  "#9333ea",
];
const CVD_DARK = [
  "#56b4e9",
  "#e69f00",
  "#009e73",
  "#f0e442",
  "#0072b2",
  "#d55e00",
  "#cc79a7",
  "#bbbbbb",
];
const CVD_LIGHT = [
  "#0072b2",
  "#e69f00",
  "#009e73",
  "#cc79a7",
  "#d55e00",
  "#56b4e9",
  "#8a6d00",
  "#000000",
];

const run = (n: number, mode: "dark" | "light", cvd: boolean) =>
  Array.from({ length: n }, (_, i) => graphColor(i, mode, cvd));

describe("graphColor", () => {
  it("gives each of the four sets exactly its colours, in order", () => {
    expect(run(10, "dark", false)).toEqual(DARK);
    expect(run(10, "light", false)).toEqual(LIGHT);
    expect(run(8, "dark", true)).toEqual(CVD_DARK);
    expect(run(8, "light", true)).toEqual(CVD_LIGHT);
  });

  it("does what the core's own test holds of it", () => {
    expect(graphColor(0, "dark")).toBe("#60a5fa");
    expect(graphColor(0, "dark", true)).toBe("#56b4e9");
    expect(graphColor(0, "light", true)).toBe("#0072b2");
    expect(graphColor(8, "dark", true)).toBe(graphColor(0, "dark", true));
  });

  it("is the plain palette when the colour-blind switch is not given", () => {
    expect(graphColor(3, "dark")).toBe(DARK[3]);
    expect(graphColor(3, "dark", false)).toBe(DARK[3]);
  });

  it("wraps past the end and never fails on a negative or a huge index", () => {
    expect(graphColor(10, "dark")).toBe(DARK[0]);
    expect(graphColor(23, "light")).toBe(LIGHT[3]);
    expect(graphColor(-1, "dark")).toBe(DARK[9]);
    expect(graphColor(-1, "dark", true)).toBe(CVD_DARK[7]);
    expect(graphColor(1_000_003, "dark")).toBe(DARK[3]);
  });

  it("gives the colour-blind set eight distinct colours in each mode", () => {
    expect(new Set(CVD_DARK).size).toBe(8);
    expect(new Set(CVD_LIGHT).size).toBe(8);
  });

  it("keeps a legible mid-grey for a colour the theme does not carry", () => {
    expect(THEME_FALLBACK).toBe("#888");
  });
});
