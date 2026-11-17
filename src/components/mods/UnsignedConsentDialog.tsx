// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The stark screen for a mod nobody has checked (Mods spec §1.1, D3, §1.15): an unsigned `.pmmod`
// (`installUnsignedFile`) or a developer's test folder (`loadTestMod`). The backend marks it with
// `screen.unsigned`; this draws it, and the backend decides everything it says:
//
//   - Nothing about the mod is vouched for. Its name, maker, repository and commit are only what the
//     file says about itself, and are drawn that way.
//   - Every permission is asked one by one and starts at NO. A permission the backend says is `off`
//     (D3's "read your data or reach the internet, never both", judged together with the user's other
//     unchecked mods, or the web engine not being sealed) has no checkbox, only the backend's reason.
//   - When the file asks for both reading and the internet, the person chooses one side first; the
//     other side's permissions can't be allowed. The backend refuses both sides anyway
//     (`grant-refused`), so this is the page being clear, not the page being the rule.
//   - Installing needs the mod's id typed exactly. The page only enables the button when the typed id
//     equals `confirmId`; Rust checks it again (`confirm-mismatch`) before it looks at anything else.
//
// An answer is sent for every permission the screen marked `answerRequired`, `false` for the ones the
// person left unticked, the blocked ones and the ones on the side not chosen.

import { useId, useState } from "react";
import { finishInstall } from "../../lib/modInstall";
import { clearModError, setModScreen, useModActions } from "../../lib/modActions";
import type { ConsentPermission, ConsentScreen } from "../../lib/modsTypes";
import { Button, Callout, Dialog, Input, RadioChoice } from "../ui";
import { modText } from "./modText";
import { classWords, permissionWords, standingLine } from "./permissionWords";
import { TierBadge } from "./TierBadge";

type Side = "read" | "internet";
type Choice = Side | "neither";

const SIDE_WORDS: Record<Side, string> = {
  read: "reading your data",
  internet: "the internet",
};

/** One line of what the file says about itself. Empty claims are left out. */
function Claim({ label, value }: { label: string; value: string }) {
  const shown = modText(value);
  if (!shown) return null;
  return (
    <li className="break-words">
      <span className="text-ink3">{label}: </span>
      {shown}
    </li>
  );
}

export function UnsignedConsentDialog({
  screen,
  refresh,
}: {
  screen: ConsentScreen;
  refresh: () => Promise<void>;
}) {
  const stark = screen.unsigned;
  const typedId = useId();
  const asked = screen.permissions.filter((p) => p.answerRequired);
  // Every answer starts at no, whatever the screen carries as its default.
  const [answers, setAnswers] = useState<Record<string, boolean>>({});
  const [choice, setChoice] = useState<Choice>("neither");
  const [typed, setTyped] = useState("");
  const { busy, errors } = useModActions();
  const working = busy.has(screen.id);
  const error = errors.get(screen.id) ?? null;
  const name = modText(screen.name);

  // The page is only built for the stark screen; a caller that hands it another draws nothing.
  if (!stark) return null;
  const choiceOf = stark.readOrInternet;

  function sideOf(id: string): Side | null {
    if (!choiceOf) return null;
    if (choiceOf.read.includes(id)) return "read";
    if (choiceOf.internet.includes(id)) return "internet";
    return null;
  }
  /** The backend says this one is off (D3, an unsealed engine): there is nothing to allow. */
  const blocked = (p: ConsentPermission) => p.access === "off";
  /** On the side of the choice that is not the one chosen. */
  const offSide = (p: ConsentPermission) => {
    const side = sideOf(p.id);
    return side !== null && side !== choice;
  };
  const locked = (p: ConsentPermission) => blocked(p) || offSide(p);
  const sideBlocked = (side: Side) =>
    (choiceOf?.[side] ?? []).every((id) => {
      const p = screen.permissions.find((x) => x.id === id);
      return !p || blocked(p);
    });

  function pick(next: Choice) {
    setChoice(next);
    // The side no longer chosen forgets its yeses, so switching back doesn't revive them.
    setAnswers((prev) =>
      Object.fromEntries(
        Object.entries(prev).map(([id, yes]) => {
          const side = sideOf(id);
          return [id, yes && (side === null || side === next)];
        }),
      ),
    );
  }

  const confirmed = typed.trim() === stark.confirmId;
  const folder = stark.testFolder !== null;

  function go() {
    if (!confirmed) return;
    const decisions = Object.fromEntries(asked.map((p) => [p.id, !locked(p) && !!answers[p.id]]));
    void finishInstall(screen, name, decisions, typed.trim()).then(() => refresh());
  }

  function close() {
    clearModError(screen.id);
    setModScreen(screen.id, null);
  }

  return (
    <Dialog
      open
      chrome="bar"
      tone="danger"
      title={folder ? `Load ${name} as a test mod?` : `Install ${name}, which nobody has checked?`}
      onClose={working ? () => {} : close}
      closeDisabled={working}
      widthClassName="max-w-2xl"
      helpId="mods-consent-unsigned"
      footer={
        <>
          <Button variant="tertiary" onClick={close} disabled={working}>
            Cancel
          </Button>
          <Button variant="primary" onClick={go} disabled={working || !confirmed}>
            {working ? (folder ? "Loading…" : "Installing…") : folder ? "Load test mod" : "Install"}
          </Button>
        </>
      }
    >
      <div className="space-y-4 text-sm text-ink2">
        <Callout tone="danger" size="md" live={false}>
          <p className="font-medium">Nobody has checked this mod.</p>
          <p>
            Atlas hasn&apos;t reviewed it or signed it, and PM can&apos;t tell what it really does.
            It runs in a sealed box and can only do what you allow below. Go on only if you made it
            or you trust the person who did.
          </p>
        </Callout>

        <div className="space-y-1">
          <p className="flex flex-wrap items-center gap-2 text-xs text-ink3">
            <TierBadge tier="third" />
            <span>
              {folder ? "From a folder on this computer." : "From a file you chose."} Version{" "}
              {modText(screen.version)}
            </span>
          </p>
          <p className="text-xs text-ink3">
            What the {folder ? "folder" : "file"} says about itself. PM has not checked any of it:
          </p>
          <ul className="space-y-0.5 text-ink2">
            <Claim label="Name" value={screen.name} />
            <Claim label="Made by" value={screen.author} />
            <Claim label="Repository" value={screen.repo} />
            <Claim label="Commit" value={screen.commit} />
          </ul>
          {screen.description && <p className="break-words pt-1">{modText(screen.description)}</p>}
        </div>

        {folder && (
          <p className="break-words text-xs text-ink3">
            PM reads the folder {modText(stark.testFolder ?? "")} where it is and never copies or
            changes it. The mod, and whatever it stores, last until PM restarts.
          </p>
        )}
        {stark.reinstall && (
          <Callout tone="warning" size="md" live={false}>
            You already installed an unchecked mod with this id. This replaces it as a fresh
            install: what you allowed and any keys it saved are reset unless you allow them again.
            What it stored itself is kept.
          </Callout>
        )}

        {choiceOf && (
          <fieldset className="space-y-1" disabled={working}>
            <legend className="font-medium text-ink">Reading your data or the internet</legend>
            <p className="text-xs text-ink3">
              This mod asks for both. PM judges unchecked mods together: all of them may read your
              data or use the internet, never both, so one can&apos;t pass your data to another that
              sends it out. Choose one.
            </p>
            {choiceOf.heldByOthers && (
              <p className="text-xs text-ink3">
                One of your other unchecked mods already has {SIDE_WORDS[choiceOf.heldByOthers]}, so
                PM won&apos;t give this one the other.
              </p>
            )}
            <RadioChoice<Choice>
              name={`${typedId}-side`}
              value="neither"
              current={choice}
              onSelect={pick}
              label="Neither"
              detail="It gets no way to read your data and none to reach the internet."
              disabled={working}
            />
            <RadioChoice<Choice>
              name={`${typedId}-side`}
              value="read"
              current={choice}
              onSelect={pick}
              label="Let it read my data"
              detail="You can then allow the permissions below that read your data. It can't use the internet."
              disabled={working || sideBlocked("read")}
            />
            <RadioChoice<Choice>
              name={`${typedId}-side`}
              value="internet"
              current={choice}
              onSelect={pick}
              label="Let it use the internet"
              detail="You can then allow the permissions below that reach outside PM. It can't read your data."
              disabled={working || sideBlocked("internet")}
            />
          </fieldset>
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
                const side = sideOf(p.id);
                return (
                  <li key={p.id} className="rounded-[var(--radius-sm)] border border-border p-2">
                    <div className="flex items-start justify-between gap-3">
                      <span className="min-w-0 font-medium text-ink">{words}</span>
                      {cls && <span className="flex-none text-xs text-ink3">{cls}</span>}
                    </div>
                    {p.why && (
                      <p className="break-words text-xs text-ink3">
                        The file says: &ldquo;{modText(p.why)}&rdquo;
                      </p>
                    )}
                    {blocked(p) ? (
                      <p className="pt-1 text-xs text-ink3">
                        Off. {p.offBecause ?? "PM won't allow this one."}
                      </p>
                    ) : p.answerRequired ? (
                      <>
                        <label className="flex items-center gap-2 pt-1 text-ink2">
                          <input
                            type="checkbox"
                            className="accent-[var(--accent)]"
                            checked={!locked(p) && !!answers[p.id]}
                            disabled={working || locked(p)}
                            aria-label={`Allow: ${words}`}
                            onChange={(e) =>
                              setAnswers((prev) => ({ ...prev, [p.id]: e.target.checked }))
                            }
                          />
                          Allow
                        </label>
                        {side && offSide(p) && (
                          <p className="text-xs text-ink3">
                            Choose &ldquo;{SIDE_WORDS[side]}&rdquo; above to be able to allow this.
                          </p>
                        )}
                      </>
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
            Sites the file says it may reach: {screen.internetHosts.map(modText).join(", ")}
          </p>
        )}
        {screen.secrets.length > 0 && (
          <p className="break-words text-xs text-ink3">
            Saved keys the file says it keeps:{" "}
            {screen.secrets.map((s) => `${modText(s.name)} (for ${modText(s.host)})`).join(", ")}
          </p>
        )}

        <div className="space-y-1">
          <label htmlFor={typedId} className="block font-medium text-ink">
            To go on, type the mod&apos;s id: <code className="font-mono">{stark.confirmId}</code>
          </label>
          <Input
            id={typedId}
            value={typed}
            onChange={(e) => setTyped(e.currentTarget.value)}
            autoComplete="off"
            spellCheck={false}
            disabled={working}
          />
          {typed.trim() !== "" && !confirmed && (
            <p className="text-xs text-ink3">That isn&apos;t the id yet. Type it exactly.</p>
          )}
        </div>

        {error && <Callout size="md">{error}</Callout>}
      </div>
    </Dialog>
  );
}
