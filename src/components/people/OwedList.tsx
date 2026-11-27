// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The meetings waiting for an answer, at the top of the People tab's All view (spec 6.3): the strip
// that follows you around can be put off with Not now, but this list is always here to come back to.
// Each row opens the meeting dialog on that one meeting; "Review all" opens every one. Titles are
// calendar text, drawn as React text.

import { formatEventWhen } from "../../lib/format";
import type { OwedMeeting } from "../../lib/types";
import { Button } from "../ui";

interface Props {
  meetings: readonly OwedMeeting[];
  onReview: (meetings: OwedMeeting[]) => void;
}

export function OwedList({ meetings, onReview }: Props) {
  if (meetings.length === 0) return null;
  return (
    <section
      aria-label="Meetings waiting for an answer"
      data-help="people-meet"
      className="flex flex-col gap-1.5 border-b border-border px-4 py-3"
    >
      <div className="flex items-center justify-between gap-2">
        <h3 className="text-xs font-semibold text-ink2">
          {meetings.length === 1
            ? "A meeting is waiting: who did you meet?"
            : `${meetings.length} meetings are waiting: who did you meet?`}
        </h3>
        {meetings.length > 1 && (
          <Button variant="secondary" size="xs" onClick={() => onReview([...meetings])}>
            Review all
          </Button>
        )}
      </div>
      <ul className="m-0 flex list-none flex-col gap-1 p-0">
        {meetings.map((m) => {
          const title = m.title.trim() || "(no title)";
          return (
            <li key={`${m.uid}\n${m.start}`} className="flex items-center justify-between gap-2">
              <span className="min-w-0 text-sm text-ink2">
                <span className="break-words">{title}</span>{" "}
                <span className="text-xs text-ink3">{formatEventWhen(m.start, m.all_day)}</span>
              </span>
              <Button
                variant="tertiary"
                size="xs"
                aria-label={`Review ${title}`}
                onClick={() => onReview([m])}
              >
                Review
              </Button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
