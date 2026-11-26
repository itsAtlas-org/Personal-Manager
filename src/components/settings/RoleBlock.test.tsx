// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// One role's half of AI & Models. The things it must get right: a person who never turned a key on
// sees exactly today's editor; a role that can't be answered says why in words; a direct role has
// one model, no fallbacks and an effort in that model's own levels; and the provider choice lives
// here and nowhere else.

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
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

import { directState, LUNA, openaiView, SOL } from "../../lib/directProvidersFixture";
import type { DirectProvidersState, RoleName } from "../../lib/types";
import { RoleBlock } from "./RoleBlock";

const OR_MODEL = "anthropic/claude-sonnet-4.6";
const onError = vi.fn();
const orChange = vi.fn();
/** What the next `reload` hands back, the way the hook re-reads after a write. */
const next: { current: DirectProvidersState | null } = { current: null };

function Harness({
  initial,
  role = "chat",
  routing = "cloud",
}: {
  initial: DirectProvidersState | null;
  role?: RoleName;
  routing?: string;
}) {
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
  return (
    <RoleBlock
      role={role}
      label={role === "chat" ? "Chat model" : "Background model"}
      description="OpenRouter's own words."
      helpId={role === "chat" ? "settings-chat-models" : "settings-background-models"}
      dp={dp}
      routing={routing}
      openRouter={{
        models: [OR_MODEL],
        onChange: orChange,
        autoSwitch: true,
        onAutoSwitchChange: () => {},
      }}
      onError={onError}
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

const providerSelect = (who = "Chat") =>
  screen.getByLabelText(`${who} provider`) as HTMLSelectElement;

describe("with nothing to choose between", () => {
  it("is today's editor and nothing else: no provider Select, OpenRouter's list and auto-switch", () => {
    // Not opted in, no key: the state every install starts in.
    const state = directState({
      view: openaiView({ opted_in: false, key: "absent", key_check: null }),
    });
    render(<Harness initial={state} />);
    expect(screen.queryByLabelText("Chat provider")).toBeNull();
    expect(screen.getByText(OR_MODEL)).toBeTruthy();
    expect(screen.getByRole("switch", { name: "Auto-switch on limit" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Add a model for chat" })).toBeTruthy();
    expect(screen.queryByLabelText("Chat effort")).toBeNull();
    // Today's DOM: no live regions for a person with nothing to choose.
    expect(screen.queryAllByRole("status")).toHaveLength(0);
  });

  it("is the same before the backend has answered", () => {
    render(<Harness initial={null} />);
    expect(screen.queryByLabelText("Chat provider")).toBeNull();
    expect(screen.getByText(OR_MODEL)).toBeTruthy();
  });
});

describe("choosing the provider", () => {
  it("offers OpenRouter and each provider that is on and keyed, each with what it is", () => {
    render(<Harness initial={directState()} />);
    const options = [...providerSelect().options].map((o) => o.textContent);
    expect(options).toEqual([
      "OpenRouter (asks for zero data retention)",
      "OpenAI, direct (no zero data retention)",
    ]);
    expect(providerSelect().value).toBe("openrouter");
  });

  it("moves the role through the command, then reads again, and says it aloud", async () => {
    next.current = directState({ chat: "openai" });
    render(<Harness initial={directState()} />);
    fireEvent.change(providerSelect(), { target: { value: "openai" } });
    await waitFor(() => expect(m.setRoleCloudProvider).toHaveBeenCalledWith("chat", "openai"));
    expect(await screen.findByText("Chat now uses OpenAI. Model: GPT-6.2 Sol")).toBeTruthy();
    // The same Select, now showing the new choice: it never unmounts.
    expect(providerSelect().value).toBe("openai");
  });

  it("shows the command's refusal, and re-reads so the Select tells the truth", async () => {
    m.setRoleCloudProvider.mockRejectedValue("OpenAI isn't turned on.");
    render(<Harness initial={directState()} />);
    fireEvent.change(providerSelect(), { target: { value: "openai" } });
    await waitFor(() => expect(onError).toHaveBeenCalledWith("OpenAI isn't turned on."));
  });

  it("swaps the list with the provider, never editing the other provider's", async () => {
    next.current = directState({ chat: "openrouter" });
    render(<Harness initial={directState({ chat: "openai" })} />);
    expect(screen.getByText("GPT-6.2 Sol")).toBeTruthy();
    expect(screen.queryByText(OR_MODEL)).toBeNull();
    fireEvent.change(providerSelect(), { target: { value: "openrouter" } });
    await waitFor(() => expect(screen.getByText(OR_MODEL)).toBeTruthy());
    expect(screen.queryByText("GPT-6.2 Sol")).toBeNull();
  });
});

describe("a role that can't be answered", () => {
  it("keeps a stored provider that has no key as a disabled option, and says what is missing", () => {
    const state = directState({
      view: openaiView({ key: "absent", key_check: null }),
      chat: "openai",
    });
    render(<Harness initial={state} />);
    const option = [...providerSelect().options].find((o) => o.value === "openai")!;
    expect(option.textContent).toBe("OpenAI (no key)");
    expect(option.disabled).toBe(true);
    expect(providerSelect().value).toBe("openai");
    expect(
      screen.getByText(
        "Your OpenAI key is missing on this device. Add it above, or move Chat back to OpenRouter.",
      ),
    ).toBeTruthy();
  });

  it("says the key is unreadable, not missing", () => {
    const state = directState({
      view: openaiView({ key: "unreadable", key_check: null }),
      background: "openai",
    });
    render(<Harness initial={state} role="background" />);
    expect(
      screen.getByText(
        "Background is set to OpenAI, but PM can't read your OpenAI key right now. Background won't answer until it can, or until you pick another provider.",
      ),
    ).toBeTruthy();
    expect(
      [...providerSelect("Background").options].some(
        (o) => o.textContent === "OpenAI (key unreadable)",
      ),
    ).toBe(true);
  });

  it("says a role on a key with no credit won't answer yet, as a status", () => {
    const state = directState({
      view: openaiView({
        key_check: { at: "x", outcome: "no_credit", listed: [], listed_total: 0 },
      }),
      chat: "openai",
    });
    render(<Harness initial={state} />);
    const line = screen.getByText(
      "Your OpenAI account is out of credit or at a spend limit, so this won't answer until you add credit or raise the limit.",
    );
    expect(line.closest('[role="status"]')).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
    // The provider stays chosen and selectable: the key is fine.
    expect(providerSelect().value).toBe("openai");
    expect([...providerSelect().options].find((o) => o.value === "openai")!.disabled).toBe(false);
  });

  it("says OpenAI is turned off, and a button puts the role back on OpenRouter", async () => {
    // Stored on OpenAI but demoted at read: the switch is off.
    const state = directState({
      view: openaiView({ opted_in: false }),
      chat: "openai",
      chatEffective: "openrouter",
    });
    next.current = directState({ view: openaiView({ opted_in: false }) });
    render(<Harness initial={state} />);
    expect(screen.getByText("OpenAI is turned off above.")).toBeTruthy();
    expect([...providerSelect().options].some((o) => o.textContent === "OpenAI (turned off)")).toBe(
      true,
    );
    fireEvent.click(screen.getByRole("button", { name: "Use OpenRouter for Chat" }));
    await waitFor(() => expect(m.setRoleCloudProvider).toHaveBeenCalledWith("chat", "openrouter"));
    await waitFor(() => expect(screen.queryByText("OpenAI is turned off above.")).toBeNull());
    expect(await screen.findByText("Chat moved back to OpenRouter")).toBeTruthy();
  });
});

describe("a role on a provider of the person's own", () => {
  it("holds one model, with no fallbacks, no order and no auto-switch, and never asks OpenRouter", () => {
    render(<Harness initial={directState({ chat: "openai" })} />);
    expect(screen.getByText("GPT-6.2 Sol")).toBeTruthy();
    expect(screen.queryByRole("switch", { name: "Auto-switch on limit" })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Move / })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Remove / })).toBeNull();
    expect(screen.queryByText(/^fallback/)).toBeNull();
    expect(screen.getByRole("button", { name: "Choose a model for chat" })).toBeTruthy();
    expect(m.listModels).not.toHaveBeenCalled();
  });

  it("replaces the model when another is picked, and takes no id typed by hand", async () => {
    render(<Harness initial={directState({ chat: "openai" })} />);
    fireEvent.click(screen.getByRole("button", { name: "Choose a model for chat" }));
    const search = await screen.findByPlaceholderText("Search models…");
    fireEvent.change(search, { target: { value: "openai/not-a-real-one" } });
    expect(screen.getByText("PM can only use models it knows the price of.")).toBeTruthy();
    expect(screen.queryByText(/Use custom model id/)).toBeNull();
    expect(m.listModels).not.toHaveBeenCalled();
    fireEvent.change(search, { target: { value: "" } });
    fireEvent.click(screen.getByText("GPT-6 Luna"));
    await waitFor(() =>
      expect(m.setDirectRole).toHaveBeenCalledWith("openai", "chat", [LUNA.id], null),
    );
  });

  it("is told what the role's routing does with the provider", () => {
    const state = directState({ chat: "openai" });
    const { unmount } = render(<Harness initial={state} routing="cloud" />);
    expect(
      screen.getByText("Chat goes straight to OpenAI with your key. It isn't zero data retention."),
    ).toBeTruthy();
    unmount();
    const second = render(<Harness initial={state} routing="local-then-cloud" />);
    expect(
      screen.getByText("OpenAI answers if your local model can't. That isn't zero data retention."),
    ).toBeTruthy();
    second.unmount();
    render(<Harness initial={state} routing="local" />);
    expect(
      screen.getByText(
        "This job runs on your local model. OpenAI is only used if you switch it to a cloud routing.",
      ),
    ).toBeTruthy();
  });

  it("tells Background, and only Background, that mods can't use the key", () => {
    const state = directState({ chat: "openai", background: "openai" });
    const bg = render(<Harness initial={state} role="background" />);
    expect(
      screen.getByText(
        "Mods can't use your own keys. While Background is on OpenAI, a mod that asks for AI gets no answer.",
      ),
    ).toBeTruthy();
    bg.unmount();
    render(<Harness initial={state} role="chat" />);
    expect(screen.queryByText(/Mods can't use your own keys/)).toBeNull();
  });
});

describe("effort", () => {
  it("offers Model default and the model's own levels for chat", () => {
    render(<Harness initial={directState({ chat: "openai" })} />);
    const select = screen.getByLabelText("Chat effort") as HTMLSelectElement;
    expect([...select.options].map((o) => o.textContent)).toEqual([
      "Model default",
      "Low",
      "Medium",
      "High",
      "Extra high",
      "Max",
    ]);
    expect(select.value).toBe("");
  });

  it("offers Background no model default, and marks its own", () => {
    render(<Harness initial={directState({ background: "openai" })} role="background" />);
    const select = screen.getByLabelText("Background effort") as HTMLSelectElement;
    expect([...select.options].map((o) => o.textContent)).toEqual([
      "Off",
      "Low (PM's default)",
      "Medium",
      "High",
    ]);
    expect(select.value).toBe("low");
  });

  it("writes a chosen level with the model, and the role default as null", async () => {
    render(<Harness initial={directState({ chat: "openai" })} />);
    fireEvent.change(screen.getByLabelText("Chat effort"), { target: { value: "high" } });
    await waitFor(() =>
      expect(m.setDirectRole).toHaveBeenCalledWith("openai", "chat", [SOL.id], "high"),
    );
    cleanup();
    render(<Harness initial={directState({ background: "openai" })} role="background" />);
    fireEvent.change(screen.getByLabelText("Background effort"), { target: { value: "medium" } });
    await waitFor(() =>
      expect(m.setDirectRole).toHaveBeenCalledWith("openai", "background", [LUNA.id], "medium"),
    );
    // Picking the default writes null, so the role keeps tracking it.
    fireEvent.change(screen.getByLabelText("Background effort"), { target: { value: "low" } });
    await waitFor(() =>
      expect(m.setDirectRole).toHaveBeenLastCalledWith("openai", "background", [LUNA.id], null),
    );
  });

  it("goes back to the default, and says so, when a new model doesn't have the chosen level", async () => {
    const view = openaiView();
    view.roles.chat = { uses_this_provider: true, model: SOL.id, effort: "xhigh" };
    render(<Harness initial={directState({ view, chat: "openai" })} />);
    // The live region is there before anything changes, so the later text is read out.
    expect(screen.getAllByRole("status").length).toBeGreaterThan(0);
    fireEvent.click(screen.getByRole("button", { name: "Choose a model for chat" }));
    fireEvent.click(await screen.findByText("GPT-6 Luna"));
    // Luna has no Extra high: the effort is written as the default in the SAME call, so the wire
    // never sees a level the model refuses.
    await waitFor(() =>
      expect(m.setDirectRole).toHaveBeenCalledWith("openai", "chat", [LUNA.id], null),
    );
    const words = "Effort is now Model default. GPT-6 Luna doesn't have Extra high.";
    const heard = (await screen.findAllByText(words)).filter((n) => n.closest('[role="status"]'));
    expect(heard).toHaveLength(1);
  });

  it("keeps a chosen level the new model also has", async () => {
    const view = openaiView();
    view.roles.chat = { uses_this_provider: true, model: SOL.id, effort: "high" };
    render(<Harness initial={directState({ view, chat: "openai" })} />);
    fireEvent.click(screen.getByRole("button", { name: "Choose a model for chat" }));
    fireEvent.click(await screen.findByText("GPT-6 Luna"));
    await waitFor(() =>
      expect(m.setDirectRole).toHaveBeenCalledWith("openai", "chat", [LUNA.id], "high"),
    );
    expect(screen.queryByText(/Effort is now/)).toBeNull();
  });
});

describe("the editor on OpenRouter", () => {
  it("still hands every change to OpenRouter's own handler", () => {
    render(<Harness initial={null} />);
    fireEvent.click(screen.getByRole("button", { name: `Remove ${OR_MODEL}` }));
    expect(orChange).toHaveBeenCalledWith([]);
    expect(within(document.body).queryByLabelText("Chat effort")).toBeNull();
  });
});
