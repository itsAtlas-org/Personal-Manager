// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What an uninstall forgets on this device: the mod's tab placement and every fold keyed to it, and
// nothing keyed to anything else. A mod's folds are `mods.mod.<id>` and `mods.mod.<id>:<section>`.

import { beforeEach, describe, expect, it } from "vitest";

import { pruneFolds, readFold, writeFold } from "./foldPrefs";
import { modFoldKey, pruneAfterUninstall } from "./modFolds";
import { NAV_PLACEMENT_KEY, parsePlacement, writeNavPlacement } from "./nav";

const readNavPlacement = () => parsePlacement(localStorage.getItem(NAV_PLACEMENT_KEY));

beforeEach(() => localStorage.clear());

describe("modFoldKey", () => {
  it("keys a mod's row and a section inside it", () => {
    expect(modFoldKey("acme.tool")).toBe("mods.mod.acme.tool");
    expect(modFoldKey("acme.tool", "permissions")).toBe("mods.mod.acme.tool:permissions");
  });
});

describe("pruneFolds", () => {
  it("takes the prefix itself and its sections, and never a longer id that merely starts with it", () => {
    writeFold("mods.mod.a", true);
    writeFold("mods.mod.a:permissions", false);
    writeFold("mods.mod.ab", true);
    writeFold("mods.mod.ab:permissions", true);
    writeFold("settings.connectors.Google", false);
    pruneFolds("mods.mod.a");
    expect(readFold("mods.mod.a")).toBeNull();
    expect(readFold("mods.mod.a:permissions")).toBeNull();
    expect(readFold("mods.mod.ab")).toBe(true);
    expect(readFold("mods.mod.ab:permissions")).toBe(true);
    expect(readFold("settings.connectors.Google")).toBe(false);
  });

  it("does nothing for an empty prefix", () => {
    writeFold("x", true);
    pruneFolds("");
    expect(readFold("x")).toBe(true);
  });
});

describe("pruneAfterUninstall", () => {
  const prune = { tab: "mod:acme.tool", foldPrefix: "mods.mod.acme.tool" };

  it("forgets the mod's tab placement and its folds on this device", () => {
    writeNavPlacement({ hidden: ["mod:acme.tool", "mod:acme.other"], pinned: ["mod:acme.tool"] });
    writeFold("mods.mod.acme.tool", true);
    writeFold("mods.mod.acme.tool:permissions", true);
    writeFold("mods.mod.acme.other", true);
    pruneAfterUninstall("acme.tool", prune);
    expect(readNavPlacement()).toEqual({ hidden: ["mod:acme.other"], pinned: [] });
    expect(readFold("mods.mod.acme.tool")).toBeNull();
    expect(readFold("mods.mod.acme.tool:permissions")).toBeNull();
    expect(readFold("mods.mod.acme.other")).toBe(true);
  });

  it("prunes nothing for a reply that names something other than the mod asked about", () => {
    writeNavPlacement({ hidden: ["mod:acme.tool", "mod:acme.other"], pinned: [] });
    writeFold("mods.mod.acme.other", true);
    writeFold("mods.mod.acme.tool", true);
    pruneAfterUninstall("acme.tool", { tab: "mod:acme.other", foldPrefix: "mods.mod.acme.other" });
    expect(readNavPlacement().hidden).toEqual(["mod:acme.tool", "mod:acme.other"]);
    expect(readFold("mods.mod.acme.other")).toBe(true);
    expect(readFold("mods.mod.acme.tool")).toBe(true);
  });
});
