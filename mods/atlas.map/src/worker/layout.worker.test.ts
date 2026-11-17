// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The worker's own file, run against a stand-in global scope (this test lives in the worker folder, so
// it is type-checked with the WebWorker lib, like the file it tests): it answers each message with
// `handle`'s reply and moves the coordinates' buffer instead of copying it.
import { describe, expect, it, vi } from "vitest";

const scope = vi.hoisted(() => {
  const s = {
    onmessage: null as ((event: { data: unknown }) => void) | null,
    postMessage: null as unknown as ReturnType<typeof import("vitest").vi.fn>,
  };
  (globalThis as unknown as { self: unknown }).self = s;
  return s;
});
scope.postMessage = vi.fn();

import "./layout.worker";

describe("layout.worker", () => {
  it("installs a message handler on the worker's global scope", () => {
    expect(typeof scope.onmessage).toBe("function");
  });

  it("answers a reduce job with handle's reply, transferring the coordinates", () => {
    scope.postMessage.mockClear();
    const vectors = new Float32Array(5 * 2);
    for (let i = 0; i < vectors.length; i++) vectors[i] = (i * 7) % 5;
    scope.onmessage?.({ data: { id: 8, kind: "reduce", vectors, count: 5, dimension: 2 } });
    expect(scope.postMessage).toHaveBeenCalledTimes(1);
    const [reply, transfer] = scope.postMessage.mock.calls[0] as [
      { id: number; kind: string; coords: Float64Array },
      Transferable[],
    ];
    expect(reply).toMatchObject({ id: 8, kind: "reduce" });
    expect(transfer).toEqual([reply.coords.buffer]);
  });

  it("answers garbage with an error reply and keeps listening", () => {
    scope.postMessage.mockClear();
    scope.onmessage?.({ data: 42 });
    expect(scope.postMessage).toHaveBeenCalledWith(expect.objectContaining({ kind: "error" }), []);
    expect(typeof scope.onmessage).toBe("function");
  });
});
