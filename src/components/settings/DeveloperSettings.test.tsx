// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Developer settings tab: the toggle waits for the backend, a failure leaves it where it was
// and says why, and the copy says what developer mode does now (it allows unsigned mods).

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const setDevMode = vi.fn<(on: boolean) => Promise<void>>();
let devMode = false;
vi.mock("../../lib/capabilities", () => ({
  isDevBuild: true,
  useDevMode: () => ({ devMode, setDevMode }),
}));

vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { failureText } from "../../lib/modsTypes";
import { DeveloperSettings } from "./DeveloperSettings";

beforeEach(() => {
  setDevMode.mockReset();
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
