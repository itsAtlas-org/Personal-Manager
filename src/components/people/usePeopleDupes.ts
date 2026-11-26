// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The possible-duplicates snapshot the People tab keeps (spec 4.1). The finder runs in the backend
// off the DB guard; the tab only reads its latest answer: on mount (the tab router unmounts the view
// on every tab switch) and again on `people://changed`, debounced so an import or a merge is one
// read. The header's "Possible duplicates (N)" needs the count whether or not the panel is open ("a
// background result nobody can see is not a result"), so the view owns this and hands it to the
// panel. A reply that lands after a newer request was sent is dropped.

import { useCallback, useEffect, useRef, useState } from "react";
import { onPeopleChanged, peopleDuplicatesSnapshot } from "../../lib/ipc";
import { subscribeUntilCleanup } from "../../lib/subscribe";
import type { DupesReport } from "../../lib/types";

/** How long after a change notice the snapshot is read again. */
const REFRESH_DELAY_MS = 250;

export interface PeopleDupes {
  /** The latest snapshot, or null before the first read (or if reading failed). */
  report: DupesReport | null;
  /** Read it again now. Stable. */
  reload: () => void;
}

/** A snapshot is a report with a list of pairs; anything else (a failed or odd reply) is none. */
function usable(r: unknown): r is DupesReport {
  return typeof r === "object" && r !== null && Array.isArray((r as DupesReport).pairs);
}

export function usePeopleDupes(): PeopleDupes {
  const [report, setReport] = useState<DupesReport | null>(null);
  const sent = useRef(0);

  const reload = useCallback(() => {
    const n = ++sent.current;
    void Promise.resolve(peopleDuplicatesSnapshot())
      .then((r) => {
        if (n === sent.current) setReport(usable(r) ? r : null);
      })
      .catch(() => {
        // A failed read keeps what was shown: the count is a hint, never a verdict.
      });
  }, []);

  useEffect(() => {
    reload();
  }, [reload]);

  const timer = useRef<number | undefined>(undefined);
  useEffect(() => {
    const stop = subscribeUntilCleanup(() =>
      onPeopleChanged(() => {
        window.clearTimeout(timer.current);
        timer.current = window.setTimeout(reload, REFRESH_DELAY_MS);
      }),
    );
    return () => {
      stop();
      window.clearTimeout(timer.current);
    };
  }, [reload]);

  return { report, reload };
}
