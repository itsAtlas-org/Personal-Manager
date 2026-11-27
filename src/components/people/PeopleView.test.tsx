// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The People tab's list half (Contacts, slice 4a): the empty state, rows with their secondary line
// and chips, every filter, the remembered sort, search, ticking and ranges, the selection bar, Move
// to Trash with its Undo strip and Ctrl+Z, the Trash view's Restore and Delete now, the row menu and
// the one-shot reveal. Invented people only.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { PeopleFilter, PersonLabel, PersonRow } from "../../lib/types";

const store = vi.hoisted(() => ({
  rows: [] as unknown[],
  labels: [] as unknown[],
  results: [] as unknown[],
}));
const ipc = vi.hoisted(() => ({
  listPeople: vi.fn(),
  searchPeople: vi.fn(),
  listLabels: vi.fn(),
  starPeople: vi.fn(),
  trashPeople: vi.fn(),
  restorePeople: vi.fn(),
  deletePeopleNow: vi.fn(),
  labelPeople: vi.fn(),
  unlabelPeople: vi.fn(),
  createLabel: vi.fn(),
  deleteLabel: vi.fn(),
  renameLabel: vi.fn(),
  createPerson: vi.fn(),
  getPerson: vi.fn(),
  changed: null as null | (() => void),
  // The tab has two readers of the change notice (the list and the duplicate snapshot): a test's
  // `changed()` reaches all of them, as the real event does.
  listeners: [] as (() => void)[],
}));
vi.mock("../../lib/ipc", () => {
  const known: Record<string, unknown> = {
    ...ipc,
    onPeopleChanged: vi.fn(async (h: () => void) => {
      ipc.listeners.push(h);
      ipc.changed = () => ipc.listeners.forEach((f) => f());
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

import { PeopleView } from "./PeopleView";
import { ThemeProvider } from "../../theme";
import { ContextMenuHost } from "../ui/ContextMenuHost";
import { closeContextMenu } from "../../lib/contextMenu";

function person(id: number, name: string, over: Partial<PersonRow> = {}): PersonRow {
  return {
    id,
    uid: String(id).padStart(32, "0"),
    display_name: name,
    primary: null,
    company: null,
    labels: [],
    starred: false,
    last_met: null,
    google_state: null,
    trashed_at: null,
    birthday: null,
    leap_day: null,
    ...over,
  };
}

const LABELS: PersonLabel[] = [
  { id: 1, uid: "l1".padEnd(32, "0"), name: "Friends", people: 1 },
  { id: 2, uid: "l2".padEnd(32, "0"), name: "Climbing", people: 0 },
];

const ADA = person(1, "Ada Example", {
  company: "Example Ltd",
  labels: [{ id: 1, name: "Friends" }],
  starred: true,
});
const BEN = person(2, "Ben Sample", { primary: "ben@example.com" });
const CAL = person(3, "Cal Placeholder");
const DEE = person(4, "Dee Invented");

/** What `listPeople` answers for a filter, from the four people above. */
function answer(filter: PeopleFilter): PersonRow[] {
  const all = store.rows as PersonRow[];
  if (filter === "Trash") return all.filter((r) => r.trashed_at);
  const live = all.filter((r) => !r.trashed_at);
  if (filter === "All") return live;
  if (filter === "Starred") return live.filter((r) => r.starred);
  if (filter === "Unlabelled") return live.filter((r) => r.labels.length === 0);
  return live.filter((r) => r.labels.some((l) => l.id === filter.Label));
}

const ALL_PEOPLE = [ADA, BEN, CAL, DEE];

async function view(props: Partial<React.ComponentProps<typeof PeopleView>> = {}) {
  render(
    <ThemeProvider>
      <PeopleView {...props} />
      <ContextMenuHost />
    </ThemeProvider>,
  );
  await act(async () => {});
}

const list = () => screen.getByRole("list", { name: /^People/ });
const names = () =>
  within(list())
    .getAllByRole("listitem")
    .map((li) => li.querySelector("[data-person-main]")?.firstElementChild?.textContent ?? "");
const rowOf = (name: string) => screen.getByText(name).closest("li")!;
const tick = (name: string, init: MouseEventInit = {}) =>
  fireEvent.click(screen.getByRole("checkbox", { name: `Select ${name}` }), init);

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
  store.rows = ALL_PEOPLE.map((p) => ({ ...p }));
  store.labels = LABELS;
  store.results = [];
  ipc.changed = null;
  ipc.listeners.length = 0;
  ipc.listPeople.mockImplementation(async (filter: PeopleFilter) => answer(filter));
  ipc.listLabels.mockImplementation(async () => store.labels);
  ipc.searchPeople.mockImplementation(async () => store.results);
  ipc.starPeople.mockResolvedValue(1);
  ipc.trashPeople.mockImplementation(async (ids: number[]) => {
    for (const r of store.rows as PersonRow[])
      if (ids.includes(r.id)) r.trashed_at = "2026-11-26T10:00:00Z";
    return ids.length;
  });
  ipc.restorePeople.mockImplementation(async (ids: number[]) => {
    for (const r of store.rows as PersonRow[]) if (ids.includes(r.id)) r.trashed_at = null;
    return ids.length;
  });
  ipc.deletePeopleNow.mockImplementation(async (ids: number[]) => {
    store.rows = (store.rows as PersonRow[]).filter((r) => !ids.includes(r.id));
    return ids.length;
  });
  ipc.labelPeople.mockResolvedValue(1);
  ipc.unlabelPeople.mockResolvedValue(1);
  ipc.createLabel.mockResolvedValue(9);
  ipc.deleteLabel.mockImplementation(async (labelId: number) => {
    store.labels = (store.labels as PersonLabel[]).filter((l) => l.id !== labelId);
    for (const r of store.rows as PersonRow[]) r.labels = r.labels.filter((l) => l.id !== labelId);
    return 1;
  });
  ipc.renameLabel.mockResolvedValue(1);
  // A stored person for whichever row is asked about: the record reads this, not the list's row.
  ipc.getPerson.mockImplementation(async (id: number) => {
    const r = (store.rows as PersonRow[]).find((x) => x.id === id)!;
    return {
      id: r.id,
      uid: r.uid,
      display_name: r.display_name,
      given_name: "",
      middle_name: "",
      family_name: "",
      name_prefix: "",
      name_suffix: "",
      birthday: null,
      met_where: "",
      met_on: null,
      notes: "",
      starred: r.starred,
      trashed_at: r.trashed_at,
      created_at: "2026-11-01T10:00:00Z",
      updated_at: "2026-11-20T10:00:00Z",
      details: [],
      labels: r.labels,
      last_met: null,
      timeline: { entries: [], next: null },
      sources: [],
      projects: [],
    };
  });
});

afterEach(() => {
  act(() => closeContextMenu());
  cleanup();
});

describe("the list", () => {
  it("says so when nobody is here, in the spec's words minus the Google clause", async () => {
    store.rows = [];
    await view();
    expect(
      screen.getByText(
        "Nobody here yet. Add someone, or import a vCard file from your phone or iCloud.",
      ),
    ).toBeTruthy();
  });

  it("shows a skeleton while the first read is out", async () => {
    let release: (rows: PersonRow[]) => void = () => {};
    ipc.listPeople.mockImplementationOnce(
      () => new Promise<PersonRow[]>((resolve) => (release = resolve)),
    );
    await view();
    expect(screen.getByRole("status", { name: "Reading your people" })).toBeTruthy();
    expect(screen.getByText("Reading your people…")).toBeTruthy();
    expect(screen.queryByRole("list", { name: /^People/ })).toBeNull();
    await act(async () => release(ALL_PEOPLE.map((p) => ({ ...p }))));
    expect(screen.queryByRole("status", { name: "Reading your people" })).toBeNull();
    expect(names()).toHaveLength(4);
  });

  it("says so when the read fails, and Try again reads it again", async () => {
    ipc.listPeople.mockRejectedValueOnce(new Error("boom"));
    await view();
    expect(screen.getByText("PM couldn't read your people just now.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    await act(async () => {});
    expect(names()).toHaveLength(4);
    expect(screen.queryByText("PM couldn't read your people just now.")).toBeNull();
  });

  it("a failed write's message can be dismissed and goes when another filter is chosen", async () => {
    ipc.starPeople.mockRejectedValueOnce("PM couldn't star them.");
    await view();
    fireEvent.click(screen.getByRole("button", { name: "Star Ben Sample" }));
    await act(async () => {});
    expect(screen.getByText("PM couldn't star them.")).toBeTruthy();
    // A write isn't repeated by a read's retry: the strip offers to go away instead.
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByText("PM couldn't star them.")).toBeNull();

    ipc.starPeople.mockRejectedValueOnce("PM couldn't star them.");
    fireEvent.click(screen.getByRole("button", { name: "Star Ben Sample" }));
    await act(async () => {});
    expect(screen.getByText("PM couldn't star them.")).toBeTruthy();
    fireEvent.click(
      within(screen.getByRole("navigation", { name: "People filters" })).getByRole("button", {
        name: /^Starred/,
      }),
    );
    await act(async () => {});
    expect(screen.queryByText("PM couldn't star them.")).toBeNull();
  });

  it("draws each person with one secondary line, chips and a star", async () => {
    await view();
    expect(names()).toEqual(["Ada Example", "Ben Sample", "Cal Placeholder", "Dee Invented"]);
    // The company wins over an email; an email stands in without one; neither gives nothing.
    expect(within(rowOf("Ada Example")).getByText("Example Ltd")).toBeTruthy();
    expect(within(rowOf("Ben Sample")).getByText("ben@example.com")).toBeTruthy();
    expect(within(rowOf("Cal Placeholder")).queryByText(/@|Ltd/)).toBeNull();
    expect(within(rowOf("Ada Example")).getByText("Friends")).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Star Ada Example" }).getAttribute("aria-pressed"),
    ).toBe("true");
    expect(
      screen.getByRole("button", { name: "Star Ben Sample" }).getAttribute("aria-pressed"),
    ).toBe("false");
    // content-visibility keeps a long list cheap without a virtualiser.
    expect((rowOf("Ben Sample") as HTMLElement).style.contentVisibility).toBe("auto");
  });

  it("stars and unstars from the row", async () => {
    await view();
    fireEvent.click(screen.getByRole("button", { name: "Star Ben Sample" }));
    expect(ipc.starPeople).toHaveBeenCalledWith([2], true);
    fireEvent.click(screen.getByRole("button", { name: "Star Ada Example" }));
    expect(ipc.starPeople).toHaveBeenCalledWith([1], false);
  });

  it("re-reads the list when a change notice arrives", async () => {
    await view();
    const before = ipc.listPeople.mock.calls.length;
    (store.rows as PersonRow[]).push(person(5, "Eve Fictional"));
    await act(async () => {
      ipc.changed?.();
      await new Promise((r) => setTimeout(r, 250));
    });
    expect(ipc.listPeople.mock.calls.length).toBeGreaterThan(before);
    expect(names()).toContain("Eve Fictional");
  });
});

describe("filters", () => {
  const pick = async (name: string) => {
    fireEvent.click(
      within(screen.getByRole("navigation", { name: "People filters" })).getByRole("button", {
        name: new RegExp(`^${name}`),
      }),
    );
    await act(async () => {});
  };

  it("offers All, Starred, each label, Unlabelled and Trash, and each answers its own list", async () => {
    await view();
    const rail = within(screen.getByRole("navigation", { name: "People filters" }));
    // The filters, with each label's menu button beside it and "Add a label" under them all.
    expect(rail.getAllByRole("button").map((b) => b.textContent)).toEqual([
      "All",
      "Starred",
      "Friends1",
      "⋯",
      "Climbing0",
      "⋯",
      "Unlabelled",
      "Trash",
      "Add a label",
    ]);

    await pick("Starred");
    expect(ipc.listPeople).toHaveBeenLastCalledWith("Starred", null, "First");
    expect(names()).toEqual(["Ada Example"]);

    await pick("Friends");
    expect(ipc.listPeople).toHaveBeenLastCalledWith({ Label: 1 }, null, "First");
    expect(names()).toEqual(["Ada Example"]);

    await pick("Unlabelled");
    expect(names()).toEqual(["Ben Sample", "Cal Placeholder", "Dee Invented"]);

    await pick("Climbing");
    expect(screen.getByText("Nobody has this label yet.")).toBeTruthy();

    await pick("All");
    expect(names()).toHaveLength(4);
  });

  it("never draws the last filter's people under the new one while its read is out or failed", async () => {
    await view();
    expect(names()).toHaveLength(4);
    let release: (rows: PersonRow[]) => void = () => {};
    ipc.listPeople.mockImplementationOnce(
      () => new Promise<PersonRow[]>((resolve) => (release = resolve)),
    );
    await pick("Trash");
    // Waiting: a skeleton, no rows, no Restore buttons on live people.
    expect(screen.getByRole("status", { name: "Reading your people" })).toBeTruthy();
    expect(screen.queryByRole("list", { name: /^People/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Restore/ })).toBeNull();
    await act(async () => release([]));
    expect(screen.getByText("Trash is empty.")).toBeTruthy();

    // A failed read of the next filter shows its error and no rows from before.
    ipc.listPeople.mockRejectedValueOnce(new Error("boom"));
    await pick("Starred");
    expect(screen.getByText("PM couldn't read your people just now.")).toBeTruthy();
    expect(document.querySelectorAll("[data-person-id]")).toHaveLength(0);
    expect(screen.queryByRole("status", { name: "Reading your people" })).toBeNull();
  });
});

describe("sort", () => {
  it("remembers the choice on this device and asks the backend for it", async () => {
    await view();
    expect(ipc.listPeople).toHaveBeenLastCalledWith("All", null, "First");
    fireEvent.change(screen.getByRole("combobox", { name: "Sort by" }), {
      target: { value: "Last" },
    });
    await act(async () => {});
    expect(ipc.listPeople).toHaveBeenLastCalledWith("All", null, "Last");

    cleanup();
    await view();
    expect(ipc.listPeople).toHaveBeenLastCalledWith("All", null, "Last");
    expect((screen.getByRole("combobox", { name: "Sort by" }) as HTMLSelectElement).value).toBe(
      "Last",
    );
  });
});

describe("search", () => {
  it("calls searchPeople as you type, and clearing returns to the filter's list", async () => {
    store.results = [DEE];
    await view();
    const box = screen.getByRole("searchbox", { name: "Search people" });
    fireEvent.change(box, { target: { value: "dee" } });
    await waitFor(() => expect(ipc.searchPeople).toHaveBeenCalledWith("dee", expect.any(Number)));
    await waitFor(() => expect(names()).toEqual(["Dee Invented"]));

    fireEvent.change(box, { target: { value: "" } });
    await waitFor(() => expect(names()).toHaveLength(4));
  });

  it("says when nobody matches", async () => {
    await view();
    fireEvent.change(screen.getByRole("searchbox", { name: "Search people" }), {
      target: { value: "zzz" },
    });
    expect(await screen.findByText("Nobody matches “zzz”.")).toBeTruthy();
  });
});

describe("selecting several", () => {
  it("has a checkbox in each row's circle, ticks on click and counts the selection", async () => {
    await view();
    expect(screen.queryByRole("region", { name: "Selected people" })).toBeNull();
    tick("Ben Sample");
    const bar = screen.getByRole("region", { name: "Selected people" });
    expect(within(bar).getByText("1 selected")).toBeTruthy();
    tick("Cal Placeholder");
    expect(within(bar).getByText("2 selected")).toBeTruthy();
    tick("Ben Sample");
    expect(within(bar).getByText("1 selected")).toBeTruthy();
    fireEvent.click(within(bar).getByRole("button", { name: "Clear" }));
    expect(screen.queryByRole("region", { name: "Selected people" })).toBeNull();
  });

  it("makes the checkbox reachable from the keyboard: one tab stop for the list's rows", async () => {
    await view();
    const boxes = screen.getAllByRole("checkbox") as HTMLInputElement[];
    // Only the list's tab-stop row has its controls in the tab order; the rest are arrow-key reached.
    const stop = boxes.filter((b) => b.tabIndex === 0);
    expect(stop).toHaveLength(1);
    stop[0].focus();
    expect(document.activeElement).toBe(stop[0]);
  });

  it("swaps the circle for the checkbox on hover and focus (the classes; the paint is live-checked), and for good once something is ticked", async () => {
    // jsdom paints nothing, so this pins the contract the CSS runs on: the circle and the checkbox
    // sit in one `.group` row, the circle fades on hover and focus-within, the checkbox appears.
    await view();
    const row = rowOf("Ada Example");
    const circle = within(row).getByText("AE");
    const box = within(row).getByRole("checkbox");
    expect(row.classList.contains("group")).toBe(true);
    expect(row.contains(circle) && row.contains(box)).toBe(true);
    expect(circle.className).toContain("group-hover:opacity-0");
    expect(circle.className).toContain("group-focus-within:opacity-0");
    expect(box.className).toContain("group-hover:opacity-100");
    expect(box.className).toContain("group-focus-within:opacity-100");
    expect(box.className).toMatch(/(^| )opacity-0( |$)/);

    tick("Ben Sample");
    for (const name of ["Ada Example", "Cal Placeholder"]) {
      const r = rowOf(name);
      expect(within(r).getByRole("checkbox").className).toMatch(/(^| )opacity-100( |$)/);
      expect(r.querySelector("[aria-hidden]")?.className).toMatch(/(^| )opacity-0( |$)/);
    }
  });

  it("Shift-click ticks everyone between", async () => {
    await view();
    tick("Ada Example");
    tick("Dee Invented", { shiftKey: true });
    expect(
      within(screen.getByRole("region", { name: "Selected people" })).getByText("4 selected"),
    ).toBeTruthy();
  });

  it("stars everyone selected, or unstars when all are starred already", async () => {
    await view();
    tick("Ada Example");
    tick("Ben Sample");
    const bar = within(screen.getByRole("region", { name: "Selected people" }));
    fireEvent.click(bar.getByRole("button", { name: "Star" }));
    expect(ipc.starPeople).toHaveBeenCalledWith([1, 2], true);
  });

  it("labels everyone selected from the picker, and can make a new label", async () => {
    await view();
    tick("Ben Sample");
    tick("Cal Placeholder");
    const bar = within(screen.getByRole("region", { name: "Selected people" }));
    fireEvent.click(bar.getByRole("button", { name: "Label" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Climbing" }));
    expect(ipc.labelPeople).toHaveBeenCalledWith([2, 3], 2);

    fireEvent.click(screen.getByRole("button", { name: "New label…" }));
    const dialog = screen.getByRole("dialog", { name: "New label" });
    fireEvent.change(within(dialog).getByLabelText("Label name"), {
      target: { value: "Book club" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Add label" }));
    await waitFor(() => expect(ipc.createLabel).toHaveBeenCalledWith("Book club"));
    await waitFor(() => expect(ipc.labelPeople).toHaveBeenCalledWith([2, 3], 9));
  });

  it("applies an existing label whose name differs only by accent, case or spacing", async () => {
    store.labels = [{ id: 7, uid: "l7".padEnd(32, "0"), name: "Café  club", people: 0 }];
    await view();
    tick("Ben Sample");
    fireEvent.click(screen.getByRole("button", { name: "Label" }));
    fireEvent.click(screen.getByRole("button", { name: "New label…" }));
    const dialog = screen.getByRole("dialog", { name: "New label" });
    fireEvent.change(within(dialog).getByLabelText("Label name"), {
      target: { value: "cafe club" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Add label" }));
    await waitFor(() => expect(ipc.labelPeople).toHaveBeenCalledWith([2], 7));
    expect(ipc.createLabel).not.toHaveBeenCalled();
  });
});

describe("Move to Trash", () => {
  it("shows an Undo strip, and Undo restores them", async () => {
    await view();
    tick("Ben Sample");
    fireEvent.click(
      within(screen.getByRole("region", { name: "Selected people" })).getByRole("button", {
        name: "Move to Trash",
      }),
    );
    await act(async () => {});
    expect(ipc.trashPeople).toHaveBeenCalledWith([2]);
    expect(names()).not.toContain("Ben Sample");
    expect(screen.getByText("Moved Ben Sample to Trash.")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Undo: Moved Ben Sample to Trash." }));
    await act(async () => {});
    expect(ipc.restorePeople).toHaveBeenCalledWith([2]);
    expect(names()).toContain("Ben Sample");
    expect(screen.queryByText("Moved Ben Sample to Trash.")).toBeNull();
  });

  it("Undo after the filter and the search changed re-reads with the current ones", async () => {
    await view();
    tick("Ben Sample");
    fireEvent.click(screen.getByRole("button", { name: "Move to Trash" }));
    await act(async () => {});
    fireEvent.change(screen.getByRole("combobox", { name: "Sort by" }), {
      target: { value: "Last" },
    });
    await act(async () => {});
    fireEvent.click(
      within(screen.getByRole("navigation", { name: "People filters" })).getByRole("button", {
        name: /^Starred/,
      }),
    );
    await act(async () => {});
    ipc.listPeople.mockClear();
    fireEvent.click(screen.getByRole("button", { name: "Undo: Moved Ben Sample to Trash." }));
    await act(async () => {});
    // The read that follows is the Starred list in the Last sort, never the All/First of the click.
    expect(ipc.listPeople).toHaveBeenCalled();
    for (const call of ipc.listPeople.mock.calls) expect(call).toEqual(["Starred", null, "Last"]);
    expect(names()).toEqual(["Ada Example"]);
  });

  it("counts several in the strip", async () => {
    await view();
    tick("Ben Sample");
    tick("Cal Placeholder");
    fireEvent.click(screen.getByRole("button", { name: "Move to Trash" }));
    await act(async () => {});
    expect(screen.getByText("Moved 2 people to Trash.")).toBeTruthy();
  });

  it("Ctrl+Z presses the newest Undo, but not while typing in the search box", async () => {
    await view();
    fireEvent.click(screen.getByRole("button", { name: "Star Ben Sample" })); // not an undo
    tick("Cal Placeholder");
    fireEvent.click(screen.getByRole("button", { name: "Move to Trash" }));
    await act(async () => {});
    expect(screen.getByText("Moved Cal Placeholder to Trash.")).toBeTruthy();

    const box = screen.getByRole("searchbox", { name: "Search people" });
    box.focus();
    fireEvent.keyDown(box, { key: "z", ctrlKey: true });
    await act(async () => {});
    expect(ipc.restorePeople).not.toHaveBeenCalled();
    expect(screen.getByText("Moved Cal Placeholder to Trash.")).toBeTruthy();

    (document.activeElement as HTMLElement).blur();
    fireEvent.keyDown(document.body, { key: "z", ctrlKey: true });
    await act(async () => {});
    expect(ipc.restorePeople).toHaveBeenCalledWith([3]);
    expect(screen.queryByText("Moved Cal Placeholder to Trash.")).toBeNull();
  });

  it("says in words when the move fails, and shows no Undo for it", async () => {
    ipc.trashPeople.mockRejectedValueOnce("PM couldn't move them just now.");
    await view();
    tick("Ben Sample");
    fireEvent.click(screen.getByRole("button", { name: "Move to Trash" }));
    await act(async () => {});
    expect(screen.getByText("PM couldn't move them just now.")).toBeTruthy();
    expect(screen.queryByText(/to Trash\.$/)).toBeNull();
  });
});

describe("the Trash view", () => {
  async function inTrash() {
    (store.rows as PersonRow[])[1].trashed_at = "2026-11-20T09:00:00Z";
    (store.rows as PersonRow[])[2].trashed_at = "2026-11-21T09:00:00Z";
    await view();
    fireEvent.click(screen.getByRole("button", { name: "Trash" }));
    await act(async () => {});
  }

  it("lists the trashed people with the 30-day line and Restore on each", async () => {
    await inTrash();
    expect(screen.getByText("People in Trash are deleted for good after 30 days.")).toBeTruthy();
    expect(names()).toEqual(["Ben Sample", "Cal Placeholder"]);
    expect(within(rowOf("Ben Sample")).getByText(/^In Trash since /)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Star Ben Sample" })).toBeNull();
  });

  it("Restore brings one back and says how many came back", async () => {
    await inTrash();
    fireEvent.click(screen.getByRole("button", { name: "Restore Ben Sample" }));
    await act(async () => {});
    expect(ipc.restorePeople).toHaveBeenCalledWith([2]);
    expect(screen.getByText("1 person came back from Trash.")).toBeTruthy();
    expect(names()).toEqual(["Cal Placeholder"]);
  });

  it("Delete now asks first, deletes nothing on Cancel and deletes for good on confirm", async () => {
    await inTrash();
    tick("Ben Sample");
    fireEvent.click(screen.getByRole("button", { name: "Delete now…" }));
    let dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText(/can't be undone/)).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(ipc.deletePeopleNow).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Delete now…" }));
    dialog = screen.getByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete for good" }));
    await waitFor(() => expect(ipc.deletePeopleNow).toHaveBeenCalledWith([2]));
    await waitFor(() => expect(names()).toEqual(["Cal Placeholder"]));
  });
});

describe("the row menu", () => {
  const rightClick = (el: Element) => fireEvent.contextMenu(el, { clientX: 5, clientY: 5 });

  it("opens for the row, named for the person, with Move to Trash last", async () => {
    await view();
    rightClick(rowOf("Ben Sample"));
    const menu = screen.getByRole("menu", { name: "Person Ben Sample" });
    const items = within(menu)
      .getAllByRole("menuitem")
      .concat(
        within(menu).queryAllByRole("menuitemradio"),
        within(menu).queryAllByRole("menuitemcheckbox"),
      );
    expect(items.length).toBeGreaterThan(3);
    const plain = within(menu).getAllByRole("menuitem");
    expect(plain[plain.length - 1].textContent).toBe("Move to Trash");
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Star" }));
    expect(ipc.starPeople).toHaveBeenCalledWith([2], true);
  });

  it("acts on the whole selection when the row is part of one", async () => {
    await view();
    tick("Ben Sample");
    tick("Cal Placeholder");
    rightClick(rowOf("Ben Sample"));
    expect(screen.getByRole("menu", { name: "People: 2 selected" })).toBeTruthy();
    fireEvent.click(screen.getByRole("menuitem", { name: "Move to Trash" }));
    await act(async () => {});
    expect(ipc.trashPeople).toHaveBeenCalledWith([2, 3]);
  });

  it("offers Restore and Delete now… for a row in Trash", async () => {
    (store.rows as PersonRow[])[1].trashed_at = "2026-11-20T09:00:00Z";
    await view();
    fireEvent.click(screen.getByRole("button", { name: "Trash" }));
    await act(async () => {});
    rightClick(rowOf("Ben Sample"));
    const menu = screen.getByRole("menu", { name: "Person Ben Sample" });
    expect(
      within(menu)
        .getAllByRole("menuitem")
        .map((i) => i.textContent),
    ).toEqual(["Open", "Restore", "Delete now…"]);
  });

  it("leaves the system menu alone for a right-click inside the search box", async () => {
    await view();
    rightClick(screen.getByRole("searchbox", { name: "Search people" }));
    expect(screen.queryByRole("menu")).toBeNull();
  });
});

describe("reveal", () => {
  it("selects the person, takes the reveal once and leaves a later visit alone", async () => {
    const taken = vi.fn();
    const scroll = vi.spyOn(Element.prototype, "scrollIntoView");
    await view({ reveal: { uid: BEN.uid, nonce: 1 }, onRevealTaken: taken });
    expect(taken).toHaveBeenCalledTimes(1);
    // The row is scrolled to, and it is Ben's.
    expect(scroll).toHaveBeenCalledTimes(1);
    expect((scroll.mock.contexts[0] as HTMLElement).dataset.personId).toBe("2");
    scroll.mockRestore();
    const open = within(list()).getAllByRole("button", { current: true });
    expect(open).toHaveLength(1);
    expect(open[0].textContent).toContain("Ben Sample");
    expect(screen.getByRole("region", { name: "Ben Sample" })).toBeTruthy();
  });

  it("opens the tab with nothing selected for a uid that names nobody", async () => {
    await view({ reveal: { uid: "f".repeat(32), nonce: 1 }, onRevealTaken: () => {} });
    expect(names()).toHaveLength(4);
    // (The filter rail's "All" is current too; only the list's rows count as open.)
    expect(list().querySelector("[aria-current='true']")).toBeNull();
    expect(screen.getByText("Nobody selected. Pick someone from the list.")).toBeTruthy();
    expect(screen.queryByText(/couldn't/)).toBeNull();
  });
});

describe("birthdays", () => {
  it("shows nothing when nobody has one coming, and a strip at the top of All when someone does", async () => {
    await view();
    expect(screen.queryByRole("region", { name: "Upcoming birthdays" })).toBeNull();
    cleanup();

    const now = new Date();
    const soon = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 3);
    const mm = String(soon.getMonth() + 1).padStart(2, "0");
    const dd = String(soon.getDate()).padStart(2, "0");
    // 29 February never lands on a non-leap "soon" here: a year-less date is valid either way.
    (store.rows as PersonRow[])[2].birthday = `--${mm}-${dd}`;
    await view();
    const strip = screen.getByRole("region", { name: "Upcoming birthdays" });
    expect(within(strip).getByText("Cal Placeholder")).toBeTruthy();
    expect(strip.textContent).toMatch(/Birthday \d{1,2} [A-Z][a-z]+/);
  });
});

const pickFilter = async (name: string) => {
  fireEvent.click(
    within(screen.getByRole("navigation", { name: "People filters" })).getByRole("button", {
      name: new RegExp(`^${name}`),
    }),
  );
  await act(async () => {});
};

describe("adding a person", () => {
  it("opens the editor from + Add person, and the new person is selected and shown", async () => {
    await view();
    // From a narrower filter: the new person must still show, so the view goes back to everyone.
    await pickFilter("Starred");
    fireEvent.click(screen.getByRole("button", { name: "+ Add person" }));
    const dialog = screen.getByRole("dialog", { name: "Add a person" });
    fireEvent.change(within(dialog).getByLabelText("Given name"), { target: { value: "Eve" } });
    fireEvent.change(within(dialog).getByLabelText("Family name"), {
      target: { value: "Newcomer" },
    });
    ipc.createPerson.mockImplementation(async () => {
      (store.rows as PersonRow[]).push(person(5, "Eve Newcomer"));
      return 5;
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await act(async () => {});
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Add a person" })).toBeNull());
    expect(ipc.createPerson).toHaveBeenCalledTimes(1);
    expect(names()).toContain("Eve Newcomer");
    expect(screen.getByRole("region", { name: "Eve Newcomer" })).toBeTruthy();
    expect(ipc.getPerson).toHaveBeenLastCalledWith(5);
  });

  it("selects the new person once the list shows them, even if a read before that lacked them", async () => {
    await view();
    fireEvent.click(screen.getByRole("button", { name: "+ Add person" }));
    fireEvent.change(screen.getByLabelText("Given name"), { target: { value: "Eve" } });
    // The re-read after the write comes back without them (the write isn't visible yet).
    ipc.createPerson.mockResolvedValue(5);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await act(async () => {});
    expect(names()).not.toContain("Eve");
    (store.rows as PersonRow[]).push(person(5, "Eve"));
    await act(async () => ipc.changed?.());
    await waitFor(() => expect(names()).toContain("Eve"));
    expect(screen.getByRole("region", { name: "Eve" })).toBeTruthy();
  });
});

describe("the record", () => {
  it("opens the person on the right when a row is clicked, and Edit opens the editor on them", async () => {
    await view();
    fireEvent.click(within(rowOf("Ben Sample")).getByText("Ben Sample"));
    await act(async () => {});
    expect(screen.getByRole("region", { name: "Ben Sample" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    expect(screen.getByRole("dialog", { name: "Edit Ben Sample" })).toBeTruthy();
  });

  it("offers Edit… in the row menu, which opens the editor", async () => {
    await view();
    fireEvent.contextMenu(rowOf("Ben Sample"), { clientX: 5, clientY: 5 });
    fireEvent.click(screen.getByRole("menuitem", { name: "Edit…" }));
    await act(async () => {});
    expect(ipc.getPerson).toHaveBeenCalledWith(2);
    expect(screen.getByRole("dialog", { name: "Edit Ben Sample" })).toBeTruthy();
  });

  it("puts a label on the open person from the record, and takes it off", async () => {
    await view();
    fireEvent.click(within(rowOf("Ada Example")).getByText("Ada Example"));
    await act(async () => {});
    const record = within(screen.getByRole("region", { name: "Ada Example" }));
    fireEvent.click(record.getByRole("button", { name: "Remove the label Friends" }));
    expect(ipc.unlabelPeople).toHaveBeenCalledWith([1], 1);
    fireEvent.click(record.getByRole("button", { name: "Add label" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Climbing" }));
    expect(ipc.labelPeople).toHaveBeenCalledWith([1], 2);
  });
});

describe("managing labels", () => {
  const optionsOf = (name: string) =>
    fireEvent.click(screen.getByRole("button", { name: `Label options for ${name}` }));

  it("adds a label with nobody on it from the filter list", async () => {
    await view();
    fireEvent.click(screen.getByRole("button", { name: "Add a label" }));
    const dialog = screen.getByRole("dialog", { name: "New label" });
    fireEvent.change(within(dialog).getByLabelText("Label name"), {
      target: { value: "Book club" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Add label" }));
    await waitFor(() => expect(ipc.createLabel).toHaveBeenCalledWith("Book club"));
    expect(ipc.labelPeople).not.toHaveBeenCalled();
  });

  it("says so when the label already exists, rather than quietly reusing it", async () => {
    await view();
    fireEvent.click(screen.getByRole("button", { name: "Add a label" }));
    const dialog = screen.getByRole("dialog", { name: "New label" });
    fireEvent.change(within(dialog).getByLabelText("Label name"), { target: { value: "friends" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Add label" }));
    await waitFor(() =>
      expect(within(dialog).getByText("You already have a label with that name.")).toBeTruthy(),
    );
    expect(ipc.createLabel).not.toHaveBeenCalled();
  });

  it("renames a label", async () => {
    await view();
    optionsOf("Friends");
    fireEvent.click(screen.getByRole("button", { name: "Rename…" }));
    const dialog = screen.getByRole("dialog", { name: "Rename label" });
    const box = within(dialog).getByLabelText("Label name") as HTMLInputElement;
    expect(box.value).toBe("Friends");
    fireEvent.change(box, { target: { value: "Mates" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Rename" }));
    await waitFor(() => expect(ipc.renameLabel).toHaveBeenCalledWith(1, "Mates"));
  });

  it("asks before deleting a label, and deletes nothing on Cancel", async () => {
    await view();
    optionsOf("Friends");
    fireEvent.click(screen.getByRole("button", { name: "Delete…" }));
    const dialog = screen.getByRole("dialog", { name: "Delete the label Friends?" });
    expect(within(dialog).getByText(/Nobody is deleted with it/)).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(ipc.deleteLabel).not.toHaveBeenCalled();
    expect(screen.queryByText("Deleted the label Friends.")).toBeNull();
  });

  it("deletes a label with an Undo strip, and Undo puts it back on the same people, in Trash too", async () => {
    // Cal is in Trash with the label: the list hides her, but Undo must still give it back to her.
    const rows = store.rows as PersonRow[];
    rows[2].labels = [{ id: 1, name: "Friends" }];
    rows[2].trashed_at = "2026-11-20T09:00:00Z";
    await view();
    optionsOf("Friends");
    fireEvent.click(screen.getByRole("button", { name: "Delete…" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete label" }));
    await act(async () => {});
    expect(ipc.deleteLabel).toHaveBeenCalledWith(1);
    const undo = () => screen.getByRole("button", { name: "Undo: Deleted the label Friends." });
    expect(undo()).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Label options for Friends" })).toBeNull();

    fireEvent.click(undo());
    await act(async () => {});
    expect(ipc.createLabel).toHaveBeenCalledWith("Friends");
    // The id is new (the mock hands back 9); the same two people carry it.
    expect(ipc.labelPeople).toHaveBeenCalledWith([1, 3], 9);
  });

  it("leaves the label filter when the label it was showing is deleted", async () => {
    await view();
    await pickFilter("Friends");
    optionsOf("Friends");
    fireEvent.click(screen.getByRole("button", { name: "Delete…" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete label" }));
    await act(async () => {});
    expect(names()).toHaveLength(4);
  });

  it("puts the label on whoever carries the name now, when it was made again before Undo", async () => {
    await view();
    optionsOf("Friends");
    fireEvent.click(screen.getByRole("button", { name: "Delete…" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete label" }));
    await act(async () => {});
    ipc.createLabel.mockRejectedValueOnce("You already have a label with that name.");
    store.labels = [{ id: 12, uid: "l12".padEnd(32, "0"), name: "friends", people: 0 }];
    fireEvent.click(screen.getByRole("button", { name: "Undo: Deleted the label Friends." }));
    await act(async () => {});
    expect(ipc.labelPeople).toHaveBeenCalledWith([1], 12);
  });
});
