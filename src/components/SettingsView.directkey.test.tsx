// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The first-run wizard's "Your own key (optional)" block. What matters: "Get started" opens for a
// person with only a direct key ONLY once both roles resolve to a provider that has one (else
// onboarding would end with an app that has nothing to answer with); a good direct key points the
// roles at itself only when there is no OpenRouter key to land on; and saving the wizard never
// writes over a role that is on a provider of its own.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { directState, openaiView } from "../lib/directProvidersFixture";
import type { DirectProvidersState } from "../lib/types";

// Hoisted with the mock, which runs `rec` while building the module.
const { calls, world, rec } = vi.hoisted(() => {
  const calls: string[] = [];
  const rec =
    (name: string) =>
    (...args: unknown[]) => {
      calls.push(`${name}(${args.map((a) => JSON.stringify(a)).join(",")})`);
      return Promise.resolve();
    };
  return { calls, world: { state: null as unknown }, rec };
});

vi.mock("../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  getLocalAiSwitch: () =>
    Promise.resolve({
      on: false,
      chat_needs_model: false,
      background_needs_model: false,
      server_saved: false,
    }),
  hasOpenRouterKey: () => Promise.resolve(false),
  getSettings: () =>
    Promise.resolve({
      chat_models: ["anthropic/claude-sonnet-4.6"],
      background_models: ["google/gemini-2.5-flash"],
      chat_auto_switch: false,
      background_auto_switch: false,
      chat_provider: "openrouter",
      background_provider: "openrouter",
      chat_cloud_model: null,
      background_cloud_model: null,
      time_zone: "UTC",
      indexing_speed: "fast",
    }),
  languageOptions: () => Promise.reject(new Error("not in this test")),
  vaultStatus: () => Promise.resolve({ mode: "device" }),
  directProvidersState: () => Promise.resolve(world.state),
  setDirectOptIn: (...a: unknown[]) => {
    calls.push(`setDirectOptIn(${a.map((x) => JSON.stringify(x)).join(",")})`);
    return Promise.resolve([]);
  },
  saveDirectKey: (...a: unknown[]) => {
    calls.push(`saveDirectKey(${a.map((x) => JSON.stringify(x)).join(",")})`);
    // The key is kept: the next read has it.
    world.state = directState({ view: openaiView() });
    return Promise.resolve({
      outcome: "ok",
      saved: true,
      listed: ["openai/gpt-6.2-sol", "openai/gpt-6-luna"],
      listed_total: 40,
    });
  },
  setRoleCloudProvider: (role: string, provider: string) => {
    calls.push(`setRoleCloudProvider("${role}","${provider}")`);
    const cur = world.state as DirectProvidersState;
    world.state = directState({
      view: cur.providers[0],
      chat:
        role === "chat"
          ? (provider as "openai")
          : cur.roles.chat.stored === "openai"
            ? "openai"
            : "openrouter",
      background:
        role === "background"
          ? (provider as "openai")
          : cur.roles.background.stored === "openai"
            ? "openai"
            : "openrouter",
    });
    return Promise.resolve();
  },
  setDirectRole: rec("setDirectRole"),
  setTimeZone: rec("setTimeZone"),
  setChatModels: rec("setChatModels"),
  setBackgroundModels: rec("setBackgroundModels"),
  setChatAutoSwitch: rec("setChatAutoSwitch"),
  setBackgroundAutoSwitch: rec("setBackgroundAutoSwitch"),
  setOpenRouterKey: rec("setOpenRouterKey"),
  setOnboardingDone: rec("setOnboardingDone"),
  setIndexingSpeed: rec("setIndexingSpeed"),
  setVaultEmbedder: rec("setVaultEmbedder"),
}));

vi.mock("./localai/LocalAiSettings", () => ({ LocalAiSettings: () => <p>Local AI tab</p> }));
vi.mock("./settings/AiModelsSettings", () => ({ AiModelsSettings: () => <p>AI & Models tab</p> }));
vi.mock("./settings/GeneralSettings", () => ({ GeneralSettings: () => <p>General tab</p> }));
vi.mock("./onboarding/OnboardingLocalConnect", () => ({
  OnboardingLocalConnect: () => <p>Connect your local server</p>,
}));
vi.mock("./ModelListEditor", () => ({ ModelListEditor: () => null }));
vi.mock("./SignInStrip", () => ({ SignInStrip: () => null }));
vi.mock("../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { SettingsView } from "./SettingsView";

beforeEach(() => {
  if (!("ResizeObserver" in globalThis)) {
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
  if (!Element.prototype.scrollTo) Element.prototype.scrollTo = () => {};
  calls.length = 0;
});
afterEach(cleanup);

const getStarted = () => screen.getByRole("button", { name: "Get started" }) as HTMLButtonElement;
const keyField = () => screen.getByLabelText("OpenAI API key") as HTMLInputElement;

async function mount(state: DirectProvidersState, onClose = vi.fn()) {
  world.state = state;
  render(<SettingsView onClose={onClose} onboarding />);
  await screen.findByText("Your own key (optional)");
  return onClose;
}

/** The wizard's switch for OpenAI, and the key flow under it. */
async function typeAndSaveKey() {
  fireEvent.change(keyField(), { target: { value: "sk-test-key-not-real-0123456789" } });
  fireEvent.submit(keyField().closest("form")!);
}

describe("the cloud pane's words", () => {
  it("calls the segment Cloud and says OpenRouter is the default, and what it asks for", async () => {
    await mount(
      directState({ view: openaiView({ opted_in: false, key: "absent", key_check: null }) }),
    );
    expect(screen.getByRole("button", { name: "Cloud" })).toBeTruthy();
    expect(
      screen.getByText(/PM asks for zero data retention on every request\./, { exact: false }),
    ).toBeTruthy();
    expect(
      screen.getByText(
        "Rather not use OpenRouter? Use your own OpenAI key. It's off unless you turn it on, and it isn't zero data retention.",
      ),
    ).toBeTruthy();
  });

  it("starts the own-key card closed: no key field and no warning until the switch is turned on", async () => {
    await mount(
      directState({ view: openaiView({ opted_in: false, key: "absent", key_check: null }) }),
    );
    expect(screen.queryByLabelText("OpenAI API key")).toBeNull();
    expect(screen.queryByText("This isn't zero data retention.")).toBeNull();
  });
});

describe("Get started", () => {
  it("stays shut with no key of either kind", async () => {
    await mount(
      directState({ view: openaiView({ opted_in: false, key: "absent", key_check: null }) }),
    );
    expect(getStarted().disabled).toBe(true);
  });

  it("stays shut when a direct key is good but a role is still on OpenRouter with no key", async () => {
    // Chat on OpenAI, Background still on OpenRouter, no OpenRouter key: Background would have
    // nothing to answer it.
    await mount(directState({ chat: "openai" }));
    expect(getStarted().disabled).toBe(true);
  });

  it("opens with only a direct key once both roles are on it", async () => {
    const onClose = await mount(directState({ chat: "openai", background: "openai" }));
    expect(getStarted().disabled).toBe(false);
    fireEvent.click(getStarted());
    await waitFor(() => expect(onClose).toHaveBeenCalled());
  });

  it("doesn't write over a role that is on a provider of its own when it saves", async () => {
    await mount(directState({ chat: "openai", background: "openai" }));
    fireEvent.click(getStarted());
    await waitFor(() => expect(calls).toContain("setOnboardingDone()"));
    // OpenRouter's lists are saved as before, and nothing here re-points a role or its direct model.
    expect(calls.some((c) => c.startsWith("setChatModels("))).toBe(true);
    expect(calls.some((c) => c.startsWith("setDirectRole("))).toBe(false);
    expect(calls.some((c) => c.startsWith("setRoleCloudProvider("))).toBe(false);
    expect(calls.some((c) => c.startsWith("setOpenRouterKey("))).toBe(false);
  });

  it("is still open for a person with an OpenRouter key and no own key, as before", async () => {
    await mount(
      directState({ view: openaiView({ opted_in: false, key: "absent", key_check: null }) }),
    );
    fireEvent.change(screen.getByPlaceholderText("sk-or-..."), { target: { value: "sk-or-x" } });
    expect(getStarted().disabled).toBe(false);
  });
});

describe("a good own key", () => {
  it("points both roles at it when there is no OpenRouter key, and names the models", async () => {
    await mount(directState({ view: openaiView({ key: "absent", key_check: null }) }));
    await typeAndSaveKey();
    await waitFor(() => expect(getStarted().disabled).toBe(false));
    expect(calls).toContain('saveDirectKey("openai","sk-test-key-not-real-0123456789")');
    expect(calls).toContain('setRoleCloudProvider("chat","openai")');
    expect(calls).toContain('setRoleCloudProvider("background","openai")');
    expect(await screen.findByText("✓ Saved to your keychain.")).toBeTruthy();
    expect(
      screen.getByText(
        "PM will use GPT-6.2 Sol for chat and GPT-6 Luna for background work. You can change that later in Settings > AI & Models.",
      ),
    ).toBeTruthy();
  });

  it("leaves both roles on OpenRouter when an OpenRouter key is typed, and lets them be moved", async () => {
    await mount(directState({ view: openaiView({ key: "absent", key_check: null }) }));
    fireEvent.change(screen.getByPlaceholderText("sk-or-..."), { target: { value: "sk-or-x" } });
    await typeAndSaveKey();
    await screen.findByText("✓ Saved to your keychain.");
    expect(calls.some((c) => c.startsWith("setRoleCloudProvider("))).toBe(false);
    // Two small selects, in their own words, to change that.
    const chat = screen.getByLabelText("Chat uses") as HTMLSelectElement;
    expect(chat.value).toBe("openrouter");
    expect([...chat.options].map((o) => o.textContent)).toEqual([
      "OpenRouter (asks for zero data retention)",
      "OpenAI, direct (no zero data retention)",
    ]);
    await act(async () => {
      fireEvent.change(chat, { target: { value: "openai" } });
    });
    await waitFor(() => expect(calls).toContain('setRoleCloudProvider("chat","openai")'));
    expect((screen.getByLabelText("Chat uses") as HTMLSelectElement).value).toBe("openai");
  });
});
