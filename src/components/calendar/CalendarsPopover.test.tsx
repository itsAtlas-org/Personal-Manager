// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The pop-out's "Calendars 2/3" button: the same list the sidebar shows, in a popover that stays open
// while you tick and closes on Esc with focus back on the button.

import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { Calendar, CalendarAccount } from "../../lib/types";

vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: "standard" }),
}));

import { CalendarsPopover } from "./CalendarsPopover";

afterEach(cleanup);

// jsdom ships no ResizeObserver; the popover builds one to keep its panel placed.
beforeAll(() => {
  if (!("ResizeObserver" in globalThis)) {
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
});

const ACCOUNTS = [
  { id: "gcal:me@x.com", provider: "google", email: "me@x.com", label: "Me" },
] as CalendarAccount[];
const cal = (id: string, name: string, selected = true) =>
  ({ id, name, selected, source_id: "gcal:me@x.com" }) as Calendar;
const CALENDARS = [cal("work", "Work"), cal("home", "Home"), cal("off", "Not synced", false)];

function mount(hidden: string[] = ["home"], onToggle = vi.fn()) {
  render(
    <CalendarsPopover
      accounts={ACCOUNTS}
      calendars={CALENDARS}
      hidden={new Set(hidden)}
      onToggle={onToggle}
      colorOf={() => "var(--ink4)"}
    />,
  );
  return { onToggle, trigger: screen.getByRole("button", { name: "Calendars 1/2" }) };
}

describe("CalendarsPopover", () => {
  it("reads how many calendars show out of how many sync, closed until pressed", () => {
    const { trigger } = mount();
    expect(trigger.getAttribute("aria-expanded")).toBe("false");
    // No aria-haspopup: "true" announces a menu, and the panel is a group of checkboxes.
    expect(trigger.hasAttribute("aria-haspopup")).toBe(false);
    expect(screen.queryByRole("group", { name: "Calendars to show" })).toBeNull();
  });

  it("lists the Personal Manager rows and the account's calendars, leaving out one that isn't synced", () => {
    const { trigger } = mount();
    fireEvent.click(trigger);
    expect(trigger.getAttribute("aria-expanded")).toBe("true");
    const list = within(screen.getByRole("group", { name: "Calendars to show" }));
    for (const name of ["Milestones", "Tasks", "Pinboard", "Work", "Home"]) {
      expect(list.getByRole("checkbox", { name })).toBeTruthy();
    }
    expect(list.getByText("me@x.com")).toBeTruthy();
    expect(list.queryByRole("checkbox", { name: "Not synced" })).toBeNull();
    // Ticked = shown.
    expect((list.getByRole("checkbox", { name: "Work" }) as HTMLInputElement).checked).toBe(true);
    expect((list.getByRole("checkbox", { name: "Home" }) as HTMLInputElement).checked).toBe(false);
  });

  it("ticking calls onToggle with the id, and the list stays open for the next", () => {
    const { trigger, onToggle } = mount();
    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole("checkbox", { name: "Work" }));
    expect(onToggle).toHaveBeenCalledWith("work");
    fireEvent.click(screen.getByRole("checkbox", { name: "Home" }));
    expect(onToggle).toHaveBeenLastCalledWith("home");
    expect(screen.getByRole("group", { name: "Calendars to show" })).toBeTruthy();
  });

  it("Esc closes it and puts focus back on the button", () => {
    const { trigger } = mount();
    trigger.focus();
    fireEvent.click(trigger);
    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(screen.queryByRole("group", { name: "Calendars to show" })).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });
});
