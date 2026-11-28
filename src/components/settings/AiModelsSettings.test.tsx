// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The cost summary on AI & Models. Its first read is what refreshes OpenRouter's prices, which the
// backend does only once an OpenRouter key is stored (`ensure_pricing_fresh`). So a key saved here
// for the first time must read the summary again, or the person who just set OpenRouter up sees no
// prices until they next open the tab.

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { costSummary, setOpenRouterKey, setOpenRouterBackgroundKey, getLocalAiSwitch } = vi.hoisted(
  () => ({
    costSummary: vi.fn(),
    setOpenRouterKey: vi.fn(),
    setOpenRouterBackgroundKey: vi.fn(),
    getLocalAiSwitch: vi.fn(),
  }),
);

const SETTINGS = {
  chat_models: [],
  background_models: [],
  chat_auto_switch: false,
  background_auto_switch: false,
  // Both roles on OpenRouter, as every install starts.
  chat_provider: "openrouter",
  background_provider: "openrouter",
  chat_cloud_model: null,
  background_cloud_model: null,
  chat_route_terms: null,
  background_route_terms: null,
};

// Spread over the real module, so a wrapper this tab doesn't call here stays as it is.
vi.mock("../../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  costSummary: () => costSummary(),
  hasOpenRouterKey: () => Promise.resolve(false),
  hasOpenRouterBackgroundKey: () => Promise.resolve(false),
  getSettings: () => Promise.resolve(SETTINGS),
  settingsDefaults: () => Promise.resolve(SETTINGS),
  getLocalAiSwitch: () => getLocalAiSwitch(),
  setOpenRouterKey: (key: string) => setOpenRouterKey(key),
  setOpenRouterBackgroundKey: (key: string) => setOpenRouterBackgroundKey(key),
}));
vi.mock("../ModelListEditor", () => ({ ModelListEditor: () => null }));
// The primitives read the theme; the ThemeProvider itself pulls in IPC.
vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { AiModelsSettings } from "./AiModelsSettings";

/** An empty Usage payload in the shape the backend sends (types.ts `CostSummary`). */
const EMPTY_SUMMARY = {
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
  costSummary.mockReturnValue(new Promise(() => {}));
  getLocalAiSwitch.mockRejectedValue(new Error("not in this test"));
  setOpenRouterKey.mockResolvedValue(undefined);
  setOpenRouterBackgroundKey.mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

/** Type `value` into the main key field (`which` 0) or the background one (1), then click away. */
async function saveIn(which: 0 | 1, value: string) {
  const field = (await screen.findAllByPlaceholderText("sk-or-..."))[which];
  fireEvent.change(field, { target: { value } });
  fireEvent.blur(field);
}

describe("the cost summary on AI & Models", () => {
  it("reads it once on opening", async () => {
    render(<AiModelsSettings />);
    await waitFor(() => expect(costSummary).toHaveBeenCalledTimes(1));
  });

  it("reads it again once a first OpenRouter key is saved", async () => {
    render(<AiModelsSettings />);
    await waitFor(() => expect(costSummary).toHaveBeenCalledTimes(1));
    await saveIn(0, "sk-or-v1-first");
    await waitFor(() => expect(setOpenRouterKey).toHaveBeenCalledWith("sk-or-v1-first"));
    await waitFor(() => expect(costSummary).toHaveBeenCalledTimes(2));
  });

  it("reads it again once a background-only key is saved", async () => {
    render(<AiModelsSettings />);
    await waitFor(() => expect(costSummary).toHaveBeenCalledTimes(1));
    await saveIn(1, "sk-or-v1-background");
    await waitFor(() =>
      expect(setOpenRouterBackgroundKey).toHaveBeenCalledWith("sk-or-v1-background"),
    );
    await waitFor(() => expect(costSummary).toHaveBeenCalledTimes(2));
  });

  it("does not read it again when saving the key failed", async () => {
    setOpenRouterKey.mockRejectedValue(new Error("keychain locked"));
    render(<AiModelsSettings />);
    await waitFor(() => expect(costSummary).toHaveBeenCalledTimes(1));
    await saveIn(0, "sk-or-v1-first");
    await waitFor(() => expect(screen.getByText(/keychain locked/)).toBeTruthy());
    expect(costSummary).toHaveBeenCalledTimes(1);
  });
});

describe("the by-mod table", () => {
  const summary = (by_mod: unknown[]) => ({
    ...EMPTY_SUMMARY,
    by_mod,
  });
  const row = (extra: Record<string, unknown>) => ({
    mod_id: "acme.tool",
    name: "Acme Tool",
    prompt_tokens: 1200,
    completion_tokens: 300,
    request_count: 4,
    cost_usd: 0.1234,
    partial: false,
    ...extra,
  });

  it("is absent when no mod has used the AI", async () => {
    costSummary.mockResolvedValue(summary([]));
    render(<AiModelsSettings />);
    await screen.findByText(/Last 30 days/);
    expect(screen.queryByText("By mod (all time)")).toBeNull();
  });

  it("lists each mod with its requests, tokens and cost, and a removed mod under PM's words", async () => {
    costSummary.mockResolvedValue(
      summary([
        row({}),
        row({ mod_id: "gone.mod", name: null, request_count: 2, cost_usd: 0.5 }),
        row({ mod_id: "free.mod", name: "Free", cost_usd: null, partial: true }),
      ]),
    );
    render(<AiModelsSettings />);
    const heading = await screen.findByText("By mod (all time)");
    const table = heading.parentElement!;
    expect(within(table).getByText("Acme Tool")).toBeTruthy();
    expect(within(table).getByText("removed mod")).toBeTruthy();
    expect(within(table).getByText("$0.12")).toBeTruthy();
    expect(within(table).getByText("$0.50")).toBeTruthy();
    // A mod whose spend could not be priced shows a dash, never an understated $0.
    expect(within(table).getByText("—")).toBeTruthy();
    // The total is a floor when some could not be priced.
    expect(table.textContent).toContain("at least $0.62");
  });

  it("draws a mod's name as one plain line of text", async () => {
    costSummary.mockResolvedValue(summary([row({ name: "<b>Bold</b>‮evil\nsecond line" })]));
    render(<AiModelsSettings />);
    const heading = await screen.findByText("By mod (all time)");
    const table = heading.parentElement!;
    expect(table.querySelector("b")).toBeNull();
    expect(table.textContent).not.toContain("‮");
    expect(table.textContent).not.toContain("\n");
  });

  it("gives the exact total when every mod's spend was priced", async () => {
    costSummary.mockResolvedValue(
      summary([row({ cost_usd: 0.25 }), row({ mod_id: "b.mod", cost_usd: 0.25 })]),
    );
    render(<AiModelsSettings />);
    const heading = await screen.findByText("By mod (all time)");
    expect(heading.parentElement!.textContent).toContain("Mods together: $0.50");
    expect(heading.parentElement!.textContent).not.toContain("at least");
  });

  it("says 'at least' on a mod whose own line leaves something out, and not on a local-only mod", async () => {
    costSummary.mockResolvedValue(
      summary([
        row({ mod_id: "floor.mod", name: "Floor", cost_usd: 0.1, partial: true }),
        row({ mod_id: "local.mod", name: "Local", cost_usd: null, partial: false }),
      ]),
    );
    render(<AiModelsSettings />);
    const heading = await screen.findByText("By mod (all time)");
    const table = heading.parentElement!;
    const floorRow = within(table).getByText("Floor").closest("tr")!;
    expect(within(floorRow).getByText("at least $0.10")).toBeTruthy();
    const localRow = within(table).getByText("Local").closest("tr")!;
    expect(within(localRow).queryByText(/at least/)).toBeNull();
    // A dash would read as "couldn't be priced"; a local-only mod cost nothing.
    expect(within(localRow).getByText("No cost")).toBeTruthy();
    // The same mods, summed: a floor because one line is.
    expect(table.textContent).toContain("Mods together: at least $0.10");
  });
});

describe("the Usage tab's honest figures", () => {
  const line = (extra: Record<string, unknown>) => ({
    model: "anthropic/claude-sonnet-5-5",
    prompt_tokens: 12000,
    completion_tokens: 3000,
    request_count: 5,
    cost_usd: 0.12,
    served: "anthropic",
    cached_tokens: 0,
    thinking_tokens: 0,
    refused_requests: 0,
    unpriced_requests: 0,
    partial: false,
    ...extra,
  });
  const summary = (lines: unknown[], extra: Record<string, unknown> = {}) => ({
    ...EMPTY_SUMMARY,
    last_30d: lines,
    all_time: lines,
    total_30d_usd: 0.12,
    total_all_time_usd: 0.12,
    ...extra,
  });
  const tableRow = async (model: string) => {
    const cell = await screen.findByText(model, { exact: false });
    return cell.closest("tr")!;
  };

  it("names the road each line took, and says so for a row from before PM recorded it", async () => {
    costSummary.mockResolvedValue(
      summary([
        line({ model: "or/model", served: "openrouter" }),
        line({ model: "openai/gpt-x", served: "openai" }),
        line({ model: "gemini/g", served: "gemini" }),
        line({ model: "anthropic/c", served: "anthropic" }),
        line({ model: "gemma", served: "local", cost_usd: null }),
        line({ model: "old/model", served: null }),
      ]),
    );
    render(<AiModelsSettings />);
    expect(within(await tableRow("or/model")).getByText("OpenRouter")).toBeTruthy();
    expect(within(await tableRow("openai/gpt-x")).getByText("Your OpenAI key")).toBeTruthy();
    expect(within(await tableRow("gemini/g")).getByText("Your Gemini key")).toBeTruthy();
    expect(within(await tableRow("anthropic/c")).getByText("Your Anthropic key")).toBeTruthy();
    const local = await tableRow("gemma");
    expect(within(local).getByText("This computer")).toBeTruthy();
    expect(within(local).getByText("No cost")).toBeTruthy();
    expect(within(await tableRow("old/model")).getByText("Before PM recorded this")).toBeTruthy();
  });

  it("shows what the cache and the thinking did, and the refusals, under a model", async () => {
    costSummary.mockResolvedValue(
      summary([
        line({ model: "busy/m", cached_tokens: 1234, thinking_tokens: 3400, refused_requests: 2 }),
        line({ model: "quiet/m" }),
      ]),
    );
    render(<AiModelsSettings />);
    expect(
      within(await tableRow("busy/m")).getByText("Cached 1.2k, thinking 3.4k, 2 refused"),
    ).toBeTruthy();
    expect(within(await tableRow("quiet/m")).queryByText(/cached|thinking|refused/i)).toBeNull();
  });

  it("puts 'at least' on a floor, the count of calls it couldn't price, and why", async () => {
    costSummary.mockResolvedValue(
      summary([line({ model: "half/m", cost_usd: 0.12, partial: true, unpriced_requests: 3 })], {
        total_30d_partial: true,
        total_all_time_partial: true,
        unpriced_30d: 3,
      }),
    );
    render(<AiModelsSettings />);
    await screen.findByText("3 calls in the last 30 days couldn't be priced in full");
    expect(screen.getAllByText("at least $0.12").length).toBeGreaterThanOrEqual(3);
    // The explanation sits behind "Why?" and says what the provider's own page is for.
    fireEvent.click(screen.getByText("Why?"));
    expect(screen.getByText(/Your provider's own usage page has the exact figure\./)).toBeTruthy();
    // It names only what the code produces: no token count, priced in part, a token PM can't price.
    expect(screen.queryByText(/cancelled/)).toBeNull();
    expect(screen.getByText(/came back with no token count/)).toBeTruthy();
  });

  it("still explains an all-time 'at least' when no call in the last 30 days is unpriced", async () => {
    costSummary.mockResolvedValue(
      summary([line({ model: "old/m", cost_usd: 0.12, partial: true, unpriced_requests: 2 })], {
        total_all_time_partial: true,
        unpriced_30d: 0,
      }),
    );
    render(<AiModelsSettings />);
    await tableRow("old/m");
    expect(screen.getAllByText("at least $0.12").length).toBeGreaterThanOrEqual(1);
    // No 30-day count to print, but the reader still gets the way to the reason.
    expect(screen.queryByText(/in the last 30 days couldn't be priced/)).toBeNull();
    expect(screen.getByText("Why?")).toBeTruthy();
  });

  it("says nothing about unpriced calls when there are none, and an exact total has no 'at least'", async () => {
    costSummary.mockResolvedValue(summary([line({ model: "exact/m" })]));
    render(<AiModelsSettings />);
    await tableRow("exact/m");
    expect(screen.queryByText(/couldn't be priced/)).toBeNull();
    // No figure says "at least" (the folded method text mentions the words, so match a figure).
    expect(screen.queryAllByText(/^at least \$/)).toHaveLength(0);
    expect(screen.queryByText("Why?")).toBeNull();
    // No token type is named either: nothing in `partial_reasons`, no warning.
    expect(screen.queryByText(/PM can't price/)).toBeNull();
  });

  it("warns by name for each token type PM can't price, and for none when there is none", async () => {
    costSummary.mockResolvedValue(
      summary([line({ model: "typed/m", partial: true, unpriced_requests: 4 })], {
        total_30d_partial: true,
        unpriced_30d: 4,
        partial_reasons: [
          { slug: "audio_tokens", replies: 3 },
          { slug: "server_tool_use", replies: 1 },
          { slug: "new_token_type", replies: 2 },
          // A slug this build can't word is never printed.
          { slug: "<b>evil</b>", replies: 9 },
        ],
      }),
    );
    render(<AiModelsSettings />);
    await screen.findByText(
      "PM can't price audio tokens on 3 replies in the last 30 days, so these figures say 'at least'.",
    );
    expect(
      screen.getByText(
        "PM can't price tool use on 1 reply in the last 30 days, so these figures say 'at least'.",
      ),
    ).toBeTruthy();
    expect(
      screen.getByText(
        "PM can't price a new kind of token the provider started reporting on 2 replies in the last 30 days, so these figures say 'at least'.",
      ),
    ).toBeTruthy();
    expect(screen.queryByText(/evil/)).toBeNull();
  });

  // Background and Review have no strip, so this standing line is where a bad first row shows up.
  it("says how many replies a later row of the list answered, why the first moved, and when", async () => {
    costSummary.mockResolvedValue(
      summary([line({ model: "typed/m" })], {
        list_walks: { replies: 4, first_reason: "busy", first_at: "2026-11-03T12:00:00.000Z" },
      }),
    );
    render(<AiModelsSettings />);
    await screen.findByText(
      "4 replies in the last 30 days went to a later model in your list because an earlier one couldn't answer. The first was on 03-11-2026 (busy).",
    );
  });

  it("says nothing about a list's walks when there were none", async () => {
    costSummary.mockResolvedValue(summary([line({ model: "typed/m" })], { list_walks: null }));
    render(<AiModelsSettings />);
    await screen.findByText("typed/m");
    expect(screen.queryByTestId("list-walks")).toBeNull();
  });

  it("says models on this computer are listed and cost nothing, when Local AI is on", async () => {
    getLocalAiSwitch.mockResolvedValue({
      on: true,
      chat_needs_model: false,
      background_needs_model: false,
      server_saved: false,
    });
    costSummary.mockResolvedValue(
      summary([line({ model: "gemma", served: "local", cost_usd: null })]),
    );
    render(<AiModelsSettings />);
    await screen.findByText(
      "Models on this computer are listed with their request count. They don't cost anything.",
    );
    expect(screen.queryByText(/aren't counted here/)).toBeNull();
  });
});
