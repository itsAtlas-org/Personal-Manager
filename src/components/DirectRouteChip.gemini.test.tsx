// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The composer chip on a Gemini route: it says the TERMS the route is on (paid, free, paused), takes
// them from Settings, where Rust resolved them for the role, and falls back to the provider's own
// view only until Settings has them. A free route and a paused one wear the due tint; a paid one is
// neutral ink, because a paid key is a choice made and not a fault.

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const m = vi.hoisted(() => ({ directProvidersState: vi.fn() }));

vi.mock("../lib/ipc", () => ({
  directProvidersState: () => m.directProvidersState(),
}));

import { geminiView, settingsFor, stateWith } from "../lib/directProvidersFixture";
import type { RegionRefusal } from "../lib/types";
import { DirectRouteChip } from "./DirectRouteChip";

const UK: RegionRefusal = {
  kind: "uk",
  label: "the United Kingdom",
  zone: "Europe/London",
  basis: "free",
};
const chat = (terms: "paid" | "free" | "blocked" | null) =>
  settingsFor({
    chat_provider: "gemini",
    chat_cloud_model: "gemini/gemini-3.8-flash",
    chat_route_terms: terms,
  });
const FREE = {
  at: "x",
  outcome: "ok" as const,
  listed: [],
  listed_total: 49,
  tier: "free" as const,
};

beforeEach(() => {
  m.directProvidersState.mockResolvedValue(stateWith([geminiView()], { chat: "gemini" }));
});
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("the composer chip on Gemini", () => {
  it("says paid, in neutral ink, with the paid terms in its title", async () => {
    render(<DirectRouteChip settings={chat("paid")} localAi={null} />);
    const chip = await screen.findByText("Gemini direct, paid");
    expect(chip.getAttribute("title")).toBe(
      "Chat goes straight to Google Gemini with your own key, on Google's paid terms. It isn't zero data retention. Change it in Settings > AI & Models.",
    );
    expect(chip.getAttribute("style")).toContain("--ink3");
  });

  it("says free, in the due tint, and says Google may train", async () => {
    m.directProvidersState.mockResolvedValue(
      stateWith([geminiView({ tier: "free", key_check: FREE })], { chat: "gemini" }),
    );
    render(<DirectRouteChip settings={chat("free")} localAi={null} />);
    const chip = await screen.findByText("Gemini direct, free");
    expect(chip.getAttribute("title")).toBe(
      "Chat goes straight to Google Gemini with a key PM treats as free. If it is free, Google may train on what you send and people at Google may read it. Change it in Settings > AI & Models.",
    );
    expect(chip.getAttribute("style")).toContain("--st-due");
  });

  it("says paused, with where PM thinks you are, in the due tint", async () => {
    m.directProvidersState.mockResolvedValue(
      stateWith([geminiView({ tier: "free", key_check: FREE, blocked: UK })], { chat: "gemini" }),
    );
    render(<DirectRouteChip settings={chat("blocked")} localAi={null} />);
    const chip = await screen.findByText("Gemini paused");
    expect(chip.getAttribute("title")).toBe(
      "Chat is set to Google Gemini, but your key reads as free and PM thinks you're in the United Kingdom. Chat can't answer until you fix this. Open Settings > AI & Models.",
    );
    expect(chip.getAttribute("style")).toContain("--st-due");
  });

  it("takes the terms from the provider's own view until Settings has them", async () => {
    render(<DirectRouteChip settings={chat(null)} localAi={null} />);
    expect(await screen.findByText("Gemini direct, paid")).toBeTruthy();
    cleanup();
    m.directProvidersState.mockResolvedValue(
      stateWith([geminiView({ tier: null })], { chat: "gemini" }),
    );
    render(<DirectRouteChip settings={chat(null)} localAi={null} />);
    // A key with no verdict reads as free.
    expect(await screen.findByText("Gemini direct, free")).toBeTruthy();
  });

  it("lets Settings win over a view that has not caught up", async () => {
    // The view still says paid; a failing call has since taught Rust the project is free.
    render(<DirectRouteChip settings={chat("free")} localAi={null} />);
    expect(await screen.findByText("Gemini direct, free")).toBeTruthy();
  });

  it("still says no credit, as a fault, for an empty prepay balance", async () => {
    m.directProvidersState.mockResolvedValue(
      stateWith(
        [
          geminiView({
            key_check: {
              at: "x",
              outcome: "no_credit",
              listed: [],
              listed_total: 49,
              tier: "paid",
            },
          }),
        ],
        { chat: "gemini" },
      ),
    );
    render(<DirectRouteChip settings={chat("paid")} localAi={null} />);
    const chip = await screen.findByText("Google Gemini: no credit");
    expect(chip.getAttribute("style")).toContain("--st-due");
  });
});
