// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Map's checked Host API calls: that they send what PM's handlers accept, that they read what
// PM answers defensively, and that the codes and limits they lean on are PM's (pinned to the Rust
// source, so a renumbering or a changed cap fails a test here and not a user's Map).

import { describe, expect, it } from "vitest";
import documentsRs from "../../../src-tauri/src/mods/api/documents.rs?raw";
import errorRs from "../../../src-tauri/src/mods/api/error.rs?raw";
import ingestRs from "../../../src-tauri/src/ingest.rs?raw";
import limitsRs from "../../../src-tauri/src/mods/api/limits.rs?raw";
import { ModCallError } from "../../../src/kit/host";
import {
  byNewest,
  clearDocRows,
  CODE,
  codeOf,
  HostAnswerError,
  isCount,
  listDocRows,
  listDocuments,
  LIST_FIELDS,
  MAX_ARGS_BYTES,
  MAX_DOCS_PER_CALL,
  readDoc,
  readKey,
  readSignature,
  setDocRows,
  writeKey,
  type MapHost,
} from "./hostApi";
import { memoryHost } from "./memoryHost";
import { doc } from "./testing";

/** A host that answers every call with `reply`. */
const answering = (reply: unknown): MapHost => ({
  call: () => Promise.resolve(reply),
  documentVectors: () => Promise.reject(new Error("not used")),
  onEvent: () => () => undefined,
});

describe("pinned to PM", () => {
  const code = (name: string) => new RegExp(`Self::${name} => "(MOD-\\d{4})"`).exec(errorRs)?.[1];

  it("the codes the engine acts on are the Rust codes of the same name", () => {
    expect(code("RateLimited")).toBe(CODE.rateLimited);
    expect(code("QuotaExceeded")).toBe(CODE.quotaExceeded);
    expect(code("StaleRevision")).toBe(CODE.staleRevision);
  });

  it("the page and batch limits are the backend's", () => {
    expect(new RegExp(`MAX_DOCS_PER_CALL: usize = ${MAX_DOCS_PER_CALL};`).test(limitsRs)).toBe(
      true,
    );
    expect(limitsRs).toMatch(/MAX_ARGS_BYTES: usize = 1 << 20;/);
    expect(MAX_ARGS_BYTES).toBe(1 << 20);
  });

  it("the Map asks for every field of the list DTO and no other", () => {
    const body = documentsRs.slice(documentsRs.indexOf("fn name(self)"));
    const rust = Array.from(body.slice(0, 700).matchAll(/Self::\w+ => "(\w+)"/g)).map((m) => m[1]);
    expect(rust).toHaveLength(9);
    expect([...LIST_FIELDS].sort()).toEqual([...rust].sort());
  });
});

describe("codeOf and isCount", () => {
  it("reads a PM refusal's code and nothing else's", () => {
    expect(codeOf(new ModCallError({ code: "MOD-4001", message: "x" }))).toBe("MOD-4001");
    expect(codeOf(new Error("x"))).toBeNull();
    expect(codeOf({ code: 5 })).toBeNull();
    expect(codeOf(null)).toBeNull();
    expect(codeOf("MOD-4001")).toBeNull();
  });

  it("a count is a whole number from zero", () => {
    expect([0, 1, 2 ** 53 - 1].every(isCount)).toBe(true);
    expect([-1, 1.5, NaN, Infinity, 2 ** 53, "1", null, undefined].some(isCount)).toBe(false);
  });
});

describe("readDoc", () => {
  const full = {
    id: 7,
    title: "A title",
    homeProject: { id: 3, name: "  Atlas " },
    importance: "high",
    chunkCount: 12,
    reviewed: true,
    sourceType: "index_only",
    tags: ["a", "b"],
    ingestedAt: "2026-02-03T04:05:06Z",
  };

  it("reads every field, and trims the project name", () => {
    expect(readDoc(full)).toEqual({
      id: 7,
      title: "A title",
      project: "Atlas",
      importance: "high",
      chunkCount: 12,
      reviewed: true,
      sourceType: "index_only",
      tags: ["a", "b"],
      ingestedAt: "2026-02-03T04:05:06Z",
    });
  });

  it("a document with no home project has the blank one (Unsorted at draw time)", () => {
    expect(readDoc({ ...full, homeProject: null })?.project).toBe("");
    expect(readDoc({ ...full, homeProject: { id: 1, name: "   " } })?.project).toBe("");
    expect(readDoc({ ...full, homeProject: { id: 1 } })?.project).toBe("");
  });

  it("takes safe defaults for a field of the wrong type, but never invents an id", () => {
    const odd = readDoc({
      id: 1,
      title: 5,
      importance: 3,
      chunkCount: -2,
      reviewed: "yes",
      sourceType: null,
      tags: ["ok", 1, null],
      ingestedAt: 9,
    });
    expect(odd).toEqual({
      id: 1,
      title: "",
      project: "",
      importance: null,
      chunkCount: 0,
      reviewed: false,
      sourceType: "vault",
      tags: ["ok"],
      ingestedAt: "",
    });
    for (const id of [undefined, null, "1", -1, 1.5, NaN]) {
      expect(readDoc({ ...full, id })).toBeNull();
    }
    expect(readDoc(null)).toBeNull();
    expect(readDoc([])).toBeNull();
    expect(readDoc("x")).toBeNull();
  });
});

describe("listDocuments", () => {
  it("reads every page, asking for the nine fields and the largest page", async () => {
    const docs = Array.from({ length: 23 }, (_, i) => doc(i + 1, i % 2 ? "A" : ""));
    const pm = memoryHost({ docs, pageSize: 7 });
    const listed = await listDocuments(pm.host);
    // Every page's documents are there (the same ingest time, so newest-first is id descending).
    expect(listed.map((d) => d.id)).toEqual(docs.map((d) => d.id).reverse());
    const calls = pm.of("documents.list");
    expect(calls).toHaveLength(4);
    expect(calls.map((c) => (c.args as { after: number }).after)).toEqual([0, 7, 14, 21]);
    expect(calls[0].args).toEqual({ fields: LIST_FIELDS, after: 0, limit: 5000 });
  });

  it("one page and no cursor is one call", async () => {
    const pm = memoryHost({ docs: [doc(1, "A")] });
    expect(await listDocuments(pm.host)).toHaveLength(1);
    expect(pm.of("documents.list")).toHaveLength(1);
  });

  it("no documents is an empty list", async () => {
    expect(await listDocuments(memoryHost({ docs: [] }).host)).toEqual([]);
  });

  it("skips a document it cannot read and keeps the others", async () => {
    const host = answering({
      documents: [{ id: 1, title: "a" }, { nope: 1 }, { id: 2 }],
      next: null,
    });
    expect((await listDocuments(host)).map((d) => d.id)).toEqual([2, 1]);
  });

  it("lists in the core Map's order, newest ingested first, across pages (so the colours match)", async () => {
    // PM pages by id; the newest documents are not the highest ids and they sit on different pages.
    const at = (id: number, ingestedAt: string) => ({ ...doc(id, "A"), ingestedAt });
    const docs = [
      at(1, "2026-03-01T00:00:00Z"),
      at(2, "2026-01-01T00:00:00Z"),
      at(3, "2026-03-01T00:00:00Z"),
      at(4, "2026-02-01T00:00:00Z"),
      at(5, "2026-03-01T00:00:00Z"),
    ];
    const listed = await listDocuments(memoryHost({ docs, pageSize: 2 }).host);
    expect(listed.map((d) => d.id)).toEqual([5, 3, 1, 4, 2]);
  });

  it("the order is the one ingest.rs gives the core Map's list", () => {
    expect(ingestRs).toContain("FROM documents d ORDER BY d.ingested_at DESC, d.id DESC");
  });

  it("byNewest compares the timestamps as text and breaks a tie by the higher id", () => {
    const at = (id: number, ingestedAt: string) => ({ ...doc(id, "A"), ingestedAt });
    expect(byNewest(at(1, "2026-02-01"), at(2, "2026-01-01"))).toBeLessThan(0);
    expect(byNewest(at(1, "2026-01-01"), at(2, "2026-02-01"))).toBeGreaterThan(0);
    expect(byNewest(at(1, "x"), at(2, "x"))).toBeGreaterThan(0);
    expect(byNewest(at(2, "x"), at(1, "x"))).toBeLessThan(0);
    // A document with no readable time sorts last, like an empty string would.
    expect(byNewest(at(1, ""), at(2, "2026-01-01"))).toBeGreaterThan(0);
  });

  it.each([
    ["not an object", null],
    ["no documents array", { next: null }],
    ["a cursor that is not a number", { documents: [], next: "5" }],
    ["a cursor that does not move forward", { documents: [], next: 0 }],
  ])("%s is PM's answer not being what the Map expects", async (_name, reply) => {
    await expect(listDocuments(answering(reply))).rejects.toBeInstanceOf(HostAnswerError);
  });

  it("a cursor that stops moving is refused at once, not paged until the bound", async () => {
    let calls = 0;
    const host: MapHost = {
      ...answering(null),
      call: () => {
        calls += 1;
        return Promise.resolve({ documents: [], next: 10 });
      },
    };
    await expect(listDocuments(host)).rejects.toBeInstanceOf(HostAnswerError);
    // The first page moved the cursor to 10; the second answered 10 again.
    expect(calls).toBe(2);
  });

  it("a PM that never stops paging is stopped, not followed forever", async () => {
    let n = 0;
    const host: MapHost = {
      ...answering(null),
      call: () => Promise.resolve({ documents: [], next: ++n }),
    };
    await expect(listDocuments(host)).rejects.toBeInstanceOf(HostAnswerError);
    expect(n).toBe(1000);
  });

  it("a refusal from PM comes through as PM's error", async () => {
    const pm = memoryHost({ docs: [doc(1, "A")] });
    pm.failAlways("documents.list", "MOD-2001");
    await expect(listDocuments(pm.host)).rejects.toMatchObject({ code: "MOD-2001" });
  });
});

describe("readSignature", () => {
  it("reads count, dimension and hash", async () => {
    expect(await readSignature(answering({ count: 4, dimension: 384, hash: "abc" }))).toEqual({
      count: 4,
      dimension: 384,
      hash: "abc",
    });
  });

  it.each([
    ["not an object", 5],
    ["a negative count", { count: -1, dimension: 3, hash: "a" }],
    ["a fractional dimension", { count: 1, dimension: 3.5, hash: "a" }],
    ["no hash", { count: 1, dimension: 3 }],
    ["an empty hash", { count: 1, dimension: 3, hash: "" }],
    ["a huge hash", { count: 1, dimension: 3, hash: "x".repeat(129) }],
    ["a numeric hash", { count: 1, dimension: 3, hash: 5 }],
  ])("%s", async (_name, reply) => {
    await expect(readSignature(answering(reply))).rejects.toBeInstanceOf(HostAnswerError);
  });
});

describe("storage", () => {
  it("readKey: null and revision 0 for nothing stored, then what was written", async () => {
    const pm = memoryHost({ docs: [] });
    expect(await readKey(pm.host, "k")).toEqual({ value: null, revision: 0 });
    expect(await writeKey(pm.host, "k", { a: 1 }, 0)).toBe(1);
    expect(await readKey(pm.host, "k")).toEqual({ value: { a: 1 }, revision: 1 });
  });

  it("writeKey with a stale revision is PM's MOD-4001, and changes nothing", async () => {
    const pm = memoryHost({ docs: [] });
    await writeKey(pm.host, "k", 1, 0);
    await expect(writeKey(pm.host, "k", 2, 0)).rejects.toMatchObject({ code: CODE.staleRevision });
    expect((await readKey(pm.host, "k")).value).toBe(1);
  });

  it.each([
    ["readKey", (h: MapHost) => readKey(h, "k")],
    ["writeKey", (h: MapHost) => writeKey(h, "k", 1, 0)],
    ["clearDocRows", (h: MapHost) => clearDocRows(h, "k")],
    ["setDocRows", (h: MapHost) => setDocRows(h, "k", [{ documentId: 1, value: 1 }])],
    ["listDocRows", (h: MapHost) => listDocRows(h, "k")],
  ])("%s refuses an answer that is not the shape PM sends", async (_name, run) => {
    await expect(run(answering({ unexpected: true }))).rejects.toBeInstanceOf(HostAnswerError);
    await expect(run(answering(null))).rejects.toBeInstanceOf(HostAnswerError);
  });

  it("setDocRows: PM must say it wrote what was sent", async () => {
    await expect(
      setDocRows(answering({ written: 1 }), "k", [
        { documentId: 1, value: 1 },
        { documentId: 2, value: 1 },
      ]),
    ).rejects.toBeInstanceOf(HostAnswerError);
  });

  it("per-document rows: set, list in pages by id, clear", async () => {
    const docs = Array.from({ length: 10 }, (_, i) => doc(i + 1, "A"));
    const pm = memoryHost({ docs, pageSize: 4 });
    await setDocRows(
      pm.host,
      "rows",
      docs.map((d) => ({ documentId: d.id, value: { n: d.id } })),
    );
    const rows = await listDocRows(pm.host, "rows");
    expect(rows.map((r) => r.documentId)).toEqual(docs.map((d) => d.id));
    expect(pm.of("storage.doc.list")).toHaveLength(3);
    await clearDocRows(pm.host, "rows");
    expect(await listDocRows(pm.host, "rows")).toEqual([]);
  });

  it("listDocRows: a cursor that does not move, or an entry with no id, is refused", async () => {
    let calls = 0;
    const standing: MapHost = {
      ...answering(null),
      call: () => {
        calls += 1;
        return Promise.resolve({ entries: [], next: 0 });
      },
    };
    await expect(listDocRows(standing, "k")).rejects.toBeInstanceOf(HostAnswerError);
    expect(calls).toBe(1);
    await expect(
      listDocRows(answering({ entries: [{ value: 1 }], next: null }), "k"),
    ).rejects.toBeInstanceOf(HostAnswerError);
  });
});
