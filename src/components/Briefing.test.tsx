// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The briefing's Refresh button is named by the word it shows (WCAG 2.5.3 Label in Name: a speech
// user says "click Refresh"). It was named "Regenerate today's briefing", which hid the visible word
// from its own name. What it does stays reachable as its description.
//
// It also tells the user what happened: a failed Refresh says why under the briefing, in every
// variant, and the "Updated" stamp carries the time so a same-day success can be seen.

import type { ReactNode } from "react";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { formatClock, formatDateTime } from "../lib/format";

const UPDATED = "2026-11-11T09:00:00Z";
const LATER = "2026-11-11T10:30:00Z";

const state = vi.hoisted(() => ({
  busy: false,
  error: null as string | null,
  updated_at: "2026-11-11T09:00:00Z",
  refresh: vi.fn(() => Promise.resolve()),
  requestRefresh: vi.fn(() => Promise.resolve(null as string | null)),
}));
vi.mock("../lib/briefing", () => ({
  useBriefing: () => ({
    briefing: { briefing: "A quiet day.", updated_at: state.updated_at },
    busy: state.busy,
    error: state.error,
    refresh: state.refresh,
    requestRefresh: state.requestRefresh,
  }),
}));
vi.mock("../lib/markdown", () => ({
  Markdown: ({ children }: { children?: ReactNode }) => <>{children}</>,
}));
vi.mock("../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: "standard" }),
}));

import { Briefing } from "./Briefing";

afterEach(() => {
  cleanup();
  state.busy = false;
  state.error = null;
  state.updated_at = UPDATED;
  state.refresh.mockClear();
  state.requestRefresh.mockClear();
});

describe("Briefing's Refresh", () => {
  it("is named by its visible word, with what it does as the hover text", () => {
    render(<Briefing variant="panel" />);
    const button = screen.getByRole("button", { name: "Refresh" });
    expect(button.getAttribute("aria-label")).toBeNull();
    expect(button.getAttribute("title")).toBe(
      "Regenerate today's briefing from your current projects and calendar",
    );
    fireEvent.click(button);
    // The click is the user asking, so it goes through the call that shows a failure, not the
    // silent one the flag-resolved refresh uses.
    expect(state.requestRefresh).toHaveBeenCalledOnce();
    expect(state.refresh).not.toHaveBeenCalled();
  });

  it("is named by the word it shows while it works too", () => {
    state.busy = true;
    render(<Briefing variant="card" />);
    expect(screen.getByRole("button", { name: "Refreshing…" })).toBeTruthy();
  });
});

describe("Briefing's failure line", () => {
  const WHY = "No AI provider is set up yet — add an OpenRouter key.";

  it.each(["card", "panel"] as const)("says why a Refresh failed in the %s variant", (variant) => {
    state.error = WHY;
    render(<Briefing variant={variant} />);
    const line = screen.getByRole("status");
    expect(line.textContent).toBe(`Couldn't refresh: ${WHY}`);
    // The briefing it already had is still there, and the failure line does not disable the button
    // (the busy -> free transition itself is proved in briefing.test.tsx).
    expect(screen.getByText("A quiet day.")).toBeTruthy();
    expect((screen.getByRole("button", { name: "Refresh" }) as HTMLButtonElement).disabled).toBe(
      false,
    );
  });

  it("says why in the resizable panel too, outside the scrolling text", () => {
    state.error = WHY;
    const { container } = render(<Briefing variant="panel" fill />);
    const line = screen.getByRole("status");
    expect(line.textContent).toBe(`Couldn't refresh: ${WHY}`);
    expect(container.querySelector(".overflow-y-auto")?.contains(line)).toBe(false);
  });

  it("gives the sidebar's failure line its own height cap (a class check, not a layout one)", () => {
    state.error = WHY;
    render(<Briefing variant="panel" />);
    expect(screen.getByRole("status").className).toContain("max-h-24");
  });

  it("shows nothing when nothing failed", () => {
    render(<Briefing variant="card" />);
    expect(screen.queryByRole("status")).toBeNull();
    expect(screen.queryByText(/Couldn't refresh/)).toBeNull();
  });
});

describe("Briefing's Updated stamp", () => {
  it.each(["card", "panel"] as const)("carries the time as well as the day (%s)", (variant) => {
    render(<Briefing variant={variant} />);
    const stamp = screen.getByText(/^Updated /);
    expect(stamp.textContent).toBe(`Updated ${formatDateTime(UPDATED)}`);
    expect(stamp.textContent).toContain(formatClock(new Date(UPDATED)));
  });

  it("changes when a same-day refresh lands", () => {
    const first = render(<Briefing variant="card" />);
    const before = screen.getByText(/^Updated /).textContent;
    first.unmount();
    state.updated_at = LATER;
    render(<Briefing variant="card" />);
    expect(screen.getByText(/^Updated /).textContent).not.toBe(before);
  });
});
