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
//
// **The money prompt** is this same dialog for `ai` (spec §1.6). It asks PM which route a call would
// take (`mod_ai_prompt_info`: the person's own settings, not anything the mod said) and, when the
// route can cost money, shows a monthly limit field that rides with the Allow, written in the same
// transaction as the grant. A route that costs nothing asks for no limit, so a mod moved to a paid
// route later is asked then (the call's own state check). Its sentences are in `aiMoneyWords`.

import { useEffect, useLayoutEffect, useState } from "react";
import { modAiPromptInfo } from "../../lib/ipc";
import type { AiPromptInfo } from "../../lib/modsTypes";
import { Button, Callout, Dialog, Field, Input } from "../ui";
import { formatUsd, parseCapUsd } from "./aiCap";
import { MONEY_PROMPT_WORDS as WORDS } from "./aiMoneyWords";
import { ModName } from "./ModName";
import { permissionWords } from "./permissionWords";
import { TierBadge } from "./TierBadge";

/** How long Allow and No stay off after the question appears. This is the one consent screen the
 *  mod itself opens (by calling something), so it controls the moment it lands under the cursor: a
 *  mod that draws "click here twice" over where Allow will be could have the second click granted.
 *  Half a second is longer than a double-click and shorter than a person reading the question.
 *  "Not now" is never held, it records nothing. The caller keys the prompt by permission, so each
 *  new question starts its own wait; the wait also starts again whenever the dialog's content
 *  changes under the cursor (the money prompt's route arrives after it opens, and adds a field). */
export const PROMPT_GUARD_MS = 500;

/** What the money prompt knows about the route. */
type Route = { kind: "loading" } | { kind: "ready"; info: AiPromptInfo } | { kind: "failed" };

export function ModPermissionPrompt({
  modId,
  name,
  tier,
  permission,
  working,
  error,
  onAnswer,
  onLater,
}: {
  /** The mod's id, to ask PM about its route when the question is about money. */
  modId: string;
  name: string;
  /** The backend's tier word; one PM doesn't know is treated as the least trusted, like the badge. */
  tier: string;
  /** The permission's id, as the backend named it. */
  permission: string;
  /** An answer is being recorded. */
  working: boolean;
  /** Why recording the last answer failed, in the backend's words. */
  error: string | null;
  /** `capUsd` is the monthly limit that goes with an Allow of `ai` on a route that can cost money. */
  onAnswer: (allow: boolean, capUsd?: number) => void;
  onLater: () => void;
}) {
  const words = permissionWords(permission);
  const money = permission === "ai";

  const [route, setRoute] = useState<Route>({ kind: "loading" });
  const [capText, setCapText] = useState("");
  useEffect(() => {
    if (!money) return;
    let cancelled = false;
    modAiPromptInfo(modId).then(
      (info) => {
        if (cancelled) return;
        setCapText(formatUsd(info.suggestedCapUsd));
        setRoute({ kind: "ready", info });
      },
      () => {
        if (!cancelled) setRoute({ kind: "failed" });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [money, modId]);

  // The hold (see PROMPT_GUARD_MS) belongs to what is on screen: it is armed for one content, and a
  // new content (the route arriving, or failing) is unarmed until its own wait has passed. A layout
  // effect, so the wait starts in the commit that paints the content, not a frame after it.
  const content = money ? route.kind : "plain";
  const [armedFor, setArmedFor] = useState<string | null>(null);
  useLayoutEffect(() => {
    const t = setTimeout(() => setArmedFor(content), PROMPT_GUARD_MS);
    return () => clearTimeout(t);
  }, [content]);
  const armed = armedFor === content;

  const info = route.kind === "ready" ? route.info : null;
  const needsCap = money && info !== null && info.paid;
  const cap = needsCap ? parseCapUsd(capText) : null;
  // A money question can be answered yes only once PM knows the route, and, on a route that can
  // cost money, once the field holds a limit PM will store.
  const canAllow = !money || (info !== null && (!needsCap || cap !== null));
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
          <Button
            variant="primary"
            onClick={() => onAnswer(true, needsCap && cap !== null ? cap : undefined)}
            disabled={held || !canAllow}
          >
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
        {money && (
          <div className="space-y-2" data-testid="money-prompt">
            {route.kind === "loading" && <p className="text-ink3">{WORDS.checking()}</p>}
            {route.kind === "failed" && <Callout size="md">{WORDS.failed()}</Callout>}
            {info && (
              <>
                <p>{WORDS.intro()}</p>
                <p>{WORDS.models(info)}</p>
                <p>{WORDS.sends()}</p>
                <p className="font-medium text-ink">{WORDS.cost(info)}</p>
                {needsCap && (
                  <Field
                    label={WORDS.capLabel()}
                    description={WORDS.capHelp()}
                    error={cap === null ? WORDS.capInvalid() : undefined}
                  >
                    {(controlProps) => (
                      <div className="flex items-center gap-2">
                        <span aria-hidden="true" className="text-ink3">
                          $
                        </span>
                        <Input
                          {...controlProps}
                          inputMode="decimal"
                          autoComplete="off"
                          value={capText}
                          disabled={working}
                          onChange={(e) => setCapText(e.target.value)}
                        />
                      </div>
                    )}
                  </Field>
                )}
              </>
            )}
          </div>
        )}
        {tier !== "first" && tier !== "second" && (
          <p className="text-ink3">Nobody has checked this mod, so PM can&apos;t vouch for it.</p>
        )}
        <p className="text-xs text-ink3">
          {/* The limit field says this itself; once is enough. */}
          {needsCap ? "" : "You can change this whenever you like in the Mods tab. "}
          &ldquo;Not now&rdquo; leaves it unanswered and the mod is told it doesn&apos;t have
          permission yet.
        </p>
        {error && <Callout size="md">{error}</Callout>}
      </div>
    </Dialog>
  );
}
