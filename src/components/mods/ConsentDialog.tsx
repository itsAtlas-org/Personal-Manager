// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The consent screen of a SIGNED install, update or repair, drawn from what the backend returned
// (`approveUpdate`, and the same `ConsentScreen` an install from the catalogue or a file returns):
// every permission the screen carries, in the order the backend listed them, in PM's words with the
// maker's reason beside them as the maker's. What is new in an update is marked, and the widening
// list is shown first. The page never widens, reorders away or hides a permission, and never decides
// trust: `tier`, `kind` and `unsigned` are the backend's.
//
// A screen with `unsigned` set is the stark one (type the id to confirm, read or internet): this
// dialog does not draw it. An update never comes with one, and drawing it here without its
// confirmation step would be a weaker screen than the one it stands for, so it says so and stops.
//
// Nothing is installed until "Update"/"Install" is pressed: it sends the staged token with an answer
// for exactly the permissions marked `answerRequired`, and the backend refuses a screen that leaves
// one out. The token is single-use and expires in minutes; a stale one comes back as the backend's own
// `consent-expired` sentence. Like every Mods action it runs to its end whatever is on screen
// (lib/modActions), and the staged screen is kept there so it survives a tab switch.

import { useState } from "react";
import { installModConsented } from "../../lib/ipc";
import {
  addModNote,
  clearModError,
  runModAction,
  setModScreen,
  useModActions,
} from "../../lib/modActions";
import type { ConsentScreen } from "../../lib/modsTypes";
import { Button, Callout, Dialog } from "../ui";
import { modText } from "./modText";
import { classWords, permissionWords, standingLine, wideningWords } from "./permissionWords";
import { TierBadge } from "./TierBadge";

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
const DOING: Record<ConsentScreen["kind"], string> = {
  update: "Updating",
  repair: "Putting back",
  fresh: "Installing",
};

export function ConsentDialog({
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

  function close() {
    clearModError(screen.id);
    setModScreen(screen.id, null);
  }

  if (screen.unsigned) {
    return (
      <Dialog
        open
        title="This one needs another screen"
        onClose={close}
        widthClassName="max-w-md"
        footer={
          <Button variant="tertiary" onClick={close}>
            Close
          </Button>
        }
      >
        <p className="mt-2 text-sm text-ink2">
          This mod comes from a file nobody has checked. PM shows those on the Install from file
          screen, where you type its name to confirm.
        </p>
      </Dialog>
    );
  }

  function go() {
    void runModAction(
      screen.id,
      DOING[kind],
      "PM couldn't finish that.",
      () => installModConsented(screen.token, answers),
      (done) => {
        setModScreen(screen.id, null);
        if (!done.grantsSaved) {
          addModNote(
            "warning",
            `${modText(screen.name)} was ${kind === "update" ? "updated" : "installed"}, but PM couldn't save your answers: ${done.grantsProblem?.message ?? "no reason given"} Everything it asked for is off until you turn it on in the Mods tab.`,
          );
        }
      },
    ).then(() => refresh());
  }

  return (
    <Dialog
      open
      title={TITLE[kind](modText(screen.name), screen.version)}
      onClose={working ? () => {} : close}
      widthClassName="max-w-lg"
      helpId="mods-consent"
      footer={
        <>
          <Button variant="tertiary" onClick={close} disabled={working}>
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
