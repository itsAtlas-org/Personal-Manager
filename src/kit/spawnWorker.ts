// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Starting a mod's Worker from the source text the build put in the page (mods/vite.config.ts turns
// `import source from "inline-worker:./w.ts"` into one string literal).
//
// Why a blob: the frame cannot read a worker file's text (its CSP is `connect-src 'none'`), and its
// `worker-src` is `blob:` (measured in the B0 spikes, build log "PR B B0"). So the way to run a
// Worker in a sealed frame is a Blob URL made from text the page already holds. This is the ONE
// place a mod makes a Blob for a Worker, so
// the shape is reviewed once: the argument is the worker's source, never anything a document or PM
// sent (a mod that fed it such text would be running someone else's code, in its own frame).
//
// The URL is revoked when the Worker is terminated, not when it starts: the engine fetches the URL
// after `new Worker` returns, and revoking sooner is a race that some engines lose.

/** A Worker running `source` (classic script, as the nested build emits it). Terminating it also
 *  frees its blob URL. Throws what `new Worker` throws; the URL does not outlive a failed start. */
export function spawnWorker(source: string): Worker {
  const url = URL.createObjectURL(new Blob([source], { type: "text/javascript" }));
  let worker: Worker;
  try {
    worker = new Worker(url);
  } catch (error) {
    URL.revokeObjectURL(url);
    throw error;
  }
  const terminate = worker.terminate.bind(worker);
  worker.terminate = () => {
    URL.revokeObjectURL(url);
    terminate();
  };
  return worker;
}
