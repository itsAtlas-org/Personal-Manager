// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The record's Timeline section (spec 6.1): "Last met 12-11-2026 (2 weeks ago)" or "Not met yet" at
// the top, "+ Log", and the entries newest first. The first page comes with the person; "Show
// earlier entries" reads the next. Edit and Delete are the owner's (it holds the dialogs and does
// the writes), so this draws and asks. A person in Trash is read-only, as their other sections are.
//
// A note and an event's title are other people's text (a file, a sync or a guest wrote them): React
// text only, line breaks kept.

import { useEffect, useRef, useState } from "react";
import { useRememberedFold } from "../../lib/foldPrefs";
import { listTimeline } from "../../lib/ipc";
import { errorWords } from "../../lib/people";
import { lastMetLine } from "../../lib/personRecord";
import { entryWhen, entryWith, kindNoun, kindWord } from "../../lib/timeline";
import type { Person, TimelineEntry, TimelinePage } from "../../lib/types";
import { Button, Collapsible } from "../ui";

interface Props {
  person: Person;
  trashed: boolean;
  today: Date;
  onLog: () => void;
  onEdit: (entry: TimelineEntry) => void;
  onDelete: (entry: TimelineEntry) => void;
}

export function PersonTimeline({ person, trashed, today, onLog, onEdit, onDelete }: Props) {
  const [open, setOpen] = useRememberedFold("people.record.timeline", true);
  const first: TimelinePage = person.timeline;
  // Later pages, read on request; the record's own page is always the newest, so they follow it.
  const [more, setMore] = useState<TimelineEntry[]>([]);
  const [next, setNext] = useState(first.next);
  const [reading, setReading] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);
  // Bumped whenever the pages read so far are thrown away: a reply sent before that is stale.
  const generation = useRef(0);

  // The record is read again after every write on it (a star, a label), and none of those touch the
  // timeline: the pages read so far stay. Start over from the first page, which is the truth, only
  // when it really changed (another person, other entries, another next page), or when this view
  // sent a log, an edit or a delete (that may sit on an earlier page the first page doesn't show).
  const seen = useRef({ id: person.id, page: JSON.stringify(first) });
  const touched = useRef(false);
  useEffect(() => {
    const page = JSON.stringify(first);
    const same = seen.current.id === person.id && seen.current.page === page && !touched.current;
    seen.current = { id: person.id, page };
    touched.current = false;
    if (same) return;
    generation.current++;
    setMore([]);
    setNext(first.next);
    setFailed(null);
  }, [first, person.id]);

  async function showMore() {
    if (!next || reading) return;
    setReading(true);
    setFailed(null);
    const sentFor = generation.current;
    try {
      const page = await listTimeline(person.id, next);
      // The timeline was started over while this was out: the new first page is the truth.
      if (generation.current !== sentFor) return;
      setMore((prev) => [...prev, ...page.entries]);
      setNext(page.next);
    } catch (err) {
      if (generation.current === sentFor) {
        setFailed(errorWords(err, "PM couldn't read earlier entries just now."));
      }
    } finally {
      setReading(false);
    }
  }

  const entries = [...first.entries, ...more];
  const lastMet = <p className="text-sm text-ink2">{lastMetLine(person.last_met, today)}</p>;
  return (
    <section aria-label="Timeline" data-help="people-timeline">
      <Collapsible
        title="Timeline"
        open={open}
        onOpenChange={setOpen}
        meta={entries.length || undefined}
      >
        <div className="mt-2 flex flex-col gap-3 pl-5">
          <div className="flex flex-wrap items-center justify-between gap-2">
            {open && lastMet}
            {!trashed && (
              <Button
                variant="secondary"
                size="xs"
                onClick={() => {
                  touched.current = true;
                  onLog();
                }}
              >
                + Log
              </Button>
            )}
          </div>

          {entries.length === 0 ? (
            <p className="text-sm text-ink3">
              Nothing logged yet.{!trashed && " Log a meeting, a call or a message to remember it."}
            </p>
          ) : (
            <ul aria-label="Timeline entries" className="m-0 flex list-none flex-col gap-3 p-0">
              {entries.map((e) => {
                const when = entryWhen(e);
                const who = entryWith(e);
                return (
                  <li key={e.id} className="flex flex-col gap-0.5 text-sm">
                    <div className="flex flex-wrap items-baseline gap-x-2">
                      <span className="font-medium text-ink">{kindWord(e.kind)}</span>
                      <span className="text-xs text-ink3">{when}</span>
                    </div>
                    {e.event_title && (
                      <p className="break-words text-xs text-ink3">From “{e.event_title}”</p>
                    )}
                    {who !== "" && <p className="break-words text-xs text-ink3">{who}</p>}
                    {e.note.trim() !== "" && (
                      <p className="whitespace-pre-wrap break-words text-ink2">{e.note}</p>
                    )}
                    {!trashed && (
                      <div className="flex items-center gap-1">
                        <Button
                          variant="tertiary"
                          size="xs"
                          aria-label={`Edit the ${kindNoun(e.kind)} on ${when}`}
                          onClick={() => {
                            touched.current = true;
                            onEdit(e);
                          }}
                        >
                          Edit
                        </Button>
                        <Button
                          variant="tertiary"
                          size="xs"
                          aria-label={`Delete the ${kindNoun(e.kind)} on ${when}`}
                          onClick={() => {
                            touched.current = true;
                            onDelete(e);
                          }}
                        >
                          Delete
                        </Button>
                      </div>
                    )}
                  </li>
                );
              })}
            </ul>
          )}

          {failed && (
            <p role="alert" className="text-xs text-st-due">
              {failed}
            </p>
          )}
          {next && (
            <div>
              <Button
                variant="tertiary"
                size="xs"
                disabled={reading}
                onClick={() => void showMore()}
              >
                Show earlier entries
              </Button>
            </div>
          )}
        </div>
      </Collapsible>
      {/* Folded, the section still says when you last met (spec 6.1 puts it at the top). */}
      {!open && <div className="mt-1 pl-5">{lastMet}</div>}
    </section>
  );
}
