// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Host API calls the Map makes, as typed functions that CHECK what comes back. The SDK hands a mod
// whatever PM answered as `unknown` (each mod validates the shapes it reads, src/kit/host.ts); this is
// where the Map does it, once, so the engine above works with values it can trust to have the shape it
// expects, and a PM that answers something else (a newer PM, a bug) is an error here and not a NaN in a
// layout. Only the calls the Map's manifest asks for are here: documents.list, embeddings.signature,
// embeddings.documentVectors (through the SDK's checked reader), storage.get/set, storage.doc.*.
//
// The error codes the engine acts on are PM's stable ones (ERROR_CODES.md); hostApi.test.ts pins each
// to error.rs so a renumbering fails a test instead of quietly turning a retry into a crash.

import type { ModHost } from "../../../src/kit/host";
import type { MapDoc } from "./types";

/** The part of the SDK's host the engine uses (a test's stand-in has just these). */
export type MapHost = Pick<ModHost, "call" | "documentVectors" | "onEvent">;

/** PM's stable codes the engine reads (ERROR_CODES.md). */
export const CODE = {
  rateLimited: "MOD-3001",
  quotaExceeded: "MOD-3003",
  staleRevision: "MOD-4001",
} as const;

/** The most documents one page or one batch may hold (limits.rs MAX_DOCS_PER_CALL). */
export const MAX_DOCS_PER_CALL = 5000;
/** A call's arguments, in bytes (limits.rs MAX_ARGS_BYTES); the engine keeps its batches well under it. */
export const MAX_ARGS_BYTES = 1 << 20;
/** The most pages `listDocuments` will read: a cursor that never ends is a PM bug, not an endless loop.
 *  At 5,000 per page it is five million documents. */
const MAX_PAGES = 1000;

/** What a bad answer says. Our words only: PM's reply is never repeated into the UI. */
export class HostAnswerError extends Error {
  constructor(what: string) {
    super(`PM's answer to ${what} was not what the Map expects.`);
    this.name = "HostAnswerError";
  }
}

/** The `code` of a PM refusal, or null for any other error. */
export function codeOf(error: unknown): string | null {
  if (typeof error !== "object" || error === null) return null;
  const code = (error as { code?: unknown }).code;
  return typeof code === "string" ? code : null;
}

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

/** A whole number from 0 up to what a JavaScript number holds exactly. */
export const isCount = (v: unknown): v is number =>
  typeof v === "number" && Number.isSafeInteger(v) && v >= 0;

// ---- documents --------------------------------------------------------------------------------------

/** The fields the Map reads (the list DTO's whole vocabulary; there is no path in it). */
export const LIST_FIELDS = [
  "id",
  "title",
  "homeProject",
  "importance",
  "chunkCount",
  "reviewed",
  "sourceType",
  "tags",
  "ingestedAt",
] as const;

/** One listed document, read defensively; null when it cannot be a node at all (no usable id). */
export function readDoc(raw: unknown): MapDoc | null {
  if (!isRecord(raw) || !isCount(raw.id)) return null;
  const home = isRecord(raw.homeProject) ? raw.homeProject : null;
  const name = home && typeof home.name === "string" ? home.name.trim() : "";
  return {
    id: raw.id,
    title: typeof raw.title === "string" ? raw.title : "",
    project: name,
    importance: typeof raw.importance === "string" ? raw.importance : null,
    chunkCount: isCount(raw.chunkCount) ? raw.chunkCount : 0,
    reviewed: raw.reviewed === true,
    sourceType: typeof raw.sourceType === "string" ? raw.sourceType : "vault",
    tags: Array.isArray(raw.tags) ? raw.tags.filter((t): t is string => typeof t === "string") : [],
    ingestedAt: typeof raw.ingestedAt === "string" ? raw.ingestedAt : "",
  };
}

/** The core Map's order: newest ingested first, ties by id descending (ingest.rs list_documents,
 *  `ORDER BY d.ingested_at DESC, d.id DESC`). Colours and the legend are handed out by a project's first
 *  appearance in this list, and the by-project simulation starts from it, so the same vault must read
 *  in the same order or switching from the core Map recolours every project. The timestamps are
 *  compared as plain text, as SQLite does (they are ISO 8601, which sorts as time). */
export function byNewest(a: MapDoc, b: MapDoc): number {
  if (a.ingestedAt !== b.ingestedAt) return a.ingestedAt < b.ingestedAt ? 1 : -1;
  return b.id - a.id;
}

/** Every document the Map may draw (PM lists all but the archived): read page by page (PM pages by
 *  id), then put in the core Map's order. */
export async function listDocuments(host: MapHost): Promise<MapDoc[]> {
  const out: MapDoc[] = [];
  let after = 0;
  for (let page = 0; page < MAX_PAGES; page++) {
    const reply = await host.call("documents.list", {
      fields: LIST_FIELDS,
      after,
      limit: MAX_DOCS_PER_CALL,
    });
    if (!isRecord(reply) || !Array.isArray(reply.documents)) {
      throw new HostAnswerError("documents.list");
    }
    for (const raw of reply.documents) {
      const doc = readDoc(raw);
      if (doc) out.push(doc);
    }
    const next = reply.next;
    if (next === null || next === undefined) return out.sort(byNewest);
    // The cursor must move forward, or the loop never ends: PM keys pages by id.
    if (!isCount(next) || next <= after) throw new HostAnswerError("documents.list");
    after = next;
  }
  throw new HostAnswerError("documents.list");
}

// ---- embeddings -------------------------------------------------------------------------------------

/** `embeddings.signature`: how many documents have vectors, how wide they are, and a hash of which. */
export interface Signature {
  count: number;
  dimension: number;
  hash: string;
}

export async function readSignature(host: MapHost): Promise<Signature> {
  const reply = await host.call("embeddings.signature", {});
  if (
    !isRecord(reply) ||
    !isCount(reply.count) ||
    !isCount(reply.dimension) ||
    typeof reply.hash !== "string" ||
    reply.hash.length === 0 ||
    reply.hash.length > 128
  ) {
    throw new HostAnswerError("embeddings.signature");
  }
  return { count: reply.count, dimension: reply.dimension, hash: reply.hash };
}

// ---- storage ----------------------------------------------------------------------------------------

/** A stored value and the revision it was read at (0 and null for a key with nothing stored). */
export interface Stored {
  value: unknown;
  revision: number;
}

export async function readKey(host: MapHost, key: string): Promise<Stored> {
  const reply = await host.call("storage.get", { key });
  if (!isRecord(reply) || !isCount(reply.revision)) throw new HostAnswerError("storage.get");
  return { value: reply.value ?? null, revision: reply.revision };
}

/** Write `value` at `expectedRevision`; the new revision. A stale one throws PM's MOD-4001. */
export async function writeKey(
  host: MapHost,
  key: string,
  value: unknown,
  expectedRevision: number,
): Promise<number> {
  const reply = await host.call("storage.set", { key, value, expectedRevision });
  if (!isRecord(reply) || !isCount(reply.revision)) throw new HostAnswerError("storage.set");
  return reply.revision;
}

/** One per-document row. */
export interface DocRow {
  documentId: number;
  value: unknown;
}

/** Every row of `key` (the Map's cache of per-document coordinates), paged by document id. */
export async function listDocRows(host: MapHost, key: string): Promise<DocRow[]> {
  const out: DocRow[] = [];
  let after = 0;
  for (let page = 0; page < MAX_PAGES; page++) {
    const reply = await host.call("storage.doc.list", { key, after, limit: MAX_DOCS_PER_CALL });
    if (!isRecord(reply) || !Array.isArray(reply.entries)) {
      throw new HostAnswerError("storage.doc.list");
    }
    for (const e of reply.entries) {
      if (!isRecord(e) || !isCount(e.documentId)) throw new HostAnswerError("storage.doc.list");
      out.push({ documentId: e.documentId, value: e.value });
    }
    const next = reply.next;
    if (next === null || next === undefined) return out;
    if (!isCount(next) || next <= after) throw new HostAnswerError("storage.doc.list");
    after = next;
  }
  throw new HostAnswerError("storage.doc.list");
}

/** Remove every row of `key`. */
export async function clearDocRows(host: MapHost, key: string): Promise<void> {
  const reply = await host.call("storage.doc.delete", { key });
  if (!isRecord(reply) || !isCount(reply.deleted)) throw new HostAnswerError("storage.doc.delete");
}

/** Write rows of `key`; PM writes a batch all or nothing. */
export async function setDocRows(host: MapHost, key: string, rows: DocRow[]): Promise<void> {
  const reply = await host.call("storage.doc.set", {
    key,
    entries: rows.map((r) => ({ documentId: r.documentId, value: r.value })),
  });
  if (!isRecord(reply) || reply.written !== rows.length)
    throw new HostAnswerError("storage.doc.set");
}
