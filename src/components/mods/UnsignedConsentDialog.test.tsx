// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The stark screen for a mod nobody has checked (an unsigned file or a test folder): nobody vouches for
// it, every permission starts at no and is answered one by one, D3's read-or-internet choice and the
// backend's reasons for what is off, and a typed id before the button works. The page sends what the
// person chose; Rust checks the id and the rules again.

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { resetModActions, setModScreen, useModActions } from "../../lib/modActions";
import type { ConsentScreen, InstallResult, UnsignedScreen } from "../../lib/modsTypes";
import { consentPerm, consentScreen, RLO } from "./modFixtures";

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

const result: InstallResult = {
  id: "acme.tool",
  version: "0.1.0",
  state: "active",
  kind: "fresh",
  pruned: [],
  grantsSaved: true,
  grantsProblem: null,
};

function Host() {
  const { screens, notes } = useModActions();
  const staged = screens.values().next().value;
  return (
    <>
      {staged && <ConsentDialog key={staged.token} screen={staged} refresh={refresh} />}
      <p data-testid="staged">{[...screens.keys()].join(",")}</p>
      <ul aria-label="notes">
        {notes.map((n) => (
          <li key={n.id}>{n.text}</li>
        ))}
      </ul>
    </>
  );
}

function open(s: ConsentScreen, restart?: () => Promise<ConsentScreen>) {
  resetModActions();
  setModScreen(s.id, s, restart);
  return render(<Host />);
}

beforeEach(() => {
  vi.clearAllMocks();
  resetModActions();
});
afterEach(cleanup);

const stark = (extra: Partial<UnsignedScreen> = {}): UnsignedScreen => ({
  confirmId: "acme.tool",
  reinstall: false,
  engineSealed: true,
  readOrInternet: null,
  testFolder: null,
  ...extra,
});

// What the backend sends for an unsigned file: every permission asked one by one, defaulting to no.
const ask = (id: string, extra = {}) =>
  consentPerm(id, { when: "ask", access: "asks", defaultAnswer: false, ...extra });

const unsigned = (extra: Partial<ConsentScreen> = {}, s: Partial<UnsignedScreen> = {}) =>
  consentScreen({
    kind: "fresh",
    installedVersion: null,
    from: "file",
    tier: "third",
    version: "0.1.0",
    token: "tok-u",
    permissions: [
      ask("tasks.read", { why: "To list them." }),
      ask("notify", { class: "local" }),
      consentPerm("storage", {
        answerRequired: false,
        when: "auto",
        class: "local",
        access: "allowed",
      }),
    ],
    unsigned: stark(s),
    ...extra,
  });

const install = () => screen.getByRole("button", { name: "Install" }) as HTMLButtonElement;
const typeId = (text: string) =>
  fireEvent.change(screen.getByLabelText(/To go on, type the mod's id/), {
    target: { value: text },
  });

describe("what it says", () => {
  it("says plainly that nobody has checked it, and that the maker's words are only claims", () => {
    open(unsigned());
    const dialog = screen.getByRole("dialog", {
      name: "Install Acme Tool, which nobody has checked?",
    });
    expect(dialog.textContent).toContain("Nobody has checked this mod.");
    expect(dialog.textContent).toContain("Atlas hasn't reviewed it or signed it");
    expect(dialog.textContent).toContain("Third party");
    expect(dialog.textContent).toContain("says about itself. PM has not checked any of it");
    expect(dialog.textContent).toContain("Made by: Acme");
    expect(dialog.textContent).toContain("Repository: acme/tool");
    expect(dialog.textContent).toContain("Commit: abcdef0123456789");
    // It never claims a review.
    expect(dialog.textContent).not.toContain("reviewed by Atlas at commit");
  });

  it("lists every permission in the backend's order with the file's reason", () => {
    open(unsigned());
    const items = within(screen.getByRole("dialog")).getAllByRole("listitem");
    // The claims list has items too; the permissions are the last three.
    const perms = items.slice(-3);
    expect(perms[0].textContent).toContain("Read your tasks");
    expect(perms[0].textContent).toContain("The file says: “To list them.”");
    expect(perms[1].textContent).toContain("Show you notices inside PM");
    expect(perms[2].textContent).toContain("Always on.");
  });

  it("draws everything the file supplied as text, without direction tricks", () => {
    const evil = '<img src=x onerror="alert(1)"><a href="https://evil.example">x</a>';
    open(
      unsigned({
        name: `Ac${RLO}me`,
        description: evil,
        author: evil,
        permissions: [ask("tasks.read", { why: evil })],
      }),
    );
    const dialog = screen.getByRole("dialog", { name: /Install Ac me, which nobody/ });
    expect(dialog.querySelector("img")).toBeNull();
    expect(dialog.querySelector("a")).toBeNull();
    expect(dialog.textContent).not.toContain(RLO);
  });

  it("says a reinstall replaces the earlier one afresh and keeps what it stored", () => {
    open(unsigned({}, { reinstall: true }));
    expect(screen.getByRole("dialog").textContent).toContain(
      "You already installed an unchecked mod with this id. This replaces it as a fresh install: what you allowed and any keys it saved are reset unless you allow them again. What it stored itself is kept.",
    );
    cleanup();
    open(unsigned());
    expect(screen.getByRole("dialog").textContent).not.toContain("You already installed");
  });

  it("lists the sites and keys as the file's claims", () => {
    open(
      unsigned({
        internetHosts: ["api.example.com"],
        secrets: [{ name: "KEY", host: "api.example.com" }],
      }),
    );
    const text = screen.getByRole("dialog").textContent;
    expect(text).toContain("Sites the file says it may reach: api.example.com");
    expect(text).toContain("Saved keys the file says it keeps: KEY (for api.example.com)");
  });

  it("a test folder says where it is read from and that it lasts until PM restarts", () => {
    open(unsigned({ from: "folder" }, { testFolder: "/home/dev/my-mod" }));
    const dialog = screen.getByRole("dialog", { name: "Load Acme Tool as a test mod?" });
    expect(dialog.textContent).toContain("/home/dev/my-mod");
    expect(dialog.textContent).toContain("never copies or changes it");
    expect(dialog.textContent).toContain("last until PM restarts");
    expect(screen.getByRole("button", { name: "Load test mod" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Install" })).toBeNull();
  });
});

describe("permissions, one by one, defaulting to no", () => {
  it("every box starts unticked even if the screen's default says yes", () => {
    open(
      unsigned({
        permissions: [ask("tasks.read", { defaultAnswer: true }), ask("notify")],
      }),
    );
    const boxes = within(screen.getByRole("dialog")).getAllByRole("checkbox") as HTMLInputElement[];
    expect(boxes.map((b) => b.checked)).toEqual([false, false]);
  });

  it("sends an answer for each permission it asks, no unless ticked", async () => {
    installModConsented.mockResolvedValue(result);
    open(unsigned());
    fireEvent.click(screen.getByRole("checkbox", { name: /Allow: Show you notices/ }));
    typeId("acme.tool");
    fireEvent.click(install());
    await waitFor(() =>
      expect(installModConsented).toHaveBeenCalledWith(
        "tok-u",
        { "tasks.read": false, notify: true },
        "acme.tool",
      ),
    );
  });

  it("a permission the backend says is off has no box, only its reason, and is sent as no", async () => {
    installModConsented.mockResolvedValue(result);
    open(
      unsigned({
        permissions: [
          ask("tasks.read", {
            access: "off",
            offBecause: "Unsigned mods can't read your data until PM's web engine is sealed.",
          }),
          ask("notify"),
        ],
      }),
    );
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getAllByRole("checkbox").length).toBe(1);
    expect(dialog.textContent).toContain(
      "Off. Unsigned mods can't read your data until PM's web engine is sealed.",
    );
    typeId("acme.tool");
    fireEvent.click(install());
    await waitFor(() =>
      expect(installModConsented).toHaveBeenCalledWith(
        "tok-u",
        { "tasks.read": false, notify: false },
        "acme.tool",
      ),
    );
  });

  it("an off permission with no reason from the backend still says it is off", () => {
    open(unsigned({ permissions: [ask("tasks.read", { access: "off", offBecause: null })] }));
    expect(screen.getByRole("dialog").textContent).toContain("Off. PM won't allow this one.");
  });
});

describe("read or the internet, never both (D3)", () => {
  const both = (s: Partial<UnsignedScreen> = {}) =>
    unsigned(
      {
        permissions: [
          ask("tasks.read", { class: "read" }),
          ask("documents.meta", { class: "read" }),
          ask("internet", { class: "egress" }),
          ask("notify", { class: "local" }),
        ],
      },
      {
        readOrInternet: {
          read: ["tasks.read", "documents.meta"],
          internet: ["internet"],
          heldByOthers: null,
        },
        ...s,
      },
    );

  it("starts on neither: the sides' boxes can't be ticked, the free one can", () => {
    open(both());
    const dialog = screen.getByRole("dialog");
    expect(
      (within(dialog).getByRole("radio", { name: /Neither/ }) as HTMLInputElement).checked,
    ).toBe(true);
    const box = (n: RegExp) =>
      within(dialog).getByRole("checkbox", { name: n }) as HTMLInputElement;
    expect(box(/Read your tasks/).disabled).toBe(true);
    expect(box(/Send and fetch things/).disabled).toBe(true);
    expect(box(/Show you notices/).disabled).toBe(false);
    expect(dialog.textContent).toContain("Choose “reading your data” above");
    expect(dialog.textContent).toContain("Choose “the internet” above");
  });

  it("choosing reading lets the read boxes be ticked and keeps the internet one shut", () => {
    open(both());
    fireEvent.click(screen.getByRole("radio", { name: /Let it read my data/ }));
    const box = (n: RegExp) => screen.getByRole("checkbox", { name: n }) as HTMLInputElement;
    expect(box(/Read your tasks/).disabled).toBe(false);
    expect(box(/See your documents/).disabled).toBe(false);
    expect(box(/Send and fetch things/).disabled).toBe(true);
  });

  it("switching sides forgets the side left, and what is sent never has both", async () => {
    installModConsented.mockResolvedValue(result);
    open(both());
    fireEvent.click(screen.getByRole("radio", { name: /Let it read my data/ }));
    fireEvent.click(screen.getByRole("checkbox", { name: /Read your tasks/ }));
    fireEvent.click(screen.getByRole("radio", { name: /Let it use the internet/ }));
    expect(
      (screen.getByRole("checkbox", { name: /Read your tasks/ }) as HTMLInputElement).checked,
    ).toBe(false);
    fireEvent.click(screen.getByRole("checkbox", { name: /Send and fetch things/ }));
    // Going back to reading does not bring the old yes back.
    fireEvent.click(screen.getByRole("radio", { name: /Let it read my data/ }));
    expect(
      (screen.getByRole("checkbox", { name: /Read your tasks/ }) as HTMLInputElement).checked,
    ).toBe(false);
    typeId("acme.tool");
    fireEvent.click(install());
    await waitFor(() => expect(installModConsented).toHaveBeenCalled());
    const sent = installModConsented.mock.calls[0][1] as Record<string, boolean>;
    expect(sent).toEqual({
      "tasks.read": false,
      "documents.meta": false,
      internet: false,
      notify: false,
    });
  });

  it("sends the one side the person chose", async () => {
    installModConsented.mockResolvedValue(result);
    open(both());
    fireEvent.click(screen.getByRole("radio", { name: /Let it use the internet/ }));
    fireEvent.click(screen.getByRole("checkbox", { name: /Send and fetch things/ }));
    typeId("acme.tool");
    fireEvent.click(install());
    await waitFor(() =>
      expect(installModConsented).toHaveBeenCalledWith(
        "tok-u",
        { "tasks.read": false, "documents.meta": false, internet: true, notify: false },
        "acme.tool",
      ),
    );
  });

  it("says when another unchecked mod already holds a side, and shuts the other side", () => {
    const s = both({
      readOrInternet: {
        read: ["tasks.read", "documents.meta"],
        internet: ["internet"],
        heldByOthers: "read",
      },
    });
    // The backend turns the opposite side off, with its words.
    s.permissions = s.permissions.map((p) =>
      p.id === "internet"
        ? {
            ...p,
            access: "off" as const,
            offBecause: "Unsigned mods may read your data or reach the internet, never both.",
          }
        : p,
    );
    open(s);
    const dialog = screen.getByRole("dialog");
    expect(dialog.textContent).toContain(
      "One of your other unchecked mods already has reading your data, so PM won't give this one the other.",
    );
    expect(
      (within(dialog).getByRole("radio", { name: /Let it use the internet/ }) as HTMLInputElement)
        .disabled,
    ).toBe(true);
    expect(
      (within(dialog).getByRole("radio", { name: /Let it read my data/ }) as HTMLInputElement)
        .disabled,
    ).toBe(false);
    expect(dialog.textContent).toContain(
      "Off. Unsigned mods may read your data or reach the internet, never both.",
    );
  });

  it("is not shown for a file that asks for only one side", () => {
    open(unsigned());
    expect(screen.queryByRole("radio")).toBeNull();
    expect(screen.queryByText("Reading your data or the internet")).toBeNull();
  });
});

describe("the typed id", () => {
  it("keeps the button off until the id is typed exactly (spaces around it don't count)", () => {
    open(unsigned());
    expect(install().disabled).toBe(true);
    expect(screen.getByLabelText(/To go on, type the mod's id/)).toBeTruthy();
    expect(screen.getByText("acme.tool", { selector: "code" })).toBeTruthy();
    for (const wrong of ["acme", "acme.tool.x", "ACME.TOOL", "acme.too1", "Acme Tool"]) {
      typeId(wrong);
      expect(install().disabled).toBe(true);
    }
    expect(screen.getByText("That isn't the id yet. Type it exactly.")).toBeTruthy();
    typeId("  acme.tool  ");
    expect(install().disabled).toBe(false);
    expect(screen.queryByText("That isn't the id yet. Type it exactly.")).toBeNull();
  });

  it("installs nothing until the button, then sends the token, the answers and the typed id", async () => {
    installModConsented.mockResolvedValue(result);
    open(unsigned());
    typeId("acme.tool");
    expect(installModConsented).not.toHaveBeenCalled();
    fireEvent.click(install());
    await waitFor(() => expect(installModConsented).toHaveBeenCalledTimes(1));
    expect(installModConsented).toHaveBeenCalledWith(
      "tok-u",
      { "tasks.read": false, notify: false },
      "acme.tool",
    );
    await waitFor(() => expect(screen.getByTestId("staged").textContent).toBe(""));
    await waitFor(() => expect(refresh).toHaveBeenCalled());
  });

  it("a test folder asks for the id too", () => {
    open(unsigned({ from: "folder" }, { testFolder: "/x" }));
    const go = screen.getByRole("button", { name: "Load test mod" }) as HTMLButtonElement;
    expect(go.disabled).toBe(true);
    typeId("acme.tool");
    expect(go.disabled).toBe(false);
  });

  it("shows the backend's refusal if the id it checks says no, and keeps the screen", async () => {
    installModConsented.mockRejectedValue({
      code: "confirm-mismatch",
      message: "To install an unsigned mod, type its id exactly as shown.",
    });
    open(unsigned());
    typeId("acme.tool");
    fireEvent.click(install());
    expect(
      await screen.findByText("To install an unsigned mod, type its id exactly as shown."),
    ).toBeTruthy();
    expect(screen.getByTestId("staged").textContent).toBe("acme.tool");
  });
});

describe("while it works, and when it can't", () => {
  it("is busy until the backend answers and can't be pressed twice or cancelled", async () => {
    let finish!: (r: InstallResult) => void;
    installModConsented.mockReturnValue(
      new Promise<InstallResult>((res) => {
        finish = res;
      }),
    );
    open(unsigned());
    typeId("acme.tool");
    fireEvent.click(install());
    const working = (await screen.findByRole("button", {
      name: "Installing…",
    })) as HTMLButtonElement;
    expect(working.disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Cancel" }) as HTMLButtonElement).disabled).toBe(
      true,
    );
    fireEvent.click(working);
    expect(installModConsented).toHaveBeenCalledTimes(1);
    finish(result);
    await waitFor(() => expect(screen.getByTestId("staged").textContent).toBe(""));
  });

  it("developer mode switched off meanwhile: the backend's sentence, and the screen stays", async () => {
    installModConsented.mockRejectedValue({
      code: "dev-mode-off",
      message: "Developer mode is off on this computer.",
    });
    open(unsigned());
    typeId("acme.tool");
    fireEvent.click(install());
    expect(await screen.findByText("Developer mode is off on this computer.")).toBeTruthy();
    expect(screen.getByTestId("staged").textContent).toBe("acme.tool");
  });

  it("an answer refused by the rules (grant-refused) is shown in the backend's words", async () => {
    installModConsented.mockRejectedValue({
      code: "grant-refused",
      message: "An unsigned mod may read your data or reach the internet, never both.",
    });
    open(unsigned());
    typeId("acme.tool");
    fireEvent.click(install());
    expect(
      await screen.findByText(
        "An unsigned mod may read your data or reach the internet, never both.",
      ),
    ).toBeTruthy();
  });

  it("an expired stark screen says so and starts again, with the typed id empty", async () => {
    installModConsented.mockRejectedValue({ code: "consent-expired", message: "Out of date." });
    const restart = vi.fn().mockResolvedValue(unsigned({ token: "tok-v" }));
    open(unsigned(), restart);
    typeId("acme.tool");
    fireEvent.click(install());
    fireEvent.click(await screen.findByRole("button", { name: "Start again" }));
    await screen.findByRole("dialog", { name: /Install Acme Tool, which nobody/ });
    expect(restart).toHaveBeenCalledTimes(1);
    expect(install().disabled).toBe(true);
    expect((screen.getByLabelText(/To go on, type the mod's id/) as HTMLInputElement).value).toBe(
      "",
    );
  });

  it("when the answers couldn't be saved it says everything is off until turned on", async () => {
    installModConsented.mockResolvedValue({
      ...result,
      grantsSaved: false,
      grantsProblem: { code: "internal", message: "The store was busy." },
    });
    open(unsigned());
    typeId("acme.tool");
    fireEvent.click(install());
    expect(
      await screen.findByText(
        /Acme Tool was installed, but PM couldn't save your answers: The store was busy\./,
      ),
    ).toBeTruthy();
  });

  it("Cancel drops the screen and installs nothing", () => {
    open(unsigned());
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(installModConsented).not.toHaveBeenCalled();
    expect(screen.getByTestId("staged").textContent).toBe("");
  });
});
