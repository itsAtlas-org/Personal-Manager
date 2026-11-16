// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the Mods tab has in flight lives outside the view, because the view unmounts on every tab
// switch while the backend's work doesn't stop: busy, the last refusal, the notes an action leaves and
// a staged consent screen all outlive a mount, and the action's follow-up runs without one.

import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  addModNote,
  clearModError,
  dismissModNote,
  resetModActions,
  runModAction,
  setModScreen,
  useModActions,
} from "./modActions";
import type { ConsentScreen } from "./modsTypes";

beforeEach(resetModActions);
afterEach(cleanup);

/** A promise settled by hand, so a test can look at the world while an action is in flight. */
function gate<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("runModAction", () => {
  it("is busy while it runs and idle when it is done, with the value", async () => {
    const view = renderHook(() => useModActions());
    const g = gate<number>();
    let result!: Promise<unknown>;
    act(() => {
      result = runModAction("acme", "Pausing", "failed", () => g.promise);
    });
    expect(view.result.current.busy.get("acme")).toBe("Pausing");
    await act(async () => {
      g.resolve(7);
      await result;
    });
    expect(view.result.current.busy.has("acme")).toBe(false);
    await expect(result).resolves.toEqual({ ok: true, value: 7 });
  });

  it("refuses a second action for a busy key, from any caller, until the first settles", async () => {
    const g = gate<void>();
    let calls = 0;
    const first = runModAction("acme", "Pausing", "failed", () => {
      calls++;
      return g.promise;
    });
    const second = await runModAction("acme", "Resuming", "failed", () => {
      calls++;
      return Promise.resolve();
    });
    expect(second).toEqual({ ok: false, error: null });
    expect(calls).toBe(1);
    g.resolve();
    await first;
    // Another key was never in the way.
    await expect(runModAction("other", "x", "failed", () => Promise.resolve(1))).resolves.toEqual({
      ok: true,
      value: 1,
    });
    await expect(
      runModAction("acme", "again", "failed", () => Promise.resolve(2)),
    ).resolves.toEqual({ ok: true, value: 2 });
  });

  it("keeps the backend's sentence for a refusal, frees the key, and clears it on the next try", async () => {
    const view = renderHook(() => useModActions());
    const refusal = { code: "refused", message: "The safety list still names this mod." };
    const r = await runModAction("acme", "Resuming", "fallback", () => Promise.reject(refusal));
    expect(r).toEqual({ ok: false, error: "The safety list still names this mod." });
    expect(view.result.current.errors.get("acme")).toBe("The safety list still names this mod.");
    expect(view.result.current.busy.has("acme")).toBe(false);

    await act(async () => {
      await runModAction("acme", "Resuming", "fallback", () => Promise.resolve());
    });
    expect(view.result.current.errors.has("acme")).toBe(false);
  });

  it("falls back to its own words when the rejection says nothing", async () => {
    const r = await runModAction("acme", "x", "PM couldn't do that.", () => Promise.reject("nope"));
    expect(r).toEqual({ ok: false, error: "PM couldn't do that." });
  });

  it("runs its follow-up with nothing mounted", async () => {
    // No hook is rendered: the view that asked is gone, and the follow-up still happens.
    let followed: number | null = null;
    await runModAction(
      "acme",
      "Uninstalling",
      "failed",
      () => Promise.resolve(3),
      (v) => {
        followed = v;
      },
    );
    expect(followed).toBe(3);
  });

  it("a follow-up that throws doesn't turn a finished action into a failure", async () => {
    const view = renderHook(() => useModActions());
    const r = await runModAction(
      "acme",
      "x",
      "failed",
      () => Promise.resolve("done"),
      () => {
        throw new Error("storage is full");
      },
    );
    expect(r).toEqual({ ok: true, value: "done" });
    expect(view.result.current.errors.size).toBe(0);
    expect(view.result.current.busy.size).toBe(0);
  });

  it("doesn't run the follow-up when the action failed", async () => {
    let ran = false;
    await runModAction(
      "acme",
      "x",
      "failed",
      () => Promise.reject(new Error("no")),
      () => {
        ran = true;
      },
    );
    expect(ran).toBe(false);
  });

  it("shows a view that mounts mid-flight what is still going", async () => {
    const g = gate<void>();
    const going = runModAction("acme", "Uninstalling", "failed", () => g.promise);
    // The view mounts (the person came back to the tab) after the action started.
    const view = renderHook(() => useModActions());
    expect(view.result.current.busy.get("acme")).toBe("Uninstalling");
    await act(async () => {
      g.resolve();
      await going;
    });
    expect(view.result.current.busy.has("acme")).toBe(false);
  });
});

describe("errors, notes and screens", () => {
  it("clears one refusal on request", async () => {
    const view = renderHook(() => useModActions());
    await act(async () => {
      await runModAction("acme", "x", "failed", () => Promise.reject(new Error("bad")));
    });
    act(() => clearModError("acme"));
    expect(view.result.current.errors.has("acme")).toBe(false);
  });

  it("keeps notes in order until each is dismissed", () => {
    const view = renderHook(() => useModActions());
    act(() => {
      addModNote("info", "one");
      addModNote("warning", "two");
    });
    expect(view.result.current.notes.map((n) => n.text)).toEqual(["one", "two"]);
    const first = view.result.current.notes[0].id;
    act(() => dismissModNote(first));
    expect(view.result.current.notes.map((n) => n.text)).toEqual(["two"]);
  });

  it("holds a staged consent screen per mod until it is dropped", () => {
    const view = renderHook(() => useModActions());
    const screen = { id: "acme", token: "t" } as unknown as ConsentScreen;
    act(() => setModScreen("acme", screen));
    expect(view.result.current.screens.get("acme")).toBe(screen);
    // A new mount still finds it.
    expect(renderHook(() => useModActions()).result.current.screens.get("acme")).toBe(screen);
    act(() => setModScreen("acme", null));
    expect(view.result.current.screens.has("acme")).toBe(false);
  });
});
