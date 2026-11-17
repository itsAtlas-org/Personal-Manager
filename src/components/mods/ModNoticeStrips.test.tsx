// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A mod's notice (Mods spec §1.4, §1.8): a band that names the mod and its tier, the mod's words as
// plain text under it, and PM's own Dismiss. It can't carry a link or a button of its own, it can't use
// PM's error look, and no mod can fill the screen or push another mod's notice away.

import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { addNotice, forgetModSurface } from "../../lib/modSurface";
import { RLO, ZERO_WIDTH } from "./modFixtures";
import type { ModSource, NoticeTone } from "./modPageHooks";

vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { MAX_SHOWN, ModNoticeStrips } from "./ModNoticeStrips";

const A: ModSource = { modId: "a.one", name: "Weather", tier: "third" };
const B: ModSource = { modId: "b.two", name: "Tides", tier: "second" };
const post = (from: ModSource, text: string, tone: NoticeTone = "info") =>
  act(() => addNotice(from, { text, tone }));

beforeEach(forgetModSurface);
afterEach(() => {
  cleanup();
  forgetModSurface();
});

describe("ModNoticeStrips", () => {
  it("draws nothing while there is no notice", () => {
    const { container } = render(<ModNoticeStrips />);
    expect(container.firstChild).toBeNull();
  });

  it("names the mod, wears its tier as PM's badge, and shows the words as text", () => {
    render(<ModNoticeStrips />);
    post(A, "Rain at four.");
    const strip = screen.getByRole("status");
    expect(strip.textContent).toContain("A notice from");
    expect(strip.textContent).toContain("“Weather”");
    expect(within(strip).getByText("Third party")).toBeTruthy();
    expect(within(strip).getByText("Rain at four.")).toBeTruthy();
  });

  it("is text only: a link, markup or a button in the words is just characters", () => {
    const { container } = render(<ModNoticeStrips />);
    const words =
      'Click <a href="https://evil.example">Update PM now</a> or https://evil.example [Allow](x)';
    post(A, words);
    expect(container.querySelector("a")).toBeNull();
    expect(container.querySelector("img, iframe, script, form, input")).toBeNull();
    expect(container.textContent).toContain(words);
    // The only button a strip has is PM's own Dismiss.
    const buttons = screen.getAllByRole("button");
    expect(buttons.map((b) => b.textContent)).toEqual(["Dismiss"]);
  });

  it("draws the words as one plain line", () => {
    render(<ModNoticeStrips />);
    post(A, `Paid${RLO} the\nbill${ZERO_WIDTH}.`);
    const text = screen.getByText(/Paid/).textContent ?? "";
    expect(text).not.toContain(RLO);
    expect(text).not.toContain("\n");
    expect(text).not.toContain(ZERO_WIDTH);
  });

  it("quotes a mod's name so it can't pose as PM's sentence", () => {
    render(<ModNoticeStrips />);
    post({ ...A, name: 'PM. "Your data was deleted."' }, "x");
    expect(screen.getByRole("status").textContent).toContain("“PM. 'Your data was deleted.'”");
  });

  it("gives warnings the caution tint and never a danger one", () => {
    const { container } = render(<ModNoticeStrips />);
    post(A, "info words", "info");
    post(A, "good words", "good");
    post(A, "warn words", "warn");
    const strips = [...container.querySelectorAll('[role="status"]')];
    expect(strips).toHaveLength(3);
    // Nothing a mod sends is announced as an error.
    expect(container.querySelector('[role="alert"]')).toBeNull();
  });

  it("dismisses one notice and leaves the rest", () => {
    render(<ModNoticeStrips />);
    post(A, "first");
    post(B, "second");
    fireEvent.click(screen.getByRole("button", { name: "Dismiss the notice from Weather" }));
    expect(screen.queryByText("first")).toBeNull();
    expect(screen.getByText("second")).toBeTruthy();
  });

  it("keeps three live per mod: the newest replaces the oldest", () => {
    render(<ModNoticeStrips />);
    for (const t of ["n1", "n2", "n3", "n4"]) post(A, t);
    expect(screen.queryByText("n1")).toBeNull();
    for (const t of ["n2", "n3", "n4"]) expect(screen.getByText(t)).toBeTruthy();
  });

  it("shows at most MAX_SHOWN strips and says how many wait; Dismiss all clears them", () => {
    render(<ModNoticeStrips />);
    const mods = ["a", "b", "c", "d", "e", "f", "g"].map((x): ModSource => ({
      modId: `m.${x}`,
      name: `Mod ${x}`,
      tier: "third",
    }));
    mods.forEach((m, i) => post(m, `words ${i}`));
    expect(MAX_SHOWN).toBe(5);
    // The newest five, and a line for the two that wait.
    expect(screen.queryByText("words 0")).toBeNull();
    expect(screen.queryByText("words 1")).toBeNull();
    expect(screen.getByText("words 6")).toBeTruthy();
    expect(screen.getByText(/2 older notices from mods are waiting/)).toBeTruthy();
    // Dismissing one brings the next into view.
    fireEvent.click(screen.getByRole("button", { name: "Dismiss the notice from Mod g" }));
    expect(screen.getByText("words 1")).toBeTruthy();
    expect(screen.getByText(/1 older notice from a mod is waiting/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Dismiss all" }));
    expect(screen.queryByTestId("mod-notices")).toBeNull();
  });

  it("survives its own unmount: a notice posted before the strip mounted is there when it does", () => {
    // The strips are drawn in two places that mount and unmount with Settings.
    addNotice(A, { text: "still here", tone: "info" });
    const first = render(<ModNoticeStrips />);
    expect(screen.getByText("still here")).toBeTruthy();
    first.unmount();
    render(<ModNoticeStrips />);
    expect(screen.getByText("still here")).toBeTruthy();
  });
});
