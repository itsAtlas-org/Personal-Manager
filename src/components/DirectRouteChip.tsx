// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect } from "react";

import { directInfo, isDirect, providerView } from "../lib/directProviders";
import type { LocalLlmStatus, Settings } from "../lib/types";
import { useDirectProviders } from "../lib/useDirectProviders";

/**
 * "OpenAI direct" in the chat composer, beside the local-endpoint chip: the one place chat says,
 * where the person is typing, that this reply goes straight to a provider with their own key and so
 * isn't zero data retention. Neutral ink, because it is a choice they made and not a fault; a role
 * whose key is gone wears the due tint instead, because chat can't answer.
 *
 * Renders nothing on OpenRouter (so a person who never turned a key on sees no change), and nothing
 * while a local model is answering chat (the reply isn't going to the provider; the note under the
 * role in Settings says what the fallback is).
 */
export function DirectRouteChip({
  settings,
  localAi,
}: {
  settings: Settings | null;
  localAi: LocalLlmStatus | null;
}) {
  const provider = settings?.chat_provider ?? "openrouter";
  if (!isDirect(provider) || localAi?.chat_local_model) return null;
  return <Chip provider={provider} settings={settings} />;
}

// Split out so the read below only happens while the chip is on screen.
function Chip({ provider, settings }: { provider: string; settings: Settings | null }) {
  const dp = useDirectProviders();
  // Settings closing re-reads `settings`; the key and the tick-box may have moved with it.
  const { reload } = dp;
  useEffect(() => {
    void reload();
  }, [settings, reload]);
  const info = directInfo(provider);
  const view = dp.state ? providerView(dp.state, provider) : undefined;
  // Nothing to say until PM knows whether the key is there: a flash of "no key" would be a lie.
  if (!info || !view) return null;
  // A key that exists but can't be read right now is not "no key": the card and the role block say
  // so in their own words, and so does this.
  const unreadable = view.key === "unreadable";
  const noKey = view.key !== "present" && !unreadable;
  const broken = noKey || unreadable;
  const label = unreadable
    ? `${info.name}: key unreadable`
    : noKey
      ? `${info.name}: no key`
      : view.shares_data
        ? info.chipSharedLabel
        : info.chipLabel;
  const title = unreadable
    ? `Chat is set to ${info.name}, but PM can't read your key right now. Chat can't answer until it can. Check Settings > AI & Models.`
    : noKey
      ? `Chat is set to ${info.name}, but there's no key to use. Chat can't answer until you fix this. Change it in Settings > AI & Models.`
      : `Chat goes straight to ${info.name} with your own key. It isn't zero data retention. Change it in Settings > AI & Models.`;
  const token = broken ? "--st-due" : "--ink3";
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
