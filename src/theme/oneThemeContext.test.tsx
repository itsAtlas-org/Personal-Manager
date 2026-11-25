// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// There is ONE theme context. The context lives in the mod kit (src/kit/theme.tsx) and core's
// `ThemeProvider` wraps it with the `ipc` store; a copy of the context in core would leave a kit
// component (a Button) reading a different theme from the core component beside it, or throwing for
// want of a provider. So: core's hook IS the kit's, a kit component under core's provider reads what
// core's provider holds, and core's provider mirrors its theme through `ipc` as it always did.

import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../lib/ipc", () => ({
  getPref: vi.fn(async () => null),
  setPref: vi.fn(async () => {}),
}));

import * as ipc from "../lib/ipc";
import { Select } from "../kit/ui/Select";
import { useTheme as kitUseTheme } from "../kit/theme";
import { ThemeProvider, useDepth, useTheme } from "./index";
import { useTheme as contextUseTheme } from "./ThemeContext";

function CoreSide() {
  const { system, setSystem } = useTheme();
  const { depth } = useDepth();
  return (
    <button onClick={() => setSystem("terminal")}>
      core:{system}:{depth}
    </button>
  );
}

beforeEach(() => localStorage.clear());
afterEach(cleanup);

describe("one theme context", () => {
  it("core's useTheme is the kit's", () => {
    expect(useTheme).toBe(kitUseTheme);
    expect(contextUseTheme).toBe(kitUseTheme);
  });

  it("a kit component under core's provider reads the theme core's components set", async () => {
    render(
      <ThemeProvider>
        <CoreSide />
        <Select aria-label="pick" />
      </ThemeProvider>,
    );
    // The kit's Select reads `system` from the context: the terminal System sets it in mono.
    expect(screen.getByRole("combobox").className).not.toContain("font-mono");
    await act(async () => screen.getByText(/^core:slate/).click());
    expect(screen.getByText(/^core:terminal/)).toBeTruthy();
    expect(screen.getByRole("combobox").className).toContain("font-mono");
  });

  it("core's provider mirrors the theme through ipc, as the `appearance` setting", async () => {
    render(
      <ThemeProvider>
        <CoreSide />
      </ThemeProvider>,
    );
    await waitFor(() => expect(ipc.getPref).toHaveBeenCalledWith("appearance"));
    await waitFor(() => expect(ipc.setPref).toHaveBeenCalled());
    const [key, value] = vi.mocked(ipc.setPref).mock.calls.slice(-1)[0];
    expect(key).toBe("appearance");
    expect(JSON.parse(value).system).toBe("slate");
  });
});
