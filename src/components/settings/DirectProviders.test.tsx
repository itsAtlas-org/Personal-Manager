// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Your own keys" with every provider: one card each, in the order Rust lists them, and an intro
// that names them all.

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../../lib/ipc", () => ({}));
vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { anthropicView, geminiView, openaiView, stateWith } from "../../lib/directProvidersFixture";
import { DirectProviders } from "./DirectProviders";

afterEach(cleanup);

describe("the Your own keys section", () => {
  it("draws a card per provider, in the order given, and names them all in the intro", () => {
    const state = stateWith([
      openaiView({ opted_in: false }),
      geminiView({ opted_in: false }),
      anthropicView({ opted_in: false }),
    ]);
    render(
      <DirectProviders
        dp={{ state, error: null, reload: async () => state }}
        orKey={{ chat: true, background: true }}
      />,
    );
    const switches = screen.getAllByRole("switch").map((s) => s.getAttribute("aria-checked"));
    expect(switches).toEqual(["false", "false", "false"]);
    const labels = screen.getAllByText(/^Use my own /).map((e) => e.textContent);
    expect(labels).toEqual([
      "Use my own OpenAI key",
      "Use my own Google Gemini key",
      "Use my own Anthropic key",
    ]);
    expect(
      screen.getByText(
        /Use your own key from OpenAI, Google Gemini or Anthropic instead of OpenRouter/,
      ),
    ).toBeTruthy();
  });

  it("shows only the cards the backend lists", () => {
    const state = stateWith([openaiView({ opted_in: false })]);
    render(
      <DirectProviders
        dp={{ state, error: null, reload: async () => state }}
        orKey={{ chat: true, background: true }}
      />,
    );
    expect(screen.queryByText("Use my own Anthropic key")).toBeNull();
  });
});
