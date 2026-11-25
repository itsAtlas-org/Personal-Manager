// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Starting the page: connect to PM, read what the page needs before it draws (the theme, and the three
// preferences), then draw the Pinboard. main.tsx passes the real connection; a test passes stand-ins.
// What the frame can say before a board exists is said here, in plain words.

import { createRoot, type Root } from "react-dom/client";
import { HostError, type ModHost } from "../../../../src/kit/host";
import { getPref, setPref, connectPm, CODE, PmError } from "../lib/pm";
import { PREFS_KEY, startPrefs, type Prefs } from "../lib/pinboard/prefs";
import { App } from "./App";
import { startCspStyles } from "./cspStyles";
import { frameTaskData } from "./taskData";
import { frameThemeStore } from "./theme";

export const STARTING = "Starting the Pinboard…";
export const NOT_FRAMED =
  "The Pinboard runs inside PM. Open it from PM's Mods tab; it doesn't open on its own.";
export const NO_ANSWER = "PM didn't answer. Close this tab and open the Pinboard again.";

export interface BootEnv {
  doc: Document;
  win: Window & typeof globalThis;
  /** PM's end of the bridge (the SDK's `connectHost`). */
  connect(): Promise<ModHost>;
}

export interface Running {
  root: Root;
  host: ModHost;
}

/** The three preferences' saving: one write at a time, the whole key, over the revision it was read at.
 *  A stale write (PM changed the key since) is read again and tried once more; any other failure just
 *  means the choice is not remembered. */
function prefsSaver(): (prefs: Prefs) => void {
  let chain: Promise<unknown> = Promise.resolve();
  return (prefs) => {
    const json = JSON.stringify(prefs);
    chain = chain.then(async () => {
      try {
        await setPref(PREFS_KEY, json);
      } catch (error) {
        if (!(error instanceof PmError) || error.code !== CODE.staleRevision) return;
        try {
          await getPref(PREFS_KEY);
          await setPref(PREFS_KEY, json);
        } catch {
          /* not remembered this time */
        }
      }
    });
  };
}

/** Make the connection and draw the Pinboard, or say why it can't be drawn (null). */
export async function boot(env: BootEnv): Promise<Running | null> {
  const { doc, win } = env;
  // Before anything can mount an editor: CodeMirror writes its base theme into a <style> element, which
  // the frame's policy refuses, so the page keeps each such element's rules in an adopted sheet.
  startCspStyles(doc);
  const mount = doc.getElementById("root") ?? doc.body;
  const say = (text: string) => {
    const p = doc.createElement("p");
    p.className = "p-4 text-ink3";
    p.textContent = text;
    mount.replaceChildren(p);
  };

  say(STARTING);
  let host: ModHost;
  try {
    host = await env.connect();
  } catch (error) {
    say(error instanceof HostError && error.kind === "not-framed" ? NOT_FRAMED : NO_ANSWER);
    return null;
  }
  connectPm(host);
  const theme = frameThemeStore(host);
  const tasks = frameTaskData(host, win);
  // The preferences before the first draw: the view reads them synchronously, as it read `localStorage`.
  // A key that cannot be read leaves the defaults (the safe ones: keep asking, show everything).
  let stored: unknown = null;
  try {
    const raw = await getPref(PREFS_KEY);
    if (raw !== null) stored = JSON.parse(raw) as unknown;
  } catch {
    /* the defaults */
  }
  startPrefs(stored, prefsSaver());
  await theme.ready;
  mount.replaceChildren();
  const root = createRoot(mount);
  root.render(<App theme={theme} tasks={tasks} />);
  // A tab switch removes the frame.
  win.addEventListener("pagehide", () => root.unmount(), { once: true });
  return { root, host };
}
