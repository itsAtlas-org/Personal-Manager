// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { ListWalks, ModelSpend, ServedRoad, UnpricedTokenSlug } from "./types";

/** Format a USD cost, or "—" when unknown (the model isn't in the price cache yet). */
export function fmtUsd(v: number | null): string {
  if (v == null) return "—";
  if (v === 0) return "$0.00";
  return v < 0.01 ? `$${v.toFixed(4)}` : `$${v.toFixed(2)}`;
}

/** A figure that is a floor reads "at least $X"; an exact one is just "$X"; no figure is "—". One
 *  formatter for every total and cost cell, so no screen prints a floor as if it were the whole. */
export function atLeast(v: number | null, partial: boolean): string {
  if (v == null) return "—";
  return partial ? `at least ${fmtUsd(v)}` : fmtUsd(v);
}

/** The hover text on a figure that is a floor or unknown: a dash says PM couldn't price it at all,
 *  "at least" says it priced some. `undefined` for an exact figure. */
export function partialTitle(v: number | null, partial: boolean): string | undefined {
  if (!partial) return undefined;
  return v == null
    ? "PM couldn't price these calls."
    : "Some of these calls couldn't be priced in full, so this is at least what they cost.";
}

/** Own keys only: a lookup by a string from outside must never reach `constructor` or `toString`. */
function own(table: object, key: string): boolean {
  return Object.prototype.hasOwnProperty.call(table, key);
}

const VIA: Record<ServedRoad, string> = {
  openrouter: "OpenRouter",
  openai: "Your OpenAI key",
  gemini: "Your Gemini key",
  anthropic: "Your Anthropic key",
  local: "This computer",
};

/** What a Usage row says about the road it took. A cloud row from before PM recorded it has none and
 *  says so; PM never guesses one from the model id (a direct id and an OpenRouter id can look alike).
 *  A value this build doesn't know reads the same way instead of printing the provider's text. */
export function viaLabel(served: ServedRoad | null): string {
  return served && own(VIA, served) ? VIA[served] : "Before PM recorded this";
}

/** 1234 -> "1.2k", 12 -> "12". Plain counts for the one detail line under a model. */
export function fmtCount(n: number): string {
  if (n < 1000) return String(n);
  // 999,500 would round to "1000k"; past that the figure is in millions.
  if (n >= 999_500) return `${(n / 1_000_000).toFixed(1).replace(/\.0$/, "")}m`;
  const k = n / 1000;
  return `${k >= 100 ? Math.round(k) : k.toFixed(1).replace(/\.0$/, "")}k`;
}

/** The small line under a model: what the cache and the thinking did, and how many replies were
 *  refused. Only what happened is named; `null` when nothing did. */
export function detailLine(
  s: Pick<ModelSpend, "cached_tokens" | "thinking_tokens" | "refused_requests">,
): string | null {
  const parts: string[] = [];
  if (s.cached_tokens > 0) parts.push(`cached ${fmtCount(s.cached_tokens)}`);
  if (s.thinking_tokens > 0) parts.push(`thinking ${fmtCount(s.thinking_tokens)}`);
  if (s.refused_requests > 0) parts.push(`${s.refused_requests} refused`);
  if (parts.length === 0) return null;
  const line = parts.join(", ");
  return line.charAt(0).toUpperCase() + line.slice(1);
}

const UNPRICED_WORDS: Record<UnpricedTokenSlug, string> = {
  audio_tokens: "audio tokens",
  prediction_tokens: "prediction tokens",
  non_text_prompt: "images or audio in the prompt",
  server_tool_use: "tool use",
  new_token_type: "a new kind of token the provider started reporting",
};

/** DD-MM-YYYY in the person's own day. An unreadable stamp is returned as it came. */
function dayOf(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getDate())}-${p(d.getMonth() + 1)}-${d.getFullYear()}`;
}

/** The Usage tab's standing line for replies a later row of the person's list answered. Background
 *  and Review have no strip, and a row PM remembers as down is passed over without a request, so
 *  this is where a bad first row shows up: how many, the first one's reason (Rust's class words) and
 *  its date. Says "couldn't answer", not "failed", because a row passed over without a request is
 *  counted too. */
export function listWalksLine(w: Pick<ListWalks, "replies" | "first_reason" | "first_at">): string {
  const n = w.replies;
  return `${n} ${n === 1 ? "reply" : "replies"} in the last 30 days went to a later model in your list because an earlier one couldn't answer. The first was on ${dayOf(w.first_at)} (${w.first_reason}).`;
}

/** The Usage tab's warning for a token type PM can't price. Only a slug PM knows is worded; the
 *  words are PM's own, never text from the provider. */
export function unpricedWarning(slug: string, replies: number): string | null {
  if (!own(UNPRICED_WORDS, slug)) return null;
  const what = UNPRICED_WORDS[slug as UnpricedTokenSlug];
  return `PM can't price ${what} on ${replies} ${replies === 1 ? "reply" : "replies"} in the last 30 days, so these figures say 'at least'.`;
}
