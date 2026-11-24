// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Test keys" in Settings → Developer (Mods spec §1.15): the keys of the loaded test mods only, in the
// same field the Installed row uses.

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { resetModActions } from "../../lib/modActions";
import { ModRowsContext } from "../../lib/modRows";
import type { ModInfo, ModSecretSlot } from "../../lib/modsTypes";
import { mod, RLO } from "../mods/modFixtures";

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

import { TestKeys } from "./TestKeys";

const slot = (extra: Partial<ModSecretSlot> = {}): ModSecretSlot => ({
  name: "weather",
  host: "api.example.com",
  header: "X-Api-Key",
  set: false,
  ...extra,
});
const refresh = vi.fn(() => Promise.resolve());
const draw = (rows: ModInfo[], loaded = true) =>
  render(
    <ModRowsContext.Provider value={{ rows, loaded, problem: null, refresh }}>
      <TestKeys />
    </ModRowsContext.Provider>,
  );

beforeEach(() => {
  vi.clearAllMocks();
  resetModActions();
});
afterEach(cleanup);

describe("TestKeys", () => {
  it("lists only the test mods loaded from a folder", () => {
    draw([
      mod({ id: "test.one", name: "Test One", installedFrom: "folder", secrets: [slot()] }),
      mod({ id: "real.one", name: "Real One", installedFrom: "catalogue", secrets: [slot()] }),
      mod({ id: "file.one", name: "File One", installedFrom: "file", secrets: [slot()] }),
    ]);
    const list = screen.getByRole("list", { name: "Test mod keys" });
    expect(within(list).getAllByRole("listitem")).toHaveLength(1);
    expect(list.textContent).toContain("Test One");
    expect(document.body.textContent).not.toContain("Real One");
    expect(document.body.textContent).not.toContain("File One");
  });

  it("says so when no test mod is loaded, and while the rows are unread", () => {
    const { unmount } = draw([mod({ installedFrom: "catalogue", secrets: [slot()] })]);
    expect(screen.getByText("No test mods are loaded.")).toBeTruthy();
    unmount();
    draw([], false);
    expect(screen.getByText("Reading…")).toBeTruthy();
    expect(screen.queryByText("No test mods are loaded.")).toBeNull();
  });

  it("says a test mod with no key slots declares none", () => {
    draw([mod({ id: "test.one", installedFrom: "folder", secrets: [] })]);
    expect(screen.getByText("It doesn't declare any keys.")).toBeTruthy();
  });

  it("types a key into the shared field, which saves it through the same command", async () => {
    modSecretSave.mockResolvedValue([slot({ set: true })]);
    draw([mod({ id: "test.one", installedFrom: "folder", secrets: [slot()] })]);
    const field = screen.getByLabelText(/Key: weather/) as HTMLInputElement;
    expect(field.type).toBe("password");
    expect(screen.getByText(/only to api.example.com, in the X-Api-Key header/)).toBeTruthy();
    fireEvent.change(field, { target: { value: "typed-not-a-real-key" } });
    fireEvent.click(screen.getByRole("button", { name: "Save the key weather" }));
    expect(modSecretSave).toHaveBeenCalledExactlyOnceWith(
      "test.one",
      "weather",
      "typed-not-a-real-key",
    );
    expect(field.value).toBe("");
    await waitFor(() => expect(refresh).toHaveBeenCalled());
  });

  it("draws the test mod's own words as one plain line", () => {
    draw([mod({ id: "test.one", name: `Te${RLO}st`, installedFrom: "folder", secrets: [slot()] })]);
    expect(document.body.textContent).not.toContain(RLO);
  });
});
