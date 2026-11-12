// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { iconBlob, modsErrorOf, type ModIcon } from "./modsTypes";

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
