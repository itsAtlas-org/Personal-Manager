// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useRef, useState } from "react";
import { renameConversation } from "../lib/ipc";

interface Props {
  conversationId: number;
  title: string;
  /** Called with the saved (trimmed/clamped) title after a successful rename. */
  onRenamed: (title: string) => void;
  /** Open in edit mode, as if the title had been clicked: the sidebar row menu's Rename asks for it
   *  from outside. `onEditStarted` is called once it has, so the asker can drop the request and a
   *  later remount doesn't start editing on its own. */
  startEditing?: boolean;
  onEditStarted?: () => void;
}

/** The active conversation's title, click-to-edit (board card 7E). Auto-generated titles are editable;
 *  committing a change latches it as user-chosen so the background title pass never overwrites it. Enter or
 *  blur commits, Escape cancels. Kept presentational + reusable so the chat header and the project sidebar's
 *  history header share one affordance. */
export function ConversationTitle({
  conversationId,
  title,
  onRenamed,
  startEditing,
  onEditStarted,
}: Props) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(title);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!startEditing) return;
    setDraft(title);
    setEditing(true);
    onEditStarted?.();
    // Only the request matters; the title it seeds from is read once, as a click reads it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [startEditing]);

  // Re-sync the draft whenever the conversation or its title changes underneath us (e.g. the background
  // pass renamed it, or the user switched chats) — but never while actively editing.
  useEffect(() => {
    if (!editing) setDraft(title);
  }, [title, conversationId, editing]);

  useEffect(() => {
    if (editing) {
      inputRef.current?.focus();
      inputRef.current?.select();
    }
  }, [editing]);

  async function commit() {
    const next = draft.trim();
    setEditing(false);
    if (!next || next === title) {
      setDraft(title);
      return;
    }
    try {
      const saved = await renameConversation(conversationId, next);
      onRenamed(saved);
    } catch {
      setDraft(title); // rename rejected (e.g. blank) — revert to the stored title
    }
  }

  if (editing) {
    return (
      <input
        ref={inputRef}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            void commit();
          } else if (e.key === "Escape") {
            e.preventDefault();
            setEditing(false);
            setDraft(title);
          }
        }}
        className="w-full max-w-md rounded-[var(--radius-sm)] border border-ink4 bg-surface px-2 py-1 font-head text-sm text-ink outline-none focus:border-accent-text"
        aria-label="Conversation title"
      />
    );
  }

  return (
    <button
      type="button"
      onClick={() => {
        setDraft(title);
        setEditing(true);
      }}
      title="Rename conversation"
      className="max-w-md truncate border-0 bg-transparent p-0 text-left font-head text-sm text-ink2 transition hover:text-ink motion-reduce:transition-none"
    >
      {title}
    </button>
  );
}
