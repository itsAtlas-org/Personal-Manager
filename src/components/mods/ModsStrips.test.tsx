// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The safety list's strips at the top of the Mods tab (spec §1.10): paused, back on, removed, an
// update waiting, "not checked since …", and a damaged or unreadable record with a reset that asks
// first. Drawn from the backend's status, which is read on mount (the cue may have come before this
// view existed) and again on every cue.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { resetModActions, useModActions } from "../../lib/modActions";
import type { ModNotice, ModsChanged, ModsSafetyStatus } from "../../lib/modsTypes";
import { RLO, safety } from "./modFixtures";

const modsSafetyStatus = vi.fn();
const dismissModResumed = vi.fn();
const resetSafetyRecord = vi.fn();
let notice: ((n: ModNotice) => void) | null = null;
let changed: ((e: ModsChanged) => void) | null = null;

vi.mock("../../lib/ipc", () => ({
  modsSafetyStatus: () => modsSafetyStatus(),
  dismissModResumed: (id: string) => dismissModResumed(id),
  resetSafetyRecord: () => resetSafetyRecord(),
  onModsNotice: (h: (n: ModNotice) => void) => {
    notice = h;
    return Promise.resolve(() => {});
  },
  onModsChanged: (h: (e: ModsChanged) => void) => {
    changed = h;
    return Promise.resolve(() => {});
  },
}));
vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { ModsStrips } from "./ModsStrips";

/** The notes the actions left (they are drawn by ModsView, so a test reads them from the store). */
function Notes() {
  return (
    <ul aria-label="notes">
      {useModActions().notes.map((n) => (
        <li key={n.id}>{n.text}</li>
      ))}
    </ul>
  );
}

const draw = () =>
  render(
    <>
      <ModsStrips />
      <Notes />
    </>,
  );

const withNotices = (notices: ModNotice[], extra: Partial<ModsSafetyStatus> = {}) =>
  safety({ notices, ...extra });

beforeEach(() => {
  vi.clearAllMocks();
  resetModActions();
  notice = null;
  changed = null;
});
afterEach(cleanup);

describe("the strips", () => {
  it("draws nothing when there is nothing to say", async () => {
    modsSafetyStatus.mockResolvedValue(safety());
    const { container } = draw();
    await waitFor(() => expect(modsSafetyStatus).toHaveBeenCalled());
    expect(container.querySelector("[data-help='mods-safety']")).toBeNull();
  });

  it("reads the status on mount: a notice that fired before this view existed is still drawn", async () => {
    modsSafetyStatus.mockResolvedValue(
      withNotices([
        {
          kind: "paused",
          id: "a.b",
          name: "Acme",
          version: "1.0.0",
          reason: "It leaks.",
          fixedIn: null,
        },
      ]),
    );
    draw();
    expect(
      await screen.findByText(
        "Acme is paused while we fix a problem: It leaks. Your data is safe. It will turn back on by itself when the fix arrives.",
      ),
    ).toBeTruthy();
  });

  it("paused with a fix on the way says which version", async () => {
    modsSafetyStatus.mockResolvedValue(
      withNotices([
        {
          kind: "paused",
          id: "a.b",
          name: "Acme",
          version: "1.0.0",
          reason: "Bad",
          fixedIn: "1.0.1",
        },
      ]),
    );
    draw();
    expect(
      await screen.findByText(/It will turn back on by itself when version 1\.0\.1 arrives\./),
    ).toBeTruthy();
  });

  it("paused with no reason on the list still says it, without a dangling colon", async () => {
    modsSafetyStatus.mockResolvedValue(
      withNotices([
        { kind: "paused", id: "a.b", name: "Acme", version: "1", reason: "", fixedIn: null },
      ]),
    );
    draw();
    expect(
      await screen.findByText(/^Acme is paused while we fix a problem\. Your data is safe\./),
    ).toBeTruthy();
  });

  it("removed: says it is off and its files are gone, and points at Uninstall for what is left", async () => {
    modsSafetyStatus.mockResolvedValue(
      withNotices([
        { kind: "removed", id: "a.b", name: "Acme", version: "1", reason: "Steals notes." },
      ]),
    );
    draw();
    const text = (await screen.findByText(/Acme was removed by PM's safety list: Steals notes\./))
      .textContent!;
    expect(text).toContain("switched off and its files are gone");
    expect(text).toContain("What it saved and what it made are still here");
    expect(text).toContain("Uninstall");
  });

  it("an update waiting, and one that needs a newer PM", async () => {
    modsSafetyStatus.mockResolvedValue(
      withNotices([
        { kind: "updateWaiting", id: "a.b", name: "Acme", version: "2.0.0" },
        { kind: "updateNeedsNewerPm", id: "c.d", name: "Beta", version: "3.0.0", needsApi: "2.0" },
      ]),
    );
    draw();
    expect(
      await screen.findByText(
        /Acme has an update, version 2\.0\.0, that asks for more than you allowed\./,
      ),
    ).toBeTruthy();
    expect(
      screen.getByText(
        "Beta has a newer version, 3.0.0, that needs a newer PM. Update PM to get it.",
      ),
    ).toBeTruthy();
  });

  it("names and reasons are text, never markup", async () => {
    const evil = '<img src=x onerror="alert(1)"><a href="https://evil.example">x</a>';
    modsSafetyStatus.mockResolvedValue(
      withNotices([{ kind: "removed", id: "a.b", name: evil, version: "1", reason: evil }]),
    );
    const { container } = draw();
    await screen.findByText(/was removed by PM's safety list/);
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("a")).toBeNull();
  });

  it("takes direction-changing characters and line breaks out of a name and a reason", async () => {
    modsSafetyStatus.mockResolvedValue(
      withNotices([
        {
          kind: "paused",
          id: "a.b",
          name: `Ac${RLO}me`,
          version: "1",
          reason: "bad\nPM says it is fine.",
          fixedIn: null,
        },
      ]),
    );
    const { container } = draw();
    await screen.findByText(/is paused while we fix a problem/);
    expect(container.textContent).toContain("Ac me is paused while we fix a problem: bad PM says");
    expect(container.textContent).not.toContain(RLO);
  });

  it("reads again on the notice cue and on a change", async () => {
    modsSafetyStatus.mockResolvedValue(safety());
    draw();
    await waitFor(() => expect(modsSafetyStatus).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(notice).not.toBeNull());
    modsSafetyStatus.mockResolvedValue(
      withNotices([{ kind: "updateWaiting", id: "a.b", name: "Acme", version: "2" }]),
    );
    await act(async () =>
      notice?.({ kind: "updateWaiting", id: "a.b", name: "Acme", version: "2" }),
    );
    expect(await screen.findByText(/Acme has an update/)).toBeTruthy();
    modsSafetyStatus.mockResolvedValue(safety());
    await act(async () => changed?.({ cause: "updated" }));
    await waitFor(() => expect(screen.queryByText(/Acme has an update/)).toBeNull());
  });

  it("says when it can't read the status, and offers Try again", async () => {
    modsSafetyStatus.mockRejectedValueOnce({ code: "locked", message: "PM is locked." });
    modsSafetyStatus.mockResolvedValue(safety());
    draw();
    expect(await screen.findByText(/PM is locked\./)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(screen.queryByText(/PM is locked\./)).toBeNull());
  });
});

describe("back on", () => {
  const resumed: ModNotice = { kind: "resumed", id: "a.b", name: "Acme", version: "1.0.1" };

  it("says it, and Dismiss tells the backend and reads again", async () => {
    modsSafetyStatus.mockResolvedValueOnce(withNotices([resumed])).mockResolvedValue(safety());
    dismissModResumed.mockResolvedValue(undefined);
    draw();
    expect(await screen.findByText("Acme is back on, fixed in version 1.0.1.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    await waitFor(() => expect(dismissModResumed).toHaveBeenCalledWith("a.b"));
    await waitFor(() => expect(screen.queryByText(/is back on/)).toBeNull());
  });

  it("keeps the strip and says why when it can't be dismissed", async () => {
    modsSafetyStatus.mockResolvedValue(withNotices([resumed]));
    dismissModResumed.mockRejectedValue({ code: "locked", message: "PM is locked." });
    draw();
    fireEvent.click(await screen.findByRole("button", { name: "Dismiss" }));
    expect(await screen.findByText("PM is locked.")).toBeTruthy();
    expect(screen.getByText(/is back on/)).toBeTruthy();
  });
});

describe("how fresh the list is", () => {
  it("says 'not checked since <date>' when the backend does", async () => {
    modsSafetyStatus.mockResolvedValue(
      safety({
        checked: {
          kind: "stale",
          at: "2026-10-01T00:00:00Z",
          text: "Safety list not checked since 01-10-2026.",
        },
      }),
    );
    draw();
    expect(await screen.findByText("Safety list not checked since 01-10-2026.")).toBeTruthy();
  });

  it("a list never checked is said quietly, and a fresh one says nothing", async () => {
    modsSafetyStatus.mockResolvedValue(
      safety({
        checked: { kind: "never", at: null, text: "PM hasn't checked the safety list yet." },
      }),
    );
    draw();
    expect(await screen.findByText("PM hasn't checked the safety list yet.")).toBeTruthy();
    cleanup();
    modsSafetyStatus.mockResolvedValue(safety());
    draw();
    await waitFor(() => expect(modsSafetyStatus).toHaveBeenCalledTimes(2));
    expect(screen.queryByText(/checked/)).toBeNull();
  });
});

describe("a damaged or unreadable record", () => {
  const damaged = safety({
    record: { kind: "damaged", text: "PM's record of the safety list is damaged." },
  });

  it("damaged: says so and offers the reset, which asks first and says what it forgets", async () => {
    modsSafetyStatus.mockResolvedValue(damaged);
    draw();
    expect(await screen.findByText("PM's record of the safety list is damaged.")).toBeTruthy();
    // Pressing it does not reset anything yet.
    fireEvent.click(screen.getByRole("button", { name: "Reset the safety list" }));
    expect(resetSafetyRecord).not.toHaveBeenCalled();
    const dialog = screen.getByRole("dialog", { name: "Reset the safety list?" });
    expect(dialog.textContent).toContain(
      "forgets which list was newest and which keys were cancelled",
    );
    expect(dialog.textContent).toContain("Your mods and their data are not touched");
    // Cancel leaves it as it was.
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(resetSafetyRecord).not.toHaveBeenCalled();
  });

  it("confirming resets, reads the status again, and says what happened", async () => {
    modsSafetyStatus.mockResolvedValueOnce(damaged).mockResolvedValue(safety());
    resetSafetyRecord.mockResolvedValue({ reset: true, fetched: true, problem: null });
    draw();
    fireEvent.click(await screen.findByRole("button", { name: "Reset the safety list" }));
    const dialog = screen.getByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Reset the safety list" }));
    await waitFor(() => expect(resetSafetyRecord).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(
      screen.getByText("PM cleared its record of the safety list and downloaded the list again."),
    ).toBeTruthy();
    expect(screen.queryByText("PM's record of the safety list is damaged.")).toBeNull();
  });

  it("says when the list couldn't be downloaded straight after", async () => {
    modsSafetyStatus.mockResolvedValue(safety());
    modsSafetyStatus.mockResolvedValueOnce(damaged);
    resetSafetyRecord.mockResolvedValue({ reset: true, fetched: false, problem: "PM is offline." });
    draw();
    fireEvent.click(await screen.findByRole("button", { name: "Reset the safety list" }));
    fireEvent.click(
      within(screen.getByRole("dialog")).getByRole("button", { name: "Reset the safety list" }),
    );
    expect(
      await screen.findByText(
        "PM cleared its record of the safety list but couldn't download the list yet: PM is offline. It will try again.",
      ),
    ).toBeTruthy();
  });

  it("says the backend's refusal in the dialog and keeps it open (the record wasn't really damaged)", async () => {
    modsSafetyStatus.mockResolvedValue(damaged);
    resetSafetyRecord.mockRejectedValue({
      code: "not-allowed",
      message: "The record is fine, so PM won't reset it.",
    });
    draw();
    fireEvent.click(await screen.findByRole("button", { name: "Reset the safety list" }));
    fireEvent.click(
      within(screen.getByRole("dialog")).getByRole("button", { name: "Reset the safety list" }),
    );
    expect(await screen.findByText("The record is fine, so PM won't reset it.")).toBeTruthy();
    expect(screen.getByRole("dialog")).toBeTruthy();
  });

  it("the dialog can't be closed or pressed again while the reset runs", async () => {
    modsSafetyStatus.mockResolvedValue(damaged);
    let finish!: (r: unknown) => void;
    resetSafetyRecord.mockReturnValue(
      new Promise((res) => {
        finish = res;
      }),
    );
    draw();
    fireEvent.click(await screen.findByRole("button", { name: "Reset the safety list" }));
    const dialog = screen.getByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Reset the safety list" }));
    const working = await within(dialog).findByRole("button", { name: "Working…" });
    expect((working as HTMLButtonElement).disabled).toBe(true);
    expect(
      (within(dialog).getByRole("button", { name: "Cancel" }) as HTMLButtonElement).disabled,
    ).toBe(true);
    await act(async () => finish({ reset: true, fetched: true, problem: null }));
    expect(resetSafetyRecord).toHaveBeenCalledTimes(1);
  });

  it("unreadable: says PM will try again, and still offers the reset behind the same confirm", async () => {
    modsSafetyStatus.mockResolvedValue(
      safety({
        record: { kind: "unreadable", text: "PM couldn't read its record of the safety list." },
      }),
    );
    draw();
    expect(await screen.findByText("PM couldn't read its record of the safety list.")).toBeTruthy();
    expect(screen.getByText(/PM will try to read it again by itself/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Reset the safety list" }));
    // Nothing is reset by the first press, and the confirm says it may be a passing problem.
    expect(resetSafetyRecord).not.toHaveBeenCalled();
    const dialog = screen.getByRole("dialog", { name: "Reset the safety list?" });
    expect(dialog.textContent).toContain("Yours can't be read right now");
    expect(dialog.textContent).toContain(
      "forgets which list was newest and which keys were cancelled",
    );
  });

  it("a damaged record's strip doesn't say PM will try again by itself", async () => {
    modsSafetyStatus.mockResolvedValue(damaged);
    draw();
    await screen.findByText("PM's record of the safety list is damaged.");
    expect(screen.queryByText(/PM will try to read it again/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Reset the safety list" }));
    expect(screen.getByRole("dialog").textContent).toContain("Yours is damaged");
  });

  it("a fault that mends itself closes the confirm, and the next fault doesn't open it by itself", async () => {
    modsSafetyStatus.mockResolvedValue(damaged);
    draw();
    fireEvent.click(await screen.findByRole("button", { name: "Reset the safety list" }));
    expect(screen.getByRole("dialog")).toBeTruthy();
    // The record mends (a cue re-reads the status and finds nothing wrong).
    modsSafetyStatus.mockResolvedValue(safety());
    await act(async () => changed?.({ cause: "safety" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    // A fault again, later: the strip is back, the dialog is not.
    modsSafetyStatus.mockResolvedValue(damaged);
    await act(async () => changed?.({ cause: "safety" }));
    expect(await screen.findByText("PM's record of the safety list is damaged.")).toBeTruthy();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(resetSafetyRecord).not.toHaveBeenCalled();
  });

  it("a fault only the cue carried is drawn the same way", async () => {
    modsSafetyStatus.mockResolvedValue(
      withNotices([{ kind: "recordFault", unreadable: false, text: "The record is damaged." }]),
    );
    draw();
    expect(await screen.findByText("The record is damaged.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Reset the safety list" })).toBeTruthy();
  });
});
