// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The People tab's timeline and the meetings owed an answer (Contacts, slice 7b): the record's
// Timeline section (last met or "Not met yet", entries newest first, + Log, Edit, Delete with its
// question, earlier pages), "Log a meeting" from the record and from the row's right-click menu,
// and the owed list at the top of the All view. The clock is pinned, expected clocks go through
// formatClock (CI is 12-hour en-US), and every person is invented.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { formatClock } from "../../lib/format";
import type { OwedMeeting, Person, PersonRow, TimelineEntry } from "../../lib/types";

const store = vi.hoisted(() => ({ person: null as unknown }));
const ipc = vi.hoisted(() => ({
  listPeople: vi.fn(),
  searchPeople: vi.fn(),
  listLabels: vi.fn(),
  getPerson: vi.fn(),
  listTimeline: vi.fn(),
  addInteraction: vi.fn(),
  updateInteraction: vi.fn(),
  deleteInteraction: vi.fn(),
  // The people notice's listeners, so a test can say "something changed".
  listeners: [] as (() => void)[],
}));
vi.mock("../../lib/ipc", () => {
  const known: Record<string, unknown> = {
    ...ipc,
    onPeopleChanged: vi.fn(async (h: () => void) => {
      ipc.listeners.push(h);
      return () => {};
    }),
    getPref: vi.fn(async () => null),
    setPref: vi.fn(async () => {}),
  };
  return new Proxy(known, {
    has: () => true,
    get(target, prop: string) {
      if (prop === "then") return undefined;
      if (!(prop in target)) {
        target[prop] = /^on[A-Z]/.test(prop) ? vi.fn(async () => () => {}) : vi.fn(async () => []);
      }
      return target[prop];
    },
  });
});

import { closeContextMenu } from "../../lib/contextMenu";
import { ThemeProvider } from "../../theme";
import { ContextMenuHost } from "../ui/ContextMenuHost";
import { PeopleView } from "./PeopleView";

const ADA_ROW: PersonRow = {
  id: 1,
  uid: "1".padStart(32, "0"),
  display_name: "Ada Example",
  primary: null,
  company: null,
  labels: [],
  starred: false,
  last_met: "2026-11-12",
  google_state: null,
  trashed_at: null,
  birthday: null,
  leap_day: null,
};

const entry = (over: Partial<TimelineEntry> = {}): TimelineEntry => ({
  id: 10,
  kind: "met",
  on_date: "2026-11-12",
  at: null,
  note: "Lunch by the canal",
  event_uid: null,
  event_start: null,
  event_title: null,
  others: [],
  created_at: "2026-11-12T12:00:00Z",
  ...over,
});

function personWith(entries: TimelineEntry[], over: Partial<Person> = {}, next = null): Person {
  return {
    id: 1,
    uid: ADA_ROW.uid,
    display_name: "Ada Example",
    given_name: "",
    middle_name: "",
    family_name: "",
    name_prefix: "",
    name_suffix: "",
    birthday: null,
    met_where: "",
    met_on: null,
    notes: "",
    starred: false,
    trashed_at: null,
    created_at: "2026-11-01T10:00:00Z",
    updated_at: "2026-11-20T10:00:00Z",
    details: [],
    labels: [],
    last_met: entries.find((e) => e.kind === "met")?.on_date ?? null,
    timeline: { entries, next },
    sources: [],
    projects: [],
    ...over,
  } as Person;
}

const owedMeeting = (over: Partial<OwedMeeting> = {}): OwedMeeting => ({
  uid: "evt-1@example.com",
  start: "2026-11-25T09:00:00Z",
  end: "2026-11-25T09:30:00Z",
  title: "Planning",
  all_day: false,
  recurs: false,
  known: [],
  unknown: [],
  ...over,
});

async function view(props: Partial<React.ComponentProps<typeof PeopleView>> = {}) {
  render(
    <ThemeProvider>
      <PeopleView zone="Etc/UTC" {...props} />
      <ContextMenuHost />
    </ThemeProvider>,
  );
  await act(async () => {});
}

/** Open Ada's record from the list. */
async function openAda() {
  fireEvent.click(screen.getByText("Ada Example"));
  await screen.findByRole("region", { name: "Timeline" });
}

const timeline = () => screen.getByRole("region", { name: "Timeline" });
const dialog = (name: string | RegExp) => screen.getByRole("dialog", { name });

beforeAll(() => {
  if (!("ResizeObserver" in globalThis)) {
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
  Element.prototype.scrollIntoView ??= () => {};
});

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(new Date("2026-11-26T12:00:00Z"));
  store.person = personWith([entry()]);
  ipc.listPeople.mockImplementation(async () => [{ ...ADA_ROW }]);
  ipc.listLabels.mockResolvedValue([]);
  ipc.searchPeople.mockResolvedValue([]);
  ipc.getPerson.mockImplementation(async () => store.person);
  ipc.addInteraction.mockResolvedValue(11);
  ipc.updateInteraction.mockResolvedValue(undefined);
  ipc.deleteInteraction.mockResolvedValue(undefined);
});

afterEach(() => {
  act(() => closeContextMenu());
  cleanup();
  vi.useRealTimers();
});

describe("the Timeline section", () => {
  it("says when you last met and lists entries newest first", async () => {
    store.person = personWith([
      entry({ id: 12, kind: "call", on_date: "2026-11-20", note: "Quick call" }),
      entry({ id: 10, on_date: "2026-11-12" }),
    ]);
    await view();
    await openAda();
    // Said once, in the section: the header leaves it out.
    expect(screen.getAllByText(/^Last met /)).toHaveLength(1);
    expect(within(timeline()).getByText(/^Last met 12-11/)).toBeTruthy();
    const items = within(timeline()).getAllByRole("listitem");
    expect(items.map((li) => li.querySelector("span")?.textContent)).toEqual(["Call", "Met"]);
    expect(within(items[0]).getByText("Quick call")).toBeTruthy();
    expect(within(items[1]).getByText("Lunch by the canal")).toBeTruthy();
  });

  it("says 'Not met yet' with no meeting, and shows a call without making it a meeting", async () => {
    store.person = personWith([entry({ kind: "call", note: "" })]);
    await view();
    await openAda();
    expect(within(timeline()).getByText("Not met yet")).toBeTruthy();
    expect(within(timeline()).getByText("Call")).toBeTruthy();
  });

  it("says nothing is logged yet, and offers + Log", async () => {
    store.person = personWith([]);
    await view();
    await openAda();
    expect(within(timeline()).getByText(/Nothing logged yet\./)).toBeTruthy();
    expect(within(timeline()).getByRole("button", { name: "+ Log" })).toBeTruthy();
  });

  it("shows an entry's time, the event it came from and who else was there", async () => {
    const at = "2026-11-12T09:30:00Z";
    store.person = personWith([
      entry({
        at,
        event_title: "Planning",
        others: [{ id: 2, display_name: "Ben Sample" }],
      }),
    ]);
    await view();
    await openAda();
    expect(
      within(timeline()).getByText(new RegExp(`12-11 ${formatClock(new Date(at))}`)),
    ).toBeTruthy();
    expect(within(timeline()).getByText("From “Planning”")).toBeTruthy();
    expect(within(timeline()).getByText("With Ben Sample")).toBeTruthy();
  });

  it("reads the next page on 'Show earlier entries'", async () => {
    const cursor = { on_date: "2026-11-12", at: "", id: 10 };
    store.person = personWith([entry()], {}, cursor as never);
    ipc.listTimeline.mockResolvedValue({
      entries: [entry({ id: 5, on_date: "2026-10-01", note: "Much earlier" })],
      next: null,
    });
    await view();
    await openAda();
    fireEvent.click(within(timeline()).getByRole("button", { name: "Show earlier entries" }));
    expect(await within(timeline()).findByText("Much earlier")).toBeTruthy();
    expect(ipc.listTimeline).toHaveBeenCalledWith(1, cursor);
    expect(within(timeline()).queryByRole("button", { name: "Show earlier entries" })).toBeNull();
  });

  it("drops an earlier page that lands after the record was read again", async () => {
    const cursor = { on_date: "2026-11-12", at: "", id: 10 };
    store.person = personWith([entry()], {}, cursor as never);
    let late: (page: unknown) => void = () => {};
    ipc.listTimeline.mockImplementation(() => new Promise((resolve) => (late = resolve)));
    await view();
    await openAda();
    fireEvent.click(within(timeline()).getByRole("button", { name: "Show earlier entries" }));
    // The record changes under it: the entry was edited elsewhere.
    store.person = personWith([entry({ note: "Edited lunch" })], {}, cursor as never);
    ipc.listeners.forEach((f) => f());
    await within(timeline()).findByText("Edited lunch");
    await act(async () => {
      late({ entries: [entry({ id: 5, note: "Stale page" })], next: null });
    });
    expect(within(timeline()).queryByText("Stale page")).toBeNull();
    // Its cursor is the record's own again: the button is still there.
    expect(within(timeline()).getByRole("button", { name: "Show earlier entries" })).toBeTruthy();
  });

  it("keeps saying when you last met while the section is folded", async () => {
    await view();
    await openAda();
    fireEvent.click(within(timeline()).getByRole("button", { name: /^Timeline/ }));
    expect(screen.getAllByText(/^Last met 12-11/)).toHaveLength(1);
  });

  it("leaves a person in Trash read-only", async () => {
    store.person = personWith([entry()], { trashed_at: "2026-11-20T10:00:00Z" });
    ipc.listPeople.mockImplementation(async (filter: unknown) =>
      filter === "Trash" ? [{ ...ADA_ROW, trashed_at: "2026-11-20T10:00:00Z" }] : [],
    );
    await view();
    fireEvent.click(screen.getByRole("button", { name: /^Trash/ }));
    await screen.findByText("Ada Example");
    fireEvent.click(screen.getByText("Ada Example"));
    await screen.findByRole("region", { name: "Timeline" });
    expect(within(timeline()).queryByRole("button", { name: "+ Log" })).toBeNull();
    expect(within(timeline()).queryByRole("button", { name: /^Edit/ })).toBeNull();
    expect(within(timeline()).queryByRole("button", { name: /^Delete/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "Log a meeting" })).toBeNull();
  });
});

describe("logging, editing and deleting an entry", () => {
  it("+ Log opens today's date, saves a meeting naming the person and reads the record again", async () => {
    await view();
    await openAda();
    const reads = ipc.getPerson.mock.calls.length;
    fireEvent.click(within(timeline()).getByRole("button", { name: "+ Log" }));
    const d = dialog(/Log something with Ada Example/);
    // Three small fields, never a native date input; today in the user's zone.
    expect((within(d).getByLabelText("Date day") as HTMLInputElement).value).toBe("26");
    expect((within(d).getByLabelText("Date month") as HTMLInputElement).value).toBe("11");
    expect((within(d).getByLabelText("Date year") as HTMLInputElement).value).toBe("2026");
    expect(d.querySelector('input[type="date"]')).toBeNull();
    fireEvent.change(within(d).getByLabelText("What happened"), { target: { value: "call" } });
    fireEvent.change(within(d).getByLabelText("Note (optional)"), {
      target: { value: " Rang her " },
    });
    fireEvent.click(within(d).getByRole("button", { name: "Log it" }));
    await waitFor(() => expect(ipc.addInteraction).toHaveBeenCalledOnce());
    expect(ipc.addInteraction).toHaveBeenCalledWith({
      kind: "call",
      on_date: "2026-11-26",
      at: null,
      note: "Rang her",
      people: [1],
    });
    await waitFor(() => expect(ipc.getPerson.mock.calls.length).toBeGreaterThan(reads));
    expect(screen.queryByRole("dialog", { name: /Log a meeting/ })).toBeNull();
  });

  it("takes a date the person typed, and says in words what is wrong with one that isn't a day", async () => {
    await view();
    await openAda();
    fireEvent.click(within(timeline()).getByRole("button", { name: "+ Log" }));
    const d = dialog(/Log something/);
    fireEvent.change(within(d).getByLabelText("Date year"), { target: { value: "" } });
    fireEvent.click(within(d).getByRole("button", { name: "Log it" }));
    expect(await within(d).findByRole("alert")).toBeTruthy();
    expect(ipc.addInteraction).not.toHaveBeenCalled();
    fireEvent.change(within(d).getByLabelText("Date year"), { target: { value: "2026" } });
    fireEvent.change(within(d).getByLabelText("Date day"), { target: { value: "3" } });
    fireEvent.change(within(d).getByLabelText("Date month"), { target: { value: "11" } });
    fireEvent.click(within(d).getByRole("button", { name: "Log it" }));
    await waitFor(() => expect(ipc.addInteraction).toHaveBeenCalled());
    expect(ipc.addInteraction.mock.calls[0][0].on_date).toBe("2026-11-03");
  });

  it("keeps the dialog and says what the backend refused with", async () => {
    ipc.addInteraction.mockRejectedValueOnce("The note is too long.");
    await view();
    await openAda();
    fireEvent.click(within(timeline()).getByRole("button", { name: "+ Log" }));
    const d = dialog(/Log something/);
    fireEvent.click(within(d).getByRole("button", { name: "Log it" }));
    expect(await within(d).findByText("The note is too long.")).toBeTruthy();
    expect(dialog(/Log something/)).toBeTruthy();
  });

  it("Log a meeting on the record opens the same dialog, as a meeting", async () => {
    await view();
    await openAda();
    fireEvent.click(screen.getByRole("button", { name: "Log a meeting" }));
    const d = dialog("Log something with Ada Example");
    expect((within(d).getByLabelText("What happened") as HTMLSelectElement).value).toBe("met");
  });

  it("Log a meeting in the row's right-click menu opens it too", async () => {
    await view();
    fireEvent.contextMenu(screen.getByText("Ada Example").closest("li")!, {
      clientX: 5,
      clientY: 5,
    });
    fireEvent.click(screen.getByRole("menuitem", { name: "Log a meeting…" }));
    expect(dialog("Log something with Ada Example")).toBeTruthy();
    // The row it was asked on is the record that opens behind it.
    expect(await screen.findByRole("region", { name: "Timeline" })).toBeTruthy();
  });

  it("Edit changes an entry in place and keeps everyone else on it", async () => {
    store.person = personWith([
      entry({ others: [{ id: 2, display_name: "Ben Sample" }], at: "2026-11-12T09:30:00Z" }),
    ]);
    await view();
    await openAda();
    fireEvent.click(within(timeline()).getByRole("button", { name: /^Edit the meeting on/ }));
    const d = dialog("Change this meeting");
    expect((within(d).getByLabelText("Note (optional)") as HTMLTextAreaElement).value).toBe(
      "Lunch by the canal",
    );
    expect((within(d).getByLabelText("Date day") as HTMLInputElement).value).toBe("12");
    fireEvent.change(within(d).getByLabelText("Note (optional)"), {
      target: { value: "Lunch and a walk" },
    });
    fireEvent.click(within(d).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(ipc.updateInteraction).toHaveBeenCalledOnce());
    expect(ipc.updateInteraction).toHaveBeenCalledWith(10, {
      kind: "met",
      on_date: "2026-11-12",
      // The day stands, so the time of day does too.
      at: "2026-11-12T09:30:00Z",
      note: "Lunch and a walk",
      people: [1, 2],
    });
  });

  it("Delete asks first, and only deleting deletes", async () => {
    await view();
    await openAda();
    fireEvent.click(within(timeline()).getByRole("button", { name: /^Delete the meeting on/ }));
    const ask = screen.getByRole("dialog", { name: "Delete this entry?" });
    fireEvent.click(within(ask).getByRole("button", { name: "Cancel" }));
    expect(ipc.deleteInteraction).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog", { name: "Delete this entry?" })).toBeNull();
    fireEvent.click(within(timeline()).getByRole("button", { name: /^Delete the meeting on/ }));
    fireEvent.click(
      within(screen.getByRole("dialog", { name: "Delete this entry?" })).getByRole("button", {
        name: "Delete entry",
      }),
    );
    await waitFor(() => expect(ipc.deleteInteraction).toHaveBeenCalledWith(10));
  });
});

describe("the meetings waiting at the top of the All view", () => {
  const rows = () => {
    ipc.listPeople.mockImplementation(async () => [{ ...ADA_ROW }]);
  };

  it("lists each owed meeting, and Review opens just that one", async () => {
    rows();
    const onReviewMeetings = vi.fn();
    const a = owedMeeting();
    const b = owedMeeting({ uid: "evt-2@example.com", title: "Retro" });
    await view({ owed: [a, b], onReviewMeetings });
    const list = screen.getByRole("region", { name: "Meetings waiting for an answer" });
    expect(within(list).getByText("2 meetings are waiting: who did you meet?")).toBeTruthy();
    fireEvent.click(within(list).getByRole("button", { name: "Review Retro" }));
    expect(onReviewMeetings).toHaveBeenLastCalledWith([b]);
    fireEvent.click(within(list).getByRole("button", { name: "Review all" }));
    expect(onReviewMeetings).toHaveBeenLastCalledWith([a, b]);
  });

  it("is there with no people yet, above the empty-state words", async () => {
    ipc.listPeople.mockImplementation(async () => []);
    const onReviewMeetings = vi.fn();
    const a = owedMeeting();
    await view({ owed: [a], onReviewMeetings });
    const list = screen.getByRole("region", { name: "Meetings waiting for an answer" });
    fireEvent.click(within(list).getByRole("button", { name: "Review Planning" }));
    expect(onReviewMeetings).toHaveBeenLastCalledWith([a]);
  });

  it("is not there with nothing owed, while searching or outside the All view", async () => {
    rows();
    await view({ owed: [], onReviewMeetings: vi.fn() });
    expect(screen.queryByRole("region", { name: "Meetings waiting for an answer" })).toBeNull();
    cleanup();
    await view({ owed: [owedMeeting()], onReviewMeetings: vi.fn() });
    expect(screen.getByRole("region", { name: "Meetings waiting for an answer" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /^Starred/ }));
    await act(async () => {});
    expect(screen.queryByRole("region", { name: "Meetings waiting for an answer" })).toBeNull();
  });
});
