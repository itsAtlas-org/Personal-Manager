// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The context meter names the route as well as the model: the same id can mean a different window on
// another provider, so "gpt-6.2-sol" alone would not say whose window the percentage is of. On
// OpenRouter it reads exactly as it always did.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const m = vi.hoisted(() => ({ chatContextStatus: vi.fn() }));

vi.mock("../lib/ipc", () => ({
  chatContextStatus: (...a: unknown[]) => m.chatContextStatus(...a),
  compressChat: vi.fn(),
  revertCompress: vi.fn(),
}));
vi.mock("../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import type { ContextStatus } from "../lib/types";
import { ContextMeter } from "./ContextMeter";

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

function status(over: Partial<ContextStatus>): ContextStatus {
  return {
    model: "anthropic/claude-sonnet-4.6",
    context_window: 200_000,
    used_tokens: 100_000,
    percent: 0.5,
    alerting: false,
    window_source: null,
    window_proven: false,
    compress: { kind: "no" } as unknown as ContextStatus["compress"],
    upgrade: [],
    provider: "openrouter",
    ...over,
  };
}

const meter = () => screen.findByRole("button", { name: /context/i });

describe("the context meter's naming", () => {
  it("names the model alone on OpenRouter, as it always did", async () => {
    m.chatContextStatus.mockResolvedValue(status({}));
    render(<ContextMeter conversationId={1} refreshKey={0} onUpgrade={() => Promise.resolve()} />);
    const button = await meter();
    expect(button.getAttribute("title")).toBe("50% of anthropic/claude-sonnet-4.6's context used");
  });

  it("names the provider after a model reached with the person's own key", async () => {
    m.chatContextStatus.mockResolvedValue(
      status({ model: "openai/gpt-6.2-sol", context_window: 1_050_000, provider: "openai" }),
    );
    render(<ContextMeter conversationId={1} refreshKey={0} onUpgrade={() => Promise.resolve()} />);
    const button = await meter();
    expect(button.getAttribute("title")).toBe("50% of openai/gpt-6.2-sol on OpenAI's context used");
    fireEvent.click(button);
    await waitFor(() =>
      expect(screen.getByText("openai/gpt-6.2-sol on OpenAI's context is 50% full.")).toBeTruthy(),
    );
  });

  it("names Anthropic after a model reached with an Anthropic key, on its 872,000-token room", async () => {
    m.chatContextStatus.mockResolvedValue(
      status({
        model: "anthropic/claude-sonnet-5-5",
        context_window: 872_000,
        provider: "anthropic",
      }),
    );
    render(<ContextMeter conversationId={1} refreshKey={0} onUpgrade={() => Promise.resolve()} />);
    const button = await meter();
    expect(button.getAttribute("title")).toBe(
      "50% of anthropic/claude-sonnet-5-5 on Anthropic's context used",
    );
  });

  it("names Google Gemini after a model reached with a Gemini key", async () => {
    m.chatContextStatus.mockResolvedValue(
      status({
        model: "gemini/gemini-3.8-flash",
        context_window: 1_048_576,
        provider: "gemini",
      }),
    );
    render(<ContextMeter conversationId={1} refreshKey={0} onUpgrade={() => Promise.resolve()} />);
    const button = await meter();
    expect(button.getAttribute("title")).toBe(
      "50% of gemini/gemini-3.8-flash on Google Gemini's context used",
    );
  });

  it("reads an older answer with no provider as OpenRouter", async () => {
    const old = status({}) as Partial<ContextStatus>;
    delete old.provider;
    m.chatContextStatus.mockResolvedValue(old);
    render(<ContextMeter conversationId={1} refreshKey={0} onUpgrade={() => Promise.resolve()} />);
    expect((await meter()).getAttribute("title")).not.toContain(" on ");
  });
});
