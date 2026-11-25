// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The dialog a mod's `secrets.set` opens: PM's own window and field, naming the mod and showing where the
// key goes from the INSTALLED mod (not from the ask), one dialog at a time, none for a mod that is gone.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { resetModActions } from "../../lib/modActions";
import { ModRowsContext } from "../../lib/modRows";
import { closeKeyAsk, forgetModSurface, openKeyAsk } from "../../lib/modSurface";
import type { ModInfo, ModSecretSlot } from "../../lib/modsTypes";
import { mod } from "./modFixtures";
import type { ModSource } from "./modPageHooks";

const modSecretSave =
  vi.fn<(id: string, name: string, value: string) => Promise<ModSecretSlot[]>>();

vi.mock("../../lib/ipc", () => ({
  modSecretSave: (id: string, name: string, value: string) => modSecretSave(id, name, value),
  modSecretClear: vi.fn(),
}));
vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { ModKeyAsks } from "./ModKeyAsk";

const TYPED = "typed-not-a-real-key";
const slot = (extra: Partial<ModSecretSlot> = {}): ModSecretSlot => ({
  name: "weather",
  host: "api.example.com",
  header: "X-Api-Key",
  set: false,
  ...extra,
});
const row = (extra: Partial<ModInfo> = {}) =>
  mod({ id: "a.one", name: "Weather Mod", tier: "third", secrets: [slot()], ...extra });
const from: ModSource = { modId: "a.one", name: "Weather Mod", tier: "third" };
const ask = (name = "weather") => ({ name, host: "evil.example", header: "X-Other", set: false });

const refresh = vi.fn(() => Promise.resolve());
const draw = (rows: ModInfo[]) =>
  render(
    <ModRowsContext.Provider value={{ rows, loaded: true, problem: null, refresh }}>
      <ModKeyAsks />
    </ModRowsContext.Provider>,
  );

beforeEach(() => {
  vi.clearAllMocks();
  resetModActions();
  forgetModSurface();
});
afterEach(() => {
  cleanup();
  forgetModSurface();
});

describe("ModKeyAsks", () => {
  it("draws nothing until a mod asks", () => {
    draw([row()]);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("opens PM's dialog naming the mod, with its tier and the field", () => {
    draw([row()]);
    act(() => openKeyAsk(from, ask()));
    const dialog = screen.getByRole("dialog", { name: /A key for “Weather Mod”/ });
    expect(dialog.textContent).toContain("Third party");
    expect(screen.getByLabelText(/Key: weather/)).toBeTruthy();
  });

  it("shows where the key goes from the installed mod, not from what the ask carried", () => {
    draw([row()]);
    act(() => openKeyAsk(from, ask()));
    expect(screen.getByText(/only to api.example.com, in the X-Api-Key header/)).toBeTruthy();
    expect(document.body.textContent).not.toContain("evil.example");
  });

  it("saves the key through PM's backend and closes on Done", async () => {
    modSecretSave.mockResolvedValue([slot({ set: true })]);
    draw([row()]);
    act(() => openKeyAsk(from, ask()));
    fireEvent.change(screen.getByLabelText(/Key: weather/), { target: { value: TYPED } });
    fireEvent.click(screen.getByRole("button", { name: "Save the key weather" }));
    expect(modSecretSave).toHaveBeenCalledExactlyOnceWith("a.one", "weather", TYPED);
    await waitFor(() => expect(screen.getByText("A key is saved.")).toBeTruthy());
    expect(refresh).toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("closes without typing anything, and the mod can ask again later", () => {
    draw([row()]);
    act(() => openKeyAsk(from, ask()));
    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(modSecretSave).not.toHaveBeenCalled();
    act(() => openKeyAsk(from, ask()));
    expect(screen.getByRole("dialog")).toBeTruthy();
  });

  it("does not swap the slot under an open dialog", () => {
    const two = row({ secrets: [slot(), slot({ name: "maps", host: "maps.example.com" })] });
    draw([two]);
    act(() => openKeyAsk(from, ask("weather")));
    act(() => openKeyAsk(from, ask("maps")));
    expect(screen.getByLabelText(/Key: weather/)).toBeTruthy();
    expect(screen.queryByLabelText(/Key: maps/)).toBeNull();
  });

  it("draws no dialog for a mod that is not installed, or for a slot it does not declare", () => {
    draw([row()]);
    act(() => openKeyAsk({ ...from, modId: "gone.mod" }, ask()));
    act(() => openKeyAsk(from, ask("nothing")));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("moves to the next mod's dialog when one closes", () => {
    const other = mod({ id: "b.two", name: "Tides", secrets: [slot({ name: "tide" })] });
    draw([row(), other]);
    act(() => openKeyAsk(from, ask()));
    act(() => openKeyAsk({ modId: "b.two", name: "Tides", tier: "second" }, ask("tide")));
    expect(screen.getByRole("dialog", { name: /Weather Mod/ })).toBeTruthy();
    act(() => closeKeyAsk("a.one"));
    expect(screen.getByRole("dialog", { name: /Tides/ })).toBeTruthy();
  });
});
