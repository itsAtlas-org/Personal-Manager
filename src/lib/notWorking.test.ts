// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The "not working" card's wording and decisions. The reason line, the reassurance and the offers are
// built in Rust (`offers.rs`); what is pinned here is what this file owns, plus the one string table
// the two sides must keep in step (each provider's one-line warning).

import { describe, expect, it } from "vitest";
import rust from "../../src-tauri/src/offers.rs?raw";
import { DIRECT_PROVIDERS } from "./directProviders";
import {
  announcement,
  entryOf,
  fromNowOnName,
  fromNowOnNote,
  introLine,
  limitLine,
  offerName,
  offerVia,
} from "./notWorking";
import type { NotWorkingOffer } from "./types";

function offer(over: Partial<NotWorkingOffer> = {}): NotWorkingOffer {
  return {
    key: "gemini:gemini/gemini-3.5-flash",
    provider: "gemini",
    model: "gemini/gemini-3.5-flash",
    label: "Gemini 3.5 Flash",
    company: "Google",
    data_note: "Google gets this reply.",
    kind: "other_company",
    ...over,
  };
}

describe("the buttons name the model and who gets the reply", () => {
  it("says the company and 'your key' for a direct provider, and plain names for the rest", () => {
    expect(offerName(offer())).toBe("Gemini 3.5 Flash (Google, your key)");
    expect(
      offerName(offer({ kind: "same_key", company: "Anthropic", label: "Claude Haiku 5.5" })),
    ).toBe("Claude Haiku 5.5 (Anthropic, your key)");
    expect(offerVia(offer({ kind: "openrouter", company: "OpenRouter" }))).toBe("OpenRouter");
    expect(offerVia(offer({ kind: "local", company: "this computer" }))).toBe("this computer");
    expect(offerVia(offer({ kind: "local", company: "your Local AI server" }))).toBe(
      "your Local AI server",
    );
    expect(fromNowOnName(offer())).toBe("Use Gemini 3.5 Flash from now on");
  });

  it("sends back exactly the provider and model the offer carried", () => {
    expect(entryOf(offer())).toEqual({ provider: "gemini", model: "gemini/gemini-3.5-flash" });
  });
});

describe("the lines around the buttons", () => {
  it("asks for a rephrase after a refusal, and says nothing when there is nothing to pick", () => {
    expect(introLine({ class: "busy", offers: [offer()] })).toBe(
      "Use something else for this reply:",
    );
    expect(introLine({ class: "refused", offers: [offer()] })).toBe(
      "Rephrase it, or ask another model:",
    );
    expect(introLine({ class: "busy", offers: [] })).toBeNull();
  });

  it("announces the subject and how many options", () => {
    const headline = "Claude Sonnet 5.5 isn't working right now";
    const busy = "busy";
    expect(announcement({ headline, class: busy, offers: [offer(), offer({ key: "b" })] })).toBe(
      "Claude Sonnet 5.5 isn't working. 2 options.",
    );
    expect(announcement({ headline, class: busy, offers: [offer()] })).toBe(
      "Claude Sonnet 5.5 isn't working. 1 option.",
    );
    // No options: no count clause at all.
    expect(announcement({ headline, class: busy, offers: [] })).toBe(
      "Claude Sonnet 5.5 isn't working.",
    );
  });

  it("a refusal's announcement follows its headline: the model answered, and said no", () => {
    const headline = "Claude Sonnet 5.5 didn't answer this one";
    expect(announcement({ headline, class: "refused", offers: [offer()] })).toBe(
      "Claude Sonnet 5.5 didn't answer this one. 1 option.",
    );
  });

  it("names who Chat will use for 'from now on'", () => {
    expect(fromNowOnNote(offer())).toBe("Chat will use Google from now on.");
    expect(fromNowOnNote(offer({ kind: "local", company: "this computer" }))).toBe(
      "Chat will use this computer from now on.",
    );
  });

  it("words the limit line from the figures, with two decimals", () => {
    expect(
      limitLine({
        provider: "anthropic",
        limit_usd: 5,
        spent_today_usd: 5.12,
        resumes_at: "2026-11-28T00:00:00+00:00",
      }),
    ).toBe(
      "PM has logged at least $5.12 of your $5.00 limit. It starts again at 00:00 (your day), or sooner if you raise the limit.",
    );
  });
});

/** "From now on" is one list edit, `promoteEntry("chat", provider, model)`: the offer's own provider
 *  and model are all it sends (the backend re-checks both, like any list write), whoever the company
 *  is. The ordering of several settings the old setters needed is gone. */
describe("what 'from now on' asks the backend", () => {
  it("is the offered row, for a direct model, OpenRouter and this computer alike", () => {
    expect(entryOf(offer())).toEqual({ provider: "gemini", model: "gemini/gemini-3.5-flash" });
    expect(entryOf(offer({ provider: "openrouter", model: "b/two", kind: "openrouter" }))).toEqual({
      provider: "openrouter",
      model: "b/two",
    });
    expect(entryOf(offer({ provider: "local", kind: "local", model: "llama3" }))).toEqual({
      provider: "local",
      model: "llama3",
    });
  });
});

/** The data note on an offer is Rust's copy of each provider's one-line warning (`retention` in
 *  `directProviders.ts`, the line the On battery question already shows). The two must say the same
 *  thing, so this reads the Rust source and fails if either side is edited alone. */
describe("the provider warnings Rust puts on an offer match the ones the page shows", () => {
  it.each([
    ["openai", "paid"],
    ["gemini", "paid"],
    ["gemini", "free"],
    ["anthropic", "paid"],
  ] as const)("%s (%s)", (id, tier) => {
    const line = DIRECT_PROVIDERS[id].retention[tier];
    expect(rust).toContain(JSON.stringify(line).slice(1, -1).replace(/\\"/g, '"'));
  });
});
