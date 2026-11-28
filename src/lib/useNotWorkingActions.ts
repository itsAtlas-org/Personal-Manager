// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What a click on the "not working" card does, once: the three buttons' wiring in one testable place.
// A click on an offer re-sends this reply to exactly that model; "Use X from now on" first changes
// Chat's setting with the setters that already exist, then re-sends as an ordinary send. Only a
// click gets here: PM never calls this by itself.

import { useCallback, useRef, useState } from "react";
import { entryOf, fromNowOnSteps, runFromNowOn, type FromNowOnSetters } from "./notWorking";
import type { EntryRef, NotWorkingOffer } from "./types";
import type { NotWorkingState } from "./useChatStream";

export interface NotWorkingActionDeps {
  /** The card on screen (null when there is none). */
  card: NotWorkingState | null;
  /** The OpenRouter list as it stands, so "from now on" keeps the others behind the new first. */
  chatModels: string[];
  setters: FromNowOnSetters;
  resend: (card: NotWorkingState, override: EntryRef | null) => Promise<boolean>;
  setError: (message: string) => void;
  /** Re-read what the send persisted (messages, the conversation list). */
  settled: (convId: number) => Promise<void>;
  refreshSettings: () => void;
  /** Take focus to the composer: the button that was pressed is about to leave the page. */
  focusComposer: () => void;
  /** Take the failed message off the send queue. The composer's send put it back at the head with a
   *  "That didn't send, Try again" chip; the click re-sends this reply itself, so a chip left behind
   *  would send the same question a second time (and pay for it), and so would the next typed message. */
  dropFailedSend: () => void;
}

/** One click at a time. A "from now on" click awaits up to three settings writes before the re-send
 *  starts, and during that window the card is still on screen with its buttons enabled; without the
 *  guard a second click would send the same reply twice (and pay for both). */
export function useNotWorkingActions(deps: NotWorkingActionDeps) {
  const latest = useRef(deps);
  latest.current = deps;
  const inFlight = useRef(false);
  const [busy, setBusy] = useState(false);
  // The card that is on screen right now came from a click's re-send: its heading takes focus, so a
  // keyboard user lands on what happened instead of at the top of the page.
  const [afterClick, setAfterClick] = useState(false);

  const use = useCallback(async (offer: NotWorkingOffer, fromNowOn: boolean) => {
    const d = latest.current;
    const card = d.card;
    if (!card || inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    try {
      if (fromNowOn) {
        try {
          await runFromNowOn(fromNowOnSteps(offer, d.chatModels), d.setters);
          d.refreshSettings();
        } catch (e) {
          d.setError(String(e));
          return;
        }
      }
      // The card (and the button that has focus) leaves the page now: focus goes to the composer
      // rather than falling to the top of the document, and a card that comes back after a second
      // failure takes focus on its heading.
      setAfterClick(true);
      d.focusComposer();
      // Just before the re-send, after the settings writes: a refused "from now on" sends nothing, so
      // the person's "Try again" must still be there for it.
      d.dropFailedSend();
      await latest.current.resend(card, fromNowOn ? null : entryOf(offer));
      // Reload persisted state, as an ordinary send does.
      try {
        await latest.current.settled(card.convId);
      } catch {
        /* keep optimistic state on reload failure */
      }
    } finally {
      inFlight.current = false;
      setBusy(false);
      // Any card this click's re-send produced is mounted by now; a later, unrelated card must not
      // take focus from where the person is typing.
      setAfterClick(false);
    }
  }, []);

  return { use, busy, afterClick };
}
