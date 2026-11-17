// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Developer settings tab: the toggle waits for the backend, a failure leaves it where it was
// and says why, and the copy says what developer mode does now (it allows unsigned mods).

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const setDevMode = vi.fn<(on: boolean) => Promise<void>>();
let devMode = false;
vi.mock("../../lib/capabilities", () => ({
  isDevBuild: true,
  useDevMode: () => ({ devMode, setDevMode }),
}));

// The kit and the test mods read the backend only while developer mode is on.
const devKitInfo = vi.fn();
const listTestMods = vi.fn();
vi.mock("../../lib/ipc", () => ({
  devKitInfo: () => devKitInfo(),
  listTestMods: () => listTestMods(),
  onModsChanged: () => Promise.resolve(() => {}),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: () => Promise.resolve(null) }));

vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { HELP } from "../../lib/help";
import { ModRowsContext } from "../../lib/modRows";
import { failureText } from "../../lib/modsTypes";
import { devKit, mod } from "../mods/modFixtures";
import { DeveloperSettings } from "./DeveloperSettings";
import { OFF_HINT } from "./kitWords";
import { SETTINGS_TABS } from "./registry";

beforeEach(() => {
  setDevMode.mockReset();
  devKitInfo.mockReset();
  listTestMods.mockReset();
  devKitInfo.mockResolvedValue(devKit());
  listTestMods.mockResolvedValue([]);
  devMode = false;
});
afterEach(cleanup);

const toggle = () => screen.getByRole("switch");

describe("the toggle", () => {
  it("asks the backend for the opposite of what it shows", async () => {
    setDevMode.mockResolvedValue();
    render(<DeveloperSettings />);
    fireEvent.click(toggle());
    expect(setDevMode).toHaveBeenCalledWith(true);
    await waitFor(() => expect(toggle().hasAttribute("disabled")).toBe(false));
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("is disabled while the write is pending, so a second click can't race it", async () => {
    let done!: () => void;
    setDevMode.mockReturnValue(new Promise<void>((r) => (done = r)));
    render(<DeveloperSettings />);
    fireEvent.click(toggle());
    await waitFor(() => expect(toggle().hasAttribute("disabled")).toBe(true));
    done();
    await waitFor(() => expect(toggle().hasAttribute("disabled")).toBe(false));
  });

  it("on a failure stays where it was and says why, in the backend's own words", async () => {
    devMode = true;
    setDevMode.mockRejectedValue({ code: "no-store", message: "PM's data isn't open right now." });
    render(<DeveloperSettings />);
    expect(toggle().getAttribute("aria-checked")).toBe("true");
    fireEvent.click(toggle());
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("PM's data isn't open right now.");
    expect(alert.textContent).toContain("Developer mode is still on.");
    expect(toggle().getAttribute("aria-checked")).toBe("true");
    expect(toggle().hasAttribute("disabled")).toBe(false);
  });

  it("clears the last error when it tries again", async () => {
    setDevMode.mockRejectedValueOnce(new Error("disk full"));
    render(<DeveloperSettings />);
    fireEvent.click(toggle());
    expect((await screen.findByRole("alert")).textContent).toContain("disk full");
    setDevMode.mockResolvedValueOnce();
    fireEvent.click(toggle());
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
  });
});

describe("what a failure says", () => {
  it("is the typed refusal's sentence, an error's message, or a plain fallback", () => {
    const fallback = "PM couldn't change developer mode.";
    expect(failureText({ code: "locked", message: "PM is locked." }, fallback)).toBe(
      "PM is locked.",
    );
    expect(failureText(new Error("boom"), fallback)).toBe("boom");
    expect(failureText("???", fallback)).toBe(fallback);
    expect(failureText(new Error(""), fallback)).toBe(fallback);
  });

  it("is what the toggle shows when the rejection is something else entirely", async () => {
    setDevMode.mockRejectedValue("nope");
    render(<DeveloperSettings />);
    fireEvent.click(toggle());
    expect((await screen.findByRole("alert")).textContent).toContain(
      "PM couldn't change developer mode.",
    );
  });
});

describe("the copy", () => {
  it("no longer says developer mode is strictly read-only, and says what it allows", () => {
    render(<DeveloperSettings />);
    const text = document.body.textContent ?? "";
    expect(text).not.toContain("Strictly read-only");
    expect(text).not.toContain("never changes your data");
    fireEvent.click(screen.getByText("What does developer mode do?"));
    const opened = document.body.textContent ?? "";
    expect(opened).toContain("unsigned mods");
    expect(opened).toContain("test mod from a folder");
    expect(opened).toContain("this computer only");
  });
});

describe("the rest of the tab", () => {
  const railIds = () => SETTINGS_TABS.find((t) => t.id === "developer")!.sections.map((s) => s.id);

  it("has every anchor the rail names, whether or not developer mode is on", async () => {
    for (const on of [false, true]) {
      devMode = on;
      const { unmount } = render(
        <ModRowsContext.Provider
          value={{ rows: [], loaded: true, problem: null, refresh: () => Promise.resolve() }}
        >
          <DeveloperSettings />
        </ModRowsContext.Provider>,
      );
      for (const id of railIds()) {
        const el = document.getElementById(id);
        expect(el, `${id} with developer mode ${on ? "on" : "off"}`).not.toBeNull();
        expect(el?.hasAttribute("data-settings-section")).toBe(true);
      }
      await waitFor(() => expect(document.body.textContent).not.toContain("Reading…"));
      unmount();
    }
  });

  it("offers no test mods and asks the backend for nothing while it is off", () => {
    render(<DeveloperSettings />);
    expect(screen.queryByRole("button", { name: "Choose a folder…" })).toBeNull();
    expect(devKitInfo).not.toHaveBeenCalled();
    expect(listTestMods).not.toHaveBeenCalled();
    const gated = document.getElementById("sec-dev-testmods")?.textContent?.includes(OFF_HINT);
    expect(gated).toBe(true);
    // Test keys too: there is no test mod to type a key for.
    expect(document.getElementById("sec-dev-testkeys")?.textContent).toContain(OFF_HINT);
    expect(screen.queryByLabelText(/Key: /)).toBeNull();
  });

  it("offers 'Test keys' once it is on, listing the loaded test mods only", () => {
    devMode = true;
    const slot = { name: "weather", host: "api.example.com", header: "X-Api-Key", set: false };
    render(
      <ModRowsContext.Provider
        value={{
          rows: [
            mod({ id: "test.one", name: "Test One", installedFrom: "folder", secrets: [slot] }),
            mod({ id: "real.one", name: "Real One", installedFrom: "catalogue", secrets: [slot] }),
          ],
          loaded: true,
          problem: null,
          refresh: () => Promise.resolve(),
        }}
      >
        <DeveloperSettings />
      </ModRowsContext.Provider>,
    );
    const section = document.getElementById("sec-dev-testkeys")!;
    expect(section.textContent).toContain("Test One");
    expect(section.textContent).not.toContain("Real One");
    expect(within(section).getAllByLabelText(/Key: weather/)).toHaveLength(1);
  });

  it("reads the kit and offers 'Load a test mod' once it is on", async () => {
    devMode = true;
    render(<DeveloperSettings />);
    expect(await screen.findByRole("button", { name: "Choose a folder…" })).toBeTruthy();
    await waitFor(() => expect(devKitInfo).toHaveBeenCalledTimes(1));
    expect(listTestMods).toHaveBeenCalled();
    expect(await screen.findByText("Calls per second, sustained")).toBeTruthy();
  });

  it("says turning it off unloads the test mods, as well as pausing the unsigned ones", () => {
    render(<DeveloperSettings />);
    expect(document.body.textContent).toContain("unloads every test mod for good");
  });

  it("gives every surface a help entry", async () => {
    devMode = true;
    render(<DeveloperSettings />);
    await screen.findByText("Calls per second, sustained");
    const ids = [...document.querySelectorAll("[data-help]")].map((e) =>
      e.getAttribute("data-help")!,
    );
    expect(ids).toEqual(
      expect.arrayContaining([
        "settings-developer",
        "settings-developer-api",
        "settings-developer-endpoints",
        "settings-developer-limits",
        "settings-developer-running",
        "settings-developer-testmods",
      ]),
    );
    for (const id of ids) expect(HELP[id], `no help entry for ${id}`).toBeDefined();
  });
});
