// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The editor's model of one person, as pure functions: the stored record to editable text,
// validation that mirrors the backend's caps in plain words, and the text back to the draft a save
// sends. No IPC, no React. The backend re-checks everything (it is the authority); this exists so a
// mistake is said next to the field, before the save, and so the dialog can hold half-typed dates
// that aren't values yet.
//
// A draft carries the WHOLE record: `details` replaces the stored list, a detail that keeps its `id`
// is updated in place (so a sync's bookkeeping on it survives) and a stored one the draft leaves out
// is deleted. So every detail of the person goes through here, including kinds the editor draws no
// date or label control for: those keep what they had.

import {
  fieldsFromIso,
  isoFromFields,
  NO_DATE,
  type DateFields,
  type DateMode,
} from "./partialDate";
import { SECTION_KINDS, type SectionId } from "./personRecord";
import type { Person, PersonDetailDraft, PersonDetailKind, PersonDraft } from "./types";

// --- the caps (mirror `people::VALUE_MAX`, `NOTES_MAX`, `DETAILS_MAX`) --------------------------------

/** Characters in one value (a name part, a label, a detail's value). */
export const VALUE_MAX = 1_024;
/** Bytes in a person's notes. */
export const NOTES_MAX = 64 * 1_024;
/** Details one person may hold. */
export const DETAILS_MAX = 500;

/** What the backend says when a save finds the record moved (`people::CHANGED_ELSEWHERE`). */
export const CHANGED_ELSEWHERE =
  "This person was changed somewhere else since you opened them. Close and reopen them to see the latest.";

/** Whether a rejected save was that refusal. The phrase is the backend's constant; matching its
 *  start keeps this working if the ending is reworded. */
export function isChangedElsewhere(err: unknown): boolean {
  return typeof err === "string" && err.startsWith("This person was changed somewhere else");
}

// --- the model -----------------------------------------------------------------------------------------

/** One detail row as the editor holds it. The date fields are text: half-typed is a state. */
export interface EditorRow {
  /** Local identity for React and focus; not the stored `id`. */
  key: number;
  /** The stored row this edits; null for one added here. */
  id: number | null;
  kind: PersonDetailKind;
  label: string;
  /** The value, as typed. For a `date` detail this is unused: the date is `when`. */
  value: string;
  /** A job title, or a school's field of study. */
  title: string;
  department: string;
  /** A `date` detail's own date. */
  when: DateFields;
  from: DateFields;
  to: DateFields;
  isCurrent: boolean;
  /** What the stored row had, so keys the editor draws no field for survive a save. */
  parts: Record<string, string> | null;
  /** The stored value, to tell whether an address was edited. */
  storedValue: string;
  /** The stored dates, for kinds the editor draws no date control for. */
  storedFrom: string | null;
  storedTo: string | null;
}

export interface EditorModel {
  display: string;
  prefix: string;
  given: string;
  middle: string;
  family: string;
  suffix: string;
  born: DateFields;
  metWhere: string;
  met: DateFields;
  notes: string;
  starred: boolean;
  rows: EditorRow[];
}

/** Whether the editor draws from/to date fields for this kind. */
export const hasDates = (kind: PersonDetailKind) =>
  kind === "work" || kind === "school" || kind === "location";

/** Whether the editor draws a label control for this kind (a job has none). */
export const hasLabel = (kind: PersonDetailKind) =>
  !["work", "school", "nickname", "former_name", "location"].includes(kind);

let nextKey = 1;
export const newKey = () => nextKey++;

/** A blank row of `kind`. */
export function blankRow(kind: PersonDetailKind): EditorRow {
  return {
    key: newKey(),
    id: null,
    kind,
    label: "",
    value: "",
    title: "",
    department: "",
    when: { ...NO_DATE },
    from: { ...NO_DATE },
    to: { ...NO_DATE },
    isCurrent: false,
    parts: null,
    storedValue: "",
    storedFrom: null,
    storedTo: null,
  };
}

/** The model for a stored person; with null, an empty one (Add person). */
export function modelFromPerson(person: Person | null): EditorModel {
  if (!person) {
    return {
      display: "",
      prefix: "",
      given: "",
      middle: "",
      family: "",
      suffix: "",
      born: { ...NO_DATE },
      metWhere: "",
      met: { ...NO_DATE },
      notes: "",
      starred: false,
      rows: [],
    };
  }
  // The same order the record lists them in: by kind in section order, then position.
  const order = Object.values(SECTION_KINDS).flat();
  const details = [...person.details].sort(
    (a, b) =>
      order.indexOf(a.kind) - order.indexOf(b.kind) || a.position - b.position || a.id - b.id,
  );
  return {
    display: person.display_name,
    prefix: person.name_prefix,
    given: person.given_name,
    middle: person.middle_name,
    family: person.family_name,
    suffix: person.name_suffix,
    born: fieldsFromIso(person.birthday),
    metWhere: person.met_where,
    met: fieldsFromIso(person.met_on),
    notes: person.notes,
    starred: person.starred,
    rows: details.map((d) => {
      const parts = d.parts ?? null;
      return {
        key: newKey(),
        id: d.id,
        kind: d.kind,
        label: d.label,
        value: d.value,
        title: parts?.title ?? parts?.field ?? "",
        department: parts?.department ?? "",
        when: d.kind === "date" ? fieldsFromIso(d.value) : { ...NO_DATE },
        from: fieldsFromIso(d.from_date),
        to: fieldsFromIso(d.to_date),
        isCurrent: d.is_current,
        parts,
        storedValue: d.value,
        storedFrom: d.from_date,
        storedTo: d.to_date,
      };
    }),
  };
}

// --- validation ----------------------------------------------------------------------------------------

/** Plain-words problems, by where they show: a field name (`given`, `born`, `notes`, `who`, `details`)
 *  or `row:<key>:<field>` (`value`, `label`, `title`, `department`, `when`, `from`, `to`). */
export type Problems = Record<string, string>;

const chars = (s: string) => [...s].length;
const bytes = (s: string) => new TextEncoder().encode(s).length;
const tooLong = (what: string, n: number, max: number, unit: string) =>
  `The ${what} is too long (${n.toLocaleString("en-GB")} of ${max.toLocaleString("en-GB")} ${unit} allowed).`;

/** The mode each of a row's date fields takes. */
const WHEN_MODE: DateMode = "any";
const SPAN_MODE: DateMode = "dated";

/** The first thing wrong with each field, in words the person can act on. Empty when the model can
 *  be saved. A blank row is not a problem: it is dropped, as the backend drops it. */
export function validateModel(model: EditorModel): Problems {
  const out: Problems = {};
  const single = (key: string, what: string, text: string) => {
    const n = chars(text.trim());
    if (n > VALUE_MAX) out[key] = tooLong(what, n, VALUE_MAX, "characters");
  };
  single("display", "name", model.display);
  single("prefix", "name prefix", model.prefix);
  single("given", "given name", model.given);
  single("middle", "middle name", model.middle);
  single("family", "family name", model.family);
  single("suffix", "name suffix", model.suffix);
  single("metWhere", "place you met", model.metWhere);
  const notesBytes = bytes(model.notes);
  if (notesBytes > NOTES_MAX) out.notes = tooLong("notes", notesBytes, NOTES_MAX, "bytes");

  const born = isoFromFields(model.born, "born");
  if (born.error) out.born = born.error;
  const met = isoFromFields(model.met, "dated");
  if (met.error) out.met = met.error;

  const kept = model.rows.filter((r) => !isBlank(r));
  if (kept.length > DETAILS_MAX) out.details = `A person can hold up to ${DETAILS_MAX} details.`;
  for (const r of model.rows) {
    const at = (field: string) => `row:${r.key}:${field}`;
    if (r.kind === "date") {
      const when = isoFromFields(r.when, WHEN_MODE);
      if (when.error) out[at("when")] = when.error;
    } else {
      single(at("value"), "value", r.value);
      // A row with no main text is dropped on save, and with it the job title, dates and the rest
      // the person typed beside it: say so, rather than close the dialog as if they were kept.
      if (r.value.trim() === "" && !out[at("value")] && holdsMore(r))
        out[at("value")] =
          r.kind === "work"
            ? "Add the company, or remove this row."
            : r.kind === "school"
              ? "Add the school, or remove this row."
              : "Add the place, or remove this row.";
    }
    if (hasLabel(r.kind)) single(at("label"), "label", r.label);
    single(at("title"), r.kind === "school" ? "field of study" : "job title", r.title);
    single(at("department"), "department", r.department);
    if (hasDates(r.kind)) {
      const from = isoFromFields(r.from, SPAN_MODE);
      if (from.error) out[at("from")] = from.error;
      if (r.kind !== "location") {
        const to = isoFromFields(r.to, SPAN_MODE);
        if (to.error) out[at("to")] = to.error;
      }
    }
  }

  const callable =
    model.display.trim() ||
    model.prefix.trim() ||
    model.given.trim() ||
    model.middle.trim() ||
    model.family.trim() ||
    model.suffix.trim() ||
    kept.some((r) => ["nickname", "work", "email", "phone"].includes(r.kind));
  if (!callable) out.who = "Give them a name, an email or a phone number.";
  return out;
}

/** Whether a job, school or place row with no main text still holds something typed beside it. */
function holdsMore(r: EditorRow): boolean {
  const any = (f: DateFields) =>
    f.day.trim() !== "" || f.month.trim() !== "" || f.year.trim() !== "";
  if (r.kind === "location") return any(r.from);
  if (r.kind !== "work" && r.kind !== "school") return false;
  return (
    r.title.trim() !== "" || r.department.trim() !== "" || any(r.from) || any(r.to) || r.isCurrent
  );
}

/** A row with nothing in its value: dropped on save, like the backend does. */
export function isBlank(r: EditorRow): boolean {
  if (r.kind === "date") {
    const w = r.when;
    return w.day.trim() === "" && w.month.trim() === "" && w.year.trim() === "";
  }
  return r.value.trim() === "";
}

// --- to a draft ------------------------------------------------------------------------------------------

function draftOf(r: EditorRow): PersonDetailDraft {
  let value = r.value;
  if (r.kind === "date") value = isoFromFields(r.when, WHEN_MODE).iso ?? "";

  let parts: Record<string, string> | null = r.parts;
  if (r.kind === "work" || r.kind === "school") {
    const next: Record<string, string> = { ...(r.parts ?? {}) };
    const key = r.kind === "school" ? "field" : "title";
    // Set what is typed; take out what was cleared, so an emptied field is gone, not kept.
    for (const [k, v] of [
      [key, r.title.trim()],
      ...(r.kind === "work" ? ([["department", r.department.trim()]] as const) : []),
    ]) {
      if (v) next[k] = v;
      else delete next[k];
    }
    parts = Object.keys(next).length > 0 ? next : null;
  } else if (r.kind === "address" && value.trim() !== r.storedValue.trim()) {
    // The structured parts describe the text they were read from. Once the text is edited they no
    // longer do, and the text is the person's own: it stands alone.
    parts = null;
  }

  const spans = hasDates(r.kind);
  const from = spans ? isoFromFields(r.from, SPAN_MODE).iso : r.storedFrom;
  const to = r.kind === "location" || !spans ? r.storedTo : isoFromFields(r.to, SPAN_MODE).iso;
  return {
    id: r.id,
    kind: r.kind,
    label: hasLabel(r.kind) ? r.label.trim() : r.label,
    value,
    parts,
    from_date: from,
    to_date: to,
    is_current: r.isCurrent,
  };
}

/** The draft to send: the whole record, blank rows dropped, rows in the order the editor shows
 *  them (the backend numbers each kind's rows from 0 in that order). Call after `validateModel`
 *  came back empty. */
export function modelToDraft(model: EditorModel): PersonDraft {
  return {
    display_name: model.display.trim(),
    given_name: model.given,
    middle_name: model.middle,
    family_name: model.family,
    name_prefix: model.prefix,
    name_suffix: model.suffix,
    birthday: isoFromFields(model.born, "born").iso,
    met_where: model.metWhere,
    met_on: isoFromFields(model.met, "dated").iso,
    notes: model.notes,
    starred: model.starred,
    details: model.rows.filter((r) => !isBlank(r)).map(draftOf),
  };
}

/** A section's rows as the editor draws them, and as a save will number them: grouped by kind in the
 *  section's order, each kind in the order the rows are held in. */
export function rowsOfSection(rows: readonly EditorRow[], id: SectionId): EditorRow[] {
  const kinds = SECTION_KINDS[id];
  return kinds.flatMap((k) => rows.filter((r) => r.kind === k));
}

/** Swap a row with the next one of its own kind (`by` -1 up, +1 down). Order only means something
 *  within a kind (the first email is the primary one), so that is the only neighbour there is; a row
 *  at the end of its kind stays put. */
export function moveRow(rows: readonly EditorRow[], key: number, by: -1 | 1): EditorRow[] {
  const next = [...rows];
  const i = next.findIndex((r) => r.key === key);
  if (i < 0) return next;
  const kind = next[i].kind;
  let j = i + by;
  while (j >= 0 && j < next.length && next[j].kind !== kind) j += by;
  if (j < 0 || j >= next.length) return next;
  [next[i], next[j]] = [next[j], next[i]];
  return next;
}

/** Whether two models hold the same edits (is there anything to lose by closing?). */
export function sameModel(a: EditorModel, b: EditorModel): boolean {
  // `key` is only local identity: two models with the same edits differ in it by construction.
  const flat = (m: EditorModel) =>
    JSON.stringify({ ...m, rows: m.rows.map((r) => ({ ...r, key: 0 })) });
  return flat(a) === flat(b);
}
