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
// TWO LAYOUTS, TWO CACHES. The quick layout (PCA) and the enhanced one (t-SNE, tsne.ts) each have their
// own key pair and their own algorithm version, so switching the Enhanced setting uses the other pair
// with no recompute, and the PCA pair is the same bytes as before the enhanced layout existed. Which
// rows a run reads, which job it posts and what it writes is the table in `loadSemanticCoords`.
//
// THE COORDINATES ARE A PER-VAULT CACHE (cache class, left out of backups). Mod storage lives in the
// vault's own store, not on the device, so every device that opens a vault draws the picture whichever
// device computed it, until the documents change. A recompute on another engine can differ in the last
// bit of `Math.exp`, so the arrangement can change after a recompute, with equal quality. Accepted.
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
import { SemanticStopped, type Compute, type SemanticJob } from "./compute";
import type { SemanticCoord } from "./types";

/** Bump to make every stored coordinate stale after a change to the projection (pca.ts) or to how the
 *  documents are chosen, so a remembered layout can never be replayed under different rules. */
export const ALGO_VERSION = 1;

/** The storage keys the layout uses. All cache class (manifest.json declares them so: a cache is left
 *  out of backups and dropped freely); a test holds the two together. */
export const KEY_SEMANTIC_META = "semantic-meta";
export const KEY_SEMANTIC_COORDS = "semantic-coords";
/** The enhanced (t-SNE) layout's own pair, so the two layouts never overwrite each other. */
export const KEY_SEMANTIC_META_TSNE = "semantic-meta-tsne";
export const KEY_SEMANTIC_COORDS_TSNE = "semantic-coords-tsne";

/** Bump to make every stored t-SNE coordinate stale after a change to tsne.ts or to how it is fed. */
export const TSNE_ALGO_VERSION = 1;

/** Which of the two layouts. */
export type LayoutMethod = "pca" | "tsne";

interface Store {
  metaKey: string;
  coordsKey: string;
  algo: number;
}
const STORES: Record<LayoutMethod, Store> = {
  pca: { metaKey: KEY_SEMANTIC_META, coordsKey: KEY_SEMANTIC_COORDS, algo: ALGO_VERSION },
  tsne: {
    metaKey: KEY_SEMANTIC_META_TSNE,
    coordsKey: KEY_SEMANTIC_COORDS_TSNE,
    algo: TSNE_ALGO_VERSION,
  },
};

/** Three documents or fewer have no t-SNE (tsne.ts spreads them): the run is the quick layout. */
const MIN_TSNE_DOCUMENTS = 4;

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
export type SemanticPhase = "checking" | "reading" | "reducing" | "sharpening" | "saving";

/** Why a run that was asked for the enhanced layout ends without it: the person pressed Cancel, the
 *  deadline passed, the quick layout could not be saved (so the slower one is not started), no Worker
 *  could be made (the enhanced layout is never run on the page's own thread), or the Worker failed
 *  while the quick layout was in hand. */
export type StoppedReason = "cancelled" | "deadline" | "unsaved" | "no-worker" | "failed";

export interface SemanticResult {
  /** Which layout these coordinates are. The subtitle says "enhanced" from this, never from a setting. */
  method: LayoutMethod;
  coords: SemanticCoord[];
  /** "cache": read back from storage; "computed": projected just now. */
  source: "cache" | "computed";
  /** Documents that have vectors, and how many of them are placed (the rest are parked by the layout:
   *  "top N of M"). */
  total: number;
  placed: number;
  /** Computed, but the coordinates could not be remembered (the next open will compute again). */
  saved: boolean;
  /** Set when the enhanced layout was asked for and these are the quick layout's coordinates, and why. */
  stopped?: StoppedReason;
}

/**
 * The page's hand on a running `loadSemanticCoords`. `cancel` is the person's Cancel: the run ends with
 * the quick layout (waiting for it if it is not made yet, since the vectors die with the worker).
 * `abandon` is a superseded run: the worker ends at once and nothing more is written.
 */
export class SemanticControl {
  cancelRequested = false;
  abandoned = false;
  private job: SemanticJob | null = null;
  private armed = false;

  cancel(): void {
    this.cancelRequested = true;
    if (this.armed) this.job?.cancel();
  }

  abandon(): void {
    this.abandoned = true;
    this.job?.cancel();
  }

  /** Internal: the job a run has posted. `armed` once the quick layout exists, so a cancel is safe. */
  attach(job: SemanticJob): void {
    this.job = job;
  }

  /** Internal: the quick layout exists; a cancel asked for earlier takes effect now. */
  arm(): void {
    this.armed = true;
    if (this.cancelRequested) this.job?.cancel();
  }
}

export interface SemanticOptions {
  /** The node cap, 200..5000 (settings.ts keeps it there). */
  nodeCap: number;
  /** Work out the enhanced layout too (the setting; a run held back passes false). */
  enhanced?: boolean;
  /** The quick layout, ready while the enhanced one is still being worked out. Not called when an older
   *  enhanced picture was already handed to `onStale` (the picture would jump twice). */
  onProvisional?: (result: SemanticResult) => void;
  /** The share of the enhanced layout done, 0 to 1. */
  onProgress?: (fraction: number) => void;
  /** The page's hand on this run (Cancel, and ending a superseded run). */
  control?: SemanticControl;
  onPhase?: (phase: SemanticPhase) => void;
  /** Called once, before the slow part, with the PREVIOUS layout when the saved one no longer fits (a
   *  document was added, the cap changed) but whole rows of it are still there. It is a placeholder to
   *  draw under the computing strip, as the core Map drew its cached coordinates while it recomputed;
   *  the result replaces it. Never called for a fresh cache, for an algorithm the code no longer
   *  speaks (those coordinates were made under other rules), or when the rows are not whole. With the
   *  enhanced layout on, the old enhanced rows come first, else the old quick ones. */
  onStale?: (coords: SemanticCoord[]) => void;
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
export function isFresh(
  meta: SemanticMeta | null,
  sig: Signature,
  cap: number,
  algo: number = ALGO_VERSION,
): boolean {
  return (
    meta !== null &&
    meta.hash === sig.hash &&
    meta.dimension === sig.dimension &&
    meta.cap === cap &&
    meta.algo === algo
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
  coordsKey: string,
): Promise<SemanticCoord[] | null> {
  const rows = await listDocRows(host, coordsKey);
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
  store: Store,
  coords: SemanticCoord[],
  fingerprint: Omit<SemanticMeta, "nodes">,
  revision: number,
  sleep: (ms: number) => Promise<void>,
): Promise<boolean> {
  try {
    // 1. No fingerprint: from here until step 4 the rows are not a layout.
    let rev = await paced(sleep, () => writeKey(host, store.metaKey, null, revision));
    // 2. The old rows go (a smaller or different set must not leave strays behind).
    await clearDocRows(host, store.coordsKey);
    // 3. The new rows, in batches.
    const rows: DocRow[] = coords.map((c) => ({ documentId: c.id, value: { x: c.x, y: c.y } }));
    for (const batch of batches(rows)) {
      await paced(sleep, () => setDocRows(host, store.coordsKey, batch));
    }
    // 4. The fingerprint, last.
    const meta: SemanticMeta = { ...fingerprint, nodes: rows.length };
    rev = await paced(sleep, () => writeKey(host, store.metaKey, meta, rev));
    return true;
  } catch {
    // Quota, a rate limit that outlasted the waits, another frame writing, a closed session: all the
    // same to the person, and the next open recomputes.
    return false;
  }
}

/** The cached coordinates of one layout when its fingerprint is `meta` and every row is whole. */
async function wholeRows(
  host: MapHost,
  store: Store,
  meta: SemanticMeta | null,
): Promise<SemanticCoord[] | null> {
  if (meta === null) return null;
  return readCachedCoords(host, meta, store.coordsKey);
}

/** The coordinates of an OLD layout, as a placeholder: only whole rows made by the algorithm the code
 *  still speaks. A look at the old picture is a courtesy, so a failure reading it is no placeholder. */
async function oldRows(
  host: MapHost,
  store: Store,
  meta: SemanticMeta | null,
): Promise<SemanticCoord[] | null> {
  if (meta === null || meta.algo !== store.algo) return null;
  try {
    const rows = await readCachedCoords(host, meta, store.coordsKey);
    return rows && rows.length > 0 ? rows : null;
  } catch {
    return null;
  }
}

/**
 * The coordinates of every placed document: from the cache when it is still right, else worked out now
 * and remembered. Rejects when the vectors or the layout cannot be had (PM refused, the worker failed
 * with no quick layout in hand): the caller says so and offers to try again.
 *
 * WHAT A RUN DOES, by the Enhanced setting and which cache is fresh ("fresh" = the fingerprint fits AND
 * every row is whole). The signature is read first in every row; the vectors are read from PM at most
 * once, and only where a job is posted.
 *
 *  row  enhanced  PCA cache  t-SNE cache  job posted                draws                     writes
 *   1     on       fresh      fresh        none                      t-SNE at once             nothing
 *   2     on       stale      fresh        none                      t-SNE at once             nothing
 *   3     on       fresh      stale        {pca: false, tsne: true}  old t-SNE or PCA, t-SNE   t-SNE pair
 *   4     on       stale      stale        {pca: true, tsne: true}   old t-SNE, else old PCA,  PCA pair (while t-SNE
 *                                                                    then PCA, then t-SNE      runs), then t-SNE pair
 *   5     off      fresh      (not read)   none                      PCA at once               nothing
 *   6     off      stale      (not read)   {pca: true, tsne: false}  old PCA, then PCA         PCA pair
 *
 * Fewer than four documents never run t-SNE (there is nothing to sharpen): such a run is row 5 or 6.
 * The quick layout is the fallback of every enhanced run: Cancel, the deadline, a failing worker and a
 * refused save of the quick layout (a t-SNE that cannot be saved would cost seconds on every open)
 * all end with it and a `stopped` reason.
 */
export async function loadSemanticCoords(
  host: MapHost,
  compute: Compute,
  options: SemanticOptions,
): Promise<SemanticResult> {
  const { nodeCap, onPhase, control } = options;
  const sleep = options.sleep ?? defaultSleep;
  const gone = () => {
    if (control?.abandoned) throw new SemanticStopped("cancelled");
  };
  onPhase?.("checking");
  const sig = await readSignature(host);
  // Nothing has a vector yet: nothing to place, and nothing worth caching (a later embedding makes the
  // signature change anyway).
  if (sig.count === 0) {
    return { method: "pca", coords: [], source: "computed", total: 0, placed: 0, saved: true };
  }
  const wantTsne = options.enhanced === true && Math.min(sig.count, nodeCap) >= MIN_TSNE_DOCUMENTS;
  const cached = (method: LayoutMethod, coords: SemanticCoord[]): SemanticResult => ({
    method,
    coords,
    source: "cache",
    total: sig.count,
    placed: coords.length,
    saved: true,
  });

  // Rows 1 and 2: the enhanced layout is saved and still right. The quick layout's cache is not read.
  let tsneStored: { value: unknown; revision: number } | null = null;
  let tsneMeta: SemanticMeta | null = null;
  if (wantTsne) {
    tsneStored = await readKey(host, STORES.tsne.metaKey);
    tsneMeta = readMeta(tsneStored.value);
    if (isFresh(tsneMeta, sig, nodeCap, TSNE_ALGO_VERSION)) {
      const rows = await wholeRows(host, STORES.tsne, tsneMeta);
      if (rows) return cached("tsne", rows);
    }
  }

  // Rows 3 to 6: the quick layout's cache.
  const pcaStored = await readKey(host, STORES.pca.metaKey);
  const pcaMeta = readMeta(pcaStored.value);
  const pcaFresh = isFresh(pcaMeta, sig, nodeCap, ALGO_VERSION);
  const pcaRows = pcaFresh ? await wholeRows(host, STORES.pca, pcaMeta) : null;
  if (!wantTsne && pcaRows) return cached("pca", pcaRows);

  // The placeholder under the strip: the old enhanced picture if there is one (so it does not jump
  // twice), else the old quick one (in row 3 that is the fresh quick layout itself).
  let showedOldTsne = false;
  if (options.onStale) {
    let placeholder: SemanticCoord[] | null = null;
    if (wantTsne) {
      placeholder = await oldRows(host, STORES.tsne, tsneMeta);
      showedOldTsne = placeholder !== null;
    }
    if (!placeholder) {
      placeholder = pcaRows ?? (await oldRows(host, STORES.pca, pcaMeta));
    }
    if (placeholder) options.onStale(placeholder);
  }

  const rowThree = wantTsne && pcaRows !== null;
  const fallbackStop = (): StoppedReason | undefined =>
    compute.mode === "inline" ? "no-worker" : undefined;
  gone();
  // Cancel pressed before there is anything to read: row 3 already has its quick layout in hand.
  if (rowThree && control?.cancelRequested) {
    return { ...cached("pca", pcaRows as SemanticCoord[]), stopped: "cancelled" };
  }

  onPhase?.("reading");
  const vectors = await host.documentVectors({ cap: nodeCap });
  gone();
  // PM sizes its answer by the cap; one with more than asked for, or ids that repeat, is not an
  // answer the layout may be built from.
  if (vectors.count > Math.min(nodeCap, MAX_DOCS_PER_CALL)) {
    throw new HostAnswerError("embeddings.documentVectors");
  }
  const ids = Array.from(vectors.ids);
  if (!ids.every(isCount) || new Set(ids).size !== ids.length) {
    throw new HostAnswerError("embeddings.documentVectors");
  }
  const place = (coords: Float64Array): SemanticCoord[] =>
    ids.map((id, i) => ({ id, x: coords[2 * i], y: coords[2 * i + 1] }));
  const computed = (method: LayoutMethod, coords: SemanticCoord[], saved: boolean) => ({
    method,
    coords,
    source: "computed" as const,
    total: sig.count,
    placed: coords.length,
    saved,
  });
  const fingerprint = (algo: number): Omit<SemanticMeta, "nodes"> => ({
    v: 1,
    hash: sig.hash,
    dimension: sig.dimension,
    cap: nodeCap,
    algo,
  });
  const saveQuick = (coords: SemanticCoord[]) =>
    control?.abandoned
      ? Promise.resolve(false)
      : persist(host, STORES.pca, coords, fingerprint(ALGO_VERSION), pcaStored.revision, sleep);

  // A Cancel pressed while the vectors were being read: only the quick layout is worked out (row 3
  // already has it, so there is no job at all).
  const sharpen = wantTsne && !control?.cancelRequested;
  if (rowThree && !sharpen) {
    return { ...cached("pca", pcaRows as SemanticCoord[]), stopped: "cancelled" };
  }
  const heldBack: StoppedReason | undefined = wantTsne && !sharpen ? "cancelled" : undefined;

  onPhase?.(rowThree ? "sharpening" : "reducing");
  let provisional: SemanticResult | null = null;
  let quickSave: Promise<boolean> | null = null;
  let unsaved = false;
  const job: SemanticJob = compute.semantic(vectors.vectors, vectors.count, vectors.dimension, {
    pca: !rowThree,
    tsne: sharpen,
    onProvisional: ({ coords }) => {
      if (provisional || control?.abandoned) return;
      provisional = computed("pca", place(coords), true);
      // The quick layout is kept at once, while t-SNE runs. If it cannot be kept, t-SNE is not carried
      // on with: a layout that cannot be saved would be worked out again on every open.
      quickSave = saveQuick(provisional.coords);
      void quickSave.then((ok) => {
        if (!ok && !control?.abandoned) {
          unsaved = true;
          job.cancel();
        }
      });
      if (!showedOldTsne) options.onProvisional?.(provisional);
      onPhase?.("sharpening");
      control?.arm();
    },
    onProgress: options.onProgress,
  });
  control?.attach(job);
  // Row 3 has its quick layout already, so a Cancel is safe from the start.
  if (rowThree) control?.arm();

  let reduction;
  try {
    reduction = await job.result;
  } catch (error) {
    gone();
    const stoppedHere = error instanceof SemanticStopped;
    // A real failure with no quick layout to fall back on is the caller's to report.
    const fallback = rowThree ? cached("pca", pcaRows as SemanticCoord[]) : provisional;
    if (!fallback || (!stoppedHere && !sharpen)) throw error;
    const saved = quickSave ? await quickSave : fallback.saved;
    const reason: StoppedReason = unsaved
      ? "unsaved"
      : stoppedHere
        ? (error as SemanticStopped).reason
        : "failed";
    return { ...fallback, saved, stopped: reason };
  }
  gone();

  if (reduction.method === "tsne") {
    const quickOk = quickSave ? await quickSave : true;
    onPhase?.("saving");
    const coords = place(reduction.coords);
    // A quick layout that could not be kept means this one would not be either.
    const saved =
      quickOk && !control?.abandoned
        ? await persist(
            host,
            STORES.tsne,
            coords,
            fingerprint(TSNE_ALGO_VERSION),
            (tsneStored as { revision: number }).revision,
            sleep,
          )
        : false;
    return computed("tsne", coords, saved);
  }

  // The quick layout is all there is: PCA was asked for alone, there was no Worker to run t-SNE in, or
  // there was nothing to sharpen.
  if (rowThree) return { ...cached("pca", pcaRows as SemanticCoord[]), stopped: fallbackStop() };
  const coords = place(reduction.coords);
  onPhase?.("saving");
  const saved = quickSave ? await quickSave : await saveQuick(coords);
  const result = computed("pca", coords, saved);
  const stopped = heldBack ?? (sharpen ? fallbackStop() : undefined);
  return stopped ? { ...result, stopped } : result;
}
