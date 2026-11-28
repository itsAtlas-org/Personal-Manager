// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The "not working" card's wording and decisions. The reason line, the reassurance and the offers are
// built in Rust (`offers.rs`); what is pinned here is what this file owns, plus the one string table
// the two sides must keep in step (each provider's one-line warning).

import { describe, expect, it, vi } from "vitest";
import rust from "../../src-tauri/src/offers.rs?raw";
import { DIRECT_PROVIDERS } from "./directProviders";
import {
  announcement,
  entryOf,
  fromNowOnName,
  fromNowOnNote,
  fromNowOnSteps,
  introLine,
  limitLine,
  offerName,
  offerVia,
  runFromNowOn,
  type FromNowOnSetters,
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

describe("what 'from now on' changes, with the setters that already exist", () => {
  it("writes a direct model, then moves Chat onto the provider", () => {
    expect(fromNowOnSteps(offer(), ["x/y"])).toEqual([
      { kind: "directRole", provider: "gemini", model: "gemini/gemini-3.5-flash" },
      { kind: "roleProvider", provider: "gemini" },
    ]);
  });

  it("puts the OpenRouter model first and keeps the others behind it, once", () => {
    const or = offer({ provider: "openrouter", model: "b/two", kind: "openrouter" });
    // The list is written before the role moves to OpenRouter, as for a direct model.
    expect(fromNowOnSteps(or, ["a/one", "b/two", "c/three"])).toEqual([
      { kind: "chatModels", models: ["b/two", "a/one", "c/three"] },
      { kind: "roleProvider", provider: "openrouter" },
    ]);
  });

  it("makes this computer Chat's routing", () => {
    expect(
      fromNowOnSteps(offer({ provider: "local", kind: "local", model: "llama3" }), []),
    ).toEqual([{ kind: "localRouting" }]);
  });
});

describe("running 'from now on' with the setters", () => {
  function setters(over: Partial<FromNowOnSetters> = {}) {
    const calls: string[] = [];
    const s: FromNowOnSetters = {
      setDirectRole: vi.fn(async (p, role, models, effort) => {
        calls.push(`directRole ${p} ${role} ${models.join(",")} ${effort}`);
      }),
      setRoleCloudProvider: vi.fn(async (role, p) => {
        calls.push(`roleProvider ${role} ${p}`);
      }),
      setChatModels: vi.fn(async (m) => {
        calls.push(`chatModels ${m.join(",")}`);
      }),
      setLocalLlmRouting: vi.fn(async (role, pref) => {
        calls.push(`localRouting ${role} ${pref}`);
      }),
      chatEffortFor: vi.fn(async () => null),
      ...over,
    };
    return { s, calls };
  }

  it("writes the model, then moves the CHAT role (never Background), for a direct model", async () => {
    const { s, calls } = setters();
    await runFromNowOn(fromNowOnSteps(offer(), []), s);
    expect(calls).toEqual([
      "directRole gemini chat gemini/gemini-3.5-flash null",
      "roleProvider chat gemini",
    ]);
  });

  it("keeps an effort the person already set on that very model", async () => {
    const { s, calls } = setters({ chatEffortFor: vi.fn(async () => "high") });
    await runFromNowOn(fromNowOnSteps(offer(), []), s);
    expect(calls[0]).toBe("directRole gemini chat gemini/gemini-3.5-flash high");
    // An unreadable state is the role's default, never a failed click.
    const bad = setters({
      chatEffortFor: vi.fn(async () => {
        throw new Error("no");
      }),
    });
    await runFromNowOn(fromNowOnSteps(offer(), []), bad.s);
    expect(bad.calls[0]).toBe("directRole gemini chat gemini/gemini-3.5-flash null");
  });

  it("OpenRouter writes the list then the role; this computer sets Chat's routing", async () => {
    const a = setters();
    await runFromNowOn(
      fromNowOnSteps(offer({ provider: "openrouter", model: "b/two", kind: "openrouter" }), [
        "a/one",
      ]),
      a.s,
    );
    expect(a.calls).toEqual(["chatModels b/two,a/one", "roleProvider chat openrouter"]);
    const b = setters();
    await runFromNowOn(fromNowOnSteps(offer({ provider: "local", kind: "local" }), []), b.s);
    expect(b.calls).toEqual(["localRouting chat local"]);
  });

  it("the first failure stops the rest and rejects", async () => {
    const { s, calls } = setters({
      setDirectRole: vi.fn(async () => {
        throw new Error("Gemini is paused");
      }),
    });
    await expect(runFromNowOn(fromNowOnSteps(offer(), []), s)).rejects.toThrow("Gemini is paused");
    expect(calls).toEqual([]);
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
