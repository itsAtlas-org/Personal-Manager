// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// PM's side of the calls the Map makes, in memory, for the engine's tests: documents.list,
// embeddings.signature and documentVectors, storage.get/set, storage.doc.*, and the events. It is written
// from what the Rust handlers do (src-tauri/src/mods/api/{documents,embeddings,storage,doc_storage}.rs),
// including the limits the engine has to live within (5,000 per page or batch, 1 MiB of arguments, a
// growth bucket on writes that add bytes, revisions on storage.set, cache-only doc keys), so a test that
// passes here was not helped by a host kinder than PM's. No test uses it to say what PM does: the Rust
// tests do that. Only tests import it, so no bundle holds it.

import {
  ModCallError,
  readDocumentVectors,
  type DocumentVectors,
  type HostEvent,
} from "../../../src/kit/host";
import type { Handler } from "../../../src/kit/fakePm";
import type { MapHost } from "./hostApi";
import type { MapDoc } from "./types";

export interface MemoryHostOptions {
  docs: MapDoc[];
  /** Embedded documents' vectors, by id: a document with none has no vector and is no node. */
  vectors?: Map<number, Float32Array>;
  dimension?: number;
  /** `documents.list` and `storage.doc.list` end a page early at this many (PM ends them under a byte
   *  budget). Default: the whole page. */
  pageSize?: number;
  /** Writes that add bytes allowed before MOD-3001, until `refill()` (PM: a bucket of 40, then 4/s). */
  growthBudget?: number;
  /** Storage keys the manifest declares as content: `storage.doc.set` refuses them. */
  contentKeys?: string[];
  /** What `theme.get` answers (PM's page answers it: modTheme.ts). Default: a dark Slate-like theme. */
  theme?: unknown;
}

export interface Call {
  method: string;
  args: unknown;
}

const LIMIT = 5000;
const ARGS_BYTES = 1 << 20;
const LIST_FIELDS = [
  "id",
  "title",
  "homeProject",
  "importance",
  "chunkCount",
  "reviewed",
  "sourceType",
  "tags",
  "ingestedAt",
];

/** What the vectors call answers inside this file: the views and the buffer both lie over. */
interface Binary {
  binary: true;
  vectors: DocumentVectors;
  buffer: ArrayBuffer;
}

/** A 64-hex-character hash of `text` (PM's signature hash is a sha256: the length is the point). */
function fakeHash(text: string): string {
  let out = "";
  for (let seed = 1; seed <= 8; seed++) {
    let h = (0x811c9dc5 ^ seed) >>> 0;
    for (let i = 0; i < text.length; i++) h = Math.imul(h ^ text.charCodeAt(i), 0x01000193) >>> 0;
    out += h.toString(16).padStart(8, "0");
  }
  return out;
}

type BackendReply = Awaited<ReturnType<Handler>>;

const refuse = (code: string, message: string) => new ModCallError({ code, message });
const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

export interface MemoryHost {
  host: MapHost;
  calls: Call[];
  /** Calls of one method, in order. */
  of(method: string): Call[];
  /** The documents and the embedded subset can change while a Map is open. */
  addDoc(doc: MapDoc, vector?: Float32Array): void;
  removeDoc(id: number): void;
  /** A re-index: the chunks' identity changes, so the signature's hash does. */
  reindex(): void;
  /** Give the growth budget more tokens. */
  refill(tokens: number): void;
  /** Make the nth next call of `method` (1 = the next) fail with `code`. */
  failNext(method: string, code: string, nth?: number): void;
  /** Make every call of `method` fail with `code` until cleared. */
  failAlways(method: string, code: string | null): void;
  /** What PM stores. */
  kv: Map<string, { value: unknown; revision: number }>;
  rows: Map<string, Map<number, unknown>>;
  /** The same PM as a fake PM's handler (src/kit/fakePm.ts), so the engine can run through the real SDK. */
  backend: Handler;
  /** Deliver an event to the handlers the Map registered. */
  emit(event: HostEvent): void;
  handlers: Set<(event: HostEvent) => void>;
  /** The ids `ui.openDocument` opened (PM's page opens its reader; here it is only recorded). */
  opened: number[];
  /** Change what `theme.get` answers from now on. */
  setTheme(theme: unknown): void;
}

export function memoryHost(options: MemoryHostOptions): MemoryHost {
  const docs = new Map(options.docs.map((d) => [d.id, d]));
  const vectors = new Map(options.vectors ?? []);
  const dimension = options.dimension ?? vectors.values().next().value?.length ?? 8;
  const contentKeys = new Set(options.contentKeys ?? []);
  const pageSize = options.pageSize ?? LIMIT;
  let growth = options.growthBudget ?? Infinity;
  let generation = 1;
  const kv = new Map<string, { value: unknown; revision: number }>();
  const rows = new Map<string, Map<number, unknown>>();
  const calls: Call[] = [];
  const handlers = new Set<(event: HostEvent) => void>();
  const failures: { method: string; code: string; left: number }[] = [];
  const always = new Map<string, string>();
  const opened: number[] = [];
  let theme: unknown = options.theme ?? {
    vars: { "--bg": "oklch(0.16 0.01 270)", "--ink": "oklch(0.95 0 0)" },
    mode: "dark",
    system: "slate",
    accent: "mono",
    colorblind: false,
    depth: "standard",
  };

  const embedded = () =>
    Array.from(vectors.keys())
      .filter((id) => docs.has(id))
      .sort((a, b) => a - b);

  const growing = () => {
    if (growth <= 0) throw refuse("MOD-3001", "Too fast.");
    growth -= 1;
  };

  const sizeOf = (value: unknown) => JSON.stringify(value).length;

  async function handle(method: string, args: unknown): Promise<unknown> {
    const json = JSON.stringify(args ?? null);
    calls.push({ method, args: JSON.parse(json) });
    const code = always.get(method);
    if (code) throw refuse(code, "Failed.");
    const hit = failures.find((f) => f.method === method && f.left > 0);
    if (hit) {
      hit.left -= 1;
      if (hit.left === 0) throw refuse(hit.code, "Failed.");
    }
    if (json.length > ARGS_BYTES) throw refuse("MOD-3002", "Too large.");
    const a = isRecord(args) ? args : {};
    const only = (...names: string[]) => {
      if (!isRecord(args) || Object.keys(args).some((k) => !names.includes(k))) {
        throw refuse("MOD-1003", "Bad arguments.");
      }
    };
    switch (method) {
      case "documents.list": {
        only("fields", "after", "limit");
        const fields = a.fields;
        if (!Array.isArray(fields) || fields.some((f) => !LIST_FIELDS.includes(String(f)))) {
          throw refuse("MOD-1003", "Bad fields.");
        }
        const after = typeof a.after === "number" ? a.after : 0;
        const limit = typeof a.limit === "number" ? a.limit : LIMIT;
        if (limit < 1 || limit > LIMIT) throw refuse("MOD-1003", "Bad limit.");
        const all = Array.from(docs.values())
          .filter((d) => d.id > after)
          .sort((x, y) => x.id - y.id);
        const take = Math.min(limit, pageSize);
        const page = all.slice(0, take);
        const dto = (d: MapDoc) => {
          const full: Record<string, unknown> = {
            id: d.id,
            title: d.title,
            homeProject: d.project === "" ? null : { id: null, name: d.project },
            importance: d.importance,
            chunkCount: d.chunkCount,
            reviewed: d.reviewed,
            sourceType: d.sourceType,
            tags: d.tags,
            ingestedAt: d.ingestedAt,
          };
          return Object.fromEntries(fields.map((f) => [String(f), full[String(f)]]));
        };
        return {
          documents: page.map(dto),
          next: all.length > take ? page[page.length - 1].id : null,
        };
      }
      case "theme.get": {
        only();
        return JSON.parse(JSON.stringify(theme));
      }
      case "ui.openDocument": {
        only("id");
        // Archived and missing documents are the same answer, as in documents.rs.
        if (typeof a.id !== "number" || !docs.has(a.id)) throw refuse("MOD-4002", "Not found.");
        opened.push(a.id);
        return null;
      }
      case "embeddings.signature": {
        only();
        const ids = embedded();
        return { count: ids.length, dimension, hash: fakeHash(`g${generation}:${ids.join(",")}`) };
      }
      case "storage.get": {
        only("key");
        const row = kv.get(String(a.key));
        return { value: row?.value ?? null, revision: row?.revision ?? 0 };
      }
      case "storage.set": {
        only("key", "value", "expectedRevision");
        const key = String(a.key);
        const row = kv.get(key);
        if (a.expectedRevision !== (row?.revision ?? 0)) {
          throw new ModCallError({
            code: "MOD-4001",
            message: "Stale.",
            revision: row?.revision ?? 0,
          });
        }
        if (sizeOf(a.value) > (row ? sizeOf(row.value) : -1)) growing();
        const revision = (row?.revision ?? 0) + 1;
        kv.set(key, { value: JSON.parse(JSON.stringify(a.value ?? null)), revision });
        return { revision };
      }
      case "storage.doc.set": {
        only("key", "entries");
        const key = String(a.key);
        if (contentKeys.has(key)) throw refuse("MOD-1003", "A per-document key is cache.");
        const entries = a.entries;
        if (!Array.isArray(entries) || entries.length === 0 || entries.length > LIMIT) {
          throw refuse("MOD-1003", "Write 1 to 5000 entries at a time.");
        }
        const seen = new Set<number>();
        for (const e of entries) {
          if (!isRecord(e) || typeof e.documentId !== "number") throw refuse("MOD-1003", "Bad.");
          if (seen.has(e.documentId)) throw refuse("MOD-1003", "A document is named twice.");
          seen.add(e.documentId);
          if (!docs.has(e.documentId)) throw refuse("MOD-4002", "Not found.");
        }
        growing();
        const table = rows.get(key) ?? new Map<number, unknown>();
        for (const e of entries as { documentId: number; value: unknown }[]) {
          table.set(e.documentId, JSON.parse(JSON.stringify(e.value ?? null)));
        }
        rows.set(key, table);
        return { written: entries.length };
      }
      case "storage.doc.list": {
        only("key", "after", "limit");
        const table = rows.get(String(a.key)) ?? new Map<number, unknown>();
        const after = typeof a.after === "number" ? a.after : 0;
        const limit = typeof a.limit === "number" ? a.limit : LIMIT;
        if (limit < 1 || limit > LIMIT) throw refuse("MOD-1003", "Bad limit.");
        const all = Array.from(table.entries())
          .filter(([id]) => id > after && docs.has(id))
          .sort((x, y) => x[0] - y[0]);
        const take = Math.min(limit, pageSize);
        const page = all.slice(0, take);
        return {
          entries: page.map(([documentId, value]) => ({ documentId, value })),
          next: all.length > take ? page[page.length - 1][0] : null,
        };
      }
      case "storage.doc.delete": {
        only("key", "documentIds");
        const table = rows.get(String(a.key));
        if (!table) return { deleted: 0 };
        const ids = Array.isArray(a.documentIds) ? (a.documentIds as number[]) : null;
        let deleted = 0;
        for (const id of Array.from(table.keys())) {
          if (ids === null || ids.includes(id)) {
            table.delete(id);
            deleted += 1;
          }
        }
        return { deleted };
      }
      case "embeddings.documentVectors": {
        only("ids", "cap");
        const cap = typeof a.cap === "number" ? a.cap : 1000;
        if (cap < 1 || cap > LIMIT) throw refuse("MOD-1003", "cap is 1 to 5000.");
        const chosen = embedded().slice(0, cap);
        // The wire layout (embeddings.rs): a 16-byte header, the ids as doubles, then the vectors.
        const buffer = new ArrayBuffer(16 + 8 * chosen.length + 4 * chosen.length * dimension);
        const header = new DataView(buffer);
        header.setUint32(0, 1, true);
        header.setUint32(4, chosen.length, true);
        header.setUint32(8, dimension, true);
        const ids = new Float64Array(buffer, 16, chosen.length);
        const flat = new Float32Array(buffer, 16 + 8 * chosen.length, chosen.length * dimension);
        chosen.forEach((id, i) => {
          ids[i] = id;
          flat.set(vectors.get(id) as Float32Array, i * dimension);
        });
        // Read back the way a frame reads the wire: the SDK's own reader over the raw buffer.
        const binary: Binary = { binary: true, vectors: readDocumentVectors(buffer), buffer };
        return binary;
      }
      default:
        throw refuse("MOD-1001", "Unknown method.");
    }
  }

  const host: MapHost = {
    call: (method, args) => handle(method, args),
    async documentVectors(request) {
      const reply = await handle("embeddings.documentVectors", request ?? {});
      return (reply as Binary).vectors;
    },
    onEvent(handler) {
      handlers.add(handler);
      return () => handlers.delete(handler);
    },
  };

  const backend: Handler = async (method, args) => {
    try {
      const reply = await handle(method, args);
      if (typeof reply === "object" && reply !== null && (reply as Binary).binary === true) {
        const { vectors: v, buffer } = reply as Binary;
        return { kind: "binary", vectors: v, buffer } satisfies BackendReply;
      }
      return { kind: "value", value: reply } satisfies BackendReply;
    } catch (error) {
      const code = (error as { code?: string }).code ?? "MOD-9001";
      return { kind: "error", error: { code, message: (error as Error).message } };
    }
  };

  return {
    host,
    backend,
    calls,
    of: (method) => calls.filter((c) => c.method === method),
    addDoc(doc, vector) {
      docs.set(doc.id, doc);
      if (vector) vectors.set(doc.id, vector);
    },
    removeDoc(id) {
      docs.delete(id);
      vectors.delete(id);
      for (const table of rows.values()) table.delete(id);
    },
    reindex() {
      generation += 1;
    },
    refill(tokens) {
      growth += tokens;
    },
    failNext(method, code, nth = 1) {
      failures.push({ method, code, left: nth });
    },
    failAlways(method, code) {
      if (code === null) always.delete(method);
      else always.set(method, code);
    },
    kv,
    rows,
    handlers,
    opened,
    setTheme(next) {
      theme = next;
    },
    emit(event) {
      for (const h of Array.from(handlers)) h(event);
    },
  };
}
