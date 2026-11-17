// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The install flows of the Mods tab (Mods spec §1.13), kept outside the view for the reason
// lib/modActions gives: the tab router unmounts the Mods tab on every switch, and a file that was
// being read, a mod that was being downloaded or an install that was being finished keeps going in the
// backend. Everything here runs through `runModAction`, so coming back shows "Installing…" (or the
// screen waiting for an answer, or the backend's refusal) instead of an idle button.
//
// Two steps, as the backend has them (INVARIANTS I-33): a START (`installModFile`,
// `installUnsignedFile`, `installFromCatalogue`) reads and checks and installs NOTHING, returning the
// consent screen with a single-use token; the FINISH (`installModConsented`) installs what the token
// staged. The page decides no trust: which screen a file gets (signed or the stark unsigned one) is
// the backend's call, and the page only follows its `unsigned` refusal to the other command.

import {
  devModeState,
  installFromCatalogue,
  installModConsented,
  installModFile,
  installUnsignedFile,
} from "./ipc";
import {
  addModNote,
  clearModError,
  markScreenExpired,
  runModAction,
  setModScreen,
  type ModActionResult,
  type ScreenRestart,
} from "./modActions";
import { modsErrorOf, type ConsentScreen, type InstallResult } from "./modsTypes";

/** The busy/error key of an install from a file (no mod id is known until the file is read). A
 *  catalogue install is keyed by the catalogue entry's mod id, as everything else about a mod is. */
export const FILE_KEY = "install:file";

/** What an unsigned file says while developer mode is off. It names the switch's home. */
export const DEV_MODE_OFF_TEXT =
  "This file isn't signed, and nobody has checked it. PM only takes a file like that when developer mode is on. Developer mode belongs to this computer: switch it on in Settings, under Developer, then choose the file again.";

/** A refusal in the backend's own shape, so `failureText` shows it as it is. */
function devModeOff(problem: string | null): { code: "dev-mode-off"; message: string } {
  return {
    code: "dev-mode-off",
    message: problem ? `${DEV_MODE_OFF_TEXT} (${problem})` : DEV_MODE_OFF_TEXT,
  };
}

/**
 * The consent screen for the file at `path`. A signed file gets the tiered screen; a file with no
 * signature comes back from the backend as `unsigned`, and then (and only then) developer mode is
 * asked: off says so, on asks the backend for the stark screen. The backend reads developer mode
 * itself again for both commands, so this check is for the sentence, not for safety.
 */
async function screenForFile(path: string): Promise<ConsentScreen> {
  try {
    return await installModFile(path);
  } catch (e) {
    if (modsErrorOf(e)?.code !== "unsigned") throw e;
  }
  const dev = await devModeState();
  if (!dev.on) throw devModeOff(dev.problem);
  try {
    return await installUnsignedFile(path);
  } catch (e) {
    if (modsErrorOf(e)?.code === "dev-mode-off") throw devModeOff(null);
    throw e;
  }
}

function stage(screen: ConsentScreen, restart: ScreenRestart): void {
  setModScreen(screen.id, screen, restart);
}

/** Read and check the `.pmmod` at `path` and stage its consent screen for the Mods tab to draw. */
export function startFileInstall(path: string): Promise<ModActionResult<ConsentScreen>> {
  clearModError(FILE_KEY);
  const read = () => screenForFile(path);
  return runModAction(FILE_KEY, "Reading the file", "PM couldn't read that file.", read, (screen) =>
    stage(screen, read),
  );
}

/** Download the catalogue's `modId` (checked against the list's hash) and stage its consent screen. */
export function startCatalogueInstall(modId: string): Promise<ModActionResult<ConsentScreen>> {
  clearModError(modId);
  const fetch = () => installFromCatalogue(modId);
  return runModAction(modId, "Downloading", "PM couldn't download that mod.", fetch, (screen) =>
    stage(screen, fetch),
  );
}

/** Get a fresh screen to replace an expired one. The new screen may be for another mod id (the file
 *  was swapped meanwhile): the old one goes either way. */
export function restartScreen(
  old: ConsentScreen,
  restart: ScreenRestart,
): Promise<ModActionResult<ConsentScreen>> {
  clearModError(old.id);
  return runModAction(old.id, "Starting again", "PM couldn't start again.", restart, (fresh) => {
    if (fresh.id !== old.id) setModScreen(old.id, null);
    stage(fresh, restart);
  });
}

export type InstallKind = ConsentScreen["kind"];

/** What an install is called while it runs, by kind. */
export const DOING: Record<InstallKind, string> = {
  update: "Updating",
  repair: "Putting back",
  fresh: "Installing",
};

/**
 * Finish the install a screen staged. `decisions` holds an answer for every permission the screen
 * marked `answerRequired` (the backend refuses a screen that leaves one out); `confirmId` is what the
 * person typed for a stark screen (the backend checks it). A `consent-expired` answer marks the screen
 * out of date, so its dialog offers to start again instead of a button that can't work. `name` is the
 * mod's name already made a plain line (it is the mod's own text, and this one lands in a note).
 */
export function finishInstall(
  screen: ConsentScreen,
  name: string,
  decisions: Record<string, boolean>,
  confirmId?: string,
): Promise<ModActionResult<InstallResult>> {
  const kind: InstallKind = screen.kind in DOING ? screen.kind : "fresh";
  return runModAction(
    screen.id,
    DOING[kind],
    "PM couldn't finish that.",
    async () => {
      try {
        return await installModConsented(screen.token, decisions, confirmId);
      } catch (e) {
        if (modsErrorOf(e)?.code === "consent-expired") markScreenExpired(screen.id);
        throw e;
      }
    },
    (done) => {
      setModScreen(screen.id, null);
      if (!done.grantsSaved) {
        addModNote(
          "warning",
          `${name} was ${kind === "update" ? "updated" : "installed"}, but PM couldn't save your answers: ${done.grantsProblem?.message ?? "no reason given"} Everything it asked for is off until you turn it on in the Mods tab.`,
        );
      }
    },
  );
}
