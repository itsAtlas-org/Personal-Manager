// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The People tab's notices that carry an Undo: "Moved Ada to Trash. Undo", and later a merge's and a
// deleted label's. They float bottom centre like the calendar's write notices (`WriteNotices`), each
// mixing its tone into `--bg` so what is behind doesn't show through, and sit under an open dialog's
// scrim (z-50) so a notice that never goes can't cover a dialog's buttons. The newest Undo is also
// Ctrl+Z (Cmd+Z on a Mac), when focus isn't in a text field and no dialog is open (`calendarUndoKey`,
// the one rule for it).
//
// One small piece on purpose: the notices live in `useUndoNotices`, a view pushes one with its Undo
// as a closure, and merge and delete-label reuse both as they are.

import { useCallback, useEffect, useRef, useState } from "react";
import { calendarUndoKey } from "../../lib/calendarNavKey";
import { IS_MAC } from "../../lib/setupGuide";
import { isAnyDialogOpen } from "../../lib/useDialogLayer";
import { Button, Callout, TONE_MIX, TONE_TOKEN, Tooltip } from "../ui";

const UNDO_KEY = IS_MAC ? "⌘Z" : "Ctrl+Z";

/** How long a notice stays before it goes by itself. A merge asks for more (spec 4.3). */
export const UNDO_NOTICE_MS = 12_000;

export interface UndoNotice {
  id: number;
  /** The words, ending where "Undo" begins: "Moved Ada to Trash." */
  text: string;
  /** Put it back. May be async; a failure is the closure's to report. */
  undo: () => void | Promise<void>;
}

export interface UndoNotices {
  notices: readonly UndoNotice[];
  /** Show a notice. Resolves to its id. `ttlMs` is how long it stays. */
  push: (text: string, undo: UndoNotice["undo"], ttlMs?: number) => number;
  dismiss: (id: number) => void;
  /** Press one notice's Undo (and dismiss it). */
  undo: (id: number) => void;
  /** Press the newest Undo. False when there is none. */
  undoNewest: () => boolean;
}

export function useUndoNotices(): UndoNotices {
  const [notices, setNotices] = useState<UndoNotice[]>([]);
  const next = useRef(1);
  const timers = useRef(new Map<number, number>());
  const latest = useRef<readonly UndoNotice[]>([]);
  latest.current = notices;

  const dismiss = useCallback((id: number) => {
    window.clearTimeout(timers.current.get(id));
    timers.current.delete(id);
    setNotices((all) => all.filter((n) => n.id !== id));
  }, []);

  const push = useCallback(
    (text: string, undo: UndoNotice["undo"], ttlMs = UNDO_NOTICE_MS) => {
      const id = next.current++;
      setNotices((all) => [...all, { id, text, undo }]);
      timers.current.set(
        id,
        window.setTimeout(() => dismiss(id), ttlMs),
      );
      return id;
    },
    [dismiss],
  );

  const undo = useCallback(
    (id: number) => {
      const notice = latest.current.find((n) => n.id === id);
      if (!notice) return;
      dismiss(id);
      void Promise.resolve(notice.undo()).catch(() => {});
    },
    [dismiss],
  );

  const undoNewest = useCallback(() => {
    const newest = latest.current[latest.current.length - 1];
    if (!newest) return false;
    undo(newest.id);
    return true;
  }, [undo]);

  useEffect(() => {
    const live = timers.current;
    return () => {
      for (const t of live.values()) window.clearTimeout(t);
      live.clear();
    };
  }, []);

  return { notices, push, dismiss, undo, undoNewest };
}

/** Ctrl/Cmd+Z presses the newest Undo, unless a field has focus (its own Undo for its text comes
 *  first) or a dialog is open. A press with nothing to undo is left alone. */
export function useUndoShortcut(undoNewest: () => boolean): void {
  const press = useRef(undoNewest);
  press.current = undoNewest;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!calendarUndoKey(e, isAnyDialogOpen())) return;
      if (press.current()) e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}

interface Props {
  notices: readonly UndoNotice[];
  onUndo: (id: number) => void;
  onDismiss: (id: number) => void;
}

export function UndoStrip({ notices, onUndo, onDismiss }: Props) {
  if (notices.length === 0) return null;
  const newest = notices[notices.length - 1].id;
  return (
    <div
      className="fixed bottom-4 left-1/2 z-[48] flex w-max max-w-md -translate-x-1/2 flex-col gap-2"
      data-help="people-trash"
    >
      {notices.map((n) => (
        <Callout
          key={n.id}
          tone="info"
          size="md"
          body="ink"
          live
          className="flex items-center justify-between gap-3 text-ink2 shadow-lg"
          style={{
            background: `color-mix(in oklab, var(${TONE_TOKEN.info}) ${TONE_MIX.surface}%, var(--bg))`,
          }}
        >
          <span>{n.text}</span>
          <span className="flex shrink-0 items-center gap-2">
            <Tooltip label={n.id === newest ? `Undo · ${UNDO_KEY}` : "Undo"}>
              <Button
                variant="secondary"
                size="xs"
                aria-label={`Undo: ${n.text}`}
                aria-keyshortcuts={n.id === newest ? "Control+Z Meta+Z" : undefined}
                onClick={() => onUndo(n.id)}
              >
                Undo
              </Button>
            </Tooltip>
            <button
              type="button"
              onClick={() => onDismiss(n.id)}
              aria-label="Dismiss"
              className="text-ink4 hover:text-ink"
            >
              ×
            </button>
          </span>
        </Callout>
      ))}
    </div>
  );
}
