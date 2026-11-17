// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// PM's document events turned into one quiet notice: debounced, never per event, `missed` always a
// notice, silence after the session ends.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { HostEvent } from "../../../src/kit/host";
import { MAX_WAIT_MS, QUIET_MS, watchDocumentChanges, type ChangeNotice } from "./changes";
import { memoryHost } from "./memoryHost";

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

const event = (name: HostEvent["name"], payload: unknown = {}, missed?: number): HostEvent => ({
  name,
  seq: 1,
  payload,
  ...(missed === undefined ? {} : { missed }),
});

function watch() {
  const pm = memoryHost({ docs: [] });
  const notices: ChangeNotice[] = [];
  const watcher = watchDocumentChanges(pm.host, (n) => notices.push(n));
  return { pm, notices, watcher };
}

describe("watchDocumentChanges", () => {
  it("says nothing until the events go quiet, then once, with what it was told", () => {
    const { pm, notices } = watch();
    pm.emit(event("documents.added", { documents: [{}, {}, {}] }));
    pm.emit(event("documents.added", { documents: [{}] }));
    pm.emit(event("documents.deleted", { ids: [1, 2] }));
    vi.advanceTimersByTime(QUIET_MS - 1);
    expect(notices).toEqual([]);
    vi.advanceTimersByTime(1);
    expect(notices).toEqual([{ added: 4, deleted: 2, missed: false }]);
    // And no more: the batch is spent.
    vi.advanceTimersByTime(60_000);
    expect(notices).toHaveLength(1);
  });

  it("each event pushes the notice back, so a steady stream is one notice at the end", () => {
    const { pm, notices } = watch();
    for (let i = 0; i < 4; i++) {
      pm.emit(event("documents.added", { documents: [{}] }));
      vi.advanceTimersByTime(QUIET_MS - 100);
    }
    expect(notices).toEqual([]);
    vi.advanceTimersByTime(100);
    expect(notices).toEqual([{ added: 4, deleted: 0, missed: false }]);
  });

  it("but never holds a notice back longer than the deadline, however long the stream runs", () => {
    const { pm, notices } = watch();
    let t = 0;
    while (t < MAX_WAIT_MS) {
      pm.emit(event("documents.added", { documents: [{}] }));
      vi.advanceTimersByTime(500);
      t += 500;
    }
    expect(notices).toHaveLength(1);
    expect(notices[0].added).toBeGreaterThan(5);
    // The stream goes on: the next notice is a new batch.
    pm.emit(event("documents.deleted", { ids: [9] }));
    vi.advanceTimersByTime(QUIET_MS);
    expect(notices).toHaveLength(2);
    expect(notices[1]).toEqual({ added: 0, deleted: 1, missed: false });
  });

  it("an event that dropped others (missed) is a notice, on any kind of event", () => {
    for (const name of ["theme.changed", "grants.changed", "documents.added"] as const) {
      const { pm, notices } = watch();
      pm.emit(event(name, {}, 12));
      vi.advanceTimersByTime(QUIET_MS);
      expect(notices).toEqual([{ added: 0, deleted: 0, missed: true }]);
    }
  });

  it("missed: 0 or absent is not missing anything, and theme and grant events alone are not a notice", () => {
    const { pm, notices } = watch();
    pm.emit(event("theme.changed", { mode: "dark" }));
    pm.emit(event("grants.changed"));
    pm.emit(event("theme.changed", {}, 0));
    vi.advanceTimersByTime(60_000);
    expect(notices).toEqual([]);
  });

  it("an added or deleted event whose payload is not what PM sends still means documents changed", () => {
    const { pm, notices } = watch();
    pm.emit(event("documents.added", null));
    pm.emit(event("documents.deleted", "ids"));
    pm.emit(event("documents.added", { documents: "no" }));
    vi.advanceTimersByTime(QUIET_MS);
    expect(notices).toEqual([{ added: 0, deleted: 0, missed: false }]);
  });

  it("the end of the session drops what is pending: nothing is raised into a frame that cannot refresh", () => {
    for (const name of ["session.closed", "mod.paused"] as const) {
      const { pm, notices } = watch();
      pm.emit(event("documents.added", { documents: [{}] }));
      pm.emit(event(name, { reason: "locked" }));
      vi.advanceTimersByTime(60_000);
      expect(notices).toEqual([]);
      // And whatever comes after it is ignored too.
      pm.emit(event("documents.added", { documents: [{}] }));
      vi.advanceTimersByTime(60_000);
      expect(notices).toEqual([]);
    }
  });

  it("stop drops what is pending, stops listening, and is safe twice", () => {
    const { pm, notices, watcher } = watch();
    pm.emit(event("documents.added", { documents: [{}] }));
    watcher.stop();
    watcher.stop();
    expect(pm.handlers.size).toBe(0);
    vi.advanceTimersByTime(60_000);
    expect(notices).toEqual([]);
    pm.emit(event("documents.added", { documents: [{}] }));
    vi.advanceTimersByTime(60_000);
    expect(notices).toEqual([]);
  });

  it("two watchers do not share a batch", () => {
    const pm = memoryHost({ docs: [] });
    const a: ChangeNotice[] = [];
    const b: ChangeNotice[] = [];
    watchDocumentChanges(pm.host, (n) => a.push(n));
    const second = watchDocumentChanges(pm.host, (n) => b.push(n));
    pm.emit(event("documents.added", { documents: [{}] }));
    second.stop();
    vi.advanceTimersByTime(QUIET_MS);
    expect(a).toHaveLength(1);
    expect(b).toHaveLength(0);
  });

  it("a handler that throws does not break the next batch", () => {
    const pm = memoryHost({ docs: [] });
    let calls = 0;
    watchDocumentChanges(pm.host, () => {
      calls += 1;
      if (calls === 1) throw new Error("ui bug");
    });
    pm.emit(event("documents.added", { documents: [{}] }));
    expect(() => vi.advanceTimersByTime(QUIET_MS)).toThrow("ui bug");
    pm.emit(event("documents.added", { documents: [{}] }));
    vi.advanceTimersByTime(QUIET_MS);
    expect(calls).toBe(2);
  });
});
