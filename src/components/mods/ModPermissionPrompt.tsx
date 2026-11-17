// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The first-use question (Mods spec §1.5): a mod called something its manifest declares and the user
// hasn't answered yet, so the backend refused the call with MOD-2002 and the permission it needs. PM
// asks here, in its own dialog over the app, never in the mod's frame: the mod can't draw it, move it
// or press its buttons. The words for the permission are PM's (permissionWords), the mod's name and
// tier are the backend's row's; nothing the mod says appears in it but its name, quoted.
//
// Three answers: Allow and No are recorded (`mod_permission_decide`), and "Not now" records nothing
// (the mod is told it still needs permission, and isn't asked again until its next session, so a mod
// can't nag by calling in a loop). ModFrame holds the mod's call while this is open and retries it
// once after an Allow.

import { useEffect, useState } from "react";
import { Button, Callout, Dialog } from "../ui";
import { ModName } from "./ModName";
import { permissionWords } from "./permissionWords";
import { TierBadge } from "./TierBadge";

/** How long Allow and No stay off after the question appears. This is the one consent screen the
 *  mod itself opens (by calling something), so it controls the moment it lands under the cursor: a
 *  mod that draws "click here twice" over where Allow will be could have the second click granted.
 *  Half a second is longer than a double-click and shorter than a person reading the question.
 *  "Not now" is never held, it records nothing. The caller keys the prompt by permission, so each
 *  new question starts its own wait. */
export const PROMPT_GUARD_MS = 500;

export function ModPermissionPrompt({
  name,
  tier,
  permission,
  working,
  error,
  onAnswer,
  onLater,
}: {
  name: string;
  /** The backend's tier word; one PM doesn't know is treated as the least trusted, like the badge. */
  tier: string;
  /** The permission's id, as the backend named it. */
  permission: string;
  /** An answer is being recorded. */
  working: boolean;
  /** Why recording the last answer failed, in the backend's words. */
  error: string | null;
  onAnswer: (allow: boolean) => void;
  onLater: () => void;
}) {
  const words = permissionWords(permission);
  const [armed, setArmed] = useState(false);
  useEffect(() => {
    const t = setTimeout(() => setArmed(true), PROMPT_GUARD_MS);
    return () => clearTimeout(t);
  }, []);
  const held = working || !armed;
  return (
    <Dialog
      open
      title={
        <>
          Let <ModName name={name} /> do this?
        </>
      }
      onClose={working ? () => {} : onLater}
      widthClassName="max-w-md"
      helpId="mods-permission-prompt"
      footer={
        <>
          <Button variant="tertiary" onClick={onLater} disabled={working} data-autofocus>
            Not now
          </Button>
          <Button variant="secondary" onClick={() => onAnswer(false)} disabled={held}>
            No
          </Button>
          <Button variant="primary" onClick={() => onAnswer(true)} disabled={held}>
            {working ? "Saving…" : "Allow"}
          </Button>
        </>
      }
    >
      <div className="mt-2 space-y-3 text-sm text-ink2">
        <p className="flex flex-wrap items-center gap-2 text-xs text-ink3">
          <TierBadge tier={tier} />
        </p>
        <p>The mod is asking, for the first time, to:</p>
        <p className="rounded-[var(--radius-sm)] border border-border p-2 font-medium text-ink">
          {words}
        </p>
        {tier !== "first" && tier !== "second" && (
          <p className="text-ink3">Nobody has checked this mod, so PM can&apos;t vouch for it.</p>
        )}
        <p className="text-xs text-ink3">
          You can change this whenever you like in the Mods tab. &ldquo;Not now&rdquo; leaves it
          unanswered and the mod is told it doesn&apos;t have permission yet.
        </p>
        {error && <Callout size="md">{error}</Callout>}
      </div>
    </Dialog>
  );
}
