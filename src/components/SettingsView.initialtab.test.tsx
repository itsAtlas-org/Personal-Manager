// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Opening Settings on a given page. A mod paused because developer mode is off offers "Open
// Settings", and the switch is on the Developer page (Bobby, 2026-11-17), so the Settings modal takes
// the page it opens on. With none given it opens on General, as it always did; a page that isn't on
// the rail falls back like any other vanished tab.

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import app from "../App.tsx?raw";

vi.mock("../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  modsSafetyStatus: async () => (await import("./mods/modFixtures")).safety(),
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
vi.mock("./settings/DeveloperSettings", () => ({
  DeveloperSettings: () => <p>Developer tab</p>,
}));
vi.mock("./settings/StorageSettings", () => ({ StorageSettings: () => <p>Storage tab</p> }));
vi.mock("./ModelListEditor", () => ({ ModelListEditor: () => null }));
vi.mock("./SignInStrip", () => ({ SignInStrip: () => null }));
vi.mock("../theme/ThemeContext", async (importOriginal) => ({
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

const railTab = (name: string) => screen.getByRole("button", { name });

describe("the page Settings opens on", () => {
  it("is General when none is asked for", async () => {
    render(<SettingsView onClose={() => {}} onboarding={false} />);
    expect(await screen.findByText("General tab")).toBeTruthy();
    expect(screen.queryByText("Developer tab")).toBeNull();
  });

  it("is the Developer page when that is asked for, with its rail entry the current one", async () => {
    render(<SettingsView onClose={() => {}} onboarding={false} initialTab="developer" />);
    expect(await screen.findByText("Developer tab")).toBeTruthy();
    expect(screen.queryByText("General tab")).toBeNull();
    expect(railTab("Developer").getAttribute("aria-current")).toBe("page");
    expect(railTab("General").getAttribute("aria-current")).toBeNull();
  });

  it("can still be moved to another page from there", async () => {
    render(<SettingsView onClose={() => {}} onboarding={false} initialTab="developer" />);
    await screen.findByText("Developer tab");
    railTab("General").click();
    expect(await screen.findByText("General tab")).toBeTruthy();
    expect(screen.queryByText("Developer tab")).toBeNull();
  });

  it("falls back to the first page left in its group when the page asked for isn't on the rail", async () => {
    // Local AI is off here, so its page isn't on the rail: AI & Models stands in.
    render(<SettingsView onClose={() => {}} onboarding={false} initialTab="localai" />);
    expect(await screen.findByText("AI tab")).toBeTruthy();
  });
});

describe("App's way in", () => {
  it("opens Settings through one function that names the page, and gives the modal that page", () => {
    // App can't be rendered in a test, so its wiring is read from the source: the one opener sets
    // the page with the open, a mod tab's link passes its page through, and the modal gets it.
    expect(app).toMatch(
      /const openSettings = \(tab: SettingsTab = "general"\) => \{\s*setSettingsTab\(tab\);\s*setShowSettings\(true\);\s*\}/,
    );
    expect(app).toMatch(/onOpenSettings=\{\(tab\) => openSettings\(tab\)\}/);
    expect(app).toMatch(/<SettingsView\s+onboarding=\{false\}\s+initialTab=\{settingsTab\}/);
  });

  it("never opens Settings any other way, so a deep link can't stick for the next plain open", () => {
    // The only `setShowSettings(true)` is inside the opener; every other way in goes through it.
    expect(app.match(/setShowSettings\(true\)/g)).toHaveLength(1);
    expect(app.match(/openSettings\(\)/g)?.length).toBeGreaterThanOrEqual(3);
  });
});
