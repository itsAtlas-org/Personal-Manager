// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Install from file: the one home of file installs. Choosing a file reads it and stages a screen, and
// installs nothing; an unsigned file with developer mode off says so and where the switch is; the read
// outlives the view.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { currentModActions, resetModActions } from "../../lib/modActions";
import { DEV_MODE_OFF_TEXT } from "../../lib/modInstall";
import { consentScreen } from "./modFixtures";

const openDialog = vi.fn();
const installModFile = vi.fn();
const installUnsignedFile = vi.fn();
const devModeState = vi.fn();
const installModConsented = vi.fn();

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: (o: unknown) => openDialog(o),
}));
vi.mock("../../lib/ipc", () => ({
  installModFile: (p: string) => installModFile(p),
  installUnsignedFile: (p: string) => installUnsignedFile(p),
  devModeState: () => devModeState(),
  installModConsented: (...a: unknown[]) => installModConsented(...a),
}));
vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { InstallFromFile } from "./InstallFromFile";

beforeEach(() => {
  vi.clearAllMocks();
  resetModActions();
});
afterEach(cleanup);

const choose = () => fireEvent.click(screen.getByRole("button", { name: "Choose a mod file…" }));

describe("what it says", () => {
  it("says this is the only home of file installs, and that an unsigned file needs developer mode and why", () => {
    render(<InstallFromFile />);
    const text = document.body.textContent ?? "";
    expect(text).toContain("This is the only place to do that.");
    expect(text).toContain("nothing is installed until you say so");
    expect(text).toContain("A file nobody has signed can't be checked by anyone");
    expect(text).toContain("developer mode is on (Settings, then Developer)");
    expect(text).toContain("type the mod's id to confirm");
  });
});

describe("choosing a file", () => {
  it("asks the file dialog for a single .pmmod, and reads the one picked", async () => {
    openDialog.mockResolvedValue("/home/me/map.pmmod");
    installModFile.mockResolvedValue(consentScreen({ id: "atlas.map", kind: "fresh" }));
    render(<InstallFromFile />);
    choose();
    await waitFor(() => expect(installModFile).toHaveBeenCalledWith("/home/me/map.pmmod"));
    expect(openDialog).toHaveBeenCalledWith({
      multiple: false,
      filters: [{ name: "PM mod", extensions: ["pmmod"] }],
    });
    await waitFor(() => expect([...currentModActions().screens.keys()]).toEqual(["atlas.map"]));
    // Reading is not installing.
    expect(installModConsented).not.toHaveBeenCalled();
  });

  it("does nothing when the dialog is cancelled", async () => {
    openDialog.mockResolvedValue(null);
    render(<InstallFromFile />);
    choose();
    await waitFor(() => expect(openDialog).toHaveBeenCalled());
    expect(installModFile).not.toHaveBeenCalled();
    expect(currentModActions().busy.size).toBe(0);
  });

  it("says so when the file dialog itself fails", async () => {
    openDialog.mockRejectedValue(new Error("no portal"));
    render(<InstallFromFile />);
    choose();
    expect(await screen.findByText("PM couldn't open the file picker.")).toBeTruthy();
    expect(installModFile).not.toHaveBeenCalled();
  });

  it("shows the backend's words when the file is refused", async () => {
    openDialog.mockResolvedValue("/x/bad.pmmod");
    installModFile.mockRejectedValue({ code: "not-a-mod", message: "That file isn't a PM mod." });
    render(<InstallFromFile />);
    choose();
    expect(await screen.findByText("That file isn't a PM mod.")).toBeTruthy();
    // And the button is back for another try.
    expect(
      (screen.getByRole("button", { name: "Choose a mod file…" }) as HTMLButtonElement).disabled,
    ).toBe(false);
  });
});

describe("an unsigned file", () => {
  const unsignedRefusal = { code: "unsigned", message: "That file isn't signed." };

  it("with developer mode off: says so and where to switch it on, and goes no further", async () => {
    openDialog.mockResolvedValue("/x/raw.pmmod");
    installModFile.mockRejectedValue(unsignedRefusal);
    devModeState.mockResolvedValue({ on: false, problem: null });
    render(<InstallFromFile />);
    choose();
    expect(await screen.findByText(DEV_MODE_OFF_TEXT)).toBeTruthy();
    expect(DEV_MODE_OFF_TEXT).toContain("Settings, under Developer");
    expect(installUnsignedFile).not.toHaveBeenCalled();
    expect(currentModActions().screens.size).toBe(0);
  });

  it("with developer mode on: asks for the stark screen and stages it, still installing nothing", async () => {
    openDialog.mockResolvedValue("/x/raw.pmmod");
    installModFile.mockRejectedValue(unsignedRefusal);
    devModeState.mockResolvedValue({ on: true, problem: null });
    installUnsignedFile.mockResolvedValue(
      consentScreen({
        id: "acme.raw",
        tier: "third",
        kind: "fresh",
        unsigned: {
          confirmId: "acme.raw",
          reinstall: false,
          engineSealed: true,
          readOrInternet: null,
          testFolder: null,
        },
      }),
    );
    render(<InstallFromFile />);
    choose();
    await waitFor(() => expect(installUnsignedFile).toHaveBeenCalledWith("/x/raw.pmmod"));
    await waitFor(() => expect([...currentModActions().screens.keys()]).toEqual(["acme.raw"]));
    expect(installModConsented).not.toHaveBeenCalled();
  });
});

describe("in flight", () => {
  it("shows the read going on, can't pick a second file, and the result survives the view", async () => {
    openDialog.mockResolvedValue("/x/slow.pmmod");
    let finish!: (s: ReturnType<typeof consentScreen>) => void;
    installModFile.mockReturnValue(
      new Promise((res) => {
        finish = res;
      }),
    );
    const first = render(<InstallFromFile />);
    choose();
    const working = await screen.findByRole("button", { name: "Reading the file…" });
    expect((working as HTMLButtonElement).disabled).toBe(true);
    first.unmount();
    // The tab is switched away and back: still reading, not idle.
    render(<InstallFromFile />);
    expect(
      (screen.getByRole("button", { name: "Reading the file…" }) as HTMLButtonElement).disabled,
    ).toBe(true);
    await act(async () => finish(consentScreen({ id: "atlas.map", kind: "fresh" })));
    expect([...currentModActions().screens.keys()]).toEqual(["atlas.map"]);
    expect(screen.getByRole("button", { name: "Choose a mod file…" })).toBeTruthy();
  });

  it("a refusal that came while the tab was away is there on return", async () => {
    openDialog.mockResolvedValue("/x/raw.pmmod");
    let fail!: (e: unknown) => void;
    installModFile.mockReturnValue(
      new Promise((_, rej) => {
        fail = rej;
      }),
    );
    const first = render(<InstallFromFile />);
    choose();
    await screen.findByRole("button", { name: "Reading the file…" });
    first.unmount();
    await act(async () =>
      fail({ code: "file-unreadable", message: "PM couldn't read that file." }),
    );
    render(<InstallFromFile />);
    expect(screen.getByText("PM couldn't read that file.")).toBeTruthy();
  });
});
