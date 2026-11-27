// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A person's timeline without a screen: the words, how an entry reads, and the drafts a new entry
// and an edit send. Clocks go through formatClock, as CI is 12-hour en-US and the dev box en_GB.

import { describe, expect, it } from "vitest";
import { formatClock } from "./format";
import {
  editEntryDraft,
  entryWhen,
  entryWith,
  kindNoun,
  kindWord,
  newEntryDraft,
} from "./timeline";
import type { TimelineEntry } from "./types";

const entry = (over: Partial<TimelineEntry> = {}): TimelineEntry => ({
  id: 4,
  kind: "met",
  on_date: "2025-11-12",
  at: null,
  note: "Lunch",
  event_uid: null,
  event_start: null,
  event_title: null,
  others: [],
  created_at: "2025-11-12T12:00:00Z",
  ...over,
});

describe("how an entry reads", () => {
  it("names each kind, and shows an unknown one as stored", () => {
    expect(["met", "call", "message", "note"].map(kindWord)).toEqual([
      "Met",
      "Call",
      "Message",
      "Note",
    ]);
    expect(kindWord("visit")).toBe("visit");
    expect(["met", "call", "message", "note"].map(kindNoun)).toEqual([
      "meeting",
      "call",
      "message",
      "note",
    ]);
    expect(kindNoun("visit")).toBe("visit");
  });

  it("gives the day, and the time when there is one", () => {
    expect(entryWhen(entry())).toBe("12-11-2025");
    const at = "2025-11-12T09:30:00Z";
    expect(entryWhen(entry({ at }))).toBe(`12-11-2025 ${formatClock(new Date(at))}`);
  });

  it("says who else was on it", () => {
    expect(entryWith(entry())).toBe("");
    const ben = { id: 2, display_name: "Ben Sample" };
    const cal = { id: 3, display_name: "Cal Placeholder" };
    const dee = { id: 4, display_name: "Dee Invented" };
    expect(entryWith(entry({ others: [ben] }))).toBe("With Ben Sample");
    expect(entryWith(entry({ others: [ben, cal] }))).toBe("With Ben Sample and Cal Placeholder");
    expect(entryWith(entry({ others: [ben, cal, dee] }))).toBe(
      "With Ben Sample, Cal Placeholder and Dee Invented",
    );
  });
});

describe("the drafts", () => {
  const v = { kind: "call" as const, on_date: "2026-11-20", note: "  Catch-up \n" };

  it("sends a new entry naming the person, with no time and no event", () => {
    expect(newEntryDraft([7], v)).toEqual({
      kind: "call",
      on_date: "2026-11-20",
      at: null,
      note: "Catch-up",
      people: [7],
    });
  });

  it("keeps everyone else on an edited entry, and leaves the viewer first", () => {
    const e = entry({
      at: "2026-11-20T09:30:00Z",
      on_date: "2026-11-20",
      others: [
        { id: 2, display_name: "Ben Sample" },
        { id: 7, display_name: "Ada Example" },
      ],
    });
    expect(editEntryDraft(e, 7, v).people).toEqual([7, 2]);
  });

  it("keeps the time while the day stands and drops it when the day moves", () => {
    const e = entry({ at: "2026-11-20T09:30:00Z", on_date: "2026-11-20" });
    expect(editEntryDraft(e, 7, v).at).toBe("2026-11-20T09:30:00Z");
    expect(editEntryDraft(e, 7, { ...v, on_date: "2026-11-21" }).at).toBeNull();
  });
});
