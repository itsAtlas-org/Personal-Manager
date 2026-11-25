// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The safety list's notices inside Settings. Settings is a Modal whose scrim covers App's column, so
// the strip App draws for a mod that was paused or removed can't be seen or used while it is open:
// SettingsView draws its own copy (the rule the sign-in strip follows).

import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { addNotice, forgetModSurface } from "../lib/modSurface";
import { safety } from "./mods/modFixtures";

const modsSafetyStatus = vi.fn();

vi.mock("../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  modsSafetyStatus: () => modsSafetyStatus(),
  onModsNotice: () => Promise.resolve(() => {}),
  onModsChanged: () => Promise.resolve(() => {}),
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
      chat_models: [],
      background_models: [],
      chat_auto_switch: false,
      background_auto_switch: false,
      time_zone: "UTC",
      indexing_speed: "fast",
    }),
  languageOptions: () => Promise.reject(new Error("not in this test")),
  vaultStatus: () => Promise.resolve({ mode: "device" }),
}));
vi.mock("./settings/AiModelsSettings", () => ({ AiModelsSettings: () => <p>AI tab</p> }));
vi.mock("./settings/GeneralSettings", () => ({ GeneralSettings: () => <p>General tab</p> }));
vi.mock("./ModelListEditor", () => ({ ModelListEditor: () => null }));
vi.mock("./SignInStrip", () => ({ SignInStrip: () => null }));
vi.mock("../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { SettingsView } from "./SettingsView";

beforeEach(() => {
  vi.clearAllMocks();
  if (!("ResizeObserver" in globalThis)) {
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
  if (!Element.prototype.scrollTo) Element.prototype.scrollTo = () => {};
});
afterEach(cleanup);

describe("the safety list's notices in Settings", () => {
  it("draws a removed mod's strip inside the Settings dialog, where its scrim hides App's", async () => {
    modsSafetyStatus.mockResolvedValue(
      safety({
        notices: [
          { kind: "removed", id: "a.b", name: "Acme", version: "1", reason: "Steals notes." },
        ],
      }),
    );
    render(<SettingsView onClose={() => {}} onboarding={false} />);
    const strip = await screen.findByText(/“Acme” was removed by PM's safety list/);
    expect(screen.getByRole("dialog").contains(strip)).toBe(true);
  });

  it("draws nothing when the list has nothing to say", async () => {
    modsSafetyStatus.mockResolvedValue(safety());
    render(<SettingsView onClose={() => {}} onboarding={false} />);
    await screen.findByText("General tab");
    expect(screen.queryByText(/safety list/)).toBeNull();
  });
});

describe("a mod's notice in Settings", () => {
  it("is drawn inside the Settings dialog too, where its scrim hides App's copy", async () => {
    modsSafetyStatus.mockResolvedValue(safety());
    render(<SettingsView onClose={() => {}} onboarding={false} />);
    await screen.findByText("General tab");
    act(() =>
      addNotice(
        { modId: "a.b", name: "Acme", tier: "third" },
        { text: "Rain at four.", tone: "info" },
      ),
    );
    const strip = screen.getByText("Rain at four.");
    expect(screen.getByRole("dialog").contains(strip)).toBe(true);
    expect(screen.getByRole("dialog").textContent).toContain("“Acme”");
    forgetModSurface();
  });
});
