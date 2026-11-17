// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The money prompt's sentences are placeholders until Bobby words them (plan Q8a): this holds that
// every one carries the visible marker while `WORDING_PENDING` is on, and that the facts they are
// built from pick the right sentence. When the wording is final, set the flag false and replace the
// marker test below with the final texts.

import { describe, expect, it } from "vitest";

import type { AiPromptInfo } from "../../lib/modsTypes";
import { MONEY_PROMPT_WORDS as W, PENDING_MARK, WORDING_PENDING, say } from "./aiMoneyWords";

const info = (extra: Partial<AiPromptInfo> = {}): AiPromptInfo => ({
  route: "cloud",
  paid: true,
  unpriced: false,
  model: "a/b",
  suggestedCapUsd: 1,
  ...extra,
});

describe("the money prompt's placeholder wording", () => {
  it("is marked 'wording pending: Bobby' on every sentence while it is pending", () => {
    expect(WORDING_PENDING).toBe(true);
    expect(PENDING_MARK).toBe("[wording pending: Bobby]");
    const all: string[] = [
      W.checking(),
      W.failed(),
      W.intro(),
      W.sends(),
      W.standingAuto(),
      W.capLabel(),
      W.capHelp(),
      W.capInvalid(),
      ...(["cloud", "local", "local-then-cloud", "cloud-for-power", "none"] as const).map((route) =>
        W.route(info({ route })),
      ),
      W.cost(info()),
      W.cost(info({ paid: false })),
      W.cost(info({ paid: false, unpriced: true })),
    ];
    expect(all.length).toBeGreaterThan(10);
    for (const s of all) expect(s.startsWith(PENDING_MARK + " "), s).toBe(true);
    expect(say("x")).toBe(`${PENDING_MARK} x`);
  });

  it("names the route the facts name, and the model as the person set it", () => {
    expect(W.route(info({ route: "cloud" }))).toContain("OpenRouter model (a/b)");
    expect(W.route(info({ route: "local", model: "llama" }))).toContain("local model (llama)");
    expect(W.route(info({ route: "local-then-cloud", model: "llama" }))).toContain(
      "falls back to the cloud",
    );
    expect(W.route(info({ route: "cloud-for-power" }))).toContain("On battery");
    expect(W.route(info({ route: "none", model: null }))).toContain("No AI model");
    expect(W.route(info({ route: "cloud", model: null }))).not.toContain("(");
  });

  it("says whether it can cost money from the facts, not from the route's name", () => {
    expect(W.cost(info({ paid: true }))).toContain("real money");
    expect(W.cost(info({ paid: false }))).toContain("costs nothing");
    expect(W.cost(info({ paid: false, unpriced: true }))).toContain("refused");
  });
});
