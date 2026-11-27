// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The right pane's record: the name block (name, other names, when you last met, star and labels),
// then a section for each kind of thing PM holds about them. A section with nothing in it is not
// drawn; "Add details" offers those, and opens the editor at the one chosen. Each section folds, and
// the fold is remembered on this device.
//
// The record re-reads its person whenever `version` moves (the tab bumps it on every change notice
// and after every write of its own), so it follows an edit from anywhere. Until the first read lands
// the header is drawn from the list's row, so the pane never flashes empty.
//
// Everything a contact's text says is untrusted (a file or a sync wrote it), so it renders as React
// text only. A web value opens only through `openLink` and only when `linkTarget` accepts it
// (http, https or mailto); anything else, a handle or a `javascript:` string, is text with Copy.
// There are no `tel:` links.

import { useEffect, useRef, useState, type ReactNode } from "react";
import { copyViaHost } from "../../lib/contextMenu";
import { useRememberedFold } from "../../lib/foldPrefs";
import { formatDate } from "../../lib/format";
import { getPerson } from "../../lib/ipc";
import { openLink } from "../../lib/linkFailure";
import { formatPartial, describeBirthday } from "../../lib/partialDate";
import { personCircleBackground, personInitials } from "../../lib/people";
import {
  copiedWords,
  detailsOfKinds,
  emptySections,
  lastMetLine,
  linkTarget,
  mailTarget,
  rangeText,
  sectionHasContent,
  SECTION_KINDS,
  SECTION_TITLES,
  sourceLine,
  type SectionId,
} from "../../lib/personRecord";
import type { Person, PersonDetail, PersonLabel, PersonRow } from "../../lib/types";
import { Button, Callout, Collapsible, Popover, Skeleton } from "../ui";
import { LabelPicker } from "./LabelPicker";
import { PersonProjects } from "./PersonProjects";

interface Props {
  /** The list's row for them: the header until their full record arrives. */
  row: PersonRow;
  /** Moves whenever something may have changed them; the record reads again. */
  version: number;
  /** Every label there is, for the picker. */
  labels: readonly PersonLabel[];
  /** One write; the tab says its failure and reads the list again. Resolves to whether it worked. */
  run: (write: () => Promise<unknown>) => Promise<boolean>;
  onEdit: (person: Person, section?: SectionId) => void;
  onStar: (starred: boolean) => void;
  onToggleLabel: (labelId: number, on: boolean) => void;
  onNewLabel: () => void;
  onOpenProject?: (project: string) => void;
  /** The Timeline section, under Projects, drawn from the full record. When it is given it carries
   *  the "Last met" line, so the header leaves it out rather than say it twice. */
  timeline?: (person: Person, today: Date) => ReactNode;
  /** "Log a meeting": open the entry dialog for this person. Without it the button is not drawn. */
  onLogMeeting?: () => void;
}

/** A person from `getPerson`, or null when what came back isn't one. */
function asPerson(value: unknown): Person | null {
  return value !== null && typeof value === "object" && Array.isArray((value as Person).details)
    ? (value as Person)
    : null;
}

/** The kind's word where a detail has no label of its own. */
const KIND_WORD: Partial<Record<PersonDetail["kind"], string>> = {
  email: "Email",
  phone: "Phone",
  address: "Address",
  web: "Link",
  date: "Date",
  relation: "Relation",
  custom: "Field",
};

const oneLine = (s: string) => s.replace(/\s+/g, " ").trim();

export function PersonRecord({
  row,
  version,
  labels,
  run,
  onEdit,
  onStar,
  onToggleLabel,
  onNewLabel,
  onOpenProject,
  timeline,
  onLogMeeting,
}: Props) {
  const [person, setPerson] = useState<Person | null>(null);
  const [failed, setFailed] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const sent = useRef(0);

  useEffect(() => {
    const n = ++sent.current;
    getPerson(row.id)
      .then((p) => {
        if (n !== sent.current) return;
        const found = asPerson(p);
        setPerson(found);
        setFailed(found === null);
      })
      .catch(() => {
        if (n === sent.current) setFailed(true);
      });
  }, [row.id, version, attempt]);

  // What the header shows comes from the full record once it is here, and from the row before.
  const name = person?.display_name ?? row.display_name;
  const ident = person?.uid ?? row.uid;
  const starred = person?.starred ?? row.starred;
  const trashed = (person?.trashed_at ?? row.trashed_at) !== null;
  const carried = person?.labels ?? row.labels;
  const today = new Date();

  // Taking a chip off unmounts the button that has focus, which would drop it to the page: once the
  // label is gone, focus moves to the chip that took its place (or the one before, or the picker).
  // Only when focus really did fall to the page, so a click elsewhere in the meantime is not undone.
  const labelsAt = useRef<HTMLDivElement>(null);
  const refocusChip = useRef<number | null>(null);
  const carriedKey = carried.map((l) => l.id).join(",");
  useEffect(() => {
    const at = refocusChip.current;
    if (at === null) return;
    refocusChip.current = null;
    const active = document.activeElement;
    if (active && active !== document.body) return;
    const xs = labelsAt.current?.querySelectorAll<HTMLElement>("button[data-chip-x]");
    const next = xs && xs.length > 0 ? xs[Math.min(at, xs.length - 1)] : null;
    (next ?? labelsAt.current?.querySelector<HTMLElement>("button:not([data-chip-x])"))?.focus();
  }, [carriedKey]);

  return (
    <section
      aria-label={row.display_name}
      data-help="people-record"
      className="flex flex-col gap-5 p-6"
    >
      <header className="flex flex-col gap-3">
        <div className="flex items-start gap-4">
          <span
            aria-hidden
            style={{ background: personCircleBackground(ident) }}
            className="grid h-14 w-14 shrink-0 place-items-center rounded-full text-lg font-semibold text-ink"
          >
            {personInitials(name)}
          </span>
          <div className="min-w-0 flex-1">
            <h2 className="break-words font-head text-lg font-semibold text-ink">{name}</h2>
            {person && <NameLines person={person} today={today} lastMet={!timeline} />}
          </div>
          <div className="flex shrink-0 items-center gap-1">
            {!trashed && (
              <Button
                variant="tertiary"
                size="xs"
                aria-pressed={starred}
                aria-label={starred ? "Unstar this person" : "Star this person"}
                onClick={() => onStar(!starred)}
              >
                {starred ? "★" : "☆"}
              </Button>
            )}
            {person && !trashed && onLogMeeting && (
              <Button variant="secondary" size="xs" onClick={onLogMeeting}>
                Log a meeting
              </Button>
            )}
            {person && !trashed && (
              <Button variant="secondary" size="xs" onClick={() => onEdit(person)}>
                Edit
              </Button>
            )}
          </div>
        </div>

        <div
          ref={labelsAt}
          data-help="people-labels"
          className="flex flex-wrap items-center gap-1.5"
        >
          {carried.length > 0 && (
            <ul aria-label="Labels" className="m-0 flex list-none flex-wrap gap-1.5 p-0">
              {carried.map((l, i) => (
                <li
                  key={l.id}
                  className="inline-flex items-center gap-1 rounded-full border border-ink4 px-2 py-0.5 text-xs text-ink3"
                >
                  {l.name}
                  {!trashed && (
                    <button
                      type="button"
                      data-chip-x
                      className="text-ink4 hover:text-ink"
                      aria-label={`Remove the label ${l.name}`}
                      onClick={() => {
                        refocusChip.current = i;
                        onToggleLabel(l.id, false);
                      }}
                    >
                      ×
                    </button>
                  )}
                </li>
              ))}
            </ul>
          )}
          {!trashed && (
            <LabelPicker
              labels={labels}
              targets={[{ labels: carried }]}
              buttonLabel="Add label"
              onToggle={onToggleLabel}
              onNew={onNewLabel}
            />
          )}
        </div>
      </header>

      {person?.trashed_at && (
        <Callout tone="info" size="sm" body="ink">
          In Trash since {formatDate(person.trashed_at)}. Restore them from the Trash filter to
          change anything.
        </Callout>
      )}

      {person ? (
        <>
          <Sections
            person={person}
            today={today}
            trashed={trashed}
            run={run}
            onEdit={onEdit}
            onOpenProject={onOpenProject}
          />
          {timeline?.(person, today)}
          <SourcesSection person={person} />
        </>
      ) : failed ? (
        <Callout tone="danger" size="sm">
          <span>PM couldn't read this person just now.</span>{" "}
          <Button variant="tertiary" size="xs" onClick={() => setAttempt((n) => n + 1)}>
            Try again
          </Button>
        </Callout>
      ) : (
        <div role="status" aria-label="Reading this person" className="flex flex-col gap-3">
          <Skeleton className="h-5" />
          <Skeleton className="h-5" />
          <Skeleton className="h-5" />
        </div>
      )}
    </section>
  );
}

/** Other names, the name in parts when it differs, and when you last met. */
function NameLines({
  person,
  today,
  lastMet,
}: {
  person: Person;
  today: Date;
  /** Say when you last met: not when the Timeline section does. */
  lastMet: boolean;
}) {
  const parts = [
    person.name_prefix,
    person.given_name,
    person.middle_name,
    person.family_name,
    person.name_suffix,
  ]
    .filter((s) => s.trim() !== "")
    .join(" ");
  const nicknames = detailsOfKinds(person, ["nickname"]).map((d) => d.value);
  const former = detailsOfKinds(person, ["former_name"]).map((d) => d.value);
  return (
    <div className="mt-0.5 flex flex-col gap-0.5 text-sm text-ink3">
      {parts !== "" && parts !== person.display_name && <p className="break-words">{parts}</p>}
      {nicknames.length > 0 && <p className="break-words">Also known as {nicknames.join(", ")}</p>}
      {former.length > 0 && <p className="break-words">Formerly {former.join(", ")}</p>}
      {lastMet && <p>{lastMetLine(person.last_met, today)}</p>}
    </div>
  );
}

// --- sections ------------------------------------------------------------------------------------------

/** One folding section; the fold is remembered on this device. */
function RecordSection({
  id,
  count,
  children,
}: {
  id: SectionId | "projects" | "sources";
  count?: number;
  children: ReactNode;
}) {
  const [open, setOpen] = useRememberedFold(`people.record.${id}`, true);
  const title = id === "projects" ? "Projects" : id === "sources" ? "Sources" : SECTION_TITLES[id];
  return (
    <section aria-label={title}>
      <Collapsible title={title} open={open} onOpenChange={setOpen} meta={count}>
        <div className="mt-2 pl-5">{children}</div>
      </Collapsible>
    </section>
  );
}

function Sections({
  person,
  today,
  trashed,
  run,
  onEdit,
  onOpenProject,
}: {
  person: Person;
  today: Date;
  trashed: boolean;
  run: Props["run"];
  onEdit: Props["onEdit"];
  onOpenProject?: (project: string) => void;
}) {
  const has = (id: SectionId) => sectionHasContent(person, id);
  const offers = emptySections(person);
  const birthday = describeBirthday(person.birthday, today);
  return (
    <>
      {has("contact") && (
        <RecordSection id="contact">
          <ul className="m-0 flex list-none flex-col gap-1.5 p-0">
            {detailsOfKinds(person, SECTION_KINDS.contact).map((d) => (
              <ContactLine key={d.id} d={d} />
            ))}
          </ul>
        </RecordSection>
      )}

      {has("work") && (
        <RecordSection id="work">
          <ul className="m-0 flex list-none flex-col gap-2 p-0">
            {detailsOfKinds(person, SECTION_KINDS.work).map((d) => {
              const role = [d.parts?.title ?? d.parts?.field, d.parts?.department]
                .filter((s): s is string => !!s)
                .join(", ");
              const span = rangeText(d);
              return (
                <li key={d.id} className="text-sm">
                  <p className="break-words font-medium text-ink">{d.value}</p>
                  {role !== "" && <p className="break-words text-ink3">{role}</p>}
                  {span !== "" && <p className="text-xs text-ink3">{span}</p>}
                </li>
              );
            })}
          </ul>
        </RecordSection>
      )}

      {has("dates") && (
        <RecordSection id="dates">
          <ul className="m-0 flex list-none flex-col gap-1.5 p-0">
            {birthday !== null && <Pair name="Birthday" value={birthday} />}
            {detailsOfKinds(person, SECTION_KINDS.dates).map((d) => (
              <Pair key={d.id} name={d.label || "Date"} value={formatPartial(d.value)} />
            ))}
          </ul>
        </RecordSection>
      )}

      {has("met") && (
        <RecordSection id="met">
          <div className="flex flex-col gap-0.5 text-sm">
            {person.met_where.trim() !== "" && (
              <p className="whitespace-pre-line break-words text-ink2">{person.met_where}</p>
            )}
            {person.met_on && <p className="text-ink3">{formatPartial(person.met_on)}</p>}
          </div>
        </RecordSection>
      )}

      {has("now") && (
        <RecordSection id="now">
          <ul className="m-0 flex list-none flex-col gap-1.5 p-0">
            {detailsOfKinds(person, SECTION_KINDS.now).map((d) => (
              <li key={d.id} className="break-words text-sm text-ink2">
                {d.value}
                {rangeText(d) !== "" && <span className="text-ink3"> ({rangeText(d)})</span>}
              </li>
            ))}
          </ul>
        </RecordSection>
      )}

      {has("relations") && (
        <RecordSection id="relations">
          <ul className="m-0 flex list-none flex-col gap-1.5 p-0">
            {detailsOfKinds(person, SECTION_KINDS.relations).map((d) => (
              <Pair key={d.id} name={d.label || "Relation"} value={d.value} />
            ))}
          </ul>
        </RecordSection>
      )}

      {has("custom") && (
        <RecordSection id="custom">
          <ul className="m-0 flex list-none flex-col gap-1.5 p-0">
            {detailsOfKinds(person, SECTION_KINDS.custom).map((d) => (
              <Pair key={d.id} name={d.label || "Field"} value={d.value} />
            ))}
          </ul>
        </RecordSection>
      )}

      {has("notes") && (
        <RecordSection id="notes">
          {/* Plain text, as typed: notes are not Markdown here, and a file's notes are untrusted. */}
          <p className="whitespace-pre-wrap break-words text-sm text-ink2">{person.notes}</p>
        </RecordSection>
      )}

      {/* Projects is always drawn: it is where a person gets linked, so it can't wait to be offered. */}
      <RecordSection id="projects" count={person.projects.length || undefined}>
        <PersonProjects
          personId={person.id}
          projects={person.projects}
          run={run}
          onOpenProject={onOpenProject}
          readOnly={trashed}
        />
      </RecordSection>

      {!trashed && offers.length > 0 && (
        <div>
          <Popover
            ariaLabel="Add details"
            trigger={({ open, toggle }) => (
              <Button
                variant="secondary"
                size="xs"
                aria-haspopup="true"
                aria-expanded={open}
                onClick={toggle}
              >
                Add details
              </Button>
            )}
            panelClassName="min-w-[12rem] p-2"
          >
            {({ close }) => (
              <div className="flex flex-col gap-1">
                {offers.map((id) => (
                  <Button
                    key={id}
                    variant="tertiary"
                    size="xs"
                    className="justify-start"
                    onClick={() => {
                      close();
                      onEdit(person, id);
                    }}
                  >
                    {id === "name" ? "Other names" : SECTION_TITLES[id]}
                  </Button>
                ))}
              </div>
            )}
          </Popover>
        </div>
      )}
    </>
  );
}

function SourcesSection({ person }: { person: Person }) {
  if (person.sources.length === 0) return null;
  return (
    <RecordSection id="sources">
      <ul className="m-0 flex list-none flex-col gap-1 p-0 text-sm text-ink3">
        {person.sources.map((s) => (
          <li key={s.id}>{sourceLine(s)}</li>
        ))}
      </ul>
    </RecordSection>
  );
}

/** A name and a value on one line. */
function Pair({ name, value }: { name: string; value: string }) {
  return (
    <li className="flex items-baseline gap-2 text-sm">
      <span className="w-24 shrink-0 break-words text-xs text-ink3">{name}</span>
      <span className="min-w-0 flex-1 break-words text-ink2">{value}</span>
    </li>
  );
}

/** One email, phone, address or web value. An address shows as written, line breaks and all. */
function ContactLine({ d }: { d: PersonDetail }) {
  const href =
    d.kind === "email" ? mailTarget(d.value) : d.kind === "web" ? linkTarget(d.value) : null;
  return (
    <li className="flex items-baseline gap-2 text-sm">
      <span className="w-24 shrink-0 break-words text-xs text-ink3">
        {d.label || KIND_WORD[d.kind]}
      </span>
      <span className="min-w-0 flex-1 break-words text-ink2">
        {href ? (
          <button
            type="button"
            className="break-all text-left text-accent-text underline underline-offset-2"
            onClick={() => openLink(href)}
          >
            {d.value}
          </button>
        ) : (
          <span className="whitespace-pre-line">{d.value}</span>
        )}
      </span>
      <Button
        variant="tertiary"
        size="xs"
        aria-label={`Copy ${oneLine(d.value)}`}
        onClick={() => copyViaHost(d.value, copiedWords(d.kind))}
      >
        Copy
      </Button>
    </li>
  );
}
