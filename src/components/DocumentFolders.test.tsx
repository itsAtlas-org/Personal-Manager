// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { buildFolderTree, openFolder } from "../lib/documentFolders";
import type { Document, MadeBy } from "../lib/types";
import { FolderCrumbs } from "./DocumentFolders";

afterEach(cleanup);

const doc = (title: string, source_id: string) =>
  ({
    id: title.length,
    title,
    source_id,
    source_type: "vault",
    source_path: null,
    project: "Unsorted",
    linked_projects: [],
    source_state: "ok",
    external_ref: null,
    source_parent_folder_name: null,
    source_folder_path: null,
  }) as unknown as Document;

const mods = new Map<string, MadeBy>([
  ["evil.board", { name: "Pinboard", tier: "third", signed: false }],
  ["acme.notes", { name: "Acme Notes", tier: "second", signed: true }],
]);

function crumbs(sourceId: string) {
  const root = buildFolderTree([doc("a", sourceId)], "folders", undefined, mods);
  const key = root.folders[0].key;
  const { trail } = openFolder(root, [key]);
  render(<FolderCrumbs trail={trail} rootLabel="Folders" onOpen={() => {}} />);
  return screen.getByRole("navigation", { name: "Folder" });
}

describe("the breadcrumb inside a mod's Documents group", () => {
  it("says unsigned beside the name of an unsigned mod that calls itself Pinboard", () => {
    const nav = crumbs("mod:evil.board:k");
    expect(nav.textContent).toContain("Pinboard");
    expect(nav.textContent).toContain("unsigned");
  });

  it("does not read like the Pinboard's own group, which names itself first party", () => {
    const own = crumbs("note:w-1");
    expect(own.textContent).toContain("First party");
    expect(own.textContent).not.toContain("unsigned");
  });

  it("carries the tier word of a reviewed mod too", () => {
    expect(crumbs("mod:acme.notes:k").textContent).toContain("reviewed by Atlas");
  });

  it("leaves the top crumb as the label it was given", () => {
    crumbs("mod:acme.notes:k");
    expect(screen.getByRole("button", { name: "Folders" })).toBeTruthy();
  });
});
