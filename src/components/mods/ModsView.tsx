// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Mods tab (Mods spec §1.13). The page header, the safety list's strips, and the three sections
// the spec names, in order:
//
//   Installed         one row per mod: its standing, permissions, update waiting, pause, uninstall
//   Available         the signed catalogue: each entry's tier, maker and what it asks for, Install
//   Install from file the one home of file installs (an unsigned one needs developer mode)
//
// Installing is two steps: Install / Choose a file reads and checks and returns a consent screen
// (lib/modInstall), staged outside the view; ConsentDialog draws it (the signed screen, the stark
// unsigned one, or "ran out of time") and its button finishes the install.
//
// Nothing the tab offers shows before the Mods disclaimer is accepted on this device in this vault
// (ModsDisclaimerGate), except the safety strips, which are about mods already installed.
//
// The installed rows are `useModRows()` (lib/modRows): the same read the sidebar's mod tabs use, so
// the two never disagree about what is installed, and it is read again on every `mods://changed`.
// What is in flight (an uninstall, a pause, a staged update screen) is kept outside this view
// (lib/modActions), because the tab router unmounts the view on every switch and the backend's work
// doesn't stop when it does.

import { useState } from "react";
import { useModRows } from "../../lib/modRows";
import { dismissModNote, useModActions } from "../../lib/modActions";
import { isFolderScreen } from "../../lib/modInstall";
import { useCatalogue } from "../../lib/useCatalogue";
import { Button, Callout, Modal } from "../ui";
import { AvailableMods } from "./AvailableMods";
import { ConsentDialog } from "./ConsentDialog";
import { InstallFromFile } from "./InstallFromFile";
import { ModFirstOpen } from "./ModFirstOpen";
import { ModRow } from "./ModRow";
import { ModsDisclaimerGate } from "./ModsDisclaimerGate";
import { ModsStrips } from "./ModsStrips";
import { UninstallDialog } from "./UninstallDialog";

const TURN_ON_TITLE = "mods-turn-on-title";

export function ModsView() {
  const { rows: read, loaded, problem, refresh } = useModRows();
  // A mod PM is still installing has a tab (and says "Installing" there) but is no row: nothing in
  // this list could be paused, repaired or uninstalled for it.
  const rows = read.filter((r) => !r.pending);
  const { catalogue, problem: catalogueProblem, refresh: refreshCatalogue } = useCatalogue();
  const { notes, screens, busy } = useModActions();
  const [uninstallId, setUninstallId] = useState<string | null>(null);
  const [turnOnId, setTurnOnId] = useState<string | null>(null);

  const turnOn = rows.find((r) => r.id === turnOnId && r.state.kind === "waiting-for-you");
  // While "Turning on" is in flight the dialog stays: closing it would hide a refusal that arrives
  // after (the row would just go on saying "Waiting for you" with nothing to say why).
  const turningOn = turnOn !== undefined && busy.has(turnOn.id);
  const closeTurnOn = turningOn ? () => {} : () => setTurnOnId(null);
  // A test folder's screen is Settings → Developer's (lib/modInstall `isFolderScreen`).
  const staged = [...screens.values()].find((s) => !isFolderScreen(s));
  // A dialog for a mod says its refusals itself, so its row doesn't repeat them.
  const dialogFor = uninstallId ?? staged?.id ?? turnOn?.id ?? null;

  return (
    <div className="flex h-full flex-col">
      <header className="border-b border-border px-6 py-3">
        <h1 className="font-head text-sm font-semibold text-ink">Mods</h1>
        <p className="text-xs text-ink3">
          {!loaded
            ? (problem ?? "Loading…")
            : rows.length === 0
              ? "No mods installed"
              : `${rows.length} mod${rows.length === 1 ? "" : "s"} installed`}
        </p>
      </header>
      <div className="flex-1 overflow-y-auto" data-help="mods-view">
        <div className="mx-auto max-w-3xl space-y-8 px-6 py-6">
          {notes.length > 0 && (
            <div className="space-y-2">
              {notes.map((n) => (
                <Callout key={n.id} tone={n.tone} size="md" live>
                  <p className="break-words">{n.text}</p>
                  <div className="pt-1">
                    <Button variant="tertiary" size="sm" onClick={() => dismissModNote(n.id)}>
                      Dismiss
                    </Button>
                  </div>
                </Callout>
              ))}
            </div>
          )}
          <ModsStrips />
          <ModsDisclaimerGate>
            <section
              aria-labelledby="mods-installed"
              data-mods-section="installed"
              data-help="mods-installed"
              className="space-y-3"
            >
              <h2 id="mods-installed" className="font-head text-sm font-semibold text-ink">
                Installed
              </h2>
              {!loaded ? (
                problem ? (
                  <Callout size="md">
                    <p>PM couldn&apos;t read the list of mods: {problem}</p>
                    <div className="pt-1">
                      <Button variant="secondary" size="sm" onClick={() => void refresh()}>
                        Try again
                      </Button>
                    </div>
                  </Callout>
                ) : (
                  <p className="text-sm text-ink3">Loading…</p>
                )
              ) : rows.length === 0 ? (
                <p className="text-sm text-ink3">No mods are installed.</p>
              ) : (
                <ul className="space-y-3">
                  {rows.map((mod) => (
                    <ModRow
                      key={mod.id}
                      mod={mod}
                      refresh={refresh}
                      onUninstall={setUninstallId}
                      onTurnOn={setTurnOnId}
                      quiet={dialogFor === mod.id}
                    />
                  ))}
                </ul>
              )}
            </section>
            <section
              aria-labelledby="mods-available"
              data-mods-section="available"
              data-help="mods-available"
              className="space-y-3"
            >
              <h2 id="mods-available" className="font-head text-sm font-semibold text-ink">
                Available
              </h2>
              <p className="text-xs text-ink3">
                Mods published through Atlas. PM checks each download is the one the catalogue lists
                before it shows you what it asks to do.
              </p>
              <AvailableMods
                catalogue={catalogue}
                problem={catalogueProblem}
                retry={() => void refreshCatalogue()}
                dialogFor={dialogFor}
              />
            </section>
            <section
              aria-labelledby="mods-from-file"
              data-mods-section="from-file"
              data-help="mods-from-file"
              className="space-y-3"
            >
              <h2 id="mods-from-file" className="font-head text-sm font-semibold text-ink">
                Install from file
              </h2>
              <InstallFromFile />
            </section>
          </ModsDisclaimerGate>
        </div>
      </div>

      {uninstallId && (
        <UninstallDialog
          modId={uninstallId}
          refresh={refresh}
          onClose={() => setUninstallId(null)}
        />
      )}
      {staged && <ConsentDialog key={staged.token} screen={staged} refresh={refresh} />}
      {turnOn && (
        <Modal open onClose={closeTurnOn} labelledBy={TURN_ON_TITLE} widthClassName="max-w-2xl">
          <div className="space-y-4 p-5">
            <ModFirstOpen
              mod={turnOn}
              titleId={TURN_ON_TITLE}
              onTurnedOn={async () => {
                await refresh();
                setTurnOnId(null);
              }}
            />
            <div className="flex justify-end">
              <Button variant="tertiary" onClick={closeTurnOn} disabled={turningOn}>
                Not now
              </Button>
            </div>
          </div>
        </Modal>
      )}
    </div>
  );
}
