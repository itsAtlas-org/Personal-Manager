// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the People tab shows, read from the backend. The tab router unmounts the view on every tab
// switch, so nothing here is the truth: the list and the labels are re-read on mount, whenever the
// filter, the search or the sort change, and on `people://changed` (any write, from any surface,
// debounced so a bulk change is one read). A reply that lands after a newer request was sent is
// dropped, so a slow answer for an old filter never replaces a fast one for the new.

import { useCallback, useEffect, useRef, useState } from "react";
import { listLabels, listPeople, onPeopleChanged, searchPeople } from "../../lib/ipc";
import { subscribeUntilCleanup } from "../../lib/subscribe";
import type { PeopleFilter, PeopleSort, PersonLabel, PersonRow } from "../../lib/types";

/** How long after a change notice before the list is read again. */
const REFRESH_DELAY_MS = 150;

/** The most people one search returns (the backend's own ceiling). */
export const SEARCH_LIMIT = 200;

/** What a filter that has not answered yet shows: nothing, never the last filter's people. */
const NO_ROWS: PersonRow[] = [];

export interface PeopleData {
  /** Always the current filter's people (a search within it may still be in flight): the rows of
   *  another filter are never returned, so they can't be drawn under the new one's heading. */
  rows: PersonRow[];
  labels: PersonLabel[];
  /** There are no rows to show yet: the first read, or the first read of a newly chosen filter. */
  loading: boolean;
  /** The last read failed; the rows shown (if any) are from before it. */
  failed: boolean;
  /** The rows are the answer to the current filter, search and sort (not a read still in flight). */
  settled: boolean;
  /** Stable: always runs the newest read, so a closure kept for later (an Undo) never reads an
   *  old query or sort. */
  reload: () => void;
}

/** A search (`query` non-blank) answers from `searchPeople` and ignores the filter and the sort, as
 *  Ctrl K does; no search is the filter's own list in the chosen sort. */
export function usePeopleData(filter: PeopleFilter, query: string, sort: PeopleSort): PeopleData {
  const [rows, setRows] = useState<PersonRow[]>([]);
  const [labels, setLabels] = useState<PersonLabel[]>([]);
  const [loading, setLoading] = useState(true);
  const [failed, setFailed] = useState(false);
  const [loadedKey, setLoadedKey] = useState<string | null>(null);
  const [loadedFilterKey, setLoadedFilterKey] = useState<string | null>(null);
  const sent = useRef(0);
  // An object filter ({ Label: id }) is a new object on every render of a caller that builds it
  // inline, so the read depends on its key and takes the value through a ref.
  const filterKey = JSON.stringify(filter);
  const filterRef = useRef(filter);
  filterRef.current = filter;
  const key = `${filterKey}|${query}|${sort}`;

  const load = useCallback(() => {
    const n = ++sent.current;
    const q = query.trim();
    // Taken now: a later filter change must not label this answer with the filter it wasn't read for.
    const readFor = filterKey;
    // A failure shown for the last read must not outlive the next attempt (nor hide its skeleton).
    setFailed(false);
    void Promise.all([
      q ? searchPeople(q, SEARCH_LIMIT) : listPeople(filterRef.current, null, sort),
      listLabels(),
    ])
      .then(([r, l]) => {
        if (n !== sent.current) return;
        setRows(r);
        setLabels(l);
        setFailed(false);
        setLoading(false);
        setLoadedKey(key);
        setLoadedFilterKey(readFor);
      })
      .catch(() => {
        if (n !== sent.current) return;
        setFailed(true);
        setLoading(false);
      });
  }, [query, sort, key, filterKey]);

  useEffect(() => {
    load();
  }, [load]);

  const timer = useRef<number | undefined>(undefined);
  const loadRef = useRef(load);
  loadRef.current = load;
  useEffect(() => {
    const stop = subscribeUntilCleanup(() =>
      onPeopleChanged(() => {
        window.clearTimeout(timer.current);
        timer.current = window.setTimeout(() => loadRef.current(), REFRESH_DELAY_MS);
      }),
    );
    return () => {
      stop();
      window.clearTimeout(timer.current);
    };
  }, []);

  const reload = useCallback(() => loadRef.current(), []);

  // A failed read of a new filter shows its error with no rows; a read still out shows the skeleton.
  const switching = loadedFilterKey !== filterKey;
  return {
    rows: switching ? NO_ROWS : rows,
    labels,
    loading: loading || (switching && !failed),
    failed,
    settled: loadedKey === key,
    reload,
  };
}
