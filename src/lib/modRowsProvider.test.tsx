// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The one read of the installed mods that the sidebar's mod tabs, Ctrl K, each mod's tab and the Mods
// tab share (lib/modRows): read on mount and on every `mods://changed`, answers applied in the order
// the questions were asked, a failed read keeping what was known.

import { act, cleanup, render, renderHook, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { consentScreen } from "../components/mods/modFixtures";
import {
  addModNote,
  currentModActions,
  resetModActions,
  runModAction,
  setModScreen,
} from "./modActions";
import {
  addNotice,
  forgetModSurface,
  openKeyAsk,
  useModKeyAsks,
  useModNotices,
} from "./modSurface";
import type { ModInfo, ModsChanged } from "./modsTypes";

const listMods = vi.fn();
let changed: ((e: ModsChanged) => void) | null = null;
const unlisten = vi.fn();
vi.mock("./ipc", () => ({
  listMods: () => listMods(),
  onModsChanged: (handler: (e: ModsChanged) => void) => {
    changed = handler;
    return Promise.resolve(unlisten);
  },
}));

import { useModRows, type ModRows } from "./modRows";
import { ModRowsProvider } from "./modRowsProvider";

let latest: ModRows;
function Probe() {
  latest = useModRows();
  return (
    <p data-testid="state">
      {latest.loaded ? "loaded" : "not loaded"}|{latest.rows.map((r) => r.id).join(",")}|
      {latest.problem ?? ""}
    </p>
  );
}

const row = (id: string, slots: string[] = []) =>
  ({ id, secrets: slots.map((name) => ({ name })) }) as ModInfo;
const state = () => screen.getByTestId("state").textContent;

/** A read that finishes when the test says. */
function deferred() {
  let resolve!: (rows: ModInfo[]) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<ModInfo[]>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

async function mount() {
  render(
    <ModRowsProvider>
      <Probe />
    </ModRowsProvider>,
  );
  await act(async () => {});
}

beforeEach(() => {
  changed = null;
  listMods.mockReset();
  unlisten.mockReset();
  resetModActions();
  forgetModSurface();
});
afterEach(() => {
  cleanup();
  resetModActions();
  forgetModSurface();
});

describe("ModRowsProvider", () => {
  it("has no answer yet, then reads the rows on mount", async () => {
    listMods.mockResolvedValue([row("acme.a"), row("acme.b")]);
    await mount();
    expect(state()).toBe("loaded|acme.a,acme.b|");
    expect(listMods).toHaveBeenCalledTimes(1);
  });

  it("reads again on every mods://changed, whatever the cause", async () => {
    listMods.mockResolvedValueOnce([row("acme.a")]).mockResolvedValueOnce([row("acme.b")]);
    await mount();
    await act(async () => changed?.({ cause: "paused" }));
    expect(state()).toBe("loaded|acme.b|");
    expect(listMods).toHaveBeenCalledTimes(2);
  });

  it("drops the notices and key field of a mod that is gone, or of a slot it dropped, and keeps the rest", async () => {
    const a = { modId: "acme.a", name: "A", tier: "third" as const };
    const b = { modId: "acme.b", name: "B", tier: "third" as const };
    listMods
      .mockResolvedValueOnce([row("acme.a", ["k"]), row("acme.b", ["k"])])
      .mockResolvedValueOnce([row("acme.a", ["k"])])
      .mockResolvedValueOnce([row("acme.a", [])]);
    await mount();
    act(() => {
      addNotice(a, { text: "from a", tone: "info" });
      addNotice(b, { text: "from b", tone: "info" });
      openKeyAsk(a, { name: "k", host: "h", header: "X", set: false });
      openKeyAsk(b, { name: "k", host: "h", header: "X", set: false });
    });
    // b is uninstalled.
    await act(async () => changed?.({ cause: "uninstalled" }));
    const surface = renderHook(() => ({ n: useModNotices(), k: useModKeyAsks() })).result.current;
    expect(surface.n.map((n) => n.notice.text)).toEqual(["from a"]);
    expect(surface.k.map((x) => x.from.modId)).toEqual(["acme.a"]);
    // a's update drops its slot: the field goes, the notice stays.
    await act(async () => changed?.({ cause: "updated" }));
    const after = renderHook(() => ({ n: useModNotices(), k: useModKeyAsks() })).result.current;
    expect(after.n).toHaveLength(1);
    expect(after.k).toEqual([]);
  });

  it("a failed read keeps what mods put on screen (nothing is known to be gone)", async () => {
    const a = { modId: "acme.a", name: "A", tier: "third" as const };
    listMods
      .mockResolvedValueOnce([row("acme.a")])
      .mockRejectedValueOnce({ code: "locked", message: "PM is locked." });
    await mount();
    act(() => addNotice(a, { text: "from a", tone: "info" }));
    await act(async () => changed?.({ cause: "paused" }));
    expect(renderHook(() => useModNotices()).result.current).toHaveLength(1);
  });

  it("a store that opens forgets what the old vault's mods put on screen", async () => {
    const a = { modId: "acme.a", name: "A", tier: "third" as const };
    listMods.mockResolvedValue([row("acme.a")]);
    await mount();
    act(() => {
      addNotice(a, { text: "from a", tone: "info" });
      openKeyAsk(a, { name: "k", host: "h", header: "X", set: false });
    });
    // The vault switch, whose first read of the new vault fails: nothing from the old one survives.
    listMods.mockRejectedValueOnce({ code: "locked", message: "PM is locked." });
    await act(async () => changed?.({ cause: "loaded" }));
    expect(renderHook(() => useModNotices()).result.current).toEqual([]);
    expect(renderHook(() => useModKeyAsks()).result.current).toEqual([]);
  });

  it("applies answers in the order the questions were asked", async () => {
    // The first read is slow and started before the change; its answer must not overwrite the one the
    // change caused.
    const slow = deferred();
    listMods.mockReturnValueOnce(slow.promise).mockResolvedValueOnce([row("acme.new")]);
    await mount();
    await act(async () => changed?.({ cause: "installed" }));
    expect(state()).toBe("loaded|acme.new|");
    await act(async () => slow.resolve([row("acme.old")]));
    expect(state()).toBe("loaded|acme.new|");
  });

  it("keeps what it had when a read fails, says why, and recovers on the next", async () => {
    listMods
      .mockResolvedValueOnce([row("acme.a")])
      .mockRejectedValueOnce({ code: "locked", message: "PM is locked." })
      .mockResolvedValueOnce([row("acme.a"), row("acme.c")]);
    await mount();
    await act(async () => changed?.({ cause: "paused" }));
    expect(state()).toBe("loaded|acme.a|PM is locked.");
    await act(async () => changed?.({ cause: "paused" }));
    expect(state()).toBe("loaded|acme.a,acme.c|");
  });

  it("a store that opens drops the old vault's rows even when the first read of the new one fails", async () => {
    // The provider sits above the tree a lock or a vault switch replaces, so it is what would keep
    // vault A's mods (and their sidebar tabs) showing in vault B.
    listMods
      .mockResolvedValueOnce([row("acme.a")])
      .mockRejectedValueOnce({ code: "locked", message: "PM is locked." });
    await mount();
    await act(async () => changed?.({ cause: "loaded" }));
    expect(state()).toBe("not loaded||PM is locked.");
  });

  it("a store that opens forgets the old vault's notes, staged screens and refusals, and keeps work in flight; the background recovery pass that follows keeps them", async () => {
    listMods.mockResolvedValue([row("acme.a")]);
    await mount();
    addModNote("info", "“X” was uninstalled.");
    setModScreen("acme.x", consentScreen({ id: "acme.x" }));
    void runModAction("acme.y", "Pausing", "No.", () => new Promise(() => {}));
    await act(async () => changed?.({ cause: "installed" }));
    expect(currentModActions().notes).toHaveLength(1);
    expect(currentModActions().screens.size).toBe(1);
    await act(async () => changed?.({ cause: "loaded" }));
    expect(currentModActions().notes).toHaveLength(0);
    expect(currentModActions().screens.size).toBe(0);
    expect(currentModActions().busy.has("acme.y")).toBe(true);
    addModNote("info", "again");
    setModScreen("acme.x", consentScreen({ id: "acme.x" }));
    // `recovered` comes seconds after `loaded`, for the same vault: what the person staged since
    // (a consent screen they are reading) is still valid in the backend and must not vanish.
    await act(async () => changed?.({ cause: "recovered" }));
    expect(currentModActions().notes).toHaveLength(1);
    expect(currentModActions().screens.size).toBe(1);
  });

  it("is not loaded while the first read has failed, and says why", async () => {
    listMods.mockRejectedValueOnce(new Error("no vault yet"));
    await mount();
    expect(state()).toBe("not loaded||no vault yet");
  });

  it("offers a refresh that reads now and never rejects", async () => {
    listMods.mockResolvedValueOnce([row("acme.a")]).mockRejectedValueOnce(new Error("boom"));
    await mount();
    await act(async () => {
      await expect(latest.refresh()).resolves.toBeUndefined();
    });
    expect(listMods).toHaveBeenCalledTimes(2);
  });

  it("stops listening when it goes away, and an answer after that changes nothing", async () => {
    const slow = deferred();
    listMods.mockReturnValueOnce(slow.promise);
    render(
      <ModRowsProvider>
        <Probe />
      </ModRowsProvider>,
    );
    await act(async () => {});
    cleanup();
    expect(unlisten).toHaveBeenCalled();
    // Resolving after the unmount must not throw or warn (nothing is set on a gone provider).
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    await act(async () => slow.resolve([row("acme.a")]));
    expect(errors).not.toHaveBeenCalled();
    errors.mockRestore();
  });
});
