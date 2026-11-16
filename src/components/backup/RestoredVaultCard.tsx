// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Shared restore result — surfaced at the top of the tab so either the file OR the Proton/Google
// restore flow shows it prominently, not buried under whichever section started it.
//
// Rendered unconditionally and short-circuiting on `restored == null` INSIDE the component, rather
// than being wrapped in `{restored && …}` by its parent: `restoreAsPrivate` is a user choice that
// survives `restored` going back to null today, and mounting the component with the card would
// reset it.

import { useState } from "react";

import type { RestoredMod, RestoreSummary, SwitchOutcome } from "../../lib/types";
import { formatDateTime } from "../../lib/format";
import { switchToVault } from "../../lib/ipc";
import { Button } from "../ui";

export interface RestoredVaultCardProps {
  restored: RestoreSummary | null;
  setError: (m: string | null) => void;
}

/** Whether the switch has something the user should read before the page reloads: a warning the
 *  reconcile raised (it used to be dropped), or a mod that isn't simply back on. */
function needsReading(out: SwitchOutcome): boolean {
  return out.warnings.length > 0 || out.mods.some((m) => m.standing !== "back");
}

/** The mods a restore lists. `pending` rows (before the switch) say only that the mod comes back if
 *  its files are here: a same-machine restore brings most back, so telling the user to install each
 *  one would be wrong for most of them. Settled rows (after it) say what happened, and what to do. */
function ModList({ mods }: { mods: RestoredMod[] }) {
  return (
    <ul className="mt-1 flex flex-col gap-0.5 text-xs text-ink3">
      {mods.map((m) => (
        <li key={m.id}>
          <span className="text-ink2">{m.name}</span>
          {m.standing === "back" ? (
            <span className="text-ink4"> is back</span>
          ) : m.standing === "pending" ? (
            <span className="text-ink4"> — comes back if its files are on this computer</span>
          ) : m.note ? (
            <span className="text-ink4"> — {m.note}</span>
          ) : (
            <span className="text-ink4"> is {m.standing}</span>
          )}
        </li>
      ))}
    </ul>
  );
}

export function RestoredVaultCard({ restored, setError }: RestoredVaultCardProps) {
  const [switching, setSwitching] = useState(false);
  // When a restored vault was passphrase-protected ("shareable"), the user chooses on this machine:
  // make it private (default — none of the sharing setup travels in a backup), or keep the passphrase.
  const [restoreAsPrivate, setRestoreAsPrivate] = useState(true);
  // What the switch answered, held only when there is something to read; otherwise the page reloads
  // at once, as it always did.
  const [outcome, setOutcome] = useState<SwitchOutcome | null>(null);

  async function doSwitch() {
    if (!restored) return;
    setError(null);
    try {
      setSwitching(true);
      // A device-mode restore is already private; only a passphrase restore honours the choice.
      const makePrivate = restored.key_mode === "passphrase" ? restoreAsPrivate : true;
      const out = await switchToVault(restored.target_dir, makePrivate);
      if (needsReading(out)) {
        // The vault is already switched; the reload waits for the user to have read this.
        setOutcome(out);
        return;
      }
      // The active vault changed underneath the whole app; reload so every view re-reads it.
      window.location.reload();
    } catch (e) {
      setError(String(e));
      setSwitching(false);
    }
  }

  if (!restored) return null;

  if (outcome) {
    const left = outcome.mods.filter((m) => m.standing !== "back");
    return (
      <div className="mt-3 max-w-sm rounded-[var(--radius-sm)] border border-border2 bg-surface p-3">
        <p className="text-sm text-ink2">Switched to the restored vault.</p>
        {outcome.warnings.length > 0 && (
          <ul className="mt-1 flex flex-col gap-1 text-xs text-st-due">
            {outcome.warnings.map((w) => (
              <li key={w} className="break-words">
                {w}
              </li>
            ))}
          </ul>
        )}
        {left.length > 0 && (
          <div className="mt-2">
            <p className="text-xs text-ink4">
              {left.length === 1 ? "One mod isn't" : `${left.length} mods aren't`} running yet:
            </p>
            <ModList mods={left} />
          </div>
        )}
        <div className="mt-2">
          <Button variant="primary" onClick={() => window.location.reload()}>
            Continue
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div className="mt-3 max-w-sm rounded-[var(--radius-sm)] border border-border2 bg-surface p-3">
      <p className="text-sm text-ink2">Restored a vault, ready to use.</p>
      <p className="mt-1 text-xs text-ink4">
        From a backup made {formatDateTime(restored.created_at)}. It&rsquo;s in a new folder; your
        current vault is still active until you switch.
      </p>
      {restored.key_mode === "passphrase" && (
        <fieldset className="mt-2 flex flex-col gap-1" disabled={switching}>
          <legend className="text-xs text-ink4">
            This backup was passphrase-protected for sharing. On this device:
          </legend>
          <label className="flex cursor-pointer items-start gap-2 text-xs text-ink3">
            <input
              type="radio"
              name="restore-privacy"
              className="mt-0.5"
              checked={restoreAsPrivate}
              onChange={() => setRestoreAsPrivate(true)}
            />
            <span>
              <span className="text-ink2">Make it private to this device</span> — recommended; your
              notes are re-encrypted with a device key and open without a passphrase.
            </span>
          </label>
          <label className="flex cursor-pointer items-start gap-2 text-xs text-ink3">
            <input
              type="radio"
              name="restore-privacy"
              className="mt-0.5"
              checked={!restoreAsPrivate}
              onChange={() => setRestoreAsPrivate(false)}
            />
            <span>
              <span className="text-ink2">Keep it passphrase-protected</span> — notes stay encrypted
              at rest; you can share the vault again later.
            </span>
          </label>
        </fieldset>
      )}
      {restored.mods.length > 0 && (
        <div className="mt-2">
          <p className="text-xs text-ink4">
            {restored.mods.length === 1
              ? "This backup held one mod:"
              : `This backup held ${restored.mods.length} mods:`}
          </p>
          <ModList mods={restored.mods} />
          <p className="mt-1 text-xs text-ink4">
            After you switch, PM says how to get back any that don&rsquo;t come back.
          </p>
        </div>
      )}
      <p className="mt-2 text-xs text-ink4">
        Developer mode comes back off, and apps you had connected need connecting again.
      </p>
      <div className="mt-2">
        <Button variant="primary" onClick={doSwitch} disabled={switching}>
          {switching ? "Switching…" : "Switch to the restored vault"}
        </Button>
      </div>
    </div>
  );
}
