// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The page's half of the frozen-page watchdog (Mods spec §1.4, plan decision 5; `mods::watchdog` in
// Rust is the other half and the one that decides).
//
// A mod's sealed frame can starve PM's own page on an engine that runs both on one thread, and the page
// can't notice that it is the one that is stuck. So while any mod frame is mounted the page proves it is
// alive: a heartbeat to Rust every BEAT_MS, on a timer of the page's own. If Rust hears nothing for
// about 10 s with a frame mounted and the window on screen, it pauses the mod whose frame was last
// active and reloads this page. The decision, and the cases that must not count as frozen (a hidden or
// minimised window, a machine that slept), are Rust's; the page only reports whether it is visible,
// because its timers throttle when it isn't, and says so the moment that changes.
//
// ModFrame calls `frameMounted()` while it is mounted. The heartbeat runs while at least one is.

import { modsHeartbeat } from "./ipc";

/** How often the page sends a heartbeat (`mods::watchdog::BEAT_EVERY`). Rust waits ~5 of them. */
export const BEAT_MS = 2000;

let mounted = 0;
let timer: ReturnType<typeof setInterval> | null = null;

/** Whether the page is on screen, as the page itself knows it. */
function visible(): boolean {
  return document.visibilityState !== "hidden";
}

function beat() {
  // Best-effort: a heartbeat that fails (the store is closed, the command is refused) is just a missed
  // one, and the page has nothing better to do about it.
  void modsHeartbeat(visible()).catch(() => {});
}

function start() {
  // At once, so Rust's clock starts from a real beat and not from the mount; then on a timer, and
  // whenever visibility changes (a timer throttled while hidden can't say it is hidden or back).
  beat();
  timer = setInterval(beat, BEAT_MS);
  document.addEventListener("visibilitychange", beat);
}

function stop() {
  if (timer !== null) clearInterval(timer);
  timer = null;
  document.removeEventListener("visibilitychange", beat);
}

/** A mod frame is mounted: the heartbeat runs until every such frame has called the returned function
 *  (once; calling it again does nothing, so StrictMode's mount-unmount-mount and a double cleanup
 *  can't leave the count wrong). */
export function frameMounted(): () => void {
  mounted += 1;
  if (mounted === 1) start();
  let released = false;
  return () => {
    if (released) return;
    released = true;
    mounted -= 1;
    if (mounted === 0) stop();
  };
}

/** How many mod frames are mounted (the tests). */
export function mountedFrames(): number {
  return mounted;
}
