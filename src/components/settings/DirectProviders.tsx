// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { directInfo } from "../../lib/directProviders";
import type { UseDirectProviders } from "../../lib/useDirectProviders";
import { Callout, SectionLabel } from "../ui";
import { DirectProviderCard } from "./DirectProviderCard";

/**
 * "Your own keys" on the AI & Models tab: one card per provider PM can reach with the person's own
 * key. Off by default, and said so in the first sentence, because a request to your own key is not
 * zero data retention the way OpenRouter's is. Renders nothing until the backend has answered, so a
 * failed read leaves the tab as it was before this section existed.
 */
export function DirectProviders({
  dp,
  orKey,
  onError,
}: {
  dp: UseDirectProviders;
  orKey: { chat: boolean; background: boolean };
  onError?: (message: string | null) => void;
}) {
  if (!dp.state && !dp.error) return null;
  return (
    <div
      id="sec-ai-direct"
      data-settings-section
      data-help="settings-direct-keys"
      className="mt-5 border-t border-border pt-4"
    >
      <SectionLabel>Your own keys</SectionLabel>
      <p className="mt-1 text-xs text-ink4">
        Use your own key from OpenAI instead of OpenRouter. It's off until you turn it on. Requests
        to your own key are not zero data retention.
      </p>
      {dp.error && !dp.state && (
        <Callout className="mt-3" live={false}>
          PM couldn't read your own-key settings. {dp.error}
        </Callout>
      )}
      <div className="mt-3 space-y-3">
        {dp.state?.providers.map((view) => {
          const info = directInfo(view.provider);
          return info ? (
            <DirectProviderCard
              key={view.provider}
              info={info}
              view={view}
              reload={dp.reload}
              orKey={orKey}
              onError={onError}
            />
          ) : null;
        })}
      </div>
    </div>
  );
}
