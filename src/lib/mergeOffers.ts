// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The merges whose Undo is still on offer, kept in memory across tab switches. The tab router
// unmounts the People tab on every switch, and a merge is the one write there that Trash can't take
// back (spec 4.3 promises 30 seconds), so the tab puts the offer up again, for the time it has left,
// when it comes back. An entry is a words-and-token pair: the before-image itself stays in the
// backend, which refuses a token once a lock, a store switch or the clock has ended it. Dropped on
// lock with the rest of what the tab keeps (`forgetAllMergeOffers`, called where the drafts go).

interface Offer {
  /** The notice's words, "Merged 2 duplicates.". */
  words: string;
  /** `Date.now()` when the offer ends. */
  until: number;
}

const offers = new Map<string, Offer>();

/** A merge went through and is on offer for `ttlMs`. */
export function offerMerge(token: string, words: string, ttlMs: number): void {
  offers.set(token, { words, until: Date.now() + ttlMs });
}

/** The merge was undone or its notice dismissed: it is not offered again. */
export function withdrawMerge(token: string): void {
  offers.delete(token);
}

/** The offers with time left, and how much; the ones that ran out are dropped. */
export function mergesStillOnOffer(): { token: string; words: string; leftMs: number }[] {
  const now = Date.now();
  const live: { token: string; words: string; leftMs: number }[] = [];
  for (const [token, o] of [...offers]) {
    if (o.until <= now) offers.delete(token);
    else live.push({ token, words: o.words, leftMs: o.until - now });
  }
  return live;
}

/** The store closed behind a lock: nothing is left to take back. */
export function forgetAllMergeOffers(): void {
  offers.clear();
}
