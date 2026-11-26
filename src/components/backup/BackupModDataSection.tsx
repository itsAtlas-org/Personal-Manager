// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Also save mod data in my backups" (Mods spec §1.11). A mod's own saved data (what it keeps in PM,
// a board or notes, say) rides in every backup unless this is off. It is one switch, applied the
// moment it flips, and what turning it off costs is part of the control, so that sentence is inline
// and never folded (the info-vs-controls doctrine: an irreversible-loss warning is not commentary).
//
// What no setting can change stays out of this switch on purpose: a mod's passwords and API keys
// and any connected app never go into a backup or an export, so there is nothing to offer.

import { useEffect, useState } from "react";

import { getBackupModData, saveBackupModData } from "../../lib/ipc";
import { Callout, SectionLabel, SettingRow, Toggle } from "../ui";

export interface BackupModDataSectionProps {
  /** The panel-wide "any op in flight" gate. A run reads the switch when its snapshot is taken, so
   *  flipping it mid-run would say one thing and do another for that run. */
  busy: boolean;
}

export function BackupModDataSection({ busy }: BackupModDataSectionProps) {
  // `null` until the stored value is read: the switch isn't drawn on a guess.
  const [keep, setKeep] = useState<boolean | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    getBackupModData()
      .then((on) => {
        if (!cancelled) setKeep(on);
      })
      .catch((e) => {
        if (!cancelled) setError(`Couldn't read this setting: ${String(e)}`);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function change(next: boolean) {
    setSaving(true);
    setError(null);
    try {
      await saveBackupModData(next);
      setKeep(next);
      // The Settings footer's "Saved" acknowledgement listens for this (a `set_*` command would
      // announce on its own, but this command isn't named that way).
      window.dispatchEvent(new Event("pm:settings-changed"));
    } catch (e) {
      // The switch stays where it was: it only moves once the store says so.
      setError(`Couldn't save this setting: ${String(e)}`);
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="mt-6" data-help="settings-backup-mod-data">
      <SectionLabel>Mods in your backups</SectionLabel>
      {keep === null ? (
        !error && <p className="mt-2 text-xs text-ink4">Loading&hellip;</p>
      ) : (
        <div className="max-w-sm">
          <SettingRow
            spacing="row"
            label="Also save mod data in my backups"
            description={
              keep
                ? "What mods have saved in PM (a board or notes they keep, say) goes into every backup. Their passwords and API keys never do."
                : "Backups leave out what mods have saved in PM. Their passwords and API keys are never saved, either way."
            }
          >
            {(a11y) => (
              <Toggle
                {...a11y}
                checked={keep}
                onChange={(n) => void change(n)}
                disabled={saving || busy}
              />
            )}
          </SettingRow>
          {!keep && (
            <Callout tone="warning" live={false} className="mt-2">
              A restore then brings those mods back empty: whatever they saved is not in your
              backups. The one exception is the Pinboard: if this is off, a backup holds only the
              board as it was when the Pinboard moved into a mod.
            </Callout>
          )}
        </div>
      )}
      {error && (
        <p className="mt-2 break-words text-xs text-st-due" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
