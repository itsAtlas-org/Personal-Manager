// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// How often the full Mods disclaimer has been shown on this device in a vault, and "Don't show this
// again" (modsDisclaimerShown.ts). Every failure has to read toward MORE disclaimer, never less.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const vaultStatus = vi.fn();
vi.mock("./ipc", () => ({ vaultStatus: () => vaultStatus() }));

import {
  beginShowing,
  offersHide,
  readShown,
  rememberHide,
  vaultKeyOf,
  type Showing,
} from "./modsDisclaimerShown";

const A = "pm.mods.disclaimerShown.vault-a";
const B = "pm.mods.disclaimerShown.vault-b";

beforeEach(() => {
  localStorage.clear();
  vaultStatus.mockReset();
  vaultStatus.mockResolvedValue({ vault_id: "vault-a" });
});
afterEach(() => vi.restoreAllMocks());

describe("vaultKeyOf", () => {
  it("keys a plain token and nothing else", () => {
    expect(vaultKeyOf("vault-a")).toBe(A);
    expect(vaultKeyOf("0f1e2d3c-aaaa-bbbb-cccc-1234567890ab")).toBe(
      "pm.mods.disclaimerShown.0f1e2d3c-aaaa-bbbb-cccc-1234567890ab",
    );
    for (const bad of [null, undefined, "", 7, {}, "a b", "a/b", "a.b", "x".repeat(65), "a\nb"]) {
      expect(vaultKeyOf(bad), String(bad)).toBeNull();
    }
  });
});

describe("readShown", () => {
  it("reads nothing shown and not opted out when there is no value", () => {
    expect(readShown(A)).toEqual({ shown: 0, hide: false });
  });

  it("reads what was written", () => {
    localStorage.setItem(A, JSON.stringify({ v: 1, shown: 3, hide: true }));
    expect(readShown(A)).toEqual({ shown: 3, hide: true });
  });

  it.each([
    ["not JSON", "{oops"],
    ["a number", "7"],
    ["null", "null"],
    ["an array", "[1,2]"],
    ["another version", JSON.stringify({ v: 2, shown: 3, hide: true })],
    ["no version", JSON.stringify({ shown: 3, hide: true })],
    ["a text count", JSON.stringify({ v: 1, shown: "3", hide: true })],
    ["a fractional count", JSON.stringify({ v: 1, shown: 1.5, hide: true })],
    ["a negative count", JSON.stringify({ v: 1, shown: -1, hide: true })],
    ["an absurd count", JSON.stringify({ v: 1, shown: 1e12, hide: true })],
    ["a text hide", JSON.stringify({ v: 1, shown: 3, hide: "true" })],
    ["no hide", JSON.stringify({ v: 1, shown: 3 })],
  ])("reads %s as zero showings and not opted out", (_what, raw) => {
    localStorage.setItem(A, raw);
    expect(readShown(A)).toEqual({ shown: 0, hide: false });
  });

  it("reads zero when localStorage itself throws", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(readShown(A)).toEqual({ shown: 0, hide: false });
  });
});

describe("beginShowing", () => {
  it("counts one showing each time it is called, starting at one", async () => {
    expect((await beginShowing(true)).number).toBe(1);
    expect((await beginShowing(true)).number).toBe(2);
    expect((await beginShowing(false)).number).toBe(3);
    expect(readShown(A)).toEqual({ shown: 3, hide: false });
  });

  it("keeps each vault's count apart", async () => {
    await beginShowing(true);
    await beginShowing(true);
    vaultStatus.mockResolvedValue({ vault_id: "vault-b" });
    expect((await beginShowing(true)).number).toBe(1);
    expect(readShown(A).shown).toBe(2);
    expect(readShown(B).shown).toBe(1);
  });

  it("starts again from a broken value", async () => {
    localStorage.setItem(A, "{oops");
    expect((await beginShowing(true)).number).toBe(1);
    expect(readShown(A)).toEqual({ shown: 1, hide: false });
  });

  it("counts nothing, and remembers nothing, when the vault can't be named", async () => {
    for (const status of [
      () => Promise.resolve({ vault_id: null }),
      () => Promise.resolve({ vault_id: "has spaces" }),
      () => undefined,
      () => Promise.reject(new Error("locked")),
    ]) {
      vaultStatus.mockImplementation(status);
      const s = await beginShowing(true);
      expect(s).toEqual({ number: 1, hide: false, key: null });
      expect(offersHide(s)).toBe(false);
    }
    expect(localStorage.length).toBe(0);
  });

  it("doesn't count a showing that is folded away, but counts one the gate always shows", async () => {
    await beginShowing(true);
    await beginShowing(true);
    rememberHide(await beginShowing(true));
    expect(readShown(A)).toEqual({ shown: 3, hide: true });
    const folded = await beginShowing(true);
    expect(folded.hide).toBe(true);
    expect(readShown(A).shown).toBe(3);
    // The Mods tab's own gate isn't foldable: full text, one more showing.
    expect((await beginShowing(false)).number).toBe(4);
    expect(readShown(A)).toEqual({ shown: 4, hide: true });
  });

  it("carries on without a count when localStorage can't be written", async () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("full");
    });
    const first = await beginShowing(true);
    const second = await beginShowing(true);
    expect([first.number, second.number]).toEqual([1, 1]);
    expect(offersHide(second)).toBe(false);
  });
});

describe("offersHide and rememberHide", () => {
  const showing = (number: number, hide = false): Showing => ({ number, hide, key: A });

  it("offers the box from the second showing on, never the first, and not once it is on", () => {
    expect(offersHide(showing(1))).toBe(false);
    expect(offersHide(showing(2))).toBe(true);
    expect(offersHide(showing(9))).toBe(true);
    expect(offersHide(showing(9, true))).toBe(false);
  });

  it("remembers the choice for the vault the showing was counted in, keeping a later count", async () => {
    const s = await beginShowing(true);
    await beginShowing(true);
    rememberHide(s);
    expect(readShown(A)).toEqual({ shown: 2, hide: true });
    expect(readShown(B)).toEqual({ shown: 0, hide: false });
  });

  it("can't remember a showing that had no vault", () => {
    rememberHide({ number: 5, hide: false, key: null });
    expect(localStorage.length).toBe(0);
  });
});
