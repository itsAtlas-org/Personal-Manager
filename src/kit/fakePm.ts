// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A stand-in for PM, for tests of code that talks to it through src/kit/host.ts: the Host API tests of
// the SDK itself, and the DOM tests of a mod's page. It is PM's side of the bridge as ModFrame.tsx
// wires it, built from the same modBridge.ts pieces (`readPortMessage`, `FloodGate`, `EventPump`,
// `callResult`, `callError`), so a test that passes here has met the page's real reading rules and its
// real flood stop. It also holds the backend's own limits (16 calls at once, 50 a second in a burst
// of 100, 256 events outstanding), so a mod that outruns them sees MOD-3001 and dropped events here
// as it would there. Ships nowhere: only tests import it.
//
// Time is a `FakeClock` the test advances, shared with the SDK through `env`, so the pace and the
// acknowledgement delay are measured in simulated milliseconds and a test is fast and exact. Port
// messages are real (a `MessageChannel`), so ordering, structured clone and transfer are the
// platform's, and `settle()` waits for them to drain.

import {
  EVENT_BUFFER,
  EventPump,
  FloodGate,
  MAX_IN_FLIGHT,
  PORT_MESSAGE,
  callError,
  callResult,
  isConnectRequest,
  readPortMessage,
  type BackendReply,
  type ModEventName,
} from "../components/mods/modBridge";
import type { FrameEnv } from "./host";

/** The backend's limits, restated (limits.rs; src/kit/host.test.ts pins them to the file). */
export const BACKEND_IN_FLIGHT = 16;
export const BACKEND_CALLS_PER_SECOND = 50;
export const BACKEND_CALL_BURST = 100;
export const BACKEND_EVENT_WINDOW = EVENT_BUFFER;
const RATE_LIMITED = "MOD-3001";

/** Simulated time and the timers set against it. */
export class FakeClock {
  now = 0;
  private next = 1;
  private readonly timers = new Map<number, { at: number; run: () => void }>();

  setTimer = (run: () => void, ms: number): unknown => {
    const id = this.next++;
    this.timers.set(id, { at: this.now + Math.max(0, ms), run });
    return id;
  };

  clearTimer = (handle: unknown): void => {
    this.timers.delete(handle as number);
  };

  /** Timers still waiting. */
  get waiting(): number {
    return this.timers.size;
  }

  /** Move time on by `ms`, running every timer that falls due, earliest first. */
  advance(ms: number): void {
    const end = this.now + ms;
    for (;;) {
      let due: [number, { at: number; run: () => void }] | null = null;
      for (const entry of this.timers) {
        if (entry[1].at <= end && (due === null || entry[1].at < due[1].at)) due = entry;
      }
      if (!due) break;
      this.timers.delete(due[0]);
      this.now = Math.max(this.now, due[1].at);
      due[1].run();
    }
    this.now = end;
  }
}

/** What PM does with a call: answer now or later. */
export type Handler = (method: string, args: unknown) => BackendReply | Promise<BackendReply>;

export interface FakePmStats {
  /** Calls that reached the handler. */
  calls: number;
  /** Calls the backend's limits refused with MOD-3001. */
  refused: number;
  /** The most calls the handler had out at once. */
  maxAdmitted: number;
  /** Simulated time each admitted call arrived. */
  arrivals: number[];
  /** The page's flood stop fired: the port was closed and the session ended. */
  stopped: boolean;
  /** Acknowledgements received, in order. */
  acks: number[];
  /** Heights asked for, in order. */
  heights: number[];
  /** Answers to `lifecycle.suspend` received (`{ flushed: true }`). */
  flushes: number;
  /** Events dropped because the frame had `BACKEND_EVENT_WINDOW` outstanding. */
  dropped: number;
  /** Messages the page could not read and dropped. */
  dropsRead: number;
}

export interface FakePm {
  env: FrameEnv;
  clock: FakeClock;
  /** What the environment was asked, in order: "listen", "post:connect". */
  log: string[];
  stats: FakePmStats;
  /** Errors a handler's exception was reported with. */
  reported: unknown[];
  /** PM tells the frame something: counted from 1 per delivered event, held until the port exists. */
  emit(name: ModEventName, payload?: unknown): void;
  /** Post `message` to the frame as it is, for what PM never sends (a malformed reply, a repeated
   *  event): the SDK reads everything from the port as untrusted. */
  forge(message: unknown): void;
  /** Wait for the messages in flight on the port, in both directions, to arrive. */
  settle(): Promise<void>;
  /** PM closes its end of the port (a pause or a stop) without a goodbye. */
  closePort(): void;
}

/** A fake PM answering the frame's calls with `handle`. Its `env.parent` is the object the frame's
 *  connect request goes to, and the `source` of the port PM posts back. */
export function fakePm(handle: Handler): FakePm {
  const clock = new FakeClock();
  const log: string[] = [];
  const reported: unknown[] = [];
  const stats: FakePmStats = {
    calls: 0,
    refused: 0,
    maxAdmitted: 0,
    arrivals: [],
    stopped: false,
    acks: [],
    heights: [],
    flushes: 0,
    dropped: 0,
    dropsRead: 0,
  };
  const listeners = new Set<
    (event: { source: unknown; data: unknown; ports: ReadonlyArray<MessagePort> }) => void
  >();
  let port: MessagePort | null = null;
  let connected = false;
  let pageInFlight = 0;
  let admitted = 0;
  const flood = new FloodGate(clock.now);
  // The backend's bucket, in thousandths of a call.
  let milli = BACKEND_CALL_BURST * 1000;
  let last = clock.now;
  let seq = 0;
  let ackedSeq = 0;
  let dropped = 0;

  const send = (message: unknown, transfer: Transferable[] = []) => {
    if (stats.stopped || !port) return;
    try {
      port.postMessage(message, transfer);
    } catch {
      // A closed port: the frame hears nothing, as it would.
    }
  };
  const pump = new EventPump((event) => send(event));

  const stop = () => {
    stats.stopped = true;
    port?.close();
    port = null;
  };

  const takeBackendToken = (): boolean => {
    const elapsed = Math.max(0, clock.now - last);
    last = Math.max(last, clock.now);
    milli = Math.min(BACKEND_CALL_BURST * 1000, milli + elapsed * BACKEND_CALLS_PER_SECOND);
    if (milli < 1000) return false;
    milli -= 1000;
    return true;
  };

  const answer = (id: number, backend: BackendReply) => {
    switch (backend.kind) {
      case "value":
        send(callResult(id, backend.value));
        return;
      case "error":
        send(callError(id, backend.error));
        return;
      case "binary":
        send(callResult(id, backend.vectors), [backend.buffer]);
        return;
      case "page":
        send(callResult(id, null));
        return;
    }
  };

  const onPortMessage = (data: unknown) => {
    if (stats.stopped) return;
    const message = readPortMessage(data);
    switch (message.kind) {
      case "resize":
        stats.heights.push(message.height);
        return;
      case "ack":
        if (!flood.take(clock.now)) return stop();
        stats.acks.push(message.seq);
        // The backend clamps an acknowledgement to what it sent.
        ackedSeq = Math.max(ackedSeq, Math.min(message.seq, seq));
        return;
      case "flushed":
        if (!flood.take(clock.now)) return stop();
        stats.flushes += 1;
        return;
      case "refuse":
        if (!flood.take(clock.now)) return stop();
        send(callError(message.id, message.error));
        return;
      case "drop":
        stats.dropsRead += 1;
        return;
      case "call": {
        // The page's line first (ModFrame.tsx), then the backend's.
        if (pageInFlight >= MAX_IN_FLIGHT || !flood.take(clock.now)) return stop();
        pageInFlight += 1;
        const done = () => {
          pageInFlight -= 1;
        };
        if (admitted >= BACKEND_IN_FLIGHT || !takeBackendToken()) {
          stats.refused += 1;
          send(
            callError(message.id, {
              code: RATE_LIMITED,
              message: "This mod is calling PM too fast.",
            }),
          );
          done();
          return;
        }
        admitted += 1;
        stats.calls += 1;
        stats.maxAdmitted = Math.max(stats.maxAdmitted, admitted);
        stats.arrivals.push(clock.now);
        void Promise.resolve(handle(message.method, JSON.parse(message.args) as unknown)).then(
          (backend) => {
            admitted -= 1;
            answer(message.id, backend);
            done();
          },
          (error: unknown) => {
            admitted -= 1;
            reported.push(error);
            done();
          },
        );
        return;
      }
    }
  };

  const parent = {
    postMessage(message: unknown, targetOrigin: string) {
      log.push(`post:${(message as { pm?: string }).pm ?? "?"}:${targetOrigin}`);
      if (!isConnectRequest(message) || connected) return;
      connected = true;
      const channel = new MessageChannel();
      port = channel.port1;
      port.onmessage = (event: MessageEvent) => onPortMessage(event.data);
      // Posted from a task of its own, as the platform does.
      queueMicrotask(() => {
        for (const listener of [...listeners]) {
          listener({ source: parent, data: PORT_MESSAGE, ports: [channel.port2] });
        }
        pump.connect();
      });
    },
  };

  const env: FrameEnv = {
    parent,
    listen(handler) {
      log.push("listen");
      listeners.add(handler);
      return () => {
        listeners.delete(handler);
      };
    },
    now: () => clock.now,
    setTimer: clock.setTimer,
    clearTimer: clock.clearTimer,
    report: (error) => {
      reported.push(error);
    },
  };

  return {
    env,
    clock,
    log,
    stats,
    reported,
    emit(name, payload = null) {
      // The backend's window: past it an event is dropped, counted, and the next one says so.
      if (seq - ackedSeq >= BACKEND_EVENT_WINDOW) {
        dropped += 1;
        stats.dropped += 1;
        return;
      }
      seq += 1;
      const raw: Record<string, unknown> = { name, seq, payload };
      if (dropped > 0) {
        raw.missed = dropped;
        dropped = 0;
      }
      pump.push(raw);
    },
    forge: (message) => send(message),
    async settle() {
      // A message crosses the port in a task of its own, and an answer to it in another: a few
      // turns of the event loop drain any chain the tests build.
      // A MessageChannel ping is one such turn, and (unlike a timer) does not wait on the clock.
      for (let i = 0; i < 6; i += 1) {
        await new Promise<void>((resolve) => {
          const ping = new MessageChannel();
          ping.port1.onmessage = () => {
            ping.port1.close();
            resolve();
          };
          ping.port2.postMessage(0);
        });
      }
    },
    closePort() {
      port?.close();
      port = null;
    },
  };
}
