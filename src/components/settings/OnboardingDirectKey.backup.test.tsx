// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The first-run wizard's one backup offer: "If X isn't working, try Y next." It is UNTICKED, shown
// only where the other provider could answer at once, adds a row only when ticked, and takes the same
// row out when unticked. A person who never ticks it gets exactly the lists they had.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const m = vi.hoisted(() => ({
  routeAdd: vi.fn(),
  routeRemove: vi.fn(),
  routeStatus: vi.fn(),
  setRoleCloudProvider: vi.fn(),
}));

vi.mock("../../lib/ipc", () => ({
  routeAdd: (...a: unknown[]) => m.routeAdd(...a),
  routeRemove: (...a: unknown[]) => m.routeRemove(...a),
  routeStatus: () => m.routeStatus(),
  onRouteChanged: () => Promise.resolve(() => {}),
  setRoleCloudProvider: (...a: unknown[]) => m.setRoleCloudProvider(...a),
}));
vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { anthropicView, geminiView, openaiView, stateWith } from "../../lib/directProvidersFixture";
import { claudeRow, geminiRow, orRow, routeView } from "../../lib/routeFixture";
import type { DirectProvidersState } from "../../lib/types";
import { OnboardingDirectKey } from "./OnboardingDirectKey";

const OR_MODELS = { chat: "a/chat", background: "a/bg" };
const onError = vi.fn();

function show(state: DirectProvidersState, hasOr: boolean) {
  const dp = { state, error: null, reload: async () => state };
  return render(
    <OnboardingDirectKey
      dp={dp}
      hasOpenRouterKey={hasOr}
      openRouterModels={OR_MODELS}
      onError={onError}
    />,
  );
}

// The Anthropic and OpenAI cards have checkboxes of their own; this is the backup offer's.
const box = () =>
  screen.queryByRole("checkbox", { name: /isn't working, try/ }) as HTMLInputElement | null;

beforeEach(() => {
  m.routeAdd.mockResolvedValue({ route: routeView([]), materialised: true, unused_saved: 0 });
  m.routeRemove.mockResolvedValue({ route: routeView([]), materialised: false, unused_saved: 0 });
  m.routeStatus.mockResolvedValue({
    chat: routeView([claudeRow()]),
    background: routeView([claudeRow()]),
  });
});
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("the backup checkbox", () => {
  it("is unticked, says which provider backs which, and writes nothing until it is ticked", async () => {
    show(stateWith([anthropicView()], { chat: "anthropic", background: "anthropic" }), true);
    await screen.findByText("If Anthropic isn't working, try OpenRouter next.");
    expect(box()?.checked).toBe(false);
    expect(m.routeAdd).not.toHaveBeenCalled();
  });

  it("ticking adds OpenRouter's model behind each role that is on Anthropic, and only then", async () => {
    show(stateWith([anthropicView()], { chat: "anthropic", background: "anthropic" }), true);
    fireEvent.click(await screen.findByRole("checkbox", { name: /isn't working, try/ }));
    await waitFor(() => expect(m.routeAdd).toHaveBeenCalledTimes(2));
    expect(m.routeAdd).toHaveBeenCalledWith("chat", "openrouter", "a/chat");
    expect(m.routeAdd).toHaveBeenCalledWith("background", "openrouter", "a/bg");
    await waitFor(() => expect(box()?.checked).toBe(true));
  });

  it("unticking takes the same rows out again and leaves the person's own row", async () => {
    show(stateWith([anthropicView()], { chat: "anthropic", background: "anthropic" }), true);
    fireEvent.click(await screen.findByRole("checkbox", { name: /isn't working, try/ }));
    await waitFor(() => expect(box()?.checked).toBe(true));
    m.routeStatus.mockResolvedValue({
      chat: routeView([claudeRow(), orRow("a/chat")]),
      background: routeView([claudeRow(), orRow("a/bg")]),
    });
    // What the backend answers once the rows are out again.
    m.routeRemove.mockImplementation(async () => {
      m.routeStatus.mockResolvedValue({
        chat: routeView([claudeRow()]),
        background: routeView([claudeRow()]),
      });
      return { route: routeView([]), materialised: false, unused_saved: 0 };
    });
    fireEvent.click(screen.getByRole("checkbox", { name: /isn't working, try/ }));
    await waitFor(() => expect(m.routeRemove).toHaveBeenCalledTimes(2));
    expect(m.routeRemove).toHaveBeenCalledWith("chat", 1, "openrouter:a/chat");
    expect(m.routeRemove).toHaveBeenCalledWith("background", 1, "openrouter:a/bg");
    await waitFor(() => expect(box()?.checked).toBe(false));
  });

  it("is not shown when nothing else could answer at once", async () => {
    // No OpenRouter key, and the only other provider has no key.
    show(
      stateWith([anthropicView(), openaiView({ key: "absent", key_check: null })], {
        chat: "anthropic",
        background: "anthropic",
      }),
      false,
    );
    await screen.findByText(/Chat uses/);
    expect(box()).toBeNull();
  });

  it("is not shown for another provider that is not turned on, or is paused by its limit", async () => {
    const state = stateWith([anthropicView(), openaiView({ opted_in: false })], {
      chat: "anthropic",
      background: "anthropic",
    });
    show(state, false);
    await screen.findByText(/Chat uses/);
    expect(box()).toBeNull();
    cleanup();
    const limited = openaiView({
      limit: { ...openaiView().limit, limit_usd: 5, spent_today_usd: 5, reached: true },
    });
    show(
      stateWith([anthropicView(), limited], { chat: "anthropic", background: "anthropic" }),
      false,
    );
    await screen.findByText(/Chat uses/);
    expect(box()).toBeNull();
  });

  it("offers another provider behind OpenRouter when the roles are still on OpenRouter", async () => {
    m.routeStatus.mockResolvedValue({
      chat: routeView([orRow("a/chat")]),
      background: routeView([orRow("a/bg")]),
    });
    show(stateWith([anthropicView()]), true);
    await screen.findByText("If OpenRouter isn't working, try Anthropic next.");
    fireEvent.click(screen.getByRole("checkbox", { name: /isn't working, try/ }));
    await waitFor(() => expect(m.routeAdd).toHaveBeenCalledTimes(2));
    expect(m.routeAdd).toHaveBeenCalledWith("chat", "anthropic", "anthropic/claude-sonnet-5-5");
    expect(m.routeAdd).toHaveBeenCalledWith(
      "background",
      "anthropic",
      "anthropic/claude-haiku-5-5",
    );
  });

  it("never adds a key that reads as free behind Background", async () => {
    m.routeStatus.mockResolvedValue({
      chat: routeView([orRow("a/chat")]),
      background: routeView([orRow("a/bg")]),
    });
    show(stateWith([geminiView({ tier: "free" })]), true);
    await screen.findByText("If OpenRouter isn't working, try Google Gemini next.");
    fireEvent.click(screen.getByRole("checkbox", { name: /isn't working, try/ }));
    await waitFor(() => expect(m.routeAdd).toHaveBeenCalledTimes(1));
    expect(m.routeAdd).toHaveBeenCalledWith("chat", "gemini", "gemini/gemini-3.8-flash");
  });

  it("does not add a row a list already has", async () => {
    m.routeStatus.mockResolvedValue({
      chat: routeView([claudeRow(), orRow("a/chat")]),
      background: routeView([claudeRow()]),
    });
    show(stateWith([anthropicView()], { chat: "anthropic", background: "anthropic" }), true);
    fireEvent.click(await screen.findByRole("checkbox", { name: /isn't working, try/ }));
    await waitFor(() => expect(m.routeAdd).toHaveBeenCalledTimes(1));
    expect(m.routeAdd).toHaveBeenCalledWith("background", "openrouter", "a/bg");
  });

  it("a refused add says why, puts back what was added, and leaves the box unticked", async () => {
    m.routeAdd
      .mockResolvedValueOnce({ route: routeView([]), materialised: true, unused_saved: 0 })
      .mockRejectedValueOnce("OpenRouter has no key.");
    show(stateWith([anthropicView()], { chat: "anthropic", background: "anthropic" }), true);
    fireEvent.click(await screen.findByRole("checkbox", { name: /isn't working, try/ }));
    await waitFor(() => expect(onError).toHaveBeenCalledWith("OpenRouter has no key."));
    m.routeStatus.mockResolvedValue({
      chat: routeView([claudeRow(), orRow("a/chat")]),
      background: routeView([claudeRow()]),
    });
    await waitFor(() => expect(box()?.checked).toBe(false));
    expect(geminiRow).toBeDefined();
  });
});
