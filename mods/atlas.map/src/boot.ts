// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Starting the page: connect to PM, then build the Map. main.ts passes the real connection and the real
// worker; a test passes stand-ins. Everything the frame can say before a map exists is said here, in
// the Map's own words (copy.ts).

import { HostError } from "../../../src/kit/host";
import { mountMap, type MapApp } from "./app";
import { workerCompute, type WorkerLike } from "./compute";
import { NO_ANSWER, NOT_FRAMED, STARTING } from "./copy";
import type { MapHost } from "./hostApi";

export interface BootEnv {
  doc: Document;
  win: Window & typeof globalThis;
  /** PM's end of the bridge (the SDK's `connectHost`). */
  connect(): Promise<MapHost>;
  /** A worker running the layout code (the SDK's `spawnWorker` over the worker's source). */
  spawn(): WorkerLike;
}

/** The running Map, or null when there is no PM to talk to (the page says why). */
export async function boot(env: BootEnv): Promise<MapApp | null> {
  const { doc, win } = env;
  const mount = doc.getElementById("app") ?? doc.body;
  const say = (text: string) => {
    const p = doc.createElement("p");
    p.className = "plain";
    p.textContent = text;
    mount.replaceChildren(p);
  };

  say(STARTING);
  let host: MapHost;
  try {
    host = await env.connect();
  } catch (error) {
    say(error instanceof HostError && error.kind === "not-framed" ? NOT_FRAMED : NO_ANSWER);
    return null;
  }
  mount.replaceChildren();
  const app = mountMap({
    doc,
    win,
    mount,
    host,
    compute: workerCompute(env.spawn),
    themeRoot: doc.documentElement,
  });
  // A tab switch removes the frame; stop the worker and the listeners with it.
  win.addEventListener("pagehide", () => app.destroy(), { once: true });
  return app;
}
