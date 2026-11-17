// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The bridge's reading rules (modBridge.ts): what a frame's messages may be, every refusal and its
// code, how the backend's answers are read, and the flood gate.

import { describe, expect, it } from "vitest";

import {
  EVENT_BUFFER,
  EventPump,
  FLOOD_BURST,
  FLOOD_CALLS_PER_SECOND,
  FloodGate,
  INTERNAL_MESSAGE,
  MAX_ARGS_BYTES,
  MAX_BINARY_BYTES,
  MAX_FRAME_HEIGHT,
  MIN_FRAME_HEIGHT,
  MOD_ERRORS,
  MOD_EVENTS,
  callError,
  clampFrameHeight,
  encodeArgs,
  errorText,
  fitsBytes,
  isArrayBuffer,
  isConnectRequest,
  isMethodName,
  readEvent,
  readPortMessage,
  readReply,
  readVectors,
} from "./modBridge";

/** The vectors layout (embeddings.rs): a 16-byte header of little-endian u32s (version, count,
 *  dimension, reserved), the ids as f64, then the floats as f32. Built by hand, byte by byte, so the
 *  page is held to the layout and not to its own helper. */
function pack(dimension: number, docs: { id: number; vec: number[] }[], header = [1, 0, 0]) {
  const count = docs.length;
  const buffer = new ArrayBuffer(16 + 8 * count + 4 * count * dimension);
  const view = new DataView(buffer);
  const [version, , reserved] = header;
  view.setUint32(0, version, true);
  view.setUint32(4, count, true);
  view.setUint32(8, dimension, true);
  view.setUint32(12, reserved, true);
  docs.forEach((doc, i) => {
    view.setFloat64(16 + 8 * i, doc.id, true);
    doc.vec.forEach((x, j) => view.setFloat32(16 + 8 * count + 4 * (i * dimension + j), x, true));
  });
  return buffer;
}

describe("the connect request", () => {
  it("is exactly the versioned shape", () => {
    expect(isConnectRequest({ pm: "connect", version: 1 })).toBe(true);
    expect(isConnectRequest({ pm: "connect", version: 1, modId: "atlas.map" })).toBe(true);
  });

  it.each([
    null,
    undefined,
    "connect",
    ["connect"],
    { pm: "connect" },
    { pm: "connect", version: "1" },
    { pm: "connect", version: 2 },
    { pm: "port", version: 1 },
    { type: "connect", version: 1 },
  ])("refuses %j", (data) => {
    expect(isConnectRequest(data)).toBe(false);
  });
});

describe("a height request", () => {
  it("is clamped and rounded", () => {
    expect(clampFrameHeight(300)).toBe(300);
    expect(clampFrameHeight(300.6)).toBe(301);
    expect(clampFrameHeight(0)).toBe(MIN_FRAME_HEIGHT);
    expect(clampFrameHeight(-1e9)).toBe(MIN_FRAME_HEIGHT);
    expect(clampFrameHeight(1e9)).toBe(MAX_FRAME_HEIGHT);
    expect(readPortMessage({ resize: 1e9 })).toEqual({ kind: "resize", height: MAX_FRAME_HEIGHT });
  });

  it.each(["300", "300px", null, undefined, Number.NaN, Infinity, -Infinity, {}, [300], true])(
    "drops %j",
    (value) => {
      expect(clampFrameHeight(value)).toBeNull();
      expect(readPortMessage({ resize: value })).toEqual({ kind: "drop" });
    },
  );
});

describe("a call", () => {
  it("is read with its id, method and arguments as JSON text, and nothing else", () => {
    expect(
      readPortMessage({
        id: 0,
        method: "storage.get",
        args: { key: "k" },
        session: "s",
        modId: "m",
      }),
    ).toEqual({ kind: "call", id: 0, method: "storage.get", args: '{"key":"k"}' });
    expect(readPortMessage({ id: 5, method: "a" })).toEqual({
      kind: "call",
      id: 5,
      method: "a",
      args: "null",
    });
  });

  it.each([
    null,
    "a string",
    42,
    [1, "storage.get"],
    { method: "storage.get" },
    { id: -1, method: "storage.get" },
    { id: 1.5, method: "storage.get" },
    { id: "1", method: "storage.get" },
    { id: Number.MAX_SAFE_INTEGER + 1, method: "storage.get" },
    { id: Number.NaN, method: "storage.get" },
  ])("drops %j, which has no id to answer", (data) => {
    expect(readPortMessage(data)).toEqual({ kind: "drop" });
  });

  it.each([
    undefined,
    "",
    "1abc",
    ".storage",
    "storage get",
    "storage/get",
    "x".repeat(129),
    "störage.get",
    "a\nb",
    7,
  ])("refuses the method %j as bad arguments", (method) => {
    expect(readPortMessage({ id: 3, method })).toEqual({
      kind: "refuse",
      id: 3,
      error: { code: MOD_ERRORS.badArgs, message: "PM couldn't read that call's method." },
    });
  });

  it("is told apart by isMethodName exactly as the page reads it (the frame SDK asks it first)", () => {
    for (const method of ["storage.get", "a", "A_b-c.9", "x".repeat(128)]) {
      expect(isMethodName(method), method).toBe(true);
      expect(readPortMessage({ id: 1, method }).kind).toBe("call");
    }
    for (const method of [undefined, "", "1abc", "a b", "x".repeat(129), 7, null]) {
      expect(isMethodName(method), String(method)).toBe(false);
      expect(readPortMessage({ id: 1, method }).kind).toBe("refuse");
    }
  });

  it("refuses arguments that aren't plain JSON", () => {
    const cycle: Record<string, unknown> = {};
    cycle.self = cycle;
    for (const args of [cycle, { n: 10n }]) {
      expect(readPortMessage({ id: 1, method: "a", args })).toEqual({
        kind: "refuse",
        id: 1,
        error: { code: MOD_ERRORS.badArgs, message: "A call's arguments must be plain JSON." },
      });
    }
  });

  it("measures arguments in UTF-8 bytes, refusing past the cap and taking them at it", () => {
    const at = "x".repeat(MAX_ARGS_BYTES - 2); // the quotes make it exactly the cap
    expect(encodeArgs(at)).toEqual({ ok: true, json: JSON.stringify(at) });
    const tooLarge = {
      ok: false,
      error: { code: MOD_ERRORS.tooLarge, message: "A call's arguments are too large." },
    };
    expect(encodeArgs(at + "x")).toEqual(tooLarge);
    // "é" is one UTF-16 unit and two bytes: a string the cap allows in units is refused in bytes.
    const wide = "é".repeat(MAX_ARGS_BYTES / 2);
    expect(wide.length).toBeLessThan(MAX_ARGS_BYTES);
    expect(encodeArgs(wide)).toEqual(tooLarge);
    expect(fitsBytes("é".repeat(10), 20)).toBe(true);
    expect(fitsBytes("é".repeat(10), 19)).toBe(false);
    expect(fitsBytes("😀", 4)).toBe(true);
    expect(fitsBytes("😀", 3)).toBe(false);
  });

  it("sends exactly the JSON it measured", () => {
    const when = new Date(0);
    expect(encodeArgs({ when, gone: undefined, list: [1, undefined] })).toEqual({
      ok: true,
      json: '{"when":"1970-01-01T00:00:00.000Z","list":[1,null]}',
    });
  });
});

describe("the backend's answer", () => {
  it("is read as a value, a page action or a refusal", () => {
    expect(readReply({ kind: "value", value: { a: 1 } })).toEqual({
      kind: "value",
      value: { a: 1 },
    });
    expect(readReply({ kind: "value", value: null })).toEqual({ kind: "value", value: null });
    expect(readReply({ kind: "page", action: "theme.get", args: null })).toEqual({
      kind: "page",
      action: "theme.get",
      args: null,
    });
    expect(
      readReply({
        kind: "error",
        error: {
          code: "MOD-4001",
          message: "stale",
          revision: 3,
          permission: "storage",
          extra: "dropped",
        },
      }),
    ).toEqual({
      kind: "error",
      error: { code: "MOD-4001", message: "stale", revision: 3, permission: "storage" },
    });
  });

  it("refuses a page action the page doesn't perform", () => {
    expect(readReply({ kind: "page", action: "shell.run", args: null })).toEqual({
      kind: "error",
      error: { code: MOD_ERRORS.unknownMethod, message: "PM has no call by that name." },
    });
  });

  it.each([
    null,
    "value",
    { kind: "value" },
    { kind: "page" },
    { kind: "page", action: 1 },
    { kind: "error" },
    { kind: "error", error: "MOD-1001" },
    { kind: "error", error: { code: "ERR-1", message: "x" } },
    { kind: "error", error: { code: "MOD-1001" } },
    { kind: "mystery", value: 1 },
  ])("treats %j as PM's own failure, said without its contents", (data) => {
    expect(readReply(data)).toEqual({
      kind: "error",
      error: { code: MOD_ERRORS.internal, message: INTERNAL_MESSAGE },
    });
  });
});

describe("the vectors answer", () => {
  const DOCS = [
    { id: 3, vec: [0.5, -0.25, 1] },
    { id: 1, vec: [0, 2, -3] },
  ];

  it("reads as views over the buffer, ids and floats in the order sent", () => {
    const buffer = pack(3, DOCS);
    const reply = readReply(buffer);
    expect(reply.kind).toBe("binary");
    if (reply.kind !== "binary") return;
    expect(reply.buffer).toBe(buffer);
    expect(reply.vectors.count).toBe(2);
    expect(reply.vectors.dimension).toBe(3);
    expect(Array.from(reply.vectors.ids)).toEqual([3, 1]);
    expect(Array.from(reply.vectors.vectors)).toEqual([0.5, -0.25, 1, 0, 2, -3]);
    // The views lie over the one buffer, which is what is transferred.
    expect(reply.vectors.ids.buffer).toBe(buffer);
    expect(reply.vectors.vectors.buffer).toBe(buffer);
    // The ids sit on an 8-byte boundary and the floats on a 4-byte one.
    expect(reply.vectors.ids.byteOffset % 8).toBe(0);
    expect(reply.vectors.vectors.byteOffset % 4).toBe(0);
  });

  it("reads the Rust side's pinned bytes", () => {
    // embeddings.rs's `the_layout_round_trips…`: version 1, 2 documents, 3 floats each.
    const buffer = pack(3, DOCS);
    expect(Array.from(new Uint8Array(buffer, 0, 16))).toEqual([
      1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0,
    ]);
    expect(buffer.byteLength).toBe(16 + 2 * 8 + 2 * 3 * 4);
  });

  it("reads an empty answer, and a lone document", () => {
    expect(readVectors(pack(384, []))).toMatchObject({ count: 0, dimension: 384 });
    expect(readVectors(pack(1, [{ id: 9, vec: [1] }]))?.vectors.length).toBe(1);
  });

  it("is told apart from JSON by its tag, even from another realm", () => {
    expect(isArrayBuffer(new ArrayBuffer(1))).toBe(true);
    for (const other of [
      new Uint8Array(4),
      { byteLength: 4 },
      "abc",
      null,
      new SharedArrayBuffer(4),
    ])
      expect(isArrayBuffer(other)).toBe(false);
  });

  it.each([
    ["a version it doesn't know", pack(3, DOCS, [2, 0, 0])],
    ["a reserved word that isn't 0", pack(3, DOCS, [1, 0, 7])],
    ["no header", new ArrayBuffer(15)],
    ["nothing", new ArrayBuffer(0)],
    ["a body one float short", pack(3, DOCS).slice(0, -4)],
    ["a body one float long", grow(pack(3, DOCS))],
    ["a count past the bytes", withCount(pack(3, DOCS), 5)],
    [
      "a count and dimension whose product overflows",
      withCountAndDimension(2 ** 32 - 1, 2 ** 32 - 1),
    ],
    ["more bytes than the cap", new ArrayBuffer(MAX_BINARY_BYTES + 1)],
  ])("treats %s as PM's own failure", (_name, buffer) => {
    expect(readVectors(buffer)).toBeNull();
    expect(readReply(buffer)).toEqual({
      kind: "error",
      error: { code: MOD_ERRORS.internal, message: INTERNAL_MESSAGE },
    });
  });

  it("has the cap the backend has", () => {
    expect(MAX_BINARY_BYTES).toBe(20 * 1024 * 1024);
    // 5,000 documents of 1,024 dimensions, the largest the Host API sizes for, fits.
    expect(16 + 5000 * (8 + 4 * 1024)).toBeLessThanOrEqual(MAX_BINARY_BYTES);
  });
});

/** `buffer` with four more bytes on the end. */
function grow(buffer: ArrayBuffer): ArrayBuffer {
  const out = new Uint8Array(buffer.byteLength + 4);
  out.set(new Uint8Array(buffer));
  return out.buffer;
}

/** `buffer` with its count word changed. */
function withCount(buffer: ArrayBuffer, count: number): ArrayBuffer {
  new DataView(buffer).setUint32(4, count, true);
  return buffer;
}

/** A bare header claiming `count` documents of `dimension` floats. */
function withCountAndDimension(count: number, dimension: number): ArrayBuffer {
  const buffer = new ArrayBuffer(16);
  const view = new DataView(buffer);
  view.setUint32(0, 1, true);
  view.setUint32(4, count, true);
  view.setUint32(8, dimension, true);
  return buffer;
}

describe("refusals sent back", () => {
  it("carry the code, the message clipped, and only what a mod can act on", () => {
    expect(errorText("short")).toBe("short");
    expect(errorText("x".repeat(600))).toHaveLength(501);
    expect(callError(4, { code: "MOD-3001", message: "y".repeat(600) })).toEqual({
      id: 4,
      ok: false,
      error: { code: "MOD-3001", message: `${"y".repeat(500)}…` },
    });
    const withDetails = callError(5, {
      code: "MOD-4001",
      message: "stale",
      revision: 2,
      permission: "storage",
    });
    expect(withDetails.error).toEqual({
      code: "MOD-4001",
      message: "stale",
      revision: 2,
      permission: "storage",
    });
  });
});

describe("the flood gate", () => {
  it("spends its burst, refills at the backend's rate, and never past its size", () => {
    const gate = new FloodGate(0);
    for (let i = 0; i < FLOOD_BURST; i += 1) expect(gate.take(0)).toBe(true);
    expect(gate.take(0)).toBe(false);
    const oneEvery = 1000 / FLOOD_CALLS_PER_SECOND;
    expect(gate.take(oneEvery - 1)).toBe(false);
    expect(gate.take(oneEvery)).toBe(true);
    expect(gate.take(oneEvery)).toBe(false);
    // An hour's rest refills to the burst and no further.
    const later = 3_600_000;
    for (let i = 0; i < FLOOD_BURST; i += 1) expect(gate.take(later)).toBe(true);
    expect(gate.take(later)).toBe(false);
    // A clock that goes back earns nothing.
    expect(gate.take(0)).toBe(false);
  });

  it("lets a steady caller at the backend's rate through forever", () => {
    const gate = new FloodGate(0);
    const step = 1000 / FLOOD_CALLS_PER_SECOND;
    for (let i = 0; i < 10_000; i += 1) expect(gate.take(i * step)).toBe(true);
  });
});

describe("events", () => {
  it("reads an event from the backend: a name PM knows and a whole sequence number", () => {
    expect(readEvent({ seq: 3, name: "documents.added", payload: { documents: [] } })).toEqual({
      event: "documents.added",
      seq: 3,
      payload: { documents: [] },
    });
    // A missing payload is null; `missed` rides along only when it is a count.
    expect(readEvent({ seq: 1, name: "session.closed" })).toEqual({
      event: "session.closed",
      seq: 1,
      payload: null,
    });
    expect(readEvent({ seq: 2, name: "mod.paused", payload: {}, missed: 5 })?.missed).toBe(5);
    for (const missed of [0, -1, 1.5, "5", null]) {
      expect(readEvent({ seq: 2, name: "mod.paused", payload: {}, missed })).not.toHaveProperty(
        "missed",
      );
    }
    // Every event PM has is one the page lets through.
    for (const name of MOD_EVENTS) expect(readEvent({ seq: 1, name, payload: {} })).not.toBeNull();
  });

  it("drops what isn't an event: an unknown name, no sequence, a bad one, not an object", () => {
    for (const bad of [
      null,
      undefined,
      "documents.added",
      [],
      { seq: 1, name: "tasks.changed", payload: {} },
      { seq: 1, name: "constructor", payload: {} },
      { seq: 1, payload: {} },
      { name: "documents.added", payload: {} },
      { seq: 0, name: "documents.added" },
      { seq: -1, name: "documents.added" },
      { seq: 1.5, name: "documents.added" },
      { seq: "1", name: "documents.added" },
      { seq: 2 ** 60, name: "documents.added" },
    ]) {
      expect(readEvent(bad), JSON.stringify(bad)).toBeNull();
    }
  });

  it("reads an acknowledgement, and drops one that isn't a whole number", () => {
    expect(readPortMessage({ ack: 0 })).toEqual({ kind: "ack", seq: 0 });
    expect(readPortMessage({ ack: 41 })).toEqual({ kind: "ack", seq: 41 });
    // A call that also says `ack` is an acknowledgement and nothing else: it never reaches the backend.
    expect(readPortMessage({ ack: 3, id: 1, method: "storage.get", args: {} })).toEqual({
      kind: "ack",
      seq: 3,
    });
    for (const ack of [-1, 1.5, "3", null, undefined, Number.NaN, Infinity, 2 ** 60, {}]) {
      expect(readPortMessage({ ack }), String(ack)).toEqual({ kind: "drop" });
    }
  });

  it("holds events until connected, then sends them in order, and sends the rest at once", () => {
    const sent: number[] = [];
    const pump = new EventPump((e) => sent.push(e.seq));
    pump.push({ seq: 1, name: "documents.added", payload: {} });
    pump.push({ seq: 2, name: "documents.added", payload: {} });
    pump.push({ seq: 3, name: "tasks.changed", payload: {} });
    expect(sent).toEqual([]);
    pump.connect();
    expect(sent).toEqual([1, 2]);
    pump.push({ seq: 4, name: "documents.added", payload: {} });
    expect(sent).toEqual([1, 2, 4]);
    // Connecting twice sends nothing twice.
    pump.connect();
    expect(sent).toEqual([1, 2, 4]);
  });

  it("holds no more than the backend allows outstanding, and reports what it had to drop", () => {
    const got: { seq: number; missed?: number }[] = [];
    const pump = new EventPump((e) => got.push({ seq: e.seq, missed: e.missed }));
    for (let seq = 1; seq <= EVENT_BUFFER + 3; seq += 1) {
      pump.push({ seq, name: "documents.deleted", payload: {} });
    }
    pump.connect();
    expect(got).toHaveLength(EVENT_BUFFER);
    // The next event after the drop says how many were lost, added to what the backend said.
    pump.push({ seq: EVENT_BUFFER + 4, name: "documents.deleted", payload: {}, missed: 2 });
    expect(got[got.length - 1]).toEqual({ seq: EVENT_BUFFER + 4, missed: 5 });
    pump.push({ seq: EVENT_BUFFER + 5, name: "documents.deleted", payload: {} });
    expect(got[got.length - 1]).toEqual({ seq: EVENT_BUFFER + 5, missed: undefined });
  });
});
