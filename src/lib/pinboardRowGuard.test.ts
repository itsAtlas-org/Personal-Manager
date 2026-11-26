// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The `pinboard` row in the store's settings is the board as it was before the Pinboard became a mod:
// it stays, frozen, as the person's own copy, and nothing in the webview reads or writes it any more
// (the core Pinboard's 500 ms debounced `setPref("pinboard")` went with its page). This is the
// source scan that keeps it so. The Rust side still reads the row (the Calendar before the move,
// project rename and delete before the move, backups' "does this store hold anything" probe) and
// has its own guard; this one is only about the TypeScript that could write it from the webview.

import { describe, expect, it } from "vitest";

/** Every non-test source file under src/. */
const SOURCES = Object.entries(
  import.meta.glob<string>(["../**/*.ts", "../**/*.tsx"], {
    query: "?raw",
    import: "default",
    eager: true,
  }),
).filter(([path]) => !/\.test\.tsx?$/.test(path));

// A call of the preference commands naming the `pinboard` row, by literal, in any quote style.
const PINBOARD_PREF_CALL = /\b(?:getPref|setPref)\s*\(\s*(["'`])pinboard\1/;

describe("the frozen pinboard settings row", () => {
  it("sees the sources it is meant to be scanning", () => {
    expect(SOURCES.length).toBeGreaterThan(200);
    expect(SOURCES.map(([path]) => path)).toContain("./ipc.ts");
  });

  it("spots the call it forbids, in every quote style, and not an unrelated key", () => {
    expect(PINBOARD_PREF_CALL.test('await getPref("pinboard")')).toBe(true);
    expect(PINBOARD_PREF_CALL.test("setPref('pinboard', json)")).toBe(true);
    expect(PINBOARD_PREF_CALL.test("getPref(\n  `pinboard`,\n)")).toBe(true);
    expect(PINBOARD_PREF_CALL.test('getPref("appearance")')).toBe(false);
    expect(PINBOARD_PREF_CALL.test('getPref("pinboard_other")')).toBe(false);
  });

  it("is read and written by no file in the webview", () => {
    const offenders = SOURCES.filter(([, text]) => PINBOARD_PREF_CALL.test(text)).map(
      ([path]) => path,
    );
    expect(offenders).toEqual([]);
  });
});
