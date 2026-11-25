// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The strips about the Pinboard's move into its mod (PR C, K8). PM draws them, never the mod: the
// mod might not be running (or installed) when one matters, and what they say is about the person's
// own board in PM's store. They are the only way the move can tell anyone anything, since PM sends no
// OS notifications, so they show on every tab (App) and inside Settings, whose scrim covers the
// app's copy (the rule `ModNoticeStrips` follows: App draws its own only while Settings is closed).
//
//   conflict  The mod already held a board with content, so the move kept BOTH. Two answers, each
//             leaving both boards recoverable: "Use my old board" (the mod's board is first saved as
//             "board before move", then the old one takes its place) and "Keep this one". The words
//             are always in view: they say what each button does to which board, which is the one
//             thing a person choosing between two boards needs (settings-info doctrine: a loss
//             warning is never folded away).
//   repaired  A few characters could not be stored as they were and were replaced in the mod's copy.
//             The old board is unchanged. One button clears it.
//   refused   The board cannot move as it is (not readable, or bigger than a mod can save). It is
//             safe in PM's store, nothing was changed, and PM says so. Nothing to press: it clears
//             itself when the board can move.
//
// Dormant: a store with no move owed, or one that moved cleanly, draws nothing.

import { useState } from "react";
import { pinboardMoveDismiss, pinboardUseOldBoard } from "../../lib/ipc";
import { failureText } from "../../lib/modsTypes";
import { usePinboardMove } from "../../lib/usePinboardMove";
import { Button, Callout } from "../ui";

type Busy = "use" | "keep" | "ok" | null;

export function PinboardMoveStrips() {
  const { state, refresh } = usePinboardMove();
  const [busy, setBusy] = useState<Busy>(null);
  const [problem, setProblem] = useState<string | null>(null);
  if (!state || (!state.conflict && !state.repaired && !state.refused)) return null;

  async function answer(which: Exclude<Busy, null>, act: () => Promise<unknown>) {
    setBusy(which);
    setProblem(null);
    try {
      await act();
    } catch (e) {
      setProblem(
        failureText(e, "PM couldn't change anything just now. Both boards are still safe."),
      );
    } finally {
      setBusy(null);
      void refresh();
    }
  }

  return (
    <div data-testid="pinboard-move-strips">
      {state.refused && (
        <Callout tone="warning" size="md" live variant="strip">
          <p className="break-words">
            {state.refused === "tooBig"
              ? "PM couldn't move your Pinboard into its mod yet, because the board is bigger than a mod can save (1 MiB). "
              : "PM couldn't move your Pinboard into its mod yet, because the saved board couldn't be read as it is. "}
            Your board is safe in PM&apos;s store, and nothing was changed.
          </p>
        </Callout>
      )}
      {state.conflict && (
        <Callout tone="warning" size="md" live variant="strip">
          <div className="space-y-2">
            <p className="break-words">
              You have a Pinboard in the mod and an older one in PM&apos;s store.
            </p>
            <p className="break-words text-xs">
              &ldquo;Use my old board&rdquo; first saves the mod&apos;s board as &ldquo;board before
              move&rdquo;, then puts your old board in its place. &ldquo;Keep this one&rdquo; keeps
              the mod&apos;s board. Nothing is deleted either way, and the old board stays in
              PM&apos;s store as it was when this notice came up: a project renamed or merged since
              then is not renamed on it.
            </p>
            {problem && <p className="break-words text-xs">{problem}</p>}
            <div className="flex flex-wrap gap-2">
              <Button
                variant="secondary"
                size="sm"
                disabled={busy !== null}
                onClick={() => void answer("use", pinboardUseOldBoard)}
              >
                {busy === "use" ? "Switching..." : "Use my old board"}
              </Button>
              <Button
                variant="secondary"
                size="sm"
                disabled={busy !== null}
                onClick={() => void answer("keep", () => pinboardMoveDismiss("conflict"))}
              >
                Keep this one
              </Button>
            </div>
          </div>
        </Callout>
      )}
      {state.repaired && (
        <Callout tone="info" size="md" live variant="strip">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <p className="min-w-0 break-words">
              Your Pinboard moved into its mod. A few characters in it couldn&apos;t be stored as
              they were, so they were replaced with a plain replacement mark. Your old board in
              PM&apos;s store is unchanged.
            </p>
            <Button
              variant="tertiary"
              size="sm"
              className="shrink-0"
              disabled={busy !== null}
              onClick={() => void answer("ok", () => pinboardMoveDismiss("repaired"))}
            >
              OK
            </Button>
          </div>
        </Callout>
      )}
    </div>
  );
}
