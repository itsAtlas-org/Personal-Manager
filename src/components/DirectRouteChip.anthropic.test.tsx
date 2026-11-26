// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The composer chip for a chat on Anthropic: its own label and title, never OpenAI's or the shared
// one, nothing while a local model answers, and "no key" only once PM has read that it isn't there.

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const m = vi.hoisted(() => ({ directProvidersState: vi.fn() }));

vi.mock("../lib/ipc", () => ({
  directProvidersState: () => m.directProvidersState(),
}));

import { anthropicView, directState, settingsFor, SONNET } from "../lib/directProvidersFixture";
import type { LocalLlmStatus } from "../lib/types";
import { DirectRouteChip } from "./DirectRouteChip";

const settings = settingsFor({ chat_provider: "anthropic", chat_cloud_model: SONNET.id });

beforeEach(() => {
  m.directProvidersState.mockResolvedValue(
    directState({ view: anthropicView(), chat: "anthropic" }),
  );
});
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("the composer chip on Anthropic", () => {
  it("says Anthropic direct, with what that means in its title, and never the OpenAI label", async () => {
    render(<DirectRouteChip settings={settings} localAi={null} />);
    const chip = await screen.findByText("Anthropic direct");
    expect(chip.getAttribute("title")).toBe(
      "Chat goes straight to Anthropic with your own key. It isn't zero data retention. Change it in Settings > AI & Models.",
    );
    expect(screen.queryByText(/OpenAI/)).toBeNull();
  });

  it("is hidden while a local model is the one answering", () => {
    const { container } = render(
      <DirectRouteChip
        settings={settings}
        localAi={{ chat_local_model: "qwen3:8b" } as unknown as LocalLlmStatus}
      />,
    );
    expect(container.textContent).toBe("");
  });

  it("says no key, and no credit, with Anthropic's name", async () => {
    m.directProvidersState.mockResolvedValue(
      directState({
        view: anthropicView({ key: "absent", key_check: null }),
        chat: "anthropic",
      }),
    );
    const { unmount } = render(<DirectRouteChip settings={settings} localAi={null} />);
    expect(await screen.findByText("Anthropic: no key")).toBeTruthy();
    unmount();
    m.directProvidersState.mockResolvedValue(
      directState({
        view: anthropicView({
          key_check: { at: "x", outcome: "no_credit", listed: [], listed_total: 0 },
        }),
        chat: "anthropic",
      }),
    );
    render(<DirectRouteChip settings={settings} localAi={null} />);
    const chip = await screen.findByText("Anthropic: no credit");
    expect(chip.getAttribute("title")).toContain("Your Anthropic account is out of credit");
  });
});
