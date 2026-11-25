// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The kit cannot import core's cleaning rule (src/kit may only import src/kit), so madeBy.ts holds its
// own copy of it. This pins the two to one answer, so a made-by name is cleaned exactly as every other
// name a mod supplies is.

import { describe, expect, it } from "vitest";
import { madeByName } from "../kit/lib/madeBy";
import { plainLine } from "./modTextClean";

const SAMPLES = [
  "Pinboard",
  "A‮B\u0007\nC​",
  "A؜B­C᠎D\u{e0041}E F G",
  "e" + "́".repeat(40),
  "Atlas⁦ ⁩first party",
  "  spaced   out  ",
  "กิ้๊",
];

describe("a made-by name is cleaned by the rule every mod-supplied name is", () => {
  it.each(SAMPLES)("%j", (name) => {
    const rule = plainLine(name);
    expect(madeByName({ name, tier: "third", signed: false })).toBe(rule === "" ? "a mod" : rule);
  });
});
