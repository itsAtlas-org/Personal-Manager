// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useRef, useState } from "react";

/**
 * `date`, once it has stopped moving: the first move of a burst passes at once, and moves less than
 * `ms` apart pass as one, `ms` after the last. The Calendar keys its network loads and its read window
 * on this, so a fling through a year of months (or a held ↓) asks for the months it ends on, not for
 * every one it passed. Leading as well as trailing, because a single jump (search reveal, the
 * mini-calendar, a header click) has nothing to wait for. What the header shows stays the live date.
 */
export function useSettledDate(date: Date, ms: number): Date {
  const [settled, setSettled] = useState(date);
  const settledRef = useRef(settled);
  const latestRef = useRef(date);
  const timerRef = useRef(0);

  const pass = (d: Date) => {
    // Same day again: keep the old object, so what is keyed on it doesn't run.
    if (d.getTime() === settledRef.current.getTime()) return;
    settledRef.current = d;
    setSettled(d);
  };

  const time = date.getTime();
  useEffect(() => {
    latestRef.current = date;
    if (timerRef.current === 0) {
      // Mount: nothing has moved, so there is no burst to wait out.
      if (time === settledRef.current.getTime()) return;
      // Nothing pending: this move starts a burst, and is the leading edge.
      pass(date);
    }
    window.clearTimeout(timerRef.current);
    // Every move restarts the wait; it ends the burst, and its last date is the trailing edge.
    timerRef.current = window.setTimeout(() => {
      timerRef.current = 0;
      pass(latestRef.current);
    }, ms);
    // `date` is read through its time: a new object for the same instant is not a move.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [time, ms]);

  useEffect(
    () => () => {
      window.clearTimeout(timerRef.current);
      timerRef.current = 0;
    },
    [],
  );

  return settled;
}
