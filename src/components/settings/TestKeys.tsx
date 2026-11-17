// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Test keys" in Settings → Developer (Mods spec §1.15): the keys of the test mods loaded from a
// folder, typed into PM's own field so a mod being built can try `net.fetch` with a real key. It is the
// SAME component the Installed row uses (ModSecretFields; one control, one home for a key field), over
// the same three commands, and it lists only the loaded test mods (the shared rows whose source is a
// folder). A test mod is a `mods` row like any other, so its key is stored like any mod's: in PM's
// encrypted store, bound to the host and header its manifest declares, and gone when the test mod is
// unloaded, when PM restarts or when developer mode goes off.
//
// Shown only while developer mode is on for this device (the parent says so): with it off every test
// mod is unloaded, so there is nothing to type a key for.

import { useModRows } from "../../lib/modRows";
import { ModSecretFields } from "../mods/ModSecretFields";
import { modText } from "../mods/modText";
import { SectionInfo } from "../ui";

export function TestKeys() {
  const { rows, loaded, refresh } = useModRows();
  const tests = rows.filter((r) => r.installedFrom === "folder");

  return (
    <>
      <p className="text-sm text-ink2">
        Type the keys a test mod needs here. A key goes to PM, not to the mod, and PM adds it to the
        mod&apos;s request itself.
      </p>
      {!loaded && <p className="text-xs text-ink3">Reading…</p>}
      {loaded && tests.length === 0 && (
        <p className="text-xs text-ink3">No test mods are loaded.</p>
      )}
      {tests.length > 0 && (
        <ul aria-label="Test mod keys" className="space-y-2">
          {tests.map((t) => (
            <li
              key={t.id}
              className="space-y-2 rounded-[var(--radius)] border border-border px-4 py-3"
            >
              <p className="break-words text-sm text-ink2">
                {modText(t.name)}{" "}
                <span className="font-mono text-xs text-ink4">{modText(t.id)}</span>
              </p>
              {t.secrets.length === 0 ? (
                <p className="text-xs text-ink3">It doesn&apos;t declare any keys.</p>
              ) : (
                <ModSecretFields modId={t.id} slots={t.secrets} onChanged={() => void refresh()} />
              )}
            </li>
          ))}
        </ul>
      )}
      <SectionInfo title="How are test keys kept?">
        <p>
          A test mod&apos;s key is kept like any mod&apos;s: in PM&apos;s encrypted store on this
          computer, tied to the one site and header its manifest names, and never shown to the mod
          or to this page again. It goes when the test mod is unloaded, when PM restarts or when
          developer mode is turned off, with everything else the test mod saved. A key is left out
          of backups.
        </p>
      </SectionInfo>
    </>
  );
}
