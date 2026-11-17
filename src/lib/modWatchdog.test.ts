// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The page's half of the frozen-page watchdog: a heartbeat to Rust every 2 s while any mod frame is
// mounted, on one timer however many frames, that says at once when the page's visibility changes,
// stops with the last frame, and never lets a failed beat surface. The decision (and what must not
// count as frozen) is Rust's, tested there.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const modsHeartbeat = vi.fn();
vi.mock("./ipc", () => ({ modsHeartbeat: (visible: boolean) => modsHeartbeat(visible) }));

import { BEAT_MS, frameMounted, mountedFrames } from "./modWatchdog";

let visibility: DocumentVisibilityState = "visible";

beforeEach(() => {
  vi.useFakeTimers();
  vi.clearAllMocks();
  modsHeartbeat.mockResolvedValue(undefined);
  visibility = "visible";
  Object.defineProperty(document, "visibilityState", {
    configurable: true,
    get: () => visibility,
  });
});

afterEach(() => {
  vi.useRealTimers();
});

function setVisibility(next: DocumentVisibilityState) {
  visibility = next;
  document.dispatchEvent(new Event("visibilitychange"));
}

describe("the heartbeat", () => {
  it("sends nothing while no mod frame is mounted", async () => {
    await vi.advanceTimersByTimeAsync(BEAT_MS * 10);
    expect(modsHeartbeat).not.toHaveBeenCalled();
    expect(mountedFrames()).toBe(0);
  });

  it("beats at once when the first frame mounts, then every BEAT_MS, and stops with the last", async () => {
    const release = frameMounted();
    expect(modsHeartbeat).toHaveBeenCalledTimes(1);
    expect(modsHeartbeat).toHaveBeenLastCalledWith(true);
    await vi.advanceTimersByTimeAsync(BEAT_MS - 1);
    expect(modsHeartbeat).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(modsHeartbeat).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(BEAT_MS * 3);
    expect(modsHeartbeat).toHaveBeenCalledTimes(5);
    release();
    expect(mountedFrames()).toBe(0);
    await vi.advanceTimersByTimeAsync(BEAT_MS * 10);
    expect(modsHeartbeat).toHaveBeenCalledTimes(5);
  });

  it("is two seconds, so Rust's ten-second threshold is five missed beats", () => {
    expect(BEAT_MS).toBe(2000);
  });

  it("runs one timer however many frames are mounted, until the last one goes", async () => {
    const a = frameMounted();
    const b = frameMounted();
    const c = frameMounted();
    expect(modsHeartbeat).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(BEAT_MS);
    expect(modsHeartbeat).toHaveBeenCalledTimes(2);
    a();
    b();
    expect(mountedFrames()).toBe(1);
    await vi.advanceTimersByTimeAsync(BEAT_MS);
    expect(modsHeartbeat).toHaveBeenCalledTimes(3);
    c();
    await vi.advanceTimersByTimeAsync(BEAT_MS * 5);
    expect(modsHeartbeat).toHaveBeenCalledTimes(3);
  });

  it("counts a frame once however often its release is called (StrictMode, a double cleanup)", async () => {
    const a = frameMounted();
    const b = frameMounted();
    a();
    a();
    a();
    expect(mountedFrames()).toBe(1);
    await vi.advanceTimersByTimeAsync(BEAT_MS);
    expect(modsHeartbeat).toHaveBeenCalledTimes(2);
    b();
    expect(mountedFrames()).toBe(0);
  });

  it("starts again for the next frame after the last one went", async () => {
    frameMounted()();
    modsHeartbeat.mockClear();
    const release = frameMounted();
    expect(modsHeartbeat).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(BEAT_MS);
    expect(modsHeartbeat).toHaveBeenCalledTimes(2);
    release();
  });

  it("says at once when the page is hidden or shown again, with what it is now", async () => {
    const release = frameMounted();
    modsHeartbeat.mockClear();
    setVisibility("hidden");
    expect(modsHeartbeat).toHaveBeenCalledTimes(1);
    expect(modsHeartbeat).toHaveBeenLastCalledWith(false);
    // A beat that its (throttled) timer sends while hidden says hidden too.
    await vi.advanceTimersByTimeAsync(BEAT_MS);
    expect(modsHeartbeat).toHaveBeenLastCalledWith(false);
    setVisibility("visible");
    expect(modsHeartbeat).toHaveBeenLastCalledWith(true);
    expect(modsHeartbeat).toHaveBeenCalledTimes(3);
    release();
  });

  it("stops listening for visibility with the last frame", () => {
    const release = frameMounted();
    release();
    modsHeartbeat.mockClear();
    setVisibility("hidden");
    expect(modsHeartbeat).not.toHaveBeenCalled();
  });

  it("never lets a failed heartbeat surface", async () => {
    // An unhandled rejection fails the whole vitest run, so reaching the end is the assertion.
    modsHeartbeat.mockRejectedValue({ code: "no-store", message: "closed" });
    const release = frameMounted();
    await vi.advanceTimersByTimeAsync(BEAT_MS * 3);
    expect(modsHeartbeat.mock.calls.length).toBeGreaterThanOrEqual(3);
    release();
    await vi.advanceTimersByTimeAsync(0);
  });
});
