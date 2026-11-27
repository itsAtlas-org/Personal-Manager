// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useId, useMemo, useState } from "react";

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
  modelWarning,
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
  /** False when OpenRouter can't take the role right now (no key for it, or its own daily limit has
   *  paused it): the paused-provider line then stops without a button that would lead nowhere.
   *  Unset means usable, as before. */
  openRouterUsable?: boolean;
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
  openRouterUsable = true,
  onError,
}: Props) {
  const [announce, setAnnounce] = useState("");
  const [effortLine, setEffortLine] = useState("");
  const noteId = useId();
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
      const nextModel = nextView ? roleModel(nextView, role) : undefined;
      // A model with a warning (Fable) is spoken as well as drawn: the standing line below is plain
      // text a listener never lands on by itself.
      setAnnounce(
        [
          roleChangedAnnouncement(role, next, nextModel?.name ?? null),
          modelWarning(next, nextModel),
        ]
          .filter(Boolean)
          .join(" "),
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
      // Only a CHANGE to a model with a warning is spoken (picking the same one again is not news).
      const warning = modelWarning(view.provider, to);
      if (to && warning && from?.id !== to.id) setAnnounce(`${to.name}. ${warning}`);
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
  const storedView = state && isDirect(stored) ? providerView(state, stored) : undefined;
  const issueCtx = {
    id: stored,
    blocked: storedView?.blocked,
    tier: storedView?.tier,
    limit: storedView?.limit,
  };
  const useOpenRouter = (
    <Button
      type="button"
      variant="tertiary"
      size="xs"
      aria-label={`Use OpenRouter for ${who}`}
      onClick={() => void pickProvider("openrouter")}
    >
      Use OpenRouter
    </Button>
  );
  // The issues that are a state of the account and that Check again can end: said in the status
  // region. An empty balance gets the way out only where the provider can't be picked with one.
  const emptyNotOffered = issue === "no_credit" && directInfo(stored)?.noCredit.offered === false;
  const statusIssue =
    issue === "blocked" || issue === "limit_reached" || issue === "no_credit" ? (
      <div className="flex flex-wrap items-center gap-2 text-xs">
        <span className="text-st-due">
          {roleIssueLine(issue, role, providerName(stored), issueCtx)}
        </span>
        {/* Another company is the person's click, never PM's move: a limit stops, and this is the way
            out it offers. */}
        {(issue === "blocked" ||
          (issue === "limit_reached" && openRouterUsable) ||
          emptyNotOffered) &&
          useOpenRouter}
      </div>
    ) : null;
  const model = view ? roleModel(view, role) : undefined;
  const efforts = direct ? effortOptions(role, model) : [];
  const effortValue = view?.roles[role].effort ?? "";
  // The effective model's own warning, standing under its list: the picker draws a row's note only
  // while its list is open, so after Fable is chosen nothing else would say it.
  const modelNote = view ? modelWarning(view.provider, model) : null;
  // Background on a model whose thinking Low switches off says so beside the Select, so the knob is
  // not hidden ("Haiku" from "Claude Haiku 5.5").
  const thinkingLine =
    role === "background" && model?.thinking_off
      ? ` Low also switches ${model.name.replace(/^Claude /, "").split(" ")[0]}'s thinking off.`
      : "";

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

      {/* A paused role says so above; "goes straight to Google" would contradict it. */}
      {direct && view && issue !== "blocked" && issue !== "limit_reached" && (
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
          describedBy={modelNote ? noteId : undefined}
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

      {modelNote && (
        // text-ink4 is the measured 4.5:1 floor the neighbouring lines use. Not folded: it is a
        // warning about the model chosen, and the picker's list is closed by now.
        <p id={noteId} className="text-xs text-ink4">
          {modelNote}
        </p>
      )}

      {direct && efforts.length > 0 && (
        <SettingRow
          label={`${who} effort`}
          description={`${EFFORT_TEXT[role]}${thinkingLine}`}
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
          <span>{roleIssueLine(issue, role, providerName(stored), issueCtx)}</span>
          {useOpenRouter}
        </div>
      )}
      {(issue === "key_missing" || issue === "key_unreadable") && (
        <p className="text-xs text-st-due">
          {roleIssueLine(issue, role, providerName(stored), issueCtx)}
        </p>
      )}
      {/* A status and not an alert: it is a state of the account, not something that just failed.
          The region stays mounted (for a role that can change, like the one below) so a line that
          appears later, after Check again, is announced. A paused provider, and a prepaid key with
          nothing left, can't be picked for a role at all, so a role already on one gets the way out
          beside the line. */}
      {live ? (
        <div role="status">{statusIssue}</div>
      ) : (
        statusIssue && <div role="status">{statusIssue}</div>
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
