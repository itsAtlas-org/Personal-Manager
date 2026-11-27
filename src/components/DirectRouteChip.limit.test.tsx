// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The composer chip when the person's own daily limit has paused the provider chat is set to: it says
// "Paused for today" in the due tint, and its title is the standing pause line, so the reason is
// there before a reply fails.

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const m = vi.hoisted(() => ({ directProvidersState: vi.fn() }));

vi.mock("../lib/ipc", () => ({
  directProvidersState: () => m.directProvidersState(),
}));

import { directState, limitView, openaiView, settingsFor } from "../lib/directProvidersFixture";
import { DirectRouteChip } from "./DirectRouteChip";

const direct = settingsFor({ chat_provider: "openai", chat_cloud_model: "openai/gpt-6.2-sol" });

beforeEach(() => {
  m.directProvidersState.mockResolvedValue(directState({ chat: "openai" }));
});
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("the composer chip under a daily limit", () => {
  it("says Paused for today, in the due tint, with the standing line as its title", async () => {
    m.directProvidersState.mockResolvedValue(
      directState({
        view: openaiView({
          limit: limitView({ limit_usd: 5, spent_today_usd: 5.12, reached: true }),
        }),
        chat: "openai",
      }),
    );
    render(<DirectRouteChip settings={direct} localAi={null} />);
    const chip = await screen.findByText("OpenAI: Paused for today");
    expect(chip.getAttribute("style")).toContain("--st-due");
    expect(chip.getAttribute("title")).toBe(
      "Paused for today. PM has logged at least $5.12 of your $5.00 limit. It starts again at 00:00 (your day), or sooner if you raise the limit. Change it in Settings > AI & Models.",
    );
  });

  it("does not promise midnight for a limit PM could not read, and names the provider", async () => {
    m.directProvidersState.mockResolvedValue(
      directState({
        view: openaiView({
          limit: limitView({ limit_usd: null, reached: true, unreadable: true }),
        }),
        chat: "openai",
      }),
    );
    render(<DirectRouteChip settings={direct} localAi={null} />);
    const chip = await screen.findByText("OpenAI: Paused");
    expect(chip.getAttribute("title")).toBe(
      "PM couldn't read today's usage, so it paused OpenAI to be safe. Check Usage, or turn the limit off. Change it in Settings > AI & Models.",
    );
  });

  it("is the ordinary chip while the limit is not reached", async () => {
    m.directProvidersState.mockResolvedValue(
      directState({
        view: openaiView({ limit: limitView({ limit_usd: 5, spent_today_usd: 1 }) }),
        chat: "openai",
      }),
    );
    render(<DirectRouteChip settings={direct} localAi={null} />);
    expect(await screen.findByText("OpenAI direct")).toBeTruthy();
    expect(screen.queryByText(/Paused for today/)).toBeNull();
  });

  it("reads the state again when a send finishes, so the reply that hit the limit shows at once", async () => {
    m.directProvidersState.mockResolvedValue(
      directState({
        view: openaiView({ limit: limitView({ limit_usd: 5, spent_today_usd: 4.9 }) }),
        chat: "openai",
      }),
    );
    const { rerender } = render(<DirectRouteChip settings={direct} localAi={null} busy={false} />);
    expect(await screen.findByText("OpenAI direct")).toBeTruthy();
    // The send starts, crosses the line, and finishes: nothing in Settings moved.
    m.directProvidersState.mockResolvedValue(
      directState({
        view: openaiView({
          limit: limitView({ limit_usd: 5, spent_today_usd: 5.12, reached: true }),
        }),
        chat: "openai",
      }),
    );
    rerender(<DirectRouteChip settings={direct} localAi={null} busy />);
    expect(screen.queryByText("OpenAI: Paused for today")).toBeNull();
    rerender(<DirectRouteChip settings={direct} localAi={null} busy={false} />);
    expect(await screen.findByText("OpenAI: Paused for today")).toBeTruthy();
  });

  it("reads the state again when the pause lifts at midnight, with nothing else changing", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.setSystemTime(new Date("2026-11-27T23:58:00Z"));
    try {
      m.directProvidersState.mockResolvedValue(
        directState({
          view: openaiView({
            limit: limitView({
              limit_usd: 5,
              spent_today_usd: 5.12,
              reached: true,
              resumes_at: "2026-11-28T00:00:00+00:00",
            }),
          }),
          chat: "openai",
        }),
      );
      render(<DirectRouteChip settings={direct} localAi={null} />);
      await screen.findByText("OpenAI: Paused for today");
      // Midnight in the person's day: the backend now says the provider is free again.
      m.directProvidersState.mockResolvedValue(directState({ chat: "openai" }));
      await vi.advanceTimersByTimeAsync(119_000);
      expect(screen.queryByText("OpenAI direct")).toBeNull();
      await vi.advanceTimersByTimeAsync(3_000);
      expect(await screen.findByText("OpenAI direct")).toBeTruthy();
    } finally {
      vi.useRealTimers();
    }
  });
});
