// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";

import {
  atLeast,
  detailLine,
  fmtCount,
  fmtUsd,
  partialTitle,
  unpricedWarning,
  viaLabel,
} from "./usageFormat";

describe("partialTitle", () => {
  it("says nothing for an exact figure, and names the case for a floor and for a dash", () => {
    expect(partialTitle(1.5, false)).toBeUndefined();
    expect(partialTitle(1.5, true)).toContain("at least");
    expect(partialTitle(null, true)).toBe("PM couldn't price these calls.");
  });
});

describe("atLeast", () => {
  it("is exact for an exact figure, a floor for a partial one, and a dash for none", () => {
    expect(atLeast(12.34, false)).toBe("$12.34");
    expect(atLeast(12.34, true)).toBe("at least $12.34");
    expect(atLeast(0.0042, true)).toBe("at least $0.0042");
    expect(atLeast(0, true)).toBe("at least $0.00");
    expect(atLeast(null, true)).toBe("—");
    expect(atLeast(null, false)).toBe("—");
    expect(fmtUsd(0)).toBe("$0.00");
  });
});

describe("viaLabel", () => {
  it("words every road, and a row from before PM recorded it says so", () => {
    expect(viaLabel("openrouter")).toBe("OpenRouter");
    expect(viaLabel("openai")).toBe("Your OpenAI key");
    expect(viaLabel("gemini")).toBe("Your Gemini key");
    expect(viaLabel("anthropic")).toBe("Your Anthropic key");
    expect(viaLabel("local")).toBe("This computer");
    expect(viaLabel(null)).toBe("Before PM recorded this");
  });

  it("never prints a value it doesn't know, or reaches an object's own methods", () => {
    expect(viaLabel("mistral" as never)).toBe("Before PM recorded this");
    expect(viaLabel("constructor" as never)).toBe("Before PM recorded this");
    expect(viaLabel("toString" as never)).toBe("Before PM recorded this");
  });
});

describe("detailLine", () => {
  it("names only what happened, and capitalises the first part", () => {
    expect(detailLine({ cached_tokens: 1234, thinking_tokens: 3400, refused_requests: 2 })).toBe(
      "Cached 1.2k, thinking 3.4k, 2 refused",
    );
    expect(detailLine({ cached_tokens: 0, thinking_tokens: 70, refused_requests: 0 })).toBe(
      "Thinking 70",
    );
    expect(detailLine({ cached_tokens: 0, thinking_tokens: 0, refused_requests: 1 })).toBe(
      "1 refused",
    );
    expect(detailLine({ cached_tokens: 0, thinking_tokens: 0, refused_requests: 0 })).toBeNull();
  });

  it("shortens counts without rounding a thousand to nothing", () => {
    expect(fmtCount(999)).toBe("999");
    expect(fmtCount(1000)).toBe("1k");
    expect(fmtCount(1500)).toBe("1.5k");
    expect(fmtCount(125_000)).toBe("125k");
    // Never "1000k": past 999,500 the figure is in millions.
    expect(fmtCount(999_499)).toBe("999k");
    expect(fmtCount(999_500)).toBe("1m");
    expect(fmtCount(2_340_000)).toBe("2.3m");
  });
});

describe("unpricedWarning", () => {
  it("words each slug in PM's own words, singular and plural", () => {
    expect(unpricedWarning("audio_tokens", 3)).toBe(
      "PM can't price audio tokens on 3 replies in the last 30 days, so these figures say 'at least'.",
    );
    expect(unpricedWarning("prediction_tokens", 1)).toBe(
      "PM can't price prediction tokens on 1 reply in the last 30 days, so these figures say 'at least'.",
    );
    expect(unpricedWarning("non_text_prompt", 2)).toContain("images or audio in the prompt");
    expect(unpricedWarning("server_tool_use", 2)).toContain("tool use");
    expect(unpricedWarning("new_token_type", 2)).toContain(
      "a new kind of token the provider started reporting",
    );
  });

  it("words nothing it was not given a slug for", () => {
    expect(unpricedWarning("<b>x</b>", 1)).toBeNull();
    expect(unpricedWarning("constructor", 1)).toBeNull();
  });
});
