// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Load a test mod" in Settings → Developer (Mods spec §1.15): the developer's own unpacked folder,
// loaded where it is (PM never copies or changes it) for as long as PM runs. It is shown only while
// developer mode is on for this device; the backend checks that itself, before it reads the folder.
//
// Loading is the same two steps as every install: choosing a folder reads and checks it and loads
// NOTHING, and what comes back is the stark screen of a mod nobody has checked (the Mods tab's own
// UnsignedConsentDialog, drawn through ConsentDialog and showing which folder). Its button is the load,
// and it asks the person to type the mod's id first. The screen is staged outside the view
// (lib/modActions) like the Mods tab's, and drawn HERE rather than in the Mods tab: the tab leaves a
// test folder's screen out (lib/modInstall `isFolderScreen`), so it never shows twice under Settings.
//
// Below it, each loaded folder with what PM knows: the mod's name and standing from the shared rows,
// why the last reload didn't take (the old files keep running), whether PM is watching the folder for
// saves, and Reload and Unload. All of what a mod or a folder supplies (its id, its path, a problem
// sentence that may quote the manifest) is drawn as one plain line (`modText`).

import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import { reloadTestMod, unloadTestMod } from "../../lib/ipc";
import { clearModError, runModAction, useModActions } from "../../lib/modActions";
import { modFoldKey, pruneAfterUninstall } from "../../lib/modFolds";
import { FOLDER_KEY, isFolderScreen, startFolderLoad } from "../../lib/modInstall";
import { useModRows } from "../../lib/modRows";
import { useTestMods } from "../../lib/useTestMods";
import { ConsentDialog } from "../mods/ConsentDialog";
import { modText } from "../mods/modText";
import { Button, Callout, SectionInfo } from "../ui";

/** The busy/error key of one loaded folder's Reload or Unload. A `:` can't be in a mod id, so it never
 *  meets a mod's own key. */
const testKey = (id: string) => `test:${id}`;

export function TestMods() {
  const { tests, problem, refresh } = useTestMods();
  const { rows } = useModRows();
  const { busy, errors, screens } = useModActions();
  const [pickError, setPickError] = useState<string | null>(null);
  // What the last Reload of each folder said, until it is pressed again.
  const [said, setSaid] = useState<ReadonlyMap<string, string>>(new Map());

  const staged = [...screens.values()].find(isFolderScreen);
  const reading = busy.get(FOLDER_KEY) ?? null;
  const folderError = pickError ?? errors.get(FOLDER_KEY) ?? null;

  async function choose() {
    setPickError(null);
    let picked: string | string[] | null;
    try {
      picked = await open({ directory: true, multiple: false });
    } catch {
      setPickError("PM couldn't open the folder picker.");
      return;
    }
    if (typeof picked === "string") void startFolderLoad(picked);
  }

  function remember(id: string, text: string | null) {
    setSaid((prev) => {
      const next = new Map(prev);
      if (text === null) next.delete(id);
      else next.set(id, text);
      return next;
    });
  }

  function reload(id: string) {
    clearModError(testKey(id));
    remember(id, null);
    void runModAction(testKey(id), "Reloading", "PM couldn't reload it.", () =>
      reloadTestMod(id),
    ).then((r) => {
      // The reason a reload didn't take is on the list's entry, so read it again either way.
      void refresh();
      if (r.ok) {
        remember(
          id,
          r.value.changed
            ? `Reloaded: it is now version ${modText(r.value.version)}.`
            : "Nothing had changed, so nothing restarted.",
        );
      }
    });
  }

  function unload(id: string) {
    clearModError(testKey(id));
    remember(id, null);
    void runModAction(testKey(id), "Unloading", "PM couldn't unload it.", () =>
      unloadTestMod(id),
    ).then((r) => {
      // Its tab placement and folds on this device go with it, as they do for an uninstall.
      if (r.ok) pruneAfterUninstall(id, { tab: `mod:${id}`, foldPrefix: modFoldKey(id) });
      void refresh();
    });
  }

  return (
    <>
      <p className="text-sm text-ink2">
        Try a mod from an unpacked folder. PM reads the folder where it is and never copies or
        changes it. A packed <code className="font-mono">.pmmod</code> file installs from the Mods
        tab.
      </p>
      <div>
        <Button variant="secondary" onClick={() => void choose()} disabled={reading !== null}>
          {reading ? `${reading}…` : "Choose a folder…"}
        </Button>
      </div>
      {folderError && <Callout size="md">{folderError}</Callout>}

      {problem && !tests && <Callout size="md">{problem}</Callout>}
      {tests && tests.length === 0 && <p className="text-xs text-ink3">No test mods are loaded.</p>}
      {tests && tests.length > 0 && (
        <ul aria-label="Test mods" className="space-y-2">
          {tests.map((t) => {
            const row = rows.find((r) => r.id === t.id);
            const working = busy.get(testKey(t.id)) ?? null;
            const error = errors.get(testKey(t.id)) ?? null;
            const note = said.get(t.id) ?? null;
            const id = modText(t.id);
            return (
              <li
                key={t.id}
                className="space-y-1 rounded-[var(--radius)] border border-border px-4 py-3"
              >
                <div className="flex flex-wrap items-start justify-between gap-3">
                  <div className="min-w-0">
                    <p className="break-words text-sm text-ink2">
                      {row ? modText(row.name) : id}{" "}
                      <span className="font-mono text-xs text-ink4">{id}</span>{" "}
                      <span className="rounded-full border border-border2 px-1.5 py-px text-xs text-ink2">
                        Test
                      </span>
                    </p>
                    {row && (
                      <p className="break-words text-xs text-ink3">{modText(row.state.text)}</p>
                    )}
                    <p className="break-all text-xs text-ink4">{modText(t.folder)}</p>
                    <p className="text-xs text-ink3">
                      {t.watching
                        ? "Watching the folder: saving a file reloads the mod."
                        : "Not watching the folder: press Reload after you save a change."}
                    </p>
                  </div>
                  <div className="flex shrink-0 gap-2">
                    <Button
                      variant="secondary"
                      onClick={() => reload(t.id)}
                      disabled={working !== null}
                      aria-label={`Reload ${id}`}
                    >
                      {working === "Reloading" ? "Reloading…" : "Reload"}
                    </Button>
                    <Button
                      variant="secondary"
                      onClick={() => unload(t.id)}
                      disabled={working !== null}
                      aria-label={`Unload ${id}`}
                    >
                      {working === "Unloading" ? "Unloading…" : "Unload"}
                    </Button>
                  </div>
                </div>
                {t.problem && (
                  <Callout tone="warning" size="sm" live={false}>
                    The last reload didn&apos;t take, so the old version keeps running:{" "}
                    {modText(t.problem)}
                  </Callout>
                )}
                {error && <Callout size="sm">{error}</Callout>}
                {note && !error && <p className="text-xs text-ink3">{note}</p>}
              </li>
            );
          })}
        </ul>
      )}

      <SectionInfo title="How does a test mod work?">
        <p>
          A test mod is read from your folder and runs like any mod nobody has checked, so you
          choose what it may do and type its id before it loads. It lasts until PM restarts or
          developer mode goes off, and what it stores goes then too. It is left out of backups.
        </p>
        <p>
          When PM is watching the folder, saving a file reloads the mod. A change that asks for more
          than you allowed, or a different id, maker or repository, doesn&apos;t reload: the old
          version keeps running and the reason shows here. Load the folder again to see the new
          screen. Unload ends it now; your folder is never touched.
        </p>
      </SectionInfo>

      {staged && <ConsentDialog key={staged.token} screen={staged} refresh={refresh} />}
    </>
  );
}
