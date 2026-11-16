// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What a mod's own tab (`mod:<id>`) shows (Mods spec §1.17, D17). The frame is for a mod that can
// run; every other standing gets a screen PM draws, with its own plain sentence and the one thing
// that brings the mod back:
//
//   running            the sealed frame (ModFrame)
//   waiting for you    the first-open screen: the Mods disclaimer and the mod's permissions; the
//                      frame starts only after "Turn on" (ModFirstOpen)
//   installing         a placeholder; the tab is in the sidebar from the row, so it survives tab
//                      switches (a view unmounts on every switch, the row doesn't)
//   paused / parked    the backend's sentence, and Resume or Repair where it offers one; a mod PM
//                      paused because developer mode is off points at Settings, where that switch is
//   removed / missing  the safety list removed it, or there is no such mod: a way to the Mods tab
//
// The words come from the backend (`state.text`) and are drawn as plain text, as is everything else
// the mod supplied. The page never decides a standing: it reads the row, and the row changes when
// `mods://changed` says so (ModRowsProvider reads it again).
//
// THE FRAME'S LIFE. It is mounted only while the row says the mod runs, so a pause, an uninstall or the
// safety list's stop swaps it for a panel and the unmount ends its session. A tab switch unmounts it
// too (views unmount on every switch). The backend's `session.closed` ends the session on its side; a
// `replaced` (an update or repair) reopens the frame, any other reason drops it for a panel that says so,
// with a button to open it again. A store that closes under a mounted frame sends no goodbye (A's rule:
// a close says nothing), so the page clears the frame itself: the lock, a curtain and a vault switch
// replace this whole tree, and the vault-fault notice (the one close that leaves the page standing)
// unmounts it here until the store is read again.

import { useCallback, useEffect, useState, type ReactNode } from "react";
import { onModsChanged, onVaultFault, repairMod, resumeMod } from "../../lib/ipc";
import { useModRows } from "../../lib/modRows";
import { failureText, type ModInfo } from "../../lib/modsTypes";
import { subscribeUntilCleanup } from "../../lib/subscribe";
import { Button, Callout } from "../ui";
import { ModFirstOpen } from "./ModFirstOpen";
import { ModFrame } from "./ModFrame";

interface ModTabProps {
  /** The mod's id (the part of the view after `mod:`). */
  modId: string;
  /** Go to the Mods tab (the panels point there for what they can't do themselves). */
  onOpenMods: () => void;
  /** Open Settings (developer mode's switch lives there). */
  onOpenSettings: () => void;
}

export function ModTab({ modId, onOpenMods, onOpenSettings }: ModTabProps) {
  const { rows, loaded, problem, refresh } = useModRows();
  const mod = rows.find((r) => r.id === modId);

  // The vault fault closes the store with this page still standing. It clears when the store is read
  // again (`loaded` after a re-open, `recovered` after a restore).
  const [storeLost, setStoreLost] = useState(false);
  useEffect(() => {
    const stopFault = subscribeUntilCleanup(() => onVaultFault(() => setStoreLost(true)));
    const stopChanged = subscribeUntilCleanup(() =>
      onModsChanged((e) => {
        if (e.cause === "loaded" || e.cause === "recovered") setStoreLost(false);
      }),
    );
    return () => {
      stopFault();
      stopChanged();
    };
  }, []);

  if (storeLost) {
    return (
      <Panel title={mod?.name ?? "Mod"}>
        <p className="text-sm text-ink2">
          PM can&apos;t reach your vault right now, so this mod was stopped. It comes back when PM
          can reach the vault again.
        </p>
      </Panel>
    );
  }

  if (!loaded) {
    return (
      <Panel title="Mod">
        {problem ? (
          <>
            <Callout size="md">PM couldn&apos;t read the list of mods: {problem}</Callout>
            <Actions>
              <Button variant="secondary" onClick={() => void refresh()}>
                Try again
              </Button>
            </Actions>
          </>
        ) : (
          <p className="text-sm text-ink3">Loading…</p>
        )}
      </Panel>
    );
  }

  if (!mod) {
    return (
      <Panel title="This mod isn't installed">
        <p className="text-sm text-ink2">
          It was removed, or PM can&apos;t find it. Its tab goes away by itself.
        </p>
        <Actions>
          <Button variant="secondary" onClick={onOpenMods}>
            Open the Mods tab
          </Button>
        </Actions>
      </Panel>
    );
  }

  if (mod.state.kind === "waiting-for-you") {
    return <ModFirstOpen mod={mod} onTurnedOn={refresh} />;
  }

  if (mod.state.kind === "running" && mod.state.running) {
    return <RunningMod mod={mod} refresh={refresh} onOpenMods={onOpenMods} />;
  }

  return (
    <StatusPanel
      mod={mod}
      refresh={refresh}
      onOpenMods={onOpenMods}
      onOpenSettings={onOpenSettings}
    />
  );
}

// --- the frame, and what replaces it --------------------------------------------------------------

function RunningMod({
  mod,
  refresh,
  onOpenMods,
}: {
  mod: ModInfo;
  refresh: () => Promise<void>;
  onOpenMods: () => void;
}) {
  // Bumped to open a fresh frame (and so a fresh session): after a `replaced`, or "Open it again".
  const [opening, setOpening] = useState(0);
  const [ended, setEnded] = useState<string | null>(null);

  const onClosed = useCallback(
    (reason: string | null) => {
      if (reason === "replaced") setOpening((n) => n + 1);
      else setEnded(reason ?? "other");
      // The row says why (a pause, a removal); the panel below only covers the moment before it
      // is read, and a session that ended for no reason the row shows.
      void refresh();
    },
    [refresh],
  );

  if (ended !== null) {
    return (
      <Panel title={mod.name}>
        <p className="text-sm text-ink2">{ENDED[ended] ?? ENDED.other}</p>
        <Actions>
          {ended !== "paused" && ended !== "uninstalled" && (
            <Button
              variant="primary"
              onClick={() => {
                setEnded(null);
                setOpening((n) => n + 1);
              }}
            >
              Open it again
            </Button>
          )}
          <Button variant="secondary" onClick={onOpenMods}>
            Open the Mods tab
          </Button>
        </Actions>
      </Panel>
    );
  }

  return (
    <ModFrame key={opening} modId={mod.id} name={mod.name} tier={mod.tier} onClosed={onClosed} />
  );
}

const ENDED: Record<string, string> = {
  paused: "This mod was paused.",
  uninstalled: "This mod was removed.",
  other: "This mod's session ended.",
};

// --- a mod that can't run ------------------------------------------------------------------------

function StatusPanel({
  mod,
  refresh,
  onOpenMods,
  onOpenSettings,
}: {
  mod: ModInfo;
  refresh: () => Promise<void>;
  onOpenMods: () => void;
  onOpenSettings: () => void;
}) {
  const { kind, fix, text } = mod.state;
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function run(action: () => Promise<unknown>, fallback: string) {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (e) {
      setError(failureText(e, fallback));
    }
    // Read the rows whether or not it worked: a refusal ("the safety list still names it") is also
    // a change in what the row says.
    await refresh();
    setBusy(false);
  }

  const installing = kind === "installing";
  return (
    <Panel title={installing ? `Installing ${mod.name}` : mod.name}>
      <p className="break-words text-sm text-ink2">
        {text ||
          (installing ? "Installing. This tab stays here until it is ready." : "It isn't running.")}
      </p>
      {kind === "paused-dev-mode" && (
        <p className="text-sm text-ink3">
          You can switch developer mode on in Settings, under Developer.
        </p>
      )}
      {error && <Callout size="md">{error}</Callout>}
      <Actions>
        {fix === "resume" && (
          <Button
            variant="primary"
            disabled={busy}
            onClick={() => void run(() => resumeMod(mod.id), "PM couldn't resume this mod.")}
          >
            Resume
          </Button>
        )}
        {fix === "repair" && (
          <Button
            variant="primary"
            disabled={busy}
            onClick={() => void run(() => repairMod(mod.id), "PM couldn't repair this mod.")}
          >
            Repair
          </Button>
        )}
        {kind === "paused-dev-mode" && (
          <Button variant="primary" onClick={onOpenSettings}>
            Open Settings
          </Button>
        )}
        {!installing && (
          <Button
            variant={
              fix === "resume" || fix === "repair" || kind === "paused-dev-mode"
                ? "secondary"
                : "primary"
            }
            onClick={onOpenMods}
          >
            Open the Mods tab
          </Button>
        )}
      </Actions>
    </Panel>
  );
}

// --- the shell ----------------------------------------------------------------------------------

/** One panel PM draws where a mod's frame would be. `title` may be a mod's own name: plain text. */
function Panel({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="h-full overflow-y-auto" data-help="mod-tab-status">
      <div className="mx-auto max-w-xl space-y-4 px-6 py-8">
        <h1 className="break-words font-head text-lg font-semibold text-ink">{title}</h1>
        {children}
      </div>
    </div>
  );
}

function Actions({ children }: { children: ReactNode }) {
  return <div className="flex flex-wrap items-center gap-2">{children}</div>;
}
