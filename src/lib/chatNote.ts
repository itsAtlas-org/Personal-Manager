// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The note id Chat's "Save as note" files a saved answer under.
//
// Saved answers reuse the pinboard's note-ingest path, which names the vault file after the id and
// refuses any id outside `[A-Za-z0-9_-]` (`ingest_note_document` in ingest.rs). The `chat-` prefix
// keeps saved answers apart from pinboard notes, whose ids are UUIDs or `w-…`, and the message id
// makes a second save of the same answer update it in place instead of duplicating it. Message ids
// are AUTOINCREMENT, so a deleted chat's id is never handed to a new answer.

import { formatDateLocal } from "./format";
import { ingestNote } from "./ipc";

/** The note id for a saved chat answer. */
export function chatNoteId(messageId: number): string {
  return `chat-${messageId}`;
}

/** Keep an assistant answer as a searchable vault note: the one path the "Save as note" button under
 *  the answer and the answer's right-click menu both take. Keyed on the message id, so saving twice
 *  re-embeds in place and never duplicates; the note is standalone, so deleting the chat never
 *  removes it. A soft "saved from a chat" date marks where it came from. Rejects with the write's
 *  error, which the caller shows as its own state. */
export async function saveAnswerAsNote(message: { id: number; content: string }): Promise<void> {
  const stamp = `_Saved from a chat, ${formatDateLocal(new Date())}_`;
  await ingestNote(chatNoteId(message.id), "", `${message.content}\n\n${stamp}`);
}
