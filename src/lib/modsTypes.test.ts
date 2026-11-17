// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import {
  ERROR_WORDS,
  failureText,
  iconBlob,
  modsErrorOf,
  pausedForDevMode,
  type ModIcon,
  type ModsErrorCode,
} from "./modsTypes";

describe("modsErrorOf", () => {
  it("reads the backend's {code, message} refusal", () => {
    expect(modsErrorOf({ code: "unsigned", message: "It isn't signed." })).toEqual({
      code: "unsigned",
      message: "It isn't signed.",
    });
  });

  it("is null for every other rejection, including a vault fault, which has an `op`", () => {
    for (const other of [
      "a bare string",
      null,
      undefined,
      42,
      new Error("boom"),
      { code: "locked", message: "x", op: "open_vault" },
      { code: 3, message: "x" },
      { code: "refused" },
      { message: "x" },
    ]) {
      expect(modsErrorOf(other)).toBeNull();
    }
  });
});

describe("iconBlob", () => {
  it("turns the base64 bytes into a Blob of the stated media type", async () => {
    const icon: ModIcon = { mediaType: "image/png", data: btoa("\x89PNG\r\n\x1a\nbits") };
    const blob = iconBlob(icon);
    expect(blob.type).toBe("image/png");
    expect(blob.size).toBe(8 + 4);
    const bytes = new Uint8Array(await blob.arrayBuffer());
    expect(Array.from(bytes.slice(0, 4))).toEqual([0x89, 0x50, 0x4e, 0x47]);
  });

  it("keeps webp as webp", () => {
    expect(iconBlob({ mediaType: "image/webp", data: btoa("RIFF....WEBPVP8 ") }).type).toBe(
      "image/webp",
    );
  });
});

describe("failureText: every code has a plain sentence", () => {
  it("has a full sentence for every code (the Rust test pins the union to the backend's 24)", () => {
    const codes = Object.keys(ERROR_WORDS);
    expect(codes.length).toBe(24);
    for (const code of codes) {
      const words = ERROR_WORDS[code as ModsErrorCode];
      expect(words.length).toBeGreaterThan(15);
      expect(words.endsWith(".")).toBe(true);
      expect(words).not.toBe(code);
    }
  });

  it("uses the backend's own message whenever it gives one, whatever the code", () => {
    for (const code of Object.keys(ERROR_WORDS)) {
      expect(failureText({ code, message: "The backend's words." }, "fallback")).toBe(
        "The backend's words.",
      );
    }
  });

  it("falls back to PM's sentence for the code when the message is empty or blank", () => {
    for (const code of Object.keys(ERROR_WORDS) as ModsErrorCode[]) {
      expect(failureText({ code, message: "" }, "fallback")).toBe(ERROR_WORDS[code]);
      expect(failureText({ code, message: "  " }, "fallback")).toBe(ERROR_WORDS[code]);
    }
  });

  it("a code PM doesn't know, with no message, uses the caller's fallback", () => {
    expect(failureText({ code: "from-the-future", message: "" }, "Something failed.")).toBe(
      "Something failed.",
    );
    expect(failureText({ code: "constructor", message: "" }, "Something failed.")).toBe(
      "Something failed.",
    );
  });

  it("other rejections: an Error's message, else the fallback", () => {
    expect(failureText(new Error("disk full"), "x")).toBe("disk full");
    expect(failureText(new Error(""), "x")).toBe("x");
    expect(failureText("nope", "x")).toBe("x");
  });
});

describe("pausedForDevMode", () => {
  it("is true for the standing the backend calls paused-dev-mode, and for no other", () => {
    const kinds = [
      "running",
      "waiting-for-you",
      "paused-by-you",
      "paused-frozen",
      "paused-by-safety-list",
      "removed-by-safety-list",
      "files-missing",
      "files-changed",
      "record-unreadable",
      "api-unsupported",
      "parked",
      "installing",
      "paused-other",
      "from-the-future",
    ];
    expect(pausedForDevMode({ state: { kind: "paused-dev-mode" } })).toBe(true);
    for (const kind of kinds) {
      expect(pausedForDevMode({ state: { kind: kind as never } }), kind).toBe(false);
    }
  });
});
