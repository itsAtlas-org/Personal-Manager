// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The bridge between a mod's sealed frame and PM (Mods spec §1.4, "The bridge"): what PM accepts
// from a frame, what it sends back, and the bounds it holds a frame to. Pure; ModFrame.tsx wires it
// to the frame and to `modCall` in ipc.ts.
//
// The protocol, version 1:
// - The frame asks for its port on its parent: `parent.postMessage({ pm: "connect", version: 1 }, "*")`.
//   PM answers only the frame it mounted (the message's `source` is that iframe's window), only while
//   the frame's origin is opaque, and once per session: it posts `{ pm: "port", version: 1 }` to the
//   frame with a MessagePort transferred. Both sides post with "*", because an opaque origin can't be
//   named. Every mod's origin is "null", so the origin can't tell two mods apart; the `source` does.
// - Over the port the frame sends a call, `{ id, method, args }`, a height request, `{ resize }`, or
//   (once PM has told it `lifecycle.suspend`, which it needs no permission to hear) its answer,
//   `{ flushed: true }`: what it was holding is saved, so PM may take the frame down. PM never
//   answers that either. Rust waits for it up to its deadline, and only from a mod whose manifest
//   says `"answersSuspend": true` (suspend.rs); any other mod's frame is not waited for.
//   PM answers a call with `{ id, ok: true, result }` or `{ id, ok: false, error }`, where `error` is
//   `{ code, message }` with a stable `MOD-####` code (ERROR_CODES.md), plus `permission` when a
//   permission is the reason and `revision` when a stored value moved on. PM never answers a height
//   request. A result is plain JSON, except `embeddings.documentVectors`'s: `{ count, dimension, ids,
//   vectors }` with `ids` a Float64Array and `vectors` a Float32Array (count x dimension, document by
//   document, in `ids`' order), views over one ArrayBuffer that PM transfers to the frame, so a
//   5,000 x 1,024 answer (20 MB) isn't copied (`readVectors`).
// - PM tells a frame things as they happen (spec §1.5 "Events"): over the port, `{ event, seq, payload,
//   missed? }`, which no reply looks like (a reply has `id` and `ok`). The backend has already decided,
//   per mod and per grant, whether the mod may hear the event, so the page forwards every one it is
//   given and filters nothing itself; it only keeps names it knows (`MOD_EVENTS`). `seq` counts from 1
//   per session. The frame acknowledges what it has taken with `{ ack: seq }` (cumulative; at most
//   once per batch, since an ack costs a token like a call); the backend lets at most `EVENT_BUFFER`
//   events be unacknowledged, and drops, counts and reports (`missed`) any past that, so a slow frame
//   can't grow anyone's memory: not Rust's, the page's, or its own port's. Events that arrive before
//   the port exists are held, never more than `EVENT_BUFFER` of them, and sent when it connects
//   (`EventPump`).
// - A frame that floods is stopped: its 33rd call in flight, or a burst well past the backend's rate
//   bucket, closes its port and ends its session (spec §1.5 "Rate limits"). The backend holds its own
//   limits behind this line (`src-tauri/src/mods/api/limits.rs`), and a test there pins the numbers
//   here against them.
//
// Nothing a frame sends names a mod or a session. PM calls under the session it opened for that frame,
// and Rust resolves the session to its mod (`mod_call`). A message PM can't read is dropped, or, when it
// carries a usable id, refused with a reason.

export const BRIDGE_VERSION = 1;

/** Calls one frame may have waiting at once. A frame that sends one more is flooding PM's page, which
 *  runs every call, and is stopped. */
export const MAX_IN_FLIGHT = 32;

/** A call's arguments, as UTF-8 bytes of JSON: the backend's cap too (`MAX_ARGS_BYTES` in
 *  limits.rs). Tauri reads the whole payload before Rust can refuse it, so the page holds the line. */
export const MAX_ARGS_BYTES = 1_048_576;

/** The most bytes of binary answer the backend sends (`MAX_BINARY_RESULT_BYTES` in limits.rs; a test
 *  holds them equal): 20 MiB, the Host API's largest, 5,000 documents of 1,024 dimensions. */
export const MAX_BINARY_BYTES = 20_971_520;

/** The page's flood line, per frame: a token bucket refilled at the backend's rate (50 calls a second)
 *  and twice its burst. A mod that keeps calling after the backend refuses it with MOD-3001 crosses
 *  it and is stopped. */
export const FLOOD_CALLS_PER_SECOND = 50;
export const FLOOD_BURST = 200;

/** The heights a frame may ask for, in CSS pixels. */
export const MIN_FRAME_HEIGHT = 64;
export const MAX_FRAME_HEIGHT = 4096;

/** The codes the page itself answers with or reads; the backend answers with the rest. A test in
 *  error.rs pins each to the backend's code of the same name. `needsPrompt` is the one the page
 *  turns into PM's first-use question; `notGranted` is what a mod is told after the person says no
 *  to it. The lines from `aiCapReached` on are the experimental calls' (PR B), listed so a page-side
 *  answer for `ai.complete`, `net.fetch`, `secrets.set` or `notify` names a code by word and the
 *  same test holds it to the backend's; their numbers freeze at the first release after PR C. The
 *  last three are PR C's: `needsConfirmation` is the one PM's page turns into its own yes/no question
 *  (answered with `mod_confirm`), and `declined` is what the mod is told after a no. */
export const MOD_ERRORS = {
  unknownMethod: "MOD-1001",
  badArgs: "MOD-1003",
  notGranted: "MOD-2001",
  needsPrompt: "MOD-2002",
  rateLimited: "MOD-3001",
  tooLarge: "MOD-3002",
  internal: "MOD-9001",
  aiCapReached: "MOD-3004",
  callTimedOut: "MOD-3005",
  noModel: "MOD-5004",
  priceUnknown: "MOD-5005",
  modelCallFailed: "MOD-5006",
  hostNotAllowed: "MOD-6001",
  hostNotPublic: "MOD-6002",
  hostUnresolved: "MOD-6003",
  networkUnavailable: "MOD-6004",
  tooManyRedirects: "MOD-6005",
  responseTooLarge: "MOD-6006",
  secretNotSet: "MOD-6007",
  engineNotReady: "MOD-5007",
  needsConfirmation: "MOD-2003",
  declined: "MOD-2004",
} as const;

/** The calls PM's page performs once the backend has authorised them (`api::PageAction`), because
 *  only the page holds what they need. A test in handlers.rs pins this list to the backend's. */
export const PAGE_ACTIONS = [
  "theme.get",
  "ui.openDocument",
  "secrets.set",
  "notify",
  "ui.openTaskEditor",
] as const;

export type PageAction = (typeof PAGE_ACTIONS)[number];

/** The events PM tells a frame (`api::events::EVENTS`; a test in events.rs pins this list to the
 *  backend's). The backend sends only these, and only to a mod that may hear them. */
export const MOD_EVENTS = [
  "documents.added",
  "documents.deleted",
  "theme.changed",
  "grants.changed",
  "mod.paused",
  "session.closed",
  "tasks.changed",
  "projects.renamed",
  "projects.merged",
  "projects.deleted",
  "calendar.synced",
  "lifecycle.suspend",
] as const;

export type ModEventName = (typeof MOD_EVENTS)[number];

/** Events the backend lets be unacknowledged at once (`EVENT_WINDOW` in limits.rs; a test holds them
 *  equal), so the most the page can hold for a frame that has no port yet. */
export const EVENT_BUFFER = 256;

/** Method names: what the host API's dotted names look like, bounded. Rust checks each against its
 *  dispatch table; this only keeps junk off the IPC channel. */
const METHOD = /^[A-Za-z][A-Za-z0-9._-]{0,127}$/;

/** Whether `name` is shaped like a method name. One test for both ends of the port: the page refuses
 *  what fails it, and the frame SDK (src/kit/host.ts) refuses it first, so a typo costs no token. */
export function isMethodName(name: unknown): name is string {
  return typeof name === "string" && METHOD.test(name);
}

/** A reason sent back to a frame is clipped: PM's messages are short, and a mod has no use for more. */
const MAX_ERROR_CHARS = 500;

/** What `internal` always says: what went wrong inside PM stays in PM. */
export const INTERNAL_MESSAGE = "PM couldn't complete that call.";

/** PM's answer to a connect request. Frozen: it is posted to every frame as is. */
export const PORT_MESSAGE = Object.freeze({ pm: "port", version: BRIDGE_VERSION });

/** A refusal, as a frame receives it. */
export interface ModError {
  code: string;
  message: string;
  permission?: string;
  revision?: number;
}

/** What PM does with one message from a frame's port. `args` is the JSON text PM measured. */
export type PortMessage =
  | { kind: "call"; id: number; method: string; args: string }
  | { kind: "resize"; height: number }
  | { kind: "ack"; seq: number }
  | { kind: "flushed" }
  | { kind: "refuse"; id: number; error: ModError }
  | { kind: "drop" };

const DROP: PortMessage = { kind: "drop" };

/** The document vectors as a frame receives them: views over one buffer (`readVectors`). */
export interface DocumentVectors {
  count: number;
  dimension: number;
  /** The document ids, in the order the vectors follow. */
  ids: Float64Array;
  /** `count` x `dimension` floats, document by document. */
  vectors: Float32Array;
}

/** What the backend answered a call with (`api::Reply`). A `binary` answer is the vectors, already
 *  checked, with the buffer both views lie over (the one to transfer). */
export type BackendReply =
  | { kind: "value"; value: unknown }
  | { kind: "page"; action: PageAction; args: unknown }
  | { kind: "binary"; vectors: DocumentVectors; buffer: ArrayBuffer }
  | { kind: "error"; error: ModError };

/** The binary layout's version, and its fixed header (four little-endian u32s: version, count,
 *  dimension, reserved); `src-tauri/src/mods/api/embeddings.rs` holds the layout. */
const VECTORS_VERSION = 1;
const VECTORS_HEADER_BYTES = 16;

/** The layout is little-endian and the views read the machine's byte order, so a big-endian machine
 *  (none PM runs on) would read garbage: it refuses instead. */
const LITTLE_ENDIAN = new Uint8Array(new Uint16Array([1]).buffer)[0] === 1;

/** Whether `data` is an ArrayBuffer, by its tag, so one from another realm (the webview's IPC layer, a
 *  test's) counts. */
export function isArrayBuffer(data: unknown): data is ArrayBuffer {
  return Object.prototype.toString.call(data) === "[object ArrayBuffer]";
}

/** The vectors answer, checked against its own header: the right version, the length exactly the
 *  header's arithmetic, inside the cap. `null` for anything else, which is PM's failure, never passed
 *  on. */
export function readVectors(buffer: ArrayBuffer): DocumentVectors | null {
  if (!LITTLE_ENDIAN) return null;
  const size = buffer.byteLength;
  if (size < VECTORS_HEADER_BYTES || size > MAX_BINARY_BYTES) return null;
  const header = new DataView(buffer);
  const [version, count, dimension, reserved] = [0, 4, 8, 12].map((at) =>
    header.getUint32(at, true),
  );
  if (version !== VECTORS_VERSION || reserved !== 0) return null;
  // `count * dimension` is exact in a double: both are u32s and the buffer is under 20 MiB, so a
  // product past the buffer is caught before it could lose precision.
  const floats = count * dimension;
  if (floats > size) return null;
  if (size !== VECTORS_HEADER_BYTES + 8 * count + 4 * floats) return null;
  return {
    count,
    dimension,
    ids: new Float64Array(buffer, VECTORS_HEADER_BYTES, count),
    vectors: new Float32Array(buffer, VECTORS_HEADER_BYTES + 8 * count, floats),
  };
}

/** Messages arrive structured-cloned: plain data, with no getters or prototypes of the sender's. */
function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Whether a message on PM's window is a frame asking for its port. The caller has already checked
 *  that it came from the frame PM mounted. */
export function isConnectRequest(data: unknown): boolean {
  return isRecord(data) && data.pm === "connect" && data.version === BRIDGE_VERSION;
}

/** A requested height held to PM's bounds; `null` for anything that isn't a finite number. */
export function clampFrameHeight(requested: unknown): number | null {
  if (typeof requested !== "number" || !Number.isFinite(requested)) return null;
  return Math.min(MAX_FRAME_HEIGHT, Math.max(MIN_FRAME_HEIGHT, Math.round(requested)));
}

/** Whether `text` is at most `cap` UTF-8 bytes. Its byte length is never less than its UTF-16 length
 *  and at most three times it, so the encoder runs only when the answer could fall either side. */
export function fitsBytes(text: string, cap: number): boolean {
  if (text.length > cap) return false;
  if (text.length * 3 <= cap) return true;
  return new TextEncoder().encode(text).length <= cap;
}

/** A refusal the page makes itself. */
export function refusal(code: keyof typeof MOD_ERRORS, message: string): ModError {
  return { code: MOD_ERRORS[code], message };
}

/** Thrown by a page action that PM's page, and not the backend, has to refuse (only the page knows
 *  that an editor is already open): the frame is told this code, never PM's own failure. */
export class PageRefusal extends Error {
  constructor(
    readonly code: keyof typeof MOD_ERRORS,
    message: string,
  ) {
    super(message);
    this.name = "PageRefusal";
  }
}

/** A call's arguments as exactly the JSON text PM will send, measured in bytes. Structured clone
 *  carries more than JSON (a BigInt, a cycle), and none of it is an argument PM's API takes. */
export function encodeArgs(
  args: unknown,
): { ok: true; json: string } | { ok: false; error: ModError } {
  let json: string;
  try {
    json = JSON.stringify(args === undefined ? null : args);
  } catch {
    return { ok: false, error: refusal("badArgs", "A call's arguments must be plain JSON.") };
  }
  if (!fitsBytes(json, MAX_ARGS_BYTES)) {
    return { ok: false, error: refusal("tooLarge", "A call's arguments are too large.") };
  }
  return { ok: true, json };
}

/** What PM makes of one message from a frame's port. */
export function readPortMessage(data: unknown): PortMessage {
  if (!isRecord(data)) return DROP;
  if ("resize" in data) {
    const height = clampFrameHeight(data.resize);
    return height === null ? DROP : { kind: "resize", height };
  }
  if ("ack" in data) {
    const seq = data.ack;
    return typeof seq === "number" && Number.isSafeInteger(seq) && seq >= 0
      ? { kind: "ack", seq }
      : DROP;
  }
  // The answer to `lifecycle.suspend`: the frame has saved what it held. Only `true` says so.
  if ("flushed" in data) return data.flushed === true ? { kind: "flushed" } : DROP;
  const id = data.id;
  if (typeof id !== "number" || !Number.isSafeInteger(id) || id < 0) return DROP;
  const method = data.method;
  if (!isMethodName(method)) {
    return {
      kind: "refuse",
      id,
      error: refusal("badArgs", "PM couldn't read that call's method."),
    };
  }
  const args = encodeArgs(data.args);
  if (!args.ok) return { kind: "refuse", id, error: args.error };
  return { kind: "call", id, method, args: args.json };
}

const CODE = /^MOD-\d{4}$/;

/** The backend's answer, checked: anything not in its shape is PM's own failure, never passed on. Raw
 *  bytes are the vectors; everything else is the JSON reply. */
export function readReply(data: unknown): BackendReply {
  const broken: BackendReply = { kind: "error", error: refusal("internal", INTERNAL_MESSAGE) };
  if (isArrayBuffer(data)) {
    const vectors = readVectors(data);
    return vectors ? { kind: "binary", vectors, buffer: data } : broken;
  }
  if (!isRecord(data)) return broken;
  switch (data.kind) {
    case "value":
      return "value" in data ? { kind: "value", value: data.value } : broken;
    case "page": {
      const action = data.action;
      if (typeof action !== "string") return broken;
      if (!(PAGE_ACTIONS as readonly string[]).includes(action)) {
        return { kind: "error", error: refusal("unknownMethod", "PM has no call by that name.") };
      }
      return { kind: "page", action: action as PageAction, args: data.args };
    }
    case "error": {
      const e = data.error;
      if (!isRecord(e) || typeof e.code !== "string" || !CODE.test(e.code)) return broken;
      if (typeof e.message !== "string") return broken;
      const error: ModError = { code: e.code, message: e.message };
      if (typeof e.permission === "string") error.permission = e.permission;
      if (typeof e.revision === "number" && Number.isSafeInteger(e.revision)) {
        error.revision = e.revision;
      }
      return { kind: "error", error };
    }
    default:
      return broken;
  }
}

/** One event as a frame receives it. */
export interface EventToFrame {
  event: ModEventName;
  seq: number;
  payload: unknown;
  /** How many events were dropped just before this one, when any were. */
  missed?: number;
}

/** An event from the backend's channel, checked: a name PM knows and a sequence number. Anything
 *  else is PM's own failure and is dropped (it can't be acknowledged, so it can't be told of). */
export function readEvent(raw: unknown): EventToFrame | null {
  if (!isRecord(raw)) return null;
  const { name, seq, payload, missed } = raw;
  if (typeof name !== "string" || !(MOD_EVENTS as readonly string[]).includes(name)) return null;
  if (typeof seq !== "number" || !Number.isSafeInteger(seq) || seq < 1) return null;
  const event: EventToFrame = { event: name as ModEventName, seq, payload: payload ?? null };
  if (typeof missed === "number" && Number.isSafeInteger(missed) && missed > 0) {
    event.missed = missed;
  }
  return event;
}

/** Where a frame's events wait until its port exists, and how they go to it after: in order, each
 *  checked. The backend never has more than `EVENT_BUFFER` outstanding, so the hold can't pass that
 *  unless the backend is wrong; if it does, the excess is dropped and the next event says so. */
export class EventPump {
  private held: EventToFrame[] = [];
  private connected = false;
  private lost = 0;
  private readonly post: (event: EventToFrame) => void;

  constructor(post: (event: EventToFrame) => void) {
    this.post = post;
  }

  /** An event from the backend. */
  push(raw: unknown): void {
    const event = readEvent(raw);
    if (!event) return;
    if (this.lost > 0) {
      event.missed = (event.missed ?? 0) + this.lost;
      this.lost = 0;
    }
    if (this.connected) {
      this.post(event);
    } else if (this.held.length >= EVENT_BUFFER) {
      this.lost += 1 + (event.missed ?? 0);
    } else {
      this.held.push(event);
    }
  }

  /** The frame's port exists: send what was held, in order, then everything from here on. */
  connect(): void {
    if (this.connected) return;
    this.connected = true;
    const held = this.held;
    this.held = [];
    for (const event of held) this.post(event);
  }
}

/** A refusal's text for the frame, clipped. */
export function errorText(text: string): string {
  return text.length > MAX_ERROR_CHARS ? `${text.slice(0, MAX_ERROR_CHARS)}…` : text;
}

/** The answer to a call that succeeded. */
export function callResult(id: number, result: unknown) {
  return { id, ok: true as const, result };
}

/** The answer to a call PM or the backend refused: the code, the message clipped, and the two
 *  details a mod can act on, and nothing else. */
export function callError(id: number, error: ModError) {
  const sent: ModError = { code: error.code, message: errorText(error.message) };
  if (error.permission !== undefined) sent.permission = error.permission;
  if (error.revision !== undefined) sent.revision = error.revision;
  return { id, ok: false as const, error: sent };
}

/** The page's flood line for one frame: a token bucket the caller feeds the time (milliseconds, as
 *  `performance.now()` gives it), so it is tested without a clock. Counted in thousandths of a call,
 *  which `perSecond` calls a second earns exactly `perSecond` of per millisecond. */
export class FloodGate {
  private milli: number;
  private last: number;
  private readonly perSecond: number;
  private readonly burst: number;

  constructor(now: number, perSecond = FLOOD_CALLS_PER_SECOND, burst = FLOOD_BURST) {
    this.perSecond = perSecond;
    this.burst = burst;
    this.milli = burst * 1000;
    this.last = now;
  }

  /** Spend one message's token at `now`. False once the frame has outrun the bucket. A clock that
   *  goes back earns nothing. */
  take(now: number): boolean {
    const elapsed = Math.max(0, now - this.last);
    this.last = Math.max(this.last, now);
    this.milli = Math.min(this.burst * 1000, this.milli + elapsed * this.perSecond);
    if (this.milli < 1000) return false;
    this.milli -= 1000;
    return true;
  }
}
