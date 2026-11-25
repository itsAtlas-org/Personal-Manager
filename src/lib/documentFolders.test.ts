// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { beforeEach, describe, expect, it } from "vitest";
import {
  buildFolderTree,
  openFolder,
  readDocView,
  readFolderPath,
  writeDocView,
  writeFolderPath,
  type FolderNode,
} from "./documentFolders";
import type { Document, MadeBy } from "./types";

let nextId = 1;
/** A document row with only the fields the folder views read; the rest are inert defaults. */
function doc(over: Partial<Document> & { title: string }): Document {
  return {
    id: nextId++,
    source_path: null,
    project: "Unsorted",
    linked_projects: [],
    source_type: "vault",
    source_state: "ok",
    external_ref: null,
    source_id: null,
    source_parent_folder_name: null,
    source_folder_path: null,
    ...over,
  } as unknown as Document;
}

const names = (n: FolderNode) => n.folders.map((f) => f.name);
const titles = (n: FolderNode) => n.files.map((d) => d.title);

describe("the Folders view", () => {
  it("puts each file under where it came from, then the source's own folders", () => {
    const tree = buildFolderTree(
      [
        doc({
          title: "plan",
          source_type: "index_only",
          source_id: "gdrive:me@x.com:f1",
          source_folder_path: ["My Drive", "Work"],
        }),
        doc({
          title: "brief",
          source_type: "index_only",
          source_id: "gdrive:me@x.com:f2",
          source_folder_path: ["My Drive"],
        }),
        doc({ title: "dropped" }),
      ],
      "folders",
    );
    expect(names(tree)).toEqual(["Google Drive", "Added to PM"]);
    const drive = tree.folders[0];
    expect(drive.detail).toBe("me@x.com");
    expect(drive.total).toBe(2);
    const myDrive = drive.folders[0];
    expect(myDrive.name).toBe("My Drive");
    expect(titles(myDrive)).toEqual(["brief"]);
    expect(titles(myDrive.folders[0])).toEqual(["plan"]);
    expect(titles(tree.folders[1])).toEqual(["dropped"]);
    expect(tree.files).toEqual([]);
  });

  it("keeps two Google accounts apart, and orders the top level by place, not by name", () => {
    const tree = buildFolderTree(
      [
        doc({ title: "chat", source_type: "chat", source_id: "chat:1" }),
        doc({ title: "note", source_id: "note:w1" }),
        doc({ title: "photo", source_type: "photo" }),
        doc({ title: "b", source_type: "index_only", source_id: "gdrive:b@x.com:1" }),
        doc({ title: "a", source_type: "index_only", source_id: "gdrive:a@x.com:1" }),
        doc({ title: "od", source_type: "index_only", source_id: "onedrive:a@x.com:1" }),
        doc({ title: "swm", source_type: "index_only", source_id: "gdrive:swm:root:1" }),
        doc({ title: "sd", source_type: "index_only", source_id: "gdrive:sd:drive:1" }),
      ],
      "folders",
    );
    expect(tree.folders.map((f) => [f.name, f.detail ?? ""])).toEqual([
      ["Google Drive", "a@x.com"],
      ["Google Drive", "b@x.com"],
      ["Shared drives", "Google Drive"],
      ["Shared with you", "Google Drive"],
      ["OneDrive", "a@x.com"],
      ["Photos", ""],
      ["Pinboard", "First party · notes you saved"],
      ["Chats", ""],
    ]);
  });

  describe("a mod's documents (PR C, K5)", () => {
    const mods = new Map<string, MadeBy>([
      ["acme.notes", { name: "Acme Notes", tier: "second", signed: true }],
      // An unsigned mod that calls itself "Pinboard".
      ["evil.board", { name: "Pinboard", tier: "third", signed: false }],
    ]);
    const group = (docs: Document[]) =>
      buildFolderTree(docs, "folders", undefined, mods).folders.map((f) => [
        f.name,
        f.detail ?? "",
        f.total,
      ]);

    it("groups documents by the mod that made them, named by its row and its tier word", () => {
      expect(
        group([
          doc({ title: "a", source_id: "mod:acme.notes:k1" }),
          doc({ title: "b", source_id: "mod:acme.notes:k2" }),
        ]),
      ).toEqual([["Acme Notes", "reviewed by Atlas", 2]]);
    });

    it("says 'a removed mod' for a mod PM no longer holds, one group for each", () => {
      expect(
        group([
          doc({ title: "a", source_id: "mod:gone.one:k1" }),
          doc({ title: "b", source_id: "mod:gone.two:k1" }),
          doc({ title: "c", source_id: "mod:gone.one:k2" }),
        ]),
      ).toEqual([
        ["a removed mod", "No longer installed", 2],
        ["a removed mod", "No longer installed", 1],
      ]);
    });

    it("does not call a mod's group removed while the mods' rows have not been read", () => {
      const docs = [doc({ title: "a", source_id: "mod:acme.notes:k1" })];
      const tree = buildFolderTree(docs, "folders", undefined, null);
      expect(tree.folders.map((f) => [f.name, f.detail, f.total])).toEqual([
        ["A mod", "Mods not read yet", 1],
      ]);
      // The same group, once they are read: the key (and so a remembered open folder) is unchanged.
      const read = buildFolderTree(docs, "folders", undefined, mods);
      expect(read.folders[0].key).toBe(tree.folders[0].key);
      expect(read.folders[0].name).toBe("Acme Notes");
    });

    it("an unsigned mod named Pinboard is a group of its own and says unsigned", () => {
      const rows = group([
        doc({ title: "legacy", source_id: "note:w-1" }),
        doc({ title: "forged", source_id: "mod:evil.board:k" }),
      ]);
      expect(rows).toEqual([
        ["Pinboard", "First party · notes you saved", 1],
        ["Pinboard", "unsigned", 1],
      ]);
    });

    it("the Pinboard's old notes are decided by their prefix, never by an installed mod's name", () => {
      // No mods installed at all: the note: group is still the Pinboard's, by the constant.
      const tree = buildFolderTree([doc({ title: "legacy", source_id: "note:w-1" })], "folders");
      expect(tree.folders.map((f) => f.name)).toEqual(["Pinboard"]);
    });

    it("keeps chat's saved answers out of the Pinboard's group", () => {
      const tree = buildFolderTree(
        [
          doc({ title: "mine", source_id: "note:w-1" }),
          doc({ title: "answer", source_id: "note:chat-77" }),
        ],
        "folders",
      );
      expect(tree.folders.map((f) => [f.name, titles(f)])).toEqual([
        ["Pinboard", ["mine"]],
        ["Chats", ["answer"]],
      ]);
    });

    it("clamps a mod's name and strips what could reorder it", () => {
      const long = new Map<string, MadeBy>([
        ["x.y", { name: "\u202e" + "n".repeat(300), tier: "third", signed: true }],
      ]);
      const [folder] = buildFolderTree(
        [doc({ title: "a", source_id: "mod:x.y:k" })],
        "folders",
        undefined,
        long,
      ).folders;
      expect([...folder.name].length).toBeLessThanOrEqual(40);
      expect(folder.name).not.toContain("\u202e");
      expect(folder.detail).toBe("third party");
    });
  });

  it("names a tracked folder on this device after its own directory", () => {
    const tree = buildFolderTree(
      [
        doc({
          title: "deep",
          source_type: "index_only",
          source_id: "local:abc:1",
          external_ref: "/home/me/Notes/2026/deep.md",
          source_folder_path: ["2026"],
        }),
        doc({
          title: "top",
          source_type: "index_only",
          source_id: "local:abc:2",
          external_ref: "/home/me/Notes/top.md",
          source_folder_path: [],
        }),
      ],
      "folders",
    );
    expect(tree.folders).toHaveLength(1);
    expect(tree.folders[0].name).toBe("Notes");
    expect(tree.folders[0].detail).toBe("On this device");
    expect(titles(tree.folders[0])).toEqual(["top"]);
    expect(titles(tree.folders[0].folders[0])).toEqual(["deep"]);
  });

  it("keeps the order the files were handed in, so the table's sort carries over", () => {
    const tree = buildFolderTree([doc({ title: "z" }), doc({ title: "a" })], "folders");
    expect(titles(tree.folders[0])).toEqual(["z", "a"]);
  });
});

describe("the Projects view", () => {
  it("puts a file in every project it belongs to, and counts each once per folder", () => {
    const shared = doc({ title: "shared", project: "Beta", linked_projects: ["Alpha"] });
    const tree = buildFolderTree([shared, doc({ title: "solo", project: "Beta" })], "projects");
    expect(names(tree)).toEqual(["Alpha", "Beta"]);
    expect(titles(tree.folders[0])).toEqual(["shared"]);
    expect(titles(tree.folders[1])).toEqual(["shared", "solo"]);
    expect(tree.total).toBe(2);
  });

  it("leaves out hidden projects' folders", () => {
    const tree = buildFolderTree(
      [doc({ title: "x", project: "Live", linked_projects: ["Old"] })],
      "projects",
      new Set(["Old"]),
    );
    expect(names(tree)).toEqual(["Live"]);
  });
});

describe("openFolder", () => {
  const tree = buildFolderTree(
    [
      doc({
        title: "plan",
        source_type: "index_only",
        source_id: "gdrive:me@x.com:f1",
        source_folder_path: ["My Drive", "Work"],
      }),
    ],
    "folders",
  );

  it("follows a path of keys and returns the folders on the way", () => {
    const at = openFolder(tree, ["gdrive:me@x.com", "f:My Drive", "f:Work"]);
    expect(at.trail.map((f) => f.name)).toEqual(["", "Google Drive", "My Drive", "Work"]);
    expect(titles(at.folder)).toEqual(["plan"]);
  });

  it("stops at the deepest folder that still exists", () => {
    const at = openFolder(tree, ["gdrive:me@x.com", "f:Gone", "f:Work"]);
    expect(at.folder.name).toBe("Google Drive");
    expect(at.path).toEqual(["gdrive:me@x.com"]);
  });
});

describe("remembered view and folder", () => {
  beforeEach(() => localStorage.clear());

  it("defaults to all files at the top, and reads back what was written", () => {
    expect(readDocView()).toBe("all");
    expect(readFolderPath("folders")).toEqual([]);
    writeDocView("projects");
    writeFolderPath("projects", ["p:Alpha"]);
    expect(readDocView()).toBe("projects");
    expect(readFolderPath("projects")).toEqual(["p:Alpha"]);
    expect(readFolderPath("folders")).toEqual([]);
  });

  it("reads junk as the defaults", () => {
    localStorage.setItem("pm.documents.view", "grid");
    localStorage.setItem("pm.documents.folder.folders", '{"a":1}');
    expect(readDocView()).toBe("all");
    expect(readFolderPath("folders")).toEqual([]);
  });
});
