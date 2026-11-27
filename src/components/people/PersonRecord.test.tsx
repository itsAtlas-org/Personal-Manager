// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The right pane's record (Contacts, slice 4b): each section is drawn and an empty one is not, dates
// and ages (with the clock pinned), links that open only when they are http, https or mailto, Copy,
// the label chips, "Add details", the sources and the project links. Invented people only.

import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { Person, PersonDetail, PersonLabel, PersonRow } from "../../lib/types";

const ipc = vi.hoisted(() => ({
  getPerson: vi.fn(),
  updatePerson: vi.fn(),
  listProjects: vi.fn(),
  linkPersonProject: vi.fn(),
  unlinkPersonProject: vi.fn(),
}));
vi.mock("../../lib/ipc", () => {
  const known: Record<string, unknown> = { ...ipc };
  return new Proxy(known, {
    has: () => true,
    get(target, prop: string) {
      if (prop === "then") return undefined;
      if (!(prop in target)) target[prop] = vi.fn(async () => []);
      return target[prop];
    },
  });
});
const copied = vi.hoisted(() => vi.fn());
vi.mock("../../lib/contextMenu", async (orig) => ({
  ...(await orig<typeof import("../../lib/contextMenu")>()),
  copyViaHost: copied,
}));
const opened = vi.hoisted(() => vi.fn());
vi.mock("../../lib/linkFailure", async (orig) => ({
  ...(await orig<typeof import("../../lib/linkFailure")>()),
  openLink: opened,
}));

import { ThemeProvider } from "../../theme";
import { PersonRecord } from "./PersonRecord";

const row: PersonRow = {
  id: 1,
  uid: "1".padStart(32, "0"),
  display_name: "Ada Example",
  primary: null,
  company: null,
  labels: [],
  starred: false,
  last_met: null,
  google_state: null,
  trashed_at: null,
  birthday: null,
  leap_day: null,
};

const LABELS: PersonLabel[] = [
  { id: 1, uid: "l1".padEnd(32, "0"), name: "Friends", people: 1 },
  { id: 2, uid: "l2".padEnd(32, "0"), name: "Climbing", people: 0 },
];

let nextId = 1;
function detail(kind: PersonDetail["kind"], value: string, over: Partial<PersonDetail> = {}) {
  return {
    id: nextId++,
    kind,
    label: "",
    value,
    parts: null,
    from_date: null,
    to_date: null,
    is_current: false,
    position: 0,
    ...over,
  } satisfies PersonDetail;
}

function bare(over: Partial<Person> = {}): Person {
  return {
    id: 1,
    uid: row.uid,
    display_name: "Ada Example",
    given_name: "Ada",
    middle_name: "",
    family_name: "Example",
    name_prefix: "",
    name_suffix: "",
    birthday: null,
    leap_day: null,
    met_where: "",
    met_on: null,
    notes: "",
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
    ...over,
  };
}

function full(): Person {
  return bare({
    birthday: "1990-04-15",
    met_where: "A climbing wall in Leeds",
    met_on: "2019-05",
    notes: "Likes tea.\nAllergic to cats.",
    labels: [{ id: 1, name: "Friends" }],
    last_met: "2026-11-12",
    timeline: { entries: [], next: null },
    sources: [
      {
        id: 1,
        provider: "vcard",
        account: "",
        state: "linked",
        synced_at: null,
        created_at: "2026-11-12T09:00:00Z",
        dirty: false,
        open_conflicts: 0,
      },
    ],
    projects: ["Atlas"],
    details: [
      detail("nickname", "Ads"),
      detail("former_name", "Ada Sample"),
      detail("email", "ada@example.com", { label: "Work" }),
      detail("phone", "555-0101", { label: "Mobile" }),
      detail("address", "1 Example Road\nLeeds", { label: "Home" }),
      detail("web", "https://example.com/ada", { label: "Website" }),
      detail("web", "@ada_example", { label: "Instagram", position: 1 }),
      detail("web", "javascript:alert(1)", { label: "Odd", position: 2 }),
      detail("work", "Example Ltd", {
        parts: { title: "Engineer" },
        from_date: "2019-03",
        is_current: true,
      }),
      detail("date", "2020-06-05", { label: "Anniversary" }),
      detail("location", "Leeds", { from_date: "2026-03" }),
      detail("relation", "Bea Example", { label: "Sister" }),
      detail("custom", "Biscuit", { label: "Pet" }),
    ],
  });
}

const handlers = {
  run: vi.fn(async (write: () => Promise<unknown>) => {
    await write();
    return true;
  }),
  onEdit: vi.fn(),
  onStar: vi.fn(),
  onToggleLabel: vi.fn(),
  onNewLabel: vi.fn(),
  onOpenProject: vi.fn(),
};

async function show(
  person: Person,
  props: Partial<React.ComponentProps<typeof PersonRecord>> = {},
) {
  ipc.getPerson.mockResolvedValue(person);
  render(
    <ThemeProvider>
      <PersonRecord
        row={{ ...row, display_name: person.display_name }}
        version={0}
        labels={LABELS}
        {...handlers}
        {...props}
      />
    </ThemeProvider>,
  );
  await act(async () => {});
}

const region = (name: string) => screen.queryByRole("region", { name });

beforeAll(() => {
  if (!("ResizeObserver" in globalThis)) {
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
});

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  nextId = 1;
  ipc.listProjects.mockResolvedValue(["Atlas", "Garden"]);
  ipc.linkPersonProject.mockResolvedValue("Garden");
  ipc.unlinkPersonProject.mockResolvedValue(undefined);
  // Only the date: timers stay real so the awaits in these tests behave.
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(new Date(2026, 10, 26, 12, 0, 0));
});

afterEach(() => {
  vi.useRealTimers();
  cleanup();
});

describe("the sections", () => {
  it("draws each section a full record has", async () => {
    await show(full());
    for (const name of [
      "Contact",
      "Work and school",
      "Dates",
      "Where you met",
      "Where they are now",
      "Relations",
      "Custom fields",
      "Notes",
      "Projects",
      "Sources",
    ]) {
      expect(region(name), name).toBeTruthy();
    }
    expect(within(region("Contact")!).getByText("ada@example.com")).toBeTruthy();
    expect(within(region("Contact")!).getByText("555-0101")).toBeTruthy();
    expect(within(region("Work and school")!).getByText("Example Ltd")).toBeTruthy();
    expect(within(region("Work and school")!).getByText("Engineer")).toBeTruthy();
    expect(within(region("Where they are now")!).getByText(/Leeds/)).toBeTruthy();
    expect(within(region("Relations")!).getByText("Bea Example")).toBeTruthy();
    expect(within(region("Custom fields")!).getByText("Biscuit")).toBeTruthy();
    expect(within(region("Where you met")!).getByText("A climbing wall in Leeds")).toBeTruthy();
    expect(within(region("Notes")!).getByText(/Likes tea/)).toBeTruthy();
  });

  it("hides every section a person has nothing in, and offers them under Add details", async () => {
    await show(bare());
    for (const name of [
      "Contact",
      "Work and school",
      "Dates",
      "Where you met",
      "Where they are now",
      "Relations",
      "Custom fields",
      "Notes",
      "Sources",
    ]) {
      expect(region(name), name).toBeNull();
    }
    // Projects is where a person gets linked, so it always shows.
    expect(region("Projects")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Add details" }));
    const offered = screen.getAllByRole("button").map((b) => b.textContent);
    for (const word of ["Contact", "Work and school", "Dates", "Notes", "Other names"]) {
      expect(offered).toContain(word);
    }
  });

  it("opens the editor at the section Add details names", async () => {
    const person = bare({ notes: "Something." });
    await show(person);
    fireEvent.click(screen.getByRole("button", { name: "Add details" }));
    fireEvent.click(screen.getByRole("button", { name: "Contact" }));
    expect(handlers.onEdit).toHaveBeenCalledWith(person, "contact");
  });

  it("opens the editor with Edit", async () => {
    const person = full();
    await show(person);
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    expect(handlers.onEdit).toHaveBeenCalledWith(person);
  });

  it("draws the timeline only when the tab hands one in, and then leaves 'Last met' to it", async () => {
    await show(full());
    expect(region("Timeline")).toBeNull();
    expect(screen.getByText(/^Last met 12-11/)).toBeTruthy();
    cleanup();
    await show(full(), {
      timeline: () => <section aria-label="Timeline">Meetings</section>,
    });
    expect(region("Timeline")).toBeTruthy();
    // Said once, in the section, not again in the header.
    expect(screen.queryByText(/^Last met 12-11/)).toBeNull();
  });

  it("offers Log a meeting beside Edit only when the tab can open the entry dialog", async () => {
    await show(full());
    expect(screen.queryByRole("button", { name: "Log a meeting" })).toBeNull();
    cleanup();
    const onLogMeeting = vi.fn();
    await show(full(), { onLogMeeting });
    fireEvent.click(screen.getByRole("button", { name: "Log a meeting" }));
    expect(onLogMeeting).toHaveBeenCalledOnce();
  });

  it("folds a section and remembers it on this device", async () => {
    await show(bare({ notes: "Something." }));
    const notes = region("Notes")!;
    const fold = () => within(region("Notes")!).getByRole("button", { name: /Notes/ });
    expect(fold().getAttribute("aria-expanded")).toBe("true");
    fireEvent.click(within(notes).getByRole("button", { name: /Notes/ }));
    expect(fold().getAttribute("aria-expanded")).toBe("false");
    cleanup();
    await show(bare({ notes: "Something." }));
    expect(fold().getAttribute("aria-expanded")).toBe("false");
  });

  it("draws a person in Trash without any control that changes them", async () => {
    await show(bare({ trashed_at: "2026-11-20T10:00:00Z", projects: ["Atlas"] }));
    expect(screen.queryByRole("button", { name: "Edit" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Add details" })).toBeNull();
    expect(screen.queryByRole("button", { name: /Star this person/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "Unlink Atlas" })).toBeNull();
    expect(screen.getByText(/In Trash since/)).toBeTruthy();
  });

  it("says plainly when the person can't be read, and tries again", async () => {
    ipc.getPerson.mockRejectedValueOnce("boom");
    render(
      <ThemeProvider>
        <PersonRecord row={row} version={0} labels={LABELS} {...handlers} />
      </ThemeProvider>,
    );
    await act(async () => {});
    expect(screen.getByText("PM couldn't read this person just now.")).toBeTruthy();
    ipc.getPerson.mockResolvedValue(bare({ notes: "Back." }));
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    await act(async () => {});
    expect(screen.getByText("Back.")).toBeTruthy();
  });
});

describe("dates", () => {
  it("shows a birthday with its year as DD-MM-YYYY and the age, as of today", async () => {
    await show(full());
    expect(within(region("Dates")!).getByText("15-04-1990 (36)")).toBeTruthy();
  });

  it("shows a birthday with no year as the day and month, with no age", async () => {
    await show(bare({ birthday: "--04-15" }));
    expect(within(region("Dates")!).getByText("15 April")).toBeTruthy();
  });

  it("counts the age down by one before the birthday has come this year", async () => {
    await show(bare({ birthday: "1990-12-25" }));
    expect(within(region("Dates")!).getByText("25-12-1990 (35)")).toBeTruthy();
  });

  it("asks a 29 February birthday with no answer which day to mark, and saves the click", async () => {
    ipc.updatePerson.mockResolvedValue({ id: 1, display_name: "Ada Example", updated_at: "x" });
    await show(bare({ birthday: "--02-29" }));
    const dates = region("Dates")!;
    expect(
      within(dates).getByText("Born on 29 February. Which day should PM mark in other years?"),
    ).toBeTruthy();
    fireEvent.click(within(dates).getByRole("button", { name: "1 March" }));
    await act(async () => {});
    expect(ipc.updatePerson).toHaveBeenCalledTimes(1);
    const [id, draft, base] = ipc.updatePerson.mock.calls[0];
    expect(id).toBe(1);
    expect(base).toBe("2026-11-20T10:00:00Z");
    expect(draft.birthday).toBe("--02-29");
    expect(draft.leap_day).toBe("mar1");
    // The whole record goes, not just the answer.
    expect(draft.display_name).toBe("Ada Example");
  });

  it("moves focus to the answer line once the question is answered", async () => {
    ipc.updatePerson.mockResolvedValue({ id: 1, display_name: "Ada Example", updated_at: "x" });
    const asked = bare({ birthday: "--02-29" });
    ipc.getPerson.mockResolvedValue(asked);
    const ui = (version: number) => (
      <ThemeProvider>
        <PersonRecord
          row={{ ...row, display_name: asked.display_name }}
          version={version}
          labels={LABELS}
          {...handlers}
        />
      </ThemeProvider>
    );
    const { rerender } = render(ui(0));
    await act(async () => {});
    const button = screen.getByRole("button", { name: "1 March" });
    button.focus();
    fireEvent.click(button);
    await act(async () => {});
    ipc.getPerson.mockResolvedValue({ ...asked, leap_day: "mar1" });
    rerender(ui(1));
    await act(async () => {});
    const line = screen.getByText("In years without 29 February, marked on 1 March.");
    expect(document.activeElement).not.toBe(document.body);
    expect(document.activeElement).toBe(line.closest("li"));
  });

  it("says the answer instead once there is one, and does not ask again", async () => {
    await show(bare({ birthday: "--02-29", leap_day: "feb28" }));
    const dates = region("Dates")!;
    expect(within(dates).queryByText(/Which day should PM mark/)).toBeNull();
    expect(
      within(dates).getByText("In years without 29 February, marked on 28 February."),
    ).toBeTruthy();
  });

  it("does not ask about any other birthday, or about someone in Trash", async () => {
    await show(bare({ birthday: "--04-15" }));
    expect(screen.queryByText(/Which day should PM mark/)).toBeNull();
    cleanup();
    await show(bare({ birthday: "--02-29", trashed_at: "2026-11-25T10:00:00Z" }));
    expect(screen.queryByText(/Which day should PM mark/)).toBeNull();
  });

  it("says so when the record changed since it was read, and keeps asking", async () => {
    ipc.updatePerson.mockRejectedValue(
      "This person was changed somewhere else since you opened them. Close and reopen them to see the latest.",
    );
    await show(bare({ birthday: "--02-29" }));
    fireEvent.click(screen.getByRole("button", { name: "28 February" }));
    await act(async () => {});
    expect(screen.getByText(/They were changed somewhere else/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "1 March" })).toBeTruthy();
  });

  it("shows another date and when you met, in the house format", async () => {
    await show(full());
    expect(within(region("Dates")!).getByText("Anniversary")).toBeTruthy();
    expect(within(region("Dates")!).getByText("05-06-2020")).toBeTruthy();
    expect(within(region("Where you met")!).getByText("05-2019")).toBeTruthy();
  });

  it("says when you last met and how long ago, or that you haven't", async () => {
    await show(full());
    expect(screen.getByText(/^Last met 12-11/)).toBeTruthy();
    expect(screen.getByText(/\(2 weeks ago\)/)).toBeTruthy();
    cleanup();
    await show(bare());
    expect(screen.getByText("Not met yet")).toBeTruthy();
  });
});

describe("links and Copy", () => {
  it("opens an https address and a mailto one through the one link choke point", async () => {
    await show(full());
    fireEvent.click(screen.getByRole("button", { name: "https://example.com/ada" }));
    expect(opened).toHaveBeenLastCalledWith("https://example.com/ada");
    fireEvent.click(screen.getByRole("button", { name: "ada@example.com" }));
    expect(opened).toHaveBeenLastCalledWith("mailto:ada@example.com");
  });

  it("shows a javascript: value and a bare handle as inert text, never a link", async () => {
    await show(full());
    const contact = within(region("Contact")!);
    expect(contact.getByText("javascript:alert(1)")).toBeTruthy();
    expect(contact.queryByRole("button", { name: "javascript:alert(1)" })).toBeNull();
    expect(contact.queryByRole("link")).toBeNull();
    expect(contact.getByText("@ada_example")).toBeTruthy();
    expect(contact.queryByRole("button", { name: "@ada_example" })).toBeNull();
    // Nothing in the record is ever a tel: or javascript: target.
    expect(document.querySelector('[href^="tel:"], [href^="javascript:"]')).toBeNull();
    expect(opened).not.toHaveBeenCalled();
  });

  it("does not link a phone number, and Copy hands it to the host with what it was", async () => {
    await show(full());
    expect(screen.queryByRole("button", { name: "555-0101" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Copy 555-0101" }));
    expect(copied).toHaveBeenCalledWith("555-0101", "Phone number copied");
    fireEvent.click(screen.getByRole("button", { name: "Copy @ada_example" }));
    expect(copied).toHaveBeenLastCalledWith("@ada_example", "Link copied");
    fireEvent.click(screen.getByRole("button", { name: "Copy ada@example.com" }));
    expect(copied).toHaveBeenLastCalledWith("ada@example.com", "Email address copied");
  });

  it("renders markup in a value as text", async () => {
    await show(
      bare({
        notes: "<img src=x onerror=alert(1)>",
        details: [detail("custom", "<b>x</b>", { label: "Odd" })],
      }),
    );
    expect(document.querySelector("img")).toBeNull();
    expect(document.querySelector("b")).toBeNull();
    expect(screen.getByText("<b>x</b>")).toBeTruthy();
  });
});

describe("names, star and labels", () => {
  it("shows the other names under the name", async () => {
    await show(full());
    expect(screen.getByText("Also known as Ads")).toBeTruthy();
    expect(screen.getByText("Formerly Ada Sample")).toBeTruthy();
  });

  it("stars and unstars", async () => {
    await show(bare());
    fireEvent.click(screen.getByRole("button", { name: "Star this person" }));
    expect(handlers.onStar).toHaveBeenCalledWith(true);
  });

  it("shows the labels as chips and takes one off", async () => {
    await show(full());
    const chips = screen.getByRole("list", { name: "Labels" });
    expect(within(chips).getByText("Friends")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Remove the label Friends" }));
    expect(handlers.onToggleLabel).toHaveBeenCalledWith(1, false);
  });

  it("keeps focus in the record when a label chip goes: on the next chip, or on the picker", async () => {
    const two = bare({
      projects: [],
      labels: [
        { id: 1, name: "Friends" },
        { id: 2, name: "Climbing" },
      ],
    });
    ipc.getPerson.mockResolvedValue(two);
    const ui = (version: number) => (
      <ThemeProvider>
        <PersonRecord
          row={{ ...row, display_name: two.display_name }}
          version={version}
          labels={LABELS}
          {...handlers}
        />
      </ThemeProvider>
    );
    const { rerender } = render(ui(0));
    await act(async () => {});
    const first = screen.getByRole("button", { name: "Remove the label Friends" });
    first.focus();
    fireEvent.click(first);
    ipc.getPerson.mockResolvedValue({ ...two, labels: [{ id: 2, name: "Climbing" }] });
    rerender(ui(1));
    await act(async () => {});
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Remove the label Climbing" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Remove the label Climbing" }));
    ipc.getPerson.mockResolvedValue({ ...two, labels: [] });
    rerender(ui(2));
    await act(async () => {});
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Add label" }));
  });

  it("adds a label from the picker, which can also make a new one", async () => {
    await show(full());
    fireEvent.click(screen.getByRole("button", { name: "Add label" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Climbing" }));
    expect(handlers.onToggleLabel).toHaveBeenCalledWith(2, true);
    fireEvent.click(screen.getByRole("button", { name: /New label/ }));
    expect(handlers.onNewLabel).toHaveBeenCalled();
  });
});

describe("sources", () => {
  it("words a file import with its date and invents nothing for another provider", async () => {
    const person = full();
    person.sources.push({
      id: 2,
      provider: "google",
      account: "someone@example.com",
      state: "linked",
      synced_at: null,
      created_at: "2026-11-13T09:00:00Z",
      dirty: false,
      open_conflicts: 0,
    });
    await show(person);
    const sources = within(region("Sources")!);
    expect(sources.getByText(/^Imported from a file on 12-11/)).toBeTruthy();
    expect(sources.getByText(/^Linked from another address book on 13-11/)).toBeTruthy();
    expect(sources.queryByText(/Google/)).toBeNull();
    expect(sources.queryByText(/someone@example.com/)).toBeNull();
  });
});

describe("projects", () => {
  it("shows each linked project as a chip that opens it", async () => {
    await show(full());
    fireEvent.click(screen.getByRole("button", { name: "Open project Atlas" }));
    expect(handlers.onOpenProject).toHaveBeenCalledWith("Atlas");
  });

  it("links a project by the name typed, which may be new", async () => {
    await show(full());
    fireEvent.change(screen.getByRole("combobox", { name: "Link a project" }), {
      target: { value: "  Garden " },
    });
    fireEvent.click(screen.getByRole("button", { name: "Link" }));
    await act(async () => {});
    expect(ipc.linkPersonProject).toHaveBeenCalledWith(1, "Garden");
    expect(handlers.run).toHaveBeenCalled();
  });

  it("keeps the typed name when the link is refused", async () => {
    await show(full());
    handlers.run.mockImplementationOnce(async (write: () => Promise<unknown>) => {
      await write().catch(() => {});
      return false;
    });
    ipc.linkPersonProject.mockRejectedValueOnce("That name is too long.");
    const box = screen.getByRole("combobox", { name: "Link a project" }) as HTMLInputElement;
    fireEvent.change(box, { target: { value: "Garden" } });
    fireEvent.click(screen.getByRole("button", { name: "Link" }));
    await act(async () => {});
    expect(box.value).toBe("Garden");
  });

  it("does not link one it already has, however it is cased", async () => {
    await show(full());
    fireEvent.change(screen.getByRole("combobox", { name: "Link a project" }), {
      target: { value: "atlas" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Link" }));
    await act(async () => {});
    expect(ipc.linkPersonProject).not.toHaveBeenCalled();
  });

  it("unlinks a project", async () => {
    await show(full());
    fireEvent.click(screen.getByRole("button", { name: "Unlink Atlas" }));
    await act(async () => {});
    expect(ipc.unlinkPersonProject).toHaveBeenCalledWith(1, "Atlas");
  });

  it("keeps focus in the record when a project is unlinked: on the next chip, or on the box", async () => {
    const two = bare({ projects: ["Atlas", "Garden"] });
    ipc.getPerson.mockResolvedValue(two);
    const ui = (version: number) => (
      <ThemeProvider>
        <PersonRecord
          row={{ ...row, display_name: two.display_name }}
          version={version}
          labels={LABELS}
          {...handlers}
        />
      </ThemeProvider>
    );
    const { rerender } = render(ui(0));
    await act(async () => {});
    const first = screen.getByRole("button", { name: "Unlink Atlas" });
    first.focus();
    fireEvent.click(first);
    await act(async () => {});
    ipc.getPerson.mockResolvedValue({ ...two, projects: ["Garden"] });
    rerender(ui(1));
    await act(async () => {});
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Unlink Garden" }));
    fireEvent.click(screen.getByRole("button", { name: "Unlink Garden" }));
    await act(async () => {});
    ipc.getPerson.mockResolvedValue({ ...two, projects: [] });
    rerender(ui(2));
    await act(async () => {});
    expect(document.activeElement).toBe(screen.getByRole("combobox", { name: "Link a project" }));
  });

  it("makes the chips plain text when nothing can open a project", async () => {
    await show(full(), { onOpenProject: undefined });
    expect(screen.queryByRole("button", { name: "Open project Atlas" })).toBeNull();
    expect(within(region("Projects")!).getByText("Atlas")).toBeTruthy();
  });
});
