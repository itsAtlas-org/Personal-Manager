// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";

import appSource from "../App.tsx?raw";

import {
  chatUpgradeApplies,
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
  keyVerdict,
  movedLine,
  offersRecheck,
  recheckStores,
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
import { directState, LUNA, openaiView, settingsFor, SOL, TERRA } from "./directProvidersFixture";

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
    expect(providerName("gemini")).toBe("gemini");
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
      text: "Saved. Your OpenAI account has no credit yet, so chats won't work until you add some at platform.openai.com. Then press Check again.",
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
      /^Your OpenAI account has no credit yet/,
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
      "Your OpenAI account has no credit yet, so this won't answer until you add some.",
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
    });
    // The other role is untouched.
    expect(footerRow(s, "background")).toEqual({
      id: "google/gemini-2.5-flash",
      fallbacks: 0,
      via: null,
    });
  });

  it("counts auto-switch fallbacks on OpenRouter as it always did", () => {
    expect(footerRow(settingsFor(), "chat")).toEqual({
      id: "anthropic/claude-sonnet-4.6",
      fallbacks: 1,
      via: null,
    });
    expect(footerRow(null, "chat")).toEqual({ id: null, fallbacks: 0, via: null });
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
