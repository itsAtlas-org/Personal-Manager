// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// AI & Models with a provider the person reaches with their own key: the "Your own keys" section
// and the role blocks share ONE read of the state; "Refresh prices" (which asks OpenRouter) goes
// only for a person using no OpenRouter at all; and Reset sends both roles back to OpenRouter and an
// effort someone chose back to the role's default, without turning a provider off or deleting a key.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { anthropicView, directState, openaiView } from "../../lib/directProvidersFixture";
import type { DirectProvidersState } from "../../lib/types";

const m = vi.hoisted(() => ({
  world: { state: null as unknown, orKey: false },
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

beforeEach(() => {
  m.world.orKey = false;
  m.costSummary.mockResolvedValue(SUMMARY);
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

describe("the Your own keys section", () => {
  it("is absent until the backend has answered, so the tab is as it was without it", () => {
    mount(null);
    expect(screen.queryByText("Your own keys")).toBeNull();
  });

  it("is a section with the consent framing, the card and an off switch by default", async () => {
    mount(directState({ view: openaiView({ opted_in: false, key: "absent", key_check: null }) }));
    expect(await screen.findByText("Your own keys")).toBeTruthy();
    expect(
      screen.getByText(
        "Use your own key from OpenAI, Google Gemini or Anthropic instead of OpenRouter. It's off until you turn it on. Requests to your own key are not zero data retention, and a free Google key lets Google train on what you send.",
      ),
    ).toBeTruthy();
    const toggle = screen.getByRole("switch", { name: "Use my own OpenAI key" });
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    // No provider to choose between, so the role blocks are today's editors.
    expect(screen.queryByLabelText("Chat provider")).toBeNull();
  });

  it("gives each role a provider choice once a provider is on and keyed", async () => {
    mount(directState());
    expect(await screen.findByLabelText("Chat provider")).toBeTruthy();
    expect(screen.getByLabelText("Background provider")).toBeTruthy();
  });

  it("says the Background OpenRouter key is only used while Background is on OpenRouter", async () => {
    mount(null);
    expect(await screen.findByText("Only used while Background is on OpenRouter.")).toBeTruthy();
  });
});

describe("Refresh prices", () => {
  it("stays for a person with no key at all, as before", async () => {
    mount(directState());
    await screen.findByText(/Last 30 days/);
    expect(screen.getByRole("button", { name: "Refresh prices" })).toBeTruthy();
  });

  it("goes for a person whose roles are both on their own key and who has no OpenRouter key", async () => {
    mount(directState({ chat: "openai", background: "openai" }));
    await screen.findByText(/Last 30 days/);
    await waitFor(() =>
      expect(screen.queryByRole("button", { name: "Refresh prices" })).toBeNull(),
    );
  });

  it("stays when either role is still on OpenRouter", async () => {
    mount(directState({ chat: "openai" }));
    await screen.findByText(/Last 30 days/);
    expect(screen.getByRole("button", { name: "Refresh prices" })).toBeTruthy();
  });

  it("stays for anyone with an OpenRouter key", async () => {
    m.world.orKey = true;
    mount(directState({ chat: "openai", background: "openai" }));
    await screen.findByText(/Last 30 days/);
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Refresh prices" })).toBeTruthy(),
    );
  });
});

describe("Usage copy", () => {
  it("says what an own-key call's cost is worked out from, and the date of PM's list", async () => {
    mount(directState());
    await screen.findByText(/Last 30 days/);
    fireEvent.click(screen.getByText("How this is calculated"));
    expect(
      await screen.findByText(/prices built into this version of PM \(checked on 26-11-2026\)/),
    ).toBeTruthy();
  });

  it("tells an Anthropic user that a stopped, cut-off or errored reply is billed but not counted", async () => {
    mount(directState({ view: anthropicView() }));
    await screen.findByText(/Last 30 days/);
    fireEvent.click(screen.getByText("How this is calculated"));
    expect(
      await screen.findByText(
        /If Anthropic stops, cuts off or errors a reply partway, it still bills what it had written; PM can't see those tokens, so they aren't counted here\./,
      ),
    ).toBeTruthy();
  });

  it("does not say it to someone who doesn't use Anthropic", async () => {
    mount(directState());
    await screen.findByText(/Last 30 days/);
    fireEvent.click(screen.getByText("How this is calculated"));
    await screen.findByText(/prices built into this version of PM/);
    expect(screen.queryByText(/If Anthropic stops, cuts off or errors/)).toBeNull();
  });
});

describe("Reset to defaults", () => {
  async function reset() {
    const button = await screen.findByRole("button", { name: "Reset to defaults" });
    await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(button);
    fireEvent.click(await screen.findByRole("button", { name: "Reset" }));
  }

  it("is at its default when both roles are on OpenRouter and no effort was chosen", async () => {
    mount(directState());
    const button = await screen.findByRole("button", { name: "Reset to defaults" });
    await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(true));
  });

  it("sends a role on its own provider back to OpenRouter, turning nothing off", async () => {
    mount(directState({ chat: "openai", background: "openai" }));
    await reset();
    await waitFor(() => expect(m.setRoleCloudProvider).toHaveBeenCalledTimes(2));
    expect(m.setRoleCloudProvider).toHaveBeenCalledWith("chat", "openrouter");
    expect(m.setRoleCloudProvider).toHaveBeenCalledWith("background", "openrouter");
  });

  it("puts a chosen effort back to the role's default (null), keeping the model", async () => {
    const view = openaiView();
    view.roles.chat = { uses_this_provider: false, model: "openai/gpt-6.2-sol", effort: "high" };
    mount(directState({ view }));
    await reset();
    await waitFor(() =>
      expect(m.setDirectRole).toHaveBeenCalledWith("openai", "chat", ["openai/gpt-6.2-sol"], null),
    );
    expect(m.setRoleCloudProvider).not.toHaveBeenCalled();
  });

  it("states what it leaves alone", async () => {
    mount(directState({ chat: "openai" }));
    fireEvent.click(await screen.findByRole("button", { name: "Reset to defaults" }));
    expect(
      await screen.findByText(
        /sends both roles back to OpenRouter.*Your saved API keys, and which providers you've turned on, aren't affected\./s,
      ),
    ).toBeTruthy();
  });
});
