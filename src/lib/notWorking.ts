// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The wording and the pure decisions of the "not working" card, in one reviewable file. What depends
// on the failure itself (the reason line, the reassurance, the offers and their data notes) is built
// in Rust (`offers.rs`) from the typed failure; what does not (the buttons, the lines around them,
// the announcement, what "from now on" changes) lives here.

import type { EntryRef, NotWorkingInfo, NotWorkingLimit, NotWorkingOffer } from "./types";

/** The line above the buttons. A refusal is told differently: the same prompt would be refused
 *  again, so the person is asked to change the prompt or ask another model, and PM never re-sends it
 *  by itself. */
export function introLine(info: Pick<NotWorkingInfo, "class" | "offers">): string | null {
  if (info.offers.length === 0) return null;
  return info.class === "refused"
    ? "Rephrase it, or ask another model:"
    : "Use something else for this reply:";
}

/** Who answers an offer, for the button's name: "Google, your key", "OpenRouter", "this computer". */
export function offerVia(offer: Pick<NotWorkingOffer, "kind" | "company">): string {
  switch (offer.kind) {
    case "same_key":
    case "other_company":
      return `${offer.company}, your key`;
    case "openrouter":
      return offer.company;
    case "local":
      // "this computer", or "your Local AI server" when the server is another machine.
      return offer.company;
  }
}

/** A button's name: the model and who gets the reply. */
export function offerName(offer: NotWorkingOffer): string {
  return `${offer.label} (${offerVia(offer)})`;
}

/** The name of the quiet "from now on" button beside an offer. */
export function fromNowOnName(offer: Pick<NotWorkingOffer, "label">): string {
  return `Use ${offer.label} from now on`;
}

/** What "from now on" does, said once for the button's description: the company is named because it
 *  is the heaviest button on the card (every later Chat reply goes there). */
export function fromNowOnNote(offer: Pick<NotWorkingOffer, "company">): string {
  return `Chat will use ${offer.company} from now on.`;
}

/** The polite announcement when the card appears. It follows the headline: a refusal is not "not
 *  working", and no options is not "0 options". */
export function announcement(info: Pick<NotWorkingInfo, "headline" | "offers" | "class">): string {
  const n = info.offers.length;
  const options = n === 0 ? "" : ` ${n} ${n === 1 ? "option" : "options"}.`;
  if (info.class === "refused") {
    return `${info.headline}.${options}`;
  }
  const subject = info.headline.replace(/ isn't working right now$/, "");
  return `${subject} isn't working.${options}`;
}

/** USD as the limit copy writes it: "$5.00". */
function usd(x: number): string {
  return `$${x.toFixed(2)}`;
}

/** The standing line for a reply stopped by the person's own daily limit. "At least" is the same
 *  honest floor the limit's own card uses: PM counts only what it logged. */
export function limitLine(limit: NotWorkingLimit): string {
  return `PM has logged at least ${usd(limit.spent_today_usd)} of your ${usd(limit.limit_usd)} limit. It starts again at 00:00 (your day), or sooner if you raise the limit.`;
}

/** What a click on an offer asks the backend: the provider and the model the offer carried. */
export function entryOf(offer: NotWorkingOffer): EntryRef {
  return { provider: offer.provider, model: offer.model };
}

// "Use X from now on" is ONE list edit: `promoteEntry("chat", provider, model)` puts the offered row
// first in Chat's list (inserted, or moved there), after the same checks any list write gets. The
// card is only ever shown for a chat reply, so Background is never touched. Nothing here chooses the
// order of several settings any more: the list is the one truth.
