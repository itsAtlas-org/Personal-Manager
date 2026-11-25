// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// PM's own yes/no for a mod's call (Mods spec §1.5; PR C, K1 and K5): the backend answered a call with
// MOD-2003 and a question it built itself, from PM's data and never from the mod's words: deleting a
// milestone the mod didn't make, creating a project the mod named, opening an address for a mod that is
// not first party. PM asks here, in its own dialog over the app, so the mod can neither draw it nor
// press its buttons.
//
// - The summary is `question`, then any number of lines that are the thing asked about (an address, a new
//   project's name, a milestone's label and its project behind a marker PM writes) or the note that an
//   address was clamped. Line 1 is the question as plain text, PM's words only; the lines after it are
//   drawn in monospace, each on its own line, because that is what a person reads character by
//   character. Both are React text: markup in a line is shown as written, and a line is drawn as the
//   shared rule for a person's words keeps it (`cleanUserLine`: an emoji sequence whole, hidden
//   characters as spaces).
// - Two answers: Accept and Decline. The dialog is a modal alert (`role="alertdialog"`, `aria-modal`,
//   named by its title and described by the question); focus starts on Decline, so Enter or a stray
//   click on arrival never says yes; focus is trapped and handed back to what held it (Modal). Escape,
//   the scrim and closing the dialog all mean Decline.
// - Accept is held for half a second after the question appears, like the first-use question
//   (ModPermissionPrompt): the mod chooses the moment this lands under the cursor.
// - Nothing here decides anything: the answer goes to `mod_confirm`, which finds the action the token
//   stored in Rust and checks it all again. The page never names an action.

import { useEffect, useId, useState } from "react";
import { Button, Dialog } from "../ui";
import { readSummary } from "./confirmSummary";
import { ModName } from "./ModName";
import { PROMPT_GUARD_MS } from "./ModPermissionPrompt";
import { TierBadge } from "./TierBadge";

export function ModConfirmDialog({
  name,
  tier,
  summary,
  working,
  onAnswer,
}: {
  name: string;
  /** The backend's tier word for the mod; one PM doesn't know is the least trusted, like the badge. */
  tier: string;
  /** What the backend asked: line 1 the question, later lines the thing asked about. */
  summary: string;
  /** The answer was sent and PM is waiting for what came of it. */
  working: boolean;
  /** `true` is Accept, `false` is Decline (and Escape, and closing). */
  onAnswer: (accept: boolean) => void;
}) {
  const { question, lines, hidden } = readSummary(summary);
  const descriptionId = useId();
  // Accept waits out the hold (see the header); Decline never does.
  const [armed, setArmed] = useState(false);
  useEffect(() => {
    const t = setTimeout(() => setArmed(true), PROMPT_GUARD_MS);
    return () => clearTimeout(t);
  }, []);
  return (
    <Dialog
      open
      role="alertdialog"
      describedBy={descriptionId}
      title={
        <>
          <ModName name={name} /> is asking
        </>
      }
      subtitle={<TierBadge tier={tier} />}
      onClose={working ? () => {} : () => onAnswer(false)}
      widthClassName="max-w-md"
      footer={
        <>
          <Button
            variant="secondary"
            onClick={() => onAnswer(false)}
            disabled={working}
            data-autofocus
          >
            Decline
          </Button>
          <Button variant="primary" onClick={() => onAnswer(true)} disabled={working || !armed}>
            {working ? "Working…" : "Accept"}
          </Button>
        </>
      }
    >
      <div id={descriptionId} className="mt-3 space-y-2 text-sm text-ink2">
        <p className="whitespace-pre-wrap break-words font-medium text-ink">{question}</p>
        {lines.map((line, i) => (
          <p
            key={i}
            className="whitespace-pre-wrap break-all rounded-[var(--radius-sm)] border border-border bg-bg px-2 py-1 font-mono text-xs text-ink"
          >
            {line}
          </p>
        ))}
        {hidden > 0 && (
          <p className="text-xs text-ink3">
            {hidden === 1 ? "1 more line is" : `${hidden} more lines are`} not shown. Decline unless
            you are sure.
          </p>
        )}
      </div>
    </Dialog>
  );
}
