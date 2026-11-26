// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Possible duplicates and merging (Contacts, slice 5): the header button that only shows when there is
// something to look at, the review panel's three answers, the "Nothing differs" group and its preview
// line, the merge dialog's choices (and Keep both), the selection bar's Merge at exactly two, the
// right-click "Merge with…", and the Undo strip after a merge. Invented people only.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { DupeCard, DupePair, DupesReport, PersonRow } from "../../lib/types";

const ipc = vi.hoisted(() => ({
  listPeople: vi.fn(),
  searchPeople: vi.fn(),
  listLabels: vi.fn(),
  getPerson: vi.fn(),
  peopleDuplicatesSnapshot: vi.fn(),
  mergePeoplePreview: vi.fn(),
  mergePeople: vi.fn(),
  mergeIdenticalPairs: vi.fn(),
  undoMergePeople: vi.fn(),
  markPeopleDistinct: vi.fn(),
  restorePeopleDistinct: vi.fn(),
  changed: null as null | (() => void),
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

function row(id: number, name: string): PersonRow {
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
  };
}

const ADA = row(1, "Ada Example");
const ADA2 = row(2, "Ada Exemplar");
const BEN = row(3, "Ben Sample");
const BEN2 = row(4, "Ben Sampel");
const CAL = row(5, "Cal Placeholder");

function card(r: PersonRow, over: Partial<DupeCard> = {}): DupeCard {
  return {
    id: r.id,
    uid: r.uid,
    display_name: r.display_name,
    birthday: null,
    met_where: "",
    met_on: null,
    note: "",
    details: [],
    ...over,
  };
}

const NO_DIFF = { name: false, birthday: false, met: false, notes: false };

function pair(a: number, b: number, over: Partial<DupePair> = {}): DupePair {
  return {
    a,
    b,
    reasons: ["Name"],
    differs: NO_DIFF,
    nothing_differs: false,
    ...over,
  };
}

function report(over: Partial<DupesReport> = {}): DupesReport {
  return {
    pairs: [],
    cards: [],
    hidden: 0,
    skipped: [],
    cut_short: false,
    scanned: 5,
    ...over,
  };
}

/** Two pairs that differ in nothing (one by email, one by name) and one that needs a look. */
function mixed(): DupesReport {
  return report({
    pairs: [
      pair(1, 2, { reasons: ["Email", "Name"], nothing_differs: true }),
      pair(3, 4, { reasons: ["Similar"], differs: { ...NO_DIFF, name: true } }),
    ],
    cards: [
      card(ADA, { details: [{ kind: "email", label: "", value: "ada@example.com" }] }),
      card(ADA2, {
        details: [
          { kind: "email", label: "work", value: "ada@example.com" },
          { kind: "phone", label: "", value: "+44 20 7946 0001" },
        ],
      }),
      card(BEN),
      card(BEN2, { birthday: "--04-15" }),
    ],
  });
}

let rows: PersonRow[] = [];
let snapshot: DupesReport = report();

async function view() {
  render(
    <ThemeProvider>
      <PeopleView />
      <ContextMenuHost />
    </ThemeProvider>,
  );
  await act(async () => {});
}

const tick = (name: string) =>
  fireEvent.click(screen.getByRole("checkbox", { name: `Select ${name}` }));
const dupesButton = () => screen.queryByRole("button", { name: /^Possible duplicates/ });

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
  rows = [ADA, ADA2, BEN, BEN2, CAL].map((r) => ({ ...r }));
  snapshot = report();
  ipc.changed = null;
  ipc.listeners.length = 0;
  ipc.listPeople.mockImplementation(async () => rows);
  ipc.listLabels.mockResolvedValue([]);
  ipc.searchPeople.mockResolvedValue([]);
  ipc.peopleDuplicatesSnapshot.mockImplementation(async () => snapshot);
  ipc.markPeopleDistinct.mockResolvedValue(undefined);
  ipc.restorePeopleDistinct.mockResolvedValue(1);
  ipc.undoMergePeople.mockResolvedValue(1);
  ipc.mergePeoplePreview.mockResolvedValue({
    details: 3,
    details_dropped: 1,
    labels: 2,
    timeline: 4,
    projects: 0,
    sources: 1,
    differs: NO_DIFF,
    lossless: true,
  });
  // Two stored people with differing birthdays, notes and names.
  ipc.getPerson.mockImplementation(async (id: number) => {
    const r = rows.find((x) => x.id === id)!;
    const second = id % 2 === 0;
    return {
      id,
      uid: r.uid,
      display_name: r.display_name,
      given_name: "",
      middle_name: "",
      family_name: "",
      name_prefix: "",
      name_suffix: "",
      birthday: second ? "1991-02-02" : "1990-01-01",
      met_where: second ? "the pub" : "the fair",
      met_on: null,
      notes: second ? "second note" : "first note",
      starred: false,
      trashed_at: null,
      created_at: "2026-11-01T10:00:00Z",
      updated_at: "2026-11-20T10:00:00Z",
      details: [],
      labels: [],
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

describe("the Possible duplicates button", () => {
  it("is not there when there is nothing to look at", async () => {
    await view();
    expect(dupesButton()).toBeNull();
  });

  it("is not there when the snapshot is not a report (a failed read)", async () => {
    ipc.peopleDuplicatesSnapshot.mockResolvedValue([]);
    await view();
    expect(dupesButton()).toBeNull();
  });

  it("counts the pairs and opens the review", async () => {
    snapshot = mixed();
    await view();
    fireEvent.click(screen.getByRole("button", { name: "Possible duplicates (2)" }));
    expect(screen.getByRole("region", { name: "Possible duplicates" })).toBeTruthy();
  });

  it("stays, without a count, while some pairs are hidden, so Show them again can be reached", async () => {
    snapshot = report({ hidden: 1 });
    await view();
    fireEvent.click(screen.getByRole("button", { name: "Possible duplicates" }));
    expect(screen.getByText(/1 pair you said isn't the same person is hidden\./)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Show them again" }));
    await waitFor(() => expect(ipc.restorePeopleDistinct).toHaveBeenCalledOnce());
  });

  it("stays, without a count, when the scan could not compare everyone", async () => {
    snapshot = report({ skipped: [{ reason: "Email", people: 250 }], scanned: 400 });
    await view();
    fireEvent.click(screen.getByRole("button", { name: "Possible duplicates" }));
    expect(screen.getByText("No duplicates found in what PM compared.")).toBeTruthy();
    expect(screen.getByText(/didn't compare 1 very large group/)).toBeTruthy();
  });

  it("reads again when the address book changes", async () => {
    await view();
    expect(dupesButton()).toBeNull();
    snapshot = mixed();
    await act(async () => {
      ipc.changed?.();
      await new Promise((r) => setTimeout(r, 300));
    });
    expect(dupesButton()).toBeTruthy();
  });
});

describe("the review", () => {
  async function open(r: DupesReport = mixed()) {
    snapshot = r;
    await view();
    fireEvent.click(screen.getByRole("button", { name: /^Possible duplicates/ }));
  }

  it("shows every row on both sides, with the reason as a sentence", async () => {
    await open();
    const group = screen.getByRole("group", { name: "Ben Sample and Ben Sampel" });
    expect(within(group).getByText("Their names are alike, like Sam and Samuel.")).toBeTruthy();
    expect(within(group).getByText("Similar name.")).toBeTruthy();
    // The birthday row is drawn for both even though only one has one: a dash holds the other's place.
    const birthday = within(group).getByRole("rowheader", { name: "Birthday" }).closest("tr")!;
    expect(within(birthday).getAllByRole("cell")).toHaveLength(2);
    expect(birthday.textContent).toContain("15 April");
    expect(birthday.textContent).toContain("—");
  });

  it("offers the three answers on every pair", async () => {
    await open();
    const group = screen.getByRole("group", { name: "Ben Sample and Ben Sampel" });
    for (const name of ["Merge…", "Not the same person", "Decide later"]) {
      expect(within(group).getByRole("button", { name })).toBeTruthy();
    }
  });

  it("Not the same person stores the pair and reads the snapshot again", async () => {
    await open();
    const group = screen.getByRole("group", { name: "Ben Sample and Ben Sampel" });
    fireEvent.click(within(group).getByRole("button", { name: "Not the same person" }));
    await waitFor(() => expect(ipc.markPeopleDistinct).toHaveBeenCalledWith(3, 4));
    await waitFor(() => expect(ipc.peopleDuplicatesSnapshot.mock.calls.length).toBeGreaterThan(1));
  });

  it("Decide later puts the pair aside for this visit and stores nothing", async () => {
    await open();
    const group = screen.getByRole("group", { name: "Ben Sample and Ben Sampel" });
    fireEvent.click(within(group).getByRole("button", { name: "Decide later" }));
    expect(screen.queryByRole("group", { name: "Ben Sample and Ben Sampel" })).toBeNull();
    expect(ipc.markPeopleDistinct).not.toHaveBeenCalled();
  });

  it("moves focus to the next card when a card goes, and to the heading when none is left", async () => {
    await open(
      report({
        pairs: [
          pair(3, 4, { reasons: ["Similar"], differs: { ...NO_DIFF, name: true } }),
          pair(1, 2, { reasons: ["Similar"], differs: { ...NO_DIFF, name: true } }),
        ],
        cards: [card(ADA), card(ADA2), card(BEN), card(BEN2)],
      }),
    );
    const later = (names: string) =>
      within(screen.getByRole("group", { name: names })).getByRole("button", {
        name: "Decide later",
      });
    const first = later("Ben Sample and Ben Sampel");
    first.focus();
    fireEvent.click(first);
    const next = within(screen.getByRole("group", { name: "Ada Example and Ada Exemplar" }));
    await waitFor(() =>
      expect(document.activeElement).toBe(next.getByRole("button", { name: "Merge…" })),
    );
    const last = later("Ada Example and Ada Exemplar");
    last.focus();
    fireEvent.click(last);
    await waitFor(() =>
      expect(document.activeElement).toBe(
        screen.getByRole("heading", { name: "Possible duplicates", level: 2 }),
      ),
    );
  });

  it("Merge… opens the merge dialog for that pair", async () => {
    await open();
    const group = screen.getByRole("group", { name: "Ben Sample and Ben Sampel" });
    fireEvent.click(within(group).getByRole("button", { name: "Merge…" }));
    await screen.findByRole("dialog", { name: "Merge Ben Sample and Ben Sampel" });
    expect(ipc.mergePeoplePreview).toHaveBeenCalledWith(3, 4);
  });

  it("groups what differs in nothing, with its preview line", async () => {
    await open();
    expect(screen.getByRole("heading", { name: "Nothing differs (1)" })).toBeTruthy();
    expect(screen.getByText("1 pair merges with nothing lost. 1 is left for you.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Merge these 1" })).toBeTruthy();
  });

  it("a Similar name pair never joins the group, whatever the backend said", async () => {
    await open(
      report({
        pairs: [pair(3, 4, { reasons: ["Similar"], nothing_differs: true })],
        cards: [card(BEN), card(BEN2)],
      }),
    );
    expect(screen.queryByRole("heading", { name: /^Nothing differs/ })).toBeNull();
    expect(screen.getByRole("group", { name: "Ben Sample and Ben Sampel" })).toBeTruthy();
  });

  it("Merge these N asks with the preview line, merges, and offers one Undo", async () => {
    const two = report({
      pairs: [
        pair(1, 2, { nothing_differs: true }),
        pair(3, 4, { reasons: ["Email"], nothing_differs: true }),
        pair(5, 5 + 1, { reasons: ["Similar"] }),
      ],
      cards: [card(ADA), card(ADA2), card(BEN), card(BEN2)],
    });
    ipc.mergeIdenticalPairs.mockResolvedValue({
      survivor: 3,
      display_name: "Ben Sample",
      merged: 2,
      undo: "token-batch",
    });
    await open(two);
    fireEvent.click(screen.getByRole("button", { name: "Merge these 2" }));
    const sure = await screen.findByRole("dialog", { name: "Merge these 2?" });
    expect(
      within(sure).getByText(/2 pairs merge with nothing lost\. 1 is left for you\./),
    ).toBeTruthy();
    fireEvent.click(within(sure).getByRole("button", { name: "Merge these 2" }));
    await waitFor(() =>
      expect(ipc.mergeIdenticalPairs).toHaveBeenCalledWith([
        [1, 2],
        [3, 4],
      ]),
    );
    expect(await screen.findByText("Merged 2 duplicates.")).toBeTruthy();
    // A merge's notice points Help at merging, not at Trash.
    expect(
      screen.getByText("Merged 2 duplicates.").closest("[data-help]")?.getAttribute("data-help"),
    ).toBe("people-merge");
    fireEvent.click(screen.getByRole("button", { name: "Undo: Merged 2 duplicates." }));
    await waitFor(() => expect(ipc.undoMergePeople).toHaveBeenCalledWith("token-batch"));
  });

  it("says how many pairs are hidden and brings them back", async () => {
    await open(report({ pairs: mixed().pairs, cards: mixed().cards, hidden: 2 }));
    expect(screen.getByText(/2 pairs you said aren't the same person are hidden\./)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Show them again" }));
    await waitFor(() => expect(ipc.restorePeopleDistinct).toHaveBeenCalledOnce());
  });

  it("never calls a scan that skipped something clean", async () => {
    await open({ ...mixed(), skipped: [{ reason: "Email", people: 250 }], scanned: 400 });
    expect(screen.queryByText(/No duplicates found among/)).toBeNull();
    expect(screen.getByText(/didn't compare 1 very large group/)).toBeTruthy();
  });

  it("says plainly when it looked at everyone and found nothing", async () => {
    snapshot = report({ scanned: 5 });
    await view();
    // The button isn't drawn at 0, so the review is reached from a state that had pairs.
    snapshot = mixed();
    await act(async () => {
      ipc.changed?.();
      await new Promise((r) => setTimeout(r, 300));
    });
    fireEvent.click(screen.getByRole("button", { name: /^Possible duplicates/ }));
    snapshot = report({ scanned: 5 });
    await act(async () => {
      ipc.changed?.();
      await new Promise((r) => setTimeout(r, 300));
    });
    expect(screen.getByText("No duplicates found among your 5 people.")).toBeTruthy();
  });
});

describe("the merge dialog", () => {
  async function openDialog() {
    snapshot = mixed();
    await view();
    tick("Ben Sample");
    tick("Ben Sampel");
    fireEvent.click(screen.getByRole("button", { name: "Merge…" }));
    return screen.findByRole("dialog", { name: "Merge Ben Sample and Ben Sampel" });
  }

  it("says what moves, with the counts of the merged record", async () => {
    ipc.mergePeoplePreview.mockResolvedValue({
      details: 3,
      details_dropped: 1,
      labels: 1,
      timeline: 4,
      projects: 0,
      sources: 1,
      differs: NO_DIFF,
      lossless: true,
    });
    const dialog = await openDialog();
    expect(
      within(dialog).getByText(
        "Everything from both moves to one record: 3 details, 4 timeline entries, 1 label. Nothing differs, so nothing is dropped.",
      ),
    ).toBeTruthy();
    expect(within(dialog).queryByRole("radio")).toBeNull();
  });

  it("asks only where a field differs, and says which value is dropped", async () => {
    ipc.mergePeoplePreview.mockResolvedValue({
      details: 3,
      details_dropped: 0,
      labels: 0,
      timeline: 0,
      projects: 0,
      sources: 0,
      differs: { name: false, birthday: true, met: false, notes: true },
      lossless: false,
    });
    const dialog = await openDialog();
    expect(within(dialog).getByRole("group", { name: "Birthday" })).toBeTruthy();
    expect(within(dialog).getByRole("group", { name: "Notes" })).toBeTruthy();
    expect(within(dialog).queryByRole("group", { name: "Name" })).toBeNull();
    expect(within(dialog).getByText("“02-02-1991 (35)” is dropped.")).toBeTruthy();
    expect(within(dialog).getByRole("radio", { name: /^first note/ })).toBeTruthy();
  });

  it("sends the choices, with Keep both for the notes, and offers Undo", async () => {
    ipc.mergePeoplePreview.mockResolvedValue({
      details: 1,
      details_dropped: 0,
      labels: 0,
      timeline: 0,
      projects: 0,
      sources: 0,
      differs: { name: false, birthday: true, met: false, notes: true },
      lossless: false,
    });
    ipc.mergePeople.mockResolvedValue({
      survivor: 3,
      display_name: "Ben Sample",
      merged: 1,
      undo: "token-1",
    });
    const dialog = await openDialog();
    const birthday = within(within(dialog).getByRole("group", { name: "Birthday" }));
    fireEvent.click(birthday.getAllByRole("radio")[1]);
    fireEvent.click(within(dialog).getByRole("radio", { name: /Keep both/ }));
    expect(within(dialog).getByText(/joined with a blank line/)).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Merge" }));
    await waitFor(() =>
      expect(ipc.mergePeople).toHaveBeenCalledWith(3, 4, { birthday: "Folded", notes: "Both" }),
    );
    expect(await screen.findByText("Merged into Ben Sample.")).toBeTruthy();
    expect(screen.queryByRole("dialog", { name: /^Merge Ben/ })).toBeNull();
  });

  it("does not ask the person to type anything to confirm", async () => {
    const dialog = await openDialog();
    expect(within(dialog).queryByRole("textbox")).toBeNull();
  });

  it("says why when the merge is refused, and stays open", async () => {
    ipc.mergePeople.mockRejectedValue("Restore people from Trash before merging them.");
    const dialog = await openDialog();
    fireEvent.click(within(dialog).getByRole("button", { name: "Merge" }));
    expect(
      await within(dialog).findByText("Restore people from Trash before merging them."),
    ).toBeTruthy();
  });
});

describe("starting a merge from the list", () => {
  it("shows Merge in the selection bar only at exactly two", async () => {
    await view();
    tick("Ada Example");
    expect(screen.queryByRole("button", { name: "Merge…" })).toBeNull();
    tick("Cal Placeholder");
    expect(screen.getByRole("button", { name: "Merge…" })).toBeTruthy();
    tick("Ben Sample");
    expect(screen.queryByRole("button", { name: "Merge…" })).toBeNull();
  });

  it("right-click Merge with… picks the other person, then opens the dialog", async () => {
    ipc.searchPeople.mockResolvedValue([ADA, CAL]);
    await view();
    const li = screen.getByText("Cal Placeholder").closest("li")!;
    fireEvent.contextMenu(li);
    fireEvent.click(await screen.findByRole("menuitem", { name: "Merge with…" }));
    const picker = await screen.findByRole("dialog", { name: "Merge Cal Placeholder with…" });
    fireEvent.change(within(picker).getByRole("searchbox", { name: "Search people" }), {
      target: { value: "ada" },
    });
    const choice = await within(picker).findByRole(
      "button",
      { name: /Ada Example/ },
      { timeout: 2000 },
    );
    fireEvent.click(choice);
    // The older record (the lower id) stays: Ada is id 1.
    await screen.findByRole("dialog", { name: "Merge Ada Example and Cal Placeholder" });
    expect(ipc.mergePeoplePreview).toHaveBeenCalledWith(1, 5);
  });

  it("never offers the person themselves as the one to merge with", async () => {
    ipc.searchPeople.mockResolvedValue([CAL, BEN]);
    await view();
    fireEvent.contextMenu(screen.getByText("Cal Placeholder").closest("li")!);
    fireEvent.click(await screen.findByRole("menuitem", { name: "Merge with…" }));
    const picker = await screen.findByRole("dialog", { name: "Merge Cal Placeholder with…" });
    fireEvent.change(within(picker).getByRole("searchbox", { name: "Search people" }), {
      target: { value: "c" },
    });
    await within(picker).findByRole("button", { name: /Ben Sample/ }, { timeout: 2000 });
    expect(within(picker).queryByRole("button", { name: /Cal Placeholder/ })).toBeNull();
  });
});

describe("Undo after a merge", () => {
  async function mergeTwo() {
    ipc.mergePeople.mockResolvedValue({
      survivor: 3,
      display_name: "Ben Sample",
      merged: 1,
      undo: "token-2",
    });
    snapshot = mixed();
    await view();
    tick("Ben Sample");
    tick("Ben Sampel");
    fireEvent.click(screen.getByRole("button", { name: "Merge…" }));
    const dialog = await screen.findByRole("dialog", { name: "Merge Ben Sample and Ben Sampel" });
    await within(dialog).findByText(/Everything from both moves/);
    fireEvent.click(within(dialog).getByRole("button", { name: "Merge" }));
    await screen.findByText("Merged into Ben Sample.");
  }

  it("the strip's Undo takes the merge back", async () => {
    await mergeTwo();
    fireEvent.click(screen.getByRole("button", { name: "Undo: Merged into Ben Sample." }));
    await waitFor(() => expect(ipc.undoMergePeople).toHaveBeenCalledWith("token-2"));
  });

  it("Ctrl+Z presses it, as for Trash", async () => {
    await mergeTwo();
    fireEvent.keyDown(document.body, { key: "z", ctrlKey: true });
    await waitFor(() => expect(ipc.undoMergePeople).toHaveBeenCalledWith("token-2"));
  });

  it("says so in words when the merge can no longer be undone", async () => {
    ipc.undoMergePeople.mockRejectedValue("This merge can no longer be undone.");
    await mergeTwo();
    fireEvent.click(screen.getByRole("button", { name: "Undo: Merged into Ben Sample." }));
    expect(await screen.findByText("This merge can no longer be undone.")).toBeTruthy();
  });

  it("stays for 30 seconds, not the 12 of a Trash notice", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      await mergeTwo();
      await act(async () => {
        await vi.advanceTimersByTimeAsync(20_000);
      });
      expect(screen.getByText("Merged into Ben Sample.")).toBeTruthy();
      await act(async () => {
        await vi.advanceTimersByTimeAsync(11_000);
      });
      expect(screen.queryByText("Merged into Ben Sample.")).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });
});
