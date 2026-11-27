// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A person's timeline (spec 6.1), the parts that need no screen: the words for each kind, how an
// entry reads, and the drafts that a new entry and an edit send. No IPC and no clock of their own.

import { formatClock, formatDateOnly } from "./format";
import type { InteractionDraft, InteractionKind, TimelineEntry } from "./types";

/** What each kind is called, in the timeline and in the picker. */
export const KIND_WORDS: Record<InteractionKind, string> = {
  met: "Met",
  call: "Call",
  message: "Message",
  note: "Note",
};

/** The kind's word, or the stored text itself for a kind this build doesn't know (a newer store). */
export const kindWord = (kind: string): string => KIND_WORDS[kind as InteractionKind] ?? kind;

/** What each kind is called in a sentence ("Edit the call on 12-11-2025"): "Met" is not a noun. */
const KIND_NOUNS: Record<InteractionKind, string> = {
  met: "meeting",
  call: "call",
  message: "message",
  note: "note",
};

/** The kind as a noun, or the stored text itself for a kind this build doesn't know. */
export const kindNoun = (kind: string): string => KIND_NOUNS[kind as InteractionKind] ?? kind;

/** An entry's day, with its time when it has one: "12-11-2025 09:30". */
export function entryWhen(e: Pick<TimelineEntry, "on_date" | "at">): string {
  const day = formatDateOnly(e.on_date);
  if (!e.at) return day;
  const clock = formatClock(new Date(e.at));
  return clock ? `${day} ${clock}` : day;
}

/** "With Ada Lovelace and Bob Baker", or "" when it names nobody else. */
export function entryWith(e: Pick<TimelineEntry, "others">): string {
  const names = e.others.map((o) => o.display_name);
  if (names.length === 0) return "";
  if (names.length === 1) return `With ${names[0]}`;
  return `With ${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/** The entry a new "Log" sends: it names these people and no event. */
export function newEntryDraft(
  people: readonly number[],
  v: { kind: InteractionKind; on_date: string; note: string },
): InteractionDraft {
  return { kind: v.kind, on_date: v.on_date, at: null, note: v.note.trim(), people: [...people] };
}

/** The edit of an entry as seen from one person's record: everyone else on it stays on it. The time
 *  of day is kept while the day is unchanged and dropped when the day moves (it was a time on the
 *  old day); the event link is the backend's to keep. */
export function editEntryDraft(
  entry: TimelineEntry,
  self: number,
  v: { kind: InteractionKind; on_date: string; note: string },
): InteractionDraft {
  return {
    kind: v.kind,
    on_date: v.on_date,
    at: v.on_date === entry.on_date ? entry.at : null,
    note: v.note.trim(),
    people: [self, ...entry.others.map((o) => o.id).filter((id) => id !== self)],
  };
}
