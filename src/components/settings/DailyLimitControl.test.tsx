// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The daily limit control. What it must get right: off by default; turning it on writes the $5
// suggestion, so the brake is real from the first click; a bad amount is refused in words and never
// sent; off is null and never 0; the pause is a standing line, not a toast; a proven-free key can't
// have one; and the page never works out a day or a spend, it asks Rust again after every write.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const m = vi.hoisted(() => ({ setDailyLimit: vi.fn() }));

vi.mock("../../lib/ipc", () => ({
  setDailyLimit: (...a: unknown[]) => m.setDailyLimit(...a),
}));
vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { limitView } from "../../lib/directProvidersFixture";
import type { LimitView } from "../../lib/types";
import { DailyLimitControl } from "./DailyLimitControl";

const onChanged = vi.fn();
const onError = vi.fn();

function mount(limit: LimitView, provider: "anthropic" | "gemini" | "openrouter" = "anthropic") {
  return render(
    <DailyLimitControl
      provider={provider}
      name="Anthropic"
      limit={limit}
      onChanged={onChanged}
      onError={onError}
    />,
  );
}
const toggle = () =>
  screen.getByRole("switch", { name: "Daily limit for Anthropic" }) as HTMLButtonElement;
// The visible label plus a hidden provider name, so four of these on one page are told apart.
const saved = () => screen.getByTestId("daily-limit-saved-anthropic");
const paused = () => screen.getByTestId("daily-limit-pause-anthropic");
const FIELD = /^Stop after \(USD per day\) for Anthropic$/;
const field = () => screen.getByLabelText(FIELD) as HTMLInputElement;

beforeEach(() => {
  m.setDailyLimit.mockResolvedValue(undefined);
  onChanged.mockResolvedValue(undefined);
});
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("the daily limit control", () => {
  it("is off by default: a switch and no field, and nothing is written", () => {
    mount(limitView());
    expect(toggle().getAttribute("aria-checked")).toBe("false");
    expect(screen.queryByLabelText(FIELD)).toBeNull();
    expect(m.setDailyLimit).not.toHaveBeenCalled();
  });

  it("turning it on writes the $5 suggestion and reads the state again", async () => {
    mount(limitView());
    fireEvent.click(toggle());
    await waitFor(() => expect(m.setDailyLimit).toHaveBeenCalledWith("anthropic", 5));
    await waitFor(() => expect(onChanged).toHaveBeenCalledTimes(1));
  });

  it("shows the stored amount in the field and saves a new one when the field is left", async () => {
    mount(limitView({ limit_usd: 5, spent_today_usd: 1.2 }));
    expect(field().value).toBe("5.00");
    fireEvent.change(field(), { target: { value: "7.5" } });
    fireEvent.blur(field());
    await waitFor(() => expect(m.setDailyLimit).toHaveBeenCalledWith("anthropic", 7.5));
  });

  it("an unchanged amount is no write", () => {
    mount(limitView({ limit_usd: 5 }));
    fireEvent.blur(field());
    expect(m.setDailyLimit).not.toHaveBeenCalled();
  });

  it.each(["0", "", "-2", "1e9", "lots"])("refuses %j in words and never sends it", async (bad) => {
    mount(limitView({ limit_usd: 5 }));
    fireEvent.change(field(), { target: { value: bad } });
    fireEvent.blur(field());
    const message = await screen.findByText("Enter an amount above 0, or turn the limit off.");
    expect(m.setDailyLimit).not.toHaveBeenCalled();
    // The accessibility contract: announced as an alert, and tied to the field that is wrong.
    expect(message.getAttribute("role")).toBe("alert");
    expect(field().getAttribute("aria-invalid")).toBe("true");
    expect(field().getAttribute("aria-describedby")?.split(" ")).toContain(message.id);
  });

  it("does not turn a stored $5.125 into $5.13 by leaving the field untouched", () => {
    // The field shows cents; reading it back must not be a different limit.
    mount(limitView({ limit_usd: 5.125 }));
    expect(field().value).toBe("5.13");
    fireEvent.blur(field());
    expect(m.setDailyLimit).not.toHaveBeenCalled();
  });

  it("says on, off and a saved amount in a visible polite status line, naming the provider", async () => {
    const { rerender } = mount(limitView());
    expect(saved().getAttribute("role")).toBe("status");
    expect(saved().getAttribute("aria-live")).toBe("polite");
    expect(saved().textContent).toBe("");
    fireEvent.click(toggle());
    await waitFor(() =>
      expect(saved().textContent).toBe("Daily limit on, $5.00 a day for Anthropic."),
    );
    // Seen as well as heard: it is not in the visually hidden wrapper.
    expect(saved().closest(".sr-only")).toBeNull();
    rerender(
      <DailyLimitControl
        provider="anthropic"
        name="Anthropic"
        limit={limitView({ limit_usd: 5 })}
        onChanged={onChanged}
        onError={onError}
      />,
    );
    fireEvent.change(field(), { target: { value: "7.5" } });
    fireEvent.blur(field());
    await waitFor(() => expect(saved().textContent).toBe("Daily limit $7.50 a day for Anthropic."));
    // The next edit clears it: a stale confirmation never sits beside a field being changed.
    fireEvent.change(field(), { target: { value: "8" } });
    expect(saved().textContent).toBe("");
    fireEvent.click(toggle());
    await waitFor(() => expect(saved().textContent).toBe("Daily limit off for Anthropic."));
    // The save words are never also in the hidden pause region (a screen reader would say them twice).
    expect(paused().textContent).toBe("");
  });

  it("announces a pause when it begins, not on first render", () => {
    const { rerender } = mount(limitView({ limit_usd: 5, spent_today_usd: 1 }));
    expect(paused().textContent).toBe("");
    rerender(
      <DailyLimitControl
        provider="anthropic"
        name="Anthropic"
        limit={limitView({ limit_usd: 5, spent_today_usd: 5.1, reached: true })}
        onChanged={onChanged}
        onError={onError}
      />,
    );
    expect(paused().textContent).toBe("Anthropic is paused for today.");
    expect(saved().textContent).toBe("");
    // Raising the limit lifts the pause and the announcement with it.
    rerender(
      <DailyLimitControl
        provider="anthropic"
        name="Anthropic"
        limit={limitView({ limit_usd: 9, spent_today_usd: 5.1 })}
        onChanged={onChanged}
        onError={onError}
      />,
    );
    expect(paused().textContent).toBe("");
  });

  it("does not say 'for today' when the pause is a limit PM could not read", () => {
    const { rerender } = mount(limitView({ limit_usd: 5 }));
    rerender(
      <DailyLimitControl
        provider="anthropic"
        name="Anthropic"
        limit={limitView({ limit_usd: null, reached: true, unreadable: true })}
        onChanged={onChanged}
        onError={onError}
      />,
    );
    expect(paused().textContent).toBe(
      "Anthropic is paused: PM couldn't read the limit or today's usage.",
    );
    expect(paused().textContent).not.toMatch(/for today/);
    rerender(
      <DailyLimitControl
        provider="anthropic"
        name="Anthropic"
        limit={limitView({ limit_usd: 5 })}
        onChanged={onChanged}
        onError={onError}
      />,
    );
    // Cleared whichever of the two forms was said.
    expect(paused().textContent).toBe("");
  });

  it("shows a limit PM can't read as ON, names the provider, and turning it off clears it", async () => {
    // A hand-edited or foreign value: no amount, but the provider is paused to be safe.
    mount(limitView({ limit_usd: null, reached: true, unreadable: true }));
    expect(toggle().getAttribute("aria-checked")).toBe("true");
    expect(
      screen.getByText(
        "PM couldn't read today's usage, so it paused Anthropic to be safe. Check Usage, or turn the limit off.",
      ),
    ).toBeTruthy();
    fireEvent.click(toggle());
    await waitFor(() => expect(m.setDailyLimit).toHaveBeenCalledWith("anthropic", null));
  });

  it("lets an unreadable limit be cleared even on a key found to be free", () => {
    mount(
      limitView({ free_key: true, limit_usd: null, reached: true, unreadable: true }),
      "gemini",
    );
    expect(toggle().disabled).toBe(false);
  });

  it("lets an amount typed over an unreadable limit replace it", async () => {
    mount(limitView({ limit_usd: null, reached: true, unreadable: true }));
    fireEvent.change(field(), { target: { value: "8" } });
    fireEvent.blur(field());
    await waitFor(() => expect(m.setDailyLimit).toHaveBeenCalledWith("anthropic", 8));
  });

  it("turning it off sends null, never 0", async () => {
    mount(limitView({ limit_usd: 5 }));
    fireEvent.click(toggle());
    await waitFor(() => expect(m.setDailyLimit).toHaveBeenCalledWith("anthropic", null));
    expect(m.setDailyLimit).not.toHaveBeenCalledWith("anthropic", 0);
  });

  it("says a write failed instead of swallowing it", async () => {
    m.setDailyLimit.mockRejectedValue("Enter an amount above 0, or turn the limit off.");
    mount(limitView());
    fireEvent.click(toggle());
    await waitFor(() =>
      expect(onError).toHaveBeenCalledWith("Enter an amount above 0, or turn the limit off."),
    );
  });

  it("keeps the pause as a standing line the person can find again, not a toast", () => {
    mount(limitView({ limit_usd: 5, spent_today_usd: 5.12, reached: true }));
    const line = screen.getByText(
      "Paused for today. PM has logged at least $5.12 of your $5.00 limit. It starts again at 00:00 (your day), or sooner if you raise the limit.",
    );
    expect(line.closest('[role="alert"]')).toBeNull();
    expect(line.closest('[aria-live="assertive"]')).toBeNull();
  });

  it("says nothing is paused within the limit", () => {
    mount(limitView({ limit_usd: 5, spent_today_usd: 1.2 }));
    expect(screen.queryByText(/Paused for today/)).toBeNull();
  });

  it("disables the switch for a key PM has found to be free, and says why", () => {
    mount(limitView({ free_key: true }), "gemini");
    expect(toggle().disabled).toBe(true);
    expect(
      screen.getByText("Your free key doesn't cost anything, so a limit isn't needed."),
    ).toBeTruthy();
    fireEvent.click(toggle());
    expect(m.setDailyLimit).not.toHaveBeenCalled();
  });

  it("lets a limit that is already on be turned off even on a free key", () => {
    mount(limitView({ free_key: true, limit_usd: 5 }), "gemini");
    expect(toggle().disabled).toBe(false);
  });

  it("re-seeds the field when Rust reports another amount", () => {
    const { rerender } = mount(limitView({ limit_usd: 5 }));
    rerender(
      <DailyLimitControl
        provider="anthropic"
        name="Anthropic"
        limit={limitView({ limit_usd: 12 })}
        onChanged={onChanged}
        onError={onError}
      />,
    );
    expect(field().value).toBe("12.00");
  });

  it("names the provider's own limit as the hard stop in the help", () => {
    mount(limitView());
    fireEvent.click(screen.getByText("About the Anthropic daily limit"));
    expect(screen.getByText(/also set a spending limit in your Anthropic Console/)).toBeTruthy();
    expect(screen.getByText(/another computer on the same key isn't counted/)).toBeTruthy();
  });
});
