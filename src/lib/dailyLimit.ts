// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { LimitView } from "./types";

/** A dollar amount the way the limit's lines print it: cents, and four places under a cent so a $0.01
 *  limit never reads "$0.00 of $0.01". The same rule as the backend's sentence (`usd_text`). */
export function limitUsd(v: number): string {
  if (v === 0 || v >= 0.01) return `$${v.toFixed(2)}`;
  return `$${v.toFixed(4)}`;
}

/** The field's text to a limit, or null when it is not an amount above zero. Plain digits with an
 *  optional "$" and decimal point only: "1e9", "-3", "0" and "" are all refused, so what is sent is
 *  always something the person typed as money. The backend checks again (a ceiling too). */
export function parseLimit(text: string): number | null {
  const t = text.trim().replace(/^\$/, "");
  if (!/^\d+(\.\d{0,4})?$/.test(t)) return null;
  const n = Number(t);
  return Number.isFinite(n) && n > 0 ? n : null;
}

/** The field's text for a stored limit: "5.00", or "0.0100" under a cent. */
export function limitFieldText(v: number): string {
  return limitUsd(v).slice(1);
}

export const LIMIT_FIELD_LABEL = "Stop after (USD per day)";
export const LIMIT_FIELD_ERROR = "Enter an amount above 0, or turn the limit off.";
export const FREE_KEY_LINE = "Your free key doesn't cost anything, so a limit isn't needed.";

/** The standing line on a paused card, chip title and role block: never folded away. */
export function pausedLine(l: LimitView, name = "this provider"): string {
  if (l.unreadable) {
    return `PM couldn't read today's usage, so it paused ${name} to be safe. Check Usage, or turn the limit off.`;
  }
  const spent = limitUsd(l.spent_today_usd);
  const limit = l.limit_usd == null ? "" : limitUsd(l.limit_usd);
  return `Paused for today. PM has logged at least ${spent} of your ${limit} limit. It starts again at 00:00 (your day), or sooner if you raise the limit.`;
}

/** What a screen reader hears when a pause begins on an open Settings page. A limit PM could not read
 *  was not "reached", so it never says "paused for today" (the standing line says why). */
export function pauseAnnouncement(name: string, unreadable: boolean): string {
  return unreadable
    ? `${name} is paused: PM couldn't read the limit or today's usage.`
    : `${name} is paused for today.`;
}

/** How long until the first pause in `limits` lifts by itself, in ms, so a page left open past
 *  midnight reads the state again instead of saying "Paused for today" into the next day. `null` when
 *  nothing is paused by a reached limit (an unreadable pause doesn't lift at midnight). A time already
 *  past waits a minute, so a backend that still says paused is never asked in a tight loop. */
export function msUntilResume(
  limits: (LimitView | null | undefined)[],
  now: number,
): number | null {
  let soonest: number | null = null;
  for (const l of limits) {
    if (!l || !l.reached || l.unreadable) continue;
    const at = Date.parse(l.resumes_at);
    if (Number.isNaN(at)) continue;
    if (soonest == null || at < soonest) soonest = at;
  }
  if (soonest == null) return null;
  const wait = soonest - now;
  // A second past the boundary, so the backend's own clock is surely in the new day.
  return wait > 0 ? wait + 1000 : 60_000;
}

/** The read-only line in Usage: "Today: $1.20 of $5.00", and "at least" only when some of today's
 *  calls couldn't be priced in full. `null` while the limit is off (nothing to read against). */
export function todayReadout(l: LimitView): string | null {
  if (l.limit_usd == null && !l.unreadable) return null;
  // Not a figure: when PM can't read today's usage the $0.00 it carries is a placeholder, and
  // printing it would invent a spend.
  if (l.unreadable || l.limit_usd == null) return "Today: PM couldn't read today's usage";
  const floor = l.unpriced_today > 0 ? "at least " : "";
  return `Today: ${floor}${limitUsd(l.spent_today_usd)} of ${limitUsd(l.limit_usd)}`;
}

/** One provider's line in Usage: "Anthropic: Today: $1.20 of $5.00", with the count of calls PM could
 *  not price beside it when there are any, so the floor says why. `null` while the limit is off. */
export function todayLine(name: string, l: LimitView): string | null {
  const readout = todayReadout(l);
  if (readout == null) return null;
  const gap =
    l.unpriced_today > 0
      ? ` (${l.unpriced_today === 1 ? "1 call" : `${l.unpriced_today} calls`} today couldn't be priced in full)`
      : "";
  return `${name}: ${readout}${gap}`;
}

/** The help text beside the control. Names the provider's own limit as the hard stop. */
export function limitHelp(provider: string, ownLimitWhere: string): string[] {
  return [
    `Pauses ${provider} for the rest of the day once PM has logged this much spent. Off by default; $5 is a sensible start.`,
    "PM stops new requests once it has logged this much. The reply that crosses the line can go over a little, and so can several replies already on their way.",
    `PM counts only what it logged: a reply that fails halfway can still cost something, and anything done with this key outside PM isn't counted. The limit is for this computer: another computer on the same key isn't counted. For a hard stop, also set a spending limit ${ownLimitWhere}.`,
    "Calls PM logged before it began recording which provider served them aren't counted, which matters only on the day you update.",
    "Your day is the time zone PM uses everywhere else. The limit lifts at 00:00 in it, or the moment you raise or clear it. Checking a key (Check again) can cost a fraction of a cent and isn't counted or stopped.",
    "A paused provider never sends the reply to another company on its own. PM tells you, and you choose.",
  ];
}

/** Where each provider's own spending limit lives, for the help's "hard stop" sentence. */
export const OWN_LIMIT_WHERE: Record<string, string> = {
  openrouter: "in your OpenRouter account (openrouter.ai/settings/limits)",
  openai: "in your OpenAI account (platform.openai.com, Billing, Limits)",
  gemini: "on the key's project in Google AI Studio (billing)",
  anthropic: "in your Anthropic Console (Settings, Limits)",
};
