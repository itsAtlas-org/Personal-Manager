// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The frame side of the bridge (Mods spec §1.4, §1.5): what a mod's page imports to talk to PM.
// `connectHost()` asks PM for the page's MessagePort and returns a `ModHost`: `call` (a promise per
// call), `onEvent`, `documentVectors` (the binary answer, checked), `resize`. The protocol is written
// down once, in the header of src/components/mods/modBridge.ts, which PM's page implements; this is
// the other half of it, and it imports that file's constants and readers instead of copying them, so
// the two halves can't drift (no npm dependency: the module is plain TypeScript).
//
// WHERE IT LIVES (Mods PR B, decision D-5). `src/kit/host.ts`, decided once, so PR C does not move it:
// the lint fence lets a mod import from `src/kit/**` and nothing else in PM's `src/`, so a second
// home (a `mods/_sdk`) would be a second door; the D14 kit (shared components) lands in the same
// folder; and `src/**` is already what vitest collects and `tsc` checks. A fence of its own keeps the
// kit pure (eslint.config.js: no ipc, no Tauri API, no HTML sinks).
//
// WHAT THE SDK DOES SO A MOD NEED NOT.
// - Handshake. The listener is in place BEFORE the connect request is posted (PM answers at once), and
//   only a `port` message from the page's parent, carrying exactly one port, is taken, once. PM
//   answers a frame's connect once per session, so a second `connectHost()` returns the first's
//   promise and never asks again.
// - A queue in front of the port. PM stops a frame that has more than `MAX_IN_FLIGHT` (32) calls
//   waiting or that outruns its token bucket, and ends its session. The backend refuses more than 16
//   calls at once or 50 a second (burst 100) with MOD-3001. A mod that fires 1,000 storage reads at
//   once must trip neither, so calls wait here: at most `MAX_CONCURRENT_CALLS` (16) out at a time,
//   and sent no faster than `PACE_PER_SECOND` (40, burst 80) in one bucket that acknowledgements
//   also draw on (an ack costs the page a token like a call). src/kit/host.test.ts pins those
//   numbers to limits.rs and modBridge.ts.
// - Local refusals. A method name PM would refuse and arguments that are not plain JSON, or are past
//   the cap, are refused here with the same codes, and spend no token and no round trip.
// - Events, acknowledged cumulatively and batched: one `{ ack }` per `ACK_DELAY_MS` of quiet or per
//   `ACK_AFTER` events, never one per event. The backend lets 256 events wait unacknowledged and
//   drops the rest, saying so in the next event's `missed` (the mod's cue to re-list). PM starts
//   sending the moment it connects, and a mod's code runs a task or more after `connectHost`
//   resolves, so events wait (never more than `EVENT_BUFFER`) for the first `onEvent` handler and
//   reach it in order.
// - The end. `session.closed` and `mod.paused` are final for a frame: PM answers nothing more and
//   posts no goodbye after them, so every call still waiting is rejected with a `HostError` (it would
//   never be settled), as is every later one.
//
// Not done here, on purpose: a timeout on a call (a first-use question holds a call for as long as
// the person takes, and a model call is slow); retries (a call may have changed something); typed
// wrappers for the Host API's methods (each mod validates the shapes it reads).

import {
  BRIDGE_VERSION,
  EventPump,
  INTERNAL_MESSAGE,
  clampFrameHeight,
  encodeArgs,
  isArrayBuffer,
  isMethodName,
  readReply,
  readVectors,
  refusal,
  type DocumentVectors,
  type EventToFrame,
  type ModError,
  type ModEventName,
} from "../components/mods/modBridge";

export type { DocumentVectors, ModError, ModEventName };

/** Calls the SDK keeps out at once: the backend's own per-mod cap (`MAX_IN_FLIGHT` in limits.rs), and
 *  half the page's flood line, which counts a call held for a first-use question too. */
export const MAX_CONCURRENT_CALLS = 16;
/** The pace calls are sent at, per second sustained and in a burst: under the backend's bucket (50 a
 *  second, burst 100) so it never refuses one, and under the page's flood line (the same 50) with
 *  room for the acknowledgements that draw on it. */
export const PACE_PER_SECOND = 40;
export const PACE_BURST = 80;
/** Calls the SDK will hold waiting. A mod that queues more has a bug, and finds out. */
export const MAX_QUEUED_CALLS = 1000;
/** How long events may sit unacknowledged, and how many: the batch an acknowledgement covers. Well
 *  inside the backend's window of 256, so a frame that keeps up never has an event dropped. */
export const ACK_DELAY_MS = 50;
export const ACK_AFTER = 64;
/** How long `connectHost` waits for PM's port. PM answers at once; a page that is not PM's frame, or
 *  whose session ended before it started, hears nothing. */
export const CONNECT_TIMEOUT_MS = 10_000;

/** PM refused a call (or failed it), with its stable code (ERROR_CODES.md). `permission` is the one a
 *  refusal is about and `revision` the current one after a stale write, when PM said so. */
export class ModCallError extends Error {
  readonly code: string;
  readonly permission?: string;
  readonly revision?: number;

  constructor(error: ModError) {
    super(error.message);
    this.name = "ModCallError";
    this.code = error.code;
    if (error.permission !== undefined) this.permission = error.permission;
    if (error.revision !== undefined) this.revision = error.revision;
  }
}

/** Why the host can't be talked to, which is not PM refusing a call (that is a `ModCallError`):
 *  `not-framed` (this page is not in a frame), `no-port` (PM never answered), `closed` (the session
 *  ended or the port failed), `paused` (PM paused the mod), `queue-full` (this call was not queued). */
export type HostErrorKind = "not-framed" | "no-port" | "closed" | "paused" | "queue-full";

export class HostError extends Error {
  readonly kind: HostErrorKind;

  constructor(kind: HostErrorKind, message: string) {
    super(message);
    this.name = "HostError";
    this.kind = kind;
  }
}

/** An event PM told this frame about. `missed` is how many it had to drop just before this one. */
export interface HostEvent {
  name: ModEventName;
  seq: number;
  payload: unknown;
  missed?: number;
}

export interface ModHost {
  /** Call a Host API method. Resolves with its result, rejects with a `ModCallError` when PM refuses
   *  or fails the call and a `HostError` when the host is gone. */
  call(method: string, args?: unknown): Promise<unknown>;
  /** `embeddings.documentVectors`, with its binary answer checked: `ids` and `vectors` are views over
   *  one buffer PM moved to this frame, `count` and `dimension` agree with their lengths. */
  documentVectors(options?: { ids?: readonly number[]; cap?: number }): Promise<DocumentVectors>;
  /** Hear PM's events, in order. Returns the way to stop. A handler that throws is reported and
   *  does not stop the others or the acknowledgement. */
  onEvent(handler: (event: HostEvent) => void): () => void;
  /** Ask PM for a frame height in CSS pixels (clamped to PM's bounds; PM never answers). */
  resize(height: number): void;
  /** Answer `lifecycle.suspend`: what this frame was holding is saved, so PM may take it down now.
   *  Once per suspend; PM never replies, and waits no longer than its own deadline without it. */
  flushed(): void;
  /** Why the host is gone, once it is; null while it is there. */
  readonly closed: HostError | null;
}

/** What the SDK needs of the page and the clock, so tests drive it without either. */
export interface FrameEnv {
  /** Where the connect request goes; null when this page is not inside a frame. */
  parent: { postMessage(message: unknown, targetOrigin: string): void } | null;
  /** Hear messages posted to this page. Returns the way to stop. */
  listen(
    handler: (event: { source: unknown; data: unknown; ports: ReadonlyArray<MessagePort> }) => void,
  ): () => void;
  /** Milliseconds, monotonic (`performance.now()`). */
  now(): number;
  setTimer(run: () => void, ms: number): unknown;
  clearTimer(handle: unknown): void;
  /** Where a throwing event handler's error goes. */
  report(error: unknown): void;
}

/** The part of `window` the SDK reads. */
export interface WindowLike {
  parent: { postMessage(message: unknown, targetOrigin: string): void };
  addEventListener(type: "message", listener: (event: MessageEvent) => void): void;
  removeEventListener(type: "message", listener: (event: MessageEvent) => void): void;
}

/** The environment of a real page: its parent window, its message events, the platform's clock. An
 *  error from a handler is rethrown from a task of its own, so it reaches the page's error reporting
 *  without unwinding the SDK. */
export function windowEnv(win: WindowLike): FrameEnv {
  return {
    parent: win.parent === (win as unknown) ? null : win.parent,
    listen(handler) {
      const listener = (event: MessageEvent) => handler(event);
      win.addEventListener("message", listener);
      return () => win.removeEventListener("message", listener);
    },
    now: () => performance.now(),
    setTimer: (run, ms) => setTimeout(run, ms),
    clearTimer: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
    report: (error) => {
      queueMicrotask(() => {
        throw error;
      });
    },
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** The pace calls are sent at, in thousandths of a call so refilling is exact. The caller says what
 *  time it is. An acknowledgement spends a token unconditionally (it must not wait, or a slow frame
 *  would have events dropped), so the bucket can go into debt, bounded, and calls wait it out. */
class Pacer {
  private milli = PACE_BURST * 1000;
  private last: number;

  constructor(now: number) {
    this.last = now;
  }

  private refill(now: number) {
    const elapsed = Math.max(0, now - this.last);
    this.last = Math.max(this.last, now);
    this.milli = Math.min(PACE_BURST * 1000, this.milli + elapsed * PACE_PER_SECOND);
  }

  take(now: number): boolean {
    this.refill(now);
    if (this.milli < 1000) return false;
    this.milli -= 1000;
    return true;
  }

  spend(now: number) {
    this.refill(now);
    this.milli = Math.max(-PACE_BURST * 1000, this.milli - 1000);
  }

  /** Milliseconds until a call may go, at least 1. */
  waitMs(now: number): number {
    this.refill(now);
    return Math.max(1, Math.ceil((1000 - this.milli) / PACE_PER_SECOND));
  }
}

interface Waiting {
  method: string;
  /** The arguments as the JSON text that was measured; sent as exactly that JSON. */
  json: string;
  resolve: (result: unknown) => void;
  reject: (error: Error) => void;
}

/** The reason `session.closed` carries, when it is a short string. */
function closeReason(payload: unknown): string | null {
  const reason = isRecord(payload) ? payload.reason : undefined;
  return typeof reason === "string" && reason.length <= 32 ? reason : null;
}

/** The vectors answer as the frame receives it, checked. PM transfers views over one buffer, so
 *  `ids` and `vectors` are typed arrays whose lengths must agree with `count` and `dimension`; a raw
 *  buffer in the bridge's layout is decoded the same way the page does. Anything else is PM's own
 *  failure, and says so as MOD-9001 rather than handing the mod a half-read answer. */
export function readDocumentVectors(value: unknown): DocumentVectors {
  const broken = () => new ModCallError(refusal("internal", INTERNAL_MESSAGE));
  if (isArrayBuffer(value)) {
    const vectors = readVectors(value);
    if (!vectors) throw broken();
    return vectors;
  }
  if (!isRecord(value)) throw broken();
  const { count, dimension, ids, vectors } = value;
  const tag = (v: unknown) => Object.prototype.toString.call(v);
  if (
    typeof count !== "number" ||
    typeof dimension !== "number" ||
    !Number.isSafeInteger(count) ||
    !Number.isSafeInteger(dimension) ||
    count < 0 ||
    dimension < 0 ||
    tag(ids) !== "[object Float64Array]" ||
    tag(vectors) !== "[object Float32Array]"
  ) {
    throw broken();
  }
  const idView = ids as Float64Array;
  const vectorView = vectors as Float32Array;
  if (idView.length !== count || vectorView.length !== count * dimension) throw broken();
  return { count, dimension, ids: idView, vectors: vectorView };
}

/** One connected frame. Built only by `connectWith`, once PM's port exists. */
class Connection implements ModHost {
  private readonly env: FrameEnv;
  private readonly port: MessagePort;
  private readonly pacer: Pacer;
  private readonly queue: Waiting[] = [];
  private readonly pending = new Map<number, Waiting>();
  private readonly handlers = new Set<(event: HostEvent) => void>();
  /** Events wait here until a handler exists, and are told to handlers in order after. */
  private readonly events = new EventPump((event) => this.deliver(event));
  private nextId = 1;
  private pumpTimer: unknown = null;
  private ackTimer: unknown = null;
  private lastSeq = 0;
  private ackedSeq = 0;
  private lastHeight: number | null = null;
  private end: HostError | null = null;

  constructor(env: FrameEnv, port: MessagePort) {
    this.env = env;
    this.port = port;
    this.pacer = new Pacer(env.now());
    // Assigning `onmessage` starts the port, and what PM posted since it connected is delivered.
    port.onmessage = (event: MessageEvent) => this.onMessage(event.data);
  }

  get closed(): HostError | null {
    return this.end;
  }

  call(method: string, args?: unknown): Promise<unknown> {
    if (this.end) return Promise.reject(this.end);
    if (!isMethodName(method)) {
      return Promise.reject(
        new ModCallError(refusal("badArgs", "That is not a method name PM could read.")),
      );
    }
    const encoded = encodeArgs(args);
    if (!encoded.ok) return Promise.reject(new ModCallError(encoded.error));
    if (this.queue.length >= MAX_QUEUED_CALLS) {
      return Promise.reject(
        new HostError("queue-full", "Too many calls are waiting to be sent to PM."),
      );
    }
    return new Promise((resolve, reject) => {
      this.queue.push({ method, json: encoded.json, resolve, reject });
      this.pump();
    });
  }

  async documentVectors(
    options: { ids?: readonly number[]; cap?: number } = {},
  ): Promise<DocumentVectors> {
    return readDocumentVectors(await this.call("embeddings.documentVectors", options));
  }

  onEvent(handler: (event: HostEvent) => void): () => void {
    this.handlers.add(handler);
    // The first handler is told what came before it, in order (`connectHost` resolves a task or
    // more before a mod's code runs, and PM starts sending at once).
    this.events.connect();
    return () => {
      this.handlers.delete(handler);
    };
  }

  resize(height: number): void {
    if (typeof height !== "number") throw new TypeError("resize takes a number of pixels");
    const clamped = clampFrameHeight(height);
    if (clamped === null) throw new RangeError("resize takes a finite number of pixels");
    if (this.end || clamped === this.lastHeight) return;
    this.lastHeight = clamped;
    this.post({ resize: clamped });
  }

  flushed(): void {
    if (this.end) return;
    // Paid for like an acknowledgement: it must not wait behind calls, since PM is waiting on it.
    this.pacer.spend(this.env.now());
    this.post({ flushed: true });
  }

  /** Send what the limits allow, and when the pace is what stops it, come back when a token exists. */
  private pump() {
    if (this.end) return;
    while (this.queue.length > 0 && this.pending.size < MAX_CONCURRENT_CALLS) {
      const now = this.env.now();
      if (!this.pacer.take(now)) {
        this.pumpLater(this.pacer.waitMs(now));
        return;
      }
      const next = this.queue.shift();
      if (!next) return;
      const id = this.nextId++;
      this.pending.set(id, next);
      // The JSON that was measured, parsed back: structured clone would throw on a function a JSON
      // text drops, and would carry more than the page's own JSON.stringify keeps.
      if (!this.post({ id, method: next.method, args: JSON.parse(next.json) })) return;
    }
  }

  private pumpLater(ms: number) {
    if (this.pumpTimer !== null) return;
    this.pumpTimer = this.env.setTimer(() => {
      this.pumpTimer = null;
      this.pump();
    }, ms);
  }

  /** A message to PM. A port that throws is a port that is gone. */
  private post(message: unknown): boolean {
    try {
      this.port.postMessage(message);
      return true;
    } catch {
      this.close(new HostError("closed", "PM's connection to this mod ended."));
      return false;
    }
  }

  private onMessage(data: unknown) {
    if (this.end || !isRecord(data)) return;
    if (typeof data.id === "number" && "ok" in data) this.onReply(data.id, data);
    else if (typeof data.event === "string") this.takeEvent(data);
  }

  private onReply(id: number, data: Record<string, unknown>) {
    const call = this.pending.get(id);
    // Not a call this frame has out: a late answer, or one PM sent twice.
    if (!call) return;
    this.pending.delete(id);
    if (data.ok === true) {
      call.resolve(data.result);
    } else {
      // Read the way the page reads the backend's refusals: the code's shape, the message a string,
      // and only the two details a mod can act on. Anything else is PM's own failure.
      const reply = readReply({ kind: "error", error: data.error });
      call.reject(
        new ModCallError(
          reply.kind === "error" ? reply.error : refusal("internal", INTERNAL_MESSAGE),
        ),
      );
    }
    this.pump();
  }

  private takeEvent(data: Record<string, unknown>) {
    const { event: name, seq, payload, missed } = data;
    if (typeof seq !== "number" || !Number.isSafeInteger(seq) || seq < 1) return;
    // `seq` only grows. One that doesn't is a repeat, which is not told twice or acknowledged back.
    if (seq <= this.lastSeq) return;
    this.lastSeq = seq;
    // Told to the handlers, or held for the first one. A name PM's page doesn't know is dropped by
    // the pump, yet is still one the backend counts as outstanding, so it is acknowledged below.
    this.events.push({ name, seq, payload, missed });
    // After the handlers have heard it: a goodbye is the last thing they are told.
    if (name === "session.closed") {
      const reason = closeReason(payload);
      this.close(
        new HostError("closed", `PM ended this mod's session${reason ? ` (${reason})` : ""}.`),
      );
      return;
    }
    if (name === "mod.paused") {
      this.close(new HostError("paused", "PM paused this mod."));
      return;
    }
    this.noteEvent();
  }

  /** One event, in order, to every handler. */
  private deliver(raw: EventToFrame) {
    const event: HostEvent = { name: raw.event, seq: raw.seq, payload: raw.payload };
    if (raw.missed !== undefined) event.missed = raw.missed;
    for (const handler of [...this.handlers]) {
      try {
        handler(event);
      } catch (error) {
        this.env.report(error);
      }
    }
  }

  /** One more event taken: acknowledge when enough have piled up, else after a short quiet. */
  private noteEvent() {
    if (this.lastSeq - this.ackedSeq >= ACK_AFTER) {
      this.ack();
    } else if (this.ackTimer === null) {
      this.ackTimer = this.env.setTimer(() => {
        this.ackTimer = null;
        this.ack();
      }, ACK_DELAY_MS);
    }
  }

  /** Acknowledge everything taken so far. Reached only with an event not yet acknowledged and a host
   *  that is there: the timer starts on such an event, and `close` clears it. */
  private ack() {
    if (this.ackTimer !== null) {
      this.env.clearTimer(this.ackTimer);
      this.ackTimer = null;
    }
    this.pacer.spend(this.env.now());
    this.ackedSeq = this.lastSeq;
    this.post({ ack: this.lastSeq });
  }

  /** The host is gone: nothing more will be answered, so everything waiting is rejected now. */
  private close(error: HostError) {
    if (this.end) return;
    this.end = error;
    for (const timer of [this.pumpTimer, this.ackTimer]) {
      if (timer !== null) this.env.clearTimer(timer);
    }
    this.pumpTimer = null;
    this.ackTimer = null;
    this.port.onmessage = null;
    this.port.close();
    const waiting = [...this.pending.values(), ...this.queue];
    this.pending.clear();
    this.queue.length = 0;
    for (const call of waiting) call.reject(error);
  }
}

/** Ask PM for this page's port, in `env`. Rejects with a `HostError` when this page is not in a frame
 *  or PM does not answer in time. Every call asks, so a caller that may run twice uses
 *  `connectHost()`, which asks once. */
export function connectWith(env: FrameEnv): Promise<ModHost> {
  return new Promise((resolve, reject) => {
    const parent = env.parent;
    if (!parent) {
      reject(new HostError("not-framed", "This page is not running inside PM."));
      return;
    }
    let timer: unknown = null;
    // In place before the request is posted: PM answers a connect once and at once.
    const stop = env.listen((event) => {
      const data = event.data;
      if (event.source !== parent) return;
      if (!isRecord(data) || data.pm !== "port" || data.version !== BRIDGE_VERSION) return;
      if (event.ports.length !== 1) return;
      stop();
      env.clearTimer(timer);
      resolve(new Connection(env, event.ports[0]));
    });
    timer = env.setTimer(() => {
      stop();
      reject(new HostError("no-port", "PM did not answer this mod's request to connect."));
    }, CONNECT_TIMEOUT_MS);
    try {
      parent.postMessage({ pm: "connect", version: BRIDGE_VERSION }, "*");
    } catch {
      stop();
      env.clearTimer(timer);
      reject(new HostError("no-port", "PM did not answer this mod's request to connect."));
    }
  });
}

let shared: Promise<ModHost> | null = null;

/** The host of this page, connected once: a second call returns the same promise (PM answers a
 *  frame's request only once per session, so a second ask would hear nothing). */
export function connectHost(): Promise<ModHost> {
  if (shared === null) {
    shared =
      typeof window === "undefined"
        ? Promise.reject(new HostError("not-framed", "This page is not running inside PM."))
        : connectWith(windowEnv(window));
  }
  return shared;
}
