// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The engine and the manifest agree. The manifest is what the person is shown and what PM enforces:
// a key it declares `cache` is left out of backups, an undeclared key is `content` (kept, backed up),
// and a call whose permission the manifest lacks is refused at run time, in the user's vault, long
// after every test here went green. So this reads the committed manifest and the dispatch table (the
// Rust source, as text) and holds the engine to both: every key it writes is declared with the right
// class, and every call it makes, and every event it hears, is covered by a permission it asks for.

import { describe, expect, it } from "vitest";
import tableRs from "../../../src-tauri/src/mods/api/table.rs?raw";
import manifest from "../manifest.json";
import { inlineCompute } from "./compute";
import { listDocuments } from "./hostApi";
import { watchDocumentChanges } from "./changes";
import { memoryHost } from "./memoryHost";
import { loadProjectLayout, KEY_PROJECT_LAYOUT } from "./projectLayout";
import { KEY_SEMANTIC_COORDS, KEY_SEMANTIC_META, loadSemanticCoords } from "./semantic";
import { KEY_SETTINGS, loadSettings, saveSettings } from "./settings";
import { documents, vectorsFor } from "./testing";

const declared = new Map(manifest.data.keys.map((k) => [k.key, k.class]));
const permissions = new Set(manifest.permissions.map((p) => p.id));

/** `call("name", "permission", ...)` rows of the dispatch table, whichever way rustfmt wrapped them. */
const callRows = new Map(
  Array.from(tableRs.matchAll(/\bcall\(\s*"([\w.]+)",\s*"([\w.]+)"/g)).map((m) => [m[1], m[2]]),
);
const eventRows = new Map(
  Array.from(tableRs.matchAll(/\bevent\(\s*"([\w.]+)",\s*Some\("([\w.]+)"\)/g)).map((m) => [
    m[1],
    m[2],
  ]),
);

describe("the committed manifest", () => {
  it("declares exactly the keys the engine uses, the user's one as content and the rest as cache", () => {
    expect(Object.fromEntries(declared)).toEqual({
      [KEY_SETTINGS]: "content",
      [KEY_SEMANTIC_META]: "cache",
      [KEY_SEMANTIC_COORDS]: "cache",
      [KEY_PROJECT_LAYOUT]: "cache",
    });
  });

  it("every key is one PM's grammar accepts (a lowercase letter, then letters, digits, _ . -)", () => {
    for (const key of declared.keys()) expect(key).toMatch(/^[a-z][a-z0-9_.-]{0,63}$/);
  });

  it("a per-document key is cache class, because PM refuses a content key in storage.doc.set", () => {
    expect(declared.get(KEY_SEMANTIC_COORDS)).toBe("cache");
  });
});

describe("what the engine does is covered by what the manifest asks for", () => {
  /** Everything the engine touches, once each, against a host that records it. */
  async function everything(): Promise<string[]> {
    const docs = documents(30, 3);
    const pm = memoryHost({
      docs,
      vectors: vectorsFor(docs, 8),
      dimension: 8,
      contentKeys: [KEY_SETTINGS],
    });
    const compute = inlineCompute();
    await loadSettings(pm.host);
    await saveSettings(pm.host, { layoutMode: "semantic" });
    const listed = await listDocuments(pm.host);
    await loadSemanticCoords(pm.host, compute, { nodeCap: 1000, sleep: () => Promise.resolve() });
    await loadSemanticCoords(pm.host, compute, { nodeCap: 1000, sleep: () => Promise.resolve() });
    await loadProjectLayout(pm.host, compute, listed);
    await loadProjectLayout(pm.host, compute, listed);
    return Array.from(new Set(pm.calls.map((c) => c.method)));
  }

  it("every call is a row of the dispatch table whose permission is in the manifest", async () => {
    const methods = await everything();
    expect(methods.sort()).toEqual([
      "documents.list",
      "embeddings.documentVectors",
      "embeddings.signature",
      "storage.doc.delete",
      "storage.doc.list",
      "storage.doc.set",
      "storage.get",
      "storage.set",
    ]);
    for (const method of methods) {
      const permission = callRows.get(method);
      expect(permission, `${method} is not a call of the dispatch table`).toBeDefined();
      expect(permissions.has(permission as string), `${method} needs ${permission}`).toBe(true);
    }
  });

  it("the events it listens to need a permission the manifest has", () => {
    const heard = ["documents.added", "documents.deleted"];
    for (const name of heard) {
      const permission = eventRows.get(name);
      expect(permission, `${name} is not an event of the dispatch table`).toBeDefined();
      expect(permissions.has(permission as string)).toBe(true);
    }
    // And the watcher really does register one handler for them.
    const pm = memoryHost({ docs: [] });
    watchDocumentChanges(pm.host, () => undefined);
    expect(pm.handlers.size).toBe(1);
  });

  it("every permission the engine needs is asked for, and the manifest asks for no data permission it has no use for", async () => {
    const used = new Set((await everything()).map((m) => callRows.get(m)));
    for (const p of ["storage", "documents.meta", "documents.vectors"]) {
      expect(used.has(p)).toBe(true);
      expect(permissions.has(p)).toBe(true);
    }
    // ui.tab is the page's (the tab itself), not a call the engine makes.
    expect(Array.from(permissions).sort()).toEqual([
      "documents.meta",
      "documents.vectors",
      "storage",
      "ui.tab",
    ]);
  });
});
