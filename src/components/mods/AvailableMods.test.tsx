// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Available section: the signed catalogue's entries with their tier, maker and what they ask for
// before anything is downloaded; Install is the backend's call to make (installable, and why not);
// the download runs outside the view.

import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { currentModActions, resetModActions } from "../../lib/modActions";
import type { Catalogue, CatalogueEntry } from "../../lib/modsTypes";
import { consentScreen, LINE_BREAK, RLO } from "./modFixtures";

const installFromCatalogue = vi.fn();

vi.mock("../../lib/ipc", () => ({
  installFromCatalogue: (id: string) => installFromCatalogue(id),
  modIcon: () => Promise.reject(new Error("none")),
}));
vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { AvailableMods } from "./AvailableMods";

beforeEach(() => {
  vi.clearAllMocks();
  resetModActions();
});
afterEach(cleanup);

const entry = (extra: Partial<CatalogueEntry> = {}): CatalogueEntry => ({
  id: "atlas.map",
  name: "Map",
  description: "Your documents on a map.",
  tier: "first",
  author: "Atlas",
  repo: "itsAtlas-org/PM-Mods",
  commit: "0123456789abcdef",
  version: "1.0.0",
  permissions: [
    { id: "documents.meta", class: "read" },
    { id: "storage", class: "local" },
  ],
  installed: false,
  installedVersion: null,
  updateAvailable: false,
  needsNewerPm: false,
  apiRetired: false,
  blocked: null,
  installable: true,
  whyNot: null,
  ...extra,
});

const list = (entries: CatalogueEntry[], extra: Partial<Catalogue> = {}): Catalogue => ({
  entries,
  usable: true,
  unavailable: null,
  ...extra,
});

const draw = (
  catalogue: Catalogue | null,
  extra: { problem?: string | null; dialogFor?: string | null } = {},
) => {
  const retry = vi.fn();
  const utils = render(
    <AvailableMods
      catalogue={catalogue}
      problem={extra.problem ?? null}
      retry={retry}
      dialogFor={extra.dialogFor ?? null}
    />,
  );
  return { ...utils, retry };
};

describe("what an entry shows", () => {
  it("first party: name, version, maker, and every permission in PM's words in the catalogue's order", () => {
    draw(
      list([
        entry({
          permissions: [
            { id: "storage", class: "local" },
            { id: "documents.meta", class: "read" },
            { id: "ai", class: "money" },
          ],
        }),
      ]),
    );
    const item = screen.getByRole("heading", { name: "Map" }).closest("li")!;
    expect(item.textContent).toContain("version 1.0.0");
    expect(item.textContent).toContain("First party");
    expect(item.textContent).toContain("Made by Atlas");
    expect(item.textContent).not.toContain("reviewed by Atlas at commit");
    expect(item.textContent).toContain("Your documents on a map.");
    const asks = within(item)
      .getAllByRole("listitem")
      .map((li) => li.textContent);
    expect(asks).toEqual([
      "Keep its own saved data inside PM (stays inside PM)",
      "See your documents' titles, projects, tags, dates and types (reads your data)",
      "Ask an AI model to do things for it, which can cost money (can cost money)",
    ]);
  });

  it("second party adds 'reviewed by Atlas at commit abc'", () => {
    draw(list([entry({ tier: "second", author: "Friend", commit: "abcdef0123456789ff" })]));
    const item = screen.getByRole("heading", { name: "Map" }).closest("li")!;
    expect(item.textContent).toContain("Reviewed by Atlas");
    expect(item.textContent).toContain("Made by Friend · reviewed by Atlas at commit abcdef012345");
  });

  it("a permission PM has no words for is shown as it is, and a class it doesn't know says nothing", () => {
    draw(list([entry({ permissions: [{ id: "future.thing", class: "unknown" }] })]));
    expect(screen.getByText('Use "future.thing"')).toBeTruthy();
  });

  it("an entry that asks for nothing says so", () => {
    draw(list([entry({ permissions: [] })]));
    expect(screen.getByText("It doesn't ask for anything.")).toBeTruthy();
  });

  it("draws the catalogue's text as text, with direction tricks and line breaks flattened", () => {
    const evil = '<img src=x onerror="alert(1)"><a href="https://evil.example">x</a>';
    draw(
      list([
        entry({
          name: `Ma${RLO}p`,
          description: `${evil}${LINE_BREAK}PM says: fine`,
          author: evil,
        }),
      ]),
    );
    const item = screen.getByRole("heading", { name: "Ma p" }).closest("li")!;
    expect(item.querySelector("img")).toBeNull();
    expect(item.querySelector("a")).toBeNull();
    expect(item.textContent).not.toContain(RLO);
    expect(item.textContent).not.toContain(LINE_BREAK);
  });

  it("keeps the catalogue's order", () => {
    draw(list([entry({ id: "z.last", name: "Zed" }), entry({ id: "a.first", name: "Alpha" })]));
    expect(screen.getAllByRole("heading").map((h) => h.textContent)).toEqual(["Zed", "Alpha"]);
  });
});

describe("installing", () => {
  it("Install downloads that mod and stages the consent screen; nothing is installed by it", async () => {
    installFromCatalogue.mockResolvedValue(consentScreen({ id: "atlas.map", kind: "fresh" }));
    draw(list([entry()]));
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Install Map" }));
    });
    expect(installFromCatalogue).toHaveBeenCalledWith("atlas.map");
    expect([...currentModActions().screens.keys()]).toEqual(["atlas.map"]);
  });

  it("is busy while downloading, and a second press does nothing", async () => {
    let finish!: (s: ReturnType<typeof consentScreen>) => void;
    installFromCatalogue.mockReturnValue(
      new Promise((res) => {
        finish = res;
      }),
    );
    draw(list([entry()]));
    fireEvent.click(screen.getByRole("button", { name: "Install Map" }));
    const working = await screen.findByRole("button", { name: "Downloading…" });
    expect((working as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(working);
    expect(installFromCatalogue).toHaveBeenCalledTimes(1);
    await act(async () => finish(consentScreen({ id: "atlas.map" })));
  });

  it("says 'Installing' while the install itself runs (it is the same mod's key)", async () => {
    draw(list([entry()]));
    const { runModAction } = await import("../../lib/modActions");
    let finish!: () => void;
    act(() => {
      void runModAction(
        "atlas.map",
        "Installing",
        "x",
        () =>
          new Promise<void>((res) => {
            finish = res;
          }),
      );
    });
    expect(
      ((await screen.findByRole("button", { name: "Installing…" })) as HTMLButtonElement).disabled,
    ).toBe(true);
    await act(async () => finish());
  });

  it("shows the backend's reason when the download fails, on the entry", async () => {
    installFromCatalogue.mockRejectedValue({
      code: "offline",
      message: "PM couldn't reach the internet.",
    });
    draw(list([entry()]));
    fireEvent.click(screen.getByRole("button", { name: "Install Map" }));
    expect(await screen.findByText("PM couldn't reach the internet.")).toBeTruthy();
    expect(
      (screen.getByRole("button", { name: "Install Map" }) as HTMLButtonElement).disabled,
    ).toBe(false);
  });

  it("leaves a refusal to the dialog that is open for the same mod", async () => {
    installFromCatalogue.mockRejectedValue({ code: "offline", message: "No connection." });
    const { rerender } = draw(list([entry()]));
    fireEvent.click(screen.getByRole("button", { name: "Install Map" }));
    await screen.findByText("No connection.");
    rerender(
      <AvailableMods
        catalogue={list([entry()])}
        problem={null}
        retry={() => {}}
        dialogFor="atlas.map"
      />,
    );
    expect(screen.queryByText("No connection.")).toBeNull();
  });
});

describe("what can't be installed", () => {
  it("an entry the backend says is not installable has a disabled button and its reason", () => {
    draw(
      list([
        entry({
          installable: false,
          needsNewerPm: true,
          whyNot: "Map 2.0.0 needs a newer PM. Update PM to get it.",
        }),
      ]),
    );
    expect(
      (screen.getByRole("button", { name: "Install Map" }) as HTMLButtonElement).disabled,
    ).toBe(true);
    expect(screen.getByText("Map 2.0.0 needs a newer PM. Update PM to get it.")).toBeTruthy();
  });

  it("the safety list's own words come first", () => {
    draw(
      list([
        entry({
          installable: false,
          blocked: "Map is on PM's safety list: it deletes files.",
          whyNot: "Map is on the safety list.",
        }),
      ]),
    );
    expect(screen.getByText("Map is on PM's safety list: it deletes files.")).toBeTruthy();
    expect(screen.queryByText("Map is on the safety list.")).toBeNull();
  });

  it("an out-of-date list shows the entries and the reason, and nothing installs", () => {
    draw(
      list([entry()], {
        usable: false,
        unavailable: "PM's safety list is out of date. Nothing installs until it is refreshed.",
      }),
    );
    expect(
      screen.getByText("PM's safety list is out of date. Nothing installs until it is refreshed."),
    ).toBeTruthy();
    // The backend said this entry is fine, but the list isn't: the page doesn't override that.
    expect(
      (screen.getByRole("button", { name: "Install Map" }) as HTMLButtonElement).disabled,
    ).toBe(true);
    expect(screen.getByText(/can't install from the catalogue until its safety list/)).toBeTruthy();
  });

  it("an installed entry has no Install, and says which version", () => {
    draw(list([entry({ installed: true, installedVersion: "0.9.0" })]));
    expect(screen.queryByRole("button", { name: /Install/ })).toBeNull();
    expect(screen.getByText("Installed, version 0.9.0")).toBeTruthy();
  });

  it("an installed entry with a newer version listed says so", () => {
    draw(list([entry({ installed: true, installedVersion: "0.9.0", updateAvailable: true })]));
    expect(screen.getByText("Version 1.0.0 is available.")).toBeTruthy();
  });
});

describe("the list itself", () => {
  it("says it is loading before the first answer", () => {
    draw(null);
    expect(screen.getByText("Loading…")).toBeTruthy();
  });

  it("says why it can't be read, with Try again", () => {
    const { retry } = draw(null, { problem: "PM is locked." });
    expect(screen.getByText(/PM couldn't read the catalogue: PM is locked\./)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(retry).toHaveBeenCalled();
  });

  it("says when nothing is on offer", () => {
    draw(list([]));
    expect(screen.getByText("Nothing is on offer yet.")).toBeTruthy();
  });

  it("an empty catalogue that can't be used still says why", () => {
    draw(list([], { usable: false, unavailable: "PM hasn't downloaded the catalogue yet." }));
    expect(screen.getByText("PM hasn't downloaded the catalogue yet.")).toBeTruthy();
  });
});
