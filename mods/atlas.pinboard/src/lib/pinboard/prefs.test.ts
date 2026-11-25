// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Pinboard's three preferences, kept in one mod-storage key (core kept three `localStorage` keys,
// which a frame does not have). What a stored value means, that a write saves the whole key, and that
// a page with nothing stored (or nothing to save to) still answers with the safe defaults.

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  isPastTimelineDate,
  readConfirmDelete,
  readPrefs,
  readShowCompletedTimelineItems,
  readShowPastTimelineItems,
  resetPrefs,
  startPrefs,
  writeConfirmDelete,
  writeShowCompletedTimelineItems,
  writeShowPastTimelineItems,
} from "./prefs";

afterEach(resetPrefs);

describe("readPrefs", () => {
  it("is every nag on and everything shown until the person says otherwise", () => {
    for (const raw of [
      null,
      undefined,
      0,
      "no",
      [],
      {},
      { confirmDelete: "false" },
      { showPast: 0 },
    ]) {
      expect(readPrefs(raw)).toEqual({ confirmDelete: true, showPast: true, showCompleted: true });
    }
  });

  it("takes only a literal false as off, the reading core's 'false' string had", () => {
    expect(readPrefs({ confirmDelete: false, showPast: false, showCompleted: false })).toEqual({
      confirmDelete: false,
      showPast: false,
      showCompleted: false,
    });
    expect(readPrefs({ confirmDelete: false, showPast: true })).toEqual({
      confirmDelete: false,
      showPast: true,
      showCompleted: true,
    });
  });
});

describe("the readers and writers", () => {
  it("answer with the defaults before anything is started", () => {
    expect([
      readConfirmDelete(),
      readShowPastTimelineItems(),
      readShowCompletedTimelineItems(),
    ]).toEqual([true, true, true]);
    // A write with nowhere to save to is kept for this visit and does not throw.
    writeConfirmDelete(false);
    expect(readConfirmDelete()).toBe(false);
  });

  it("answer from what storage held, synchronously", () => {
    startPrefs({ confirmDelete: false, showCompleted: false }, () => {});
    expect([
      readConfirmDelete(),
      readShowPastTimelineItems(),
      readShowCompletedTimelineItems(),
    ]).toEqual([false, true, false]);
  });

  it("each write saves the whole key, with the other two as they were", () => {
    const save = vi.fn();
    startPrefs({ showPast: false }, save);
    writeConfirmDelete(false);
    writeShowCompletedTimelineItems(false);
    writeShowPastTimelineItems(true);
    expect(save.mock.calls.map((c) => c[0])).toEqual([
      { confirmDelete: false, showPast: false, showCompleted: true },
      { confirmDelete: false, showPast: false, showCompleted: false },
      { confirmDelete: false, showPast: true, showCompleted: false },
    ]);
  });

  it("resetPrefs forgets what was kept and where it was saved", () => {
    const save = vi.fn();
    startPrefs({ confirmDelete: false }, save);
    resetPrefs();
    expect(readConfirmDelete()).toBe(true);
    writeShowPastTimelineItems(false);
    expect(save).not.toHaveBeenCalled();
  });
});

describe("isPastTimelineDate", () => {
  it("is a date string compared with today's, never a Date (a bare date reads as UTC midnight)", () => {
    expect(isPastTimelineDate("2026-11-23", "2026-11-24")).toBe(true);
    expect(isPastTimelineDate("2026-11-24", "2026-11-24")).toBe(false);
    expect(isPastTimelineDate("2026-11-24T10:00:00Z", "2026-11-24")).toBe(false);
    expect(isPastTimelineDate("", "2026-11-24")).toBe(false);
    expect(isPastTimelineDate(null, "2026-11-24")).toBe(false);
    expect(isPastTimelineDate("soon", "2026-11-24")).toBe(false);
  });
});
