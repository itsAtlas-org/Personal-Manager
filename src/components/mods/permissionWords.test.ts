// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The plain words for each permission (permissionWords.ts) cover exactly the permissions the backend
// has: a new one without words would be shown to the user as a raw id, and a retired one's words would
// say something nothing can do. The list is read from the backend's table.

import { describe, expect, it } from "vitest";
import permissionsRs from "../../../src-tauri/src/mods/api/permissions.rs?raw";
import { classWords, PERMISSION_IDS, permissionWords } from "./permissionWords";

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
