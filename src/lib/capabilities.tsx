// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The capability layer: the ONE source of truth for the two "developer" signals, so feature
// code never reads `import.meta.env.DEV` directly nor smears a bare `if (devMode)` through render
// logic (issue #78). Two distinct signals for two audiences:
//
//   * `useDevMode()` — RUNTIME, user-flippable, persisted. The MASTER switch for every developer
//     surface: when off, nothing developer-facing shows, in any build. It gates the read-only
//     INSPECTION surfaces directly, and it is what lets an unsigned mod be installed and run
//     (Mods spec §1.15). A normal supported setting (default off), NOT a build-time gate — so it
//     ships in the release and can be switched on by a curious user.
//   * `isDevBuild`  — BUILD-TIME (`import.meta.env.DEV`), re-exposed here. The hard FLOOR under the
//     maintainer-only TEST HARNESSES that must never ship (they write synthetic state): they are
//     dead-code-eliminated from release bundles. Harnesses gate on `isDevBuild && devMode` — the
//     build gate strips them from release, and the runtime toggle still hides them in a dev build.
//     This module is the single place that literal is read.
//
// **The backend is the authority, per device.** Developer mode is "on for THIS device in THIS
// vault" (Rust's `mods::devmode` record): a restore onto another machine or a joined shared vault
// never inherits it, and switching it never reaches another machine using the same vault. So the
// page doesn't decide it. Its answer is `devModeState()`, asked on mount, whenever the vault opens
// (`mods://changed`, cause `loaded`) and whenever any window switches it (`mods://dev-mode`), and it
// always wins over what this webview remembers. The localStorage copy is only a cache so the Dev
// tab never flashes on first paint; it is never believed over the backend, and with no answer from
// the backend yet (the vault isn't open) it simply stays until one comes.
//
// **The toggle awaits the backend.** `setDevMode` resolves with the state the backend now holds
// (what the page shows is that, not what it asked for) and rejects, changing nothing, when the
// write failed: the caller shows the error and the switch stays where it was.

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { devModeState, devModeSwitch, onDevMode, onModsChanged } from "./ipc";

/** Build-time signal: true only in a dev build. The ONE place `import.meta.env.DEV` is read. */
export const isDevBuild: boolean = import.meta.env.DEV;

const LS_KEY = "pm:dev:mode";

interface CapabilityState {
  /** Runtime developer mode for this device — gates the inspection surfaces and unsigned mods. */
  devMode: boolean;
  /** Switch it for this device and wait for the backend: resolves once it is written (the value
   *  shown is then the backend's), rejects with the backend's refusal and changes nothing. */
  setDevMode: (on: boolean) => Promise<void>;
}

const CapabilityContext = createContext<CapabilityState | null>(null);

// localStorage can throw (locked-down webviews); never let a capability read/write crash the app.
function readLs(): boolean {
  try {
    return localStorage.getItem(LS_KEY) === "true";
  } catch {
    return false;
  }
}
function writeLs(on: boolean): void {
  try {
    localStorage.setItem(LS_KEY, on ? "true" : "false");
  } catch {
    /* ignore — the cache just won't be there next time */
  }
}

/** The causes of `mods://changed` after which developer mode may have changed under us. */
const REHYDRATE_CAUSES = new Set(["loaded", "recovered", "dev-mode"]);

export function CapabilityProvider({ children }: { children: ReactNode }) {
  const [devMode, setDevModeState] = useState<boolean>(readLs);
  // Which question the page is waiting on: an answer to anything but the latest is stale (a switch
  // landed, or a newer question was asked) and is dropped, so a slow answer can't undo a newer one.
  const asked = useRef(0);

  const adopt = useCallback((on: boolean) => {
    asked.current += 1;
    setDevModeState(on);
    writeLs(on);
  }, []);

  useEffect(() => {
    let cancelled = false;
    const hydrate = () => {
      const mine = ++asked.current;
      devModeState()
        .then((state) => {
          if (cancelled || mine !== asked.current) return;
          setDevModeState(state.on);
          writeLs(state.on);
        })
        .catch(() => {
          /* the vault isn't open (or locked): keep the cache until it opens and we are asked again */
        });
    };
    hydrate();
    // A listener that couldn't subscribe (no Tauri here) resolves to null: nothing to undo.
    const listeners = [
      onModsChanged((e) => {
        if (REHYDRATE_CAUSES.has(e.cause)) hydrate();
      }),
      onDevMode((e) => {
        if (!cancelled) adopt(e.on);
      }),
    ].map((p) => p.catch(() => null));
    return () => {
      cancelled = true;
      for (const l of listeners) void l.then((unlisten) => unlisten?.());
    };
  }, [adopt]);

  const setDevMode = useCallback(
    async (on: boolean): Promise<void> => {
      // A rejection leaves the state and the cache exactly as they were.
      const done = await devModeSwitch(on);
      adopt(done.on);
    },
    [adopt],
  );

  return (
    <CapabilityContext.Provider value={{ devMode, setDevMode }}>
      {children}
    </CapabilityContext.Provider>
  );
}

export function useDevMode(): CapabilityState {
  const ctx = useContext(CapabilityContext);
  if (!ctx) throw new Error("useDevMode must be used within <CapabilityProvider>");
  return ctx;
}
