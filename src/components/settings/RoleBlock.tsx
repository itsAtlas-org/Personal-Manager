// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useMemo, useState } from "react";

import {
  directInfo,
  directPickerRows,
  effortAfterModelChange,
  effortCoercedLine,
  effortOptions,
  effortToWrite,
  eligibleProviders,
  isDirect,
  pickerCaption,
  providerName,
  providerOptionLabel,
  providerView,
  roleChangedAnnouncement,
  roleIssue,
  roleIssueLine,
  roleModel,
  roleName,
  routeNote,
  unavailableLabel,
} from "../../lib/directProviders";
import { setDirectRole, setRoleCloudProvider } from "../../lib/ipc";
import type { UseDirectProviders } from "../../lib/useDirectProviders";
import type { CloudProviderId, RoleName } from "../../lib/types";
import { ModelListEditor } from "../ModelListEditor";
import { Button, Select, SettingRow, VisuallyHidden } from "../ui";

interface Props {
  role: RoleName;
  /** "Chat model" / "Background model". */
  label: string;
  /** The OpenRouter editor's description. A direct role has its own words. */
  description: string;
  helpId: string;
  dp: UseDirectProviders;
  /** The role's routing from the Local AI settings ("cloud" | "local" | "local-then-cloud"); "cloud"
   *  while Local AI is off. It decides what the note under a direct provider says. */
  routing: string;
  /** OpenRouter's own list, exactly as the tab edited it before there was a second provider. */
  openRouter: {
    models: string[];
    onChange: (models: string[]) => void;
    autoSwitch: boolean;
    onAutoSwitchChange: (on: boolean) => void;
    onReset?: () => void;
  };
  onError: (message: string | null) => void;
}

const EFFORT_TEXT: Record<RoleName, string> = {
  chat: "How hard the model thinks before it answers. More effort is slower and costs more.",
  background: "Background work runs a lot. Low keeps it quick and cheap.",
};

/**
 * One role's half of AI & Models: which provider answers it, and that provider's own model list.
 *
 * With no provider but OpenRouter to offer (the state before anyone opts in) this is today's editor
 * and nothing else, so the tab doesn't change for a person who never turns a key on. The provider
 * choice lives here and nowhere else (one control, one home); the card above only opts a provider
 * in. A role that can't be answered, because its provider's key went or its switch was turned off,
 * says so in words rather than quietly answering from somewhere else.
 *
 * The Select never unmounts when the provider changes. The editor under it is keyed by the
 * provider, so switching swaps the list instead of editing the other provider's.
 */
export function RoleBlock({
  role,
  label,
  description,
  helpId,
  dp,
  routing,
  openRouter,
  onError,
}: Props) {
  const [announce, setAnnounce] = useState("");
  const [effortLine, setEffortLine] = useState("");
  const state = dp.state;
  const who = roleName(role);
  const lower = who.toLowerCase();

  const stored: CloudProviderId = state?.roles[role].stored ?? "openrouter";
  const effective: CloudProviderId = state?.roles[role].effective ?? "openrouter";
  const view = state && isDirect(effective) ? providerView(state, effective) : undefined;
  const eligible = useMemo(() => (state ? eligibleProviders(state) : []), [state]);
  // Nothing to choose between, and nothing wrong: the editor alone, as before.
  const showSelect = state !== null && (eligible.length > 0 || isDirect(stored));
  const issue = state ? roleIssue(state, role) : null;
  // The live region stays once it has appeared: moving the role back to OpenRouter can remove the
  // Select in the same render that the announcement is set, and a region that goes and comes back
  // with its text is the one a screen reader misses.
  const changeable = showSelect || (view !== undefined && directInfo(view.provider) !== null);
  const [live, setLive] = useState(false);
  if (changeable && !live) setLive(true);

  async function pickProvider(next: string) {
    onError(null);
    setEffortLine("");
    try {
      await setRoleCloudProvider(role, next as CloudProviderId);
      const fresh = await dp.reload();
      const nextView = fresh && isDirect(next) ? providerView(fresh, next) : undefined;
      setAnnounce(
        roleChangedAnnouncement(
          role,
          next,
          nextView ? (roleModel(nextView, role)?.name ?? null) : null,
        ),
      );
    } catch (e) {
      onError(String(e));
      await dp.reload();
    }
  }

  async function pickModel(models: string[]) {
    if (!view || models.length === 0) return;
    const id = models[0];
    setAnnounce("");
    const from = roleModel(view, role);
    const to = view.models.find((m) => m.id === id);
    const { effort, coercedFrom } = effortAfterModelChange(role, view.roles[role].effort, from, to);
    onError(null);
    try {
      await setDirectRole(view.provider, role, [id], effort);
      await dp.reload();
      setEffortLine(coercedFrom ? effortCoercedLine(role, to, coercedFrom) : "");
    } catch (e) {
      onError(String(e));
      await dp.reload();
    }
  }

  async function pickEffort(value: string) {
    if (!view) return;
    const model = roleModel(view, role);
    onError(null);
    setEffortLine("");
    try {
      await setDirectRole(
        view.provider,
        role,
        [view.roles[role].model],
        effortToWrite(role, model, value),
      );
      await dp.reload();
    } catch (e) {
      onError(String(e));
      await dp.reload();
    }
  }

  const direct = view !== undefined && directInfo(view.provider) !== null;
  const model = view ? roleModel(view, role) : undefined;
  const efforts = direct ? effortOptions(role, model) : [];
  const effortValue = view?.roles[role].effort ?? "";

  return (
    <div className="space-y-3">
      {showSelect && state && (
        <SettingRow label={`${who} provider`} helpId="settings-role-provider" spacing="none">
          {(a11y) => (
            <Select {...a11y} value={stored} onChange={(e) => void pickProvider(e.target.value)}>
              <option value="openrouter">{providerOptionLabel("openrouter")}</option>
              {eligible.map((p) => (
                <option key={p.provider} value={p.provider}>
                  {providerOptionLabel(p.provider)}
                </option>
              ))}
              {isDirect(stored) && !eligible.some((p) => p.provider === stored) && (
                <option value={stored} disabled>
                  {unavailableLabel(stored, state, role)}
                </option>
              )}
            </Select>
          )}
        </SettingRow>
      )}

      {direct && view && (
        <p className="text-xs text-ink4">{routeNote(role, providerName(view.provider), routing)}</p>
      )}

      {direct && view ? (
        <ModelListEditor
          key={view.provider}
          label={label}
          description={`Your ${lower} model, straight from ${providerName(view.provider)}. PM knows these models' prices, so your spend is counted. It can't use a model it doesn't know.`}
          helpId={helpId}
          models={[view.roles[role].model]}
          onChange={(m) => void pickModel(m)}
          autoSwitch={false}
          onAutoSwitchChange={() => {}}
          source={{
            kind: "direct",
            models: directPickerRows(view),
            caption: pickerCaption(view),
          }}
          maxModels={1}
          showAutoSwitch={false}
          allowCustom={false}
          roleName={lower}
          nameFor={(id) => view.models.find((m) => m.id === id)?.name}
        />
      ) : (
        <ModelListEditor
          key="openrouter"
          label={label}
          description={description}
          helpId={helpId}
          models={openRouter.models}
          onChange={openRouter.onChange}
          autoSwitch={openRouter.autoSwitch}
          onAutoSwitchChange={openRouter.onAutoSwitchChange}
          onReset={openRouter.onReset}
          roleName={lower}
        />
      )}

      {direct && efforts.length > 0 && (
        <SettingRow
          label={`${who} effort`}
          description={EFFORT_TEXT[role]}
          helpId="settings-role-effort"
          spacing="none"
        >
          {(a11y) => (
            <Select {...a11y} value={effortValue} onChange={(e) => void pickEffort(e.target.value)}>
              {efforts.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </Select>
          )}
        </SettingRow>
      )}
      {effortLine && (
        // The same words are in the live region at the bottom, which is what a screen reader hears.
        <p aria-hidden="true" className="text-xs text-ink4">
          {effortLine}
        </p>
      )}

      {issue === "turned_off" && state && (
        <div className="flex flex-wrap items-center gap-2 text-xs text-ink3">
          <span>{roleIssueLine(issue, role, providerName(stored))}</span>
          <Button
            type="button"
            variant="tertiary"
            size="xs"
            aria-label={`Use OpenRouter for ${who}`}
            onClick={() => void pickProvider("openrouter")}
          >
            Use OpenRouter
          </Button>
        </div>
      )}
      {(issue === "key_missing" || issue === "key_unreadable") && (
        <p className="text-xs text-st-due">{roleIssueLine(issue, role, providerName(stored))}</p>
      )}
      {/* A status and not an alert: it is a state of the account, not something that just failed.
          The region stays mounted (for a role that can change, like the one below) so a line that
          appears later, after Check again, is announced. */}
      {live ? (
        <div role="status">
          {issue === "no_credit" && (
            <p className="text-xs text-st-due">
              {roleIssueLine(issue, role, providerName(stored))}
            </p>
          )}
        </div>
      ) : (
        issue === "no_credit" && (
          <p role="status" className="text-xs text-st-due">
            {roleIssueLine(issue, role, providerName(stored))}
          </p>
        )
      )}
      {role === "background" && direct && (
        <p className="text-xs text-ink4">
          Mods can't use your own keys. While Background is on {providerName(effective)}, a mod that
          asks for AI gets no answer.
        </p>
      )}

      {/* One live region for the provider change and the effort coercion. It is in the DOM from the
          first render of a role that can change (a region that arrives together with its text is
          often not read), and absent for a person with nothing to choose, so their tab is today's. */}
      {live && (
        <VisuallyHidden role="status" aria-live="polite">
          {[announce, effortLine].filter(Boolean).join(" ")}
        </VisuallyHidden>
      )}
    </div>
  );
}
