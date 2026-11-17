// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The briefing's Refresh button is named by the word it shows (WCAG 2.5.3 Label in Name: a speech
// user says "click Refresh"). It was named "Regenerate today's briefing", which hid the visible word
// from its own name. What it does stays reachable as its description.

import type { ReactNode } from "react";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const state = vi.hoisted(() => ({
  busy: false,
  refresh: vi.fn(() => Promise.resolve()),
}));
vi.mock("../lib/briefing", () => ({
  useBriefing: () => ({
    briefing: { briefing: "A quiet day.", updated_at: "2026-11-11T09:00:00Z" },
    busy: state.busy,
    refresh: state.refresh,
  }),
}));
vi.mock("../lib/markdown", () => ({
  Markdown: ({ children }: { children?: ReactNode }) => <>{children}</>,
}));
vi.mock("../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: "standard" }),
}));

import { Briefing } from "./Briefing";

afterEach(() => {
  cleanup();
  state.busy = false;
  state.refresh.mockClear();
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
    expect(state.refresh).toHaveBeenCalledOnce();
  });

  it("is named by the word it shows while it works too", () => {
    state.busy = true;
    render(<Briefing variant="card" />);
    expect(screen.getByRole("button", { name: "Refreshing…" })).toBeTruthy();
  });
});
