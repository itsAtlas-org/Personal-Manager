// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "You had 3 meetings with guests recently. Who did you meet?" as an app-wide strip (spec 6.3), the
// PowerConsentStrip's pattern: non-modal and never focused, so a stray Enter or Escape can't answer
// it, and with no ✕ because every way out is an answer. Review opens the meeting dialog; Not now
// hides it until tomorrow (the user's civil day) or until a meeting is owed that wasn't when it was
// said, and what was hidden is remembered on this device only. Not drawn inside Settings: App leaves
// it out there, since it can wait.

import { useState } from "react";
import {
  civilToday,
  dismissFor,
  readDismissal,
  stripVisible,
  stripWords,
  writeDismissal,
  type Dismissal,
} from "../../lib/meetings";
import type { OwedMeeting } from "../../lib/types";
import { Button, Callout } from "../ui";

interface Props {
  owed: readonly OwedMeeting[];
  /** The user's IANA zone, so "tomorrow" is theirs; the device's own day when it isn't known. */
  zone?: string | null;
  onReview: () => void;
}

export function MeetStrip({ owed, zone, onReview }: Props) {
  const [dismissal, setDismissal] = useState<Dismissal | null>(readDismissal);
  const today = civilToday(zone, new Date());
  if (!stripVisible(owed, dismissal, today)) return null;
  return (
    <Callout
      tone="info"
      variant="strip"
      size="md"
      body="ink"
      live
      data-help="people-meet"
      className="flex flex-wrap items-center justify-between gap-3"
    >
      <p className="min-w-0 flex-1 break-words">{stripWords(owed)}</p>
      <div className="flex flex-wrap items-center gap-2">
        <Button variant="primary" size="sm" onClick={onReview}>
          Review
        </Button>
        <Button
          variant="tertiary"
          size="sm"
          onClick={() => {
            const next = dismissFor(owed, today);
            writeDismissal(next);
            setDismissal(next);
          }}
        >
          Not now
        </Button>
      </div>
    </Callout>
  );
}
