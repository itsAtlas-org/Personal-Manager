// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// One meeting's card on "Who did you meet?" (spec 6.3): its title, date and time, the guests PM knows
// (ticked), the ones it doesn't under "Not in People yet" (unticked, "In Trash" for someone in Trash:
// ticking brings them back), a person picker behind "Someone else…", an optional note, and the four
// answers. It answers by itself through ipc, with no App state, so the same card works in the main
// window, in a dialog over the Calendar tab, and in the pop-out Calendar window, which mounts
// CalendarView without App.
//
// Everything about a guest or a title is another person's text: it renders as React text only.

import { useEffect, useId, useMemo, useRef, useState } from "react";
import { formatClock, formatEventWhen } from "../../lib/format";
import { answerMeeting, getPerson } from "../../lib/ipc";
import { foldGuests, guestKey, guestLine, personKey, refsFor } from "../../lib/meetings";
import { forgetDraft, keepDraft, restoreDraft } from "../../lib/meetingDrafts";
import { errorWords } from "../../lib/people";
import type { MeetingAnswer, OwedMeeting, PersonRow } from "../../lib/types";
import { Button, Callout, Textarea } from "../ui";
import { PersonPicker } from "./PersonPicker";

interface Props {
  meeting: OwedMeeting;
  /** The meeting was answered (saved, skipped or its series skipped): the dialog drops the card. */
  onAnswered: (meeting: OwedMeeting) => void;
  /** Open a known guest's record. Without it the names are plain text. */
  onOpenPerson?: (uid: string) => void;
  /** A "Skip all" in progress: the card's own buttons wait. */
  locked?: boolean;
  /** Called as an answer starts and ends, so the dialog can keep its footer still meanwhile. */
  onBusy?: (busy: boolean) => void;
}

/** "26-11 09:00 – 09:30" for a timed meeting, the bare date for an all-day one. */
function whenLine(m: OwedMeeting): string {
  const start = formatEventWhen(m.start, m.all_day);
  if (m.all_day || !m.end || !m.start.includes("T")) return start;
  const end = new Date(m.end);
  const clock = formatClock(end);
  return clock ? `${start} – ${clock}` : start;
}

export function MeetCard({ meeting, onAnswered, onOpenPerson, locked, onBusy }: Props) {
  const base = useId();
  // What was ticked, picked and written before the dialog last closed (clicking a guest's name
  // opens their record and closes it), kept in memory by `meetingDrafts`; else a fresh card.
  const [kept] = useState(() => restoreDraft(meeting));
  const [ticks, setTicks] = useState<Set<string>>(kept.ticks);
  // People picked with "Someone else…": ticked, and listed under the guests.
  const [extra, setExtra] = useState<PersonRow[]>(kept.extra);
  const [picking, setPicking] = useState(false);
  const [note, setNote] = useState(kept.note);
  const [expanded, setExpanded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  // The picker replaces its own button, so focus is handed back to the button when it closes.
  const elseButton = useRef<HTMLButtonElement>(null);
  const refocus = useRef(false);
  useEffect(() => {
    if (!picking && refocus.current) {
      refocus.current = false;
      elseButton.current?.focus();
    }
  }, [picking]);

  // Unsaved input is kept as it changes, so it survives the dialog closing; an answered card has
  // had its entry dropped by the dialog and must not write it back.
  const answered = useRef(false);
  useEffect(() => {
    if (!answered.current) keepDraft(meeting, { ticks, extra, note });
  }, [meeting, ticks, extra, note]);

  const { shown, hidden } = foldGuests(meeting.unknown, expanded);
  const refs = useMemo(() => refsFor(meeting, ticks, extra), [meeting, ticks, extra]);
  // Everyone already on the card, so the picker doesn't offer them twice.
  const onCard = useMemo(
    () => new Set<number>([...meeting.known.map((g) => g.id), ...extra.map((p) => p.id)]),
    [meeting.known, extra],
  );
  const title = meeting.title.trim() || "(no title)";
  const answerable = meeting.uid !== "";
  const off = busy || !!locked;

  function toggle(key: string) {
    setTicks((prev) => {
      const next = new Set(prev);
      if (!next.delete(key)) next.add(key);
      return next;
    });
  }

  async function answer(how: MeetingAnswer) {
    if (off) return;
    setBusy(true);
    onBusy?.(true);
    setProblem(null);
    try {
      await answerMeeting(
        meeting.uid,
        meeting.start,
        how === "logged" ? refs : [],
        how,
        how === "logged" ? note.trim() || null : null,
        meeting.title,
      );
      answered.current = true;
      forgetDraft(meeting);
      onAnswered(meeting);
    } catch (err) {
      setProblem(errorWords(err));
    } finally {
      setBusy(false);
      onBusy?.(false);
    }
  }

  async function open(id: number) {
    try {
      const person = await getPerson(id);
      if (person?.uid) onOpenPerson?.(person.uid);
    } catch {
      // A link that can't be followed just doesn't go; the card still works.
    }
  }

  const name = (id: number, display: string) =>
    onOpenPerson ? (
      <button
        type="button"
        className="min-w-0 break-words text-left text-accent-text underline underline-offset-2"
        aria-label={`Open ${display}`}
        // Not while an answer is being saved: opening a record closes the dialog under it.
        disabled={off}
        onClick={() => void open(id)}
      >
        {display}
      </button>
    ) : (
      <span className="min-w-0 break-words">{display}</span>
    );

  return (
    <section
      aria-label={`Meeting ${title}`}
      className="flex flex-col gap-3 rounded-[var(--radius-sm)] border border-border p-3"
    >
      <header>
        <h3 className="break-words text-sm font-semibold text-ink">{title}</h3>
        <p className="text-xs text-ink3">{whenLine(meeting)}</p>
      </header>

      {meeting.known.length > 0 && (
        <ul aria-label="Guests in People" className="m-0 flex list-none flex-col gap-1 p-0">
          {meeting.known.map((g) => {
            const id = `${base}-k${g.id}`;
            return (
              <li key={g.id} className="flex items-center gap-2 text-sm text-ink2">
                <input
                  id={id}
                  type="checkbox"
                  className="accent-[var(--accent)]"
                  checked={ticks.has(personKey(g.id))}
                  disabled={off}
                  onChange={() => toggle(personKey(g.id))}
                  aria-label={`${g.display_name} was there`}
                />
                {name(g.id, g.display_name)}
                <span className="min-w-0 truncate text-xs text-ink3">{g.email}</span>
              </li>
            );
          })}
        </ul>
      )}

      {extra.length > 0 && (
        <ul aria-label="Other people" className="m-0 flex list-none flex-col gap-1 p-0">
          {extra.map((p) => (
            <li key={p.id} className="flex items-center gap-2 text-sm text-ink2">
              <span className="min-w-0 break-words">{p.display_name}</span>
              <Button
                variant="tertiary"
                size="xs"
                disabled={off}
                aria-label={`Remove ${p.display_name}`}
                onClick={() => setExtra((prev) => prev.filter((x) => x.id !== p.id))}
              >
                Remove
              </Button>
            </li>
          ))}
        </ul>
      )}

      {meeting.unknown.length > 0 && (
        <div className="flex flex-col gap-1">
          <p className="text-xs font-medium text-ink3">Not in People yet</p>
          <ul aria-label="Not in People yet" className="m-0 flex list-none flex-col gap-1 p-0">
            {shown.map((g) => {
              const id = `${base}-u${g.email}`;
              return (
                <li key={g.email} className="flex items-center gap-2 text-sm text-ink2">
                  <input
                    id={id}
                    type="checkbox"
                    className="accent-[var(--accent)]"
                    checked={ticks.has(guestKey(g))}
                    disabled={off}
                    onChange={() => toggle(guestKey(g))}
                    aria-label={`${guestLine(g)} was there`}
                  />
                  <span className="min-w-0 break-words">{guestLine(g)}</span>
                  {g.trashed_id !== null && <span className="text-xs text-ink3">In Trash</span>}
                </li>
              );
            })}
          </ul>
          {hidden > 0 && (
            <div>
              <Button variant="tertiary" size="xs" disabled={off} onClick={() => setExpanded(true)}>
                {`Show ${hidden} more guests`}
              </Button>
            </div>
          )}
        </div>
      )}

      <div className="flex flex-col gap-1.5">
        {picking ? (
          <PersonPicker
            exclude={onCard}
            label={`Someone else who was at ${title}`}
            onPick={(p) => {
              setExtra((prev) => [...prev, p]);
              refocus.current = true;
              setPicking(false);
            }}
          />
        ) : (
          <div>
            <Button
              ref={elseButton}
              variant="secondary"
              size="xs"
              disabled={off}
              onClick={() => setPicking(true)}
            >
              Someone else…
            </Button>
          </div>
        )}
      </div>

      <div className="flex flex-col gap-1">
        <label htmlFor={`${base}-note`} className="text-xs text-ink3">
          Note (optional)
        </label>
        <Textarea
          id={`${base}-note`}
          rows={2}
          value={note}
          disabled={off}
          placeholder="What you talked about, or anything to remember"
          onChange={(e) => setNote(e.target.value)}
        />
      </div>

      {problem && (
        <Callout tone="danger" size="sm">
          {problem}
        </Callout>
      )}

      <div className="flex flex-wrap items-center gap-2">
        <Button
          variant="primary"
          size="sm"
          disabled={off || refs.length === 0}
          onClick={() => void answer("logged")}
        >
          Save
        </Button>
        {answerable && (
          <Button
            variant="secondary"
            size="sm"
            disabled={off}
            onClick={() => void answer("skipped")}
          >
            Skip
          </Button>
        )}
        {answerable && meeting.recurs && (
          <Button
            variant="tertiary"
            size="sm"
            disabled={off}
            onClick={() => void answer("series_skipped")}
          >
            Don't ask about this series
          </Button>
        )}
        {refs.length === 0 && (
          <span className="text-xs text-ink3">Tick at least one person to save.</span>
        )}
      </div>
    </section>
  );
}
