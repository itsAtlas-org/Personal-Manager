// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Merge {a} and {b}" (spec 4.3). It previews what moves, counted from the rows themselves, and asks
// only where a single-value field differs: the name, the birthday, where and when you met, the
// notes (which also offer Keep both). Each choice says plainly which value is dropped. There is no
// type-to-confirm: a merge is taken back from the Undo strip for 30 seconds, which is the safety net.
//
// The older record (the lower id) is the survivor and keeps its id and uid; the other is folded in.
// Both are read here so the choices show real values, and the preview is the backend's: which fields
// differ is its rule (`people::merge::differences`), never re-derived in the page.

import { useEffect, useId, useState } from "react";
import { getPerson, mergePeople, mergePeoplePreview } from "../../lib/ipc";
import { errorWords } from "../../lib/people";
import { differingFields, mergeBody, mergeExtrasLine } from "../../lib/peopleMerge";
import { describeBirthdayChoice, formatPartial } from "../../lib/partialDate";
import type {
  MergeChoices,
  MergeDone,
  PeopleMergePreview,
  MergeSide,
  Person,
} from "../../lib/types";
import { Button, Callout, Dialog, RadioChoice } from "../ui";

type Field = "name" | "birthday" | "met" | "notes";
type Pick = MergeSide | "Both";

const LEGEND: Record<Field, string> = {
  name: "Name",
  birthday: "Birthday",
  met: "Where and when you met",
  notes: "Notes",
};

/** A person's name as the choice shows it: the display name, with the parts when they say more. */
function nameText(p: Person): string {
  const parts = [p.name_prefix, p.given_name, p.middle_name, p.family_name, p.name_suffix]
    .filter(Boolean)
    .join(" ");
  return parts && parts !== p.display_name ? `${p.display_name} (${parts})` : p.display_name;
}

function metText(p: Person): string {
  const when = p.met_on ? formatPartial(p.met_on) : "";
  return [p.met_where, when].filter(Boolean).join(", ") || "nothing";
}

function noteText(p: Person): string {
  const t = p.notes.trim();
  return t.length > 80 ? `${t.slice(0, 80)}…` : t || "nothing";
}

function valueOf(field: Field, p: Person, today: Date): string {
  switch (field) {
    case "name":
      return nameText(p);
    case "birthday":
      return describeBirthdayChoice(p.birthday, today, p.leap_day) ?? p.birthday ?? "nothing";
    case "met":
      return metText(p);
    case "notes":
      return noteText(p);
  }
}

interface Props {
  /** The two people, in either order. */
  a: number;
  b: number;
  onClose: () => void;
  onMerged: (done: MergeDone) => void;
}

export function MergePeopleDialog({ a, b, onClose, onMerged }: Props) {
  const into = Math.min(a, b);
  const from = Math.max(a, b);
  const base = useId();
  const [people, setPeople] = useState<[Person, Person] | null>(null);
  const [preview, setPreview] = useState<PeopleMergePreview | null>(null);
  const [picks, setPicks] = useState<Partial<Record<Field, Pick>>>({});
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const today = new Date();

  useEffect(() => {
    let cancelled = false;
    void Promise.all([getPerson(into), getPerson(from), mergePeoplePreview(into, from)])
      .then(([first, second, p]) => {
        if (cancelled) return;
        setPeople([first, second]);
        setPreview(p);
      })
      .catch((err: unknown) => {
        if (!cancelled) setProblem(errorWords(err, "PM couldn't read these two people."));
      });
    return () => {
      cancelled = true;
    };
  }, [into, from]);

  const fields = preview ? differingFields(preview.differs) : [];

  async function merge() {
    if (!preview || busy) return;
    const choices: MergeChoices = {};
    for (const f of fields) {
      const pick = picks[f] ?? "Survivor";
      if (f === "notes") choices.notes = pick;
      else choices[f] = pick === "Both" ? "Survivor" : pick;
    }
    setBusy(true);
    setProblem(null);
    try {
      onMerged(await mergePeople(into, from, choices));
    } catch (err) {
      setProblem(errorWords(err));
      setBusy(false);
    }
  }

  const title = people
    ? `Merge ${people[0].display_name} and ${people[1].display_name}`
    : "Merge two people";

  return (
    <Dialog
      open
      onClose={() => {
        if (!busy) onClose();
      }}
      title={title}
      helpId="people-merge"
      widthClassName="max-w-lg"
      footer={
        <>
          <Button variant="tertiary" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" onClick={() => void merge()} disabled={!preview || busy}>
            {busy ? "Merging…" : "Merge"}
          </Button>
        </>
      }
    >
      {preview && people ? (
        <>
          <p className="mt-2 text-sm leading-relaxed text-ink2">{mergeBody(preview)}</p>
          {mergeExtrasLine(preview) && (
            <p className="mt-1 text-sm leading-relaxed text-ink2">{mergeExtrasLine(preview)}</p>
          )}
          {fields.map((f) => {
            const first = valueOf(f, people[0], today);
            const second = valueOf(f, people[1], today);
            const current = picks[f] ?? "Survivor";
            const set = (v: Pick) => setPicks((prev) => ({ ...prev, [f]: v }));
            return (
              <fieldset key={f} className="mt-4">
                <legend className="text-xs font-medium text-ink3">{LEGEND[f]}</legend>
                <RadioChoice
                  name={`${base}-${f}`}
                  value="Survivor"
                  current={current}
                  onSelect={set}
                  label={first}
                  detail={`“${second}” is dropped.`}
                  disabled={busy}
                />
                <RadioChoice
                  name={`${base}-${f}`}
                  value="Folded"
                  current={current}
                  onSelect={set}
                  label={second}
                  detail={`“${first}” is dropped.`}
                  disabled={busy}
                />
                {f === "notes" && (
                  <RadioChoice
                    name={`${base}-${f}`}
                    value="Both"
                    current={current}
                    onSelect={set}
                    label="Keep both"
                    detail="Nothing is dropped: the two notes are joined with a blank line."
                    disabled={busy}
                  />
                )}
              </fieldset>
            );
          })}
          <p className="mt-4 text-xs text-ink3">
            {people[0].display_name} stays and {people[1].display_name} is folded in. You can undo
            this for 30 seconds afterwards.
          </p>
        </>
      ) : (
        !problem && <p className="mt-2 text-sm text-ink3">Reading both people…</p>
      )}
      {problem && (
        <Callout tone="danger" size="sm" className="mt-3">
          {problem}
        </Callout>
      )}
    </Dialog>
  );
}
