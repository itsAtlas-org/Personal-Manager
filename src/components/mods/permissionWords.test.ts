// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The plain words for each permission (permissionWords.ts) cover exactly the permissions the backend
// has: a new one without words would be shown to the user as a raw id, and a retired one's words would
// say something nothing can do. The list is read from the backend's table.

import { describe, expect, it } from "vitest";
import permissionsRs from "../../../src-tauri/src/mods/api/permissions.rs?raw";
import { LRI, RLO } from "./modFixtures";
import {
  classWords,
  PERMISSION_IDS,
  permissionWords,
  standingLine,
  wideningWords,
} from "./permissionWords";

/** The ids in `POLICIES`, in order, read from the source. */
function backendIds(): string[] {
  const table = permissionsRs.slice(
    permissionsRs.indexOf("pub const POLICIES"),
    permissionsRs.indexOf("pub fn policy("),
  );
  return [...table.matchAll(/row\(\s*"([a-z.]+)"/g)].map((m) => m[1]);
}

describe("permissionWords", () => {
  it("has words for every permission the backend has, and for no other", () => {
    expect(backendIds().length).toBeGreaterThan(10);
    expect([...PERMISSION_IDS].sort()).toEqual([...backendIds()].sort());
  });

  it("shows a permission PM has no words for as it is, never hidden", () => {
    expect(permissionWords("documents.text")).toBe(
      "Read the text of your documents and search them",
    );
    expect(permissionWords("future.thing")).toBe('Use "future.thing"');
    // A name that is a property of every object isn't mistaken for a known permission.
    expect(permissionWords("constructor")).toBe('Use "constructor"');
  });

  it("names each class, and says nothing for one it doesn't know", () => {
    expect(classWords("read")).toBe("Reads your data");
    expect(classWords("egress")).toBe("Reaches outside PM");
    expect(classWords("toString")).toBeNull();
    expect(classWords("unknown")).toBeNull();
  });
});

describe("standingLine", () => {
  it("says what a permission with no checkbox of its own comes to", () => {
    expect(standingLine({ when: "auto", offBecause: null })).toBe("Always on.");
    expect(standingLine({ when: "first-use", offBecause: null })).toBe(
      "PM asks you the first time the mod needs this.",
    );
    expect(standingLine({ when: "install-then-first-use", offBecause: null })).toBe(
      "PM asks you the first time the mod needs this.",
    );
    expect(standingLine({ when: "ask", offBecause: null })).toBe(
      "Off until you turn it on in the Mods tab.",
    );
    // The backend's own reason wins where it gave one.
    expect(standingLine({ when: "ask", offBecause: "Nobody has checked this mod." })).toBe(
      "Nobody has checked this mod.",
    );
  });
});

describe("wideningWords", () => {
  it("says each kind of widening in words, in PM's wording for permissions", () => {
    expect(wideningWords("permission:ai")).toBe(
      "Ask an AI model to do things for it, which can cost money",
    );
    expect(wideningWords("host:api.example.com")).toBe("Reach api.example.com on the internet");
    expect(wideningWords("secret:API_KEY")).toBe("Keep a saved key called “API_KEY”");
    expect(wideningWords("contribution:calendar overlay")).toBe(
      "Add something of its own to PM: calendar overlay",
    );
  });

  it("shows one it doesn't know as it is, never hidden", () => {
    expect(wideningWords("something:else")).toBe("something:else");
    expect(wideningWords("plain")).toBe("plain");
    expect(wideningWords("permission:future.thing")).toBe('Use "future.thing"');
  });

  it("takes direction-changing characters and line breaks out of what the mod named", () => {
    expect(wideningWords(`host:a${RLO}.example\ncom`)).toBe("Reach a .example com on the internet");
    expect(wideningWords(`secret:K${RLO}y`)).toBe("Keep a saved key called “K y”");
    expect(wideningWords(`contribution:x${LRI}y`)).toBe("Add something of its own to PM: x y");
    expect(wideningWords(`odd${RLO}:thing`)).toBe("odd :thing");
  });
});
