// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect } from "react";

import { pausedLine } from "../lib/dailyLimit";
import {
  directInfo,
  hasTiers,
  isDirect,
  providerView,
  routeLabel,
  routeTermsOf,
  routeTitle,
  viewTerms,
} from "../lib/directProviders";
import type { LocalLlmStatus, Settings } from "../lib/types";
import { useDirectProviders } from "../lib/useDirectProviders";

/**
 * "OpenAI direct" in the chat composer, beside the local-endpoint chip: the one place chat says,
 * where the person is typing, that this reply goes straight to a provider with their own key and so
 * isn't zero data retention. Neutral ink, because it is a choice they made and not a fault; a role
 * whose key is gone wears the due tint instead, because chat can't answer. On a provider with tiers
 * (Gemini) it also says the terms the route is on, and a free or paused route wears the due tint too.
 *
 * Renders nothing on OpenRouter (so a person who never turned a key on sees no change), and nothing
 * while a local model is answering chat (the reply isn't going to the provider; the note under the
 * role in Settings says what the fallback is).
 */
export function DirectRouteChip({
  settings,
  localAi,
  busy = false,
}: {
  settings: Settings | null;
  localAi: LocalLlmStatus | null;
  /** A chat reply is on its way. The chip reads provider state again when this goes back to false: a
   *  reply that crossed the daily limit paused the provider without any Settings change. */
  busy?: boolean;
}) {
  const provider = settings?.chat_provider ?? "openrouter";
  if (!isDirect(provider) || localAi?.chat_local_model) return null;
  return <Chip provider={provider} settings={settings} busy={busy} />;
}

// Split out so the read below only happens while the chip is on screen.
function Chip({
  provider,
  settings,
  busy,
}: {
  provider: string;
  settings: Settings | null;
  busy: boolean;
}) {
  const dp = useDirectProviders();
  // Settings closing re-reads `settings`; the key and the tick-box may have moved with it. So does a
  // finished send: the reply that reached the daily limit changes nothing else the page sees.
  const { reload } = dp;
  useEffect(() => {
    if (!busy) void reload();
  }, [settings, busy, reload]);
  const info = directInfo(provider);
  const view = dp.state ? providerView(dp.state, provider) : undefined;
  // Nothing to say until PM knows whether the key is there: a flash of "no key" would be a lie.
  if (!info || !view) return null;
  // A key that exists but can't be read right now is not "no key": the card and the role block say
  // so in their own words, and so does this.
  const unreadable = view.key === "unreadable";
  const noKey = view.key !== "present" && !unreadable;
  const noCredit = !noKey && !unreadable && view.key_check?.outcome === "no_credit";
  // Where Settings has the role's terms they win: they are what Rust resolved for the route, region
  // and all. Before Settings has them (or for an older shape) the provider's own view says it.
  const tiered = hasTiers(info);
  const terms = tiered ? routeTermsOf(settings?.chat_route_terms ?? viewTerms(view)) : null;
  const paused = !noKey && !unreadable && terms === "blocked";
  // The person's own daily limit: a pause that ends by itself at midnight.
  const limited = !noKey && !unreadable && view.limit.reached;
  const broken = noKey || unreadable || noCredit || paused || limited;
  const free = !broken && terms === "free";
  const label = unreadable
    ? `${info.name}: key unreadable`
    : noKey
      ? `${info.name}: no key`
      : limited
        ? view.limit.unreadable
          ? `${info.name}: Paused`
          : `${info.name}: Paused for today`
        : noCredit
          ? `${info.name}: no credit`
          : routeLabel(info, terms, view.shares_data);
  const title = unreadable
    ? `Chat is set to ${info.name}, but PM can't read your key right now. Chat can't answer until it can. Check Settings > AI & Models.`
    : noKey
      ? `Chat is set to ${info.name}, but there's no key to use. Chat can't answer until you fix this. Change it in Settings > AI & Models.`
      : limited
        ? `${pausedLine(view.limit, info.name)} Change it in Settings > AI & Models.`
        : noCredit
          ? `Your ${info.name} account is out of credit or at a spend limit, so chat won't answer until you add credit or raise the limit. Then press Check again in Settings > AI & Models.`
          : tiered
            ? routeTitle(info, "chat", terms, view.blocked)
            : `Chat goes straight to ${info.name} with your own key. It isn't zero data retention. Change it in Settings > AI & Models.`;
  // The due tint is for a route to look at: a key that can't answer, or one on free terms.
  const token = broken || free ? "--st-due" : "--ink3";
  return (
    <span
      className="inline-flex items-center gap-1 rounded-[var(--radius-sm)] px-1.5 py-0.5 text-[0.625rem] font-medium"
      style={{
        color: `var(${token})`,
        background: `color-mix(in oklab, var(${token}) 15%, transparent)`,
      }}
      title={title}
    >
      {label}
    </span>
  );
}
