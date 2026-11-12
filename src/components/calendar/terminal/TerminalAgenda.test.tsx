// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Terminal's open-ended agenda mirrors AgendaView's empty list (#1090): when a filter (Hide done,
// All-day) is why nothing is left, it says so rather than the plain line, which would read as an empty
// calendar.

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "terminal", depth: "standard", accent: "mono", colorblind: false }),
}));

import { TerminalAgenda } from "./TerminalAgenda";

afterEach(cleanup);

describe("TerminalAgenda's empty list", () => {
  it("says what the calendar gives it, and the plain line otherwise", () => {
    const empty = (emptyText?: string) =>
      render(
        <TerminalAgenda
          events={[]}
          fromDay={new Date(2026, 9, 12)}
          colorOf={() => "#336699"}
          emptyText={emptyText}
        />,
      );
    empty("No all-day events in view.");
    expect(screen.getByText("No all-day events in view.")).toBeTruthy();
    expect(screen.queryByText("No events in view.")).toBeNull();
    cleanup();
    empty();
    expect(screen.getByText("No events in view.")).toBeTruthy();
  });
});
