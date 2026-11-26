// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";

import appSource from "../App.tsx?raw";

import {
  adoptRoles,
  chatUpgradeApplies,
  directInfo,
  directPickerRows,
  directReady,
  effortAfterModelChange,
  effortCoercedLine,
  effortLabel,
  effortOptions,
  effortToWrite,
  eligibleProviders,
  footerRow,
  isEligible,
  flippedLine,
  keyVerdict,
  movedLine,
  notThereLine,
  offersRecheck,
  offersRecheckFor,
  pausedText,
  readsFree,
  recheckStores,
  refusalText,
  routeLabel,
  routeTitle,
  termsFor,
  tierBadge,
  pickerCaption,
  providerName,
  providerOptionLabel,
  recheckVerdict,
  roleDefaultEffort,
  roleIssue,
  roleIssueLine,
  routeNote,
  storedVerdict,
  unavailableLabel,
  wizardModelsLine,
} from "./directProviders";
import {
  directState,
  FLASH,
  FLASH_LITE,
  geminiView,
  LUNA,
  openaiView,
  PRO,
  settingsFor,
  SOL,
  stateWith,
  TERRA,
} from "./directProvidersFixture";
import type { RegionRefusal } from "./types";

describe("effort levels", () => {
  it("names every level the catalogue can carry, and Title-cases one it has not met", () => {
    for (const [level, label] of [
      ["none", "Off"],
      ["minimal", "Minimal"],
      ["low", "Low"],
      ["medium", "Medium"],
      ["high", "High"],
      ["xhigh", "Extra high"],
      ["max", "Max"],
    ] as const) {
      expect(effortLabel(level)).toBe(label);
    }
    // A catalogue row from a newer PM can ship a level this version has no word for: never a throw.
    expect(effortLabel("ultra_deep")).toBe("Ultra deep");
    expect(effortLabel("")).toBe("");
    // Not the object's own prototype keys either.
    expect(effortLabel("constructor")).toBe("Constructor");
  });

  it("starts chat on the model's default and background on Low, or the lowest level it has", () => {
    expect(roleDefaultEffort("chat", SOL)).toBeNull();
    expect(roleDefaultEffort("background", LUNA)).toBe("low");
    expect(roleDefaultEffort("background", TERRA)).toBe("medium");
    expect(roleDefaultEffort("background", undefined)).toBeNull();
  });

  it("offers Model default to chat only, and marks background's own default", () => {
    expect(effortOptions("chat", SOL).map((o) => o.label)).toEqual([
      "Model default",
      "Low",
      "Medium",
      "High",
      "Extra high",
      "Max",
    ]);
    const bg = effortOptions("background", LUNA);
    expect(bg.map((o) => o.label)).toEqual(["Off", "Low (PM's default)", "Medium", "High"]);
    expect(bg.some((o) => o.value === "")).toBe(false);
    expect(effortOptions("chat", undefined)).toEqual([]);
  });

  it("keeps OpenAI's chat option worded as it always was, even for a row with its own default", () => {
    expect(TERRA.effort_default).toBe("medium");
    expect(effortOptions("chat", TERRA)[0]).toEqual({ value: "", label: "Model default" });
  });

  it("writes the role default as null, so a role on its default keeps tracking it", () => {
    expect(effortToWrite("chat", SOL, "")).toBeNull();
    expect(effortToWrite("chat", SOL, "high")).toBe("high");
    expect(effortToWrite("background", LUNA, "low")).toBeNull();
    expect(effortToWrite("background", LUNA, "high")).toBe("high");
  });

  it("keeps a chosen level the new model offers, and reports one it does not", () => {
    expect(effortAfterModelChange("chat", "high", SOL, LUNA)).toEqual({
      effort: "high",
      coercedFrom: null,
    });
    expect(effortAfterModelChange("chat", "max", SOL, LUNA)).toEqual({
      effort: null,
      coercedFrom: "max",
    });
    // A role on its default stays on it, and says nothing.
    expect(effortAfterModelChange("background", "low", LUNA, TERRA)).toEqual({
      effort: null,
      coercedFrom: null,
    });
    expect(effortAfterModelChange("chat", null, SOL, LUNA)).toEqual({
      effort: null,
      coercedFrom: null,
    });
    expect(effortCoercedLine("chat", LUNA, "max")).toBe(
      "Effort is now Model default. GPT-6 Luna doesn't have Max.",
    );
    expect(effortCoercedLine("background", TERRA, "low")).toBe(
      "Effort is now Medium. GPT-6.1 Terra doesn't have Low.",
    );
  });
});

describe("provider words", () => {
  it("names a provider, and falls back to the raw word for one it has no card for", () => {
    expect(providerName("openrouter")).toBe("OpenRouter");
    expect(providerName("openai")).toBe("OpenAI");
    expect(providerName("gemini")).toBe("Google Gemini");
    // A provider this version has no card for is still shown as its own word.
    expect(providerName("anthropic")).toBe("anthropic");
    expect(providerOptionLabel("openrouter")).toBe("OpenRouter (asks for zero data retention)");
    expect(providerOptionLabel("openai")).toBe("OpenAI, direct (no zero data retention)");
  });
});

describe("what a key check says", () => {
  it("words every outcome, and counts models", () => {
    expect(keyVerdict("ok", 4, "OpenAI")).toEqual({
      tone: "ok",
      text: "Key works. 4 OpenAI models are ready to use.",
    });
    expect(keyVerdict("ok", 1, "OpenAI").text).toContain(
      "Key works. 1 OpenAI model is ready to use.",
    );
    expect(keyVerdict("restricted", 0, "OpenAI")).toMatchObject({ tone: "warn" });
    expect(keyVerdict("restricted", 0, "OpenAI").text).toMatch(/^Saved\. This key is restricted/);
    for (const o of ["rejected", "region_blocked", "malformed", "unreachable"] as const) {
      const v = keyVerdict(o, 0, "OpenAI");
      expect(v.tone).toBe("refused");
      expect(v.text).toMatch(/^Not saved\./);
    }
    expect(keyVerdict("rejected", 0, "OpenAI").text).toContain("Admin key");
    expect(keyVerdict({ unexpected: 418 }, 0, "OpenAI").text).toBe(
      "Not saved. OpenAI answered with an error (418). Try again shortly.",
    );
  });

  it("says a key with no credit is saved, and what to do", () => {
    expect(keyVerdict("no_credit", 4, "OpenAI")).toEqual({
      tone: "warn",
      text: "Saved. Your OpenAI account is out of credit or at a spend limit, so chats won't work until you add credit or raise the limit at platform.openai.com. Then press Check again.",
    });
  });

  it("words a key that lists models but can't chat, and one that can do neither", () => {
    expect(keyVerdict("restricted", 4, "OpenAI", "read_only")).toEqual({
      tone: "warn",
      text: "Saved. This key can list models but can't chat. Give it access to chat in your OpenAI project, then press Check again.",
    });
    expect(keyVerdict("restricted", 0, "OpenAI", "no_access").text).toMatch(
      /^Saved\. This key can't list models or chat\..*press Check again\.$/,
    );
  });

  it("adds the chat-not-tried note to the listing's verdict, never a failure or no credit", () => {
    const v = keyVerdict("ok", 4, "OpenAI", "chat_untried");
    // A caveat is drawn as a warning, not a green tick.
    expect(v.tone).toBe("warn");
    expect(keyVerdict("ok", 4, "OpenAI").tone).toBe("ok");
    expect(v.text).toBe(
      "Key works. 4 OpenAI models are ready to use. PM couldn't try a chat just now.",
    );
    expect(keyVerdict("restricted", 0, "OpenAI", "chat_untried").text).toMatch(
      /PM couldn't try a chat just now\.$/,
    );
    expect(v.text).not.toMatch(/credit/);
  });

  it("offers Check again for no credit, a restricted key and an untried chat, not a plain working key", () => {
    expect(offersRecheck("no_credit")).toBe(true);
    expect(offersRecheck("restricted")).toBe(true);
    expect(offersRecheck("ok", "chat_untried")).toBe(true);
    expect(offersRecheck("ok")).toBe(false);
    expect(offersRecheck("ok", null)).toBe(false);
    expect(offersRecheck(undefined)).toBe(false);
  });

  it("drops 'Saved.' when a key that is already saved is checked again", () => {
    expect(recheckVerdict("no_credit", 4, "OpenAI").text).toMatch(
      /^Your OpenAI account is out of credit or at a spend limit/,
    );
    expect(recheckVerdict("restricted", 4, "OpenAI", "read_only").text).toMatch(
      /^This key can list models/,
    );
    expect(keyVerdict("no_credit", 4, "OpenAI").text).toMatch(/^Saved\. /);
  });

  it("says which answers a re-check stores, so trouble reaching the provider keeps the old verdict", () => {
    for (const o of ["ok", "restricted", "no_credit", "rejected", "region_blocked"] as const) {
      expect(recheckStores(o), o).toBe(true);
    }
    for (const o of ["unreachable", "malformed", { unexpected: 502 }] as const) {
      expect(recheckStores(o)).toBe(false);
    }
  });

  it("tells a refusal of a SAVED key without saying 'Not saved'", () => {
    for (const o of ["rejected", "region_blocked", "unreachable"] as const) {
      const v = recheckVerdict(o, 0, "OpenAI");
      expect(v.tone).toBe("refused");
      expect(v.text).not.toMatch(/Not saved/);
    }
    expect(recheckVerdict("ok", 4, "OpenAI")).toEqual(keyVerdict("ok", 4, "OpenAI"));
  });

  it("keeps a key with no credit eligible, and the picker's list from its listing", () => {
    const noCredit = openaiView({
      key_check: {
        at: "x",
        outcome: "no_credit",
        listed: ["openai/gpt-6.2-sol"],
        listed_total: 90,
      },
    });
    expect(isEligible(noCredit)).toBe(true);
    expect(directPickerRows(noCredit).find((r) => r.id === "openai/gpt-6-luna")?.unlisted).toBe(
      true,
    );
    expect(pickerCaption(noCredit)).toBe("Models your OpenAI key can use.");
    // A no-credit answer reached through a refused listing has no list to show.
    const blind = openaiView({
      key_check: { at: "x", outcome: "no_credit", listed: [], listed_total: 0 },
    });
    expect(directPickerRows(blind).some((r) => r.unlisted)).toBe(false);
    expect(pickerCaption(blind)).toBe("PM couldn't check which of these your key can use.");
  });

  it("names a role on a key with no credit, as its own issue", () => {
    const state = directState({
      view: openaiView({
        key_check: { at: "x", outcome: "no_credit", listed: [], listed_total: 0 },
      }),
      chat: "openai",
    });
    expect(roleIssue(state, "chat")).toBe("no_credit");
    expect(roleIssue(state, "background")).toBeNull();
    expect(roleIssueLine("no_credit", "chat", "OpenAI")).toBe(
      "Your OpenAI account is out of credit or at a spend limit, so this won't answer until you add credit or raise the limit.",
    );
  });

  it("shows an outcome it has no sentence for as itself, and counts it as not saved", () => {
    const v = keyVerdict("tier_blocked" as never, 0, "OpenAI");
    expect(v.tone).toBe("refused");
    expect(v.text).toContain("tier_blocked");
  });

  it("draws a stored verdict only beside a key that is really there", () => {
    expect(storedVerdict(openaiView(), "OpenAI")?.tone).toBe("ok");
    // A verdict carried in a backup, on a device with no key.
    expect(storedVerdict(openaiView({ key: "absent" }), "OpenAI")).toBeNull();
    expect(storedVerdict(openaiView({ key: "unreadable" }), "OpenAI")).toBeNull();
    expect(storedVerdict(openaiView({ key_check: null }), "OpenAI")).toBeNull();
  });
});

describe("who can be chosen, and what is wrong with a stored choice", () => {
  it("offers a provider only when it is on, keyed and not turned away", () => {
    expect(eligibleProviders(directState())).toHaveLength(1);
    expect(eligibleProviders(directState({ view: openaiView({ opted_in: false }) }))).toHaveLength(
      0,
    );
    expect(eligibleProviders(directState({ view: openaiView({ key: "absent" }) }))).toHaveLength(0);
    expect(
      eligibleProviders(
        directState({
          view: openaiView({
            key_check: { at: "x", outcome: "rejected", listed: [], listed_total: 0 },
          }),
        }),
      ),
    ).toHaveLength(0);
  });

  it("finds the reason a role cannot be answered, and words it", () => {
    expect(roleIssue(directState(), "chat")).toBeNull();
    expect(roleIssue(directState({ chat: "openai" }), "chat")).toBeNull();
    const off = directState({ chat: "openai", chatEffective: "openrouter" });
    expect(roleIssue(off, "chat")).toBe("turned_off");
    expect(roleIssueLine("turned_off", "chat", "OpenAI")).toBe("OpenAI is turned off above.");
    const missing = directState({ chat: "openai", view: openaiView({ key: "absent" }) });
    expect(roleIssue(missing, "chat")).toBe("key_missing");
    expect(roleIssueLine("key_missing", "chat", "OpenAI")).toBe(
      "Your OpenAI key is missing on this device. Add it above, or move Chat back to OpenRouter.",
    );
    const unreadable = directState({
      background: "openai",
      view: openaiView({ key: "unreadable" }),
    });
    expect(roleIssue(unreadable, "background")).toBe("key_unreadable");
    expect(roleIssueLine("key_unreadable", "background", "OpenAI")).toBe(
      "Background is set to OpenAI, but PM can't read your OpenAI key right now. Background won't answer until it can, or until you pick another provider.",
    );
  });

  it("labels the stored option when it cannot be chosen", () => {
    expect(
      unavailableLabel(
        "openai",
        directState({ chat: "openai", chatEffective: "openrouter" }),
        "chat",
      ),
    ).toBe("OpenAI (turned off)");
    expect(
      unavailableLabel(
        "openai",
        directState({ chat: "openai", view: openaiView({ key: "absent" }) }),
        "chat",
      ),
    ).toBe("OpenAI (no key)");
    expect(
      unavailableLabel(
        "openai",
        directState({ chat: "openai", view: openaiView({ key: "unreadable" }) }),
        "chat",
      ),
    ).toBe("OpenAI (key unreadable)");
  });
});

describe("what moved, and where a role's route goes", () => {
  const move = (role: "chat" | "background") => ({
    role,
    from: "openai" as const,
    to: "openrouter" as const,
  });

  it("says which roles went back, and warns when there is no OpenRouter key to land on", () => {
    const withKey = { chat: true, background: true };
    expect(movedLine([], withKey)).toBeNull();
    expect(movedLine([move("chat"), move("background")], withKey)).toBe(
      "Chat and Background are back on OpenRouter.",
    );
    expect(movedLine([move("chat")], withKey)).toBe("Chat is back on OpenRouter.");
    expect(movedLine([move("background")], withKey)).toBe("Background is back on OpenRouter.");
    expect(movedLine([move("chat")], { chat: false, background: true })).toBe(
      "Chat is back on OpenRouter, but there's no OpenRouter key yet. Add one above, or choose another provider.",
    );
  });

  it("words the note under a direct role by its routing", () => {
    expect(routeNote("chat", "OpenAI", "cloud")).toBe(
      "Chat goes straight to OpenAI with your key. It isn't zero data retention.",
    );
    expect(routeNote("background", "OpenAI", "cloud")).toContain("sends parts of your documents");
    expect(routeNote("chat", "OpenAI", "local-then-cloud")).toBe(
      "OpenAI answers if your local model can't. That isn't zero data retention.",
    );
    expect(routeNote("background", "OpenAI", "local-then-cloud")).toContain(
      "answers background work if your local model can't",
    );
    expect(routeNote("chat", "OpenAI", "local")).toBe(
      "This job runs on your local model. OpenAI is only used if you switch it to a cloud routing.",
    );
  });
});

describe("the picker's rows", () => {
  it("turns per-million prices into per-token, and flags what the key's list leaves out", () => {
    const rows = directPickerRows(openaiView());
    const sol = rows.find((r) => r.id === SOL.id)!;
    expect(sol.prompt_price).toBeCloseTo(2 / 1_000_000, 12);
    expect(sol.completion_price).toBeCloseTo(10 / 1_000_000, 12);
    expect(sol.unlisted).toBe(false);
    expect(rows.find((r) => r.id === TERRA.id)!.unlisted).toBe(true);
    expect(sol.tip).toContain("cached input $0.10 per million tokens");
    expect(sol.tip).toContain("26-11-2026");
  });

  it("flags nothing when the key's list was never read, and says so in the caption", () => {
    const restricted = openaiView({
      key_check: { at: "x", outcome: "restricted", listed: [], listed_total: 0 },
    });
    expect(directPickerRows(restricted).some((r) => r.unlisted)).toBe(false);
    expect(pickerCaption(restricted)).toBe("PM couldn't check which of these your key can use.");
    expect(pickerCaption(openaiView())).toBe("Models your OpenAI key can use.");
  });

  it("shows a retirement date as DD-MM-YYYY", () => {
    const v = openaiView({ models: [{ ...SOL, retires_on: "2027-03-09" }] });
    expect(directPickerRows(v)[0].retires).toBe("09-03-2027");
  });
});

describe("the first-run wizard", () => {
  it("is ready with only a direct key once both roles resolve to a provider with a key", () => {
    expect(directReady(directState({ chat: "openai", background: "openai" }), false)).toBe(true);
    // A role still on OpenRouter with no OpenRouter key is a dead role.
    expect(directReady(directState({ chat: "openai" }), false)).toBe(false);
    expect(directReady(directState({ chat: "openai" }), true)).toBe(true);
    expect(directReady(null, true)).toBe(false);
    expect(
      directReady(
        directState({
          chat: "openai",
          background: "openai",
          view: openaiView({ opted_in: false }),
        }),
        false,
      ),
    ).toBe(false);
  });

  it("names the models that will answer", () => {
    const v = openaiView();
    expect(wizardModelsLine(v, true, true)).toBe(
      "PM will use GPT-6.2 Sol for chat and GPT-6 Luna for background work. You can change that later in Settings > AI & Models.",
    );
    expect(wizardModelsLine(v, true, false)).toContain("GPT-6.2 Sol for chat");
    expect(wizardModelsLine(v, false, false)).toBeNull();
  });
});

describe("the sidebar footer", () => {
  it("names the model that answers, not the head of OpenRouter's list, for a direct role", () => {
    const s = settingsFor({
      chat_provider: "openai",
      chat_cloud_model: "openai/gpt-6.2-sol",
    });
    // chat_models[0] is still Claude: the footer must not say so.
    expect(footerRow(s, "chat")).toEqual({
      id: "openai/gpt-6.2-sol",
      fallbacks: 0,
      via: "OpenAI direct",
      viaTitle: null,
      viaDue: false,
    });
    // The other role is untouched.
    expect(footerRow(s, "background")).toEqual({
      id: "google/gemini-2.5-flash",
      fallbacks: 0,
      via: null,
      viaTitle: null,
      viaDue: false,
    });
  });

  it("counts auto-switch fallbacks on OpenRouter as it always did", () => {
    const none = { viaTitle: null, viaDue: false };
    expect(footerRow(settingsFor(), "chat")).toEqual({
      id: "anthropic/claude-sonnet-4.6",
      fallbacks: 1,
      via: null,
      ...none,
    });
    expect(footerRow(null, "chat")).toEqual({ id: null, fallbacks: 0, via: null, ...none });
  });

  it("says a Gemini route's terms from Settings, due-tinted when it is free or paused", () => {
    const on = (terms: "paid" | "free" | "blocked" | null | undefined) =>
      footerRow(
        settingsFor({
          chat_provider: "gemini",
          chat_cloud_model: "gemini/gemini-3.8-flash",
          chat_route_terms: terms,
        }),
        "chat",
      );
    expect(on("paid")).toMatchObject({ via: "Gemini direct, paid", viaDue: false });
    expect(on("free")).toMatchObject({ via: "Gemini direct, free", viaDue: true });
    expect(on("blocked")).toMatchObject({ via: "Gemini paused", viaDue: true });
    // A Settings that predates the field reads as free: Unknown is free.
    expect(on(undefined)).toMatchObject({ via: "Gemini direct, free", viaDue: true });
    // Rust's `null` is a role on Gemini with no key: nothing is known about the terms, so the footer
    // says only where the route goes (never "free", never the due tint).
    expect(on(null)).toEqual({
      id: "gemini/gemini-3.8-flash",
      fallbacks: 0,
      via: "Gemini direct",
      viaTitle: null,
      viaDue: false,
    });
    expect(on("paid").viaTitle).toBe(
      "Chat goes straight to Google Gemini with your own key, on Google's paid terms. It isn't zero data retention. Change it in Settings > AI & Models.",
    );
    expect(on("free").viaTitle).toMatch(/train on what you send and people at Google may read it/);
    // The key may be paid and merely unchecked: the title says PM treats it as free.
    expect(on("free").viaTitle).toMatch(/a key PM treats as free\. If it is free/);
    expect(on("paid").viaTitle).not.toMatch(/free/i);
  });
});

describe("the context meter's larger-model switch", () => {
  it("rewrites chat's list only while chat is on OpenRouter", () => {
    expect(chatUpgradeApplies(settingsFor())).toBe(true);
    expect(chatUpgradeApplies(null)).toBe(true);
    expect(chatUpgradeApplies(settingsFor({ chat_provider: "openai" }))).toBe(false);
    // A word this version has no card for is not OpenRouter either.
    expect(chatUpgradeApplies(settingsFor({ chat_provider: "gemini" }))).toBe(false);
  });

  it("is what App's handleUpgrade asks before it writes anything", () => {
    // App is too large to mount here, so pin the wiring in its text: the guard comes first, ahead of
    // the first write of the OpenRouter list.
    const body = appSource.slice(appSource.indexOf("function handleUpgrade("));
    const guard = body.indexOf("if (!chatUpgradeApplies(settings)) return Promise.resolve();");
    expect(guard).toBeGreaterThan(-1);
    expect(guard).toBeLessThan(body.indexOf("setChatModels("));
  });
});

// ---- Google Gemini: the tier is a second axis, and the UI only words what Rust decided -----------

const UK: RegionRefusal = {
  kind: "uk",
  label: "the United Kingdom",
  zone: "Europe/London",
  basis: "free",
};
const UNPLACED: RegionRefusal = { kind: "unplaceable", label: "x", zone: "UTC", basis: "free" };
const gemini = () => directInfo("gemini")!;
const freeView = (over = {}) =>
  geminiView({
    tier: "free",
    key_check: {
      at: "x",
      outcome: "ok",
      listed: [FLASH.id],
      listed_total: 49,
      tier: "free",
      flipped_free_at: null,
    },
    ...over,
  });

describe("Gemini's warning, by verdict", () => {
  it("says the free-tier rule before any key is pasted, and never claims zero data retention", () => {
    const t = termsFor(gemini(), geminiView({ key: "absent", key_check: null, tier: null }));
    expect(t.tone).toBe("warning");
    expect(t.headline).toBe("This isn't zero data retention.");
    expect(t.points.join(" ")).toMatch(/free Gemini key lets Google train/);
    expect(t.points.join(" ")).toMatch(
      /UK, the EU, Iceland, Liechtenstein, Norway and Switzerland/,
    );
  });

  it("words paid, free, unknown and unchecked differently, danger for all but paid", () => {
    const paid = termsFor(gemini(), geminiView());
    expect(paid.tone).toBe("warning");
    expect(paid.points[0]).toMatch(/doesn't use what you send to improve its products/);
    const free = termsFor(gemini(), freeView());
    expect(free.tone).toBe("danger");
    expect(free.headline).toMatch(/train on what you send, and people at Google may read it/);
    const unknown = termsFor(gemini(), geminiView({ tier: "unknown" }));
    expect(unknown.tone).toBe("danger");
    expect(unknown.points[0]).toMatch(/couldn't tell whether this key is free or paid/);
    // A key with NO usable verdict says nothing was asked, which is not "couldn't tell".
    const none = termsFor(gemini(), geminiView({ tier: null }));
    expect(none.tone).toBe("danger");
    expect(none.headline).toBe("PM hasn't checked this key, so it treats it as free.");
    expect(JSON.stringify(none)).not.toMatch(/couldn't tell/);
    for (const t of [paid, free, unknown, none]) {
      expect(JSON.stringify(t)).not.toMatch(/sends zero data retention/);
    }
  });

  it("keeps OpenAI's one warning and its shared headline", () => {
    const o = directInfo("openai")!;
    expect(termsFor(o, openaiView()).headline).toBe("This isn't zero data retention.");
    expect(termsFor(o, openaiView({ shares_data: true })).headline).toBe(
      "OpenAI may train on what you send.",
    );
  });
});

describe("the tier badge", () => {
  it("is real words with a decorative glyph, one per verdict", () => {
    expect(tierBadge(gemini(), geminiView())).toEqual({
      glyph: "✓",
      text: "Paid key",
      tone: "ok",
    });
    expect(tierBadge(gemini(), freeView())).toMatchObject({ glyph: "⚠", text: "Free key" });
    expect(tierBadge(gemini(), geminiView({ tier: "unknown" }))?.text).toBe(
      "Free key (PM couldn't tell)",
    );
    // A present key with no verdict for it is "not checked", never "couldn't tell".
    expect(tierBadge(gemini(), geminiView({ tier: null }))?.text).toBe(
      "Not checked yet (treated as free)",
    );
  });

  it("adds paused or out of credit, and is absent without a key or without tiers", () => {
    expect(tierBadge(gemini(), freeView({ blocked: UK }))?.text).toBe("Free key (paused)");
    const empty = geminiView({
      key_check: { at: "x", outcome: "no_credit", listed: [], listed_total: 0, tier: "paid" },
    });
    expect(tierBadge(gemini(), empty)?.text).toBe("Paid key (out of credit)");
    expect(tierBadge(gemini(), geminiView({ key: "absent" }))).toBeNull();
    expect(tierBadge(directInfo("openai")!, openaiView())).toBeNull();
  });
});

describe("what a Gemini key check says", () => {
  const v = (outcome: Parameters<typeof keyVerdict>[0], tier?: "paid" | "free" | "unknown") =>
    keyVerdict(outcome, 3, "Google Gemini", undefined, { info: gemini(), tier });

  it("words a paid key as working and on paid terms", () => {
    expect(v("ok", "paid")).toEqual({
      tone: "ok",
      text: "Key works. It's on Google's paid terms. 3 Gemini models are ready to use.",
    });
    expect(
      keyVerdict("ok", 1, "Google Gemini", undefined, { info: gemini(), tier: "paid" }).text,
    ).toContain("1 Gemini model is ready to use.");
    expect(
      keyVerdict("ok", 0, "Google Gemini", undefined, { info: gemini(), tier: "paid" }).text,
    ).toMatch(/none of the models PM knows are in its list/);
  });

  it("warns on a free or unknown key outside the region, and never draws a tick", () => {
    expect(v("ok", "free")).toEqual({
      tone: "warn",
      text: `Saved. This key is on Google's free tier, so Google may train on what you send. 3 Gemini models are ready, but models marked "Needs a paid key" won't work on it.`,
    });
    expect(v("ok", "unknown")).toEqual({
      tone: "warn",
      text: "Saved. PM couldn't tell whether this key is free or paid, so it treats it as free: Google may train on what you send.",
    });
    // A tier that never arrived reads as Unknown.
    expect(v("ok").text).toMatch(/couldn't tell/);
  });

  it("words an empty prepay balance with the word the backend uses, no_credit", () => {
    expect(v("no_credit").text).toBe(
      "Saved. This key is paid, but its prepay balance is empty, so Google has stopped it. Add credit at aistudio.google.com/billing, then press Check again.",
    );
    // And OpenAI's own sentence is unchanged.
    expect(keyVerdict("no_credit", 3, "OpenAI").text).toMatch(/platform\.openai\.com/);
  });

  it("refuses with Google's reasons, and says busy is not a connection problem", () => {
    expect(v("needs_verification")).toEqual({
      tone: "refused",
      text: "Not saved. Google wants this project verified in AI Studio before the key can be used. Verify it, then press Save and check again.",
    });
    expect(v("rejected").text).toMatch(/^Not saved\. Google says this key isn't right\./);
    expect(v("rejected").text).toMatch(/leaked keys, old keys with no restrictions/);
    expect(v("region_blocked").text).toMatch(/doesn't offer free Gemini keys where you are/);
    const busy = v("busy");
    expect(busy.tone).toBe("refused");
    expect(busy.text).toBe(
      "Google was busy or limiting requests, so PM couldn't check the key. Nothing was decided. Wait a minute and press Save and check again.",
    );
    expect(busy.text).not.toMatch(/connection/);
    expect(v("unreachable").text).toBe(
      "Not saved. PM couldn't reach Google to check it. Check your connection and try again.",
    );
    expect(v("free_key_refused")).toEqual({
      tone: "refused",
      text: "Not saved. It's a free key and PM only uses paid keys where you are.",
    });
  });

  it("shows a word it has no sentence for as itself", () => {
    expect(v("brand_new_word" as never).text).toContain("brand_new_word");
  });

  it("keeps a re-check neutral beside a pause, and drops 'Saved.' otherwise", () => {
    const r = recheckVerdict("ok", 3, "Google Gemini", undefined, {
      info: gemini(),
      tier: "free",
      refusal: UK,
    });
    expect(r).toEqual({ tone: "warn", text: "Checked: this key still reads as free." });
    expect(r.text).not.toMatch(/Saved/);
    const plain = recheckVerdict("ok", 3, "Google Gemini", undefined, {
      info: gemini(),
      tier: "free",
    });
    expect(plain.text).toMatch(/^This key is on Google's free tier/);
    expect(recheckVerdict("busy", 0, "Google Gemini", undefined, { info: gemini() }).text).toMatch(
      /press Check again\.$/,
    );
  });

  it("stores a verification answer on re-check and never a busy one", () => {
    expect(recheckStores("needs_verification")).toBe(true);
    expect(recheckStores("busy")).toBe(false);
  });

  it("hides a stored verdict that isn't about THIS key", () => {
    // A carried row beside a different key reads as no verdict: no "Key works" about another key.
    expect(storedVerdict(geminiView({ tier: null }), "Google Gemini")).toBeNull();
    expect(storedVerdict(geminiView(), "Google Gemini")?.text).toMatch(/paid terms/);
    expect(storedVerdict(freeView({ blocked: UK }), "Google Gemini")?.text).toBe(
      "Checked: this key still reads as free.",
    );
  });
});

describe("the paused and refused sentences", () => {
  it("names where PM thinks you are and what it read", () => {
    expect(pausedText(gemini(), UK, "free")).toBe(
      "Gemini is paused. Your key reads as free, and PM thinks you're in the United Kingdom (it read your computer's time zone as Europe/London). Google's terms only allow paid keys there, so PM won't use it. Make the project paid in AI Studio, then press Check again.",
    );
    expect(pausedText(gemini(), UNPLACED, "free")).toBe(
      "Gemini is paused. Your key reads as free, and PM can't tell where this computer is (its time zone reads as UTC), so it only uses paid keys. Set your computer's time zone, or make the project paid in AI Studio, then press Check again.",
    );
    expect(pausedText(gemini(), { ...UNPLACED, zone: "" }, "free")).toContain("reads as unknown");
  });

  it("leads with Google's own word when it told PM the project is free", () => {
    expect(pausedText(gemini(), UK, "free", "2026-11-26T10:00:00Z")).toMatch(
      /^Google told PM on 26-11-2026 that this project is on the free tier\. Gemini is paused\./,
    );
    expect(flippedLine("2026-11-26T10:00:00Z")).toBe(
      "Google told PM on 26-11-2026 that this project is now on the free tier, so Google may use what you send to improve its products. Make the project paid in AI Studio, then press Check again.",
    );
  });

  it("says 'hasn't checked' for no verdict and 'couldn't tell' for an unknown one", () => {
    const unknown: RegionRefusal = { ...UK, basis: "unknown" };
    expect(pausedText(gemini(), unknown, null)).toContain(
      "PM hasn't checked your key, so it treats it as free, and PM thinks you're in",
    );
    expect(pausedText(gemini(), unknown, "unknown")).toContain(
      "PM couldn't tell whether your key is free or paid, so it treats it as free, and",
    );
  });

  it("tells a typed free key it wasn't saved, with the zone and the way out", () => {
    expect(refusalText(UK)).toMatch(
      /^It's a free key, and PM thinks you're in the United Kingdom: it read your computer's time zone as Europe\/London\./,
    );
    expect(refusalText(UK)).toMatch(
      /link billing to the key's project in AI Studio and add prepay credit/,
    );
    expect(refusalText({ ...UK, basis: "unknown" })).toMatch(
      /^PM couldn't tell whether it's free or paid, so it treats it as free, and PM thinks/,
    );
    expect(refusalText(UNPLACED)).toMatch(
      /can't tell where this computer is \(its time zone reads as UTC\)/,
    );
    expect(notThereLine(UK)).toBe(
      "Not in the United Kingdom? Set your computer's time zone to where you really are, then press Check again.",
    );
    expect(notThereLine(UNPLACED)).toBeNull();
  });
});

describe("who Gemini can be picked for, and what is wrong with a role on it", () => {
  const paused = freeView({ blocked: UK });
  const empty = geminiView({
    key_check: { at: "x", outcome: "no_credit", listed: [], listed_total: 0, tier: "paid" },
  });

  it("never offers a paused or out-of-credit Gemini, but still offers OpenAI with no credit", () => {
    expect(isEligible(geminiView())).toBe(true);
    expect(isEligible(paused)).toBe(false);
    expect(isEligible(empty)).toBe(false);
    expect(
      isEligible(
        openaiView({ key_check: { at: "x", outcome: "no_credit", listed: [], listed_total: 0 } }),
      ),
    ).toBe(true);
  });

  it("gives a role already on a paused Gemini its own issue and label", () => {
    const state = stateWith([paused], { chat: "gemini" });
    expect(roleIssue(state, "chat")).toBe("blocked");
    expect(
      roleIssueLine("blocked", "chat", "Google Gemini", {
        id: "gemini",
        blocked: UK,
        tier: "free",
      }),
    ).toBe(
      "Chat is set to Google Gemini, but Gemini is paused (your key reads as free and PM thinks you're in the United Kingdom). Chat won't answer until the project is paid, or until you pick another provider.",
    );
    expect(unavailableLabel("gemini", state, "chat")).toBe("Google Gemini (paused)");
  });

  it("gives a role on an empty prepaid Gemini its own line and label", () => {
    const state = stateWith([empty], { background: "gemini" });
    expect(roleIssue(state, "background")).toBe("no_credit");
    expect(roleIssueLine("no_credit", "background", "Google Gemini", { id: "gemini" })).toBe(
      "Background is set to Google Gemini, but its prepay balance is empty. Background won't answer until you add credit in AI Studio, or you pick another provider.",
    );
    expect(unavailableLabel("gemini", state, "background")).toBe("Google Gemini (out of credit)");
  });

  it("makes the wizard unable to finish on a paused provider", () => {
    expect(directReady(stateWith([paused], { chat: "gemini", background: "gemini" }), false)).toBe(
      false,
    );
    expect(
      directReady(stateWith([geminiView()], { chat: "gemini", background: "gemini" }), false),
    ).toBe(true);
  });

  it("adopts a paid key for both roles and a free or unknown one for chat only", () => {
    expect(adoptRoles(geminiView())).toEqual(["chat", "background"]);
    expect(adoptRoles(freeView())).toEqual(["chat"]);
    expect(adoptRoles(geminiView({ tier: "unknown" }))).toEqual(["chat"]);
    expect(adoptRoles(geminiView({ tier: null }))).toEqual(["chat"]);
    // Nothing that can't be picked is adopted.
    expect(adoptRoles(freeView({ blocked: UK }))).toEqual([]);
    expect(adoptRoles(openaiView())).toEqual(["chat", "background"]);
    expect(adoptRoles(undefined)).toEqual([]);
  });

  it("offers Check again for everything but a known-paid, working key", () => {
    const ok = { outcome: "ok" as const };
    expect(offersRecheckFor(gemini(), ok, "paid", null)).toBe(false);
    expect(offersRecheckFor(gemini(), ok, "free", null)).toBe(true);
    expect(offersRecheckFor(gemini(), ok, "unknown", null)).toBe(true);
    expect(offersRecheckFor(gemini(), ok, null, null)).toBe(true);
    expect(offersRecheckFor(gemini(), ok, "paid", UK)).toBe(true);
    expect(offersRecheckFor(gemini(), { outcome: "needs_verification" }, "paid", null)).toBe(true);
    // OpenAI has no tiers: only its own verdicts offer it.
    expect(offersRecheckFor(directInfo("openai")!, ok, null, null)).toBe(false);
  });
});

describe("the picker's Gemini rows", () => {
  it("flags a paid-only model against a key that reads as free, and only then", () => {
    const flag = (v: ReturnType<typeof geminiView>) =>
      directPickerRows(v).find((r) => r.id === PRO.id)?.note;
    expect(flag(geminiView())).toBeNull();
    expect(flag(freeView())).toBe("Needs a paid key");
    expect(flag(geminiView({ tier: null }))).toBe("Needs a paid key");
    // A model with a free allowance is never flagged.
    expect(directPickerRows(freeView()).find((r) => r.id === FLASH.id)?.note).toBeNull();
    expect(readsFree(freeView())).toBe(true);
    expect(readsFree(openaiView())).toBe(false);
  });

  it("says when a price rises", () => {
    const flash = directPickerRows(geminiView()).find((r) => r.id === FLASH.id);
    expect(flash?.tip).toContain(
      "Rises to $1.50 in and $7.50 out per million tokens on 01-01-2027.",
    );
    expect(directPickerRows(geminiView()).find((r) => r.id === PRO.id)?.tip).not.toMatch(/Rises/);
  });

  it("names the model's own default effort in chat's first option", () => {
    expect(effortOptions("chat", FLASH)[0]).toEqual({ value: "", label: "Model default (Medium)" });
    // 3.8 Flash has no Minimal.
    expect(effortOptions("chat", FLASH).map((o) => o.label)).not.toContain("Minimal");
    expect(effortOptions("background", FLASH_LITE).map((o) => o.label)).toEqual([
      "Minimal",
      "Low (PM's default)",
      "Medium",
      "High",
    ]);
  });
});

describe("the route labels", () => {
  it("says the terms for Gemini and the one label for OpenAI", () => {
    expect(routeLabel(gemini(), "paid")).toBe("Gemini direct, paid");
    expect(routeLabel(gemini(), "free")).toBe("Gemini direct, free");
    expect(routeLabel(gemini(), "blocked")).toBe("Gemini paused");
    expect(routeLabel(directInfo("openai")!, null)).toBe("OpenAI direct");
    expect(routeLabel(directInfo("openai")!, null, true)).toBe("OpenAI direct, shared");
  });

  it("titles a paused route with where PM thinks you are, when it knows", () => {
    expect(routeTitle(gemini(), "chat", "blocked", UK)).toBe(
      "Chat is set to Google Gemini, but your key reads as free and PM thinks you're in the United Kingdom. Chat can't answer until you fix this. Open Settings > AI & Models.",
    );
  });
});
