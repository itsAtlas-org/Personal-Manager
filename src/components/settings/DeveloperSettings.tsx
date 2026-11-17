// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useState } from "react";
import { isDevBuild, useDevMode } from "../../lib/capabilities";
import { failureText } from "../../lib/modsTypes";
import { Button, SectionInfo, SectionLabel, SettingRow, Toggle } from "../ui";
import { DevSection, DeveloperKit } from "./DeveloperKit";
import { OFF_HINT } from "./kitWords";
import { TestMods } from "./TestMods";

/** The Developer-mode Settings tab. `devMode` is switched for THIS device by the backend, which
 *  owns it (`useDevMode`): the toggle waits for the answer, shows what the backend now holds, and
 *  when the write fails it stays where it was and says why. `onOpenDev` jumps to the Dev tab.
 *
 *  Under the switch sit the kit's rail sections (registry.ts): the API, endpoints and limits PM
 *  answers for (DeveloperKit) and "Load a test mod" (TestMods). They are always there, so the rail's
 *  links land, and say so while developer mode is off. */
export function DeveloperSettings({ onOpenDev }: { onOpenDev?: () => void }) {
  const { devMode, setDevMode } = useDevMode();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function change(on: boolean) {
    setBusy(true);
    setError(null);
    try {
      await setDevMode(on);
    } catch (e) {
      setError(failureText(e, "PM couldn't change developer mode."));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <div
        id="sec-dev-mode"
        data-settings-section
        className="mt-5 border-t border-border pt-4"
        data-help="settings-developer"
      >
        <SectionLabel>Developer mode</SectionLabel>
        <SettingRow label="Developer mode">
          {(a11y) => <Toggle {...a11y} checked={devMode} disabled={busy} onChange={change} />}
        </SettingRow>
        {error && (
          <p role="alert" className="mt-2 text-xs text-st-due">
            {error} Developer mode is still {devMode ? "on" : "off"}.
          </p>
        )}
        <div className="mt-3 flex items-center justify-between gap-3 text-xs">
          <span className="text-ink3">Signals</span>
          <span className="font-mono text-ink4">
            build: {isDevBuild ? "dev" : "release"} · runtime: {devMode ? "on" : "off"}
          </span>
        </div>
        {devMode && onOpenDev && (
          <div className="mt-3">
            <Button variant="tertiary" onClick={onOpenDev}>
              Open Dev tab →
            </Button>
          </div>
        )}
        {/* The build/runtime signals above stay put — a readout. Only the paragraphs about
          what the switch does fold. */}
        <SectionInfo title="What does developer mode do?">
          <p>
            Shows read-only inspection surfaces — a dedicated Dev tab (raw tables, row counts, the
            corrections log, system &amp; build info) plus internals shown in place — for debugging
            and watching how PM works. Those never change your data.
          </p>
          <p className="mt-2">
            It also allows unsigned mods: installing a mod file nobody has signed, and loading a
            test mod from a folder. Unsigned mods are the part that can matter, so this setting
            belongs to this computer only: a restored backup or a shared vault never turns it on
            somewhere else, and switching it here never changes another computer. Turning it off
            pauses every unsigned mod until it is turned back on, and unloads every test mod for
            good. Independent of the density preset, and off by default.
          </p>
        </SectionInfo>
      </div>

      <DeveloperKit on={devMode} />

      <DevSection
        id="sec-dev-testmods"
        helpId="settings-developer-testmods"
        title="Load a test mod"
      >
        {devMode ? <TestMods /> : <p className="text-sm text-ink3">{OFF_HINT}</p>}
      </DevSection>
    </>
  );
}
