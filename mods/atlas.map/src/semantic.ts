// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The "by meaning" arrangement: where each document sits when nearby means similar. Ported from the
// core Map's backend half (src-tauri/src/layout.rs, which reads the vectors, runs the sidecar's
// reducer and caches the coordinates in `doc_layout`), now done by the mod over the Host API:
//
//   1. `embeddings.signature` (cheap: a count, a width, a hash of which documents and which of their
//      chunks; no vector is read) and the cached fingerprint
//      `{hash, dimension, cap, ALGO_VERSION}` decide whether the coordinates in storage are still right.
//   2. Fresh: read them back (`storage.doc.list`), no vector read, no projection. This is the common
//      open and it is a few calls.
//   3. Stale: `embeddings.documentVectors({cap})` (the node cap keeps the most important and most
//      substantial documents; the rest are parked at their project's centre by the layout), the
//      projection in the Worker (pca.ts), then the coordinates are cached.
//
// THE CACHE IS WRITTEN SO IT IS NEVER HALF TRUE. The fingerprint is the only thing that says the rows
// are good, so it is cleared BEFORE the rows change and written AFTER they are all in: a frame closed
// halfway (a tab switch rebuilds the frame), a refused write, a crash, all leave "no fingerprint", which
// reads as stale, and the next open recomputes and heals. A fingerprint is also not trusted on its own:
// the rows must number what it says and each must be a usable coordinate, or it is stale. Writing the
// cache is best-effort, as it was in the core (`persist`): a failure to save costs the next open a
// recompute, never the map the person is looking at.
//
// The fingerprint uses the signature read BEFORE the vectors. A document that lands between the two
// calls makes the stored hash older than the vectors, so the next open sees a mismatch and recomputes:
// the error can only ever be toward doing the work again.

import {
  clearDocRows,
  CODE,
  codeOf,
  HostAnswerError,
  isCount,
  listDocRows,
  MAX_ARGS_BYTES,
  MAX_DOCS_PER_CALL,
  readKey,
  readSignature,
  setDocRows,
  writeKey,
  type DocRow,
  type MapHost,
  type Signature,
} from "./hostApi";
import type { Compute } from "./compute";
import type { SemanticCoord } from "./types";

/** Bump to make every stored coordinate stale after a change to the projection (pca.ts) or to how the
 *  documents are chosen, so a remembered layout can never be replayed under different rules. */
export const ALGO_VERSION = 1;

/** The storage keys the layout uses. All cache class (manifest.json declares them so: a cache is left
 *  out of backups and dropped freely); a test holds the two together. */
export const KEY_SEMANTIC_META = "semantic-meta";
export const KEY_SEMANTIC_COORDS = "semantic-coords";

/** What the cache says about the coordinates beside it. */
export interface SemanticMeta {
  v: 1;
  hash: string;
  dimension: number;
  cap: number;
  algo: number;
  /** How many coordinate rows were written under it. */
  nodes: number;
}

/** Where the engine is, for the "computing" strip. */
export type SemanticPhase = "checking" | "reading" | "reducing" | "saving";

export interface SemanticResult {
  coords: SemanticCoord[];
  /** "cache": read back from storage; "computed": projected just now. */
  source: "cache" | "computed";
  /** Documents that have vectors, and how many of them are placed (the rest are parked by the layout:
   *  "top N of M"). */
  total: number;
  placed: number;
  /** Computed, but the coordinates could not be remembered (the next open will compute again). */
  saved: boolean;
}

export interface SemanticOptions {
  /** The node cap, 200..5000 (settings.ts keeps it there). */
  nodeCap: number;
  onPhase?: (phase: SemanticPhase) => void;
  /** Wait (a rate limit's refill); injectable so a test does not wait in real time. */
  sleep?: (ms: number) => Promise<void>;
}

/** Entries per `storage.doc.set` and the most bytes of JSON in one: a batch under both stays well
 *  inside PM's caps (5,000 entries, 1 MiB of arguments) and writes a 5,000-document layout in a few
 *  growing writes (the storage growth bucket allows 40 at once, then 4 a second). */
export const BATCH_ENTRIES = 1000;
export const BATCH_BYTES = MAX_ARGS_BYTES / 4;
/** A rate-limited write waits one growth token (250 ms) and tries again, up to this many times. */
const RATE_RETRIES = 8;
const RATE_WAIT_MS = 300;

const defaultSleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

const isUnit = (v: unknown): v is number => typeof v === "number" && v >= 0 && v <= 1;

/** The meta in storage, if it is one. */
export function readMeta(value: unknown): SemanticMeta | null {
  if (typeof value !== "object" || value === null) return null;
  const m = value as Record<string, unknown>;
  if (
    m.v !== 1 ||
    typeof m.hash !== "string" ||
    !isCount(m.dimension) ||
    !isCount(m.cap) ||
    !isCount(m.algo) ||
    !isCount(m.nodes)
  ) {
    return null;
  }
  return { v: 1, hash: m.hash, dimension: m.dimension, cap: m.cap, algo: m.algo, nodes: m.nodes };
}

/** Whether a stored fingerprint is the one these inputs would write. */
export function isFresh(meta: SemanticMeta | null, sig: Signature, cap: number): boolean {
  return (
    meta !== null &&
    meta.hash === sig.hash &&
    meta.dimension === sig.dimension &&
    meta.cap === cap &&
    meta.algo === ALGO_VERSION
  );
}

/** One coordinate row, if it is one: `{x, y}` both in [0,1]. */
function readRow(row: DocRow): SemanticCoord | null {
  const v = row.value;
  if (typeof v !== "object" || v === null) return null;
  const { x, y } = v as Record<string, unknown>;
  return isUnit(x) && isUnit(y) ? { id: row.documentId, x, y } : null;
}

/** The cached coordinates, or null unless every row is usable and they number what the meta says. */
async function readCachedCoords(
  host: MapHost,
  meta: SemanticMeta,
): Promise<SemanticCoord[] | null> {
  const rows = await listDocRows(host, KEY_SEMANTIC_COORDS);
  if (rows.length !== meta.nodes) return null;
  const coords: SemanticCoord[] = [];
  for (const row of rows) {
    const c = readRow(row);
    if (!c) return null;
    coords.push(c);
  }
  return coords;
}

/** Entries in groups of at most `BATCH_ENTRIES` and `BATCH_BYTES` of JSON each. */
export function batches(rows: DocRow[]): DocRow[][] {
  const out: DocRow[][] = [];
  let current: DocRow[] = [];
  let bytes = 0;
  for (const row of rows) {
    // The entry as it will travel, plus the comma; sized exactly so a row never tips a batch over.
    const size = JSON.stringify({ documentId: row.documentId, value: row.value }).length + 1;
    if (current.length > 0 && (current.length >= BATCH_ENTRIES || bytes + size > BATCH_BYTES)) {
      out.push(current);
      current = [];
      bytes = 0;
    }
    current.push(row);
    bytes += size;
  }
  if (current.length > 0) out.push(current);
  return out;
}

/** Run a storage write, waiting out the growth limit (PM's MOD-3001) a few times. */
async function paced<T>(sleep: (ms: number) => Promise<void>, run: () => Promise<T>): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    try {
      return await run();
    } catch (error) {
      if (codeOf(error) !== CODE.rateLimited || attempt >= RATE_RETRIES) throw error;
      await sleep(RATE_WAIT_MS);
    }
  }
}

/** Remember `coords` under `fingerprint`, clearing the fingerprint first and writing it last. */
async function persist(
  host: MapHost,
  coords: SemanticCoord[],
  fingerprint: Omit<SemanticMeta, "nodes">,
  revision: number,
  sleep: (ms: number) => Promise<void>,
): Promise<boolean> {
  try {
    // 1. No fingerprint: from here until step 4 the rows are not a layout.
    let rev = await paced(sleep, () => writeKey(host, KEY_SEMANTIC_META, null, revision));
    // 2. The old rows go (a smaller or different set must not leave strays behind).
    await clearDocRows(host, KEY_SEMANTIC_COORDS);
    // 3. The new rows, in batches.
    const rows: DocRow[] = coords.map((c) => ({ documentId: c.id, value: { x: c.x, y: c.y } }));
    for (const batch of batches(rows)) {
      await paced(sleep, () => setDocRows(host, KEY_SEMANTIC_COORDS, batch));
    }
    // 4. The fingerprint, last.
    const meta: SemanticMeta = { ...fingerprint, nodes: rows.length };
    rev = await paced(sleep, () => writeKey(host, KEY_SEMANTIC_META, meta, rev));
    return true;
  } catch {
    // Quota, a rate limit that outlasted the waits, another frame writing, a closed session: all the
    // same to the person, and the next open recomputes.
    return false;
  }
}

/**
 * The coordinates of every placed document: from the cache when it is still right, else projected now
 * and remembered. Rejects when the vectors or the projection cannot be had (PM refused, the worker
 * failed): the caller says so and offers to try again.
 */
export async function loadSemanticCoords(
  host: MapHost,
  compute: Compute,
  options: SemanticOptions,
): Promise<SemanticResult> {
  const { nodeCap, onPhase } = options;
  const sleep = options.sleep ?? defaultSleep;
  onPhase?.("checking");
  const sig = await readSignature(host);
  // Nothing has a vector yet: nothing to place, and nothing worth caching (a later embedding makes the
  // signature change anyway).
  if (sig.count === 0) return { coords: [], source: "computed", total: 0, placed: 0, saved: true };

  const stored = await readKey(host, KEY_SEMANTIC_META);
  const meta = readMeta(stored.value);
  if (isFresh(meta, sig, nodeCap)) {
    const cached = await readCachedCoords(host, meta as SemanticMeta);
    if (cached) {
      return {
        coords: cached,
        source: "cache",
        total: sig.count,
        placed: cached.length,
        saved: true,
      };
    }
  }

  onPhase?.("reading");
  const vectors = await host.documentVectors({ cap: nodeCap });
  // PM sizes its answer by the cap; one with more than asked for, or ids that repeat, is not an
  // answer the layout may be built from.
  if (vectors.count > Math.min(nodeCap, MAX_DOCS_PER_CALL)) {
    throw new HostAnswerError("embeddings.documentVectors");
  }
  const ids = Array.from(vectors.ids);
  if (!ids.every(isCount) || new Set(ids).size !== ids.length) {
    throw new HostAnswerError("embeddings.documentVectors");
  }

  onPhase?.("reducing");
  const reduction = await compute.reduce(vectors.vectors, vectors.count, vectors.dimension);
  const coords: SemanticCoord[] = ids.map((id, i) => ({
    id,
    x: reduction.coords[2 * i],
    y: reduction.coords[2 * i + 1],
  }));

  onPhase?.("saving");
  const saved = await persist(
    host,
    coords,
    { v: 1, hash: sig.hash, dimension: sig.dimension, cap: nodeCap, algo: ALGO_VERSION },
    stored.revision,
    sleep,
  );
  return { coords, source: "computed", total: sig.count, placed: coords.length, saved };
}
