// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The money prompt's final wording (Bobby's, 2026-11-17), and the facts that pick each sentence.
// The model sentence names the whole Background list, local models included.

import { describe, expect, it } from "vitest";

import type { AiPromptInfo } from "../../lib/modsTypes";
import { MONEY_PROMPT_WORDS as W } from "./aiMoneyWords";

const info = (extra: Partial<AiPromptInfo> = {}): AiPromptInfo => ({
  route: "cloud",
  paid: true,
  unpriced: false,
  cloudModels: ["a/cheap", "b/dear"],
  localModel: null,
  suggestedCapUsd: 1,
  ...extra,
});

const WHERE = "Settings → AI & Models";

describe("the money prompt's wording", () => {
  it("ships no placeholder mark on any sentence", () => {
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
        W.models(info({ route, localModel: "llama" })),
      ),
      W.cost(info()),
      W.cost(info({ paid: false })),
      W.cost(info({ paid: false, unpriced: true })),
    ];
    expect(all.length).toBeGreaterThan(10);
    for (const s of all) {
      expect(s, s).not.toMatch(/pending|\[|\]|—|–/);
      expect(s.trim(), s).not.toBe("");
    }
  });

  it("states the final texts of the fixed sentences", () => {
    expect(W.intro()).toBe("This mod wants to ask an AI model to do things for it.");
    expect(W.sends()).toBe(
      "PM sends the text the mod writes to that model and nothing else of yours, and sends it only to providers that keep nothing.",
    );
    expect(W.capLabel()).toBe("Monthly limit");
    expect(W.capHelp()).toBe("You can change it any time in the Mods tab.");
    expect(W.cost(info())).toBe("It can cost real money.");
    expect(W.cost(info({ paid: false }))).toBe("A local model costs nothing.");
  });

  it("names the whole Background list, local included, and says a later change applies", () => {
    expect(W.models(info({ route: "local-then-cloud", localModel: "llama" }))).toBe(
      `It uses your Background models (${WHERE}), local ones included: a/cheap, b/dear and your local model llama. It tries your local model first. If you change them, it uses the new ones.`,
    );
    expect(W.models(info())).toBe(
      `It uses your Background models (${WHERE}): a/cheap and b/dear. If you change them, it uses the new ones.`,
    );
    expect(W.models(info({ cloudModels: ["a/cheap"] }))).toBe(
      `It uses your Background model (${WHERE}): a/cheap. If you change it, it uses the new one.`,
    );
    expect(
      W.models(info({ route: "local", paid: false, cloudModels: [], localModel: "llama" })),
    ).toBe(
      `It uses your Background model (${WHERE}): your local model llama. If you change it, it uses the new one.`,
    );
  });

  it("on battery says the list, then what applies now", () => {
    const said = W.models(info({ route: "cloud-for-power", localModel: "llama" }));
    expect(said).toContain("a/cheap, b/dear and your local model llama");
    expect(said).toContain("You are on battery, so right now it uses only the cloud ones.");
    expect(said.indexOf("local model llama")).toBeLessThan(said.indexOf("on battery"));
  });

  it("says plainly when no Background model is set up", () => {
    const none = info({ route: "none", paid: false, cloudModels: [], localModel: null });
    expect(W.models(none)).toBe(
      `No Background model is set up yet, so its requests fail until you choose one in ${WHERE}.`,
    );
  });

  it("says whether it can cost money from the facts, not from the route's name", () => {
    expect(W.cost(info({ paid: true }))).toContain("real money");
    expect(W.cost(info({ paid: false }))).toContain("costs nothing");
    expect(W.cost(info({ paid: false, unpriced: true }))).toContain("refused");
  });
});
