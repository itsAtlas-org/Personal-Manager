// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The strips about the Pinboard's move into its mod (PR C, K8): PM draws them, from the backend's
// state (booleans and a word). Each says what happened to the person's board in words they can act
// on; the two answers to a conflict call PM's commands and nothing else; a refusal is shown, not
// swallowed; and with nothing owed there is nothing on screen.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ModsChanged, PinboardMoveState } from "../../lib/modsTypes";

const pinboardMoveState = vi.fn();
const pinboardUseOldBoard = vi.fn();
const pinboardMoveDismiss = vi.fn();
let changed: ((e: ModsChanged) => void) | null = null;

vi.mock("../../lib/ipc", () => ({
  pinboardMoveState: () => pinboardMoveState(),
  pinboardUseOldBoard: () => pinboardUseOldBoard(),
  pinboardMoveDismiss: (note: string) => pinboardMoveDismiss(note),
  onModsChanged: (h: (e: ModsChanged) => void) => {
    changed = h;
    return Promise.resolve(() => {});
  },
}));

vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { PinboardMoveStrips } from "./PinboardMoveStrips";

const NOTHING: PinboardMoveState = {
  conflict: false,
  repaired: false,
  restored: false,
  refused: null,
};

beforeEach(() => {
  vi.clearAllMocks();
  changed = null;
  pinboardMoveState.mockResolvedValue(NOTHING);
  pinboardUseOldBoard.mockResolvedValue(undefined);
  pinboardMoveDismiss.mockResolvedValue(undefined);
});
afterEach(cleanup);

describe("PinboardMoveStrips", () => {
  it("draws nothing for a store with no move owed, and asks the backend once", async () => {
    const { container } = render(<PinboardMoveStrips />);
    await waitFor(() => expect(pinboardMoveState).toHaveBeenCalledTimes(1));
    expect(container.firstChild).toBeNull();
  });

  it("draws nothing while the vault can't be read, and tries again on the next change", async () => {
    pinboardMoveState.mockRejectedValueOnce({ code: "no-store", message: "Locked." });
    const { container } = render(<PinboardMoveStrips />);
    await waitFor(() => expect(pinboardMoveState).toHaveBeenCalledTimes(1));
    expect(container.firstChild).toBeNull();
    pinboardMoveState.mockResolvedValue({ ...NOTHING, repaired: true });
    await waitFor(() => expect(changed).not.toBeNull());
    act(() => changed?.({ cause: "installed" } as ModsChanged));
    expect(await screen.findByText(/replacement mark/)).toBeTruthy();
  });

  describe("two boards", () => {
    beforeEach(() => pinboardMoveState.mockResolvedValue({ ...NOTHING, conflict: true }));

    it("says it in PM's words, and says what each button does to which board", async () => {
      render(<PinboardMoveStrips />);
      expect(
        await screen.findByText("You have a Pinboard in the mod and an older one in PM's store."),
      ).toBeTruthy();
      const text = document.body.textContent ?? "";
      // The loss warning is in view, never folded away.
      expect(text).toContain("first saves the mod's board as “board before move”");
      expect(text).toContain("Nothing is deleted either way");
      // The old board is a snapshot: the strip says a later rename does not reach it.
      expect(text).toContain("a project renamed or merged since then is not renamed on it");
      expect(screen.getByRole("button", { name: "Use my old board" })).toBeTruthy();
      expect(screen.getByRole("button", { name: "Keep this one" })).toBeTruthy();
      expect(text).not.toContain("—");
    });

    it("Use my old board asks PM to switch, then reads the state again and the strip goes", async () => {
      render(<PinboardMoveStrips />);
      const button = await screen.findByRole("button", { name: "Use my old board" });
      pinboardMoveState.mockResolvedValue(NOTHING);
      fireEvent.click(button);
      await waitFor(() => expect(pinboardUseOldBoard).toHaveBeenCalledTimes(1));
      expect(pinboardMoveDismiss).not.toHaveBeenCalled();
      await waitFor(() => expect(screen.queryByText(/older one in PM/)).toBeNull());
    });

    it("Keep this one clears only the conflict note", async () => {
      render(<PinboardMoveStrips />);
      const button = await screen.findByRole("button", { name: "Keep this one" });
      pinboardMoveState.mockResolvedValue(NOTHING);
      fireEvent.click(button);
      await waitFor(() => expect(pinboardMoveDismiss).toHaveBeenCalledWith("conflict"));
      expect(pinboardUseOldBoard).not.toHaveBeenCalled();
      await waitFor(() => expect(screen.queryByText(/older one in PM/)).toBeNull());
    });

    it("keeps the strip, and says why, when PM refuses to switch", async () => {
      pinboardUseOldBoard.mockRejectedValueOnce({
        code: "refused",
        message: "The old board is bigger than a mod can save, so nothing was changed.",
      });
      render(<PinboardMoveStrips />);
      fireEvent.click(await screen.findByRole("button", { name: "Use my old board" }));
      expect(await screen.findByText(/bigger than a mod can save/)).toBeTruthy();
      expect(screen.getByRole("button", { name: "Keep this one" })).toBeTruthy();
    });

    it("can't be answered twice at once", async () => {
      let finish: () => void = () => {};
      pinboardUseOldBoard.mockReturnValueOnce(new Promise<void>((r) => (finish = r)));
      render(<PinboardMoveStrips />);
      fireEvent.click(await screen.findByRole("button", { name: "Use my old board" }));
      await waitFor(() =>
        expect(
          (screen.getByRole("button", { name: "Keep this one" }) as HTMLButtonElement).disabled,
        ).toBe(true),
      );
      fireEvent.click(screen.getByRole("button", { name: "Keep this one" }));
      expect(pinboardMoveDismiss).not.toHaveBeenCalled();
      finish();
      await waitFor(() => expect(pinboardMoveState.mock.calls.length).toBeGreaterThan(1));
    });
  });

  it("tells a repaired copy once, with the old board unchanged, and OK clears it", async () => {
    pinboardMoveState.mockResolvedValue({ ...NOTHING, repaired: true });
    render(<PinboardMoveStrips />);
    expect(await screen.findByText(/replaced with a plain replacement mark/)).toBeTruthy();
    expect(document.body.textContent).toContain("Your old board in PM's store is unchanged.");
    pinboardMoveState.mockResolvedValue(NOTHING);
    fireEvent.click(screen.getByRole("button", { name: "OK" }));
    await waitFor(() => expect(pinboardMoveDismiss).toHaveBeenCalledWith("repaired"));
    await waitFor(() => expect(screen.queryByText(/replacement mark/)).toBeNull());
  });

  it("tells a board that can't move that it is safe, with nothing to press", async () => {
    pinboardMoveState.mockResolvedValue({ ...NOTHING, refused: "unreadable" });
    const { unmount } = render(<PinboardMoveStrips />);
    expect(await screen.findByText(/couldn't be read as it is/)).toBeTruthy();
    expect(document.body.textContent).toContain(
      "Your board is safe in PM's store, and nothing was changed.",
    );
    expect(screen.queryByRole("button")).toBeNull();
    unmount();
    pinboardMoveState.mockResolvedValue({ ...NOTHING, refused: "tooBig" });
    render(<PinboardMoveStrips />);
    expect(await screen.findByText(/bigger than a mod can save \(1 MiB\)/)).toBeTruthy();
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("draws every strip that is owed, refusal first", async () => {
    pinboardMoveState.mockResolvedValue({
      conflict: true,
      repaired: true,
      restored: true,
      refused: "tooBig",
    });
    render(<PinboardMoveStrips />);
    await screen.findByText(/older one in PM/);
    const text = document.body.textContent ?? "";
    expect(text.indexOf("bigger than a mod")).toBeLessThan(text.indexOf("older one in PM"));
    expect(text.indexOf("older one in PM")).toBeLessThan(text.indexOf("replacement mark"));
    // The restored strip never draws beside a conflict, whatever the flag says.
    expect(text).not.toContain("PM restored your Pinboard");
  });

  it("tells a restored board once, and OK clears it and reads the state again", async () => {
    pinboardMoveState.mockResolvedValue({ ...NOTHING, restored: true });
    render(<PinboardMoveStrips />);
    expect(
      await screen.findByText(
        /PM restored your Pinboard as it was when it moved into a mod\. Backups without mod data don't hold later edits\./,
      ),
    ).toBeTruthy();
    pinboardMoveState.mockResolvedValue(NOTHING);
    fireEvent.click(screen.getByRole("button", { name: "OK" }));
    await waitFor(() => expect(pinboardMoveDismiss).toHaveBeenCalledWith("restored"));
    await waitFor(() => expect(screen.queryByText(/PM restored your Pinboard/)).toBeNull());
  });

  it("does not draw the restored strip while a conflict is open", async () => {
    pinboardMoveState.mockResolvedValue({ ...NOTHING, restored: true, conflict: true });
    render(<PinboardMoveStrips />);
    await screen.findByText(/older one in PM/);
    expect(screen.queryByText(/PM restored your Pinboard/)).toBeNull();
  });
});
