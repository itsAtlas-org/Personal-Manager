// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// AI & Models with a provider the person reaches with their own key: the "Your own keys" section
// and the role blocks share ONE read of the state; "Refresh prices" (which asks OpenRouter) goes
// only for a person using no OpenRouter at all; and Reset puts both roles' lists back to the default
// one (and edits only a list that differs), without turning a provider off or deleting a key.

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { anthropicView, directState, openaiView } from "../../lib/directProvidersFixture";
import { claudeRow, orRow, routeView } from "../../lib/routeFixture";
import type { DirectProvidersState, RouteView } from "../../lib/types";

const DEFAULT_ROUTE = routeView([orRow("anthropic/claude-sonnet-4.6")]);

const m = vi.hoisted(() => ({
  world: {
    state: null as unknown,
    orKey: false,
    chat: null as unknown,
    background: null as unknown,
  },
  routeReset: vi.fn(),
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
  chat_route: DEFAULT_ROUTE,
  background_route: DEFAULT_ROUTE,
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
  routeStatus: () => Promise.resolve({ chat: m.world.chat, background: m.world.background }),
  onRouteChanged: () => Promise.resolve(() => {}),
  routeReset: (...a: unknown[]) => m.routeReset(...a),
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
  m.world.chat = DEFAULT_ROUTE;
  m.world.background = DEFAULT_ROUTE;
  m.costSummary.mockResolvedValue(SUMMARY);
  m.routeReset.mockImplementation(async () => ({
    route: DEFAULT_ROUTE,
    materialised: true,
    unused_saved: 0,
  }));
});
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

function mount(
  state: DirectProvidersState | null,
  routes?: { chat?: RouteView; background?: RouteView },
) {
  m.world.state = state;
  if (routes?.chat) m.world.chat = routes.chat;
  if (routes?.background) m.world.background = routes.background;
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
    // There is no provider Select any more: the list says who answers, row by row.
    expect(screen.queryByLabelText("Chat provider")).toBeNull();
  });

  it("draws each role's list and an Add a model beside each, and offers the provider in it", async () => {
    mount(directState());
    expect(await screen.findByRole("region", { name: "Chat model" })).toBeTruthy();
    expect(screen.getByRole("region", { name: "Background model" })).toBeTruthy();
    fireEvent.click(await screen.findByRole("button", { name: "Add a model to Chat" }));
    expect(screen.getByRole("dialog", { name: "Add a model to Chat" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "OpenAI" })).toBeTruthy();
  });

  it("draws the rows of the list the backend sent, in its order", async () => {
    mount(directState(), { chat: routeView([claudeRow(), orRow("a/one")]) });
    await screen.findByRole("region", { name: "Chat model" });
    const chat = await screen.findAllByText("Claude Sonnet 5.5");
    expect(chat.length).toBeGreaterThan(0);
    const items = [...document.querySelectorAll("[data-row-key]")].map((li) =>
      li.getAttribute("data-row-key"),
    );
    expect(items.slice(0, 2)).toEqual([
      "anthropic:anthropic/claude-sonnet-5-5",
      "openrouter:a/one",
    ]);
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
    // A list that differs also has its own "Reset" link beside it: the confirm is the dialog's.
    fireEvent.click(
      within(await screen.findByRole("dialog")).getByRole("button", { name: "Reset" }),
    );
  }

  it("is at its default when both roles are on OpenRouter and no effort was chosen", async () => {
    mount(directState());
    const button = await screen.findByRole("button", { name: "Reset to defaults" });
    await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(true));
  });

  it("puts both lists back to the default one, turning nothing off", async () => {
    mount(directState({ chat: "openai", background: "openai" }), {
      chat: routeView([claudeRow(), orRow("a/one")]),
      background: routeView([claudeRow()]),
    });
    await reset();
    await waitFor(() => expect(m.routeReset).toHaveBeenCalledTimes(2));
    expect(m.routeReset).toHaveBeenCalledWith("chat");
    expect(m.routeReset).toHaveBeenCalledWith("background");
  });

  it("edits only a list that differs, so a role nobody edited stays unwritten", async () => {
    mount(directState(), { chat: routeView([claudeRow()]) });
    await reset();
    await waitFor(() => expect(m.routeReset).toHaveBeenCalledTimes(1));
    expect(m.routeReset).toHaveBeenCalledWith("chat");
  });

  it("a chosen effort on a direct row is a list that differs, and reset clears it with the list", async () => {
    const view = openaiView();
    mount(directState({ view }), {
      chat: routeView([claudeRow({ effort: "high" }), orRow("anthropic/claude-sonnet-4.6")]),
    });
    await reset();
    await waitFor(() => expect(m.routeReset).toHaveBeenCalledWith("chat"));
    expect(m.routeReset).not.toHaveBeenCalledWith("background");
  });

  it("states what it leaves alone", async () => {
    mount(directState({ chat: "openai" }), { chat: routeView([claudeRow()]) });
    fireEvent.click(await screen.findByRole("button", { name: "Reset to defaults" }));
    expect(
      await screen.findByText(
        /back to PM's default model on OpenRouter.*Your saved API keys, and which providers you've turned on, aren't affected\./s,
      ),
    ).toBeTruthy();
  });
});
