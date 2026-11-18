// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// spawnWorker: a Worker from source text, through a blob URL that lives exactly as long as the Worker.
// Node has no Worker of this kind, so a stand-in records what it was given.

import { afterEach, describe, expect, it, vi } from "vitest";

import { spawnWorker } from "./spawnWorker";

class FakeWorker {
  static starts: string[] = [];
  static failWith: Error | null = null;
  terminated = 0;
  constructor(readonly url: string) {
    if (FakeWorker.failWith) throw FakeWorker.failWith;
    FakeWorker.starts.push(url);
  }
  terminate() {
    this.terminated += 1;
  }
}

function stub() {
  const made: Blob[] = [];
  const revoked: string[] = [];
  FakeWorker.starts = [];
  FakeWorker.failWith = null;
  vi.stubGlobal("Worker", FakeWorker);
  vi.spyOn(URL, "createObjectURL").mockImplementation((blob) => {
    made.push(blob as Blob);
    return `blob:fake/${made.length}`;
  });
  vi.spyOn(URL, "revokeObjectURL").mockImplementation((url) => {
    revoked.push(url);
  });
  return { made, revoked };
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("spawnWorker", () => {
  it("runs a classic script made from exactly the text it is given", async () => {
    const { made } = stub();
    const worker = spawnWorker("self.onmessage = () => {};") as unknown as FakeWorker;
    expect(FakeWorker.starts).toEqual(["blob:fake/1"]);
    expect(worker.url).toBe("blob:fake/1");
    expect(made).toHaveLength(1);
    expect(made[0].type).toBe("text/javascript");
    expect(await made[0].text()).toBe("self.onmessage = () => {};");
  });

  it("keeps the URL alive while the worker runs and frees it, once, when it is terminated", () => {
    const { revoked } = stub();
    const worker = spawnWorker("1") as unknown as FakeWorker;
    expect(revoked).toEqual([]);
    worker.terminate();
    expect(revoked).toEqual(["blob:fake/1"]);
    expect(worker.terminated).toBe(1);
  });

  it("frees the URL of a worker that failed to start, and lets the failure through", () => {
    const { revoked } = stub();
    FakeWorker.failWith = new Error("blocked by the CSP");
    expect(() => spawnWorker("1")).toThrow("blocked by the CSP");
    expect(revoked).toEqual(["blob:fake/1"]);
  });
});
