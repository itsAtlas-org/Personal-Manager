// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The skeleton's one test, there to prove vitest collects `mods/**` (vitest.config.ts) and that the
// source and the manifest agree on the mod's identity.
import { describe, expect, it } from "vitest";
import manifest from "../manifest.json";
import { MOD_ID } from "./id";

describe("atlas.map skeleton", () => {
  it("names itself the way its manifest does", () => {
    expect(MOD_ID).toBe(manifest.id);
  });

  it("keeps the all-zero placeholder commit in the source tree", () => {
    // A manifest cannot hold its own commit; pack-mod stamps the real one into the package.
    expect(manifest.publisher.commit).toBe("0".repeat(40));
  });
});
