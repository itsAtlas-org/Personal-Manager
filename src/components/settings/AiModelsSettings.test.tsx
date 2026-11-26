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

const { costSummary, setOpenRouterKey, setOpenRouterBackgroundKey } = vi.hoisted(() => ({
  costSummary: vi.fn(),
  setOpenRouterKey: vi.fn(),
  setOpenRouterBackgroundKey: vi.fn(),
}));

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
};

// Spread over the real module, so a wrapper this tab doesn't call here stays as it is.
vi.mock("../../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  costSummary: () => costSummary(),
  hasOpenRouterKey: () => Promise.resolve(false),
  hasOpenRouterBackgroundKey: () => Promise.resolve(false),
  getSettings: () => Promise.resolve(SETTINGS),
  settingsDefaults: () => Promise.resolve(SETTINGS),
  getLocalAiSwitch: () => Promise.reject(new Error("not in this test")),
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

beforeEach(() => {
  costSummary.mockReturnValue(new Promise(() => {}));
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
    last_30d: [],
    all_time: [],
    by_mod,
    total_30d_usd: 0,
    total_all_time_usd: 0,
    pricing_updated_at: null,
  });
  const row = (extra: Record<string, unknown>) => ({
    mod_id: "acme.tool",
    name: "Acme Tool",
    prompt_tokens: 1200,
    completion_tokens: 300,
    request_count: 4,
    cost_usd: 0.1234,
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
        row({ mod_id: "free.mod", name: "Free", cost_usd: null }),
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
});
