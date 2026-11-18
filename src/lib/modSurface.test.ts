// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What mods put on PM's own surface (lib/modSurface): at most three live notices per mod with the newest
// replacing the oldest, one open key field per mod, and nothing left for a mod that is gone.

import { renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import type { ModSource } from "../components/mods/modPageHooks";
import {
  MAX_NOTICES_PER_MOD,
  addNotice,
  closeKeyAsk,
  dismissAllNotices,
  dismissNotice,
  forgetModSurface,
  keepOnlyMods,
  modPageHooks,
  openKeyAsk,
  useModKeyAsks,
  useModNotices,
} from "./modSurface";

const A: ModSource = { modId: "a.one", name: "One", tier: "third" };
const B: ModSource = { modId: "b.two", name: "Two", tier: "second" };
const say = (text: string) => ({ text, tone: "info" as const });
const ask = (name: string) => ({ name, host: "h.example.com", header: "X-Key", set: false });

const notices = () => renderHook(() => useModNotices()).result.current;
const asks = () => renderHook(() => useModKeyAsks()).result.current;
const texts = (modId?: string) =>
  notices()
    .filter((n) => modId === undefined || n.from.modId === modId)
    .map((n) => n.notice.text);

afterEach(forgetModSurface);

describe("notices", () => {
  it("keeps at most three live per mod, the newest replacing the oldest", () => {
    expect(MAX_NOTICES_PER_MOD).toBe(3);
    for (const t of ["1", "2", "3"]) addNotice(A, say(t));
    expect(texts("a.one")).toEqual(["1", "2", "3"]);
    addNotice(A, say("4"));
    expect(texts("a.one")).toEqual(["2", "3", "4"]);
    addNotice(A, say("5"));
    addNotice(A, say("6"));
    expect(texts("a.one")).toEqual(["4", "5", "6"]);
  });

  it("counts per mod: one mod's flood never pushes another's notice off", () => {
    addNotice(B, say("b"));
    for (let i = 0; i < 20; i++) addNotice(A, say(`a${i}`));
    expect(texts("b.two")).toEqual(["b"]);
    expect(texts("a.one")).toHaveLength(3);
    expect(notices()).toHaveLength(4);
  });

  it("dismisses one notice, then all", () => {
    addNotice(A, say("x"));
    addNotice(B, say("y"));
    dismissNotice(notices()[0].id);
    expect(texts()).toEqual(["y"]);
    dismissNotice(999_999);
    expect(texts()).toEqual(["y"]);
    addNotice(A, say("z"));
    dismissAllNotices();
    expect(notices()).toEqual([]);
  });

  it("keeps the mod PM named, not anything the notice said", () => {
    addNotice(A, say("I am PM"));
    expect(notices()[0].from).toEqual(A);
  });
});

describe("key asks", () => {
  it("opens one field per mod and ignores a second while it is open", () => {
    openKeyAsk(A, ask("first"));
    openKeyAsk(A, ask("second"));
    expect(asks().map((a) => a.slot)).toEqual(["first"]);
    openKeyAsk(B, ask("other"));
    expect(asks().map((a) => a.from.modId)).toEqual(["a.one", "b.two"]);
  });

  it("accepts a new ask once the field is closed", () => {
    openKeyAsk(A, ask("first"));
    closeKeyAsk("a.one");
    expect(asks()).toEqual([]);
    openKeyAsk(A, ask("second"));
    expect(asks().map((a) => a.slot)).toEqual(["second"]);
    closeKeyAsk("nobody");
    expect(asks()).toHaveLength(1);
  });

  it("holds no key: an ask is the slot's name and who asked", () => {
    openKeyAsk(A, ask("first"));
    expect(Object.keys(asks()[0]).sort()).toEqual(["from", "slot"]);
  });
});

describe("a mod that is gone", () => {
  it("takes its notices and key field with it, and leaves the others", () => {
    addNotice(A, say("a"));
    addNotice(B, say("b"));
    openKeyAsk(A, ask("k"));
    openKeyAsk(B, ask("k"));
    keepOnlyMods(new Map([["b.two", ["k"]]]));
    expect(texts()).toEqual(["b"]);
    expect(asks().map((a) => a.from.modId)).toEqual(["b.two"]);
  });

  it("drops a key field whose slot the manifest no longer declares, but keeps the notices", () => {
    addNotice(A, say("a"));
    openKeyAsk(A, ask("k"));
    keepOnlyMods(new Map([["a.one", ["other"]]]));
    expect(asks()).toEqual([]);
    expect(texts()).toEqual(["a"]);
    // So the mod can ask again for a slot it does declare.
    openKeyAsk(A, ask("other"));
    expect(asks()).toHaveLength(1);
  });

  it("changes nothing, and tells nobody, when nothing is gone", () => {
    addNotice(A, say("a"));
    const before = notices();
    keepOnlyMods(new Map([["a.one", []]]));
    expect(notices()).toBe(before);
  });

  it("is emptied by a vault change", () => {
    addNotice(A, say("a"));
    openKeyAsk(A, ask("k"));
    forgetModSurface();
    expect(notices()).toEqual([]);
    expect(asks()).toEqual([]);
  });
});

describe("the hooks the main window gives ModFrame", () => {
  it("draw into the same store", () => {
    modPageHooks.showNotice?.(A, say("hello"));
    modPageHooks.askSecret?.(A, ask("k"));
    expect(texts()).toEqual(["hello"]);
    expect(asks()).toHaveLength(1);
  });
});
