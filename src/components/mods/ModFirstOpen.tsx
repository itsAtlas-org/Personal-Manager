// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The first-open screen of a mod PM installed for the user (Mods spec D17): one screen, drawn by PM,
// with the Mods disclaimer and what the mod asks to do. The mod's frame doesn't start until "Turn
// on", which is `consent_mod`: the answers become grants, the mod starts and the disclaimer counts as
// accepted, all in one step in the backend. This page sends the answers; it doesn't decide them.
//
// What the mod supplied (its name, author, the reason it gives for each permission, the sites it
// names) is untrusted and drawn as plain text. What each permission MEANS is PM's wording
// (permissionWords.ts), so a maker can't soften it, and every permission the row declares is shown
// in the order the backend listed it: none is hidden or moved.
//
// The backend asks for one answer per permission it asks at install (`when: "install"`), and no
// others (first-use ones wait for their own prompt, automatic ones need none), so those are the
// checkboxes. They start on, which is what a person who pressed "Turn on" wants; each can be turned
// off, and a "no" is recorded as one.

import { useState } from "react";
import { consentMod } from "../../lib/ipc";
import { runModAction, useModActions } from "../../lib/modActions";
import { useModRows } from "../../lib/modRows";
import type { ModInfo } from "../../lib/modsTypes";
import { Button, Callout, Card } from "../ui";
import { ModName } from "./ModName";
import { modText } from "./modText";
import { ModsDisclaimer } from "./ModsDisclaimer";
import { classWords, permissionWords, standingLine } from "./permissionWords";
import { TierBadge } from "./TierBadge";

export function ModFirstOpen({
  mod,
  onTurnedOn,
  titleId,
}: {
  mod: ModInfo;
  /** The mod's row changed: read the rows again. Resolves when they are read. */
  onTurnedOn: () => Promise<void>;
  /** Set when the screen sits in a dialog: the heading's id for the dialog's name. */
  titleId?: string;
}) {
  const asked = mod.permissions.filter((p) => p.when === "install");
  const [answers, setAnswers] = useState<Record<string, boolean>>(() =>
    Object.fromEntries(asked.map((p) => [p.id, p.decision !== "denied"])),
  );
  // In flight in the shared store (lib/modActions), keyed by the mod like its row's buttons: this
  // screen, the Mods tab's row and its dialog all see one pending "Turning on", whichever of them
  // was on screen when it was pressed, and none can send a second.
  const { busy: inFlight, errors } = useModActions();
  const { problem } = useModRows();
  const busy = inFlight.has(mod.id);
  const error = errors.get(mod.id) ?? null;
  // The backend accepted the answers. If the rows then can't be read again this screen would sit on
  // the old row, so it says so and offers the read, instead of a button that looks stuck.
  const [saved, setSaved] = useState(false);

  function turnOn() {
    void runModAction(mod.id, "Turning on", "PM couldn't turn this mod on.", async () => {
      await consentMod(mod.id, answers);
      setSaved(true);
      // Part of the action, so the button stays off until the row says "running" (this screen goes
      // away with it): a second press in the gap would turn on what is already on.
      await onTurnedOn();
    });
  }

  function readAgain() {
    void runModAction(mod.id, "Reading the list again", "PM couldn't read the list.", onTurnedOn);
  }

  // Embedded (the Mods tab's row offers the same screen in a dialog, for a mod that has no tab of its
  // own to open): the dialog supplies the scrolling and the width, and names itself by this heading.
  const Heading = titleId ? "h2" : "h1";
  return (
    <div className={titleId ? undefined : "h-full overflow-y-auto"} data-help="mod-first-open">
      <div className={titleId ? "space-y-5" : "mx-auto max-w-2xl space-y-5 px-6 py-6"}>
        <header className="space-y-1">
          <Heading id={titleId} className="break-words font-head text-lg font-semibold text-ink">
            Turn on <ModName name={mod.name} />?
          </Heading>
          <p className="flex flex-wrap items-center gap-2 text-xs text-ink3">
            <TierBadge tier={mod.tier} />
            <span className="break-words">
              {mod.author ? `Made by ${modText(mod.author)}` : "Maker not named"} · version{" "}
              {mod.version}
            </span>
          </p>
          {mod.description && (
            <p className="break-words pt-1 text-sm text-ink2">{modText(mod.description)}</p>
          )}
        </header>

        {(mod.tier === "third" || !mod.signed) && (
          <Callout tone="warning" size="md" live={false}>
            Nobody has checked this mod, and PM can&apos;t vouch for it.
          </Callout>
        )}

        <Card className="p-4">
          <h2 className="mb-2 font-head text-sm font-semibold text-ink">About mods</h2>
          <ModsDisclaimer />
        </Card>

        <section aria-labelledby="mod-first-open-asks" className="space-y-2">
          <h2 id="mod-first-open-asks" className="font-head text-sm font-semibold text-ink">
            What this mod asks to do
          </h2>
          {mod.permissions.length === 0 ? (
            <p className="text-sm text-ink3">It doesn&apos;t ask for anything.</p>
          ) : (
            <ul className="space-y-2">
              {mod.permissions.map((p) => {
                const words = permissionWords(p.id);
                const kind = classWords(p.class);
                const isAsked = p.when === "install";
                return (
                  <li key={p.id}>
                    <Card className="space-y-1 p-3">
                      <div className="flex items-start justify-between gap-3">
                        <span className="min-w-0 text-sm font-medium text-ink">{words}</span>
                        {kind && <span className="flex-none text-xs text-ink3">{kind}</span>}
                      </div>
                      {p.why && (
                        <p className="break-words text-xs text-ink3">
                          The maker says: &ldquo;{modText(p.why)}&rdquo;
                        </p>
                      )}
                      {isAsked ? (
                        <label className="flex items-center gap-2 pt-1 text-sm text-ink2">
                          <input
                            type="checkbox"
                            className="accent-[var(--accent)]"
                            checked={answers[p.id] ?? false}
                            disabled={busy}
                            aria-label={`Allow: ${words}`}
                            onChange={(e) =>
                              setAnswers((prev) => ({ ...prev, [p.id]: e.target.checked }))
                            }
                          />
                          Allow
                        </label>
                      ) : (
                        <p className="text-xs text-ink3">{standingLine(p)}</p>
                      )}
                    </Card>
                  </li>
                );
              })}
            </ul>
          )}
          {mod.internetHosts.length > 0 && (
            <p className="break-words text-xs text-ink3">
              Sites it may reach: {mod.internetHosts.map(modText).join(", ")}
            </p>
          )}
        </section>

        {error && <Callout size="md">{error}</Callout>}
        {saved && !busy && problem && (
          <Callout size="md">
            <p className="break-words">
              PM turned it on, but couldn&apos;t read the list of mods again: {problem}
            </p>
            <div className="pt-1">
              <Button variant="secondary" size="sm" onClick={readAgain}>
                Read the list again
              </Button>
            </div>
          </Callout>
        )}

        <div className="flex items-center gap-3">
          <Button variant="primary" onClick={turnOn} disabled={busy || saved}>
            {busy ? "Turning on…" : "Turn on"}
          </Button>
          <p className="text-xs text-ink3">
            Turning it on also means you&apos;ve read the notice above.
          </p>
        </div>
      </div>
    </div>
  );
}
