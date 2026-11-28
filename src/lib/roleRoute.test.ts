// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The sentences and small decisions a role's list is worded from. The copy is in one file so it can
// be reviewed in one place; these pin the words a person reads and the facts they come from.

import { describe, expect, it } from "vitest";

import { anthropicView, directState, openaiView, stateWith } from "./directProvidersFixture";
import { claudeRow, geminiRow, localRow, orRow, routeView } from "./routeFixture";
import {
  addedAnnouncement,
  atDefaultCloudRows,
  chipLabel,
  fallbackCount,
  firstEditLine,
  groupedWithAbove,
  headDownNote,
  localFailureLine,
  localPlaceLine,
  modsBlocked,
  movedAnnouncement,
  moveName,
  pickerSections,
  removedAnnouncement,
  rowName,
  sectionFor,
  somethingElseUsable,
  statusText,
  upgradeChatModel,
} from "./roleRoute";
import type { Settings } from "./types";

const NO_LOCAL = { on: false, model: null };

describe("the context meter's larger-model switch", () => {
  const settings = (over: Partial<Settings>) =>
    ({ chat_provider: "openrouter", chat_models: ["a/one", "a/two"], ...over }) as Settings;

  it("hands the backend the ONE model, so no other row of the list is moved, and says how many saved models were left out", async () => {
    const written: string[] = [];
    const note = await upgradeChatModel(settings({}), "a/big", async (m) => {
      written.push(m);
      return 2;
    });
    // Not the whole OpenRouter set: the backend puts this one where the first OpenRouter row stands
    // and leaves a Claude row, and an OpenRouter row below it, exactly where they were.
    expect(written).toEqual(["a/big"]);
    expect(note).toBe(firstEditLine(2));
    expect(note).toContain("2 other saved models weren't in use");
  });

  it("says nothing when nothing was left out, and nothing for a backend that answers nothing", async () => {
    expect(await upgradeChatModel(settings({}), "a/big", async () => 0)).toBeNull();
    expect(await upgradeChatModel(settings({}), "a/big", async () => undefined)).toBeNull();
  });

  it("never writes the OpenRouter list for a chat on a provider of the person's own", async () => {
    let wrote = false;
    const note = await upgradeChatModel(
      settings({ chat_provider: "anthropic" }),
      "a/big",
      async () => {
        wrote = true;
        return 3;
      },
    );
    expect(wrote).toBe(false);
    expect(note).toBeNull();
  });
});

describe("chips and names", () => {
  it("says who a row goes to", () => {
    expect(chipLabel(claudeRow())).toBe("Claude, your key");
    expect(chipLabel(geminiRow())).toBe("Gemini, your key");
    expect(chipLabel(orRow("a/b"))).toBe("OpenRouter");
    expect(chipLabel(localRow())).toBe("This computer");
    expect(chipLabel({ provider: "openai" })).toBe("OpenAI, your key");
  });

  it("offers a reset by the cloud rows only: this computer is the person's own choice", () => {
    const def = routeView([orRow("a/1")], true);
    expect(atDefaultCloudRows(routeView([localRow(), orRow("a/1")], true), def)).toBe(true);
    expect(atDefaultCloudRows(routeView([orRow("a/1"), localRow()]), def)).toBe(true);
    expect(atDefaultCloudRows(routeView([localRow(), orRow("b/2")]), def)).toBe(false);
    expect(atDefaultCloudRows(routeView([orRow("a/1"), orRow("b/2")]), def)).toBe(false);
  });

  it("offers no reset to a Local-only list: Reset would add a cloud fallback nobody chose", () => {
    const def = routeView([orRow("a/1")], true);
    expect(atDefaultCloudRows(routeView([localRow()]), def)).toBe(true);
  });

  it("never calls a Local AI server on another machine 'this computer'", () => {
    const away = localRow({ local_elsewhere: true });
    expect(chipLabel(away)).toBe("Your Local AI server");
    expect(rowName(away)).toBe("llama3.2 on your Local AI server");
    expect(rowName(localRow({ local_elsewhere: true, name: "This computer" }))).toBe(
      "Your Local AI server",
    );
    expect(rowName(localRow())).toBe("llama3.2 on this computer");
    expect(localFailureLine(false, true)).toBe(
      "If your Local AI server can't answer, the next row is tried.",
    );
    expect(localFailureLine(true)).toBe("If this computer can't answer, nothing else is tried.");
  });

  it("puts the model and its current place in a Move button's name", () => {
    expect(moveName("Claude Sonnet 5.5", "up", 1, 3)).toBe("Move Claude Sonnet 5.5 up, now 2 of 3");
  });

  it("announces a move, a removal and an addition", () => {
    expect(movedAnnouncement("Claude Sonnet 5.5", 1, 3)).toBe(
      "Claude Sonnet 5.5 is now number 1 of 3",
    );
    expect(removedAnnouncement("Gemini 3.5 Flash", 2, "chat")).toBe(
      "Removed Gemini 3.5 Flash. 2 models left in Chat",
    );
    expect(removedAnnouncement("Gemini 3.5 Flash", 1, "background")).toBe(
      "Removed Gemini 3.5 Flash. 1 model left in Background",
    );
    expect(addedAnnouncement("OpenAI GPT 6 Sol")).toBe("Added OpenAI GPT 6 Sol at the end");
  });

  it("says how many saved models the first edit left out", () => {
    expect(firstEditLine(0)).toBeNull();
    expect(firstEditLine(1)).toBe("1 other saved model wasn't in use and isn't in this list.");
    expect(firstEditLine(4)).toBe("4 other saved models weren't in use and aren't in this list.");
  });
});

describe("a row's status words", () => {
  it("is the brief's list, and never the row's own colour", () => {
    expect(statusText({ kind: "ready" }, "anthropic")).toBe("Ready");
    expect(statusText({ kind: "not_on" }, "anthropic")).toBe("Turn on Anthropic in Your own keys");
    expect(statusText({ kind: "no_key" }, "gemini")).toBe("No key saved");
    expect(statusText({ kind: "paused_region" }, "gemini")).toBe("Paused where you are");
    expect(statusText({ kind: "paused_limit" }, "openai")).toBe("Paused for today");
    expect(statusText({ kind: "local_off" }, "local")).toBe("Local AI is off");
  });

  it("says a row after a this-computer row that cannot run is never tried", () => {
    expect(statusText({ kind: "after_local_off" }, "openrouter")).toBe(
      "Not tried while Local AI is off",
    );
    expect(statusText({ kind: "after_local_pick" }, "anthropic")).toBe(
      "Not tried until a model is chosen on this computer",
    );
  });

  it("not working says when and why, and leaves out a time it can't read", () => {
    expect(statusText({ kind: "not_working", reason: "busy" }, "anthropic")).toBe(
      "Not working: busy",
    );
    expect(
      statusText(
        { kind: "not_working", reason: "busy", since: "2026-11-28T14:05:00" },
        "anthropic",
      ),
    ).toMatch(/^Not working since \d{1,2}:\d{2}.*: busy$/);
    expect(statusText({ kind: "not_working", since: "nonsense" }, "anthropic")).toBe("Not working");
  });
});

describe("the list's shape", () => {
  it("groups only a row under another OpenRouter row", () => {
    const e = [orRow("a/1"), orRow("a/2"), claudeRow(), orRow("a/3")];
    expect([0, 1, 2, 3].map((i) => groupedWithAbove(e, i))).toEqual([false, true, false, false]);
  });

  it("the footer counts the rows behind the first that can answer", () => {
    const down = { kind: "not_working" as const };
    expect(fallbackCount(routeView([orRow("a/1")]))).toBe(0);
    expect(fallbackCount(routeView([orRow("a/1"), claudeRow(), localRow()]))).toBe(2);
    expect(fallbackCount(routeView([claudeRow({ status: down }), geminiRow(), orRow("a/1")]))).toBe(
      1,
    );
    expect(fallbackCount(routeView([orRow("a/1"), claudeRow({ status: down })]))).toBe(0);
  });

  it("says the head is down only when a later row can answer", () => {
    const down = { kind: "not_working" as const };
    expect(headDownNote(routeView([claudeRow({ status: down }), geminiRow()]))).toBe(
      "Claude isn't working, Gemini is next",
    );
    expect(headDownNote(routeView([orRow("a/1", { status: down }), orRow("a/2")]))).toBe(
      "a/1 isn't working, a/2 is next",
    );
    expect(headDownNote(routeView([claudeRow(), geminiRow()]))).toBeNull();
    expect(headDownNote(routeView([claudeRow({ status: down })]))).toBeNull();
    expect(
      headDownNote(routeView([claudeRow({ status: down }), geminiRow({ status: down })])),
    ).toBeNull();
    expect(headDownNote(null)).toBeNull();
  });

  it("never promises a move to another company after a daily limit: only this computer follows one", () => {
    const limit = { kind: "paused_limit" as const };
    // [Claude (limit reached), Gemini]: PM stops with the card, so the chip must not say Gemini is next.
    expect(headDownNote(routeView([claudeRow({ status: limit }), geminiRow()]))).toBeNull();
    expect(headDownNote(routeView([claudeRow({ status: limit }), orRow("a/1")]))).toBeNull();
    // This computer, free, may answer after a limit by itself.
    expect(headDownNote(routeView([claudeRow({ status: limit }), localRow()]))).toBe(
      "Claude is paused for today, this computer is next",
    );
    // A Local row below another company's row is still the one that follows.
    expect(headDownNote(routeView([claudeRow({ status: limit }), geminiRow(), localRow()]))).toBe(
      "Claude is paused for today, this computer is next",
    );
    expect(
      headDownNote(routeView([claudeRow({ status: limit }), localRow({ local_elsewhere: true })])),
    ).toBe("Claude is paused for today, your Local AI server is next");
    // A Local row that cannot answer is not named.
    expect(
      headDownNote(
        routeView([claudeRow({ status: limit }), localRow({ status: { kind: "local_off" } })]),
      ),
    ).toBeNull();
  });

  it("mods get no AI while the first cloud row is a direct provider", () => {
    expect(modsBlocked(routeView([claudeRow(), orRow("a/1")]))?.provider).toBe("anthropic");
    expect(modsBlocked(routeView([localRow(), claudeRow()]))?.provider).toBe("anthropic");
    expect(modsBlocked(routeView([orRow("a/1"), claudeRow()]))).toBeUndefined();
    expect(modsBlocked(routeView([localRow()]))).toBeUndefined();
  });

  it("the Local AI tab's line says where this computer sits", () => {
    expect(localPlaceLine("chat", null)).toBeNull();
    expect(localPlaceLine("chat", routeView([orRow("a/1")]))).toBe(
      "This computer is not in Chat's list.",
    );
    expect(localPlaceLine("background", routeView([localRow()]))).toBe(
      "This computer is Background's only row.",
    );
    expect(localPlaceLine("chat", routeView([orRow("a/1"), localRow()]))).toBe(
      "This computer is number 2 of 2 in Chat's list.",
    );
  });
});

describe("what could answer if the only row went down", () => {
  const one = routeView([claudeRow()]);
  it("counts OpenRouter with a key, an eligible provider, or a ready local model", () => {
    const dp = directState({ view: anthropicView() });
    expect(somethingElseUsable(one, dp, false, NO_LOCAL)).toBe(false);
    expect(somethingElseUsable(one, dp, true, NO_LOCAL)).toBe(true);
    expect(somethingElseUsable(one, dp, false, { on: true, model: "llama3.2" })).toBe(true);
    expect(somethingElseUsable(one, dp, false, { on: true, model: null })).toBe(false);
    expect(
      somethingElseUsable(one, stateWith([anthropicView(), openaiView()]), false, NO_LOCAL),
    ).toBe(true);
    expect(
      somethingElseUsable(
        one,
        stateWith([anthropicView(), openaiView({ key: "absent", key_check: null })]),
        false,
        NO_LOCAL,
      ),
    ).toBe(false);
  });
});

describe("the picker's sections", () => {
  it("are OpenRouter, each provider with a card, then this computer", () => {
    const dp = stateWith([anthropicView(), openaiView()]);
    expect(pickerSections(dp, true, NO_LOCAL).map((s) => s.title)).toEqual([
      "OpenRouter",
      "Anthropic",
      "OpenAI",
      "This computer",
    ]);
  });

  it("each says its own reason", () => {
    const dp = stateWith([
      anthropicView({ opted_in: false }),
      openaiView({ key: "absent", key_check: null }),
    ]);
    const by = Object.fromEntries(pickerSections(dp, false, NO_LOCAL).map((s) => [s.provider, s]));
    expect(by.anthropic.line).toBe("Turn on Anthropic in Your own keys first");
    expect(by.openai.line).toBe("No key saved");
    // N4 (R11): the line now says the way out as well as the state.
    expect(by.local.line).toBe("Local AI is off. Turn it on just below the lists first.");
    expect(by.openrouter.blocked).toBe(false);
    expect(by.openrouter.line).toMatch(/No key saved yet/);
  });

  it("a provider paused by the daily limit can still be added, and says so", () => {
    const view = anthropicView({
      limit: { ...anthropicView().limit, limit_usd: 5, spent_today_usd: 5, reached: true },
    });
    const s = sectionFor("anthropic", directState({ view }), true, NO_LOCAL);
    expect(s.blocked).toBe(false);
    expect(s.line).toMatch(/Paused for today by your daily limit/);
  });
});
