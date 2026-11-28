// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useRef, useState } from "react";

import { onRouteChanged, routeStatus } from "./ipc";
import type { RoleName, RouteView } from "./types";

export interface UseRoutes {
  chat: RouteView | null;
  background: RouteView | null;
  /** An edit answered with the list as it now is: show it at once, without waiting for the event. */
  set: (role: RoleName, route: RouteView) => void;
  /** Read both lists again (one cheap read, no network). */
  reload: () => Promise<void>;
  /** The newest read failed and no list has ever been read: the page says so, with a way to retry,
   *  instead of drawing a heading over nothing. False once any read has landed. */
  failed: boolean;
}

/** Both roles' ordered lists and every row's status, read again whenever the backend says a list or
 *  a row's health changed (`route://changed`). null until the first answer, or if there is none:
 *  every reader draws no list then, never a guessed one. */
export function useRoutes(): UseRoutes {
  const [routes, setRoutes] = useState<{ chat: RouteView | null; background: RouteView | null }>({
    chat: null,
    background: null,
  });
  const [failed, setFailed] = useState(false);
  const alive = useRef(true);
  // Which read, or which edit's answer, is the newest. Every edit's answer is the list as it now is,
  // and a read that STARTED before it can land after it with the list as it was one edit ago; the
  // next keypress would then carry indices of a list that no longer exists and move or remove the
  // wrong row. A read applies only if nothing newer was applied since it began.
  const newest = useRef(0);
  const loaded = useRef(false);

  const reload = useCallback(async () => {
    const mine = ++newest.current;
    try {
      const next = await routeStatus();
      if (alive.current && mine === newest.current) {
        loaded.current = true;
        setFailed(false);
        setRoutes({ chat: next.chat, background: next.background });
      }
    } catch {
      // No list is drawn, and the page says why (a list already read stays on screen).
      if (alive.current && mine === newest.current && !loaded.current) setFailed(true);
    }
  }, []);

  useEffect(() => {
    alive.current = true;
    void reload();
    let off: (() => void) | undefined;
    let gone = false;
    try {
      void Promise.resolve(onRouteChanged(() => void reload()))
        .then((un) => {
          if (gone) un();
          else off = un;
        })
        .catch(() => {});
    } catch {
      /* no event bridge: the lists still read on open and after every edit */
    }
    return () => {
      alive.current = false;
      gone = true;
      off?.();
    };
  }, [reload]);

  const set = useCallback((role: RoleName, route: RouteView) => {
    // An edit's own answer is newer than any read already in flight: that read is now stale. (Before
    // the first read has landed there is nothing else to show, so that one read is still wanted.)
    if (loaded.current) newest.current += 1;
    setRoutes((r) => ({ ...r, [role]: route }));
  }, []);

  return { chat: routes.chat, background: routes.background, set, reload, failed };
}
