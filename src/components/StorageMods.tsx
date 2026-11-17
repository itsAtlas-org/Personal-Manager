// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Mods half of Settings → Storage (Mods spec §1.14): what each installed mod keeps on this
// device, with Uninstall per mod and "Remove all mods".
//
// The sizes are the backend's (`StorageReport.mods`): a mod's files are a real measurement of its
// own folder, and what it saved in the database is an approximation (the bytes of the values, not the
// file's growth), so the total is drawn with a "~" whenever any of it is approximate.
//
// A mod does NOT leave through the component rows' Remove (`removeStorageComponent`): that deletes a
// download and a mod is more than one. Both buttons here run the mod runtime's own uninstall, through
// the Mods actions (uninstallActions.ts), so a mod's sessions end, its tab and folds are forgotten and
// the Uninstall dialog's "also delete what it made" choice stays the person's. "Remove all mods"
// never makes that choice for them: what the mods made stays, and the confirm says so.
//
// A mod's name is the mod's own text: it is drawn as a plain line through `modText`, never as markup.

import { useState } from "react";
import { formatBytes } from "../lib/format";
import { useModActions } from "../lib/modActions";
import { useModRows } from "../lib/modRows";
import { modSize, modsTotal } from "../lib/modStorage";
import type { StorageMod } from "../lib/types";
import { Button, Callout, ConfirmDialog, SectionInfo } from "./ui";
import { modText } from "./mods/modText";
import { removeAllMods, removeAllNote, REMOVE_ALL_KEY } from "./mods/uninstallActions";
import { UninstallDialog } from "./mods/UninstallDialog";
import { plural } from "./mods/uninstallNote";

export function StorageMods({
  mods,
  reload,
}: {
  mods: readonly StorageMod[];
  /** Read the report again, so the list shows what is really left. */
  reload: () => void;
}) {
  const { busy, errors } = useModActions();
  const { rows } = useModRows();
  const [uninstallId, setUninstallId] = useState<string | null>(null);
  const [confirmAll, setConfirmAll] = useState(false);
  const [note, setNote] = useState<{ warning: boolean; text: string } | null>(null);

  const removingAll = busy.has(REMOVE_ALL_KEY);
  const removeAllError = errors.get(REMOVE_ALL_KEY) ?? null;

  function removeAll() {
    setNote(null);
    // The list the person saw and confirmed, not whatever it has become since.
    void removeAllMods(mods).then((r) => {
      reload();
      setConfirmAll(false);
      if (r.ok) setNote(removeAllNote(r.value));
    });
  }

  return (
    <div data-help="settings-storage-mods">
      {note && (
        <Callout tone={note.warning ? "warning" : "info"} size="md" live className="mb-3">
          <p className="break-words">{note.text}</p>
          <div className="pt-1">
            <Button variant="tertiary" size="sm" onClick={() => setNote(null)}>
              Dismiss
            </Button>
          </div>
        </Callout>
      )}
      {removeAllError && !confirmAll && (
        <Callout size="md" className="mb-3">
          {removeAllError}
        </Callout>
      )}

      {mods.length === 0 ? (
        <p className="rounded-[var(--radius)] border border-border px-4 py-6 text-center text-sm text-ink4">
          No mods are installed.
        </p>
      ) : (
        <>
          <ul className="divide-y divide-border rounded-[var(--radius)] border border-border">
            {mods.map((m) => {
              const row = rows.find((r) => r.id === m.id);
              const name = modText(m.name) || m.id;
              return (
                <li
                  key={m.id}
                  id={`storage-mod-${m.id}`}
                  className="flex items-start justify-between gap-4 px-4 py-3"
                >
                  <div className="min-w-0">
                    <div className="flex flex-wrap items-center gap-x-2">
                      {/* Clipped to its own box: a name of stacked accents can't paint over its row. */}
                      <span className="inline-block max-w-full overflow-hidden break-words align-bottom text-sm text-ink2">
                        {name}
                      </span>
                      <span className="shrink-0 font-mono text-xs text-ink4">{modSize(m)}</span>
                      <span className="shrink-0 font-mono text-xs text-ink4">
                        {modText(m.version)}
                      </span>
                    </div>
                    <p className="mt-0.5 text-xs text-ink4">
                      {row?.installedFrom === "folder"
                        ? "A test mod, read from your own folder. "
                        : `Its files ${formatBytes(m.folder_bytes)}. `}
                      What it saved{m.data_bytes > 0 ? " about " : " "}
                      {formatBytes(m.data_bytes)}
                      {m.data_bytes > 0 ? " (an estimate)" : ""}.
                    </p>
                  </div>
                  <Button
                    variant="secondary"
                    className="shrink-0"
                    onClick={() => setUninstallId(m.id)}
                    disabled={removingAll || busy.has(m.id)}
                    aria-label={`Uninstall ${name}`}
                  >
                    {busy.has(m.id) ? "Uninstalling…" : "Uninstall…"}
                  </Button>
                </li>
              );
            })}
          </ul>
          <p className="mt-2 text-xs text-ink3">
            Uninstalling a mod also removes what it saved, and that can&apos;t be brought back. What
            it made in your tasks, milestones and notes stays unless you choose otherwise.
          </p>
          <div className="mt-3">
            <Button
              variant="secondary"
              onClick={() => {
                setNote(null);
                setConfirmAll(true);
              }}
              disabled={removingAll}
            >
              {removingAll ? "Removing all mods…" : "Remove all mods…"}
            </Button>
          </div>
        </>
      )}

      <SectionInfo title="Can I get a mod back?">
        <p>
          A mod from the catalogue can be installed again from the Mods tab. A mod you installed
          from a file, or a test mod from a folder, can&apos;t be downloaded again: you need the
          file or the folder. Either way, what the mod saved is gone with it.
        </p>
        <p>
          The files figure is the mod&apos;s own folder. What it saved is an estimate: the size of
          the values it stored in PM&apos;s database, which the database may hold in more room than
          that.
        </p>
      </SectionInfo>

      {uninstallId && (
        <UninstallDialog
          modId={uninstallId}
          refresh={async () => reload()}
          onDone={setNote}
          onClose={() => setUninstallId(null)}
        />
      )}

      <ConfirmDialog
        open={confirmAll}
        title="Remove all mods?"
        helpId="settings-storage-mods-remove-all"
        confirmLabel="Remove all mods"
        danger
        initialFocus="cancel"
        busy={removingAll}
        onConfirm={removeAll}
        onClose={() => setConfirmAll(false)}
      >
        <div className="space-y-2 text-sm text-ink2">
          <p>
            This uninstalls {plural(mods.length, "mod", "mods")} ({modsTotal(mods)}):
          </p>
          <ul className="max-h-40 list-disc overflow-y-auto pl-5">
            {mods.map((m) => (
              <li key={m.id} className="overflow-hidden break-words">
                {modText(m.name) || m.id}
              </li>
            ))}
          </ul>
          <p>
            Everything they saved goes with them and can&apos;t be brought back. The tasks,
            milestones and notes they made stay in PM. Mods from the catalogue can be installed
            again; a mod from a file or a folder needs its file or folder again.
          </p>
          {removeAllError && <Callout size="md">{removeAllError}</Callout>}
        </div>
      </ConfirmDialog>
    </div>
  );
}
