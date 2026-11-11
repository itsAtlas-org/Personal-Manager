// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The bridge's reading rules (modBridge.ts): what a frame's messages may be, every refusal and its
// code, how the backend's answers are read, and the flood gate.

import { describe, expect, it } from "vitest";

import {
  FLOOD_BURST,
  FLOOD_CALLS_PER_SECOND,
  FloodGate,
  INTERNAL_MESSAGE,
  MAX_ARGS_BYTES,
  MAX_FRAME_HEIGHT,
  MIN_FRAME_HEIGHT,
  MOD_ERRORS,
  callError,
  clampFrameHeight,
  encodeArgs,
  errorText,
  fitsBytes,
  isConnectRequest,
  readPortMessage,
  readReply,
} from "./modBridge";

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
