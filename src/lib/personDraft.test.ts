// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The editor's model: a stored person to text and back, the caps in plain words, reorder, and what
// a save sends. Invented people only.

import { describe, expect, it } from "vitest";
import {
  blankRow,
  isChangedElsewhere,
  modelFromPerson,
  modelToDraft,
  moveRow,
  NOTES_MAX,
  rowsOfSection,
  sameModel,
  validateModel,
  VALUE_MAX,
  DETAILS_MAX,
} from "./personDraft";
import type { Person, PersonDetail } from "./types";

function detail(
  id: number,
  kind: PersonDetail["kind"],
  value: string,
  over: Partial<PersonDetail> = {},
) {
  return {
    id,
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

function person(details: PersonDetail[] = [], over: Partial<Person> = {}): Person {
  return {
    id: 7,
    uid: "7".repeat(32),
    display_name: "Ada Example",
    given_name: "Ada",
    middle_name: "",
    family_name: "Example",
    name_prefix: "",
    name_suffix: "",
    birthday: "--04-15",
    met_where: "A climbing wall",
    met_on: "2019-03",
    notes: "Likes tea.",
    starred: true,
    trashed_at: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-02-02T00:00:00.000Z",
    details,
    labels: [],
    last_met: null,
    timeline: { entries: [], next: null },
    sources: [],
    projects: [],
    ...over,
  };
}

describe("model and draft", () => {
  it("round-trips a stored person into the same draft, ids kept", () => {
    const p = person([
      detail(1, "email", "ada@example.com", { label: "Home" }),
      detail(2, "phone", "555-0100", { label: "Mobile" }),
      detail(3, "work", "Example Ltd", {
        parts: { title: "Engineer", department: "Platform", extra: "kept" },
        from_date: "2019-03",
        is_current: true,
      }),
      detail(4, "date", "2015-06-01", { label: "Anniversary" }),
    ]);
    const draft = modelToDraft(modelFromPerson(p));
    expect(draft).toMatchObject({
      display_name: "Ada Example",
      birthday: "--04-15",
      met_where: "A climbing wall",
      met_on: "2019-03",
      notes: "Likes tea.",
      starred: true,
    });
    expect(draft.details.map((d) => [d.id, d.kind, d.label, d.value])).toEqual([
      [1, "email", "Home", "ada@example.com"],
      [2, "phone", "Mobile", "555-0100"],
      [3, "work", "", "Example Ltd"],
      [4, "date", "Anniversary", "2015-06-01"],
    ]);
    // A key the editor has no field for survives; the dates come back as stored.
    expect(draft.details[2]).toMatchObject({
      parts: { title: "Engineer", department: "Platform", extra: "kept" },
      from_date: "2019-03",
      is_current: true,
    });
  });

  it("drops a cleared job title from the parts rather than keeping the old one", () => {
    const p = person([detail(3, "work", "Example Ltd", { parts: { title: "Engineer" } })]);
    const model = modelFromPerson(p);
    model.rows[0].title = "";
    expect(modelToDraft(model).details[0].parts).toBeNull();
  });

  it("keeps an address's parts only while its text is untouched", () => {
    const p = person([
      detail(5, "address", "1 Example Road", { parts: { street: "1 Example Road" } }),
    ]);
    const model = modelFromPerson(p);
    expect(modelToDraft(model).details[0].parts).toEqual({ street: "1 Example Road" });
    model.rows[0].value = "2 Example Road";
    expect(modelToDraft(model).details[0].parts).toBeNull();
  });

  it("leaves out a blank row, as the backend does, and sends a new row without an id", () => {
    const model = modelFromPerson(null);
    model.given = "Ben";
    const phone = blankRow("phone");
    phone.value = "555-0101";
    model.rows = [blankRow("email"), phone];
    const draft = modelToDraft(model);
    expect(draft.details).toHaveLength(1);
    expect(draft.details[0]).toMatchObject({ id: null, kind: "phone", value: "555-0101" });
    expect(draft.starred).toBe(false);
  });

  it("sameModel ignores row keys but sees an edit", () => {
    const p = person([detail(1, "email", "ada@example.com")]);
    const a = modelFromPerson(p);
    const b = modelFromPerson(p);
    expect(sameModel(a, b)).toBe(true);
    b.rows[0].value = "other@example.com";
    expect(sameModel(a, b)).toBe(false);
  });
});

describe("validateModel", () => {
  it("accepts a person with a name", () => {
    expect(validateModel(modelFromPerson(person()))).toEqual({});
  });

  it("needs something to call them by, and counts an email or phone", () => {
    const empty = modelFromPerson(null);
    expect(validateModel(empty).who).toBe("Give them a name, an email or a phone number.");
    const row = blankRow("email");
    row.value = "x@example.com";
    expect(validateModel({ ...empty, rows: [row] }).who).toBeUndefined();
    // A blank row doesn't count.
    expect(validateModel({ ...empty, rows: [blankRow("phone")] }).who).toBeDefined();
  });

  it("says which field is too long, in the backend's words", () => {
    const m = modelFromPerson(person());
    m.given = "a".repeat(VALUE_MAX + 1);
    expect(validateModel(m).given).toBe(
      "The given name is too long (1,025 of 1,024 characters allowed).",
    );
    m.given = "a".repeat(VALUE_MAX);
    expect(validateModel(m).given).toBeUndefined();
    // Characters, not UTF-16 units.
    m.family = "😀".repeat(VALUE_MAX);
    expect(validateModel(m).family).toBeUndefined();
  });

  it("caps notes in bytes and details in number", () => {
    const m = modelFromPerson(person());
    m.notes = "é".repeat(NOTES_MAX / 2 + 1);
    expect(validateModel(m).notes).toMatch(/notes is too long/);
    m.notes = "";
    m.rows = Array.from({ length: DETAILS_MAX + 1 }, () => {
      const r = blankRow("custom");
      r.value = "x";
      return r;
    });
    expect(validateModel(m).details).toBe("A person can hold up to 500 details.");
  });

  it("flags a job, school or place with no main text but other things typed, rather than dropping it", () => {
    const m = modelFromPerson(person());
    const job = blankRow("work");
    job.title = "Freelance designer";
    const school = blankRow("school");
    school.from = { day: "", month: "", year: "2008" };
    const place = blankRow("location");
    place.from = { day: "", month: "03", year: "2026" };
    const empty = blankRow("work");
    m.rows = [job, school, place, empty];
    const problems = validateModel(m);
    expect(problems[`row:${job.key}:value`]).toBe("Add the company, or remove this row.");
    expect(problems[`row:${school.key}:value`]).toBe("Add the school, or remove this row.");
    expect(problems[`row:${place.key}:value`]).toBe("Add the place, or remove this row.");
    // A row with nothing at all is still dropped quietly.
    expect(problems[`row:${empty.key}:value`]).toBeUndefined();
    job.value = "Example Ltd";
    expect(validateModel({ ...m, rows: [job] })).toEqual({});
  });

  it("flags a date that is not real, on the row or the field it is in", () => {
    const m = modelFromPerson(person());
    m.born = { day: "31", month: "4", year: "" };
    m.met = { day: "", month: "5", year: "" };
    const when = blankRow("date");
    when.when = { day: "", month: "", year: "20" };
    const job = blankRow("work");
    job.value = "Example Ltd";
    job.from = { day: "", month: "13", year: "2020" };
    m.rows = [when, job];
    const problems = validateModel(m);
    expect(problems.born).toBe("That isn't a real date.");
    expect(problems.met).toMatch(/year first/);
    expect(problems[`row:${when.key}:when`]).toBe("Use a four-digit year.");
    expect(problems[`row:${job.key}:from`]).toBe("That isn't a real date.");
  });
});

describe("reordering", () => {
  const rows = () => {
    const a = blankRow("email");
    const b = blankRow("phone");
    const c = blankRow("email");
    const d = blankRow("email");
    return { a, b, c, d, all: [a, b, c, d] };
  };

  it("moves a row past the next of its own kind, skipping other kinds", () => {
    const { a, b, c, d, all } = rows();
    expect(moveRow(all, a.key, 1).map((r) => r.key)).toEqual([c.key, b.key, a.key, d.key]);
    expect(moveRow(all, d.key, -1).map((r) => r.key)).toEqual([a.key, b.key, d.key, c.key]);
  });

  it("leaves the first up and the last down where they are", () => {
    const { a, d, all } = rows();
    expect(moveRow(all, a.key, -1).map((r) => r.key)).toEqual(all.map((r) => r.key));
    expect(moveRow(all, d.key, 1).map((r) => r.key)).toEqual(all.map((r) => r.key));
  });

  it("lists a section's rows grouped by kind in the section's order", () => {
    const { a, b, c, d, all } = rows();
    expect(rowsOfSection(all, "contact").map((r) => r.key)).toEqual([a.key, c.key, d.key, b.key]);
  });

  it("changes the order a save sends: the first of a kind is the primary", () => {
    const m = modelFromPerson(person());
    const first = blankRow("email");
    first.value = "one@example.com";
    const second = blankRow("email");
    second.value = "two@example.com";
    m.rows = moveRow([first, second], second.key, -1);
    expect(modelToDraft(m).details.map((d) => d.value)).toEqual([
      "two@example.com",
      "one@example.com",
    ]);
  });
});

describe("isChangedElsewhere", () => {
  it("recognises the backend's refusal and nothing else", () => {
    expect(
      isChangedElsewhere(
        "This person was changed somewhere else since you opened them. Close and reopen them to see the latest.",
      ),
    ).toBe(true);
    expect(isChangedElsewhere("The notes is too long.")).toBe(false);
    expect(isChangedElsewhere(new Error("This person was changed somewhere else"))).toBe(false);
  });
});
