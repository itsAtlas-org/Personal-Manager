// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Don't show this again" on a mod's first-open screen (D17). Bobby, 2026-11-17: an optional tick
// box, shown only from the SECOND showing of the full Mods disclaimer, so the notice is seen at least
// twice before it can be turned off; it takes effect when the person presses Turn on; later screens
// fold the notice to one line that opens on a click. The permissions and Turn on are never folded.
// The count is per vault on this device (lib/modsDisclaimerShown); every failure shows MORE notice.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { StrictMode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { HELP } from "../../lib/help";
import { resetModActions } from "../../lib/modActions";
import { readShown } from "../../lib/modsDisclaimerShown";
import { mod, perm, standing } from "./modFixtures";

const vaultStatus = vi.fn();
const consentMod = vi.fn();

vi.mock("../../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  vaultStatus: () => vaultStatus(),
  consentMod: (id: string, answers: Record<string, boolean>) => consentMod(id, answers),
  modsDisclaimerState: () => Promise.resolve({ accepted: false, problem: null }),
  onModsChanged: () => Promise.resolve(() => {}),
}));
vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { ModFirstOpen } from "./ModFirstOpen";
import { ModsDisclaimerGate } from "./ModsDisclaimerGate";

const A = "pm.mods.disclaimerShown.vault-a";
const B = "pm.mods.disclaimerShown.vault-b";
const BOX = { name: "Don't show this again" };

const waiting = mod({
  consented: false,
  state: standing("waiting-for-you", { fix: "turn-on" }),
  permissions: [perm("documents.meta"), perm("storage", { when: "auto", access: "allowed" })],
});

const onTurnedOn = vi.fn(() => Promise.resolve());

function open(strict = false) {
  const screenOf = <ModFirstOpen mod={waiting} onTurnedOn={onTurnedOn} />;
  return render(strict ? <StrictMode>{screenOf}</StrictMode> : screenOf);
}

/** Open the screen and let its showing be counted and drawn. */
async function opened(n: number, vault = A) {
  const view = open();
  await waitFor(() => expect(readShown(vault).shown).toBe(n));
  await act(async () => {});
  return view;
}

async function turnOn() {
  fireEvent.click(screen.getByRole("button", { name: "Turn on" }));
  await waitFor(() => expect(onTurnedOn).toHaveBeenCalled());
  onTurnedOn.mockClear();
}

const folded = () => screen.queryByRole("button", { name: "About mods" });
const fullNotice = () => screen.queryByRole("heading", { name: "About mods" });

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  resetModActions();
  vaultStatus.mockResolvedValue({ vault_id: "vault-a" });
  consentMod.mockResolvedValue(undefined);
});
afterEach(cleanup);

describe("the first showing", () => {
  it("shows the whole notice and no tick box", async () => {
    await opened(1);
    expect(fullNotice()).toBeTruthy();
    expect(screen.getByText(/Mods are small add-ons made by people/)).toBeTruthy();
    expect(screen.queryByRole("checkbox", BOX)).toBeNull();
    expect(folded()).toBeNull();
  });

  it("is not changed by pressing Turn on: the next showing is the second", async () => {
    await opened(1);
    await turnOn();
    expect(readShown(A)).toEqual({ shown: 1, hide: false });
  });
});

describe("the second showing and after", () => {
  it("has the tick box, unticked, under the whole notice", async () => {
    (await opened(1)).unmount();
    await opened(2);
    const box = screen.getByRole("checkbox", BOX) as HTMLInputElement;
    expect(box.checked).toBe(false);
    expect(fullNotice()).toBeTruthy();
    expect(screen.getByRole("button", { name: "Turn on" })).toBeTruthy();
  });

  it("still has it, unticked, on the third and fourth", async () => {
    for (const n of [1, 2, 3, 4]) (await opened(n)).unmount();
    await opened(5);
    expect((screen.getByRole("checkbox", BOX) as HTMLInputElement).checked).toBe(false);
  });

  it("ticking changes nothing by itself", async () => {
    (await opened(1)).unmount();
    await opened(2);
    fireEvent.click(screen.getByRole("checkbox", BOX));
    expect((screen.getByRole("checkbox", BOX) as HTMLInputElement).checked).toBe(true);
    expect(readShown(A)).toEqual({ shown: 2, hide: false });
    expect(fullNotice()).toBeTruthy();
  });

  it("ticking and pressing Turn on folds the next first-open screen", async () => {
    (await opened(1)).unmount();
    await opened(2);
    fireEvent.click(screen.getByRole("checkbox", BOX));
    await turnOn();
    expect(readShown(A)).toEqual({ shown: 2, hide: true });
    cleanup();

    open();
    await screen.findByRole("button", { name: "About mods" });
    // One line: the notice is folded, no heading and no box, and the screen is counted no further.
    expect((folded() as HTMLElement).getAttribute("aria-expanded")).toBe("false");
    expect(
      screen.getByText(/Mods are small add-ons made by people/).closest("[inert]"),
    ).toBeTruthy();
    expect(fullNotice()).toBeNull();
    expect(screen.queryByRole("checkbox", BOX)).toBeNull();
    expect(readShown(A)).toEqual({ shown: 2, hide: true });
  });

  it("opens the whole notice on a click, and the next screen starts folded again", async () => {
    localStorage.setItem(A, JSON.stringify({ v: 1, shown: 2, hide: true }));
    open();
    const line = await screen.findByRole("button", { name: "About mods" });
    fireEvent.click(line);
    expect(line.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByText(/Mods are small add-ons made by people/).closest("[inert]")).toBeNull();
    cleanup();
    open();
    expect(
      ((await screen.findByRole("button", { name: "About mods" })) as HTMLElement).getAttribute(
        "aria-expanded",
      ),
    ).toBe("false");
  });

  it("never folds what the mod asks to do, or Turn on", async () => {
    localStorage.setItem(A, JSON.stringify({ v: 1, shown: 2, hide: true }));
    open();
    await screen.findByRole("button", { name: "About mods" });
    expect(screen.getByRole("heading", { name: "What this mod asks to do" })).toBeTruthy();
    const items = within(screen.getByRole("list")).getAllByRole("listitem");
    expect(items).toHaveLength(2);
    expect(items[0].textContent).toContain("See your documents' titles");
    expect(screen.getByRole("checkbox", { name: /^Allow: / })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Turn on" })).toBeTruthy();
    // And Turn on still sends the answers: the backend's rules are not skipped.
    await turnOn();
    expect(consentMod).toHaveBeenCalledWith("acme.tool", { "documents.meta": true });
  });

  it("ticking and not pressing Turn on leaves it off", async () => {
    (await opened(1)).unmount();
    const second = await opened(2);
    fireEvent.click(screen.getByRole("checkbox", BOX));
    second.unmount(); // closes the screen: no Turn on
    expect(consentMod).not.toHaveBeenCalled();
    expect(readShown(A)).toEqual({ shown: 2, hide: false });
    await opened(3);
    expect(fullNotice()).toBeTruthy();
    expect(folded()).toBeNull();
    expect((screen.getByRole("checkbox", BOX) as HTMLInputElement).checked).toBe(false);
  });

  it("a Turn on the backend refuses doesn't fold anything", async () => {
    (await opened(1)).unmount();
    await opened(2);
    consentMod.mockRejectedValue({ code: "locked", message: "PM is locked." });
    fireEvent.click(screen.getByRole("checkbox", BOX));
    fireEvent.click(screen.getByRole("button", { name: "Turn on" }));
    expect(await screen.findByText("PM is locked.")).toBeTruthy();
    expect(readShown(A)).toEqual({ shown: 2, hide: false });
    // The box is still ticked, so the person can try again.
    expect((screen.getByRole("checkbox", BOX) as HTMLInputElement).checked).toBe(true);
  });

  it("pressing Turn on without the box ticked doesn't fold it", async () => {
    (await opened(1)).unmount();
    await opened(2);
    await turnOn();
    expect(readShown(A)).toEqual({ shown: 2, hide: false });
  });
});

describe("counting", () => {
  it("counts a showing once however often the screen draws", async () => {
    const view = await opened(1);
    view.rerender(<ModFirstOpen mod={{ ...waiting, version: "1.3.0" }} onTurnedOn={onTurnedOn} />);
    view.rerender(<ModFirstOpen mod={{ ...waiting, version: "1.4.0" }} onTurnedOn={onTurnedOn} />);
    await act(async () => {});
    expect(readShown(A).shown).toBe(1);
  });

  it("counts once under StrictMode, which runs the opening effect twice", async () => {
    open(true);
    await waitFor(() => expect(readShown(A).shown).toBe(1));
    await act(async () => {});
    await act(async () => {});
    expect(readShown(A).shown).toBe(1);
    cleanup();
    open(true);
    await waitFor(() => expect(readShown(A).shown).toBe(2));
    await act(async () => {});
    expect(readShown(A).shown).toBe(2);
    expect(screen.getByRole("checkbox", BOX)).toBeTruthy();
  });

  it("counts each time the screen is opened again", async () => {
    for (const n of [1, 2, 3]) (await opened(n)).unmount();
  });

  it("shows the whole notice and no box while the count is still being read", async () => {
    vaultStatus.mockReturnValue(new Promise(() => {}));
    localStorage.setItem(A, JSON.stringify({ v: 1, shown: 4, hide: true }));
    open();
    await act(async () => {});
    expect(fullNotice()).toBeTruthy();
    expect(screen.queryByRole("checkbox", BOX)).toBeNull();
  });
});

describe("the Mods tab's own gate counts too", () => {
  it("makes a mod's first-open screen the second showing, so it has the box", async () => {
    render(
      <ModsDisclaimerGate>
        <p>the sections</p>
      </ModsDisclaimerGate>,
    );
    await screen.findByRole("button", { name: "I understand" });
    await waitFor(() => expect(readShown(A).shown).toBe(1));
    cleanup();
    await opened(2);
    expect(screen.getByRole("checkbox", BOX)).toBeTruthy();
  });

  it("and a first-open screen's showing counts toward the gate's, which never has the box", async () => {
    (await opened(1)).unmount();
    render(
      <ModsDisclaimerGate>
        <p>the sections</p>
      </ModsDisclaimerGate>,
    );
    await screen.findByRole("button", { name: "I understand" });
    await waitFor(() => expect(readShown(A).shown).toBe(2));
    expect(screen.queryByRole("checkbox", BOX)).toBeNull();
  });
});

describe("per vault", () => {
  it("keeps each vault's showings and choice apart", async () => {
    (await opened(1)).unmount();
    await opened(2);
    fireEvent.click(screen.getByRole("checkbox", BOX));
    await turnOn();
    cleanup();

    // A vault this device has never opened a mod's first-open screen in starts again: the first
    // showing, in full, no box.
    vaultStatus.mockResolvedValue({ vault_id: "vault-b" });
    await opened(1, B);
    expect(fullNotice()).toBeTruthy();
    expect(folded()).toBeNull();
    expect(screen.queryByRole("checkbox", BOX)).toBeNull();
    cleanup();

    // Back in the first one it is still folded.
    vaultStatus.mockResolvedValue({ vault_id: "vault-a" });
    open();
    expect(await screen.findByRole("button", { name: "About mods" })).toBeTruthy();
    expect(readShown(A)).toEqual({ shown: 2, hide: true });
    expect(readShown(B)).toEqual({ shown: 1, hide: false });
  });
});

describe("when what is stored can't be trusted", () => {
  it.each([
    ["not JSON", "{oops"],
    ["another version", JSON.stringify({ v: 9, shown: 5, hide: true })],
    ["a count that isn't one", JSON.stringify({ v: 1, shown: "many", hide: true })],
    ["a negative count", JSON.stringify({ v: 1, shown: -3, hide: true })],
    ["an opt-out without a count", JSON.stringify({ v: 1, hide: true })],
  ])("%s reads as the first showing and not opted out", async (_what, raw) => {
    localStorage.setItem(A, raw);
    open();
    await waitFor(() => expect(readShown(A).shown).toBe(1));
    await act(async () => {});
    expect(fullNotice()).toBeTruthy();
    expect(folded()).toBeNull();
    expect(screen.queryByRole("checkbox", BOX)).toBeNull();
  });

  it("shows the whole notice and no box when localStorage can't be read", async () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    open();
    await act(async () => {});
    await act(async () => {});
    expect(fullNotice()).toBeTruthy();
    expect(screen.queryByRole("checkbox", BOX)).toBeNull();
    vi.restoreAllMocks();
  });

  it("shows the whole notice, no box and stores nothing when the vault can't be named", async () => {
    for (const status of [
      () => Promise.resolve({ vault_id: null }),
      () => Promise.reject(new Error("locked")),
    ]) {
      vaultStatus.mockImplementation(status);
      for (let i = 0; i < 3; i++) {
        open();
        await act(async () => {});
        await act(async () => {});
        expect(fullNotice()).toBeTruthy();
        expect(screen.queryByRole("checkbox", BOX)).toBeNull();
        cleanup();
      }
    }
    expect(localStorage.length).toBe(0);
  });
});

describe("a mod PM installed on its own", () => {
  const SENTENCE =
    "PM downloaded this for you because you used the Map. It does nothing until you turn it on.";
  const MOD_WORDS = "Does a thing.";
  const map = { ...waiting, id: "atlas.map", name: "Document Map" };

  it("says plainly that PM downloaded it, and that it does nothing until turned on", async () => {
    render(<ModFirstOpen mod={{ ...map, autoInstalled: true }} onTurnedOn={onTurnedOn} />);
    expect(await screen.findByText(SENTENCE)).toBeTruthy();
    // After the mod's own description, never in place of it or of what it asks.
    expect(screen.getByText(MOD_WORDS)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Turn on" })).toBeTruthy();
    expect(screen.getByText("What this mod asks to do")).toBeTruthy();
  });

  it("says nothing of the kind for a mod the person chose", async () => {
    open();
    await screen.findByRole("button", { name: "Turn on" });
    expect(screen.queryByText(/PM downloaded this for you/)).toBeNull();
  });

  it("says nothing of the kind for the Document Map the person installed themselves", async () => {
    // The same id as the one PM downloads, but the backend says PM didn't install this one.
    render(<ModFirstOpen mod={{ ...map, autoInstalled: false }} onTurnedOn={onTurnedOn} />);
    await screen.findByRole("button", { name: "Turn on" });
    expect(screen.queryByText(/PM downloaded this for you/)).toBeNull();
  });

  it("names no feature for an auto-installed mod that is not a pilot", async () => {
    render(<ModFirstOpen mod={{ ...waiting, autoInstalled: true }} onTurnedOn={onTurnedOn} />);
    await screen.findByRole("button", { name: "Turn on" });
    expect(screen.queryByText(/PM downloaded this for you/)).toBeNull();
  });
});

describe("help", () => {
  it("has an entry for each new surface", () => {
    for (const id of ["mod-first-open-notice", "mod-first-open-hide", "mod-first-open-about"]) {
      expect(HELP[id]?.title, id).toBeTruthy();
      expect(HELP[id]?.body.length, id).toBeGreaterThan(40);
    }
  });

  it("is carried by the surfaces themselves", async () => {
    (await opened(1)).unmount();
    const { container } = await opened(2);
    expect(container.querySelector('[data-help="mod-first-open-notice"]')).toBeTruthy();
    expect(container.querySelector('[data-help="mod-first-open-hide"]')).toBeTruthy();
    cleanup();
    localStorage.setItem(A, JSON.stringify({ v: 1, shown: 2, hide: true }));
    const folded2 = open();
    await screen.findByRole("button", { name: "About mods" });
    expect(folded2.container.querySelector('[data-help="mod-first-open-about"]')).toBeTruthy();
  });
});
