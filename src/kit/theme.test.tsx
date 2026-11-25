// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The kit's theme provider and the `ThemeStore` it is given (the one thing it cannot reach on its own:
// core gives it the encrypted settings table through `ipc`, a mod's frame gives it the host's theme).
// What the store is asked and when is what core's old provider did with `getPref` and `setPref`.

import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ThemeProvider, useTheme, type ThemeStore } from "./theme";

function Show() {
  const { system, modePref, accent, setSystem } = useTheme();
  return (
    <button onClick={() => setSystem("terminal")}>
      {system}:{modePref}:{accent}
    </button>
  );
}

function store(blob: string | null): ThemeStore & {
  get: ReturnType<typeof vi.fn>;
  set: ReturnType<typeof vi.fn>;
} {
  return { get: vi.fn(async () => blob), set: vi.fn(async () => {}) };
}

const settle = () => act(async () => void (await Promise.resolve()));

beforeEach(() => localStorage.clear());
afterEach(cleanup);

describe("ThemeProvider with a ThemeStore", () => {
  it("on a fresh machine (nothing in localStorage) takes the stored theme from the store", async () => {
    const s = store(JSON.stringify({ system: "terminal", modePref: "light" }));
    render(
      <ThemeProvider store={s}>
        <Show />
      </ThemeProvider>,
    );
    await waitFor(() => expect(screen.getByRole("button").textContent).toMatch(/^terminal:light:/));
    expect(s.get).toHaveBeenCalledTimes(1);
  });

  it("on a machine that already has its theme keeps it, and reads the store only to unlock the mirror", async () => {
    localStorage.setItem("pm:theme:system", "slate");
    localStorage.setItem("pm:theme:modePref", "dark");
    const s = store(JSON.stringify({ system: "terminal", modePref: "light" }));
    render(
      <ThemeProvider store={s}>
        <Show />
      </ThemeProvider>,
    );
    await settle();
    expect(screen.getByRole("button").textContent).toMatch(/^slate:dark:/);
    // Hydrated (the mirror is unlocked), so the first persist wrote the theme it has.
    await waitFor(() => expect(s.set).toHaveBeenCalled());
    expect(JSON.parse(s.set.mock.calls[s.set.mock.calls.length - 1][0])).toMatchObject({
      system: "slate",
      modePref: "dark",
    });
  });

  it("writes nothing to the store until the store has been read (a default can't overwrite a stored theme)", async () => {
    let release: (v: string | null) => void = () => {};
    const s: ThemeStore & { set: ReturnType<typeof vi.fn> } = {
      get: () => new Promise<string | null>((r) => (release = r)),
      set: vi.fn(async () => {}),
    };
    render(
      <ThemeProvider store={s}>
        <Show />
      </ThemeProvider>,
    );
    await settle();
    expect(s.set).not.toHaveBeenCalled();
    await act(async () => release(null));
    await waitFor(() => expect(s.set).toHaveBeenCalled());
  });

  it("mirrors a change into the store as one JSON blob of every axis", async () => {
    const s = store(null);
    render(
      <ThemeProvider store={s}>
        <Show />
      </ThemeProvider>,
    );
    await waitFor(() => expect(s.set).toHaveBeenCalled());
    s.set.mockClear();
    await act(async () => screen.getByRole("button").click());
    await waitFor(() => expect(s.set).toHaveBeenCalled());
    const blob = JSON.parse(s.set.mock.calls[s.set.mock.calls.length - 1][0]);
    expect(blob.system).toBe("terminal");
    expect(Object.keys(blob).sort()).toEqual(
      [
        "accent",
        "accentBySystem",
        "autoLocation",
        "colorblind",
        "contrast",
        "density",
        "depth",
        "fontScale",
        "legibleFont",
        "modePref",
        "reduceMotion",
        "system",
      ].sort(),
    );
  });

  it("goes on with its defaults when the store cannot be read, and when it cannot be written", async () => {
    const s: ThemeStore & { set: ReturnType<typeof vi.fn> } = {
      get: async () => {
        throw new Error("no store yet");
      },
      set: vi.fn(async () => {
        throw new Error("read-only");
      }),
    };
    render(
      <ThemeProvider store={s}>
        <Show />
      </ThemeProvider>,
    );
    await waitFor(() => expect(s.set).toHaveBeenCalled());
    expect(screen.getByRole("button").textContent).toMatch(/^slate:dark:/);
  });

  it("throws outside a provider, as the core hook always has", () => {
    const quiet = vi.spyOn(console, "error").mockImplementation(() => {});
    expect(() => render(<Show />)).toThrow("useTheme must be used within <ThemeProvider>");
    quiet.mockRestore();
  });
});

describe("a ThemeStore that changes under the provider (a frame's)", () => {
  /** A store the test pushes blobs through, with PM's own variables. */
  function pushing() {
    const subs = new Set<(blob: string | null) => void>();
    let vars: Record<string, string> | null = null;
    return {
      get: vi.fn(async () => null as string | null),
      set: vi.fn(async () => {}),
      subscribe: (cb: (blob: string | null) => void) => {
        subs.add(cb);
        return () => void subs.delete(cb);
      },
      vars: () => vars,
      push: (blob: string | null, next?: Record<string, string> | null) => {
        if (next !== undefined) vars = next;
        for (const cb of [...subs]) cb(blob);
      },
      live: () => subs.size,
    };
  }

  it("applies each blob it is told of, and stops listening when the provider goes", async () => {
    const s = pushing();
    const { unmount } = render(
      <ThemeProvider store={s}>
        <Show />
      </ThemeProvider>,
    );
    await settle();
    expect(s.live()).toBe(1);
    await act(async () => s.push(JSON.stringify({ system: "terminal", modePref: "light" })));
    expect(screen.getByRole("button").textContent).toMatch(/^terminal:light:/);
    unmount();
    expect(s.live()).toBe(0);
  });

  it("sets the store's own variables on the root after the provider's, and again when they change", async () => {
    const s = pushing();
    document.documentElement.style.removeProperty("--ink3");
    render(
      <ThemeProvider store={s}>
        <Show />
      </ThemeProvider>,
    );
    await settle();
    const derived = document.documentElement.style.getPropertyValue("--ink3");
    expect(derived).not.toBe("");
    // The same axes, new variables: nothing in the blob changes, the variables do.
    await act(async () => s.push(null, { "--ink3": "oklch(0.9 0 0)" }));
    expect(document.documentElement.style.getPropertyValue("--ink3")).toBe("oklch(0.9 0 0)");
    await act(async () => s.push(null, { "--ink3": "oklch(0.8 0 0)" }));
    expect(document.documentElement.style.getPropertyValue("--ink3")).toBe("oklch(0.8 0 0)");
  });

  it("keeps the theme it has when a pushed blob is not JSON", async () => {
    const s = pushing();
    render(
      <ThemeProvider store={s}>
        <Show />
      </ThemeProvider>,
    );
    await settle();
    await act(async () => s.push("{nope"));
    expect(screen.getByRole("button").textContent).toMatch(/^slate:dark:/);
  });

  it("a store with neither is core's: nothing is subscribed and nothing is overridden", async () => {
    const s = store(null);
    render(
      <ThemeProvider store={s}>
        <Show />
      </ThemeProvider>,
    );
    await settle();
    expect(screen.getByRole("button").textContent).toMatch(/^slate:dark:/);
  });
});
