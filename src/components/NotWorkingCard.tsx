// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useId, useRef, useState } from "react";
import type { NotWorkingInfo, NotWorkingOffer } from "../lib/types";
import {
  announcement,
  fromNowOnName,
  fromNowOnNote,
  introLine,
  limitLine,
  offerName,
} from "../lib/notWorking";
import { Button, VisuallyHidden } from "./ui";

/**
 * The card a failed chat reply shows instead of a dead red line: why it isn't working, whether
 * anything was sent anywhere else (only said when true), and up to three one-click ways to get an
 * answer. Every offer is the person's own explicit choice: a click is the only way a reply ever
 * goes to another company here.
 *
 * Accessibility: a labelled group; a polite live region says what appeared once the card is on
 * screen; focus is NOT moved (the composer keeps it), and the first button is simply the next Tab
 * stop after the failed turn, because the card sits right after it in the page. Buttons are real
 * buttons whose names carry the model and who gets the reply, and the data note is their
 * description, so a screen reader hears it with the button. Wraps at phone width, and a tall card
 * scrolls inside itself rather than pushing the conversation and the composer off the screen.
 *
 * `focusOnMount` is for a card that appears because a click's re-send failed again: the button that
 * was pressed is gone, so the heading takes focus (it is not otherwise a tab stop).
 */
export function NotWorkingCard({
  info,
  busy,
  focusOnMount = false,
  onUse,
  onUseFromNowOn,
  onEditList,
  onRaiseLimit,
}: {
  info: NotWorkingInfo;
  /** A re-send is under way: every button waits. */
  busy: boolean;
  focusOnMount?: boolean;
  onUse: (offer: NotWorkingOffer) => void;
  onUseFromNowOn: (offer: NotWorkingOffer) => void;
  onEditList: () => void;
  onRaiseLimit: () => void;
}) {
  const titleId = useId();
  const headingRef = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    if (focusOnMount) headingRef.current?.focus();
    // Once, when the card appears: a later change of the prop must not pull focus again.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const introId = useId();
  const intro = introLine(info);
  // A live region only speaks what changes after it is on screen, so the sentence is set once the
  // card has mounted rather than rendered with it.
  const [said, setSaid] = useState("");
  const spoken = announcement(info);
  useEffect(() => {
    setSaid(spoken);
  }, [spoken, info.turn_id]);

  return (
    <section
      role="group"
      aria-labelledby={titleId}
      data-help="chat-not-working"
      className="max-h-[50vh] overflow-y-auto border-b border-rule px-4 py-3 font-ui text-sm"
    >
      <h2
        id={titleId}
        ref={headingRef}
        tabIndex={-1}
        className="font-semibold text-ink focus:outline-none focus-visible:underline"
      >
        {info.headline}
      </h2>
      <p className="mt-1 text-ink2">{info.reason}</p>
      {info.detail && <p className="mt-1 text-ink3">{info.detail}</p>}
      {info.limit && <p className="mt-1 text-ink2">{limitLine(info.limit)}</p>}
      {info.reassurance && <p className="mt-1 text-ink3">{info.reassurance}</p>}
      {intro && (
        <p id={introId} className="mt-3 text-ink2">
          {intro}
        </p>
      )}
      {info.offers.length > 0 && (
        <ul aria-labelledby={introId} className="mt-2 flex flex-col gap-3">
          {info.offers.map((offer) => (
            <OfferRow
              key={offer.key}
              offer={offer}
              busy={busy}
              onUse={onUse}
              onUseFromNowOn={onUseFromNowOn}
            />
          ))}
        </ul>
      )}
      <div className="mt-3 flex flex-wrap gap-2">
        {info.can_edit_list && (
          <Button variant="tertiary" size="sm" onClick={onEditList}>
            Edit my list
          </Button>
        )}
        {info.limit && (
          <Button variant="tertiary" size="sm" onClick={onRaiseLimit}>
            Raise your limit
          </Button>
        )}
      </div>
      <VisuallyHidden role="status" aria-live="polite">
        {said}
      </VisuallyHidden>
    </section>
  );
}

function OfferRow({
  offer,
  busy,
  onUse,
  onUseFromNowOn,
}: {
  offer: NotWorkingOffer;
  busy: boolean;
  onUse: (offer: NotWorkingOffer) => void;
  onUseFromNowOn: (offer: NotWorkingOffer) => void;
}) {
  const noteId = useId();
  const foreverId = useId();
  return (
    <li className="flex flex-wrap items-start gap-x-3 gap-y-1">
      <Button
        variant="secondary"
        size="md"
        disabled={busy}
        aria-describedby={noteId}
        onClick={() => onUse(offer)}
      >
        {offerName(offer)}
      </Button>
      <Button
        variant="tertiary"
        size="sm"
        disabled={busy}
        aria-describedby={`${foreverId} ${noteId}`}
        onClick={() => onUseFromNowOn(offer)}
      >
        {fromNowOnName(offer)}
      </Button>
      <VisuallyHidden id={foreverId}>{fromNowOnNote(offer)}</VisuallyHidden>
      <p id={noteId} className="basis-full text-xs text-ink3">
        {offer.data_note}
      </p>
    </li>
  );
}
