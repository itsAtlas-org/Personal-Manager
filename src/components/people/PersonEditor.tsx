// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The editor: one dialog for adding a person and changing one, with Save and Cancel and no
// autosave (E6: one save is one change). It holds the person as text (`personDraft.ts` turns that
// into a draft and checks it in plain words) and sends the WHOLE record on Save.
//
// A save carries the `updated_at` the person had when the dialog opened. When the backend says the
// record moved since (a sync edited it, or another window), the dialog stays open with everything
// the person typed, says so, and offers to load the latest. It never saves over the newer copy, and
// loading the latest is the person's own choice: it replaces their edits.
//
// "Add details" on a record opens this at the section it names (`section`), scrolled to and focused.

import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { createPerson, getPerson, updatePerson } from "../../lib/ipc";
import { LeapDayChoice } from "./LeapDayChoice";
import { PartialDateFields } from "./PartialDateFields";
import { DetailRow } from "./DetailRow";
import { errorWords } from "../../lib/people";
import {
  blankRow,
  isChangedElsewhere,
  modelFromPerson,
  modelToDraft,
  moveRow,
  readsLeapDay,
  rowsOfSection,
  sameModel,
  validateModel,
  type EditorModel,
  type EditorRow,
} from "../../lib/personDraft";
import { SECTION_TITLES, type SectionId } from "../../lib/personRecord";
import type { Person, PersonDetailKind } from "../../lib/types";
import { Button, Callout, ConfirmDialog, Dialog, Field, Input, Textarea } from "../ui";

/** What each kind is called, for the row names a screen reader hears ("Email 2"). */
const KIND_NAMES: Record<PersonDetailKind, string> = {
  email: "Email",
  phone: "Phone",
  address: "Address",
  web: "Link",
  work: "Work",
  school: "School",
  nickname: "Nickname",
  former_name: "Former name",
  date: "Date",
  relation: "Relation",
  custom: "Custom field",
  location: "Place",
};

/** The buttons that add a row, per section. */
const ADD: Partial<Record<SectionId, { kind: PersonDetailKind; words: string }[]>> = {
  name: [
    { kind: "nickname", words: "Add a nickname" },
    { kind: "former_name", words: "Add a former name" },
  ],
  contact: [
    { kind: "email", words: "Add an email" },
    { kind: "phone", words: "Add a phone number" },
    { kind: "address", words: "Add an address" },
    { kind: "web", words: "Add a link or social" },
  ],
  work: [
    { kind: "work", words: "Add work" },
    { kind: "school", words: "Add school" },
  ],
  dates: [{ kind: "date", words: "Add a date" }],
  now: [{ kind: "location", words: "Add a place" }],
  relations: [{ kind: "relation", words: "Add a relation" }],
  custom: [{ kind: "custom", words: "Add a custom field" }],
};

/** The order the dialog lists its sections in. */
const ORDER: SectionId[] = [
  "name",
  "contact",
  "work",
  "dates",
  "met",
  "now",
  "relations",
  "custom",
  "notes",
];

interface Props {
  /** The person to change, or null to add one. */
  person: Person | null;
  /** The section to open at (from "Add details"). */
  section?: SectionId;
  onClose: () => void;
  /** Saved: their id, and whether they are new. The dialog is closed by the owner. */
  onSaved: (id: number, created: boolean) => void;
}

export function PersonEditor({ person, section, onClose, onSaved }: Props) {
  const [model, setModel] = useState<EditorModel>(() => modelFromPerson(person));
  const [initial, setInitial] = useState<EditorModel>(model);
  // The `updated_at` this edit is based on; replaced when the person loads the latest.
  const [base, setBase] = useState(person?.updated_at ?? "");
  const [attempted, setAttempted] = useState(false);
  const [busy, setBusy] = useState(false);
  const [refusal, setRefusal] = useState<string | null>(null);
  const [moved, setMoved] = useState(false);
  const [discarding, setDiscarding] = useState(false);
  const [reloading, setReloading] = useState(false);
  const body = useRef<HTMLDivElement>(null);
  const focusKey = useRef<number | null>(null);
  // The section whose add buttons take focus after its last row was removed.
  const focusAdders = useRef<SectionId | null>(null);

  const problems = useMemo(() => validateModel(model), [model]);
  const shown = attempted ? problems : {};
  const dirty = !sameModel(model, initial);

  const patch = (p: Partial<EditorModel>) => setModel((m) => ({ ...m, ...p }));
  const patchRow = (key: number, p: Partial<EditorRow>) =>
    setModel((m) => ({ ...m, rows: m.rows.map((r) => (r.key === key ? { ...r, ...p } : r)) }));
  const addRow = (kind: PersonDetailKind) => {
    const row = blankRow(kind);
    focusKey.current = row.key;
    setModel((m) => ({ ...m, rows: [...m.rows, row] }));
  };

  // A new row takes focus, and so does a removed row's neighbour (or the section's add buttons when
  // it was the last): the button that was clicked is gone, and focus must not drop to the page
  // behind the dialog. "Add details" lands on its section.
  useEffect(() => {
    if (focusKey.current !== null) {
      const key = focusKey.current;
      focusKey.current = null;
      body.current
        ?.querySelector<HTMLElement>(`[data-row="${key}"] input, [data-row="${key}"] textarea`)
        ?.focus();
    } else if (focusAdders.current !== null) {
      const id = focusAdders.current;
      focusAdders.current = null;
      body.current
        ?.querySelector<HTMLElement>(`[data-section="${id}"] [data-adders] button`)
        ?.focus();
    }
  }, [model.rows]);
  useEffect(() => {
    if (!section) return;
    const at = body.current?.querySelector<HTMLElement>(`[data-section="${section}"]`);
    at?.scrollIntoView?.({ block: "start" });
    at?.querySelector<HTMLElement>("input, textarea, select, button")?.focus();
    // Only the first draw lands on the section; later edits must not pull the dialog back.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  function close() {
    if (busy) return;
    if (dirty) setDiscarding(true);
    else onClose();
  }

  async function save() {
    if (busy) return;
    setAttempted(true);
    setRefusal(null);
    if (Object.keys(validateModel(model)).length > 0) return;
    setBusy(true);
    try {
      const draft = modelToDraft(model);
      if (person) {
        await updatePerson(person.id, draft, base);
        onSaved(person.id, false);
      } else {
        const id = await createPerson(draft);
        onSaved(id, true);
      }
    } catch (err) {
      if (isChangedElsewhere(err)) setMoved(true);
      else setRefusal(errorWords(err, "PM couldn't save that. Try again."));
      setBusy(false);
    }
  }

  /** Their latest details, in place of what was typed: only ever on the person's say-so. */
  async function loadLatest() {
    if (!person) return;
    setReloading(true);
    try {
      const fresh = await getPerson(person.id);
      const next = modelFromPerson(fresh);
      setModel(next);
      setInitial(next);
      setBase(fresh.updated_at);
      setMoved(false);
      setAttempted(false);
    } catch (err) {
      setRefusal(errorWords(err, "PM couldn't read them just now. Try again."));
    } finally {
      setReloading(false);
    }
  }

  const rowName = (r: EditorRow) =>
    `${KIND_NAMES[r.kind]} ${model.rows.filter((x) => x.kind === r.kind).indexOf(r) + 1}`;

  function rowsFor(id: SectionId): ReactNode {
    const rows = rowsOfSection(model.rows, id);
    if (rows.length === 0) return null;
    return (
      <ul className="m-0 flex list-none flex-col gap-3 p-0">
        {rows.map((r) => {
          const sameKind = model.rows.filter((x) => x.kind === r.kind);
          return (
            <DetailRow
              key={r.key}
              row={r}
              name={rowName(r)}
              problems={shown}
              first={sameKind[0] === r}
              last={sameKind[sameKind.length - 1] === r}
              onChange={(p) => patchRow(r.key, p)}
              onMove={(by) => setModel((m) => ({ ...m, rows: moveRow(m.rows, r.key, by) }))}
              onRemove={() => {
                const at = rows.indexOf(r);
                const next = rows[at + 1] ?? rows[at - 1];
                if (next) focusKey.current = next.key;
                else focusAdders.current = id;
                setModel((m) => ({ ...m, rows: m.rows.filter((x) => x.key !== r.key) }));
              }}
            />
          );
        })}
      </ul>
    );
  }

  const adders = (id: SectionId) => (
    <div data-adders className="flex flex-wrap gap-2">
      {(ADD[id] ?? []).map((a) => (
        <Button key={a.kind} variant="secondary" size="xs" onClick={() => addRow(a.kind)}>
          {a.words}
        </Button>
      ))}
    </div>
  );

  const text = (key: keyof EditorModel, label: string, extra?: string) => (
    <Field label={label} error={shown[key as string]} className="min-w-36 flex-1">
      {(c) => (
        <Input
          {...c}
          value={model[key] as string}
          placeholder={extra}
          onChange={(e) => patch({ [key]: e.target.value } as Partial<EditorModel>)}
        />
      )}
    </Field>
  );

  const heading = (id: SectionId, title = SECTION_TITLES[id]) => (
    <h3 className="font-mono text-xs uppercase tracking-wide text-ink4">{title}</h3>
  );

  const sections: Record<SectionId, ReactNode> = {
    name: (
      <>
        <div className="flex flex-wrap gap-3">
          {text("prefix", "Prefix", "Dr.")}
          {text("given", "Given name")}
          {text("middle", "Middle name")}
          {text("family", "Family name")}
          {text("suffix", "Suffix", "Jr.")}
        </div>
        {text("display", "Display name", "PM puts it together from the parts above")}
        {rowsFor("name")}
        {adders("name")}
      </>
    ),
    contact: (
      <>
        {rowsFor("contact")}
        {adders("contact")}
      </>
    ),
    work: (
      <>
        {rowsFor("work")}
        {adders("work")}
      </>
    ),
    dates: (
      <>
        <div className="flex flex-col gap-1">
          <span className="text-sm text-ink2">Birthday</span>
          <PartialDateFields
            name="Birthday"
            fields={model.born}
            error={shown.born}
            // Changing the date away from 29 February hides the question and forgets the answer.
            onChange={(born) => patch({ born, ...(readsLeapDay(born) ? {} : { leap: null }) })}
          />
          {readsLeapDay(model.born) && (
            <LeapDayChoice
              value={model.leap}
              error={shown.leap}
              onChange={(leap) => patch({ leap })}
            />
          )}
        </div>
        {rowsFor("dates")}
        {adders("dates")}
      </>
    ),
    met: (
      <>
        {text("metWhere", "Where you met")}
        <div className="flex flex-col gap-1">
          <span className="text-sm text-ink2">When you met</span>
          <PartialDateFields
            name="When you met"
            fields={model.met}
            error={shown.met}
            onChange={(met) => patch({ met })}
          />
        </div>
      </>
    ),
    now: (
      <>
        {rowsFor("now")}
        {adders("now")}
      </>
    ),
    relations: (
      <>
        {rowsFor("relations")}
        {adders("relations")}
      </>
    ),
    custom: (
      <>
        {rowsFor("custom")}
        {adders("custom")}
      </>
    ),
    notes: (
      <Field label="Notes" error={shown.notes}>
        {(c) => (
          <Textarea
            {...c}
            rows={5}
            value={model.notes}
            onChange={(e) => patch({ notes: e.target.value })}
          />
        )}
      </Field>
    ),
  };

  const count = Object.keys(shown).length;
  return (
    <>
      <Dialog
        open
        chrome="bar"
        widthClassName="max-w-3xl"
        helpId="people-editor"
        title={person ? `Edit ${person.display_name}` : "Add a person"}
        closeLabel="Cancel"
        closeDisabled={busy}
        onClose={close}
        footer={
          <Button disabled={busy || reloading} onClick={() => void save()}>
            Save
          </Button>
        }
      >
        <div ref={body} className="flex flex-col gap-6">
          {moved && (
            <Callout tone="warning" size="sm" body="ink">
              <p>
                This person was changed somewhere else since you opened them. What you typed is
                still here, and nothing has been saved over their newer details.
              </p>
              <div className="mt-2">
                <Button
                  variant="secondary"
                  size="xs"
                  disabled={reloading}
                  onClick={() => void loadLatest()}
                >
                  Load their latest details
                </Button>{" "}
                <span className="text-xs text-ink3">This replaces what you typed.</span>
              </div>
            </Callout>
          )}
          {refusal && (
            <Callout tone="danger" size="sm">
              {refusal}
            </Callout>
          )}
          {attempted && count > 0 && (
            <Callout tone="danger" size="sm">
              {shown.who ??
                shown.details ??
                (count === 1
                  ? "One thing needs a look before you save."
                  : `${count} things need a look before you save.`)}
            </Callout>
          )}
          {ORDER.map((id) => (
            <section
              key={id}
              data-section={id}
              aria-label={SECTION_TITLES[id]}
              className="flex flex-col gap-3"
            >
              {heading(id)}
              {sections[id]}
            </section>
          ))}
        </div>
      </Dialog>
      <ConfirmDialog
        open={discarding}
        title="Throw away your changes?"
        confirmLabel="Throw away"
        cancelLabel="Keep editing"
        danger
        initialFocus="cancel"
        onConfirm={() => {
          setDiscarding(false);
          onClose();
        }}
        onClose={() => setDiscarding(false)}
      >
        <p className="mt-2 text-sm text-ink2">
          {person
            ? "What you changed here hasn't been saved."
            : "This person hasn't been added yet."}
        </p>
      </ConfirmDialog>
    </>
  );
}
