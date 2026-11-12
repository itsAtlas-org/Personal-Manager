// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The tooltip meets WCAG 2.2 1.4.13 (Content on Hover or Focus): shown on hover AND keyboard focus,
// dismissible with Esc without moving focus, hoverable (the pointer can rest on it), and kept inside
// the window. jsdom has no layout, so the measuring is fed through stubbed rects and offset sizes.

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, onTestFinished, vi } from "vitest";
import { Tooltip } from "./Tooltip";

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const settle = (ms = 200) => act(() => void vi.advanceTimersByTime(ms));

function mount(placement?: "above" | "below") {
  render(
    <Tooltip label="Refresh · last synced 10-11 14:32" placement={placement}>
      <button type="button">Go</button>
    </Tooltip>,
  );
  return screen.getByRole("button", { name: "Go" });
}

// jsdom decides `:focus-visible` its own way, so the modality is stated: a Tab press matches it, a
// mouse click's focus does not. Everything else `matches` is asked goes to the real one.
function focusAs(button: HTMLElement, modality: "keyboard" | "mouse") {
  const real = Element.prototype.matches;
  vi.spyOn(Element.prototype, "matches").mockImplementation(function (this: Element, sel: string) {
    return sel === ":focus-visible" ? modality === "keyboard" : real.call(this, sel);
  });
  fireEvent.focus(button);
}

describe("Tooltip", () => {
  it("shows on keyboard focus, and goes when focus leaves", () => {
    const button = mount();
    expect(screen.queryByRole("tooltip")).toBeNull();
    focusAs(button, "keyboard");
    settle();
    expect(screen.getByRole("tooltip").textContent).toBe("Refresh · last synced 10-11 14:32");
    fireEvent.blur(button);
    expect(screen.queryByRole("tooltip")).toBeNull();
  });

  it("shows on hover", () => {
    const button = mount();
    fireEvent.pointerEnter(button);
    settle();
    expect(screen.getByRole("tooltip")).toBeTruthy();
  });

  it("is dismissed by Esc, with focus staying on the control and the key left for others", () => {
    const button = mount();
    focusAs(button, "keyboard");
    button.focus();
    settle();
    expect(screen.getByRole("tooltip")).toBeTruthy();
    // A listener further up, as a dialog's own Esc: it still hears the key.
    const heard = vi.fn();
    window.addEventListener("keydown", heard);
    fireEvent.keyDown(button, { key: "Escape" });
    window.removeEventListener("keydown", heard);
    expect(screen.queryByRole("tooltip")).toBeNull();
    expect(document.activeElement).toBe(button);
    expect(heard).toHaveBeenCalledOnce();
  });

  it("stays while the pointer moves from the control onto it, and goes after it leaves", () => {
    const button = mount();
    fireEvent.pointerEnter(button);
    settle();
    const tip = screen.getByRole("tooltip");
    // Off the control: within the grace it is still up, and resting on it keeps it.
    fireEvent.pointerLeave(button);
    settle(100);
    fireEvent.pointerEnter(tip);
    settle(500);
    expect(screen.getByRole("tooltip")).toBeTruthy();
    // Off the tooltip as well: gone after the short grace.
    fireEvent.pointerLeave(tip);
    settle(100);
    expect(screen.getByRole("tooltip")).toBeTruthy();
    settle(100);
    expect(screen.queryByRole("tooltip")).toBeNull();
  });

  it("stays while the control keeps keyboard focus, even after the pointer leaves", () => {
    const button = mount();
    focusAs(button, "keyboard");
    fireEvent.pointerEnter(button);
    settle();
    fireEvent.pointerLeave(button);
    settle(500);
    expect(screen.getByRole("tooltip")).toBeTruthy();
  });

  it("does not stay after a click: the focus a press gives is not keyboard focus", () => {
    const button = mount();
    fireEvent.pointerEnter(button);
    settle();
    // pointerdown closes it, the press's focus must not bring it back or pin it...
    fireEvent.pointerDown(button);
    focusAs(button, "mouse");
    settle();
    expect(screen.queryByRole("tooltip")).toBeNull();
    // ...so after the pointer leaves, a later hover-and-leave still comes and goes.
    fireEvent.pointerEnter(button);
    settle();
    expect(screen.getByRole("tooltip")).toBeTruthy();
    fireEvent.pointerLeave(button);
    settle(200);
    expect(screen.queryByRole("tooltip")).toBeNull();
  });

  it("stays when the pointer comes back from the tooltip onto the control", () => {
    const button = mount();
    fireEvent.pointerEnter(button);
    settle();
    const tip = screen.getByRole("tooltip");
    fireEvent.pointerLeave(button);
    fireEvent.pointerEnter(tip);
    fireEvent.pointerLeave(tip, { relatedTarget: button });
    settle(500);
    expect(screen.getByRole("tooltip")).toBeTruthy();
  });

  it("is dismissed by pressing the control", () => {
    const button = mount();
    fireEvent.pointerEnter(button);
    settle();
    fireEvent.pointerDown(button);
    expect(screen.queryByRole("tooltip")).toBeNull();
  });

  it("is clamped inside the window for a control in the right-hand corner", () => {
    const wide = window.innerWidth;
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 480 });
    onTestFinished(() => {
      Object.defineProperty(window, "innerWidth", { configurable: true, value: wide });
    });
    vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(200);
    vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(20);
    const button = mount("below");
    // The control's box sits against the right edge: centring a 200 px tooltip on it would end past 480.
    vi.spyOn(button.parentElement as HTMLElement, "getBoundingClientRect").mockReturnValue({
      left: 450,
      right: 474,
      top: 40,
      bottom: 64,
      width: 24,
      height: 24,
      x: 450,
      y: 40,
      toJSON: () => ({}),
    });
    fireEvent.pointerEnter(button);
    settle();
    const tip = screen.getByRole("tooltip");
    expect(tip.style.left).toBe("272px"); // 480 - 200 - 8
    expect(tip.style.top).toBe("70px"); // below the control: 64 + 6
    expect(tip.className).not.toContain("invisible");
  });

  it("is clamped at the left edge too", () => {
    vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(200);
    const button = mount();
    vi.spyOn(button.parentElement as HTMLElement, "getBoundingClientRect").mockReturnValue({
      left: 0,
      right: 24,
      top: 100,
      bottom: 124,
      width: 24,
      height: 24,
      x: 0,
      y: 100,
      toJSON: () => ({}),
    });
    fireEvent.pointerEnter(button);
    settle();
    expect(screen.getByRole("tooltip").style.left).toBe("8px");
  });

  it("puts a className on its wrapping span, for a control that fills its row", () => {
    render(
      <Tooltip label="New event" className="w-full">
        <button type="button">Go</button>
      </Tooltip>,
    );
    const wrap = screen.getByRole("button", { name: "Go" }).parentElement as HTMLElement;
    expect(wrap.className).toContain("w-full");
    expect(wrap.className).toContain("inline-flex");
  });
});
