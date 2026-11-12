// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The Week/Day gutter's top-left corner: one zone-row button whose visible text is a code over each
// hour column (the city on hover and in the accessible name), the one menu that adds and removes
// extra zones, "+ New" in the first row, and the gutter's width as the body gutter's own string.

import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { gutterWidth, MAX_EXTRA_ZONES } from "../../lib/calendarPrefs";
import { ZoneGutter } from "./ZoneGutter";

// This computer is in London, whatever the box running the test is.
vi.mock("../../theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  deviceTimeZone: () => "Europe/London",
}));

// The Button reads the Depth and the System from the theme.
vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: "standard" }),
}));

afterEach(cleanup);

// A winter day, so London reads GMT and Kolkata GMT+5:30.
const AT = new Date(2026, 0, 15, 12);

function gutter(zones: string[], over: Partial<Parameters<typeof ZoneGutter>[0]> = {}) {
  const onChange = vi.fn();
  const view = render(<ZoneGutter zones={zones} onChange={onChange} at={AT} {...over} />);
  return { onChange, ...view };
}

describe("ZoneGutter", () => {
  it("is one button, named by the visible codes first and then the cities", () => {
    gutter(["Asia/Kolkata"]);
    const buttons = screen.getAllByRole("button");
    expect(buttons).toHaveLength(1);
    const name = buttons[0].textContent ?? "";
    // Visible text leads (WCAG 2.5.3); the cities follow, for a screen reader.
    expect(name).toMatch(/^GMT\+5:30\s+Kolkata,\s*GMT\s+London,\s*change time zones$/);
    expect(
      screen.getByRole("button", { name: /^GMT\+5:30 Kolkata, GMT London, change time zones/ }),
    ).toBe(buttons[0]);
    // The city is on hover.
    expect(buttons[0].getAttribute("title")).toBe(
      "Kolkata GMT+5:30 · London GMT. Add or remove time zones",
    );
    expect(buttons[0].getAttribute("aria-expanded")).toBe("false");
  });

  it("puts nothing else in the local column: no second button where the + used to sit", () => {
    const { container } = gutter(["Asia/Kolkata"]);
    expect(container.querySelectorAll("button")).toHaveLength(1);
    // Two cells, one per hour column, in the body gutter's own widths.
    const cells = container.querySelectorAll<HTMLElement>("button > span[style]");
    expect([...cells].map((c) => c.style.width)).toEqual(["3.25rem", "3.75rem"]);
  });

  it("with no extra zones, offers '+ Zone' and no local code", () => {
    gutter([]);
    const button = screen.getByRole("button");
    expect(button.textContent).toBe("+ Zone");
    expect(button.getAttribute("title")).toBe("Add a time zone");
  });

  it("with the row switched off and no zones, draws no button at all but keeps its width", () => {
    const { container } = gutter([], { allowAdd: false });
    expect(screen.queryByRole("button")).toBeNull();
    expect((container.firstChild as HTMLElement).style.width).toBe(gutterWidth(0));
  });

  it("is as wide as the body gutter: the same string for the same zones", () => {
    const { container } = gutter(["Asia/Kolkata", "America/New_York"]);
    expect((container.firstChild as HTMLElement).style.width).toBe(gutterWidth(2));
  });

  it("draws the corner (+ New) in the first row, above the zone row", () => {
    gutter(["Asia/Kolkata"], { corner: <button type="button">+ New</button> });
    const [first, second] = screen.getAllByRole("button");
    expect(first.textContent).toBe("+ New");
    expect(second.getAttribute("aria-expanded")).toBe("false");
  });

  it("reads each code for the week shown: a letter code where the platform has one", () => {
    gutter(["America/New_York"], { at: new Date(2026, 6, 15, 12) });
    expect(screen.getByRole("button").textContent).toMatch(/^EDT\s+New York,\s*GMT\+1\s+London,/);
  });
});

describe("the zone menu", () => {
  const open = () => fireEvent.click(screen.getByRole("button"));

  it("opens on a click, lists the zones, and removes one", () => {
    const { onChange } = gutter(["Asia/Kolkata"]);
    open();
    expect(screen.getAllByRole("button")[0].getAttribute("aria-expanded")).toBe("true");
    const panel = screen.getByRole("group", { name: "Time zones" });
    expect(within(panel).getByText("Asia / India / Kolkata")).toBeTruthy();
    expect(within(panel).getByText(/this computer/)).toBeTruthy();
    // The computer's own zone is not removable.
    expect(within(panel).getAllByRole("button", { name: /^Remove/ })).toHaveLength(1);
    fireEvent.click(within(panel).getByRole("button", { name: "Remove Kolkata" }));
    expect(onChange).toHaveBeenCalledWith([]);
    // The menu closes with it, so focus returns to the zone row instead of falling off the page.
    expect(screen.queryByRole("group", { name: "Time zones" })).toBeNull();
  });

  it("does not offer this computer's zone in the search: it is already a column", () => {
    gutter(["Asia/Kolkata"]);
    open();
    fireEvent.change(screen.getByLabelText("Filter timezones"), { target: { value: "london" } });
    // The menu's own "this computer" row carries that title; no pickable button may.
    expect(screen.queryAllByTitle("Europe/London").filter((el) => el.closest("button"))).toEqual(
      [],
    );
    // The search itself works, so the absence above is the exclusion's doing.
    fireEvent.change(screen.getByLabelText("Filter timezones"), { target: { value: "tokyo" } });
    expect(screen.getByTitle("Asia/Tokyo")).toBeTruthy();
  });

  it("adds a zone picked from the search", () => {
    const { onChange } = gutter(["Asia/Kolkata"]);
    open();
    fireEvent.change(screen.getByLabelText("Filter timezones"), { target: { value: "tokyo" } });
    fireEvent.click(screen.getByTitle("Asia/Tokyo"));
    expect(onChange).toHaveBeenCalledWith(["Asia/Kolkata", "Asia/Tokyo"]);
  });

  it("at the cap, drops the search and says to remove one first", () => {
    gutter(["Asia/Kolkata", "America/New_York"].slice(0, MAX_EXTRA_ZONES));
    open();
    expect(screen.queryByLabelText("Filter timezones")).toBeNull();
    expect(
      screen.getByText(`Up to ${MAX_EXTRA_ZONES} extra time zones. Remove one to add another.`),
    ).toBeTruthy();
  });
});
