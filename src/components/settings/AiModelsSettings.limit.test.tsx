// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The daily limit on AI & Models: a control on the OpenRouter key block and on each provider's card
// (the control lives beside what it brakes), a read-only "Today" line per limit in Usage, a paused
// role that says so and offers the way out as a click, and a Reset that never touches a limit.

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  anthropicView,
  directState,
  limitView,
  openaiView,
} from "../../lib/directProvidersFixture";
import type { DirectProvidersState, LimitView } from "../../lib/types";

const m = vi.hoisted(() => ({
  world: { state: null as unknown, orKey: false, orLimit: null as unknown },
  setDailyLimit: vi.fn(),
  setRoleCloudProvider: vi.fn(),
  setDirectRole: vi.fn(),
  costSummary: vi.fn(),
}));

const SETTINGS = {
  chat_models: ["anthropic/claude-sonnet-4.6"],
  background_models: ["google/gemini-2.5-flash"],
  chat_auto_switch: false,
  background_auto_switch: false,
  chat_provider: "openrouter",
  background_provider: "openrouter",
  chat_cloud_model: null,
  background_cloud_model: null,
  chat_route_terms: null,
  background_route_terms: null,
};

vi.mock("../../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  costSummary: () => m.costSummary(),
  hasOpenRouterKey: () => Promise.resolve(m.world.orKey),
  hasOpenRouterBackgroundKey: () => Promise.resolve(false),
  getSettings: () => Promise.resolve(SETTINGS),
  settingsDefaults: () => Promise.resolve(SETTINGS),
  getLocalAiSwitch: () => Promise.reject(new Error("not in this test")),
  directProvidersState: () => Promise.resolve(m.world.state),
  openrouterLimitState: () => Promise.resolve(m.world.orLimit),
  setDailyLimit: (...a: unknown[]) => m.setDailyLimit(...a),
  setRoleCloudProvider: (...a: unknown[]) => m.setRoleCloudProvider(...a),
  setDirectRole: (...a: unknown[]) => m.setDirectRole(...a),
  listModels: () => Promise.resolve([]),
}));
vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { AiModelsSettings } from "./AiModelsSettings";

const SUMMARY = {
  last_30d: [],
  all_time: [],
  by_mod: [],
  total_30d_usd: 0,
  total_all_time_usd: 0,
  total_30d_partial: false,
  total_all_time_partial: false,
  unpriced_30d: 0,
  partial_reasons: [],
  pricing_updated_at: null,
};

const REACHED: LimitView = limitView({ limit_usd: 5, spent_today_usd: 5.12, reached: true });
const PAUSED_LINE =
  "Paused for today. PM has logged at least $5.12 of your $5.00 limit. It starts again at 00:00 (your day), or sooner if you raise the limit.";

beforeEach(() => {
  m.world.orKey = false;
  m.world.orLimit = limitView();
  m.costSummary.mockResolvedValue(SUMMARY);
  m.setDailyLimit.mockResolvedValue(undefined);
  m.setRoleCloudProvider.mockResolvedValue(undefined);
  m.setDirectRole.mockResolvedValue(undefined);
});
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

function mount(state: DirectProvidersState | null) {
  m.world.state = state;
  return render(<AiModelsSettings />);
}

describe("OpenRouter's daily limit", () => {
  it("is drawn beside the OpenRouter key once there is one, off by default", async () => {
    m.world.orKey = true;
    mount(null);
    const control = await screen.findByTestId("daily-limit-openrouter");
    expect(
      within(control).getByRole("switch", { name: "Daily limit for OpenRouter" }),
    ).toBeTruthy();
    fireEvent.click(within(control).getByRole("switch", { name: "Daily limit for OpenRouter" }));
    await waitFor(() => expect(m.setDailyLimit).toHaveBeenCalledWith("openrouter", 5));
  });

  it("is not drawn for a person with no OpenRouter key and no limit on it", async () => {
    mount(directState());
    await screen.findByText(/Last 30 days/);
    expect(screen.queryByTestId("daily-limit-openrouter")).toBeNull();
  });

  it("is drawn with no key when a limit is already on it (a restore can bring one)", async () => {
    m.world.orLimit = limitView({ limit_usd: 5, spent_today_usd: 1 });
    mount(null);
    expect(await screen.findByTestId("daily-limit-openrouter")).toBeTruthy();
  });

  it("says a paused OpenRouter is paused, in a line that stays", async () => {
    m.world.orKey = true;
    m.world.orLimit = REACHED;
    mount(null);
    const control = await screen.findByTestId("daily-limit-openrouter");
    expect(within(control).getByText(PAUSED_LINE)).toBeTruthy();
  });
});

describe("each provider's card", () => {
  it("carries the control, off until the person turns it on", async () => {
    mount(directState({ view: openaiView() }));
    expect(await screen.findByTestId("daily-limit-openai")).toBeTruthy();
  });

  it("says a paused provider is paused on its card", async () => {
    mount(directState({ view: anthropicView({ limit: REACHED }) }));
    const control = await screen.findByTestId("daily-limit-anthropic");
    expect(within(control).getByText(PAUSED_LINE)).toBeTruthy();
  });
});

describe("the Usage read-out", () => {
  it("shows a Today line for each limit that is on, and none for one that is off", async () => {
    m.world.orKey = true;
    m.world.orLimit = limitView({ limit_usd: 5, spent_today_usd: 1.2 });
    mount(
      directState({
        view: anthropicView({
          limit: limitView({ limit_usd: 10, spent_today_usd: 3, unpriced_today: 2 }),
        }),
      }),
    );
    const list = await screen.findByRole("list", { name: "Today against your daily limits" });
    const lines = within(list)
      .getAllByRole("listitem")
      .map((li) => li.textContent);
    expect(lines).toEqual([
      "OpenRouter: Today: $1.20 of $5.00",
      "Anthropic: Today: at least $3.00 of $10.00 (2 calls today couldn't be priced in full)",
    ]);
  });

  it("draws no list at all while every limit is off", async () => {
    m.world.orKey = true;
    mount(directState());
    await screen.findByText(/Last 30 days/);
    expect(screen.queryByRole("list", { name: "Today against your daily limits" })).toBeNull();
  });
});

describe("a role on a paused provider", () => {
  it("says so under the role and offers OpenRouter as a click, never as PM's own move", async () => {
    // The way out is only offered where OpenRouter can take the role: it has a key.
    m.world.orKey = true;
    mount(
      directState({
        view: anthropicView({ limit: REACHED }),
        chat: "anthropic",
      }),
    );
    expect(await screen.findByText(/Chat is set to Anthropic\. Paused for today\./)).toBeTruthy();
    // The "goes straight to Anthropic" note would contradict the pause beside it.
    expect(
      screen.queryByText(/Chat goes straight to Anthropic|answers if your local model/),
    ).toBeNull();
    const out = await screen.findByRole("button", { name: "Use OpenRouter for Chat" });
    expect(m.setRoleCloudProvider).not.toHaveBeenCalled();
    fireEvent.click(out);
    await waitFor(() => expect(m.setRoleCloudProvider).toHaveBeenCalledWith("chat", "openrouter"));
  });

  it("offers no way out through OpenRouter when OpenRouter has no key", async () => {
    mount(directState({ view: anthropicView({ limit: REACHED }), chat: "anthropic" }));
    expect(await screen.findByText(/Chat is set to Anthropic\. Paused for today\./)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Use OpenRouter for Chat" })).toBeNull();
  });

  it("offers no way out through OpenRouter when OpenRouter is itself paused", async () => {
    m.world.orKey = true;
    m.world.orLimit = REACHED;
    mount(directState({ view: anthropicView({ limit: REACHED }), chat: "anthropic" }));
    expect(await screen.findByText(/Chat is set to Anthropic\. Paused for today\./)).toBeTruthy();
    // OpenRouter's own pause line has loaded by the time the role line has.
    await screen.findByTestId("daily-limit-openrouter");
    await waitFor(() =>
      expect(screen.queryByRole("button", { name: "Use OpenRouter for Chat" })).toBeNull(),
    );
  });
});

describe("Reset to defaults", () => {
  it("leaves every limit alone, and says so", async () => {
    mount(
      directState({
        view: openaiView({ limit: limitView({ limit_usd: 5, spent_today_usd: 1 }) }),
        chat: "openai",
        background: "openai",
      }),
    );
    const button = await screen.findByRole("button", { name: "Reset to defaults" });
    await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(button);
    expect(await screen.findByText(/Nor are your daily limits/)).toBeTruthy();
    fireEvent.click(await screen.findByRole("button", { name: "Reset" }));
    await waitFor(() => expect(m.setRoleCloudProvider).toHaveBeenCalledTimes(2));
    expect(m.setDailyLimit).not.toHaveBeenCalled();
  });
});
