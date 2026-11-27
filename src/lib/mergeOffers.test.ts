// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { afterEach, describe, expect, it, vi } from "vitest";
import { forgetAllMergeOffers, mergesStillOnOffer, offerMerge, withdrawMerge } from "./mergeOffers";

afterEach(() => {
  forgetAllMergeOffers();
  vi.useRealTimers();
});

describe("merges on offer", () => {
  it("lists a merge with the time it has left, until it runs out", () => {
    vi.useFakeTimers();
    offerMerge("t1", "Merged 2 duplicates.", 30_000);
    vi.advanceTimersByTime(10_000);
    expect(mergesStillOnOffer()).toEqual([
      { token: "t1", words: "Merged 2 duplicates.", leftMs: 20_000 },
    ]);
    vi.advanceTimersByTime(20_000);
    expect(mergesStillOnOffer()).toEqual([]);
  });

  it("forgets one that was used or dismissed, and all of them on a lock", () => {
    offerMerge("a", "Merged into Ben Sample.", 30_000);
    offerMerge("b", "Merged into Cal Placeholder.", 30_000);
    withdrawMerge("a");
    expect(mergesStillOnOffer().map((m) => m.token)).toEqual(["b"]);
    forgetAllMergeOffers();
    expect(mergesStillOnOffer()).toEqual([]);
  });
});
