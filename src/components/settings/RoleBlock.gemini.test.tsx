// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A role on Google Gemini. A role whose key reads free where PM only uses paid ones, or whose prepay
// balance is empty, is NOT moved: it says so in words and offers the way out, "Use OpenRouter". The
// effort options are the model's own native levels, and a provider that can't be picked is still
// named in the Select so the person sees what the role is on.

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const m = vi.hoisted(() => ({
  setRoleCloudProvider: vi.fn(),
  setDirectRole: vi.fn(),
  listModels: vi.fn(),
}));

vi.mock("../../lib/ipc", () => ({
  setRoleCloudProvider: (...a: unknown[]) => m.setRoleCloudProvider(...a),
  setDirectRole: (...a: unknown[]) => m.setDirectRole(...a),
  listModels: () => m.listModels(),
}));
vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { geminiView, stateWith } from "../../lib/directProvidersFixture";
import type { DirectProvidersState, RegionRefusal, RoleName } from "../../lib/types";
import { RoleBlock } from "./RoleBlock";

const UK: RegionRefusal = {
  kind: "uk",
  label: "the United Kingdom",
  zone: "Europe/London",
  basis: "free",
};
const FREE = {
  at: "x",
  outcome: "ok" as const,
  listed: [],
  listed_total: 49,
  tier: "free" as const,
};
const next: { current: DirectProvidersState | null } = { current: null };

function Harness({ state, role = "chat" }: { state: DirectProvidersState; role?: RoleName }) {
  const dp = {
    state,
    error: null,
    reload: async () => next.current ?? state,
  };
  return (
    <RoleBlock
      role={role}
      label={role === "chat" ? "Chat model" : "Background model"}
      description="OpenRouter's own words."
      helpId="settings-chat-models"
      dp={dp}
      routing="cloud"
      openRouter={{
        models: ["anthropic/claude-sonnet-4.6"],
        onChange: () => {},
        autoSwitch: true,
        onAutoSwitchChange: () => {},
      }}
      onError={() => {}}
    />
  );
}

beforeEach(() => {
  next.current = null;
  m.setRoleCloudProvider.mockResolvedValue(undefined);
  m.setDirectRole.mockResolvedValue(undefined);
  m.listModels.mockResolvedValue([]);
});
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

const select = (who = "Chat") => screen.getByLabelText(`${who} provider`) as HTMLSelectElement;

describe("a role on a paid Gemini key", () => {
  it("has no issue line, and lists Google Gemini as a choice", () => {
    render(<Harness state={stateWith([geminiView()], { chat: "gemini" })} />);
    expect([...select().options].map((o) => o.textContent)).toEqual([
      "OpenRouter (asks for zero data retention)",
      "Google Gemini, direct (no zero data retention)",
    ]);
    expect(screen.queryByText(/is paused/)).toBeNull();
    expect(screen.queryByRole("button", { name: "Use OpenRouter for Chat" })).toBeNull();
  });

  it("offers the model's own levels: 3.8 Flash has no Minimal, and chat names its default", () => {
    render(<Harness state={stateWith([geminiView()], { chat: "gemini" })} />);
    const effort = screen.getByLabelText("Chat effort") as HTMLSelectElement;
    expect([...effort.options].map((o) => o.textContent)).toEqual([
      "Model default (Medium)",
      "Low",
      "Medium",
      "High",
    ]);
  });

  it("offers Minimal on Flash-Lite for background, with Low as PM's default", () => {
    render(
      <Harness role="background" state={stateWith([geminiView()], { background: "gemini" })} />,
    );
    const effort = screen.getByLabelText("Background effort") as HTMLSelectElement;
    expect([...effort.options].map((o) => o.textContent)).toEqual([
      "Minimal",
      "Low (PM's default)",
      "Medium",
      "High",
    ]);
  });
});

describe("a role on a paused Gemini key", () => {
  const paused = () =>
    stateWith([geminiView({ tier: "free", key_check: FREE, blocked: UK })], { chat: "gemini" });

  it("says it is paused and why, in a status region, with Use OpenRouter beside it", () => {
    render(<Harness state={paused()} />);
    const line = screen.getByText(
      "Chat is set to Google Gemini, but Gemini is paused (your key reads as free and PM thinks you're in the United Kingdom). Chat won't answer until the project is paid, or until you pick another provider.",
    );
    expect(line.closest('[role="status"]')).not.toBeNull();
    expect(screen.getByRole("button", { name: "Use OpenRouter for Chat" })).toBeTruthy();
  });

  it("does not say the role 'goes straight to' Google above its own paused line", () => {
    render(<Harness state={paused()} />);
    expect(screen.queryByText(/goes straight to Google Gemini with your key/)).toBeNull();
  });

  it("lists the provider as paused in the Select, and nothing else as choosable", () => {
    render(<Harness state={paused()} />);
    const options = [...select().options];
    expect(options.map((o) => o.textContent)).toEqual([
      "OpenRouter (asks for zero data retention)",
      "Google Gemini (paused)",
    ]);
    expect(options[1].disabled).toBe(true);
    expect(select().value).toBe("gemini");
  });

  it("moves the role back to OpenRouter only when the person presses the button", async () => {
    render(<Harness state={paused()} />);
    expect(m.setRoleCloudProvider).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Use OpenRouter for Chat" }));
    await waitFor(() => expect(m.setRoleCloudProvider).toHaveBeenCalledWith("chat", "openrouter"));
  });

  it("says 'hasn't checked' when the key has no verdict, not 'reads as free'", () => {
    render(
      <Harness
        state={stateWith([geminiView({ tier: null, blocked: { ...UK, basis: "unknown" } })], {
          chat: "gemini",
        })}
      />,
    );
    expect(
      screen.getByText(
        /PM hasn't checked your key, so it treats it as free and PM thinks you're in/,
      ),
    ).toBeTruthy();
  });
});

describe("a role on a Gemini key with an empty prepay balance", () => {
  const empty = () =>
    stateWith(
      [
        geminiView({
          key_check: { at: "x", outcome: "no_credit", listed: [], listed_total: 49, tier: "paid" },
        }),
      ],
      { chat: "gemini" },
    );

  it("keeps the role, says so, and offers the way out", () => {
    render(<Harness state={empty()} />);
    expect(
      screen.getByText(
        "Chat is set to Google Gemini, but its prepay balance is empty. Chat won't answer until you add credit in AI Studio, or you pick another provider.",
      ),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Use OpenRouter for Chat" })).toBeTruthy();
    expect([...select().options].map((o) => o.textContent)).toEqual([
      "OpenRouter (asks for zero data retention)",
      "Google Gemini (out of credit)",
    ]);
  });

  it("is not offered to a role that isn't on it", () => {
    const state = stateWith(
      [
        geminiView({
          key_check: { at: "x", outcome: "no_credit", listed: [], listed_total: 49, tier: "paid" },
        }),
      ],
      {},
    );
    render(<Harness state={state} />);
    // Nothing eligible and nothing stored on a direct provider: today's editor.
    expect(screen.queryByLabelText("Chat provider")).toBeNull();
  });
});

describe("the picker rows", () => {
  it("flags a paid-only model against a key that reads free, and keeps it selectable", async () => {
    const state = stateWith([geminiView({ tier: "free", key_check: FREE })], { chat: "gemini" });
    render(<Harness state={state} />);
    fireEvent.click(screen.getByRole("button", { name: "Choose a model for chat" }));
    const pro = (await screen.findByText("Gemini 3.1 Pro (preview)")).closest("button")!;
    expect(within(pro).getByText("Needs a paid key")).toBeTruthy();
    expect(pro.hasAttribute("disabled")).toBe(false);
    // 3.8 Flash has a free allowance, so it is never flagged.
    const flash = screen
      .getByText("gemini/gemini-3.8-flash", { selector: "div" })
      .closest("button")!;
    expect(within(flash).queryByText("Needs a paid key")).toBeNull();
    // PM's own list is the picker's source: OpenRouter's model list is never asked for.
    expect(m.listModels).not.toHaveBeenCalled();
  });
});
