// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The first-run wizard's "Your own key" block with two providers. The line names every provider the
// app knows; a key saved when there is no OpenRouter key takes both roles for that provider; a second
// provider saved afterwards never takes a role that has already left OpenRouter; and the models line
// names Sonnet and Haiku, never Fable.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const m = vi.hoisted(() => ({
  setDirectOptIn: vi.fn(),
  saveDirectKey: vi.fn(),
  recheckDirectKey: vi.fn(),
  clearDirectKey: vi.fn(),
  setDirectDataSharing: vi.fn(),
  setRoleCloudProvider: vi.fn(),
}));

vi.mock("../../lib/ipc", () => ({
  setDirectOptIn: (...a: unknown[]) => m.setDirectOptIn(...a),
  saveDirectKey: (...a: unknown[]) => m.saveDirectKey(...a),
  recheckDirectKey: (...a: unknown[]) => m.recheckDirectKey(...a),
  clearDirectKey: (...a: unknown[]) => m.clearDirectKey(...a),
  setDirectDataSharing: (...a: unknown[]) => m.setDirectDataSharing(...a),
  setRoleCloudProvider: (...a: unknown[]) => m.setRoleCloudProvider(...a),
}));
vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { anthropicView, openaiView, stateWith } from "../../lib/directProvidersFixture";
import type { DirectProvidersState } from "../../lib/types";
import { OnboardingDirectKey } from "./OnboardingDirectKey";

const next: { current: DirectProvidersState | null } = { current: null };

function Harness({ initial, hasOr }: { initial: DirectProvidersState; hasOr: boolean }) {
  const [state, setState] = useState(initial);
  const dp = {
    state,
    error: null,
    reload: async () => {
      const n = next.current ?? state;
      setState(n);
      return n;
    },
  };
  return <OnboardingDirectKey dp={dp} hasOpenRouterKey={hasOr} onError={() => {}} />;
}

const FAKE = "sk-ant-api03-test-key-not-real-0123456789";
const OK = { outcome: "ok", saved: true, listed: [], listed_total: 15 };

beforeEach(() => {
  next.current = null;
  m.saveDirectKey.mockResolvedValue(OK);
  m.setRoleCloudProvider.mockResolvedValue(undefined);
});
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("the wizard with Anthropic", () => {
  it("names every provider in its line", () => {
    render(
      <Harness
        initial={stateWith([openaiView({ opted_in: false }), anthropicView({ opted_in: false })])}
        hasOr={false}
      />,
    );
    expect(
      screen.getByText(
        "Rather not use OpenRouter? Use your own OpenAI, Google Gemini or Anthropic key. It's off unless you turn it on, and it isn't zero data retention.",
      ),
    ).toBeTruthy();
    expect(screen.getByRole("switch", { name: "Use my own Anthropic key" })).toBeTruthy();
  });

  it("points both roles at Anthropic when its key is saved first and OpenRouter has no key", async () => {
    const before = stateWith([
      openaiView({ opted_in: false }),
      anthropicView({ key: "absent", key_check: null }),
    ]);
    next.current = stateWith([openaiView({ opted_in: false }), anthropicView()]);
    render(<Harness initial={before} hasOr={false} />);
    fireEvent.change(screen.getByLabelText("Anthropic API key"), { target: { value: FAKE } });
    fireEvent.submit(screen.getByLabelText("Anthropic API key").closest("form")!);
    await waitFor(() => expect(m.setRoleCloudProvider).toHaveBeenCalledTimes(2));
    expect(m.setRoleCloudProvider).toHaveBeenCalledWith("chat", "anthropic");
    expect(m.setRoleCloudProvider).toHaveBeenCalledWith("background", "anthropic");
  });

  it("does not take a role that already left OpenRouter for OpenAI", async () => {
    // OpenAI was saved first and holds both roles; Anthropic is saved second.
    const before = stateWith([openaiView(), anthropicView({ key: "absent", key_check: null })], {
      chat: "openai",
      background: "openai",
    });
    next.current = stateWith([openaiView(), anthropicView()], {
      chat: "openai",
      background: "openai",
    });
    render(<Harness initial={before} hasOr={false} />);
    fireEvent.change(screen.getByLabelText("Anthropic API key"), { target: { value: FAKE } });
    fireEvent.submit(screen.getByLabelText("Anthropic API key").closest("form")!);
    // adopt() has run once the wizard says the key is saved; it found no role on OpenRouter to take.
    await screen.findAllByText("✓ Saved to your keychain.");
    await new Promise((r) => setTimeout(r, 20));
    expect(m.saveDirectKey).toHaveBeenCalledTimes(1);
    expect(m.setRoleCloudProvider).not.toHaveBeenCalled();
  });

  it("says Sonnet and Haiku will answer, and never names Fable", async () => {
    const state = stateWith([anthropicView()], { chat: "anthropic", background: "anthropic" });
    render(<Harness initial={state} hasOr={false} />);
    expect(
      screen.getByText(
        "PM will use Claude Sonnet 5.5 for chat and Claude Haiku 5.5 for background work. You can change that later in Settings > AI & Models.",
      ),
    ).toBeTruthy();
  });
});
