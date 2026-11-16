// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The pop-out Calendar's own gate (#1039): it shows the Calendar only past the same locks the main
// window has, hides it when another profile takes the vault, and sends its links to the main window.

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const ipc = vi.hoisted(() => {
  const handlers: Record<string, () => void> = {};
  return {
    handlers,
    appLockStatus: vi.fn(),
    vaultStatus: vi.fn(),
    vaultLockStatus: vi.fn(),
    showInMainWindow: vi.fn(() => Promise.resolve()),
    checkGoogleCalendars: vi.fn(() => Promise.resolve({})),
    onVaultCurtain: vi.fn(async (h: () => void) => {
      handlers.curtain = h;
      return () => {};
    }),
    onVaultAcquired: vi.fn(async (h: () => void) => {
      handlers.acquired = h;
      return () => {};
    }),
  };
});
vi.mock("./lib/ipc", () => ipc);

const stored = vi.hoisted(() => ({
  hydrateStoredPrefs: vi.fn(() => Promise.resolve()),
  followStoredPrefs: vi.fn(() => () => {}),
}));
vi.mock("./lib/storedPrefs", () => stored);
vi.mock("./lib/useExternalLinks", () => ({ useExternalLinks: () => {} }));
vi.mock("./lib/scrollAxis", () => ({ installAxisScrollNormalizer: () => () => {} }));

// The Calendar itself is tested on its own; here it only has to show, and to hand over its links.
vi.mock("./components/calendar/CalendarView", () => ({
  CalendarView: (p: {
    onOpenProject: (p: string) => void;
    onOpenPinboard: () => void;
    rosterInHeader?: boolean;
  }) => (
    <div>
      <p>the calendar</p>
      <p>{p.rosterInHeader ? "list in the header" : "no list in the header"}</p>
      <button onClick={() => p.onOpenProject("Atlas")}>project</button>
      <button onClick={() => p.onOpenPinboard()}>pinboard</button>
    </div>
  ),
}));

vi.mock("./theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: "standard" }),
}));

import CalendarWindow from "./CalendarWindow";

const OPEN = { locked: false, enabled: false, available: false };
const VAULT = { needs_unlock: false, fault: null };

beforeEach(() => {
  ipc.appLockStatus.mockResolvedValue(OPEN);
  ipc.vaultStatus.mockResolvedValue(VAULT);
  ipc.vaultLockStatus.mockResolvedValue({ active: true });
  stored.hydrateStoredPrefs.mockClear();
  ipc.checkGoogleCalendars.mockClear();
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

async function mount() {
  render(<CalendarWindow />);
  await act(async () => {});
}

describe("CalendarWindow", () => {
  it("shows the Calendar past the locks, after reading the stored preferences", async () => {
    await mount();
    expect(screen.getByText("the calendar")).toBeTruthy();
    expect(stored.hydrateStoredPrefs).toHaveBeenCalledOnce();
    expect(stored.followStoredPrefs).toHaveBeenCalled();
  });

  it("sends a project or the pinboard to the main window", async () => {
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "project" }));
    expect(ipc.showInMainWindow).toHaveBeenCalledWith({ view: "project", project: "Atlas" });
    fireEvent.click(screen.getByRole("button", { name: "pinboard" }));
    expect(ipc.showInMainWindow).toHaveBeenCalledWith({ view: "pinboard" });
  });

  it("shows nothing of the calendar while PM is locked, or its vault waits for a passphrase", async () => {
    ipc.appLockStatus.mockResolvedValue({ ...OPEN, locked: true });
    await mount();
    expect(screen.queryByText("the calendar")).toBeNull();
    expect(screen.getByText("PM is locked, so the calendar is hidden.")).toBeTruthy();
    cleanup();

    ipc.appLockStatus.mockResolvedValue(OPEN);
    ipc.vaultStatus.mockResolvedValue({ ...VAULT, needs_unlock: true });
    await mount();
    expect(screen.queryByText("the calendar")).toBeNull();
    // Unlocked in the main window: Try again brings it.
    ipc.vaultStatus.mockResolvedValue(VAULT);
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    await act(async () => {});
    expect(screen.getByText("the calendar")).toBeTruthy();
  });

  it("holds the calendar list in the header, since it has no sidebar", async () => {
    await mount();
    expect(screen.getByText("list in the header")).toBeTruthy();
  });

  it("tells a locked PM's user where the way back is", async () => {
    ipc.appLockStatus.mockResolvedValue({ ...OPEN, locked: true });
    await mount();
    // The Open PM button itself is in the title bar (TitleBar.test); the hint points at it.
    expect(screen.getByText(/Unlock PM in its main window \(Open PM,/)).toBeTruthy();
  });

  it("checks Google for changes while the pop-out is open, and not behind the lock gate", async () => {
    // Only the timers: the gate's own promises must still resolve.
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    ipc.appLockStatus.mockResolvedValue({ ...OPEN, locked: true });
    await mount();
    await act(async () => void vi.advanceTimersByTime(120_000));
    expect(ipc.checkGoogleCalendars).not.toHaveBeenCalled();
    cleanup();

    ipc.appLockStatus.mockResolvedValue(OPEN);
    await mount();
    expect(ipc.checkGoogleCalendars).not.toHaveBeenCalled();
    // Within a quarter either way of 30 s (calendarPoll): by 40 s it has asked at least once.
    await act(async () => void vi.advanceTimersByTime(40_000));
    expect(ipc.checkGoogleCalendars.mock.calls.length).toBeGreaterThanOrEqual(1);
  });

  it("treats a store it can't read as locked", async () => {
    ipc.appLockStatus.mockRejectedValue(new Error("the vault is locked"));
    await mount();
    expect(screen.queryByText("the calendar")).toBeNull();
  });

  it("hides the calendar when another profile takes the vault, and brings it back after", async () => {
    await mount();
    act(() => ipc.handlers.curtain());
    expect(screen.queryByText("the calendar")).toBeNull();
    expect(screen.getByText(/Another account on this computer is using your vault/)).toBeTruthy();
    act(() => ipc.handlers.acquired());
    await act(async () => {});
    expect(screen.getByText("the calendar")).toBeTruthy();
  });
});
