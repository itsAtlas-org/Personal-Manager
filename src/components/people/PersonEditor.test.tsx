// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The editor dialog (Contacts, slice 4b): validation in plain words, the reorder arrows, "Other
// label…", a date as three small fields (never a native date input), Save sending the `updated_at`
// the edit was based on, and a "changed elsewhere" refusal that keeps what was typed. Invented
// people only.

import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { CHANGED_ELSEWHERE } from "../../lib/personDraft";
import type { Person, PersonDetail } from "../../lib/types";

const ipc = vi.hoisted(() => ({
  createPerson: vi.fn(),
  updatePerson: vi.fn(),
  getPerson: vi.fn(),
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

import { ThemeProvider } from "../../theme";
import { PersonEditor } from "./PersonEditor";

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

function ada(over: Partial<Person> = {}): Person {
  return {
    id: 7,
    uid: "7".padStart(32, "0"),
    display_name: "Ada Example",
    given_name: "Ada",
    middle_name: "",
    family_name: "Example",
    name_prefix: "",
    name_suffix: "",
    birthday: null,
    met_where: "",
    met_on: null,
    notes: "Keep me.",
    starred: true,
    trashed_at: null,
    created_at: "2026-11-01T10:00:00Z",
    updated_at: "2026-11-20T10:00:00Z",
    details: [
      detail("email", "ada@example.com", { label: "Work", position: 0 }),
      detail("email", "ada.home@example.com", { label: "Home", position: 1 }),
    ],
    labels: [],
    last_met: null,
    timeline: { entries: [], next: null },
    sources: [],
    projects: [],
    ...over,
  };
}

const onClose = vi.fn();
const onSaved = vi.fn();

async function open(
  person: Person | null,
  section?: React.ComponentProps<typeof PersonEditor>["section"],
) {
  render(
    <ThemeProvider>
      <PersonEditor person={person} section={section} onClose={onClose} onSaved={onSaved} />
    </ThemeProvider>,
  );
  await act(async () => {});
}

const type = (label: string | RegExp, value: string) =>
  fireEvent.change(screen.getByLabelText(label), { target: { value } });
const notes = () => screen.getByRole("textbox", { name: "Notes" });
const press = (name: string | RegExp) => fireEvent.click(screen.getByRole("button", { name }));
const save = async (name = "Save") => {
  press(name);
  await act(async () => {});
};
/** The first argument list a save sent: [id, draft, base] for an update, [draft] for a create. */
const sentDraft = () =>
  ipc.updatePerson.mock.calls.length > 0
    ? ipc.updatePerson.mock.calls[0][1]
    : ipc.createPerson.mock.calls[0][0];

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
  nextId = 1;
  ipc.createPerson.mockResolvedValue(42);
  ipc.updatePerson.mockResolvedValue({ id: 7, display_name: "Ada Example", updated_at: "x" });
});

afterEach(cleanup);

describe("adding", () => {
  it("is titled for a new person and starts empty", async () => {
    await open(null);
    expect(screen.getByRole("dialog", { name: "Add a person" })).toBeTruthy();
    expect((screen.getByLabelText("Given name") as HTMLInputElement).value).toBe("");
  });

  it("creates the person and says who and that they are new, so the tab can select them", async () => {
    await open(null);
    type("Given name", "Cal");
    type("Family name", "Placeholder");
    await save("Save");
    expect(ipc.createPerson).toHaveBeenCalledTimes(1);
    expect(sentDraft()).toMatchObject({
      given_name: "Cal",
      family_name: "Placeholder",
      details: [],
    });
    expect(onSaved).toHaveBeenCalledWith(42, true);
    expect(ipc.updatePerson).not.toHaveBeenCalled();
  });

  it("refuses an empty person in words, and sends nothing", async () => {
    await open(null);
    await save("Save");
    expect(screen.getByText("Give them a name, an email or a phone number.")).toBeTruthy();
    expect(ipc.createPerson).not.toHaveBeenCalled();
    expect(onSaved).not.toHaveBeenCalled();
  });

  it("counts an email as enough to call them by", async () => {
    await open(null);
    press("Add an email");
    type("Email 1", "dee@example.com");
    await save("Save");
    expect(ipc.createPerson).toHaveBeenCalled();
    expect(sentDraft().details).toEqual([
      expect.objectContaining({ id: null, kind: "email", value: "dee@example.com" }),
    ]);
  });

  it("says why a job with only a title cannot be saved, beside the row", async () => {
    await open(null);
    type("Given name", "Cal");
    press("Add work");
    type("Work 1 job title", "Freelance designer");
    await save("Save");
    expect(screen.getByText("Add the company, or remove this row.")).toBeTruthy();
    expect(ipc.createPerson).not.toHaveBeenCalled();
  });

  it("shows the 500-details limit in the summary", async () => {
    await open(
      ada({
        details: Array.from({ length: 501 }, (_, i) =>
          detail("custom", `v${i}`, { label: "Field", position: i }),
        ),
      }),
    );
    await save();
    expect(screen.getByText("A person can hold up to 500 details.")).toBeTruthy();
    expect(ipc.updatePerson).not.toHaveBeenCalled();
  });

  it("drops a blank row instead of sending it", async () => {
    await open(null);
    type("Given name", "Cal");
    press("Add a phone number");
    await save("Save");
    expect(sentDraft().details).toEqual([]);
  });

  it("shows the backend's own words when it refuses, and keeps the dialog", async () => {
    ipc.createPerson.mockRejectedValue(
      "PM holds up to 100000 people. Delete some before adding more.",
    );
    await open(null);
    type("Given name", "Cal");
    await save("Save");
    expect(screen.getByText(/PM holds up to 100000 people/)).toBeTruthy();
    expect(onSaved).not.toHaveBeenCalled();
    expect((screen.getByLabelText("Given name") as HTMLInputElement).value).toBe("Cal");
  });
});

describe("validation", () => {
  it("says which field is too long and sends nothing", async () => {
    await open(ada());
    type("Given name", "x".repeat(1_025));
    await save();
    expect(screen.getByText(/The given name is too long/)).toBeTruthy();
    expect(ipc.updatePerson).not.toHaveBeenCalled();
  });

  it("says what is wrong with a date, next to it", async () => {
    await open(ada());
    type("Birthday day", "31");
    type("Birthday month", "04");
    await save();
    expect(screen.getByText("That isn't a real date.")).toBeTruthy();
    expect(ipc.updatePerson).not.toHaveBeenCalled();
  });

  it("needs a day and a month for a birthday, and calls the year optional", async () => {
    await open(ada());
    type("Birthday year", "1990");
    await save();
    expect(
      screen.getByText("A birthday needs a day and a month. The year is optional."),
    ).toBeTruthy();
  });

  it("sees a problem only after the first try to save", async () => {
    await open(ada());
    type("Birthday day", "99");
    expect(screen.queryByText("That isn't a real date.")).toBeNull();
  });
});

describe("dates are three small fields", () => {
  it("never draws a native date input", async () => {
    await open(ada());
    expect(document.querySelector('input[type="date"], input[type="month"]')).toBeNull();
    const group = screen.getByRole("group", { name: "Birthday" });
    expect(within(group).getAllByRole("textbox")).toHaveLength(3);
  });

  it("stores a birthday with no year as --MM-DD", async () => {
    await open(ada());
    type("Birthday day", "5");
    type("Birthday month", "4");
    await save();
    expect(sentDraft().birthday).toBe("--04-05");
  });

  it("stores a birthday with its year as YYYY-MM-DD", async () => {
    await open(ada());
    type("Birthday day", "15");
    type("Birthday month", "04");
    type("Birthday year", "1990");
    await save();
    expect(sentDraft().birthday).toBe("1990-04-15");
  });

  it("stores where you met as a year alone when that is all that is known", async () => {
    await open(ada());
    type("When you met year", "2019");
    await save();
    expect(sentDraft().met_on).toBe("2019");
  });

  it("fills the fields from a stored birthday", async () => {
    await open(ada({ birthday: "--02-29" }));
    expect((screen.getByLabelText("Birthday day") as HTMLInputElement).value).toBe("29");
    expect((screen.getByLabelText("Birthday month") as HTMLInputElement).value).toBe("02");
    expect((screen.getByLabelText("Birthday year") as HTMLInputElement).value).toBe("");
  });
});

describe("rows", () => {
  it("moves a value up or down with the arrows, which changes which one is primary", async () => {
    await open(ada());
    expect(
      screen.getByRole("button", { name: "Move Email 1 up" }).getAttribute("aria-disabled"),
    ).toBe("true");
    expect(
      screen.getByRole("button", { name: "Move Email 2 down" }).getAttribute("aria-disabled"),
    ).toBe("true");
    press("Move Email 1 down");
    expect((screen.getByLabelText("Email 1") as HTMLInputElement).value).toBe(
      "ada.home@example.com",
    );
    await save();
    const emails = sentDraft().details.filter((d: { kind: string }) => d.kind === "email");
    expect(emails.map((d: { value: string }) => d.value)).toEqual([
      "ada.home@example.com",
      "ada@example.com",
    ]);
    // Each keeps the id it had, so a sync's record of it survives.
    expect(emails.map((d: { id: number }) => d.id)).toEqual([2, 1]);
  });

  it("removes a value", async () => {
    await open(ada());
    press("Remove Email 1");
    await save();
    expect(sentDraft().details).toHaveLength(1);
  });

  it("offers the suggested labels and an Other label… that opens a free field", async () => {
    await open(ada());
    const select = screen.getByLabelText("Email 1 label") as HTMLSelectElement;
    const words = [...select.options].map((o) => o.textContent);
    expect(words).toEqual(["No label", "Home", "Work", "Other label…"]);
    expect(screen.queryByLabelText("Email 1 own label")).toBeNull();
    fireEvent.change(select, { target: { value: "__other__" } });
    type("Email 1 own label", "Godmother's");
    await save();
    expect(sentDraft().details[0]).toMatchObject({ kind: "email", label: "Godmother's" });
  });

  it("keeps the own-label field while its text matches a suggestion or is emptied", async () => {
    await open(ada({ details: [detail("email", "ada@example.com", { label: "Cell" })] }));
    type("Email 1 own label", "Work");
    expect(screen.getByLabelText("Email 1 own label")).toBeTruthy();
    type("Email 1 own label", "");
    expect(screen.getByLabelText("Email 1 own label")).toBeTruthy();
  });

  it("keeps focus in the dialog when a row goes: on the next row, or on the add buttons", async () => {
    await open(ada());
    press("Remove Email 1");
    expect(document.activeElement).toBe(screen.getByLabelText("Email 1"));
    press("Remove Email 1");
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Add an email" }));
  });

  it("keeps focus on the arrow that was pressed when its row reaches the end", async () => {
    await open(ada());
    const down = screen.getByRole("button", { name: "Move Email 1 down" });
    down.focus();
    fireEvent.click(down);
    const moved = screen.getByRole("button", { name: "Move Email 2 down" });
    expect(moved.getAttribute("aria-disabled")).toBe("true");
    expect(moved.hasAttribute("disabled")).toBe(false);
    // Button only styles :disabled, so the arrow carries the inert look itself, hover included.
    expect(moved.className).toContain("aria-disabled:text-faint");
    expect(moved.className).toContain("aria-disabled:hover:text-faint");
    expect(moved.className).toContain("aria-disabled:cursor-not-allowed");
  });

  it("opens the free field for a label that isn't a suggestion", async () => {
    await open(ada({ details: [detail("web", "@ada_example", { label: "Mastodon" })] }));
    expect((screen.getByLabelText("Link 1 own label") as HTMLInputElement).value).toBe("Mastodon");
  });

  it("suggests the social labels for a link", async () => {
    await open(null);
    press("Add a link or social");
    const select = screen.getByLabelText("Link 1 label") as HTMLSelectElement;
    expect([...select.options].map((o) => o.textContent)).toEqual([
      "No label",
      "Website",
      "Instagram",
      "LinkedIn",
      "Spotify",
      "Discord",
      "Other label…",
    ]);
  });

  it("offers Anniversary for a date and writes it as a date row", async () => {
    await open(ada());
    press("Add a date");
    fireEvent.change(screen.getByLabelText("Date 1 label"), { target: { value: "Anniversary" } });
    type("Date 1 day", "05");
    type("Date 1 month", "06");
    type("Date 1 year", "2020");
    await save();
    expect(sentDraft().details.at(-1)).toMatchObject({
      kind: "date",
      label: "Anniversary",
      value: "2020-06-05",
    });
  });

  it("keeps a job's title and department in its parts, with from and to as partial dates", async () => {
    await open(null);
    type("Given name", "Cal");
    press("Add work");
    type("Work 1 organisation", "Example Ltd");
    type("Work 1 job title", "Engineer");
    type("Work 1 department", "Platform");
    type("Work 1 from year", "2019");
    type("Work 1 from month", "03");
    fireEvent.click(screen.getByLabelText("Work 1 is current"));
    await save("Save");
    expect(sentDraft().details[0]).toMatchObject({
      kind: "work",
      value: "Example Ltd",
      parts: { title: "Engineer", department: "Platform" },
      from_date: "2019-03",
      to_date: null,
      is_current: true,
    });
  });

  it("lands on the section Add details named", async () => {
    await open(ada(), "dates");
    expect(document.activeElement?.closest("[data-section]")?.getAttribute("data-section")).toBe(
      "dates",
    );
  });
});

describe("Save and Cancel", () => {
  it("sends the whole record and the updated_at it was opened on", async () => {
    await open(ada());
    fireEvent.change(notes(), { target: { value: "Changed." } });
    await save();
    expect(ipc.updatePerson).toHaveBeenCalledTimes(1);
    const [id, draft, base] = ipc.updatePerson.mock.calls[0];
    expect(id).toBe(7);
    expect(base).toBe("2026-11-20T10:00:00Z");
    // The WHOLE record: a field left alone is still sent as it was, never blanked.
    expect(draft).toMatchObject({
      display_name: "Ada Example",
      given_name: "Ada",
      family_name: "Example",
      notes: "Changed.",
      starred: true,
    });
    expect(onSaved).toHaveBeenCalledWith(7, false);
  });

  it("closes at once with nothing changed", async () => {
    await open(ada());
    press("Cancel");
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("asks before throwing away edits, and keeps them on Keep editing", async () => {
    await open(ada());
    fireEvent.change(notes(), { target: { value: "Half a thought" } });
    press("Cancel");
    expect(onClose).not.toHaveBeenCalled();
    press("Keep editing");
    expect((notes() as HTMLTextAreaElement).value).toBe("Half a thought");
    press("Cancel");
    press("Throw away");
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(ipc.updatePerson).not.toHaveBeenCalled();
  });
});

describe("changed elsewhere", () => {
  it("keeps the dialog and the edits open, says so, and saves nothing over the newer copy", async () => {
    ipc.updatePerson.mockRejectedValueOnce(CHANGED_ELSEWHERE);
    await open(ada());
    fireEvent.change(notes(), { target: { value: "My edit." } });
    await save();
    expect(screen.getByText(/was changed somewhere else since you opened them/)).toBeTruthy();
    expect(screen.getByText(/What you typed is still here/)).toBeTruthy();
    expect((notes() as HTMLTextAreaElement).value).toBe("My edit.");
    expect(onSaved).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    expect(ipc.updatePerson).toHaveBeenCalledTimes(1);
    expect(ipc.getPerson).not.toHaveBeenCalled();
  });

  it("loads the latest only when asked, replaces the edits, and saves on the new base", async () => {
    ipc.updatePerson.mockRejectedValueOnce(CHANGED_ELSEWHERE);
    ipc.getPerson.mockResolvedValue(
      ada({ notes: "Their newer notes.", updated_at: "2026-11-25T08:00:00Z" }),
    );
    await open(ada());
    fireEvent.change(notes(), { target: { value: "My edit." } });
    await save();
    press("Load their latest details");
    await act(async () => {});
    expect(ipc.getPerson).toHaveBeenCalledWith(7);
    expect((notes() as HTMLTextAreaElement).value).toBe("Their newer notes.");
    expect(screen.queryByText(/was changed somewhere else/)).toBeNull();
    fireEvent.change(notes(), { target: { value: "Their newer notes, plus mine." } });
    await save();
    expect(ipc.updatePerson).toHaveBeenCalledTimes(2);
    expect(ipc.updatePerson.mock.calls[1][2]).toBe("2026-11-25T08:00:00Z");
    expect(onSaved).toHaveBeenCalledWith(7, false);
  });

  it("shows any other refusal in the backend's words and also keeps the edits", async () => {
    ipc.updatePerson.mockRejectedValueOnce("That person is in Trash. Restore them to edit.");
    await open(ada());
    fireEvent.change(notes(), { target: { value: "My edit." } });
    await save();
    expect(screen.getByText("That person is in Trash. Restore them to edit.")).toBeTruthy();
    expect(screen.queryByText(/changed somewhere else/)).toBeNull();
    expect((notes() as HTMLTextAreaElement).value).toBe("My edit.");
  });
});
