// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The switch for single-key shortcuts (WCAG 2.2 SC 2.1.4): on unless turned off, per device, and
// followed live by whatever advertises the keys.

import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  keyboardPrefsAreDefault,
  readCharacterShortcuts,
  resetKeyboardPrefs,
  useCharacterShortcuts,
  writeCharacterShortcuts,
} from "./keyboardPrefs";

const KEY = "pm:a11y:characterShortcuts";

beforeEach(() => localStorage.clear());
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("keyboardPrefs", () => {
  it("is on by default, and keyboardPrefsAreDefault says so", () => {
    expect(readCharacterShortcuts()).toBe(true);
    expect(keyboardPrefsAreDefault()).toBe(true);
  });

  it("reads a value it does not recognise as on", () => {
    for (const bad of ["", "off", "0", "FALSE", "null", "{", "yes"]) {
      localStorage.setItem(KEY, bad);
      expect(readCharacterShortcuts(), bad).toBe(true);
    }
  });

  it("is on when storage cannot be read", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(readCharacterShortcuts()).toBe(true);
  });

  it("does not throw when storage cannot be written", () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(() => writeCharacterShortcuts(false)).not.toThrow();
    expect(() => resetKeyboardPrefs()).not.toThrow();
  });

  it("writes off and on, and announces each change", () => {
    const changed = vi.fn();
    window.addEventListener("pm:settings-changed", changed);
    try {
      writeCharacterShortcuts(false);
      expect(localStorage.getItem(KEY)).toBe("false");
      expect(readCharacterShortcuts()).toBe(false);
      expect(keyboardPrefsAreDefault()).toBe(false);
      writeCharacterShortcuts(true);
      expect(readCharacterShortcuts()).toBe(true);
      expect(changed).toHaveBeenCalledTimes(2);
    } finally {
      window.removeEventListener("pm:settings-changed", changed);
    }
  });

  it("resets to on, and announces it", () => {
    writeCharacterShortcuts(false);
    const changed = vi.fn();
    window.addEventListener("pm:settings-changed", changed);
    try {
      resetKeyboardPrefs();
      expect(localStorage.getItem(KEY)).toBeNull();
      expect(readCharacterShortcuts()).toBe(true);
      expect(keyboardPrefsAreDefault()).toBe(true);
      expect(changed).toHaveBeenCalledTimes(1);
    } finally {
      window.removeEventListener("pm:settings-changed", changed);
    }
  });
});

describe("useCharacterShortcuts", () => {
  function Probe() {
    return <p data-testid="state">{useCharacterShortcuts() ? "on" : "off"}</p>;
  }
  const state = () => screen.getByTestId("state").textContent;

  it("follows a write in this window", () => {
    render(<Probe />);
    expect(state()).toBe("on");
    act(() => writeCharacterShortcuts(false));
    expect(state()).toBe("off");
    act(() => resetKeyboardPrefs());
    expect(state()).toBe("on");
  });

  it("follows the other window's storage event, for this key or a clear, and not another key", () => {
    render(<Probe />);
    localStorage.setItem(KEY, "false");
    act(() => void window.dispatchEvent(new StorageEvent("storage", { key: "pm.calendar.zones" })));
    expect(state()).toBe("on");
    act(() => void window.dispatchEvent(new StorageEvent("storage", { key: KEY })));
    expect(state()).toBe("off");
    localStorage.clear();
    act(() => void window.dispatchEvent(new StorageEvent("storage", { key: null })));
    expect(state()).toBe("on");
  });

  it("catches up when the window is focused or shown again", () => {
    render(<Probe />);
    localStorage.setItem(KEY, "false");
    act(() => void window.dispatchEvent(new Event("focus")));
    expect(state()).toBe("off");
    localStorage.removeItem(KEY);
    act(() => void document.dispatchEvent(new Event("visibilitychange")));
    expect(state()).toBe("on");
  });

  it("lets go of its listeners on unmount", () => {
    const { unmount } = render(<Probe />);
    const remove = vi.spyOn(window, "removeEventListener");
    unmount();
    expect(remove.mock.calls.map((c) => c[0])).toEqual(
      expect.arrayContaining(["pm:settings-changed", "storage", "focus"]),
    );
  });
});
