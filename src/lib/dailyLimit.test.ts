// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The daily limit's words and its one parser. Rust works out the day, the spend and whether the limit
// is reached; this file only says what Rust answered, and reads the amount a person typed.

import { describe, expect, it } from "vitest";

import { blockedReason } from "./powerRoute";
import {
  limitFieldText,
  limitUsd,
  msUntilResume,
  parseLimit,
  pausedLine,
  todayLine,
  todayReadout,
} from "./dailyLimit";
import { roleIssue, roleIssueLine, unavailableLabel } from "./directProviders";
import { anthropicView, directState, limitView, openaiView } from "./directProvidersFixture";

const REACHED = limitView({ limit_usd: 5, spent_today_usd: 5.12, reached: true });

describe("the amount a person types", () => {
  it("takes plain money, with an optional dollar sign and up to four decimals", () => {
    expect(parseLimit("5")).toBe(5);
    expect(parseLimit(" $5.00 ")).toBe(5);
    expect(parseLimit("0.01")).toBe(0.01);
    expect(parseLimit("0.0100")).toBe(0.01);
  });

  it("refuses everything that is not an amount above zero, so it is never sent", () => {
    for (const bad of [
      "",
      "0",
      "0.00",
      "-3",
      "1e9",
      "five",
      "5,00",
      "5.12345",
      "NaN",
      "Infinity",
    ]) {
      expect(parseLimit(bad), bad).toBeNull();
    }
  });

  it("prints cents, and four places under a cent so a $0.01 limit never reads $0.00", () => {
    expect(limitUsd(5)).toBe("$5.00");
    expect(limitUsd(5.12)).toBe("$5.12");
    expect(limitUsd(0.004)).toBe("$0.0040");
    expect(limitUsd(0)).toBe("$0.00");
    expect(limitFieldText(5)).toBe("5.00");
    expect(limitFieldText(0.005)).toBe("0.0050");
  });
});

describe("the standing line and the Usage read-out", () => {
  it("says what is paused, how much was logged and when it lifts", () => {
    expect(pausedLine(REACHED)).toBe(
      "Paused for today. PM has logged at least $5.12 of your $5.00 limit. It starts again at 00:00 (your day), or sooner if you raise the limit.",
    );
  });

  it("says PM could not read today's usage when that is the reason, never a spend it does not have", () => {
    const line = pausedLine(limitView({ limit_usd: 5, reached: true, unreadable: true }));
    expect(line).toBe(
      "PM couldn't read today's usage, so it paused this provider to be safe. Check Usage, or turn the limit off.",
    );
    expect(line).not.toContain("$");
    // Given the provider's name it says which one (the chip title and the role line do).
    expect(pausedLine(limitView({ reached: true, unreadable: true }), "OpenAI")).toBe(
      "PM couldn't read today's usage, so it paused OpenAI to be safe. Check Usage, or turn the limit off.",
    );
  });

  it("reads 'Today: $1.20 of $5.00', and 'at least' only when some of today's calls had no price", () => {
    const within = limitView({ limit_usd: 5, spent_today_usd: 1.2 });
    expect(todayReadout(within)).toBe("Today: $1.20 of $5.00");
    expect(todayReadout({ ...within, unpriced_today: 2 })).toBe("Today: at least $1.20 of $5.00");
    expect(todayReadout(limitView())).toBeNull();
  });

  it("never prints a $0.00 it does not have when today's usage could not be read", () => {
    const unreadable = limitView({ limit_usd: 5, reached: true, unreadable: true });
    expect(todayReadout(unreadable)).toBe("Today: PM couldn't read today's usage");
    // A stored value PM can't read has no amount either, and says the same.
    expect(todayReadout(limitView({ reached: true, unreadable: true }))).toBe(
      "Today: PM couldn't read today's usage",
    );
    expect(todayLine("Anthropic", unreadable)).toBe(
      "Anthropic: Today: PM couldn't read today's usage",
    );
    expect(todayReadout(unreadable)).not.toContain("$");
  });

  it("names the provider and counts the calls it could not price", () => {
    const v = limitView({ limit_usd: 5, spent_today_usd: 1.2, unpriced_today: 1 });
    expect(todayLine("Anthropic", v)).toBe(
      "Anthropic: Today: at least $1.20 of $5.00 (1 call today couldn't be priced in full)",
    );
    expect(todayLine("Anthropic", { ...v, unpriced_today: 3 })).toContain("3 calls today");
    expect(todayLine("Anthropic", limitView())).toBeNull();
  });
});

describe("a role on a provider the limit has paused", () => {
  const reached = () =>
    directState({ view: openaiView({ limit: REACHED }), chat: "openai", background: "openai" });

  it("is its own issue, after the region's pause and before an empty balance", () => {
    expect(roleIssue(reached(), "chat")).toBe("limit_reached");
    const both = directState({
      view: openaiView({
        limit: REACHED,
        key_check: { at: "x", outcome: "no_credit", listed: [], listed_total: 0 },
      }),
      chat: "openai",
    });
    expect(roleIssue(both, "chat")).toBe("limit_reached");
    // Within the limit it says nothing.
    expect(
      roleIssue(directState({ view: openaiView({ limit: limitView() }), chat: "openai" }), "chat"),
    ).toBeNull();
  });

  it("says it stops for the day and never moves on by itself", () => {
    expect(roleIssueLine("limit_reached", "chat", "OpenAI", { limit: REACHED })).toBe(
      "Chat is set to OpenAI. Paused for today. PM has logged at least $5.12 of your $5.00 limit. It starts again at 00:00 (your day), or sooner if you raise the limit. PM won't send this to another company on its own.",
    );
    expect(roleIssueLine("limit_reached", "background", "OpenAI")).toContain(
      "PM won't send this to another company on its own.",
    );
  });

  it("is labelled 'paused for today' in the picker, and the On battery reason names the limit", () => {
    expect(unavailableLabel("openai", reached(), "chat")).toBe("OpenAI (paused for today)");
    // An unreadable limit does not lift at midnight, so the label does not say it will.
    const unreadable = directState({
      view: openaiView({ limit: limitView({ limit_usd: null, reached: true, unreadable: true }) }),
      chat: "openai",
      background: "openai",
    });
    expect(unavailableLabel("openai", unreadable, "chat")).toBe("OpenAI (paused)");
    expect(
      blockedReason("chat", "limit_reached", {
        chat: { cloud_provider: "anthropic" },
      } as unknown as Parameters<typeof blockedReason>[2]),
    ).toBe("Chat is on Anthropic, which your daily limit has paused");
    // The unused fixture keeps the import honest.
    expect(anthropicView().limit.reached).toBe(false);
  });
});

describe("reading the state again when a pause lifts at midnight", () => {
  const MIDNIGHT = Date.parse("2026-11-28T00:00:00+00:00");

  it("waits until a second after the soonest midnight among reached limits", () => {
    const later = limitView({ reached: true, resumes_at: "2026-11-28T05:00:00+00:00" });
    expect(msUntilResume([later, REACHED], MIDNIGHT - 60_000)).toBe(61_000);
  });

  it("never waits for a limit that isn't reached, is off, or couldn't be read", () => {
    const unreadable = limitView({ reached: true, unreadable: true });
    expect(msUntilResume([limitView(), unreadable, null, undefined], MIDNIGHT - 60_000)).toBeNull();
  });

  it("waits a minute, never zero, when the midnight it was given has already passed", () => {
    expect(msUntilResume([REACHED], MIDNIGHT + 5_000)).toBe(60_000);
  });
});
