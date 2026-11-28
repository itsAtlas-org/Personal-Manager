// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// One role's ordered list of models. What it must get right: every row says in words whether it can
// answer; the buttons carry the model and its place; focus and announcements follow a move; the list
// is Rust's (a refused edit leaves it alone); a person with one row and nothing else to add is not
// nagged; a direct row carries the I-13 warning; the picker is a real dialog that offers only what
// can be used and never Mythos or a custom id for a direct provider.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const m = vi.hoisted(() => ({
  routeAdd: vi.fn(),
  routeMove: vi.fn(),
  routeRemove: vi.fn(),
  routeSetEffort: vi.fn(),
  routeReset: vi.fn(),
  routeTryAgain: vi.fn(),
  listModels: vi.fn(),
}));

vi.mock("../../lib/ipc", () => ({
  routeAdd: (...a: unknown[]) => m.routeAdd(...a),
  routeMove: (...a: unknown[]) => m.routeMove(...a),
  routeRemove: (...a: unknown[]) => m.routeRemove(...a),
  routeSetEffort: (...a: unknown[]) => m.routeSetEffort(...a),
  routeReset: (...a: unknown[]) => m.routeReset(...a),
  routeTryAgain: (...a: unknown[]) => m.routeTryAgain(...a),
  listModels: () => m.listModels(),
}));
vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import {
  anthropicView,
  directState,
  FABLE,
  openaiView,
  stateWith,
} from "../../lib/directProvidersFixture";
import { claudeRow, geminiRow, localRow, orRow, routeView } from "../../lib/routeFixture";
import type {
  DirectProvidersState,
  RoleName,
  RouteEdited,
  RouteRow,
  RouteView,
} from "../../lib/types";
import { RoleRoute } from "./RoleRoute";

const onError = vi.fn();
const onChanged = vi.fn();
const be: { entries: RouteRow[] } = { entries: [] };
const edited = (over: Partial<RouteEdited> = {}): RouteEdited => ({
  route: routeView([...be.entries]),
  materialised: false,
  unused_saved: 0,
  ...over,
});

function Harness({
  entries,
  role = "chat",
  dp = null,
  openRouterKey = true,
  local = { on: false, model: null },
  defaultRoute,
}: {
  entries: RouteRow[];
  role?: RoleName;
  dp?: DirectProvidersState | null;
  openRouterKey?: boolean;
  local?: { on: boolean; model: string | null; elsewhere?: boolean };
  defaultRoute?: RouteView;
}) {
  const [route, setRoute] = useState<RouteView>(routeView(entries));
  be.entries = [...entries];
  return (
    <RoleRoute
      role={role}
      route={route}
      dp={dp}
      openRouterKey={openRouterKey}
      local={local}
      helpId="settings-chat-models"
      onRoute={(_r, v) => setRoute(v)}
      onChanged={onChanged}
      onError={onError}
      defaultRoute={defaultRoute}
    />
  );
}

beforeEach(() => {
  m.routeMove.mockImplementation(async (_r: RoleName, from: number, to: number) => {
    const [x] = be.entries.splice(from, 1);
    be.entries.splice(to, 0, x);
    return edited();
  });
  m.routeRemove.mockImplementation(async (_r: RoleName, i: number) => {
    be.entries.splice(i, 1);
    return edited();
  });
  m.routeSetEffort.mockImplementation(async () => edited());
  m.routeTryAgain.mockResolvedValue(undefined);
  m.routeReset.mockImplementation(async () => edited());
  m.listModels.mockResolvedValue([]);
});
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

const button = (name: string | RegExp) => screen.getByRole("button", { name });
const live = () =>
  screen
    .getAllByRole("status")
    .map((n) => n.textContent ?? "")
    .join(" | ");

describe("a role's list as it is drawn", () => {
  it("shows the heading, the one visible rule, an ordered list and Add a model", () => {
    render(<Harness entries={[orRow("anthropic/claude-sonnet-4.6")]} />);
    const section = screen.getByRole("region", { name: "Chat model" });
    expect(
      within(section).getByText(
        "PM tries these top to bottom. A model that isn't here is never used.",
      ),
    ).toBeTruthy();
    const list = within(section).getByRole("list", { name: "Chat model" });
    expect(within(list).getAllByRole("listitem")).toHaveLength(1);
    expect(button("Add a model to Chat")).toBeTruthy();
    // Everything else is folded: the fold is a disclosure, never the status.
    expect(screen.getByText("How the order works")).toBeTruthy();
  });

  it("says PM couldn't read the list, with a way to try again, when no list could be read", () => {
    const onChanged = vi.fn();
    render(
      <RoleRoute
        role="chat"
        route={null}
        failed
        dp={null}
        openRouterKey
        local={{ on: false, model: null }}
        helpId="settings-chat-models"
        onRoute={() => {}}
        onChanged={onChanged}
        onError={onError}
      />,
    );
    expect(screen.getByRole("alert").textContent).toContain("PM couldn't read this list.");
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(onChanged).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("list")).toBeNull();
  });

  it("makes the heading a target for the 'Edit my list' deep link, never a tab stop", () => {
    render(<Harness entries={[orRow("anthropic/claude-sonnet-4.6")]} />);
    expect(screen.getByRole("heading", { name: "Chat model" }).getAttribute("tabindex")).toBe("-1");
  });

  it("draws nothing for the list until the backend has answered", () => {
    render(
      <RoleRoute
        role="chat"
        route={null}
        dp={null}
        openRouterKey
        local={{ on: false, model: null }}
        helpId="settings-chat-models"
        onRoute={() => {}}
        onChanged={() => {}}
        onError={onError}
      />,
    );
    expect(screen.getByRole("heading", { name: "Chat model" })).toBeTruthy();
    expect(screen.queryByRole("list")).toBeNull();
    expect(screen.queryByRole("button", { name: /Add a model/ })).toBeNull();
  });

  it("numbers the rows, disables an end's Move button, and the only row's Remove says why it does nothing", () => {
    render(<Harness entries={[orRow("a/one"), orRow("a/two"), orRow("a/three")]} />);
    expect((button("Move a/one up, now 1 of 3") as HTMLButtonElement).disabled).toBe(true);
    expect((button("Move a/one down, now 1 of 3") as HTMLButtonElement).disabled).toBe(false);
    expect((button("Move a/three down, now 3 of 3") as HTMLButtonElement).disabled).toBe(true);
    expect((button("Remove a/two") as HTMLButtonElement).disabled).toBe(false);
    cleanup();
    render(<Harness entries={[orRow("a/only")]} />);
    // R11: this used to pin `disabled`, which takes the button out of the tab order, so a keyboard
    // or screen-reader user never heard why. It is `aria-disabled`, focusable and described now.
    const only = button("Remove a/only") as HTMLButtonElement;
    expect(only.disabled).toBe(false);
    expect(only.getAttribute("aria-disabled")).toBe("true");
    // Round 2: described by a LIST of ids (the row's model note, when it has one and no effort
    // control carries it, rides beside the "keep one" text instead of being replaced by it).
    const ids = (only.getAttribute("aria-describedby") ?? "").split(" ").filter(Boolean);
    const texts = ids.map((id) => document.getElementById(id)?.textContent);
    expect(texts).toContain("A list keeps at least one model.");
    fireEvent.click(only);
    expect(m.routeRemove).not.toHaveBeenCalled();
  });

  it("says OpenRouter rows sit together, under the second and later of a run only", () => {
    render(<Harness entries={[orRow("a/one"), orRow("a/two"), claudeRow(), orRow("a/four")]} />);
    const line = "Sent together with the row above; OpenRouter tries them in this order";
    expect(screen.getAllByText(line)).toHaveLength(1);
    const items = screen.getAllByRole("listitem");
    expect(within(items[1]).getByText(line)).toBeTruthy();
    expect(within(items[3]).queryByText(line)).toBeNull();
  });

  it("puts the I-13 tag and the provider's own warning on a direct row, and on no other", () => {
    render(
      <Harness
        entries={[claudeRow(), orRow("a/one")]}
        dp={directState({ view: anthropicView() })}
      />,
    );
    const [direct, or] = screen.getAllByRole("listitem");
    expect(
      within(direct).getByText(/Sent straight to Anthropic, not through OpenRouter/),
    ).toBeTruthy();
    expect(within(direct).getByText(/This isn't zero data retention/)).toBeTruthy();
    expect(within(or).queryByText(/Sent straight to/)).toBeNull();
    expect(within(direct).getByText("Claude, your key")).toBeTruthy();
    expect(within(or).getByText("OpenRouter")).toBeTruthy();
  });

  it("gives this computer an ordinary row with what happens when it fails", () => {
    render(
      <Harness entries={[localRow(), orRow("a/one")]} local={{ on: true, model: "llama3.2" }} />,
    );
    const [local] = screen.getAllByRole("listitem");
    expect(within(local).getByText("This computer")).toBeTruthy();
    expect(
      within(local).getByText("If this computer can't answer, the next row is tried."),
    ).toBeTruthy();
    expect(
      (button("Move llama3.2 on this computer down, now 1 of 2") as HTMLButtonElement).disabled,
    ).toBe(false);
    cleanup();
    render(
      <Harness entries={[orRow("a/one"), localRow()]} local={{ on: true, model: "llama3.2" }} />,
    );
    expect(screen.getByText("If this computer can't answer, nothing else is tried.")).toBeTruthy();
  });
});

describe("every status is said in words", () => {
  const cases: [RouteRow, string][] = [
    [claudeRow(), "Ready"],
    [claudeRow({ status: { kind: "no_key" } }), "No key saved"],
    [claudeRow({ status: { kind: "not_on" } }), "Turn on Anthropic in Your own keys"],
    [claudeRow({ status: { kind: "key_unreadable" } }), "PM can't read the key right now"],
    [claudeRow({ status: { kind: "paused_region" } }), "Paused where you are"],
    [claudeRow({ status: { kind: "paused_limit" } }), "Paused for today"],
    [localRow({ status: { kind: "local_off" } }), "Local AI is off"],
    [localRow({ status: { kind: "no_local_model" } }), "No model chosen on this computer"],
  ];
  for (const [row, text] of cases) {
    it(`${row.status.kind}: ${text}`, () => {
      render(<Harness entries={[row, orRow("a/one")]} />);
      expect(within(screen.getAllByRole("listitem")[0]).getByText(text)).toBeTruthy();
    });
  }

  it("not working says since when and why, and Try again now clears the mark", async () => {
    const since = "2026-11-28T14:05:00";
    render(
      <Harness
        entries={[
          claudeRow({ status: { kind: "not_working", reason: "busy", since } }),
          orRow("a/one"),
        ]}
      />,
    );
    const item = screen.getAllByRole("listitem")[0];
    expect(item.textContent).toMatch(/Not working since \d{1,2}:\d{2}.*: busy/);
    fireEvent.click(button("Try again now, Claude Sonnet 5.5"));
    await waitFor(() => expect(m.routeTryAgain).toHaveBeenCalledWith("anthropic"));
    await waitFor(() => expect(onChanged).toHaveBeenCalled());
  });

  it("is never colour alone: a row that can't answer has its words in the text", () => {
    render(<Harness entries={[claudeRow({ status: { kind: "paused_limit" } }), orRow("a/one")]} />);
    expect(screen.getAllByRole("listitem")[0].textContent).toContain("Paused for today");
  });
});

describe("effort and the Fable note", () => {
  it("has one effort Select per direct row, labelled with the model, and writes its change", async () => {
    render(
      <Harness
        entries={[claudeRow(), orRow("a/one")]}
        dp={directState({ view: anthropicView() })}
      />,
    );
    const select = screen.getByLabelText("Effort for Claude Sonnet 5.5") as HTMLSelectElement;
    expect(screen.queryByLabelText("Effort for a/one")).toBeNull();
    fireEvent.change(select, { target: { value: "low" } });
    await waitFor(() =>
      expect(m.routeSetEffort).toHaveBeenCalledWith(
        "chat",
        0,
        "anthropic:anthropic/claude-sonnet-5-5",
        "low",
      ),
    );
  });

  it("Background's own default is not written, so the role keeps tracking it", async () => {
    render(
      <Harness
        role="background"
        entries={[claudeRow(), orRow("a/one")]}
        dp={directState({ view: anthropicView() })}
      />,
    );
    const select = screen.getByLabelText("Effort for Claude Sonnet 5.5") as HTMLSelectElement;
    fireEvent.change(select, { target: { value: "low" } });
    await waitFor(() =>
      expect(m.routeSetEffort).toHaveBeenCalledWith(
        "background",
        0,
        "anthropic:anthropic/claude-sonnet-5-5",
        null,
      ),
    );
  });

  it("shows Fable's warning and ties it to the row's effort control", () => {
    const fable = claudeRow({
      model: FABLE.id,
      name: FABLE.name,
      warning: FABLE.note ?? undefined,
    });
    render(
      <Harness entries={[fable, orRow("a/one")]} dp={directState({ view: anthropicView() })} />,
    );
    const note = screen.getByText("Anthropic keeps what you send to Fable for 30 days.");
    const select = screen.getByLabelText("Effort for Claude Fable 5.2");
    expect(select.getAttribute("aria-describedby")).toBe(note.id);
  });
});

describe("moving and removing, for the keyboard and a screen reader", () => {
  it("a move asks for one edit, announces the new place and keeps focus on the same button", async () => {
    render(<Harness entries={[orRow("a/one"), orRow("a/two"), orRow("a/three")]} />);
    fireEvent.click(button("Move a/two up, now 2 of 3"));
    await waitFor(() => expect(m.routeMove).toHaveBeenCalledWith("chat", 1, 0, "openrouter:a/two"));
    await waitFor(() => expect(live()).toContain("a/two is now number 1 of 3"));
    // It reached the top, so "up" is disabled: focus goes to the opposite button.
    await waitFor(() => expect(document.activeElement).toBe(button("Move a/two down, now 1 of 3")));
  });

  it("keeps focus on the same button when it is still enabled", async () => {
    render(<Harness entries={[orRow("a/one"), orRow("a/two"), orRow("a/three")]} />);
    fireEvent.click(button("Move a/one down, now 1 of 3"));
    await waitFor(() => expect(m.routeMove).toHaveBeenCalledWith("chat", 0, 1, "openrouter:a/one"));
    await waitFor(() => expect(document.activeElement).toBe(button("Move a/one down, now 2 of 3")));
  });

  it("an item moved to the bottom hands focus to its Move up button", async () => {
    render(<Harness entries={[orRow("a/one"), orRow("a/two")]} />);
    fireEvent.click(button("Move a/one down, now 1 of 2"));
    await waitFor(() => expect(document.activeElement).toBe(button("Move a/one up, now 2 of 2")));
  });

  // R11: this test pinned "focus goes to Add a model", which is the far end of the page from the
  // row the person just removed (WCAG 2.4.3). Focus now stays in the list, on the Remove button of
  // the row that took the removed one's place, and on the row above when the last one went.
  it("a removal says how many are left and keeps focus in the list, on the row that took its place", async () => {
    render(<Harness entries={[orRow("a/one"), orRow("a/two"), orRow("a/three")]} />);
    fireEvent.click(button("Remove a/two"));
    await waitFor(() => expect(m.routeRemove).toHaveBeenCalledWith("chat", 1, "openrouter:a/two"));
    await waitFor(() => expect(live()).toContain("Removed a/two. 2 models left in Chat"));
    await waitFor(() => expect(document.activeElement).toBe(button("Remove a/three")));
  });

  it("removing the last row puts focus on the row above it", async () => {
    render(<Harness entries={[orRow("a/one"), orRow("a/two"), orRow("a/three")]} />);
    fireEvent.click(button("Remove a/three"));
    await waitFor(() => expect(m.routeRemove).toHaveBeenCalled());
    await waitFor(() => expect(document.activeElement).toBe(button("Remove a/two")));
  });

  it("every edit that names a position also names the row, so a stale position is refused by the backend", async () => {
    render(<Harness entries={[orRow("a/one"), orRow("a/two"), orRow("a/three")]} />);
    fireEvent.click(button("Move a/three up, now 3 of 3"));
    await waitFor(() => expect(m.routeMove).toHaveBeenCalled());
    // The key is the fourth argument: the row at that place on the page, not whatever is there now.
    expect(m.routeMove.mock.calls[0]).toEqual(["chat", 2, 1, "openrouter:a/three"]);
  });

  it("the backend's 'your list changed' refusal is shown and the list is read again", async () => {
    m.routeRemove.mockRejectedValueOnce("Your list changed. Here it is again.");
    render(<Harness entries={[orRow("a/one"), orRow("a/two")]} />);
    fireEvent.click(button("Remove a/two"));
    await waitFor(() =>
      expect(onError).toHaveBeenCalledWith("Your list changed. Here it is again."),
    );
    expect(onChanged).toHaveBeenCalled();
  });

  it("a refused edit leaves the list as it was, says why and reads it again", async () => {
    m.routeMove.mockRejectedValueOnce("That list can't hold it.");
    render(<Harness entries={[orRow("a/one"), orRow("a/two")]} />);
    fireEvent.click(button("Move a/one down, now 1 of 2"));
    await waitFor(() => expect(onError).toHaveBeenCalledWith("That list can't hold it."));
    expect(onChanged).toHaveBeenCalled();
    const names = screen.getAllByRole("listitem").map((li) => li.getAttribute("data-row-key"));
    expect(names).toEqual(["openrouter:a/one", "openrouter:a/two"]);
  });

  it("drops a second click while an edit is on its way, so two edits never come from one old list", async () => {
    let finish: (e: RouteEdited) => void = () => {};
    m.routeMove.mockImplementationOnce(() => new Promise<RouteEdited>((r) => (finish = r)));
    render(<Harness entries={[orRow("a/one"), orRow("a/two"), orRow("a/three")]} />);
    fireEvent.click(button("Move a/one down, now 1 of 3"));
    fireEvent.click(button("Move a/three up, now 3 of 3"));
    expect(m.routeMove).toHaveBeenCalledTimes(1);
    await act(async () => finish(edited()));
  });

  it("tab order inside a row is effort, up, down, remove", () => {
    render(
      <Harness
        entries={[claudeRow(), orRow("a/one")]}
        dp={directState({ view: anthropicView() })}
      />,
    );
    const item = screen.getAllByRole("listitem")[0];
    const order = [...item.querySelectorAll("select, button")].map(
      (el) => el.getAttribute("aria-label") ?? el.textContent,
    );
    expect(order).toEqual([
      "Effort for Claude Sonnet 5.5",
      "Move Claude Sonnet 5.5 up, now 1 of 2",
      "Move Claude Sonnet 5.5 down, now 1 of 2",
      "Remove Claude Sonnet 5.5",
    ]);
  });

  it("names every control, ids nothing twice, and keeps one polite status region", () => {
    const { container } = render(
      <Harness
        entries={[claudeRow(), geminiRow(), orRow("a/one"), localRow()]}
        dp={stateWith([anthropicView(), openaiView()])}
        local={{ on: true, model: "llama3.2" }}
      />,
    );
    for (const el of container.querySelectorAll("button, select")) {
      const name = el.getAttribute("aria-label") ?? el.textContent ?? "";
      expect(name.trim().length, el.outerHTML).toBeGreaterThan(0);
    }
    const ids = [...container.querySelectorAll("[id]")].map((e) => e.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(screen.getAllByRole("status")).toHaveLength(1);
    expect(screen.getByRole("list", { name: "Chat model" }).tagName).toBe("OL");
  });
});

describe("the hint, the first edit and the mods line", () => {
  it("hints at a second model only when something else could answer", () => {
    const hint = "Add a second model so a reply doesn't stop when this one is down.";
    render(
      <Harness
        entries={[claudeRow()]}
        dp={directState({ view: anthropicView() })}
        openRouterKey={false}
      />,
    );
    // OpenAI is not in the state, no OpenRouter key and no local model: nothing else is usable.
    expect(screen.queryByText(hint)).toBeNull();
    cleanup();
    render(
      <Harness entries={[claudeRow()]} dp={directState({ view: anthropicView() })} openRouterKey />,
    );
    expect(screen.getByText(hint)).toBeTruthy();
    cleanup();
    render(<Harness entries={[claudeRow(), orRow("a/one")]} openRouterKey />);
    expect(screen.queryByText(hint)).toBeNull();
  });

  it("says how many saved models the first edit left out, under the list and aloud", async () => {
    m.routeRemove.mockResolvedValueOnce({
      route: routeView([orRow("a/one")]),
      materialised: true,
      unused_saved: 3,
    });
    render(<Harness entries={[orRow("a/one"), orRow("a/two")]} />);
    fireEvent.click(button("Remove a/two"));
    const line = "3 other saved models weren't in use and aren't in this list.";
    await waitFor(() => expect(screen.getAllByText(line).length).toBeGreaterThan(0));
    expect(live()).toContain(line);
  });

  it("says one saved model in the singular, and nothing for none", async () => {
    m.routeRemove.mockResolvedValueOnce({
      route: routeView([orRow("a/one")]),
      materialised: true,
      unused_saved: 1,
    });
    render(<Harness entries={[orRow("a/one"), orRow("a/two")]} />);
    fireEvent.click(button("Remove a/two"));
    await waitFor(() =>
      expect(
        screen.getAllByText("1 other saved model wasn't in use and isn't in this list.").length,
      ).toBeGreaterThan(0),
    );
  });

  it("tells Background that mods get no answer while its first cloud row is a direct provider", () => {
    render(
      <Harness
        role="background"
        entries={[claudeRow(), orRow("a/one")]}
        dp={directState({ view: anthropicView() })}
      />,
    );
    expect(
      screen.getByText(
        "Mods can't use your own keys. While Background is on Anthropic, a mod that asks for AI gets no answer.",
      ),
    ).toBeTruthy();
    cleanup();
    render(<Harness role="background" entries={[orRow("a/one"), claudeRow()]} />);
    expect(screen.queryByText(/Mods can't use your own keys/)).toBeNull();
    cleanup();
    render(<Harness role="chat" entries={[claudeRow()]} />);
    expect(screen.queryByText(/Mods can't use your own keys/)).toBeNull();
  });

  it("offers Reset only when the list differs from a fresh install's", () => {
    const def = routeView([orRow("anthropic/claude-sonnet-4.6")]);
    render(<Harness entries={[orRow("anthropic/claude-sonnet-4.6")]} defaultRoute={def} />);
    expect(screen.queryByRole("button", { name: "Reset" })).toBeNull();
    cleanup();
    render(<Harness entries={[orRow("a/one"), orRow("a/two")]} defaultRoute={def} />);
    expect(screen.getByRole("button", { name: "Reset" })).toBeTruthy();
  });
});

describe("Add a model", () => {
  const dp = () =>
    stateWith([anthropicView(), openaiView({ opted_in: false, key: "absent", key_check: null })]);

  it("opens a real, named dialog with one section per provider and its own eligibility line", () => {
    render(<Harness entries={[orRow("a/one")]} dp={dp()} local={{ on: false, model: null }} />);
    fireEvent.click(button("Add a model to Chat"));
    const dialog = screen.getByRole("dialog", { name: "Add a model to Chat" });
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    for (const name of ["OpenRouter", "Anthropic", "OpenAI", "This computer"]) {
      expect(within(dialog).getByRole("heading", { name })).toBeTruthy();
    }
    expect(within(dialog).getByText("Turn on OpenAI in Your own keys first")).toBeTruthy();
    // N4: the section says the way out, not only the state.
    expect(
      within(dialog).getByText("Local AI is off. Turn it on just below the lists first."),
    ).toBeTruthy();
  });

  it("calls a Local AI server on another machine that, in the picker, not This computer", () => {
    render(
      <Harness entries={[orRow("a/one")]} local={{ on: true, model: null, elsewhere: true }} />,
    );
    fireEvent.click(button("Add a model to Chat"));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByRole("heading", { name: "Your Local AI server" })).toBeTruthy();
    expect(within(dialog).queryByRole("heading", { name: "This computer" })).toBeNull();
  });

  it("words each reason a section can't be used", () => {
    const state = stateWith([
      anthropicView({ key: "absent", key_check: null }),
      openaiView({
        blocked: { kind: "uk", label: "the UK", zone: "Europe/London", basis: "free" },
      }),
    ]);
    render(<Harness entries={[orRow("a/one")]} dp={state} local={{ on: true, model: null }} />);
    fireEvent.click(button("Add a model to Chat"));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText("No key saved")).toBeTruthy();
    expect(within(dialog).getByText("Paused where you are")).toBeTruthy();
    expect(within(dialog).getByText("Pick a local model in Local AI first")).toBeTruthy();
  });

  it("shows a model already in the list disabled, and never Mythos", () => {
    const state = directState({
      view: anthropicView({
        models: [
          ...anthropicView().models,
          { ...FABLE, id: "anthropic/claude-mythos-1", name: "Claude Mythos 1" },
        ],
      }),
    });
    render(<Harness entries={[claudeRow()]} dp={state} />);
    fireEvent.click(button("Add a model to Chat"));
    const dialog = screen.getByRole("dialog");
    const sonnet = within(dialog).getByRole("button", {
      name: /Claude Sonnet 5\.5/,
    }) as HTMLButtonElement;
    // N5 (R11): `aria-disabled`, not `disabled`, so the button stays reachable and "Already in your
    // list" is read as part of its name; the click does nothing.
    expect(sonnet.disabled).toBe(false);
    expect(sonnet.getAttribute("aria-disabled")).toBe("true");
    expect(sonnet.textContent).toContain("Already in your list");
    fireEvent.click(sonnet);
    expect(m.routeAdd).not.toHaveBeenCalled();
    expect(within(dialog).queryByText(/Mythos/)).toBeNull();
    // A direct provider has no custom id.
    expect(within(dialog).queryByText(/custom model id/)).toBeNull();
  });

  it("keeps this computer's button reachable, and inert, when it is already in the list", () => {
    render(
      <Harness entries={[orRow("a/one"), localRow()]} local={{ on: true, model: "llama3.2" }} />,
    );
    fireEvent.click(button("Add a model to Chat"));
    const dialog = screen.getByRole("dialog");
    const local = within(dialog).getByRole("button", { name: /llama3\.2/ }) as HTMLButtonElement;
    expect(local.disabled).toBe(false);
    expect(local.getAttribute("aria-disabled")).toBe("true");
    expect(local.textContent).toContain("Already in your list");
    fireEvent.click(local);
    expect(m.routeAdd).not.toHaveBeenCalled();
  });

  it("adds a pick at the end, closes, announces it and returns focus to Add a model", async () => {
    m.routeAdd.mockImplementation(async (_r, p, model) => {
      be.entries.push(claudeRow({ model, name: "Claude Haiku 5.5", key: `${p}:${model}` }));
      return edited();
    });
    render(<Harness entries={[orRow("a/one")]} dp={directState({ view: anthropicView() })} />);
    fireEvent.click(button("Add a model to Chat"));
    fireEvent.click(
      within(screen.getByRole("dialog")).getByRole("button", { name: /Claude Haiku 5\.5/ }),
    );
    await waitFor(() =>
      expect(m.routeAdd).toHaveBeenCalledWith("chat", "anthropic", "anthropic/claude-haiku-5-5"),
    );
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(live()).toContain("Added Claude Haiku 5.5 at the end");
    await waitFor(() => expect(document.activeElement).toBe(button("Add a model to Chat")));
  });

  it("adds this computer once, with no model chosen by the page", async () => {
    m.routeAdd.mockImplementation(async () => {
      be.entries.push(localRow());
      return edited();
    });
    render(<Harness entries={[orRow("a/one")]} local={{ on: true, model: "llama3.2" }} />);
    fireEvent.click(button("Add a model to Chat"));
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: /llama3\.2/ }));
    await waitFor(() => expect(m.routeAdd).toHaveBeenCalledWith("chat", "local", ""));
  });

  it("keeps the dialog open and says why when a pick is refused", async () => {
    m.routeAdd.mockRejectedValueOnce("Anthropic is turned off.");
    render(<Harness entries={[orRow("a/one")]} dp={directState({ view: anthropicView() })} />);
    fireEvent.click(button("Add a model to Chat"));
    fireEvent.click(
      within(screen.getByRole("dialog")).getByRole("button", { name: /Claude Haiku 5\.5/ }),
    );
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toContain("Anthropic is turned off."),
    );
    expect(screen.getByRole("dialog")).toBeTruthy();
  });

  it("closes on Cancel and gives focus back to the button that opened it", async () => {
    render(<Harness entries={[orRow("a/one")]} dp={dp()} />);
    // A real click focuses the button first; fireEvent does not.
    button("Add a model to Chat").focus();
    fireEvent.click(button("Add a model to Chat"));
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(button("Add a model to Chat")));
  });
});
