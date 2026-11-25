// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Pinboard's state + persistence. The board is user content (notes, task lists, timelines), so
// it lives in the mod's own storage (`storage.get`/`storage.set`, through lib/pm): PM keeps it with
// the vault, so it travels with a backup or a transfer and is encrypted at rest, rather than sitting in
// a webview's localStorage (a frame has none).
//
// How it is kept safe (Mods plan K7, and boardSync.ts for the pure half):
//   - the board STORED is the source and is kept verbatim; what the view draws is a DISPLAY board
//     derived from it (sanitised, reflowed into this frame's width). A save is the stored board with
//     only the person's changes applied, so reflow never rewrites a card's stored position and what
//     this build cannot read (a newer build's cards, fields) is written back as it was;
//   - nothing is written on load: a save needs a change the person made (`unsaved`), and a board that
//     did not change is not sent;
//   - a board of a version this build cannot read is never written: it is shown as unreadable;
//   - a board over PM's one-call cap is not sent: the edit stays in memory and the header says so;
//   - a write names the revision it was read at; if PM says it is stale, the board is read again and
//     the person's changes are applied to what is there now, never over it;
//   - a flush on `lifecycle.suspend` and on unmount writes what was not yet written.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { CODE, getPref, PmError, setOverlay, setPref } from "../pm";
import {
  applyBoard,
  applyList,
  diffBoards,
  fullness as boardFullness,
  freshStored,
  isObj,
  MAX_ARGS_BYTES,
  readStored,
  sameJson,
  saveBytes,
  type Obj,
} from "./boardSync";
import { overlayEntries } from "./calendarEntries";
import {
  carryGameState,
  GAME_FIELDS,
  keepsRound,
  markDrawn,
  pruneSpent,
  withPrunedRound,
} from "./game";
import {
  besideRect,
  boardExtent,
  clampRect,
  COLS,
  findFreeRect,
  FOLDER_H,
  FOLDER_W,
  minSize,
  placeAt,
  reflowToWidth,
  resolveDrop,
  ROWS,
  type BoardSpace,
} from "./grid";
import {
  commit,
  commitBarrier,
  commitSilent,
  initHistory,
  redo,
  resetHistory,
  undo,
  type History,
} from "./history";
import { DEFAULT_TINT } from "./palette";
import {
  BOARD_VERSION,
  EMPTY_BOARD,
  PINBOARD_PREF_KEY,
  type Board,
  type CellPoint,
  type Rect,
  type TimelineItem,
  type Widget,
} from "./types";

// The mod-storage key the board is kept under (declared in the manifest's `data.keys`).
const PREF_KEY = PINBOARD_PREF_KEY;

/** A stable unique id; `crypto.randomUUID` in the webview, with a cheap fallback. */
function makeId(): string {
  try {
    return crypto.randomUUID();
  } catch {
    return `w-${Date.now().toString(36)}-${Math.floor(Math.random() * 1e6).toString(36)}`;
  }
}

/** Every kind this build can draw. A widget of any other kind is one a NEWER build made. */
const KNOWN_KINDS: readonly string[] = ["note", "timeline", "folder", "tasklist"];

/** The games a folder can be given (`GameKind`); any other value is one a newer build made. */
const GAME_KINDS: readonly string[] = ["roulette", "straws", "box", "coin", "rps"];

/** The card as the view can draw it: a field of a known kind with a value the view would trip on (a
 *  note whose text is a number, a timeline whose entries are not a list) is left out of what is
 *  DRAWN. It is not removed from what is STORED: a save writes only what the person changed, so a
 *  field this leaves out comes back exactly as it was unless they edit that very field. */
function drawable(x: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = { ...x };
  const keep = (key: string, ok: (v: unknown) => boolean) => {
    if (key in out && !ok(out[key])) delete out[key];
  };
  const isString = (v: unknown) => typeof v === "string";
  const isBool = (v: unknown) => typeof v === "boolean";
  for (const key of ["text", "title", "color", "ingestedAt", "ingestedHash"]) keep(key, isString);
  keep("project", (v) => v === null || typeof v === "string");
  for (const key of ["showOnCalendar", "gameOn", "autoPopOut", "repeat"]) keep(key, isBool);
  keep("view", (v) => v === "list" || v === "row");
  keep("expandMode", (v) => v === "inline" || v === "overlay");
  keep("game", (v) => typeof v === "string" && GAME_KINDS.includes(v));
  keep("weight", (v) => typeof v === "number" && Number.isFinite(v));
  keep("taskSort", isObj);
  keep("items", Array.isArray);
  // Only a folder holds cards, and validWidget builds that list itself. A `children` on any other kind
  // is a stored value the view would trip on (not a list, a list of nulls) and has no use for.
  if (out.kind !== "folder") delete out.children;
  if (Array.isArray(out.items)) {
    out.items = (out.items as unknown[])
      .filter((it): it is Record<string, unknown> => isObj(it) && typeof it.id === "string")
      .map((it) => ({
        ...it,
        date: typeof it.date === "string" ? it.date : "",
        label: typeof it.label === "string" ? it.label : "",
      }));
  }
  return out;
}

/** A widget is only usable if it has an id, a known kind, and a fully-numeric rect — the
 *  view dereferences `w.rect.x` unconditionally while rendering, and there's no error
 *  boundary, so one malformed entry would white-screen the app. Validate per-widget, not
 *  just the top-level shape. Returns the widget to DRAW (a folder may come back with fewer
 *  children), or null to leave it out of the drawing. A card left out is still STORED, and a save
 *  writes it back as it was (boardSync.ts): not drawing a card never deletes it. `depth` is how far
 *  inside a folder this is: folders do not nest, so a folder in a folder is not read at all. */
function validWidget(w: unknown, depth = 0): Widget | null {
  if (!w || typeof w !== "object" || Array.isArray(w)) return null;
  const x = w as Record<string, unknown>;
  if (typeof x.id !== "string") return null;
  if (typeof x.kind !== "string" || !KNOWN_KINDS.includes(x.kind)) return null;
  const r = x.rect as Record<string, unknown> | undefined;
  const rectOk =
    !!r &&
    typeof r.x === "number" &&
    typeof r.y === "number" &&
    typeof r.w === "number" &&
    typeof r.h === "number";
  if (!rectOk) return null;
  if (x.kind === "folder") {
    if (depth > 0 || !Array.isArray(x.children)) return null;
    // A folder keeps its valid, NON-folder children (folders never nest) in what is drawn. The others
    // are not drawn, and stay in what is stored (a kind a newer build added, say). The folder is
    // drawn if anything valid is left in it, or if it was empty to begin with (an empty folder is a
    // real thing the user made). A folder whose every child is unreadable is not drawn at all,
    // because there is nothing of it to show; it is still stored.
    const kids = x.children
      .map((c) => validWidget(c, depth + 1))
      .filter((c): c is Widget => c !== null && c.kind !== "folder");
    if (kids.length === 0 && x.children.length > 0) return null;
    return { ...(drawable(x) as unknown as Widget), children: kids };
  }
  return drawable(x) as unknown as Widget;
}

/** Apply `fn` to the widget with `id` wherever it lives — top-level or a folder child. Folders
 *  never nest, so a depth-1 walk reaches everything, and note/timeline mutators keep working
 *  unchanged whether the target is on the board or inside an open folder. */
function mapWidget(widgets: Widget[], id: string, fn: (w: Widget) => Widget): Widget[] {
  return widgets.map((w) => {
    if (w.id === id) return fn(w);
    if (w.kind === "folder" && w.children?.some((c) => c.id === id)) {
      return { ...w, children: w.children.map((c) => (c.id === id ? fn(c) : c)) };
    }
    return w;
  });
}

/** The kind of the widget with `id`, top-level or a folder child (depth-1, as {@link mapWidget}). */
function kindOf(widgets: Widget[], id: string): Widget["kind"] | undefined {
  for (const w of widgets) {
    if (w.id === id) return w.kind;
    const child = w.children?.find((c) => c.id === id);
    if (child) return child.kind;
  }
  return undefined;
}

/** A generous absolute cap (cells) so a corrupt/hand-edited pref can't blow the board up to a
 *  pathological size, while still dwarfing any real (even multi-monitor) layout. */
const MAX_BOARD_CELLS = 512;

/** The board to DRAW from a stored board's cards. Throws only on a value no board should hold (so deep
 *  it overflows the stack); the caller treats that board as unreadable and never writes it. */
export function deriveDisplay(stored: readonly unknown[], cols: number, rows: number): Board {
  // Discard anything that isn't the current shape rather than rendering garbage — both the
  // envelope and each widget, clamping rects back in-bounds for safety.
  const valid = stored.map((w) => validWidget(w)).filter((w): w is Widget => w !== null);
  // The board is a FIXED-WIDTH canvas = `cols` (the window) and grows only downward; give it a
  // generous vertical cap so widgets that overhang the width wrap onto new rows (the board scrolls)
  // rather than piling up. `rows` is the caller's generous vertical extent (≈ the screen height).
  const rowCap = Math.min(Math.max(rows, ROWS), MAX_BOARD_CELLS);
  // Sanity-clamp every rect to a generous box first (preserving the stored x/y up to the cap, so a
  // fitter keeps its exact spot) — this only guards a corrupt/hand-edited pref. Children are
  // clamped kind-aware; they aren't board-positioned so they don't take part in the re-flow.
  const sane = valid.map((w) => {
    const rect = clampRect(w.rect, MAX_BOARD_CELLS, MAX_BOARD_CELLS, minSize(w.kind));
    if (w.kind === "folder") {
      const children = (w.children ?? []).map((c) => ({
        ...c,
        rect: clampRect(c.rect, MAX_BOARD_CELLS, MAX_BOARD_CELLS, minSize(c.kind)),
      }));
      // The round a game folder is part-way through. Sanitised then pruned on the way in, because
      // this is the one moment we know the folder's real contents after an arbitrary gap: PM may
      // have been shut for an hour or a fortnight, and a card that left in between is not
      // "already drawn" — it is gone. Doing it here also means every later reader can trust the
      // list without re-checking it, which is what makes "only offer the cards that aren't greyed
      // out" true at launch and not merely true while the app stays open.
      const folder = { ...w, rect, children };
      const spent = Array.isArray(w.spent) ? w.spent.filter((s) => typeof s === "string") : [];
      return { ...folder, spent: pruneSpent({ ...folder, spent }) };
    }
    return { ...w, rect };
  });
  // Re-flow any widget that overhangs the fixed width back on-screen (wrapping to a new row). A
  // folder is NOT normalised by child count: an empty or single-card folder is a legitimate thing
  // the user made (+ Folder) and must survive a reload. This is what is DRAWN: the stored rects are
  // not changed by it, and a save writes a rect only for a card the person moved.
  return { version: BOARD_VERSION, widgets: reflowToWidth(sane, cols, rowCap) };
}

/** How the stored board's arrival went, and the ONLY thing that decides whether the board may be
 *  written back:
 *  - `loading` — nothing known yet.
 *  - `ready` — a board arrived (an empty one is a real answer: a fresh install has no board).
 *  - `failed` — the store didn't answer. The board on screen is the empty default, which is NOT the
 *    user's board; it is what we fall back to when we don't know theirs. Persisting it would destroy
 *    the real one, so a failed load never arms persistence and the view says so instead of pretending
 *    the board is empty.
 *  - `unreadable` — a board arrived that this build cannot read (a version it does not know, or not a
 *    board at all). It is left exactly as it is: nothing is drawn from it and nothing is written. */
export type BoardLoad = "loading" | "ready" | "failed" | "unreadable";

/** Why a save is not being kept, for the header to say: the last write failed (`failed`), the board
 *  is over PM's cap for one save (`tooBig`), or PM's copy changed to something this build cannot
 *  read, so saving is stopped (`blocked`). Null when what is on screen is what is kept. */
export type SaveProblem = "failed" | "tooBig" | "blocked" | null;

/** How long a change waits for a quiet moment before it is written. */
const SAVE_DELAY_MS = 500;
/** A revision conflict is read and merged this many times before the save is given up (the next
 *  change tries again). */
const MAX_MERGES = 3;

// The one board on screen asks for its own flush and refresh here, so the page's start-up can answer
// `lifecycle.suspend` and the project events without knowing the hook.
let flusher: (() => Promise<void>) | null = null;
let refresher: (() => Promise<void>) | null = null;

/** Write whatever the board holds that is not yet written, and wait for it (the page's answer to
 *  `lifecycle.suspend`). Resolves at once when there is no board on screen. Never rejects. */
export function flushBoard(): Promise<void> {
  return flusher ? flusher() : Promise.resolve();
}

/** Read the board again (a project was renamed, merged or deleted, which rewrites the project names
 *  in it) and merge what changed into what is on screen. Never rejects. */
export function refreshBoard(): Promise<void> {
  return refresher ? refresher() : Promise.resolve();
}

/** How a change enters the undo history. See {@link commitForPatch} and the mutators below for which
 *  change is which, and history.ts for why `silent` and `barrier` exist at all. */
type CommitKind = { mode: "push"; key?: string | null } | { mode: "silent" } | { mode: "barrier" };

const SILENT: CommitKind = { mode: "silent" };

/** The undoable weight of a board: the text it holds. Snapshots share structure for everything the
 *  user didn't touch, so rects and ids are effectively free — but a note's text is a fresh string on
 *  every keystroke, and it is the only thing here that can grow without bound. */
function weighBoard(b: Board): number {
  let n = 0;
  for (const w of b.widgets) {
    n += w.text?.length ?? 0;
    for (const c of w.children ?? []) n += c.text?.length ?? 0;
  }
  return n;
}

/** How a widget patch should be recorded. The default is one undo step per change; the exceptions are
 *  the changes that reach past the board. */
function commitForPatch(id: string, patch: Partial<Widget>, kind?: Widget["kind"]): CommitKind {
  // Ingest metadata mirrors a REAL vault document, which undo cannot delete. Rolling these fields
  // back would only make the note lie about whether it had been filed.
  if ("ingestedAt" in patch || "ingestedHash" in patch) return { mode: "silent" };
  // A folder's game and the round in progress are deliberately outside the undoable document:
  // `carryGameState` re-grafts the LIVE values onto whatever board undo restores, so a step that
  // only touches them would be a keystroke that visibly does nothing — and, since it still consumes
  // one, a second Ctrl+Z is then needed to reach the edit the user actually meant.
  const touched = Object.keys(patch);
  if (touched.length > 0 && touched.every((k) => (GAME_FIELDS as readonly string[]).includes(k))) {
    return { mode: "silent" };
  }
  // Linking a timeline to a project writes real milestones to the backend BEFORE this patch lands.
  // No board snapshot can retract them, and restoring the freeform entries they were made from would
  // draw both on the calendar — so a link is where the board's history honestly ends. (Unlinking
  // writes nothing, so it stays an ordinary, undoable step.)
  //
  // A task list's `project` is only its filter — choosing one writes nothing anywhere — so for that
  // kind it is an ordinary step, and Ctrl+Z puts the old filter back.
  if (kind !== "tasklist" && "project" in patch && patch.project !== undefined) {
    return { mode: "barrier" };
  }
  // Typing is grouped so one Ctrl+Z doesn't take a single character — keyed per widget and per field
  // so two notes, or a title and a body, never merge into each other.
  if ("text" in patch) return { mode: "push", key: `text:${id}` };
  if ("title" in patch) return { mode: "push", key: `title:${id}` };
  // Everything else — colour, geometry, view toggles — is its own step. Three colour clicks should be
  // three undos; and the swatches only show while editing, so a shared key would let a colour change
  // be swallowed by the text bucket around it.
  return { mode: "push" };
}

/** Board state + the mutators the view needs. Loads once from the store on mount and
 *  re-persists every change. `viewport` is the measured window (see PinboardView); the board's live
 *  extent is that grown to contain its content, computed here and returned as `bounds` (and read via
 *  a ref by the load effect + mutators): stored widgets are clamped to it on load and new widgets
 *  placed within it, so a widget survives a reload instead of snapping back.
 *
 *  Undo lives HERE rather than around the view's handlers, because this is the only writer: widgets
 *  are also created and destroyed inside `resolveDrop` (filing, folding) and dropped by `deriveDisplay`,
 *  paths no caller sees — and nothing outside could rebuild a deleted widget's id, text and rect
 *  anyway. One funnel, one place to be right. */
export function usePinboard(viewport: BoardSpace = { cols: COLS, rows: ROWS }) {
  const [hist, setHist] = useState<History<Board>>(() => initHistory(EMPTY_BOARD));
  const board = hist.present;

  // The board's live cell extent: the window (`viewport`), grown to contain any widget that sits past
  // it — the load-time reflow only wraps overhanging widgets downward, so growth is vertical and the
  // board scrolls to reach it, with room left for that scrollbar (`boardExtent`). Filing a widget into
  // a folder drops the top-level extent, so this shrinks back to the window. Read through a ref so the
  // mount-only load effect and the stable mutators track it without re-subscribing, and returned so
  // the canvas + drag clamp size to the same value. Never persisted — a pure function of window +
  // content, so it always fits the current screen.
  const bounds = useMemo(() => boardExtent(viewport, board.widgets), [viewport, board.widgets]);
  const boundsRef = useRef(bounds);
  boundsRef.current = bounds;

  const [load, setLoad] = useState<BoardLoad>("loading");
  const [problem, setProblem] = useState<SaveProblem>(null);
  const [unreadable, setUnreadable] = useState<"version" | "shape" | null>(null);
  /** The bytes the next save sends (what the header's meter shows against PM's cap). */
  const [sizeBytes, setSizeBytes] = useState(0);
  // ARMED IN EXACTLY ONE PLACE: the load's success path, below. Every write goes through `save`,
  // which refuses to run until this is true — so "we never learned what the board is" cannot end with
  // us writing what it isn't.
  const armed = useRef(false);
  /** PM's copy of the board turned into something this build cannot read after it had been read (the
   *  one way `armed` goes false once the load has landed). Saving waits, and the next change or the
   *  next refresh looks again: the moment PM's copy can be read, saving carries on. */
  const blocked = useRef(false);
  const [attempt, setAttempt] = useState(0);

  /** The board as PM holds it, verbatim: the only thing a save is built on. */
  const storedRef = useRef<(Obj & { widgets: unknown[] }) | null>(null);
  /** The display board the stored one was last in step with: a save is the difference from this. */
  const baselineRef = useRef<Board | null>(null);
  /** The person changed something that is not yet written. NEVER set by a load. */
  const unsaved = useRef(false);
  /** Counts changes, so a save can tell whether more came in while it was being written. */
  const changes = useRef(0);
  /** The dates the Calendar was last told about (JSON), so it is told again only when they change. */
  const lastOverlay = useRef<string | null>(null);
  const queue = useRef<Promise<unknown>>(Promise.resolve());
  const boardRef = useRef(board);
  boardRef.current = board;

  /** One thing at a time touches the stored board: saves, merges and refreshes wait their turn. */
  const enqueue = useCallback((job: () => Promise<void>): Promise<void> => {
    const run = queue.current.then(job, job);
    queue.current = run.catch(() => undefined);
    return queue.current as Promise<void>;
  }, []);

  /** The person changed the board (or undid a change): it is theirs to keep. */
  const touch = useCallback(() => {
    unsaved.current = true;
    changes.current += 1;
  }, []);

  /** Tell the Calendar the dated entries of the stored board, when they are not what it was last
   *  told. The first call of a frame always tells it (nothing was told yet): PM drops a paused mod's
   *  dates, so a frame that starts after a resume has to say them again. */
  const publishOverlay = useCallback(async () => {
    const stored = storedRef.current;
    if (!stored) return;
    try {
      const entries = overlayEntries(stored);
      const text = JSON.stringify(entries);
      if (text === lastOverlay.current) return;
      await setOverlay(entries);
      lastOverlay.current = text;
    } catch {
      /* a board too deep to read, or PM said no: the next saved change tells it again */
    }
  }, []);

  /** Take PM's copy of the board as the stored one, and carry what the person changed since the last
   *  save onto it: the display board becomes theirs-plus-ours, the undo trail ends (its snapshots
   *  predate theirs), and the next save is the same difference over what is now stored. Returns false
   *  when PM's copy cannot be read (saving stops: it is not ours to overwrite). */
  const adopt = useCallback(async (): Promise<boolean> => {
    let raw: string | null;
    try {
      raw = await getPref(PREF_KEY);
    } catch {
      setProblem("failed");
      return false;
    }
    const read = readStored(raw);
    let theirs: Obj & { widgets: unknown[] };
    let display: Board;
    try {
      if (read.kind === "unreadable") throw new Error("unreadable");
      theirs = read.kind === "ok" ? read.board : freshStored();
      display = deriveDisplay(theirs.widgets, boundsRef.current.cols, boundsRef.current.rows);
    } catch {
      armed.current = false;
      blocked.current = true;
      setProblem("blocked");
      return false;
    }
    const baseline = baselineRef.current;
    const mine = baseline ? diffBoards(baseline, boardRef.current) : null;
    storedRef.current = theirs;
    if (blocked.current) {
      // PM's copy can be read again: carry on saving, over what is there now (our changes are merged
      // into it below), and stop saying it is not being kept.
      blocked.current = false;
      armed.current = true;
      setProblem(null);
    }
    if (!mine && baseline && sameJson(display.widgets, baseline.widgets)) return true;
    baselineRef.current = display;
    const merged: Board = mine
      ? { ...display, widgets: applyList(display.widgets, mine) as Widget[] }
      : display;
    boardRef.current = merged;
    setHist(resetHistory(merged));
    setSizeBytes(saveBytes(PREF_KEY, JSON.stringify(theirs)));
    return true;
  }, []);

  /** The one write: the stored board with the person's changes applied. Every failure is REPORTED, not
   *  swallowed: the header tells the user their board is "Saved on this device", and a promise that
   *  can quietly be false is worse than no promise — they'd keep typing into a board that isn't being
   *  kept. What is not written stays `unsaved`, in memory, and goes with the next save. */
  const save = useCallback(async (): Promise<void> => {
    for (let merges = 0; merges <= MAX_MERGES; merges += 1) {
      if (blocked.current && !(await adopt())) return;
      const stored = storedRef.current;
      const baseline = baselineRef.current;
      if (!armed.current || !stored || !baseline) return;
      const current = boardRef.current;
      const seen = changes.current;
      const patch = diffBoards(baseline, current);
      if (!patch) {
        unsaved.current = changes.current !== seen;
        setProblem(null);
        return;
      }
      const next = applyBoard(stored, patch);
      const json = JSON.stringify(next);
      const bytes = saveBytes(PREF_KEY, json);
      setSizeBytes(bytes);
      if (bytes > MAX_ARGS_BYTES) {
        // Over PM's cap for one call: not sent, and not lost. It stays on screen and in memory.
        setProblem("tooBig");
        return;
      }
      try {
        await setPref(PREF_KEY, json);
      } catch (error) {
        const code = error instanceof PmError ? error.code : null;
        if (code === CODE.staleRevision && merges < MAX_MERGES) {
          // PM's copy changed since it was read (a rename rewrote a project name in it, say). Read it
          // again and apply our changes to THAT; never write over it.
          if (await adopt()) continue;
          return;
        }
        // PM refusing the size (the call is too large, or this mod's storage is full) is the board being
        // too big to keep, whatever our own count said.
        setProblem(code === CODE.tooLarge || code === CODE.quotaExceeded ? "tooBig" : "failed");
        return;
      }
      storedRef.current = next;
      baselineRef.current = current;
      unsaved.current = changes.current !== seen;
      setProblem(null);
      void publishOverlay();
      return;
    }
    setProblem("failed");
  }, [adopt, publishOverlay]);

  // Load the stored board. `attempt` re-runs it for the view's Retry.
  useEffect(() => {
    let cancelled = false;
    setLoad("loading");
    getPref(PREF_KEY)
      .then((raw) => {
        // A cancelled load must not arm: it belongs to a superseded attempt (or a StrictMode
        // double-mount), and its board is about to be replaced by the one still in flight.
        if (cancelled) return;
        const read = readStored(raw);
        const shut = (why: "version" | "shape") => {
          armed.current = false;
          blocked.current = false;
          storedRef.current = null;
          baselineRef.current = null;
          setUnreadable(why);
          setHist(resetHistory(EMPTY_BOARD));
          setLoad("unreadable");
        };
        if (read.kind === "unreadable") return shut(read.why);
        const stored = read.kind === "ok" ? read.board : freshStored();
        let display: Board;
        try {
          display = deriveDisplay(stored.widgets, boundsRef.current.cols, boundsRef.current.rows);
        } catch {
          return shut("shape");
        }
        storedRef.current = stored;
        baselineRef.current = display;
        boardRef.current = display;
        unsaved.current = false;
        setUnreadable(null);
        setProblem(null);
        setSizeBytes(saveBytes(PREF_KEY, raw ?? ""));
        // RESET, not commit: the board didn't change, it arrived. Committing it would leave the empty
        // default sitting in `past`, and the very first Ctrl+Z would wipe the user's board.
        setHist(resetHistory(display));
        blocked.current = false;
        armed.current = true;
        setLoad("ready");
        void publishOverlay();
      })
      .catch(() => {
        // The store didn't answer. Leave the empty board on screen but DON'T arm persistence: this
        // board is our fallback, not the user's, and writing it back — on the next edit or on the
        // unmount flush — would overwrite the real one with nothing. The view reports this rather
        // than showing an empty board that claims to be saved.
        if (cancelled) return;
        setLoad("failed");
      });
    return () => {
      cancelled = true;
    };
  }, [attempt, publishOverlay]);

  /** Try the load again after a failure (the store may simply not have been ready yet). */
  const retryLoad = useCallback(() => setAttempt((n) => n + 1), []);

  /** The one writer. `fn` produces the next board from the current one; `kind` says how that enters
   *  the undo history. `now` is read out here so the state updater stays pure (React may run it
   *  twice under StrictMode). */
  const change = useCallback(
    (fn: (b: Board) => Board, kind: CommitKind = { mode: "push" }) => {
      const now = Date.now();
      touch();
      setHist((h) => {
        const next = fn(h.present);
        if (kind.mode === "silent") return commitSilent(h, next);
        if (kind.mode === "barrier") return commitBarrier(h, next);
        return commit(h, next, { key: kind.key ?? null, now, weigh: weighBoard });
      });
    },
    [touch],
  );

  // Undo and redo both restore a whole past board, so both must re-graft the live round onto it —
  // see `carryGameState`. Without this, undoing a tint would un-grey cards a game had already drawn.
  const undoBoard = useCallback(() => {
    touch();
    setHist((h) => {
      const next = undo(h);
      return next === h ? h : { ...next, present: carryGameState(next.present, h.present) };
    });
  }, [touch]);
  const redoBoard = useCallback(() => {
    touch();
    setHist((h) => {
      const next = redo(h);
      return next === h ? h : { ...next, present: carryGameState(next.present, h.present) };
    });
  }, [touch]);

  // Write the changes — DEBOUNCED (F-15). The board object changes on every keystroke (a note's text
  // lives in it), and writing the whole JSON to the encrypted store each time is one IPC + SQLCipher
  // write per character. Coalesce to a single write ~500 ms after the last change; `save` reads the
  // latest board when it runs. A board that merely LOADED has nothing unsaved, so it sets no timer.
  useEffect(() => {
    if ((!armed.current && !blocked.current) || !unsaved.current) return;
    const handle = setTimeout(() => void enqueue(save), SAVE_DELAY_MS);
    return () => clearTimeout(handle);
  }, [board, enqueue, save]);

  /** Write what is not yet written, after anything already being written (the unmount flush, and the
   *  answer to `lifecycle.suspend`). */
  const flush = useCallback(
    () =>
      enqueue(async () => {
        if (unsaved.current) await save();
      }),
    [enqueue, save],
  );
  const refresh = useCallback(
    () =>
      enqueue(async () => {
        if (!armed.current && !blocked.current) return;
        if (await adopt()) await publishOverlay();
      }),
    [enqueue, adopt, publishOverlay],
  );
  // The unmount flush is best-effort: there is no UI left to report a failure to, and the debounced
  // write has already carried everything older than the last ~500 ms.
  useEffect(() => {
    flusher = flush;
    refresher = refresh;
    return () => {
      void flush();
      if (flusher === flush) flusher = null;
      if (refresher === refresh) refresher = null;
    };
  }, [flush, refresh]);

  // add* return the new widget's id so the view can scroll it into sight — a note placed on a
  // lower row would otherwise be created off-screen. The id is minted up front (outside the
  // functional update) so it can be returned synchronously.
  //
  // `at` is where the right-click menu was opened ("New note here"): the card goes there when the
  // spot is clear (`placeAt`).
  const addNote = useCallback(
    (at?: CellPoint) => {
      const id = makeId();
      change((b) => {
        const rect = placeAt(b.widgets, 7, 5, boundsRef.current.cols, boundsRef.current.rows, at);
        const widget: Widget = { id, kind: "note", rect, text: "", color: DEFAULT_TINT };
        return { ...b, widgets: [...b.widgets, widget] };
      });
      return id;
    },
    [change],
  );

  const addTimeline = useCallback(
    (at?: CellPoint) => {
      const id = makeId();
      change((b) => {
        const rect = placeAt(b.widgets, 9, 8, boundsRef.current.cols, boundsRef.current.rows, at);
        // Seed an empty title so the header shows its "Timeline" placeholder (matching notes/folders).
        const widget: Widget = { id, kind: "timeline", rect, title: "", items: [] };
        return { ...b, widgets: [...b.widgets, widget] };
      });
      return id;
    },
    [change],
  );

  /** A Task list card (PA-1): a live view of the global task list, filtered to `project` (unset →
   *  every task). The card stores only that filter; the tasks themselves live in the backend.
   *
   *  `near` places it beside that rect — right of it, else below it — when the spot is clear, so a
   *  note turned into a task list gets its list next to it; otherwise it takes the first free slot.
   *
   *  `fromConversion` records it as a history BARRIER, the same reason a timeline's project link is
   *  one: "Make this a task list" has already written real task rows before this card lands, and no
   *  board snapshot can take them back. An undo that removed the card would leave the tasks, and
   *  invite a second conversion that only finds them all "already there". The view passes it only
   *  when the conversion wrote tasks (`placeConvertedTaskList`): one that found them all there wrote
   *  nothing, so its card is an ordinary step and the history before it survives.
   *
   *  `at`, as for a note, is where the right-click menu was opened. */
  const addTaskList = useCallback(
    (project?: string, near?: Rect, fromConversion = false, at?: CellPoint) => {
      const id = makeId();
      change(
        (b) => {
          const { cols, rows } = boundsRef.current;
          const W = 9;
          const H = 8;
          const beside = near ? besideRect(b.widgets, near, W, H, cols) : null;
          const rect = beside ?? placeAt(b.widgets, W, H, cols, rows, at);
          const widget: Widget = { id, kind: "tasklist", rect, title: "" };
          if (project !== undefined) widget.project = project;
          return { ...b, widgets: [...b.widgets, widget] };
        },
        fromConversion ? { mode: "barrier" } : { mode: "push" },
      );
      return id;
    },
    [change],
  );

  /** An EMPTY folder, made on purpose. Nothing auto-dissolves it: it lives until the user ungroups
   *  it, so it can sit there waiting to be filled. `children: []` is not optional — `isValidWidget`
   *  silently drops a folder whose `children` isn't an array, which would lose it on the next load. */
  const addFolder = useCallback(
    (at?: CellPoint) => {
      const id = makeId();
      change((b) => {
        const rect = placeAt(
          b.widgets,
          FOLDER_W,
          FOLDER_H,
          boundsRef.current.cols,
          boundsRef.current.rows,
          at,
        );
        const widget: Widget = {
          id,
          kind: "folder",
          rect,
          title: "",
          children: [],
          expandMode: "inline",
        };
        return { ...b, widgets: [...b.widgets, widget] };
      });
      return id;
    },
    [change],
  );

  /** A copy of a card on the board, beside it where there's room (the right-click menu's Duplicate).
   *  The copy is a new card in its own right: it isn't the original's document (no ingest marks, so
   *  it offers its own Ingest), and its timeline entries get ids of their own. A folder isn't
   *  duplicated: its cards would each need copying, and the menu doesn't offer it. Returns the copy's
   *  id, or null when there was nothing to copy. */
  const duplicateWidget = useCallback(
    (id: string) => {
      const original = boardRef.current.widgets.find((w) => w.id === id);
      if (!original || original.kind === "folder") return null;
      const copyId = makeId();
      change((b) => {
        const w = b.widgets.find((x) => x.id === id);
        if (!w) return b;
        const { cols, rows } = boundsRef.current;
        const rect =
          besideRect(b.widgets, w.rect, w.rect.w, w.rect.h, cols) ??
          findFreeRect(b.widgets, w.rect.w, w.rect.h, cols, rows);
        const copy: Widget = { ...w, id: copyId, rect };
        delete copy.ingestedAt;
        delete copy.ingestedHash;
        if (w.items) copy.items = w.items.map((item) => ({ ...item, id: makeId() }));
        return { ...b, widgets: [...b.widgets, copy] };
      });
      return copyId;
    },
    [change],
  );

  const updateWidget = useCallback(
    (id: string, patch: Partial<Widget>) => {
      change(
        (b) => ({ ...b, widgets: mapWidget(b.widgets, id, (w) => ({ ...w, ...patch })) }),
        commitForPatch(id, patch, kindOf(boardRef.current.widgets, id)),
      );
    },
    [change],
  );

  /** Reposition a widget (used by resize; move gestures go through {@link dropWidget}). */
  const moveWidget = useCallback(
    (id: string, rect: Rect) => {
      change((b) => ({
        ...b,
        widgets: b.widgets.map((w) => (w.id === id ? { ...w, rect } : w)),
      }));
    },
    [change],
  );

  /** Commit a MOVE drop: may file the widget into the folder under the POINTER, merge two
   *  identically-placed widgets into a folder, or just reposition (see {@link resolveDrop}).
   *  `pointer` is the board cell the mouse was over on release, and is required rather than
   *  optional so every caller has to say what it means — `null` (unknown) files into nothing,
   *  which is the safe reading. */
  const dropWidget = useCallback(
    (id: string, rect: Rect, pointer: CellPoint | null) => {
      change((b) => ({
        ...b,
        widgets: resolveDrop(
          b.widgets,
          id,
          rect,
          boundsRef.current.cols,
          boundsRef.current.rows,
          makeId,
          pointer,
        ),
      }));
    },
    [change],
  );

  const removeWidget = useCallback(
    (id: string) => {
      change((b) => {
        const top = b.widgets.find((w) => w.id === id);
        if (top) {
          // A top-level FOLDER ungroups (spill its children back onto the board — non-destructive, so
          // deleting a tile never nukes the user's notes); a note/timeline is deleted outright.
          if (top.kind === "folder") {
            const { cols, rows } = boundsRef.current;
            let others = b.widgets.filter((w) => w.id !== id);
            for (const child of top.children ?? []) {
              const rect = findFreeRect(others, child.rect.w, child.rect.h, cols, rows);
              others = [...others, { ...child, rect }];
            }
            return { ...b, widgets: others };
          }
          return { ...b, widgets: b.widgets.filter((w) => w.id !== id) };
        }
        // A folder CHILD: remove it from its parent. The folder stays, however few cards are left —
        // it's the user's object, and its ✕ ("Ungroup") is how it goes away.
        return {
          ...b,
          widgets: b.widgets.map((w) =>
            w.kind === "folder" && w.children?.some((c) => c.id === id)
              ? // Deleted, so it is out of the round too — see `withPrunedRound`.
                withPrunedRound({ ...w, children: (w.children ?? []).filter((c) => c.id !== id) })
              : w,
          ),
        };
      });
    },
    [change],
  );

  /** Pull a child out of a folder back onto the board, into a free slot. The source folder stays put
   *  (empty if that was its last card) — only the user's ✕ ungroups it. */
  const popOutChild = useCallback(
    (folderId: string, childId: string) => {
      change((b) => {
        const { cols, rows } = boundsRef.current;
        const folder = b.widgets.find((w) => w.id === folderId && w.kind === "folder");
        const child = folder?.children?.find((c) => c.id === childId);
        if (!folder || !child) return b;
        const remaining = (folder.children ?? []).filter((c) => c.id !== childId);
        // A free slot, so the popped card lands in the clear. It is NOT put back through resolveDrop:
        // findFreeRect falls back to an OVERLAPPING origin when the board is full, which would let a
        // card pop straight back into the folder it just came out of.
        const landing = findFreeRect(b.widgets, child.rect.w, child.rect.h, cols, rows);
        // The card has left, so it is no longer "drawn this round" — it is gone. Same reasoning as
        // the draw's own pop-out branch: the round has to stay the same length as the pool it
        // describes, or the legend counts a card that is not there.
        const ws = b.widgets.map((w) =>
          w.id === folderId ? withPrunedRound({ ...w, children: remaining }) : w,
        );
        return { ...b, widgets: [...ws, { ...child, rect: landing }] };
      });
    },
    [change],
  );

  /**
   * Record that a game drew `childId` from `folderId`, and — when the folder is set to — move that
   * card out onto the board in the same step.
   *
   * SILENT, and for the same reason `ingestedAt` is: a draw is a record of something that already
   * happened, not an edit to the board. Ctrl+Z cannot un-draw a card, and if it recorded a step
   * then a run through a twenty-card folder would bury every real edit under twenty of them. The
   * write still persists — silence keeps it out of the history, not out of the store — which is the
   * whole point of storing the round: shut PM and the game is where you left it.
   *
   * The pop-out is folded in here rather than left to a second call so the two land as ONE board,
   * and so `markDrawn` sees the folder BEFORE the card leaves it: it decides whether that draw ends
   * the round, and a pool that had already shrunk would loop back one card early.
   *
   * `assigned` is what separates the two kinds of game. A wheel only ever hands you work, so its
   * draws are assignments. A verdict game (a coin, a throw) also takes cards OFF the table — you
   * won, so you dodged that one — and a card you dodged has still had its turn this round but is
   * emphatically not a job to move onto your board. Only an assignment is ever popped out.
   *
   * REQUIRED, with no default, and that is the point. It had one, and every caller in the app went
   * through a `(childId) => onDraw(folder.id, childId)` adapter that silently dropped it — so a
   * dodged card was popped out anyway, and TypeScript could not say so: a parameter with a default
   * is optional, which assigns cleanly to a prop type declared one argument short. A card that a
   * game has told you you are off the hook for must not land on your board, so the compiler is the
   * thing that has to catch the next adapter that forgets.
   */
  const drawCard = useCallback(
    (folderId: string, childId: string, assigned: boolean) => {
      change((b) => {
        const { cols, rows } = boundsRef.current;
        const folder = b.widgets.find((w) => w.id === folderId && w.kind === "folder");
        const child = folder?.children?.find((c) => c.id === childId);
        if (!folder || !child) return b;
        // A folder set to repeat keeps no round at all, so a draw records nothing: every card stays
        // in every play and the same one can come up twice running. `keepsRound` is the single gate
        // for that — greying and the live pool read it too, so the three can't disagree.
        const spent = keepsRound(folder) ? markDrawn(folder, childId) : [];
        if (!folder.autoPopOut || !assigned) {
          // Nothing changed and nothing is being moved: don't mint a board (and a write) per spin.
          if (!keepsRound(folder) && (folder.spent ?? []).length === 0) return b;
          return {
            ...b,
            widgets: b.widgets.map((w) => (w.id === folderId ? { ...w, spent } : w)),
          };
        }
        // Same landing rule as popOutChild: a free slot, so the card arrives in the clear.
        const landing = findFreeRect(b.widgets, child.rect.w, child.rect.h, cols, rows);
        const remaining = (folder.children ?? []).filter((c) => c.id !== childId);
        const ws = b.widgets.map((w) =>
          w.id === folderId
            ? // The card has left, so it is no longer "drawn this round" — it is gone. Pruning here
              // keeps the round the same length as the pool it describes.
              { ...w, children: remaining, spent: spent.filter((id) => id !== childId) }
            : w,
        );
        return { ...b, widgets: [...ws, { ...child, rect: landing }] };
      }, SILENT);
    },
    [change],
  );

  /** Move a widget to the end of the list (= painted on top) when it's interacted with. SILENT: it
   *  fires on every grab, so recording it would make merely touching a card an undo step — and undoing
   *  a z-order change nobody asked for is worse than not being able to. */
  const raiseWidget = useCallback(
    (id: string) => {
      change((b) => {
        if (b.widgets[b.widgets.length - 1]?.id === id) return b;
        const w = b.widgets.find((x) => x.id === id);
        if (!w) return b;
        return { ...b, widgets: [...b.widgets.filter((x) => x.id !== id), w] };
      }, SILENT);
    },
    [change],
  );

  /** The same, one level down: raise a child within its folder's own board, where cards overlap just
   *  like they do outside. Separate from {@link raiseWidget}, which only walks the top level. Silent
   *  for the same reason. */
  const raiseChild = useCallback(
    (folderId: string, childId: string) => {
      change((b) => {
        const folder = b.widgets.find((w) => w.id === folderId && w.kind === "folder");
        const kids = folder?.children;
        if (!kids || kids[kids.length - 1]?.id === childId) return b;
        const child = kids.find((c) => c.id === childId);
        if (!child) return b;
        return {
          ...b,
          widgets: b.widgets.map((w) =>
            w.id === folderId
              ? { ...w, children: [...kids.filter((c) => c.id !== childId), child] }
              : w,
          ),
        };
      }, SILENT);
    },
    [change],
  );

  const addTimelineItem = useCallback(
    (id: string) => {
      const item: TimelineItem = { id: makeId(), date: "", label: "" };
      change((b) => ({
        ...b,
        widgets: mapWidget(b.widgets, id, (w) => ({ ...w, items: [...(w.items ?? []), item] })),
      }));
    },
    [change],
  );

  const updateTimelineItem = useCallback(
    (id: string, itemId: string, patch: Partial<TimelineItem>) => {
      change(
        (b) => ({
          ...b,
          widgets: mapWidget(b.widgets, id, (w) => ({
            ...w,
            items: (w.items ?? []).map((it) => (it.id === itemId ? { ...it, ...patch } : it)),
          })),
        }),
        // Typed into, like a note — grouped per entry so an undo takes a few seconds of the label,
        // not one character.
        { mode: "push", key: `item:${id}:${itemId}` },
      );
    },
    [change],
  );

  const removeTimelineItem = useCallback(
    (id: string, itemId: string) => {
      change((b) => ({
        ...b,
        widgets: mapWidget(b.widgets, id, (w) => ({
          ...w,
          items: (w.items ?? []).filter((it) => it.id !== itemId),
        })),
      }));
    },
    [change],
  );

  return {
    board,
    /** The board's live cell extent (the window grown to contain content). The canvas sizes to it and
     *  the drag clamp reads it, so every consumer agrees on one board size. */
    bounds,
    /** Whether the stored board arrived. The add buttons wait on `ready`: a widget created before the
     *  load lands is overwritten by it, and with a history that would be an unrecoverable loss. */
    load,
    /** Retry a `failed` load. */
    retryLoad,
    /** The last write to the store failed (or PM's copy became one this build cannot read), so what's
     *  on screen isn't (all) on disk. Sticky until a write succeeds — the board keeps working, it just
     *  isn't being kept. */
    saveFailed: problem === "failed" || problem === "blocked",
    /** PM's copy of the board changed to something this build cannot read: nothing is written over it,
     *  and changes made here are kept in memory until it can be read again. */
    blocked: problem === "blocked",
    /** The board is over what PM takes in one save. Not sent and not lost: it is on screen. */
    tooBig: problem === "tooBig",
    /** The bytes the next save sends, and how full that is against PM's cap (percent). */
    sizeBytes,
    fullness: boardFullness(sizeBytes),
    /** Why a stored board is shown as unreadable (`load` is "unreadable"), or null. */
    unreadable,
    /** Write what is not yet written (and wait for it). */
    flush,
    undo: undoBoard,
    redo: redoBoard,
    /** Whether there's anything to undo or redo (the right-click menu greys them otherwise). */
    canUndo: hist.past.length > 0,
    canRedo: hist.future.length > 0,
    addNote,
    duplicateWidget,
    addTimeline,
    addFolder,
    addTaskList,
    updateWidget,
    moveWidget,
    dropWidget,
    removeWidget,
    raiseWidget,
    raiseChild,
    popOutChild,
    drawCard,
    addTimelineItem,
    updateTimelineItem,
    removeTimelineItem,
  };
}
