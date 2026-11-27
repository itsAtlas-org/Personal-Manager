// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useState } from "react";

import {
  adoptRoles,
  BACKGROUND_NOT_SET_UP,
  directInfo,
  directNames,
  eligibleProviders,
  orList,
  providerOptionLabel,
  providerView,
  roleName,
  wizardModelsLine,
} from "../../lib/directProviders";
import { setRoleCloudProvider } from "../../lib/ipc";
import type { CloudProviderId, DirectProvidersState, RoleName } from "../../lib/types";
import type { UseDirectProviders } from "../../lib/useDirectProviders";
import { SectionLabel, Select, SettingRow } from "../ui";
import { DirectProviderCard } from "./DirectProviderCard";

const ROLES: RoleName[] = ["chat", "background"];

/**
 * The first-run wizard's "Your own key (optional)" block: the same card as Settings (off until the
 * person turns it on, with its warning), and, once a key has checked out, which provider answers
 * each role. The card writes through IPC the moment a key is checked, so nothing here waits for
 * "Get started".
 *
 * With no OpenRouter key the roles are pointed at the new provider the moment its key is good,
 * because "Get started" would otherwise finish onboarding with an app that has nothing to answer
 * with. With one, both roles stay on OpenRouter, the default, and the person can move them below.
 */
export function OnboardingDirectKey({
  dp,
  hasOpenRouterKey,
  onError,
}: {
  dp: UseDirectProviders;
  /** An OpenRouter key is saved or typed in the wizard. */
  hasOpenRouterKey: boolean;
  onError: (message: string | null) => void;
}) {
  const [saved, setSaved] = useState(false);
  const state = dp.state;
  if (!state) return null;
  const eligible = eligibleProviders(state);

  async function adopt(provider: CloudProviderId, next: DirectProvidersState | null) {
    setSaved(true);
    if (hasOpenRouterKey || !next) return;
    try {
      // A paid key takes both roles. A key that reads as free takes CHAT only: pointing background
      // at it would send documents to be trained on with no choice made (S8). A key that can't be
      // picked at all (paused, out of credit) takes neither.
      const take = adoptRoles(providerView(next, provider));
      for (const role of ROLES) {
        if (take.includes(role) && next.roles[role].effective === "openrouter") {
          await setRoleCloudProvider(role, provider);
        }
      }
      await dp.reload();
    } catch (e) {
      onError(String(e));
    }
  }

  async function pick(role: RoleName, provider: string) {
    onError(null);
    try {
      await setRoleCloudProvider(role, provider as CloudProviderId);
    } catch (e) {
      onError(String(e));
    }
    await dp.reload();
  }

  return (
    <div className="mt-5 border-t border-border pt-4">
      <SectionLabel>Your own key (optional)</SectionLabel>
      <p className="mt-1 text-xs text-ink4">
        Rather not use OpenRouter? Use your own {orList(directNames())} key. It's off unless you
        turn it on, and it isn't zero data retention.
      </p>
      <div className="mt-3 space-y-3">
        {state.providers.map((view) => {
          const info = directInfo(view.provider);
          if (!info) return null;
          return (
            <div key={view.provider} className="space-y-3">
              <DirectProviderCard
                info={info}
                view={view}
                reload={dp.reload}
                orKey={{ chat: hasOpenRouterKey, background: hasOpenRouterKey }}
                onError={onError}
                onKeySaved={(_r, next) => void adopt(view.provider, next)}
              />
              {saved && view.key === "present" && (
                <p className="text-xs font-medium text-st-track">✓ Saved to your keychain.</p>
              )}
            </div>
          );
        })}
        {eligible.length > 0 && (
          <div className="space-y-2">
            {ROLES.map((role) => (
              <SettingRow key={role} label={`${roleName(role)} uses`} spacing="none">
                {(a11y) => (
                  <Select
                    {...a11y}
                    value={state.roles[role].stored}
                    onChange={(e) => void pick(role, e.target.value)}
                  >
                    <option value="openrouter">{providerOptionLabel("openrouter")}</option>
                    {eligible.map((p) => (
                      <option key={p.provider} value={p.provider}>
                        {providerOptionLabel(p.provider)}
                      </option>
                    ))}
                  </Select>
                )}
              </SettingRow>
            ))}
            {eligible.map((p) => {
              const line = wizardModelsLine(
                p,
                p.roles.chat.uses_this_provider,
                p.roles.background.uses_this_provider,
              );
              return line ? (
                <p key={p.provider} className="text-xs text-ink4">
                  {line}
                </p>
              ) : null;
            })}
            {/* A free key took chat only, and nothing else answers background work yet. */}
            {!hasOpenRouterKey && state.roles.background.effective === "openrouter" && (
              <p className="text-xs text-st-due">{BACKGROUND_NOT_SET_UP}</p>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
