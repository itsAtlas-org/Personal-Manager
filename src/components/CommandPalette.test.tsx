// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The palette's dialog semantics, which it had been hand-rolling and getting wrong in one specific
// way: Escape was a React `onKeyDown` on the `<input>`, and the result rows are `tabIndex={-1}`, so
// the FIRST Tab moved focus out of the palette entirely — and from there a keydown on a background
// control never bubbled back to the input. Escape was dead, with the palette still on screen over a
// scrim, and the only way out was the mouse.
//
// Wearing `Modal` fixes that (its Escape is a window listener) and brings the rest of the shell with
// it: a `role="dialog"` with a name, and focus handed back to whatever opened the palette. Those are
// what these tests pin — not the fuzzy matcher, which is pure and belongs to its own unit.

import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const listConversations = vi.fn();
const listDocuments = vi.fn();
const listProjectOverviews = vi.fn();
const searchCalendarEvents = vi.fn();
const searchPeople = vi.fn();
// The Calendar tab's hidden calendars live in the encrypted store, which no test hydrates.
const hiddenCalendars = vi.hoisted(() => ({ ids: new Set<string>() }));
vi.mock("../lib/calendarPrefs", () => ({ readHidden: () => hiddenCalendars.ids }));

vi.mock("../lib/ipc", () => ({
  listConversations: () => listConversations(),
  listDocuments: () => listDocuments(),
  listProjectOverviews: () => listProjectOverviews(),
  searchCalendarEvents: (query: string, hidden: string[]) => searchCalendarEvents(query, hidden),
  searchPeople: (query: string, limit: number) => searchPeople(query, limit),
}));

// The Depth and developer mode the "Go to" list is gated on (lib/nav), set per test.
const gates = vi.hoisted(() => ({ depth: "standard", devMode: false }));

// The same stub the other component tests use: the palette's primitives reach for `useTheme`, and
// the real ThemeProvider pulls in IPC.
vi.mock("../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({
    system: "slate",
    mode: "dark",
    modePref: "system",
    modeSource: "system",
    accent: "mono",
    depth: gates.depth,
    autoLocation: "",
    setSystem: () => {},
    setModePref: () => {},
    setAccent: () => {},
    setDepth: () => {},
    setAutoLocation: () => {},
  }),
}));

vi.mock("../lib/capabilities", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useDevMode: () => ({ devMode: gates.devMode, setDevMode: () => {} }),
}));

import { CommandPalette } from "./CommandPalette";
import type { CalendarEvent, PersonRow } from "../lib/types";
import { writeNavPlacement } from "../lib/nav";
import { ModRowsContext, type ModRows } from "../lib/modRows";
import type { ModInfo } from "../lib/modsTypes";

const noop = () => {};

function renderPalette(
  onClose: () => void,
  onOpenEvent: (event: CalendarEvent) => void = noop,
  onOpenPerson: (uid: string) => void = noop,
) {
  return render(
    <CommandPalette
      onClose={onClose}
      onOpenProject={noop}
      onOpenConversation={noop}
      onOpenEvent={onOpenEvent}
      onOpenPerson={onOpenPerson}
      onNavigate={noop}
      onOpenSettings={noop}
    />,
  );
}

const onNavigate = vi.fn();
const MODS: ModRows = { rows: [], loaded: true, problem: null, refresh: () => Promise.resolve() };
/** An installed mod's row, as far as the sidebar reads it. */
function modRow(id: string, navTab: string): ModInfo {
  return { id, name: navTab, navTab } as ModInfo;
}

/** Something outside the palette to hold focus — the position a single Tab used to land on. */
function outsideButton(label: string): HTMLButtonElement {
  const el = document.createElement("button");
  el.textContent = label;
  document.body.appendChild(el);
  return el;
}

beforeEach(() => {
  listConversations.mockResolvedValue([]);
  listDocuments.mockResolvedValue([]);
  listProjectOverviews.mockResolvedValue([]);
  searchCalendarEvents.mockResolvedValue([]);
  searchPeople.mockResolvedValue([]);
  // jsdom implements no layout, so it has no `scrollIntoView` at all; the palette keeps the
  // highlighted row in view on every arrow press. Same stub as ChatView.test.tsx.
  Element.prototype.scrollIntoView = vi.fn();
});

afterEach(() => {
  cleanup();
  document.body.innerHTML = "";
  vi.clearAllMocks();
  hiddenCalendars.ids = new Set();
  localStorage.clear();
  gates.depth = "standard";
  gates.devMode = false;
});

/** The "Go to" rows, in order. */
async function gotoLabels(): Promise<string[]> {
  const { findByRole } = renderPalette(noop);
  await waitFor(() => expect(listDocuments).toHaveBeenCalled());
  const group = await findByRole("group", { name: "Go to" });
  return [...group.querySelectorAll('[role="option"]')].map(
    (o) => o.querySelector("span[title]")?.textContent ?? "",
  );
}

// "Go to" reads lib/nav, the list the sidebar draws, so the two can't drift apart again.
describe("CommandPalette's Go to", () => {
  it("offers what the sidebar always did, then Settings", async () => {
    expect(await gotoLabels()).toEqual([
      "Focus",
      "Chats",
      "Calendar",
      "People",
      "Documents",
      "Review",
      "Teach",
      "Mods",
      "Settings",
    ]);
  });

  it("leaves out what Minimal leaves out, as it did", async () => {
    gates.depth = "min";
    expect(await gotoLabels()).toEqual([
      "Focus",
      "Chats",
      "Calendar",
      "People",
      "Documents",
      "Mods",
      "Settings",
    ]);
  });

  it("still offers a tab hidden from the sidebar, after the sidebar's own", async () => {
    writeNavPlacement({ hidden: ["calendar"], pinned: ["documents"] });
    expect(await gotoLabels()).toEqual([
      "Documents",
      "Focus",
      "Chats",
      "People",
      "Review",
      "Teach",
      "Mods",
      "Calendar",
      "Settings",
    ]);
  });

  it("offers an installed mod's tab, marked as a mod's, and still offers it once hidden", async () => {
    const withMod = () => (
      <ModRowsContext.Provider value={{ ...MODS, rows: [modRow("acme.tool", "Acme Tool")] }}>
        <CommandPalette
          onClose={noop}
          onOpenProject={noop}
          onOpenConversation={noop}
          onOpenEvent={noop}
          onOpenPerson={noop}
          onNavigate={onNavigate}
          onOpenSettings={noop}
        />
      </ModRowsContext.Provider>
    );
    const { findByRole } = render(withMod());
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());
    const group = await findByRole("group", { name: "Go to" });
    const row = [...group.querySelectorAll('[role="option"]')].find((o) =>
      o.textContent?.includes("Acme Tool"),
    ) as HTMLElement;
    // Marked, so a mod can't pass its tab off as one of PM's by what it calls it.
    expect(row.textContent).toContain("Mod");
    fireEvent.click(row);
    expect(onNavigate).toHaveBeenCalledWith("mod:acme.tool");

    cleanup();
    writeNavPlacement({ hidden: ["mod:acme.tool"], pinned: [] });
    const again = render(withMod());
    const hiddenGroup = await again.findByRole("group", { name: "Go to" });
    expect(hiddenGroup.textContent).toContain("Acme Tool");
  });

  it("offers Dev only in developer mode", async () => {
    expect(await gotoLabels()).not.toContain("Dev");
    cleanup();
    gates.devMode = true;
    expect(await gotoLabels()).toContain("Dev");
  });
});

function event(id: string, summary: string, calendarId: string): CalendarEvent {
  return {
    id,
    calendar_id: calendarId,
    summary,
    start: "2026-11-09T09:00:00Z",
    all_day: false,
    recurring: true,
  } as CalendarEvent;
}

describe("CommandPalette", () => {
  it("closes on Escape from a focus position that is NOT the input", async () => {
    // The regression this conversion exists for. Focus is deliberately moved off the input first:
    // with the old input-bound handler this Escape reached nothing at all.
    const onClose = vi.fn();
    renderPalette(onClose);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());

    const elsewhere = outsideButton("behind the palette");
    elsewhere.focus();
    expect(document.activeElement).toBe(elsewhere);

    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("still closes on Escape while the input holds focus", async () => {
    // The path that already worked must keep working — the fix removed the input's own branch.
    const onClose = vi.fn();
    const { getByRole } = renderPalette(onClose);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());

    const input = getByRole("combobox");
    expect(document.activeElement).toBe(input);

    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("keeps Tab on the query field instead of letting it leave", async () => {
    // The rows are `tabIndex={-1}` and selected via `aria-activedescendant`, so the query field is
    // the palette's ONLY tab stop — Tab should return to it, not walk out into the app behind the
    // scrim. (`useFocusTrap` had to learn that a tabindex="-1" button is not a stop; see its test.)
    const { getByRole } = renderPalette(noop);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());

    const input = getByRole("combobox");
    fireEvent.keyDown(input, { key: "Tab" });
    expect(document.activeElement).toBe(input);
  });

  it("is a named dialog, and the input keeps its own field name", async () => {
    // Two different names, both required: `aria-label` on a combobox names the FIELD, so it could
    // never have been the dialog's name. The palette had no dialog role at all before this.
    const { getByRole } = renderPalette(noop);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());

    expect(getByRole("dialog", { name: "Command palette" })).toBeTruthy();
    expect(
      getByRole("combobox", { name: "Search projects, files, conversations, events and people" }),
    ).toBeTruthy();
  });

  it("hands focus back to whatever opened it", async () => {
    // Closing used to drop focus on <body>: keyboard users landed at the top of the document.
    const opener = outsideButton("Open the palette");
    opener.focus();

    const { unmount } = renderPalette(noop);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());
    expect(document.activeElement).not.toBe(opener);

    unmount();
    expect(document.activeElement).toBe(opener);
  });

  it("keeps the help anchor on the panel itself", async () => {
    // Two things depend on where this sits. HelpOverlay resolves a hovered element with
    // `closest("[data-help]")`, so it must be an ANCESTOR of the rows and the input — and
    // `.help-mode [data-help]:hover` draws the outline that says help is available here, which
    // needs an element that actually paints a box. The dialog element is both.
    const { getByRole } = renderPalette(noop);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());

    expect(getByRole("dialog").getAttribute("data-help")).toBe("command-palette");
    expect(getByRole("combobox").closest("[data-help]")).toBe(getByRole("dialog"));
    expect(getByRole("listbox").closest("[data-help]")).toBe(getByRole("dialog"));
  });

  it("finds calendar events, leaving hidden calendars to Rust, and opens one", async () => {
    // The Search button found nothing in the calendar (#1047): events are searched in Rust as you
    // type, the Calendar tab's hidden calendars left out there (before the limit), and a hit opens
    // the Calendar on it.
    hiddenCalendars.ids = new Set(["cal-hidden"]);
    const standup = event("standup-09", "Team standup", "cal-work");
    searchCalendarEvents.mockResolvedValue([standup]);
    const onClose = vi.fn();
    const onOpenEvent = vi.fn();
    const { getByRole, findByRole } = renderPalette(onClose, onOpenEvent);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());

    fireEvent.change(getByRole("combobox"), { target: { value: "  Team " } });
    const hit = await findByRole("option", { name: /Team standup/ });
    expect(searchCalendarEvents).toHaveBeenCalledTimes(1);
    expect(searchCalendarEvents).toHaveBeenCalledWith("team", ["cal-hidden"]);
    expect(getByRole("group", { name: "Events" })).toBeTruthy();
    expect(hit.textContent).toContain("repeats");

    fireEvent.click(hit);
    expect(onOpenEvent).toHaveBeenCalledWith(standup);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("finds people through the backend's search, shows at most six, and opens one by uid", async () => {
    // The backend does the match (accents, nicknames, phone digits); the palette only asks, with
    // the words as typed, and a click or Enter opens the person by uid.
    const person = (id: number, name: string, company: string | null): PersonRow => ({
      id,
      uid: `${id}`.padStart(32, "0"),
      display_name: name,
      primary: `p${id}@example.com`,
      company,
      labels: [],
      starred: false,
      last_met: null,
      google_state: null,
      trashed_at: null,
      birthday: null,
      leap_day: null,
    });
    const found = [1, 2, 3, 4, 5, 6, 7, 8].map((i) =>
      person(i, `Sam Example ${i}`, i === 1 ? "Example Ltd" : null),
    );
    searchPeople.mockResolvedValue(found);
    const onClose = vi.fn();
    const onOpenPerson = vi.fn();
    const { getByRole, findByRole } = renderPalette(onClose, noop, onOpenPerson);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());
    const box = getByRole("combobox");

    // Blanks are nothing, so nothing is searched.
    fireEvent.change(box, { target: { value: "  " } });
    await new Promise((r) => setTimeout(r, 250));
    expect(searchPeople).not.toHaveBeenCalled();

    fireEvent.change(box, { target: { value: "  Sam " } });
    const group = await findByRole("group", { name: "People" });
    expect(searchPeople).toHaveBeenCalledTimes(1);
    expect(searchPeople.mock.calls[0][0]).toBe("sam");
    const options = [...group.querySelectorAll('[role="option"]')];
    expect(options).toHaveLength(6);
    expect(options[0].textContent).toContain("Sam Example 1");
    expect(options[0].textContent).toContain("Example Ltd");
    expect(options[1].textContent).toContain("p2@example.com");

    fireEvent.click(options[2]);
    expect(onOpenPerson).toHaveBeenCalledWith(found[2].uid);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("searches people by the words as typed: an email keeps its @, one character is enough", async () => {
    const { getByRole } = renderPalette(noop, noop, vi.fn());
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());
    const box = getByRole("combobox");

    fireEvent.change(box, { target: { value: " Ada@Example.com " } });
    await waitFor(() => expect(searchPeople).toHaveBeenCalledWith("ada@example.com", 6));

    fireEvent.change(box, { target: { value: "山" } });
    await waitFor(() => expect(searchPeople).toHaveBeenCalledWith("山", 6));
  });

  it("opens the highlighted person with Enter", async () => {
    searchPeople.mockResolvedValue([
      {
        id: 9,
        uid: "9".padStart(32, "0"),
        display_name: "Noor Example",
        primary: null,
        company: null,
        labels: [],
        starred: false,
        last_met: null,
        google_state: null,
        trashed_at: null,
        birthday: null,
        leap_day: null,
      } satisfies PersonRow,
    ]);
    const onOpenPerson = vi.fn();
    const { getByRole, findByRole } = renderPalette(noop, noop, onOpenPerson);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());
    fireEvent.change(getByRole("combobox"), { target: { value: "noor" } });
    await findByRole("option", { name: /Noor Example/ });
    fireEvent.keyDown(getByRole("combobox"), { key: "Enter" });
    expect(onOpenPerson).toHaveBeenCalledWith("9".padStart(32, "0"));
  });

  it("never shows an older search's answer under a newer query", async () => {
    const answers = new Map<string, (events: CalendarEvent[]) => void>();
    searchCalendarEvents.mockImplementation(
      (query: string) => new Promise<CalendarEvent[]>((resolve) => answers.set(query, resolve)),
    );
    const { getByRole, findByRole, queryByRole } = renderPalette(noop);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());
    const box = getByRole("combobox");

    fireEvent.change(box, { target: { value: "te" } });
    await waitFor(() => expect(answers.has("te")).toBe(true));
    fireEvent.change(box, { target: { value: "team" } });
    await waitFor(() => expect(answers.has("team")).toBe(true));
    answers.get("team")?.([event("standup", "Team standup", "cal-work")]);
    await findByRole("option", { name: /Team standup/ });
    answers.get("te")?.([event("tea", "Tea with Noor", "cal-work")]);
    await new Promise((r) => setTimeout(r, 50));
    expect(queryByRole("option", { name: /Tea with Noor/ })).toBeNull();
  });

  it("keeps the arrowed row highlighted when the calendar's hits land, and keeps hits meanwhile", async () => {
    // An index used to snap back to the top when the hits landed, under a row already picked; and
    // the Events group blinked out on every keystroke until the next answer.
    listDocuments.mockResolvedValue([
      { id: 1, title: "Team notes", project: "Atlas", linked_projects: [], tags: [] },
      { id: 2, title: "Team plan", project: "Atlas", linked_projects: [], tags: [] },
    ]);
    const answers = new Map<string, (events: CalendarEvent[]) => void>();
    searchCalendarEvents.mockImplementation(
      (query: string) => new Promise<CalendarEvent[]>((resolve) => answers.set(query, resolve)),
    );
    const { getByRole, findByRole } = renderPalette(noop);
    await findByRole("option", { name: /Team notes/ });
    const box = getByRole("combobox");

    fireEvent.change(box, { target: { value: "team" } });
    fireEvent.keyDown(box, { key: "ArrowDown" });
    const plan = getByRole("option", { name: /Team plan/ });
    expect(plan.getAttribute("aria-selected")).toBe("true");
    await waitFor(() => expect(answers.has("team")).toBe(true));
    answers.get("team")?.([event("standup", "Team standup", "cal-work")]);
    await findByRole("option", { name: /Team standup/ });
    expect(getByRole("option", { name: /Team plan/ }).getAttribute("aria-selected")).toBe("true");

    // A longer query: the hit that still holds every word stays while its own search is on the way.
    fireEvent.change(box, { target: { value: "team stand" } });
    expect(getByRole("option", { name: /Team standup/ })).toBeTruthy();
  });

  it("doesn't search the calendar for a single letter", async () => {
    const { getByRole } = renderPalette(noop);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());
    fireEvent.change(getByRole("combobox"), { target: { value: "t" } });
    await new Promise((r) => setTimeout(r, 300));
    expect(searchCalendarEvents).not.toHaveBeenCalled();
  });
});
