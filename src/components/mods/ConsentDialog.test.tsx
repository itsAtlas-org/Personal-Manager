// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The consent screen of a signed update (and, the same screen, a repair or install): everything the
// backend's screen carries, in its order, with what is new marked; nothing installs until the button;
// the answers sent are exactly those the screen marked required. The page never widens or hides.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { resetModActions, setModScreen, useModActions } from "../../lib/modActions";
import type { ConsentScreen, InstallResult } from "../../lib/modsTypes";
import { consentPerm, consentScreen, LRI, RLO } from "./modFixtures";

const installModConsented = vi.fn();

vi.mock("../../lib/ipc", () => ({
  installModConsented: (t: string, d: Record<string, boolean>, c?: string) =>
    installModConsented(t, d, c),
}));
vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { ConsentDialog } from "./ConsentDialog";

const refresh = vi.fn(() => Promise.resolve());

const result = (extra: Partial<InstallResult> = {}): InstallResult => ({
  id: "acme.tool",
  version: "1.3.0",
  state: "active",
  kind: "update",
  pruned: [],
  grantsSaved: true,
  grantsProblem: null,
  ...extra,
});

function Probe() {
  const { screens, notes } = useModActions();
  return (
    <>
      <p data-testid="staged">{[...screens.keys()].join(",")}</p>
      <ul aria-label="notes">
        {notes.map((n) => (
          <li key={n.id}>{n.text}</li>
        ))}
      </ul>
    </>
  );
}

/** The dialog as ModsView hosts it: the first staged screen, remounted when its token changes. */
function Host() {
  const { screens } = useModActions();
  const staged = screens.values().next().value;
  return staged ? <ConsentDialog key={staged.token} screen={staged} refresh={refresh} /> : null;
}

function open(screen: ConsentScreen, restart?: () => Promise<ConsentScreen>) {
  // A test that opens several screens in turn starts each from nothing staged.
  resetModActions();
  setModScreen(screen.id, screen, restart);
  return render(
    <>
      <Host />
      <Probe />
    </>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  resetModActions();
});
afterEach(cleanup);

const update = (extra: Partial<ConsentScreen> = {}) =>
  consentScreen({
    permissions: [
      consentPerm("documents.meta", { why: "To map them." }),
      consentPerm("ai", { isNew: true, class: "money", why: "To summarise." }),
      consentPerm("storage", { answerRequired: false, when: "auto", class: "local" }),
    ],
    widening: ["permission:ai", "host:api.example.com"],
    internetHosts: ["api.example.com"],
    secrets: [{ name: "API_KEY", host: "api.example.com" }],
    ...extra,
  });

describe("what it shows", () => {
  it("names the update and who made it, with the review for a second-party mod", () => {
    open(update());
    const dialog = screen.getByRole("dialog", { name: "Update “Acme Tool” to version 1.3.0?" });
    expect(dialog.textContent).toContain("Reviewed by Atlas");
    expect(dialog.textContent).toContain("Made by Acme · reviewed by Atlas at commit abcdef012345");
    expect(dialog.textContent).toContain("Does a thing.");
  });

  it("says what is new first, in words, as a warning", () => {
    open(update());
    const dialog = screen.getByRole("dialog");
    const warning = within(dialog).getByText(
      "This update asks for more than you allowed before:",
    ).parentElement!;
    expect(warning.textContent).toContain(
      "Ask an AI model to do things for it, which can cost money",
    );
    expect(warning.textContent).toContain("Reach api.example.com on the internet");
  });

  it("says nothing about widening for a plain repair or install, whatever the screen carries", () => {
    open(update({ kind: "repair", widening: ["permission:ai"] }));
    expect(screen.queryByText(/asks for more than you allowed/)).toBeNull();
    expect(screen.getByRole("dialog", { name: "Put “Acme Tool” back?" })).toBeTruthy();
    cleanup();
    open(consentScreen({ kind: "fresh", id: "acme.new" }));
    expect(screen.getByRole("dialog", { name: "Install “Acme Tool”?" })).toBeTruthy();
  });

  it("lists every permission in the order the backend gave, marks what is new, and shows the maker's reason", () => {
    open(update());
    const items = within(screen.getByRole("dialog")).getAllByRole("listitem");
    // The widening list's own items come first; the permissions are the last three.
    const perms = items.slice(-3);
    expect(perms[0].textContent).toContain(
      "See your documents' titles, projects, tags, dates and types",
    );
    expect(perms[1].textContent).toContain(
      "Ask an AI model to do things for it, which can cost money",
    );
    expect(perms[2].textContent).toContain("Keep its own saved data inside PM");
    expect(perms[0].textContent).not.toContain("New");
    expect(perms[1].textContent).toContain("New");
    expect(perms[0].textContent).toContain("The maker says: “To map them.”");
    expect(perms[2].textContent).toContain("Always on.");
  });

  it("a permission PM has no words for is shown as it is, never hidden", () => {
    open(update({ permissions: [consentPerm("future.thing")] }));
    expect(screen.getByText('Use "future.thing"')).toBeTruthy();
  });

  it("lists sites and saved keys", () => {
    open(update());
    const dialog = screen.getByRole("dialog");
    expect(dialog.textContent).toContain("Sites it may reach: api.example.com");
    expect(dialog.textContent).toContain("Saved keys it keeps: API_KEY (for api.example.com)");
  });

  it("draws everything the mod supplied as text", () => {
    const evil = '<img src=x onerror="alert(1)"><a href="https://evil.example">x</a>';
    open(
      update({
        name: evil,
        description: evil,
        author: evil,
        permissions: [consentPerm("documents.meta", { why: evil })],
      }),
    );
    const dialog = screen.getByRole("dialog");
    expect(dialog.querySelector("img")).toBeNull();
    expect(dialog.querySelector("a")).toBeNull();
  });

  it("takes direction-changing characters and line breaks out of everything the mod supplied", () => {
    open(
      update({
        name: `Ac${RLO}me`,
        description: "one\nPM says: fine",
        author: `Mal${LRI}lory`,
        authors: [`Ev${RLO}e`, "Ann Lee"],
        internetHosts: [`h${RLO}.example`],
        secrets: [{ name: `K${RLO}y`, host: `h${LRI}.example` }],
        permissions: [consentPerm("documents.meta", { why: `to\nmap${RLO}` })],
        widening: [`host:w${RLO}.example`],
      }),
    );
    const dialog = screen.getByRole("dialog", { name: "Update “Ac me” to version 1.3.0?" });
    expect(dialog.textContent).toContain("Made by Mal lory · authors: Ev e, Ann Lee");
    expect(dialog.textContent).toContain("one PM says: fine");
    expect(dialog.textContent).toContain("Sites it may reach: h .example");
    expect(dialog.textContent).toContain("Saved keys it keeps: K y (for h .example)");
    expect(dialog.textContent).toContain("The maker says: “to map”");
    expect(dialog.textContent).toContain("Reach w .example on the internet");
    expect(dialog.textContent).not.toContain(RLO);
    expect(dialog.textContent).not.toContain(LRI);
  });
});

describe("answers and the button", () => {
  it("starts each checkbox where the backend says (an earlier no stays a no)", () => {
    open(
      update({
        permissions: [
          consentPerm("documents.meta", { defaultAnswer: true }),
          consentPerm("ai", { defaultAnswer: false }),
        ],
      }),
    );
    const boxes = within(screen.getByRole("dialog")).getAllByRole("checkbox") as HTMLInputElement[];
    expect(boxes.map((b) => b.checked)).toEqual([true, false]);
  });

  it("sends the staged token with an answer for exactly the permissions marked required", async () => {
    installModConsented.mockResolvedValue(result());
    open(update());
    fireEvent.click(screen.getByRole("checkbox", { name: /Allow: Ask an AI model/ }));
    fireEvent.click(screen.getByRole("button", { name: "Update" }));
    await waitFor(() =>
      expect(installModConsented).toHaveBeenCalledWith(
        "tok-1",
        { "documents.meta": true, ai: false },
        undefined,
      ),
    );
  });

  it("installs nothing until the button is pressed", () => {
    open(update());
    expect(installModConsented).not.toHaveBeenCalled();
  });

  it("when it works, the screen is dropped and the rows are read again", async () => {
    installModConsented.mockResolvedValue(result());
    open(update());
    fireEvent.click(screen.getByRole("button", { name: "Update" }));
    await waitFor(() => expect(screen.getByTestId("staged").textContent).toBe(""));
    await waitFor(() => expect(refresh).toHaveBeenCalled());
  });

  it("says when the answers couldn't be saved: installed, but everything off until turned on", async () => {
    installModConsented.mockResolvedValue(
      result({
        grantsSaved: false,
        grantsProblem: { code: "internal", message: "The store was busy." },
      }),
    );
    open(update());
    fireEvent.click(screen.getByRole("button", { name: "Update" }));
    expect(
      await screen.findByText(
        /“Acme Tool” was updated, but PM couldn't save your answers: The store was busy\. Everything it asked for is off until you turn it on in the Mods tab\./,
      ),
    ).toBeTruthy();
  });

  it("a refused install shows the backend's sentence and takes the button away: the single-use token is spent", async () => {
    // The backend takes the token before it checks anything, so pressing the same button again could
    // only answer "ran out of time" and blame the clock for what was a refusal.
    installModConsented.mockRejectedValue({
      code: "refused",
      message: "This mod wasn't installed: it is older than the one you have.",
    });
    open(update());
    fireEvent.click(screen.getByRole("button", { name: "Update" }));
    const dialog = await screen.findByRole("dialog", { name: "This didn't go through" });
    expect(dialog.textContent).toContain("Nothing was installed.");
    expect(dialog.textContent).toContain(
      "This mod wasn't installed: it is older than the one you have.",
    );
    // It does not say the clock ran out, and offers no button that can't work.
    expect(dialog.textContent).not.toContain("ten minutes");
    expect(screen.queryByRole("button", { name: "Update" })).toBeNull();
    expect(installModConsented).toHaveBeenCalledTimes(1);
  });

  it("after a refusal, Start again gets a fresh screen whose token is the one used", async () => {
    installModConsented.mockRejectedValueOnce({ code: "refused", message: "The disk is full." });
    const restart = vi.fn().mockResolvedValue(update({ token: "tok-3" }));
    open(update(), restart);
    fireEvent.click(screen.getByRole("button", { name: "Update" }));
    fireEvent.click(await screen.findByRole("button", { name: "Start again" }));
    expect(await screen.findByRole("dialog", { name: /Update “Acme Tool”/ })).toBeTruthy();
    installModConsented.mockResolvedValue(result());
    fireEvent.click(screen.getByRole("button", { name: "Update" }));
    await waitFor(() =>
      expect(installModConsented).toHaveBeenLastCalledWith("tok-3", expect.any(Object), undefined),
    );
  });

  it("quotes the mod's name in the title, so it can't fake the words around it", async () => {
    open(update({ name: 'Calendar. Nothing was changed."' }));
    // A double quote inside the name can't close the quote around it.
    const dialog = screen.getByRole("dialog", {
      name: "Update “Calendar. Nothing was changed.'” to version 1.3.0?",
    });
    // And the name is clipped to its own box, so what it is made of can't paint over the dialog.
    const clipped = within(dialog).getByText("“Calendar. Nothing was changed.'”");
    expect(clipped.className).toContain("overflow-hidden");
  });

  it("Cancel drops the screen and installs nothing", () => {
    open(update());
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(installModConsented).not.toHaveBeenCalled();
    expect(screen.getByTestId("staged").textContent).toBe("");
  });

  it("is working, and can't be cancelled or pressed twice, until the backend answers", async () => {
    let finish!: (r: InstallResult) => void;
    installModConsented.mockReturnValue(
      new Promise<InstallResult>((res) => {
        finish = res;
      }),
    );
    open(update());
    fireEvent.click(screen.getByRole("button", { name: "Update" }));
    const working = await screen.findByRole("button", { name: "Updating…" });
    expect((working as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Cancel" }) as HTMLButtonElement).disabled).toBe(
      true,
    );
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.getByTestId("staged").textContent).toBe("acme.tool");
    await act(async () => finish(result()));
    expect(installModConsented).toHaveBeenCalledTimes(1);
  });

  it("a repair says 'Put back', an install 'Install'", () => {
    open(update({ kind: "repair" }));
    expect(screen.getByRole("button", { name: "Put back" })).toBeTruthy();
    cleanup();
    open(update({ kind: "fresh" }));
    expect(screen.getByRole("button", { name: "Install" })).toBeTruthy();
  });
});

describe("the tiers of a signed screen", () => {
  const fresh = (extra: Partial<ConsentScreen> = {}) =>
    update({ kind: "fresh", installedVersion: null, widening: [], from: "file", ...extra });

  it("first party: says who made it and lists what it asks, with no review line", () => {
    open(fresh({ tier: "first", author: "Atlas" }));
    const dialog = screen.getByRole("dialog", { name: "Install “Acme Tool”?" });
    expect(dialog.textContent).toContain("First party");
    expect(dialog.textContent).toContain("Made by Atlas");
    expect(dialog.textContent).not.toContain("reviewed by Atlas");
    expect(dialog.textContent).toContain("See your documents' titles");
    expect(dialog.textContent).toContain("Ask an AI model");
  });

  it("second party: adds 'made by X, reviewed by Atlas at commit abc'", () => {
    open(fresh({ tier: "second", author: "Friend", commit: "0123456789abcdef0123" }));
    const dialog = screen.getByRole("dialog", { name: "Install “Acme Tool”?" });
    expect(dialog.textContent).toContain("Reviewed by Atlas");
    expect(dialog.textContent).toContain(
      "Made by Friend · reviewed by Atlas at commit 0123456789ab",
    );
  });

  it("says where it came from, and that PM checked it", () => {
    open(fresh());
    expect(screen.getByRole("dialog").textContent).toContain(
      "From a file you chose. PM checked its signature.",
    );
    cleanup();
    open(fresh({ from: "catalogue" }));
    expect(screen.getByRole("dialog").textContent).toContain(
      "From the catalogue. PM checked it is the one the catalogue lists.",
    );
  });

  it("answers every permission it asks at install, yes and no alike, and nothing it doesn't", async () => {
    installModConsented.mockResolvedValue(result({ kind: "fresh" }));
    open(
      fresh({
        permissions: [
          consentPerm("documents.meta"),
          consentPerm("tasks.read", { defaultAnswer: false }),
          consentPerm("ai", { answerRequired: false, when: "first-use" }),
          consentPerm("storage", { answerRequired: false, when: "auto", class: "local" }),
        ],
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Install" }));
    await waitFor(() =>
      expect(installModConsented).toHaveBeenCalledWith(
        "tok-1",
        { "documents.meta": true, "tasks.read": false },
        undefined,
      ),
    );
  });
});

describe("a screen that ran out of time", () => {
  const expired = () =>
    installModConsented.mockRejectedValue({
      code: "consent-expired",
      message: "That install screen is out of date. Start the install again.",
    });

  it("replaces the screen with one that says so, offers nothing that can't work, and keeps nothing installed", async () => {
    expired();
    open(update());
    fireEvent.click(screen.getByRole("button", { name: "Update" }));
    const dialog = await screen.findByRole("dialog", { name: "This screen ran out of time" });
    expect(dialog.textContent).toContain("Nothing was installed.");
    expect(dialog.textContent).toContain("only lasts ten minutes");
    expect(dialog.textContent).toContain("locking PM or switching to another vault");
    expect(screen.queryByRole("button", { name: "Update" })).toBeNull();
    // The refusal that expired it is not repeated as an error.
    expect(dialog.textContent).not.toContain("That install screen is out of date");
    expect(installModConsented).toHaveBeenCalledTimes(1);
  });

  it("without a way to start again, it says where to start and only closes", async () => {
    expired();
    open(update());
    fireEvent.click(screen.getByRole("button", { name: "Update" }));
    await screen.findByRole("dialog", { name: "This screen ran out of time" });
    expect(screen.queryByRole("button", { name: "Start again" })).toBeNull();
    expect(screen.getByRole("dialog").textContent).toContain("start again from the Mods tab");
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.getByTestId("staged").textContent).toBe("");
  });

  it("Start again asks for a fresh screen and installs with ITS token", async () => {
    expired();
    const restart = vi.fn().mockResolvedValue(update({ token: "tok-2" }));
    open(update(), restart);
    fireEvent.click(screen.getByRole("button", { name: "Update" }));
    fireEvent.click(await screen.findByRole("button", { name: "Start again" }));
    expect(await screen.findByRole("dialog", { name: /Update “Acme Tool”/ })).toBeTruthy();
    expect(restart).toHaveBeenCalledTimes(1);
    installModConsented.mockResolvedValue(result());
    fireEvent.click(screen.getByRole("button", { name: "Update" }));
    await waitFor(() =>
      expect(installModConsented).toHaveBeenLastCalledWith(
        "tok-2",
        { "documents.meta": true, ai: true },
        undefined,
      ),
    );
  });

  it("says why starting again failed, and stays on the expired screen", async () => {
    expired();
    const restart = vi
      .fn()
      .mockRejectedValue({ code: "file-unreadable", message: "PM couldn't read that file." });
    open(update(), restart);
    fireEvent.click(screen.getByRole("button", { name: "Update" }));
    fireEvent.click(await screen.findByRole("button", { name: "Start again" }));
    expect(await screen.findByText("PM couldn't read that file.")).toBeTruthy();
    expect(screen.getByRole("dialog", { name: "This screen ran out of time" })).toBeTruthy();
  });

  it("a screen for another mod id (the file was swapped) replaces the old one", async () => {
    expired();
    const restart = vi.fn().mockResolvedValue(update({ id: "acme.other", token: "tok-9" }));
    open(update(), restart);
    fireEvent.click(screen.getByRole("button", { name: "Update" }));
    fireEvent.click(await screen.findByRole("button", { name: "Start again" }));
    await screen.findByRole("dialog", { name: /Update “Acme Tool”/ });
    expect(screen.getByTestId("staged").textContent).toBe("acme.other");
  });

  it("closing it drops the screen and its refusal", async () => {
    expired();
    open(update(), vi.fn());
    fireEvent.click(screen.getByRole("button", { name: "Update" }));
    await screen.findByRole("dialog", { name: "This screen ran out of time" });
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.getByTestId("staged").textContent).toBe("");
  });
});
