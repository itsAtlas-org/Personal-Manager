// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import app from "../App.tsx?raw";
import { ListNotice } from "./ListNotice";

afterEach(cleanup);

describe("the list notice", () => {
  it("is information, not an error: a polite status that is never an alert", () => {
    render(<ListNotice text="2 other saved models weren't in use." onDismiss={() => {}} />);
    expect(screen.getByRole("status").textContent).toContain("2 other saved models weren't in use");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("can be dismissed", () => {
    const onDismiss = vi.fn();
    render(<ListNotice text="x" onDismiss={onDismiss} />);
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });

  it("is where the app puts the first edit's line, never the chat's error banner", () => {
    expect(app).toContain("<ListNotice");
    expect(app).not.toContain("chat.setError(note)");
  });

  it("is also where 'Use … from now on' says how many saved models its first edit left out", () => {
    // The card's promote used to drop the edit's answer, so a first edit made from the card said
    // nothing about the saved models it left out (Q7).
    const promote = app.slice(app.indexOf("promote: async (entry) =>"));
    const handler = promote.slice(0, promote.indexOf("\n    },"));
    expect(handler).toContain("firstEditLine(edited.unused_saved)");
    expect(handler).toContain("setListNotice(line)");
  });
});
