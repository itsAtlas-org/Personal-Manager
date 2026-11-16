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

function open(screen: ConsentScreen) {
  setModScreen(screen.id, screen);
  return render(
    <>
      <ConsentDialog screen={screen} refresh={refresh} />
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
    const dialog = screen.getByRole("dialog", { name: "Update Acme Tool to version 1.3.0?" });
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
    expect(screen.getByRole("dialog", { name: "Put Acme Tool back?" })).toBeTruthy();
    cleanup();
    open(consentScreen({ kind: "fresh", id: "acme.new" }));
    expect(screen.getByRole("dialog", { name: "Install Acme Tool?" })).toBeTruthy();
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
        internetHosts: [`h${RLO}.example`],
        secrets: [{ name: `K${RLO}y`, host: `h${LRI}.example` }],
        permissions: [consentPerm("documents.meta", { why: `to\nmap${RLO}` })],
        widening: [`host:w${RLO}.example`],
      }),
    );
    const dialog = screen.getByRole("dialog", { name: "Update Ac me to version 1.3.0?" });
    expect(dialog.textContent).toContain("Made by Mal lory");
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
        /Acme Tool was updated, but PM couldn't save your answers: The store was busy\. Everything it asked for is off until you turn it on in the Mods tab\./,
      ),
    ).toBeTruthy();
  });

  it("shows the backend's sentence when the screen has expired, and keeps the screen", async () => {
    installModConsented.mockRejectedValue({
      code: "consent-expired",
      message: "That screen has expired. Start again.",
    });
    open(update());
    fireEvent.click(screen.getByRole("button", { name: "Update" }));
    expect(await screen.findByText("That screen has expired. Start again.")).toBeTruthy();
    expect(screen.getByTestId("staged").textContent).toBe("acme.tool");
    // The person can try again or cancel.
    expect((screen.getByRole("button", { name: "Update" }) as HTMLButtonElement).disabled).toBe(
      false,
    );
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

describe("a screen it doesn't draw", () => {
  it("an unsigned file's stark screen is not drawn here, and nothing can be installed from it", () => {
    open(
      update({
        tier: "third",
        unsigned: {
          confirmId: "acme.tool",
          reinstall: false,
          engineSealed: true,
          readOrInternet: null,
          testFolder: null,
        },
      }),
    );
    expect(screen.getByRole("dialog", { name: "This one needs another screen" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Update" })).toBeNull();
    expect(installModConsented).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.getByTestId("staged").textContent).toBe("");
  });
});
