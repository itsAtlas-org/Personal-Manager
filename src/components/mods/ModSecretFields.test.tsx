// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// PM's own key field (Mods spec §1.7, §1.15): masked, says where the key goes, Replace and Remove, and
// the typed key leaves the page when it is sent, whether the save worked or not. Everything the mod
// supplied (the slot's name, host, header) is drawn as one plain line.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { resetModActions } from "../../lib/modActions";
import type { ModSecretSlot } from "../../lib/modsTypes";
import { RLO } from "./modFixtures";

const modSecretSave =
  vi.fn<(id: string, name: string, value: string) => Promise<ModSecretSlot[]>>();
const modSecretClear = vi.fn<(id: string, name: string) => Promise<void>>();

vi.mock("../../lib/ipc", () => ({
  modSecretSave: (id: string, name: string, value: string) => modSecretSave(id, name, value),
  modSecretClear: (id: string, name: string) => modSecretClear(id, name),
}));
vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { ModSecretFields } from "./ModSecretFields";

const slot = (extra: Partial<ModSecretSlot> = {}): ModSecretSlot => ({
  name: "weather",
  host: "api.example.com",
  header: "X-Api-Key",
  set: false,
  ...extra,
});

// A made-up value that is not a key for anything.
const TYPED = "typed-not-a-real-key";

const input = () => screen.getByLabelText(/Key: weather/) as HTMLInputElement;
const type = (value: string) => fireEvent.change(input(), { target: { value } });

beforeEach(() => {
  vi.clearAllMocks();
  resetModActions();
});
afterEach(cleanup);

describe("ModSecretFields", () => {
  it("masks the key and says where PM sends it", () => {
    render(<ModSecretFields modId="acme.tool" slots={[slot()]} />);
    expect(input().type).toBe("password");
    expect(input().autocomplete).toBe("off");
    expect(
      screen.getByText(
        "PM sends it only to api.example.com, in the X-Api-Key header. The mod never sees it.",
      ),
    ).toBeTruthy();
    expect(screen.getByText("No key is saved.")).toBeTruthy();
  });

  it("offers Save, not Replace or Remove, while no key is stored", () => {
    render(<ModSecretFields modId="acme.tool" slots={[slot()]} />);
    expect(screen.getByRole("button", { name: "Save the key weather" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Replace the key/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /Remove the key/ })).toBeNull();
  });

  it("offers Replace and Remove while a key is stored", () => {
    render(<ModSecretFields modId="acme.tool" slots={[slot({ set: true })]} />);
    expect(screen.getByText("A key is saved.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Replace the key weather" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Remove the key weather" })).toBeTruthy();
  });

  it("cannot save an empty or blank key (the backend reads an empty key as 'delete it')", () => {
    render(<ModSecretFields modId="acme.tool" slots={[slot()]} />);
    const save = screen.getByRole("button", { name: "Save the key weather" }) as HTMLButtonElement;
    expect(save.disabled).toBe(true);
    type("   ");
    expect(save.disabled).toBe(true);
    type(TYPED);
    expect(save.disabled).toBe(false);
  });

  it("sends the key to the backend once, and clears the field when it works", async () => {
    modSecretSave.mockResolvedValue([slot({ set: true })]);
    const onChanged = vi.fn();
    render(<ModSecretFields modId="acme.tool" slots={[slot()]} onChanged={onChanged} />);
    type(TYPED);
    fireEvent.click(screen.getByRole("button", { name: "Save the key weather" }));
    expect(modSecretSave).toHaveBeenCalledExactlyOnceWith("acme.tool", "weather", TYPED);
    // Gone from the page at once, not when the answer comes.
    expect(input().value).toBe("");
    await waitFor(() => expect(screen.getByText("A key is saved.")).toBeTruthy());
    expect(input().value).toBe("");
    expect(onChanged).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "Replace the key weather" })).toBeTruthy();
  });

  it("clears the field when the save fails, and the sentence never carries the key", async () => {
    modSecretSave.mockRejectedValue({
      code: "bad-request",
      message: "PM can't send that key in a header.",
    });
    const onChanged = vi.fn();
    const { container } = render(
      <ModSecretFields modId="acme.tool" slots={[slot()]} onChanged={onChanged} />,
    );
    type(TYPED);
    fireEvent.click(screen.getByRole("button", { name: "Save the key weather" }));
    await waitFor(() => expect(screen.getByRole("alert").textContent).toMatch(/PM can't send/));
    expect(input().value).toBe("");
    expect(container.textContent).not.toContain(TYPED);
    expect(onChanged).not.toHaveBeenCalled();
    // Still not stored, so still a Save.
    expect(screen.getByText("No key is saved.")).toBeTruthy();
  });

  it("draws only PM's fallback for an error that is not PM's refusal, even one quoting the key", async () => {
    // Something below PM's backend (the IPC layer) failed and put what it was sent in its message.
    modSecretSave.mockRejectedValue(new Error(`invalid args: value "${TYPED}"`));
    const { container } = render(<ModSecretFields modId="acme.tool" slots={[slot()]} />);
    type(TYPED);
    fireEvent.click(screen.getByRole("button", { name: "Save the key weather" }));
    await waitFor(() => expect(screen.getByRole("alert")).toBeTruthy());
    expect(input().value).toBe("");
    expect(screen.getByRole("alert").textContent).toBe("PM couldn't save that key.");
    expect(container.textContent).not.toContain(TYPED);
  });

  it("does the same for a remove", async () => {
    modSecretClear.mockRejectedValue(new Error("boom"));
    render(<ModSecretFields modId="acme.tool" slots={[slot({ set: true })]} />);
    fireEvent.click(screen.getByRole("button", { name: "Remove the key weather" }));
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toBe("PM couldn't remove that key."),
    );
  });

  it("holds the field and both buttons while the save is on its way", async () => {
    let settle: (s: ModSecretSlot[]) => void = () => {};
    modSecretSave.mockReturnValue(new Promise((r) => (settle = r)));
    render(<ModSecretFields modId="acme.tool" slots={[slot({ set: true })]} />);
    type(TYPED);
    fireEvent.click(screen.getByRole("button", { name: "Replace the key weather" }));
    expect(input().disabled).toBe(true);
    expect(
      (screen.getByRole("button", { name: "Remove the key weather" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
    expect(screen.getByText("Saving…")).toBeTruthy();
    await act(async () => settle([slot({ set: true })]));
    expect(input().disabled).toBe(false);
  });

  it("removes a key without a typed value, and shows it as not saved", async () => {
    modSecretClear.mockResolvedValue(undefined);
    const onChanged = vi.fn();
    render(
      <ModSecretFields modId="acme.tool" slots={[slot({ set: true })]} onChanged={onChanged} />,
    );
    // Something half-typed goes with it.
    type(TYPED);
    fireEvent.click(screen.getByRole("button", { name: "Remove the key weather" }));
    expect(input().value).toBe("");
    await waitFor(() => expect(screen.getByText("No key is saved.")).toBeTruthy());
    expect(modSecretClear).toHaveBeenCalledExactlyOnceWith("acme.tool", "weather");
    expect(modSecretSave).not.toHaveBeenCalled();
    expect(onChanged).toHaveBeenCalledTimes(1);
  });

  it("says why a remove did not work and leaves the key shown as saved", async () => {
    modSecretClear.mockRejectedValue({
      code: "not-installed",
      message: "That mod isn't installed.",
    });
    render(<ModSecretFields modId="acme.tool" slots={[slot({ set: true })]} />);
    fireEvent.click(screen.getByRole("button", { name: "Remove the key weather" }));
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toBe("That mod isn't installed."),
    );
    expect(screen.getByText("A key is saved.")).toBeTruthy();
  });

  it("follows the rows when they are read again (a newer list wins over the last answer)", async () => {
    modSecretSave.mockResolvedValue([slot({ set: true })]);
    const { rerender } = render(<ModSecretFields modId="acme.tool" slots={[slot()]} />);
    type(TYPED);
    fireEvent.click(screen.getByRole("button", { name: "Save the key weather" }));
    await waitFor(() => expect(screen.getByText("A key is saved.")).toBeTruthy());
    // The mod's update moved the slot: the rows say there is no key for the new host.
    rerender(<ModSecretFields modId="acme.tool" slots={[slot({ host: "api.other.example" })]} />);
    expect(screen.getByText("No key is saved.")).toBeTruthy();
    expect(screen.getByText(/only to api.other.example/)).toBeTruthy();
  });

  it("draws one field per slot, each saved on its own", async () => {
    modSecretSave.mockResolvedValue([]);
    render(
      <ModSecretFields
        modId="acme.tool"
        slots={[slot(), slot({ name: "maps", host: "maps.example.com", header: "Authorization" })]}
      />,
    );
    fireEvent.change(screen.getByLabelText(/Key: maps/), { target: { value: TYPED } });
    fireEvent.click(screen.getByRole("button", { name: "Save the key maps" }));
    expect(modSecretSave).toHaveBeenCalledExactlyOnceWith("acme.tool", "maps", TYPED);
    // The other field was untouched.
    expect((screen.getByLabelText(/Key: weather/) as HTMLInputElement).value).toBe("");
    await waitFor(() => expect(screen.getAllByText("No key is saved.")).toHaveLength(2));
  });

  it("draws what the mod supplied as one plain line", () => {
    render(
      <ModSecretFields
        modId="acme.tool"
        slots={[
          slot({ name: `we${RLO}ather`, host: `api.example.com${RLO}`, header: "X-Api-Key" }),
        ]}
      />,
    );
    expect(document.body.textContent).not.toContain(RLO);
  });

  it("is held while another change to the mod is running", () => {
    render(<ModSecretFields modId="acme.tool" slots={[slot()]} disabled />);
    expect(input().disabled).toBe(true);
  });
});
