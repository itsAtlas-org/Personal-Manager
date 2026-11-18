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
import {
  KEY_SEMANTIC_COORDS,
  KEY_SEMANTIC_COORDS_TSNE,
  KEY_SEMANTIC_META,
  KEY_SEMANTIC_META_TSNE,
  loadSemanticCoords,
} from "./semantic";
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
      [KEY_SEMANTIC_META_TSNE]: "cache",
      [KEY_SEMANTIC_COORDS_TSNE]: "cache",
      [KEY_PROJECT_LAYOUT]: "cache",
    });
  });

  it("every key is one PM's grammar accepts (a lowercase letter, then letters, digits, _ . -)", () => {
    for (const key of declared.keys()) expect(key).toMatch(/^[a-z][a-z0-9_.-]{0,63}$/);
  });

  it("a per-document key is cache class, because PM refuses a content key in storage.doc.set", () => {
    expect(declared.get(KEY_SEMANTIC_COORDS)).toBe("cache");
    expect(declared.get(KEY_SEMANTIC_COORDS_TSNE)).toBe("cache");
  });
});

// The published 1.0.0 (the manifest at 5682c3d3, the B1 tip) is what every later update is compared
// with. PM's `policy::widening` (src-tauri/src/mods/policy.rs:423) lists what an update asks for that the
// installed version did not: a permission, an internet host, a secret, a nav.tab or calendar.overlay
// contribution, or a help id. An unattended update that widens is refused and waits for the person's
// approval (ledger.rs:696). It does NOT look at `data.keys`, so adding the two enhanced-layout cache keys
// is free. These tests pin the rest to 1.0.0, so the enhanced layout (which this version already has)
// can never be the thing that makes a later update ask. The lists below were read from that commit.
describe("the manifest asks for nothing more than 1.0.0 did", () => {
  it("has exactly the help ids 1.0.0 had (a new help id is a widening)", () => {
    expect(manifest.contributions.help.map((h) => h.id)).toEqual([
      "map-overview",
      "map-canvas",
      "map-arrangement",
      "map-cohesion",
      "map-navigate",
      "map-names",
      "map-settings",
      "map-updates",
    ]);
  });

  it("asks for the same four permissions, no internet host and no secret", () => {
    expect(manifest.permissions.map((p) => p.id)).toEqual([
      "storage",
      "ui.tab",
      "documents.meta",
      "documents.vectors",
    ]);
    expect("internetHosts" in manifest).toBe(false);
    expect("secrets" in manifest).toBe(false);
  });

  it("contributes only a tab and help", () => {
    expect(Object.keys(manifest.contributions).sort()).toEqual(["help", "nav.tab"]);
  });

  it("keeps every key 1.0.0 declared, with the class it had (a key's class is read from the manifest at every write)", () => {
    for (const [key, cls] of [
      [KEY_SETTINGS, "content"],
      [KEY_SEMANTIC_META, "cache"],
      [KEY_SEMANTIC_COORDS, "cache"],
      [KEY_PROJECT_LAYOUT, "cache"],
    ]) {
      expect(declared.get(key), key).toBe(cls);
    }
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

describe("the help entries", () => {
  const help = manifest.contributions.help;

  it("are the eight the core Map had, reworded: an overview, the canvas, the arrangement, cohesion, moving, names, settings and refreshing", () => {
    expect(help.map((h) => h.id)).toEqual([
      "map-overview",
      "map-canvas",
      "map-arrangement",
      "map-cohesion",
      "map-navigate",
      "map-names",
      "map-settings",
      "map-updates",
    ]);
  });

  it("follow package.rs's rules for a help entry (the Rust parse of this file is a test of its own)", () => {
    expect(help.length).toBeLessThanOrEqual(64);
    expect(new Set(help.map((h) => h.id)).size).toBe(help.length);
    for (const h of help) {
      // is_word(id, 48, '-') and a lowercase first letter.
      expect(h.id, h.id).toMatch(/^[a-z][a-z0-9-]{0,47}$/);
      // plain_text: no edge space, no control character but a newline in a body, short enough.
      for (const [what, text, max, newlines] of [
        ["title", h.title, 80, false],
        ["body", h.body, 2000, true],
      ] as const) {
        expect(text.length, `${h.id} ${what}`).toBeGreaterThan(0);
        expect(Array.from(text).length, `${h.id} ${what}`).toBeLessThanOrEqual(max);
        expect(text.trim(), `${h.id} ${what}`).toBe(text);
        // Written as code points, not as the characters themselves, so this file holds none.
        const controls = (c: number) =>
          c < 32 || (c >= 127 && c <= 159) || c === 0x2028 || c === 0x2029;
        const invisible = (c: number) =>
          (c >= 0x200b && c <= 0x200f) ||
          (c >= 0x202a && c <= 0x202e) ||
          (c >= 0x2060 && c <= 0x206f) ||
          c === 0xfeff;
        for (const ch of text) {
          const c = ch.codePointAt(0) as number;
          const allowedNewline = newlines && c === 10;
          expect(controls(c) && !allowedNewline, `${h.id} ${what} has a control character`).toBe(
            false,
          );
          expect(invisible(c), `${h.id} ${what} has an invisible character`).toBe(false);
        }
      }
    }
  });

  it("name the enhanced (t-SNE) layout where the person looks for it: the arrangement, the settings and the refreshing", () => {
    const named = help.filter((h) => /t-?sne|enhanced/i.test(h.body)).map((h) => h.id);
    expect(named).toEqual(["map-arrangement", "map-settings", "map-updates"]);
    const body = (id: string) => help.find((h) => h.id === id)?.body ?? "";
    expect(body("map-settings")).toContain("Enhanced layout (t-SNE)");
    expect(body("map-updates")).toContain("Cancel");
    expect(body("map-updates")).toContain("Sharpen");
  });

  it("say nothing that is no longer so: no Settings tab for the Map, nothing kept on this device", () => {
    const all = help.map((h) => `${h.title}\n${h.body}`).join("\n");
    // Nothing is downloaded any more (the layout is built in), and the page runs it in a Worker.
    expect(all).not.toMatch(/download|install/i);
    expect(all).not.toMatch(/Memory map/i);
    expect(all).not.toMatch(/remembered on this device/i);
    expect(all).not.toMatch(/background/i);
  });

  it("describe the controls the page really has", () => {
    const all = help.map((h) => h.body).join("\n");
    for (const word of ["By project", "Semantic", "Cohesion", "Names"]) {
      expect(all.toLowerCase()).toContain(word.toLowerCase());
    }
    // The settings entry names the cap by the label the page shows.
    expect(help.find((h) => h.id === "map-settings")?.body).toContain(
      "Most documents placed by meaning",
    );
    expect(help.find((h) => h.id === "map-settings")?.body).toMatch(/200 to 5,000/);
  });

  it("never offers a path or a file location: the Map is not given one", () => {
    expect(help.map((h) => h.body).join("\n")).not.toMatch(/source path|file path/i);
  });
});

describe("the page's own calls", () => {
  it("theme.get and ui.openDocument are rows of the dispatch table under permissions the manifest asks for", () => {
    for (const method of ["theme.get", "ui.openDocument"]) {
      const permission = callRows.get(method);
      expect(permission, `${method} is not a call of the dispatch table`).toBeDefined();
      expect(permissions.has(permission as string), `${method} needs ${permission}`).toBe(true);
    }
    expect(callRows.get("theme.get")).toBe("storage");
    expect(callRows.get("ui.openDocument")).toBe("documents.meta");
    expect(eventRows.get("theme.changed")).toBe("storage");
  });

  it("the tab's label fits PM's limit and the description is plain text", () => {
    expect(manifest.contributions["nav.tab"].label.length).toBeLessThanOrEqual(24);
    expect(manifest.description.length).toBeLessThanOrEqual(200);
  });
});
