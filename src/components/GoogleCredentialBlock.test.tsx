// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The shared Google credentials block: its setup guide names every API a connector needs (the People
// API for Contacts included), and clearing the credentials tells the user their people stay in PM.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const clearGoogleClient = vi.fn();

vi.mock("../lib/ipc", () => ({
  clearGoogleClient: () => clearGoogleClient(),
  setGoogleClient: vi.fn(),
}));

// The <Button>s reach for `useTheme`, and the real ThemeProvider pulls in IPC.
vi.mock("../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({
    system: "slate",
    mode: "dark",
    modePref: "system",
    modeSource: "system",
    accent: "mono",
    depth: "standard",
    autoLocation: "",
    teachVisible: true,
    setSystem: () => {},
    setModePref: () => {},
    setAccent: () => {},
    setDepth: () => {},
    setAutoLocation: () => {},
    setTeachVisible: () => {},
  }),
}));

import { GoogleCredentialBlock } from "./GoogleCredentialBlock";

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("GoogleCredentialBlock", () => {
  it("lists the Google People API with its console link in the setup step", () => {
    render(<GoogleCredentialBlock configured={false} onChange={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: /how do i create this/i }));
    const link = screen.getByRole("link", { name: "Google People API" });
    expect(link.getAttribute("href")).toBe(
      "https://console.cloud.google.com/apis/library/people.googleapis.com",
    );
    // Beside the three it already named.
    for (const name of ["Google Drive API", "Google Calendar API", "Google Sheets API"]) {
      expect(screen.getByRole("link", { name })).toBeTruthy();
    }
  });

  it("says the people stay in PM before clearing the credentials", () => {
    render(<GoogleCredentialBlock configured onChange={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Clear credentials" }));
    const dialog = screen.getByRole("dialog");
    expect(dialog.textContent).toContain("Your people stay in PM.");
    expect(dialog.textContent).toContain("(Calendar, Drive, Contacts)");
    // Nothing is cleared until the user says so.
    expect(clearGoogleClient).not.toHaveBeenCalled();
  });
});
