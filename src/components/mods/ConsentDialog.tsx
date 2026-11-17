// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The consent screen of an install, update or repair, drawn from what the backend returned
// (`approveUpdate`, and the same `ConsentScreen` an install from the catalogue or a file returns):
// every permission the screen carries, in the order the backend listed them, in PM's words with the
// maker's reason beside them as the maker's. What is new in an update is marked, and the widening
// list is shown first. The page never widens, reorders away or hides a permission, and never decides
// trust: `tier`, `kind` and `unsigned` are the backend's.
//
// Three screens come through here:
//   - the SIGNED one (this file): first party lists what it asks; second party adds "reviewed by
//     Atlas at commit …"; every permission asked at install carries a checkbox, and an answer for each
//     goes to the backend (it refuses a screen that leaves one out);
//   - the STARK one for an unsigned file or a test folder (`screen.unsigned` set): its own dialog
//     (UnsignedConsentDialog), because it asks for much more before the button works;
//   - the EXPIRED one: a screen's token is single-use and good for ten minutes, and a lock or a vault
//     switch ends it early. When the backend says so, the screen is replaced by one that says so and
//     offers to start again (the file read again, the mod downloaded again) rather than a button that
//     can't work.
//
// Nothing is installed until the button is pressed. Like every Mods action it runs to its end whatever
// is on screen (lib/modActions, lib/modInstall), and the staged screen is kept there so it survives a
// tab switch.

import { useState } from "react";
import { clearModError, setModScreen, useModActions } from "../../lib/modActions";
import { DOING, finishInstall, restartScreen } from "../../lib/modInstall";
import type { ConsentScreen } from "../../lib/modsTypes";
import { Button, Callout, Dialog } from "../ui";
import { modText } from "./modText";
import { classWords, permissionWords, standingLine, wideningWords } from "./permissionWords";
import { TierBadge } from "./TierBadge";
import { UnsignedConsentDialog } from "./UnsignedConsentDialog";

const TITLE: Record<ConsentScreen["kind"], (name: string, version: string) => string> = {
  update: (name, version) => `Update ${name} to version ${version}?`,
  repair: (name) => `Put ${name} back?`,
  fresh: (name) => `Install ${name}?`,
};
const GO: Record<ConsentScreen["kind"], string> = {
  update: "Update",
  repair: "Put back",
  fresh: "Install",
};

/** Close a staged screen: forget it and the refusal it was showing. */
function closeScreen(screen: ConsentScreen): void {
  clearModError(screen.id);
  setModScreen(screen.id, null);
}

export function ConsentDialog({
  screen,
  refresh,
}: {
  screen: ConsentScreen;
  refresh: () => Promise<void>;
}) {
  const { expired, restarts } = useModActions();
  if (expired.has(screen.id)) {
    return <ExpiredDialog screen={screen} restart={restarts.get(screen.id)} />;
  }
  if (screen.unsigned) return <UnsignedConsentDialog screen={screen} refresh={refresh} />;
  return <SignedConsentDialog screen={screen} refresh={refresh} />;
}

function ExpiredDialog({
  screen,
  restart,
}: {
  screen: ConsentScreen;
  restart: (() => Promise<ConsentScreen>) | undefined;
}) {
  const { busy, errors } = useModActions();
  // The refusal that expired the screen is not news here; only a failed "Start again" is.
  const [tried, setTried] = useState(false);
  const working = busy.has(screen.id);
  const error = tried && !working ? (errors.get(screen.id) ?? null) : null;

  function again() {
    if (!restart) return;
    setTried(true);
    void restartScreen(screen, restart);
  }

  return (
    <Dialog
      open
      title="This screen ran out of time"
      onClose={working ? () => {} : () => closeScreen(screen)}
      widthClassName="max-w-md"
      helpId="mods-consent-expired"
      footer={
        <>
          <Button variant="tertiary" onClick={() => closeScreen(screen)} disabled={working}>
            Close
          </Button>
          {restart && (
            <Button variant="primary" onClick={again} disabled={working}>
              {working ? "Starting again…" : "Start again"}
            </Button>
          )}
        </>
      }
    >
      <div className="mt-2 space-y-3 text-sm text-ink2">
        <p>
          Nothing was installed. The install screen for {modText(screen.name)} only lasts ten
          minutes, and locking PM or switching to another vault ends it sooner.
        </p>
        <p>
          {restart
            ? "Start again to get a fresh one."
            : "Close this and start again from the Mods tab."}
        </p>
        {error && <Callout size="md">{error}</Callout>}
      </div>
    </Dialog>
  );
}

function SignedConsentDialog({
  screen,
  refresh,
}: {
  screen: ConsentScreen;
  refresh: () => Promise<void>;
}) {
  const asked = screen.permissions.filter((p) => p.answerRequired);
  const [answers, setAnswers] = useState<Record<string, boolean>>(() =>
    Object.fromEntries(asked.map((p) => [p.id, p.defaultAnswer])),
  );
  const { busy, errors } = useModActions();
  const working = busy.has(screen.id);
  const error = errors.get(screen.id) ?? null;
  const kind = screen.kind in TITLE ? screen.kind : "fresh";
  const name = modText(screen.name);

  function go() {
    // An answer for exactly the permissions the screen marked required, each yes or no.
    const decisions = Object.fromEntries(asked.map((p) => [p.id, answers[p.id] ?? false]));
    void finishInstall(screen, name, decisions).then(() => refresh());
  }

  return (
    <Dialog
      open
      title={TITLE[kind](name, screen.version)}
      onClose={working ? () => {} : () => closeScreen(screen)}
      widthClassName="max-w-lg"
      helpId="mods-consent"
      footer={
        <>
          <Button variant="tertiary" onClick={() => closeScreen(screen)} disabled={working}>
            Cancel
          </Button>
          <Button variant="primary" onClick={go} disabled={working}>
            {working ? `${DOING[kind]}…` : GO[kind]}
          </Button>
        </>
      }
    >
      <div className="mt-2 space-y-3 text-sm text-ink2">
        <p className="flex flex-wrap items-center gap-2 text-xs text-ink3">
          <TierBadge tier={screen.tier} />
          <span className="break-words">
            {screen.author ? `Made by ${modText(screen.author)}` : "Maker not named"}
            {screen.tier === "second" && screen.commit
              ? ` · reviewed by Atlas at commit ${modText(screen.commit).slice(0, 12)}`
              : ""}
          </span>
        </p>
        <p className="text-xs text-ink3">
          {screen.from === "file"
            ? "From a file you chose. PM checked its signature."
            : "From the catalogue. PM checked it is the one the catalogue lists."}
        </p>
        {screen.description && <p className="break-words">{modText(screen.description)}</p>}

        {kind === "update" && screen.widening.length > 0 && (
          <Callout tone="warning" size="md" live={false}>
            <p className="font-medium">This update asks for more than you allowed before:</p>
            <ul className="list-disc pl-5">
              {screen.widening.map((w) => (
                <li key={w} className="break-words">
                  {wideningWords(w)}
                </li>
              ))}
            </ul>
          </Callout>
        )}

        <div className="space-y-2">
          <p className="font-medium text-ink">What it asks to do</p>
          {screen.permissions.length === 0 ? (
            <p className="text-ink3">It doesn&apos;t ask for anything.</p>
          ) : (
            <ul className="space-y-2">
              {screen.permissions.map((p) => {
                const words = permissionWords(p.id);
                const cls = classWords(p.class);
                return (
                  <li key={p.id} className="rounded-[var(--radius-sm)] border border-border p-2">
                    <div className="flex items-start justify-between gap-3">
                      <span className="min-w-0 font-medium text-ink">
                        {words}
                        {p.isNew && (
                          <span className="ml-2 rounded-full border border-border2 px-1.5 py-px text-xs font-normal text-ink2">
                            New
                          </span>
                        )}
                      </span>
                      {cls && <span className="flex-none text-xs text-ink3">{cls}</span>}
                    </div>
                    {p.why && (
                      <p className="break-words text-xs text-ink3">
                        The maker says: &ldquo;{modText(p.why)}&rdquo;
                      </p>
                    )}
                    {p.answerRequired ? (
                      <label className="flex items-center gap-2 pt-1 text-ink2">
                        <input
                          type="checkbox"
                          className="accent-[var(--accent)]"
                          checked={answers[p.id] ?? false}
                          disabled={working}
                          aria-label={`Allow: ${words}`}
                          onChange={(e) =>
                            setAnswers((prev) => ({ ...prev, [p.id]: e.target.checked }))
                          }
                        />
                        Allow
                      </label>
                    ) : (
                      <p className="pt-1 text-xs text-ink3">{standingLine(p)}</p>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
        </div>

        {screen.internetHosts.length > 0 && (
          <p className="break-words text-xs text-ink3">
            Sites it may reach: {screen.internetHosts.map(modText).join(", ")}
          </p>
        )}
        {screen.secrets.length > 0 && (
          <p className="break-words text-xs text-ink3">
            Saved keys it keeps:{" "}
            {screen.secrets.map((s) => `${modText(s.name)} (for ${modText(s.host)})`).join(", ")}
          </p>
        )}

        {error && <Callout size="md">{error}</Callout>}
      </div>
    </Dialog>
  );
}
