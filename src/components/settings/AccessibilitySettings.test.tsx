// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Keyboard section of Settings → Accessibility (WCAG 2.2 SC 2.1.4, Character Key Shortcuts): a
// switch for the Calendar's single-key shortcuts that writes the per-device preference, and the tab's
// own Reset, which brings it back on.

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const resetAccessibility = vi.fn();
const themeIsDefault = { value: true };
vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({
    system: "slate",
    mode: "dark",
    accent: "mono",
    depth: "standard",
    fontScale: "default",
    setFontScale: vi.fn(),
    density: "standard",
    setDensity: vi.fn(),
    reduceMotion: false,
    setReduceMotion: vi.fn(),
    legibleFont: false,
    setLegibleFont: vi.fn(),
    colorblind: false,
    setColorblind: vi.fn(),
    contrast: "aa",
    setContrast: vi.fn(),
    accessibilityIsDefault: themeIsDefault.value,
    resetAccessibility,
  }),
}));

import { AccessibilitySettings } from "./AccessibilitySettings";
import { SETTINGS_TABS } from "./registry";
import { HELP } from "../../lib/help";
import { readCharacterShortcuts, writeCharacterShortcuts } from "../../lib/keyboardPrefs";

beforeEach(() => {
  localStorage.clear();
  resetAccessibility.mockReset();
  themeIsDefault.value = true;
});
afterEach(cleanup);

const keys = () => screen.getByRole("switch", { name: "Single-key shortcuts" });
const resetButton = () => screen.getByRole("button", { name: "Reset to defaults" });

describe("Settings → Accessibility → Keyboard", () => {
  it("has a section registered for the search and the rail, and the div the registry points at", () => {
    const tab = SETTINGS_TABS.find((t) => t.id === "accessibility");
    expect(tab?.sections.find((s) => s.id === "sec-a11y-keys")?.label).toBe("Keyboard");
    const { container } = render(<AccessibilitySettings />);
    for (const s of tab!.sections) {
      expect(container.querySelector(`#${s.id}[data-settings-section]`), s.id).not.toBeNull();
    }
  });

  it("has help for its section", () => {
    expect(HELP["settings-a11y-keys"]?.title).toBe("Single-key shortcuts");
  });

  it("says which keys it covers, in the row", () => {
    render(<AccessibilitySettings />);
    const description = document.getElementById(keys().getAttribute("aria-describedby") ?? "");
    expect(description?.textContent).toContain("t, c, Shift+C, j, k, n and p");
    expect(description?.textContent).toContain("speech input");
    expect(description?.textContent).toContain("arrow keys and every button still work");
  });

  it("starts on, and the switch writes the preference both ways", () => {
    render(<AccessibilitySettings />);
    expect(keys().getAttribute("aria-checked")).toBe("true");
    fireEvent.click(keys());
    expect(readCharacterShortcuts()).toBe(false);
    expect(keys().getAttribute("aria-checked")).toBe("false");
    fireEvent.click(keys());
    expect(readCharacterShortcuts()).toBe(true);
    expect(keys().getAttribute("aria-checked")).toBe("true");
  });

  it("follows a change made elsewhere", () => {
    render(<AccessibilitySettings />);
    writeCharacterShortcuts(false);
    return waitFor(() => expect(keys().getAttribute("aria-checked")).toBe("false"));
  });

  it("offers a Reset link in the section only while the switch is off, and it turns it back on", () => {
    render(<AccessibilitySettings />);
    const section = () => document.getElementById("sec-a11y-keys")!;
    expect(within(section()).queryByRole("button", { name: /reset/i })).toBeNull();
    fireEvent.click(keys());
    fireEvent.click(within(section()).getByRole("button", { name: /reset/i }));
    expect(readCharacterShortcuts()).toBe(true);
  });

  it("counts the switch in the tab's Reset: all at default only while it is on", () => {
    render(<AccessibilitySettings />);
    expect((resetButton() as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(keys());
    expect((resetButton() as HTMLButtonElement).disabled).toBe(false);
  });

  it("the tab's Reset restores the switch and the theme axes, and its confirm names it", async () => {
    themeIsDefault.value = false;
    writeCharacterShortcuts(false);
    render(<AccessibilitySettings />);
    fireEvent.click(resetButton());
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).toContain("single-key shortcuts");
    fireEvent.click(within(dialog).getByRole("button", { name: "Reset" }));
    await waitFor(() => expect(readCharacterShortcuts()).toBe(true));
    expect(resetAccessibility).toHaveBeenCalledTimes(1);
  });

  it("Reset with only the switch off still resets, and leaves the theme's own reset to the theme", async () => {
    writeCharacterShortcuts(false);
    render(<AccessibilitySettings />);
    fireEvent.click(resetButton());
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Reset" }));
    await waitFor(() => expect(readCharacterShortcuts()).toBe(true));
  });
});
