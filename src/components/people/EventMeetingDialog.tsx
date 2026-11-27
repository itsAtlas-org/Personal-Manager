// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Log who you met" on one calendar event (spec 6.3): the same card the strip's Review shows, for
// the one occurrence, whether or not PM would have asked about it (an answered or skipped meeting,
// one older than 14 days, one with no guests at all). An occurrence that is on the owed list uses
// that card as it is; any other is built from the event's own guests, sorted by the backend the way
// the owed list sorts them. Asking again after a skip replaces the skip (the answer is an upsert).
//
// It reads and writes through ipc alone, so it opens in the main window and in the pop-out Calendar.

import { useEffect, useState } from "react";
import { eventGuests, owedMeetings } from "../../lib/ipc";
import { cardForEvent, isOwedEvent } from "../../lib/meetings";
import { errorWords } from "../../lib/people";
import type { CalendarEvent, OwedMeeting } from "../../lib/types";
import { Button, Callout, Dialog, Skeleton } from "../ui";
import { MeetingDialog } from "./MeetingDialog";

interface Props {
  event: CalendarEvent;
  onClose: () => void;
  onOpenPerson?: (uid: string) => void;
}

export function EventMeetingDialog({ event, onClose, onOpenPerson }: Props) {
  const [card, setCard] = useState<OwedMeeting | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let live = true;
    setProblem(null);
    void (async () => {
      try {
        // The owed list is a convenience for the guest sorting; if it can't be read the event's own
        // guests still make the card.
        const owed = await owedMeetings().catch(() => [] as OwedMeeting[]);
        const hit = owed.find((m) => isOwedEvent(m, event));
        const built = hit ?? cardForEvent(event, await eventGuests(event.attendees ?? []));
        if (live) setCard(built);
      } catch (err) {
        if (live) setProblem(errorWords(err));
      }
    })();
    return () => {
      live = false;
    };
  }, [event, attempt]);

  if (card)
    return (
      <MeetingDialog
        title="Log who you met"
        meetings={[card]}
        onClose={onClose}
        onOpenPerson={onOpenPerson}
      />
    );
  return (
    <Dialog
      open
      onClose={onClose}
      title="Log who you met"
      helpId="people-meet"
      footer={
        <Button variant="secondary" onClick={onClose}>
          Close
        </Button>
      }
    >
      {problem ? (
        <Callout tone="danger" size="sm" className="mt-3">
          <span>{problem}</span>{" "}
          <Button variant="tertiary" size="xs" onClick={() => setAttempt((n) => n + 1)}>
            Try again
          </Button>
        </Callout>
      ) : (
        <div role="status" aria-label="Reading the guests" className="mt-4 flex flex-col gap-3">
          <Skeleton className="h-5" />
          <Skeleton className="h-5" />
        </div>
      )}
    </Dialog>
  );
}
