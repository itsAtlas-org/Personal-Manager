// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The frame SDK (host.ts) against a fake PM (fakePm.ts) that is the page's real half of the bridge:
// the connect handshake, id-matched calls, the queue that keeps a mod under PM's flood stop and the
// backend's limits, cumulative acknowledgements, the binary answer, and the end of a session. Time is
// simulated, so the pace and the acknowledgement delay are exact.

import { describe, expect, it, vi } from "vitest";

import limitsRs from "../../src-tauri/src/mods/api/limits.rs?raw";
import {
  EVENT_BUFFER,
  FLOOD_BURST,
  FLOOD_CALLS_PER_SECOND,
  MAX_ARGS_BYTES,
  MAX_BINARY_BYTES,
  MAX_IN_FLIGHT as PAGE_MAX_IN_FLIGHT,
  MOD_ERRORS,
  PORT_MESSAGE,
  readVectors,
  type BackendReply,
} from "../components/mods/modBridge";
import {
  BACKEND_CALLS_PER_SECOND,
  BACKEND_CALL_BURST,
  BACKEND_EVENT_WINDOW,
  BACKEND_IN_FLIGHT,
  fakePm,
  type FakePm,
  type Handler,
} from "./fakePm";
import {
  ACK_AFTER,
  ACK_DELAY_MS,
  CONNECT_TIMEOUT_MS,
  HostError,
  MAX_CONCURRENT_CALLS,
  MAX_QUEUED_CALLS,
  ModCallError,
  PACE_BURST,
  PACE_PER_SECOND,
  connectWith,
  readDocumentVectors,
  windowEnv,
  type HostEvent,
  type ModHost,
  type WindowLike,
} from "./host";

const value = (v: unknown): BackendReply => ({ kind: "value", value: v });
const echo: Handler = (method, args) => value({ method, args });

/** A connected SDK and the fake PM it talks to. */
async function connected(handle: Handler = echo): Promise<{ pm: FakePm; host: ModHost }> {
  const pm = fakePm(handle);
  const pending = connectWith(pm.env);
  await pm.settle();
  return { pm, host: await pending };
}

/** Advance simulated time in steps, letting the port drain after each, until `until` holds. */
async function run(pm: FakePm, until: () => boolean, maxMs = 600_000, stepMs = 10) {
  for (let spent = 0; !until() && spent < maxMs; spent += stepMs) {
    pm.clock.advance(stepMs);
    await pm.settle();
  }
}

/** A promise settled from outside, for a handler that holds its answer. */
function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

/** The Rust constant `name` in limits.rs. */
function rustConst(name: string): number {
  const match = new RegExp(`pub const ${name}: \\w+ = ([0-9_]+);`).exec(limitsRs);
  if (!match) throw new Error(`limits.rs has no ${name}`);
  return Number(match[1].replace(/_/g, ""));
}

/** The vectors layout (embeddings.rs), built by hand like modBridge.test.ts does. */
function pack(dimension: number, docs: { id: number; vec: number[] }[]) {
  const count = docs.length;
  const buffer = new ArrayBuffer(16 + 8 * count + 4 * count * dimension);
  const view = new DataView(buffer);
  view.setUint32(0, 1, true);
  view.setUint32(4, count, true);
  view.setUint32(8, dimension, true);
  docs.forEach((doc, i) => {
    view.setFloat64(16 + 8 * i, doc.id, true);
    doc.vec.forEach((x, j) => view.setFloat32(16 + 8 * count + 4 * (i * dimension + j), x, true));
  });
  return buffer;
}

describe("the numbers the SDK keeps to", () => {
  it("sit under the backend's limits and the page's flood line, and are the ones in those files", () => {
    // The fake restates the backend; the file is the authority.
    expect(BACKEND_IN_FLIGHT).toBe(rustConst("MAX_IN_FLIGHT"));
    expect(BACKEND_CALLS_PER_SECOND).toBe(rustConst("CALLS_PER_SECOND"));
    expect(BACKEND_CALL_BURST).toBe(rustConst("CALL_BURST"));
    expect(BACKEND_EVENT_WINDOW).toBe(rustConst("EVENT_WINDOW"));
    expect(EVENT_BUFFER).toBe(rustConst("EVENT_WINDOW"));
    // Calls out at once: no more than the backend takes, so MOD-3001 for concurrency never comes,
    // and well under the page's 32.
    expect(MAX_CONCURRENT_CALLS).toBeLessThanOrEqual(rustConst("MAX_IN_FLIGHT"));
    expect(MAX_CONCURRENT_CALLS).toBeLessThan(PAGE_MAX_IN_FLIGHT);
    // The pace: under the backend's bucket, and with room under the page's for acknowledgements.
    expect(PACE_PER_SECOND).toBeLessThan(rustConst("CALLS_PER_SECOND"));
    expect(PACE_BURST).toBeLessThan(rustConst("CALL_BURST"));
    expect(PACE_PER_SECOND).toBeLessThan(FLOOD_CALLS_PER_SECOND);
    expect(PACE_BURST).toBeLessThan(FLOOD_BURST);
    // A batch of events is acknowledged long before the backend's window fills.
    expect(ACK_AFTER * 2).toBeLessThanOrEqual(rustConst("EVENT_WINDOW"));
  });
});

describe("connecting", () => {
  it("listens before it asks, asks exactly as the page reads it, and takes the one port", async () => {
    const pm = fakePm(echo);
    const pending = connectWith(pm.env);
    await pm.settle();
    const host = await pending;
    // The listener is in place before PM can answer; the request goes to "*" (an opaque origin
    // can't be named).
    expect(pm.log).toEqual(["listen", "post:connect:*"]);
    await expect(host.call("theme.get")).resolves.toEqual({ method: "theme.get", args: null });
  });

  it("takes a port only from the parent, only the versioned message, only with exactly one port", async () => {
    const listeners: Array<(e: { source: unknown; data: unknown; ports: MessagePort[] }) => void> =
      [];
    const parent = { postMessage: vi.fn() };
    const clock = fakePm(echo).clock;
    const pending = connectWith({
      parent,
      listen: (h) => {
        listeners.push(h);
        return () => {};
      },
      now: () => clock.now,
      setTimer: clock.setTimer,
      clearTimer: clock.clearTimer,
      report: () => {},
    });
    const settled = vi.fn();
    void pending.then(settled, settled);
    const { port1, port2 } = new MessageChannel();
    const hear = listeners[0];
    hear({ source: {}, data: PORT_MESSAGE, ports: [port2] }); // not the parent
    hear({ source: parent, data: { pm: "port", version: 2 }, ports: [port2] });
    hear({ source: parent, data: { pm: "connect", version: 1 }, ports: [port2] });
    hear({ source: parent, data: null, ports: [port2] });
    hear({ source: parent, data: PORT_MESSAGE, ports: [] });
    await Promise.resolve();
    expect(settled).not.toHaveBeenCalled();
    hear({ source: parent, data: PORT_MESSAGE, ports: [port2] });
    const host = await pending;
    expect(host.closed).toBeNull();
    port1.close();
    port2.close();
  });

  it("stops listening once it has the port, so a later port message is not taken", async () => {
    const pm = fakePm(echo);
    const removed = vi.fn();
    const env = {
      ...pm.env,
      listen: (h: Parameters<typeof pm.env.listen>[0]) => {
        const stop = pm.env.listen(h);
        return () => {
          removed();
          stop();
        };
      },
    };
    const pending = connectWith(env);
    await pm.settle();
    await pending;
    expect(removed).toHaveBeenCalledTimes(1);
    expect(pm.clock.waiting).toBe(0); // the connect timeout was cleared too
  });

  it("refuses a page that is not in a frame", async () => {
    const pm = fakePm(echo);
    await expect(connectWith({ ...pm.env, parent: null })).rejects.toMatchObject({
      name: "HostError",
      kind: "not-framed",
    });
  });

  it("gives up when PM never answers, and stops listening", async () => {
    const pm = fakePm(echo);
    const stopped = vi.fn();
    // A parent that swallows the request.
    const env = {
      ...pm.env,
      parent: { postMessage: () => {} },
      listen: (h: Parameters<typeof pm.env.listen>[0]) => {
        const stop = pm.env.listen(h);
        return () => {
          stopped();
          stop();
        };
      },
    };
    const settled = vi.fn();
    const pending = connectWith(env).catch((e: unknown) => {
      settled();
      throw e;
    });
    const assertion = expect(pending).rejects.toMatchObject({ kind: "no-port" });
    pm.clock.advance(CONNECT_TIMEOUT_MS - 1);
    await Promise.resolve();
    expect(settled).not.toHaveBeenCalled();
    pm.clock.advance(1);
    await assertion;
    expect(stopped).toHaveBeenCalledTimes(1);
  });

  it("answers a failed post as no port", async () => {
    const pm = fakePm(echo);
    const env = {
      ...pm.env,
      parent: {
        postMessage: () => {
          throw new Error("closed");
        },
      },
    };
    await expect(connectWith(env)).rejects.toMatchObject({ kind: "no-port" });
    expect(pm.clock.waiting).toBe(0);
  });
});

describe("a call", () => {
  it("resolves with PM's result, numbering calls from 1 in the order they are sent", async () => {
    const seen: unknown[] = [];
    const { pm, host } = await connected((method, args) => {
      seen.push([method, args]);
      return value({ n: seen.length });
    });
    const a = host.call("storage.get", { key: "settings" });
    const b = host.call("documents.list", { after: 5 });
    await pm.settle();
    await expect(a).resolves.toEqual({ n: 1 });
    await expect(b).resolves.toEqual({ n: 2 });
    expect(seen).toEqual([
      ["storage.get", { key: "settings" }],
      ["documents.list", { after: 5 }],
    ]);
  });

  it("sends exactly the JSON it measured: what JSON drops is dropped, not an error", async () => {
    const { pm, host } = await connected();
    // A function or an undefined is dropped by JSON; structured clone would throw on the function.
    const result = host.call("storage.set", { a: 1, f: () => 1, u: undefined, d: new Date(0) });
    await pm.settle();
    await expect(result).resolves.toEqual({
      method: "storage.set",
      args: { a: 1, d: "1970-01-01T00:00:00.000Z" },
    });
    // No argument at all is null, as the page reads it.
    const bare = host.call("theme.get");
    await pm.settle();
    await expect(bare).resolves.toEqual({ method: "theme.get", args: null });
  });

  it("matches answers to calls by id, whatever order they come in, and ignores strays", async () => {
    const gates = new Map<string, ReturnType<typeof deferred<BackendReply>>>();
    const { pm, host } = await connected((method) => {
      const gate = deferred<BackendReply>();
      gates.set(method, gate);
      return gate.promise;
    });
    const slow = host.call("a.slow");
    const fast = host.call("a.fast");
    await pm.settle();
    gates.get("a.fast")?.resolve(value("fast"));
    await pm.settle();
    await expect(fast).resolves.toBe("fast");
    gates.get("a.slow")?.resolve(value("slow"));
    await pm.settle();
    await expect(slow).resolves.toBe("slow");
    expect(host.closed).toBeNull();
  });

  it("rejects with PM's refusal: its code, its message, and the permission or revision it names", async () => {
    const { pm, host } = await connected((method) => {
      if (method === "a.permission") {
        return {
          kind: "error",
          error: { code: "MOD-2002", message: "Asking first.", permission: "documents.meta" },
        };
      }
      return {
        kind: "error",
        error: { code: "MOD-4001", message: "Stale.", revision: 7 },
      };
    });
    // Caught as they are made: a rejection nobody is yet waiting on is an unhandled one.
    const permission = host.call("a.permission").catch((e: unknown) => e);
    const stale = host.call("a.stale").catch((e: unknown) => e);
    await pm.settle();
    const first = await permission;
    expect(first).toBeInstanceOf(ModCallError);
    expect(first).toMatchObject({
      name: "ModCallError",
      code: MOD_ERRORS.needsPrompt,
      message: "Asking first.",
      permission: "documents.meta",
    });
    expect(await stale).toMatchObject({
      code: "MOD-4001",
      revision: 7,
    });
  });

  it("treats a reply that isn't in the shape as PM's own failure, for that call alone", async () => {
    const { pm, host } = await connected(() => new Promise<BackendReply>(() => {}));
    // Ids are 1, 2, 3, ... in the order sent; PM's answers are forged by hand.
    const calls = [1, 2, 3, 4].map(() => host.call("a.held").catch((e: unknown) => e));
    await pm.settle();
    pm.forge({ id: 1, ok: false, error: { code: "not a code", message: "x" } });
    pm.forge({ id: 2, ok: false });
    pm.forge({ id: 3, ok: false, error: { code: "MOD-1001", message: 5 } });
    pm.forge({ id: 4, ok: "yes" });
    pm.forge({ id: 99, ok: true, result: "a stray" });
    pm.forge("junk");
    pm.forge(null);
    pm.forge({ id: "1", ok: true, result: "a string id" });
    await pm.settle();
    // Each is PM failing (an answer that says neither yes nor no is not guessed at), and the
    // strays and the junk touched nothing.
    for (const r of (await Promise.all(calls)) as unknown[]) {
      expect(r).toBeInstanceOf(ModCallError);
      expect(r).toMatchObject({ code: MOD_ERRORS.internal });
    }
    expect(host.closed).toBeNull();
  });

  it("settles a call once: a second answer to it is ignored", async () => {
    const { pm, host } = await connected(() => new Promise<BackendReply>(() => {}));
    const call = host.call("a.held");
    await pm.settle();
    pm.forge({ id: 1, ok: true, result: "first" });
    pm.forge({ id: 1, ok: true, result: "second" });
    pm.forge({ id: 1, ok: false, error: { code: "MOD-9001", message: "late" } });
    await pm.settle();
    await expect(call).resolves.toBe("first");
  });

  it("refuses a bad method name and bad arguments itself, spending nothing and sending nothing", async () => {
    const { pm, host } = await connected();
    const bad = await Promise.all(
      [host.call("not a method"), host.call("1abc"), host.call(""), host.call("a".repeat(129))].map(
        (p) => p.catch((e: unknown) => e),
      ),
    );
    for (const e of bad) expect(e).toMatchObject({ code: MOD_ERRORS.badArgs });
    const cyclic: Record<string, unknown> = {};
    cyclic.self = cyclic;
    const unreadable = [
      await host.call("a.b", cyclic).catch((e: unknown) => e),
      await host.call("a.b", { n: 1n }).catch((e: unknown) => e),
    ];
    for (const e of unreadable) expect(e).toMatchObject({ code: MOD_ERRORS.badArgs });
    const big = await host.call("a.b", "x".repeat(MAX_ARGS_BYTES)).catch((e: unknown) => e);
    expect(big).toMatchObject({ code: MOD_ERRORS.tooLarge });
    await pm.settle();
    // None of it reached PM, and the whole burst of tokens is still there.
    expect(pm.stats.calls + pm.stats.refused).toBe(0);
    expect(pm.stats.dropsRead).toBe(0);
    const burst = Array.from({ length: PACE_BURST }, () => host.call("a.ok"));
    await pm.settle();
    await Promise.all(burst);
    expect(pm.stats.arrivals.every((at) => at === 0)).toBe(true);
  });
});

describe("the queue in front of the port", () => {
  it("sends a thousand calls without PM's flood stop, the backend's limits or a faster pace than it keeps", async () => {
    const { pm, host } = await connected();
    let done = 0;
    const calls = Array.from({ length: 1000 }, (_, i) =>
      host.call("storage.get", { key: `k${i}` }).then(() => {
        done += 1;
      }),
    );
    await pm.settle();
    await run(pm, () => done === 1000);
    await Promise.all(calls);
    expect(pm.stats.stopped).toBe(false);
    expect(pm.stats.refused).toBe(0);
    expect(pm.stats.calls).toBe(1000);
    // Call k (from 1) can have arrived no earlier than its token: the burst, then 40 a second.
    pm.stats.arrivals.forEach((at, i) => {
      const earliest = ((i + 1 - PACE_BURST) / PACE_PER_SECOND) * 1000;
      expect(at, `call ${i + 1}`).toBeGreaterThanOrEqual(earliest - 1);
    });
    // And the burst really was a burst: the first 80 went at once.
    expect(pm.stats.arrivals.slice(0, PACE_BURST).every((at) => at === 0)).toBe(true);
  });

  it("holds at most 16 calls out at once, and counts one PM holds for a question as out", async () => {
    const gates: Array<(r: BackendReply) => void> = [];
    const { pm, host } = await connected(
      () =>
        new Promise<BackendReply>((resolve) => {
          gates.push(resolve);
        }),
    );
    let done = 0;
    const calls = Array.from({ length: 100 }, () =>
      host.call("a.held").then(() => {
        done += 1;
      }),
    );
    await pm.settle();
    expect(pm.stats.calls).toBe(MAX_CONCURRENT_CALLS);
    // Answering one lets exactly one more go.
    gates[0](value(null));
    await pm.settle();
    expect(pm.stats.calls).toBe(MAX_CONCURRENT_CALLS + 1);
    // The rest, as they are answered.
    let released = 1;
    while (done < 100) {
      while (released < gates.length) gates[released++](value(null));
      await pm.settle();
      pm.clock.advance(100);
      await pm.settle();
    }
    await Promise.all(calls);
    expect(pm.stats.maxAdmitted).toBeLessThanOrEqual(MAX_CONCURRENT_CALLS);
    expect(pm.stats.stopped).toBe(false);
    expect(pm.stats.refused).toBe(0);
  });

  it("refuses a call past the queue's bound, and still sends every one it took", async () => {
    const gate = deferred<BackendReply>();
    const { pm, host } = await connected(() => gate.promise);
    // 16 go out, the rest wait; the bound counts the waiting.
    const taken = Array.from({ length: MAX_CONCURRENT_CALLS + MAX_QUEUED_CALLS }, () =>
      host.call("a.held"),
    );
    await pm.settle();
    const over = await host.call("a.held").catch((e: unknown) => e);
    expect(over).toBeInstanceOf(HostError);
    expect(over).toMatchObject({ kind: "queue-full" });
    gate.resolve(value("ok"));
    await run(pm, () => pm.stats.calls === taken.length);
    await expect(Promise.all(taken)).resolves.toHaveLength(taken.length);
  });

  it("does not trip the flood stop under a storm of events and calls together", async () => {
    // Acknowledgements draw on the same bucket the page counts calls against: a call every pace
    // tick plus an acknowledgement every delay is more than the page's 50 a second if the two are
    // not one budget. Sixty simulated seconds of both.
    const { pm, host } = await connected();
    host.onEvent(() => {});
    let done = 0;
    const calls = Array.from({ length: 1000 }, () =>
      host.call("a.b").then(() => {
        done += 1;
      }),
    );
    // An event every 20 ms, so an acknowledgement every 50: 20 a second beside the calls.
    for (let ms = 0; ms < 40_000; ms += 20) {
      pm.emit("documents.added", { ids: [ms] });
      pm.clock.advance(20);
      await pm.settle();
    }
    expect(pm.stats.stopped).toBe(false);
    expect(pm.stats.refused).toBe(0);
    expect(pm.stats.dropped).toBe(0);
    expect(pm.stats.acks.length).toBeGreaterThan(500);
    await run(pm, () => done === 1000);
    await Promise.all(calls);
    expect(pm.stats.stopped).toBe(false);
  });
});

describe("events", () => {
  const added = (n: number) => ({ ids: [n] });

  it("are told to handlers in order, with what was missed, and one handler throwing stops nobody", async () => {
    const { pm, host } = await connected();
    const heard: HostEvent[] = [];
    host.onEvent(() => {
      throw new Error("a handler's bug");
    });
    host.onEvent((e) => heard.push(e));
    pm.emit("documents.added", added(1));
    pm.emit("theme.changed", { vars: {} });
    await pm.settle();
    expect(heard.map((e) => [e.name, e.seq, e.payload])).toEqual([
      ["documents.added", 1, added(1)],
      ["theme.changed", 2, { vars: {} }],
    ]);
    expect(heard[0].missed).toBeUndefined();
    expect(pm.reported).toHaveLength(2);
    expect(pm.reported[0]).toEqual(new Error("a handler's bug"));
    // The throwing handler did not stop the acknowledgement either.
    pm.clock.advance(ACK_DELAY_MS);
    await pm.settle();
    expect(pm.stats.acks).toEqual([2]);
  });

  it("reach the first handler even when it is added after the connection has been used", async () => {
    // PM starts sending the moment it connects; a mod's first await after connecting may take
    // several turns, and what arrived meanwhile is not lost.
    const pm = fakePm(echo);
    pm.emit("theme.changed", { vars: { a: "1" } });
    pm.emit("documents.added", added(1));
    const pending = connectWith(pm.env);
    await pm.settle();
    const host = await pending;
    await host.call("a.b");
    await pm.settle();
    const heard: HostEvent[] = [];
    host.onEvent((e) => heard.push(e));
    expect(heard.map((e) => e.name)).toEqual(["theme.changed", "documents.added"]);
    pm.emit("documents.added", added(2));
    await pm.settle();
    expect(heard.map((e) => e.seq)).toEqual([1, 2, 3]);
  });

  it("report how many were dropped before them", async () => {
    const { pm, host } = await connected();
    const heard: HostEvent[] = [];
    host.onEvent((e) => heard.push(e));
    // All at once, before the frame can have acknowledged any: the window fills and two are lost.
    for (let i = 0; i < BACKEND_EVENT_WINDOW; i += 1) pm.emit("documents.added", added(i));
    pm.emit("documents.added", added(-1));
    pm.emit("documents.added", added(-2));
    await pm.settle();
    expect(pm.stats.dropped).toBe(2);
    // The frame acknowledged as the batch came in (every ACK_AFTER), which frees the window, and
    // the next event says what was lost.
    expect(pm.stats.acks[pm.stats.acks.length - 1]).toBe(BACKEND_EVENT_WINDOW);
    pm.emit("documents.deleted", { ids: [9] });
    await pm.settle();
    expect(heard[heard.length - 1]).toMatchObject({ name: "documents.deleted", missed: 2 });
  });

  it("are acknowledged cumulatively, once per batch, and never past what was received", async () => {
    const { pm, host } = await connected();
    host.onEvent(() => {});
    for (let i = 0; i < 10; i += 1) pm.emit("documents.added", added(i));
    await pm.settle();
    // Nothing yet: an acknowledgement waits for a quiet, since each costs a token.
    expect(pm.stats.acks).toEqual([]);
    pm.clock.advance(ACK_DELAY_MS - 1);
    await pm.settle();
    expect(pm.stats.acks).toEqual([]);
    pm.clock.advance(1);
    await pm.settle();
    expect(pm.stats.acks).toEqual([10]);
    // Nothing new, nothing sent.
    pm.clock.advance(10 * ACK_DELAY_MS);
    await pm.settle();
    expect(pm.stats.acks).toEqual([10]);
    // The next batch starts its own wait.
    pm.emit("documents.added", added(11));
    pm.emit("documents.added", added(12));
    await pm.settle();
    pm.clock.advance(ACK_DELAY_MS);
    await pm.settle();
    expect(pm.stats.acks).toEqual([10, 12]);
  });

  it("are acknowledged without waiting once ACK_AFTER have piled up", async () => {
    const { pm, host } = await connected();
    host.onEvent(() => {});
    for (let i = 0; i < ACK_AFTER; i += 1) pm.emit("documents.added", added(i));
    await pm.settle();
    expect(pm.stats.acks).toEqual([ACK_AFTER]);
    expect(pm.clock.waiting).toBe(0);
  });

  it("are acknowledged even before a handler exists", async () => {
    // The backend counts an event outstanding until it is acknowledged, handler or not.
    const { pm } = await connected();
    pm.emit("documents.added", added(1));
    await pm.settle();
    pm.clock.advance(ACK_DELAY_MS);
    await pm.settle();
    expect(pm.stats.acks).toEqual([1]);
  });

  it("are acknowledged when their name is one this SDK doesn't know, and told to no one", async () => {
    const { pm, host } = await connected();
    const heard: string[] = [];
    host.onEvent((e) => heard.push(e.name));
    pm.forge({ event: "from.the.future", seq: 1, payload: null });
    await pm.settle();
    pm.clock.advance(ACK_DELAY_MS);
    await pm.settle();
    expect(heard).toEqual([]);
    expect(pm.stats.acks).toEqual([1]);
  });

  it("are told once: a repeated or backwards sequence number is dropped, and so is a malformed one", async () => {
    const { pm, host } = await connected();
    const heard: Array<[number, unknown]> = [];
    host.onEvent((e) => heard.push([e.seq, e.payload]));
    const ev = (seq: unknown, payload: unknown = null) =>
      pm.forge({ event: "documents.added", seq, payload });
    ev(1, "a");
    ev(1, "again");
    ev(3, "c");
    ev(2, "late");
    ev(0);
    ev(-1);
    ev(1.5);
    ev("4");
    ev(Number.MAX_SAFE_INTEGER + 1);
    pm.forge({ seq: 9 }); // no name
    await pm.settle();
    expect(heard).toEqual([
      [1, "a"],
      [3, "c"],
    ]);
    pm.clock.advance(ACK_DELAY_MS);
    await pm.settle();
    // Only what was taken is acknowledged, and never past it.
    expect(pm.stats.acks).toEqual([3]);
  });
});

describe("the end of a session", () => {
  async function waiting() {
    const gate = deferred<BackendReply>();
    const { pm, host } = await connected(() => gate.promise);
    // 16 out, 4 queued: both kinds must be rejected.
    const calls = Array.from({ length: MAX_CONCURRENT_CALLS + 4 }, () =>
      host.call("a.held").catch((e: unknown) => e),
    );
    await pm.settle();
    return { pm, host, calls, gate };
  }

  it("session.closed is told to the handlers first, then every waiting and later call is rejected", async () => {
    const { pm, host, calls } = await waiting();
    const order: string[] = [];
    host.onEvent((e) => {
      order.push(`${e.name}:${String(host.closed)}`);
    });
    pm.emit("session.closed", { reason: "uninstalled" });
    await pm.settle();
    // The goodbye was heard before the host counted itself gone.
    expect(order).toEqual(["session.closed:null"]);
    const results = await Promise.all(calls);
    for (const r of results) {
      expect(r).toBeInstanceOf(HostError);
      expect(r).toMatchObject({ kind: "closed" });
    }
    expect(host.closed).toMatchObject({ kind: "closed" });
    expect(host.closed?.message).toContain("uninstalled");
    await expect(host.call("a.b")).rejects.toBe(host.closed);
    // Nothing more is acknowledged or asked of a session that is over.
    pm.clock.advance(10 * ACK_DELAY_MS);
    await pm.settle();
    expect(pm.stats.acks).toEqual([]);
    expect(() => host.resize(500)).not.toThrow();
    await pm.settle();
    expect(pm.stats.heights).toEqual([]);
  });

  it("mod.paused ends it the same way, as paused", async () => {
    const { pm, host, calls } = await waiting();
    pm.emit("mod.paused", { reason: "blacklisted" });
    await pm.settle();
    for (const r of await Promise.all(calls)) expect(r).toMatchObject({ kind: "paused" });
    await expect(host.call("a.b")).rejects.toMatchObject({ kind: "paused" });
  });

  it("holds the goodbye for a first handler added after it", async () => {
    const pm = fakePm(echo);
    const pending = connectWith(pm.env);
    await pm.settle();
    const host = await pending;
    pm.emit("session.closed", { reason: "paused" });
    await pm.settle();
    expect(host.closed).not.toBeNull();
    const heard: string[] = [];
    host.onEvent((e) => heard.push(e.name));
    expect(heard).toEqual(["session.closed"]);
  });

  it("answers a port that throws as a host that is gone", async () => {
    const { pm, host } = await connected(() => new Promise<BackendReply>(() => {}));
    const held = host.call("a.held");
    await pm.settle();
    // PM's end of the port is closed; the frame's postMessage on a closed port does not throw in
    // the platform, so the failure is simulated on the SDK's own port by closing it under it.
    const failing = host as unknown as { port: MessagePort };
    failing.port.postMessage = () => {
      throw new Error("detached");
    };
    const next = host.call("a.next");
    await expect(next).rejects.toMatchObject({ kind: "closed" });
    await expect(held).rejects.toMatchObject({ kind: "closed" });
  });
});

describe("the vectors answer", () => {
  const docs = [
    { id: 11, vec: [0.5, -1, 2] },
    { id: 7, vec: [3, 4, 5] },
  ];
  const vectorsHandler: Handler = (method, args) => {
    if (method !== "embeddings.documentVectors") return value(null);
    const buffer = pack(3, docs);
    const vectors = readVectors(buffer);
    if (!vectors) throw new Error("fixture");
    return args !== null ? { kind: "binary", vectors, buffer } : value(null);
  };

  it("arrives as typed views over one buffer, ids and floats in the order sent", async () => {
    const { pm, host } = await connected(vectorsHandler);
    const answer = host.documentVectors({ cap: 2 });
    await pm.settle();
    const got = await answer;
    expect(got.count).toBe(2);
    expect(got.dimension).toBe(3);
    expect(got.ids).toBeInstanceOf(Float64Array);
    expect(got.vectors).toBeInstanceOf(Float32Array);
    expect([...got.ids]).toEqual([11, 7]);
    expect([...got.vectors]).toEqual([0.5, -1, 2, 3, 4, 5]);
    // Views over the one buffer that crossed, not copies of it.
    expect(got.ids.buffer).toBe(got.vectors.buffer);
  });

  it("is asked for with the ids and cap given, or with none", async () => {
    const seen: unknown[] = [];
    const { pm, host } = await connected((method, args) => {
      seen.push(args);
      return vectorsHandler(method, args);
    });
    const a = host.documentVectors({ ids: [1, 2], cap: 9 });
    const b = host.documentVectors();
    await pm.settle();
    await Promise.all([a, b]);
    expect(seen).toEqual([{ ids: [1, 2], cap: 9 }, {}]);
  });

  it("is read from a raw buffer in the bridge's layout too", () => {
    const got = readDocumentVectors(pack(2, [{ id: 5, vec: [1, 2] }]));
    expect([...got.ids]).toEqual([5]);
    expect([...got.vectors]).toEqual([1, 2]);
    expect(MAX_BINARY_BYTES).toBeGreaterThan(0);
  });

  it.each([
    ["nothing", null],
    ["a string", "vectors"],
    [
      "a count that isn't a number",
      { count: "2", dimension: 1, ids: new Float64Array(2), vectors: new Float32Array(2) },
    ],
    [
      "a fractional count",
      { count: 1.5, dimension: 1, ids: new Float64Array(2), vectors: new Float32Array(2) },
    ],
    [
      "a negative dimension",
      { count: 0, dimension: -1, ids: new Float64Array(0), vectors: new Float32Array(0) },
    ],
    [
      "ids of the wrong length",
      { count: 2, dimension: 1, ids: new Float64Array(1), vectors: new Float32Array(2) },
    ],
    [
      "vectors of the wrong length",
      { count: 2, dimension: 2, ids: new Float64Array(2), vectors: new Float32Array(3) },
    ],
    [
      "ids that aren't f64",
      { count: 1, dimension: 1, ids: new Float32Array(1), vectors: new Float32Array(1) },
    ],
    [
      "vectors that aren't f32",
      { count: 1, dimension: 1, ids: new Float64Array(1), vectors: new Float64Array(1) },
    ],
    ["plain arrays", { count: 1, dimension: 1, ids: [1], vectors: [1] }],
    ["a buffer that is not the layout", new ArrayBuffer(5)],
  ])("refuses %s as PM's own failure, never a half-read answer", (_name, answer) => {
    try {
      readDocumentVectors(answer);
      expect.unreachable();
    } catch (e) {
      expect(e).toBeInstanceOf(ModCallError);
      expect(e).toMatchObject({ code: MOD_ERRORS.internal });
    }
  });

  it("reads an empty answer", () => {
    const got = readDocumentVectors({
      count: 0,
      dimension: 384,
      ids: new Float64Array(0),
      vectors: new Float32Array(0),
    });
    expect(got.count).toBe(0);
  });
});

describe("the frame's height", () => {
  it("is asked for as a clamped whole number, and only when it changes", async () => {
    const { pm, host } = await connected();
    host.resize(300.4);
    host.resize(300);
    host.resize(1e9);
    host.resize(1);
    host.resize(1);
    await pm.settle();
    expect(pm.stats.heights).toEqual([300, 4096, 64]);
  });

  it("refuses what is not a number of pixels", async () => {
    const { host } = await connected();
    expect(() => host.resize("300" as unknown as number)).toThrow(TypeError);
    expect(() => host.resize(Number.NaN)).toThrow(RangeError);
    expect(() => host.resize(Number.POSITIVE_INFINITY)).toThrow(RangeError);
  });
});

describe("the real window", () => {
  function windowWith(parent: WindowLike["parent"] | "self") {
    const listeners = new Set<(e: MessageEvent) => void>();
    const win: WindowLike = {
      parent: undefined as unknown as WindowLike["parent"],
      addEventListener: (_t, l) => {
        listeners.add(l);
      },
      removeEventListener: (_t, l) => {
        listeners.delete(l);
      },
    };
    win.parent = parent === "self" ? (win as unknown as WindowLike["parent"]) : parent;
    return { win, listeners };
  }

  it("is not framed when its parent is itself, and framed when it is not", () => {
    const own = windowWith("self");
    expect(windowEnv(own.win).parent).toBeNull();
    const parent = { postMessage: vi.fn() };
    expect(windowEnv(windowWith(parent).win).parent).toBe(parent);
  });

  it("hears the window's message events until told to stop", () => {
    const { win, listeners } = windowWith({ postMessage: vi.fn() });
    const env = windowEnv(win);
    const heard: unknown[] = [];
    const stop = env.listen((e) => heard.push(e.data));
    expect(listeners.size).toBe(1);
    for (const l of listeners) l({ data: "x" } as MessageEvent);
    expect(heard).toEqual(["x"]);
    stop();
    expect(listeners.size).toBe(0);
  });

  it("rethrows a handler's error from a microtask of its own, so the page's error reporting sees it", () => {
    const env = windowEnv(windowWith({ postMessage: vi.fn() }).win);
    const queued: Array<() => void> = [];
    vi.stubGlobal("queueMicrotask", (run: () => void) => queued.push(run));
    try {
      env.report(new Error("from a handler"));
    } finally {
      vi.unstubAllGlobals();
    }
    // Not thrown into the SDK's own call stack...
    expect(queued).toHaveLength(1);
    // ...but out of the microtask it queued.
    expect(queued[0]).toThrow("from a handler");
  });

  it("keeps time and timers with the platform", async () => {
    const env = windowEnv(windowWith({ postMessage: vi.fn() }).win);
    const before = env.now();
    const ran = vi.fn();
    env.clearTimer(env.setTimer(() => ran("cleared"), 0));
    env.setTimer(() => ran("kept"), 0);
    await new Promise((r) => setTimeout(r, 10));
    expect(ran.mock.calls).toEqual([["kept"]]);
    expect(env.now()).toBeGreaterThan(before);
  });
});

describe("connectHost", () => {
  it("asks once for the page's port: a second call is the first's promise", async () => {
    vi.resetModules();
    const pm = fakePm(echo);
    const win = {
      parent: pm.env.parent,
      addEventListener: (_t: string, l: (e: MessageEvent) => void) => {
        pm.env.listen((e) => l(e as unknown as MessageEvent));
      },
      removeEventListener: () => {},
    };
    vi.stubGlobal("window", win);
    try {
      const { connectHost } = await import("./host");
      const first = connectHost();
      const second = connectHost();
      expect(second).toBe(first);
      await pm.settle();
      await first;
      expect(pm.log.filter((l) => l.startsWith("post:"))).toEqual(["post:connect:*"]);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("is not framed with no window at all", async () => {
    vi.resetModules();
    const { connectHost } = await import("./host");
    await expect(connectHost()).rejects.toMatchObject({ kind: "not-framed" });
  });
});
