// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useRef, useState } from "react";

import { msUntilResume, todayLine } from "../../lib/dailyLimit";
import { dayMonthYear, directInfo, isDirect } from "../../lib/directProviders";
import { formatWhen } from "../../lib/format";
import {
  costSummary,
  getLocalLlmConfig,
  hasOpenRouterBackgroundKey,
  hasOpenRouterKey,
  importAiMemory,
  openrouterLimitState,
  refreshPricing,
  routeReset,
  setOpenRouterBackgroundKey,
  setOpenRouterKey,
  settingsDefaults,
} from "../../lib/ipc";
import { useDirectProviders } from "../../lib/useDirectProviders";
import { atDefaultCloudRows } from "../../lib/roleRoute";
import { useRoutes } from "../../lib/useRoutes";
import type { CostSummary, LimitView, RoleName, RouteView, Settings } from "../../lib/types";
import {
  atLeast,
  detailLine,
  fmtUsd,
  listWalksLine,
  partialTitle,
  unpricedWarning,
  viaLabel,
} from "../../lib/usageFormat";
import { IngestProgress } from "../IngestProgress";
import { modText } from "../mods/modText";
import {
  Button,
  Callout,
  Input,
  SectionInfo,
  SectionLabel,
  Textarea,
  Toggle,
  useFieldA11y,
  VisuallyHidden,
} from "../ui";
import { DailyLimitControl } from "./DailyLimitControl";
import { DirectProviders } from "./DirectProviders";
import { RoleRoute } from "./RoleRoute";
import { TabResetSection } from "./ResetControls";
import { LocalAiOffRow, LocalAiPickPrompt } from "./LocalAiOffControls";
import { readReviewAiEnabled, writeReviewAiEnabled } from "../../lib/reviewPrefs";
import { useLocalAiSwitch } from "../../lib/localAiSwitch";
import { isLoopback } from "../localai/readiness";

/** Whether a role's ordered list is the one a fresh install has (the same cloud rows in the same
 *  order; this computer is the person's own choice and never what a reset is about): a reset is
 *  offered only when it isn't. Until both are read there is nothing to reset. */
function isDefaultRoute(route: RouteView | null, defaults: RouteView | undefined): boolean {
  if (!route || !defaults) return true;
  return atDefaultCloudRows(route, defaults);
}

/** What mods spent, all time, from the by-mod rows: the sum of what could be priced, and whether
 *  some could not (then the sum is a floor, never presented as the whole). PM says which rows are
 *  incomplete (`partial`); a mod that only used a model on this computer cost nothing and is not. */
function modTotal(rows: CostSummary["by_mod"]): { usd: number; partial: boolean } {
  let usd = 0;
  let partial = false;
  for (const r of rows) {
    if (r.partial) partial = true;
    if (r.cost_usd != null) usd += r.cost_usd;
  }
  return { usd, partial };
}

/** The copy-paste prompt handed to another AI to produce a clean, importable memory export. Kept
 *  factual + durable, and explicitly excludes anything sensitive. */
const IMPORT_PROMPT =
  "Summarise everything you know about me and how I like to work, communicate, and organise things " +
  "as a plain list of short factual statements, one per line. Include only durable preferences and " +
  "facts about me — not our conversation, and nothing sensitive like passwords, financial details, " +
  "or health information.";

/** The AI & Models Settings tab. Self-contained and immediate-save: model lists and auto-switch
 *  persist the moment you change them; the API keys save when you click away from the field, with a
 *  green confirmation. Errors surface inline. Onboarding has its own key + model inputs (they share a
 *  Get-started button there) — this is the non-onboarding tab. `onOpenTeach` opens the Teach tab so
 *  the user can review freshly-imported preferences, and `onOpenLocalAi` the Local AI tab, which owns
 *  the other half of model choice (see the pointer under the model lists). */
export function AiModelsSettings({
  onOpenTeach,
  onOpenLocalAi,
}: {
  onOpenTeach?: () => void;
  onOpenLocalAi?: () => void;
}) {
  const [memoryText, setMemoryText] = useState("");
  const [importing, setImporting] = useState(false);
  const [importResult, setImportResult] = useState<string | null>(null);
  const [importError, setImportError] = useState<string | null>(null);
  const [promptCopied, setPromptCopied] = useState(false);
  const [key, setKey] = useState("");
  const [bgKey, setBgKey] = useState("");
  const [keyAlreadySet, setKeyAlreadySet] = useState(false);
  const [bgKeyAlreadySet, setBgKeyAlreadySet] = useState(false);
  const [keySaved, setKeySaved] = useState(false);
  const [bgKeySaved, setBgKeySaved] = useState(false);
  // The out-of-the-box defaults (one backend source), for the per-role + per-tab reset affordances.
  const [defaults, setDefaults] = useState<Settings | null>(null);
  const [cost, setCost] = useState<CostSummary | null>(null);
  const [refreshingPrices, setRefreshingPrices] = useState(false);
  // OpenRouter's daily limit, read from Rust (it has no card of its own; the three direct providers'
  // ride in the own-keys state). null until it answers, or if it can't: then no control is drawn.
  const [orLimit, setOrLimit] = useState<LimitView | null>(null);
  // Whether Review asks the model to suggest a project/tags/importance per item. A per-device frontend
  // gate (localStorage), shared with the Review tab's "Turn on AI" banner — off by default.
  const [reviewAi, setReviewAi] = useState(readReviewAiEnabled);
  const [error, setError] = useState<string | null>(null);
  // The Local AI switch (D7): while it is off, this tab carries the way back on and a cloud model
  // for any role that was routed to a local one. `null` until it answers, which shows neither.
  const localAi = useLocalAiSwitch();
  // The providers a person reaches with their own key, shared by the "Your own keys" card and both
  // role blocks so they never disagree about what is on, keyed, or moved.
  const dp = useDirectProviders();
  // Each role's ordered list and every row's status (the lists are Rust's; this only draws them).
  const routes = useRoutes();
  // The model chosen on this computer for each role, which is what "This computer" in a list means.
  // None while Local AI is off (nothing local can answer then) and until the read lands.
  const [localModels, setLocalModels] = useState<{
    chat: string | null;
    background: string | null;
  }>({ chat: null, background: null });
  const localOn = localAi?.on === true;
  // Local AI points at a server on another machine: the picker then says "Your Local AI server",
  // never "This computer" (the rows, the card and the strip say the same, by the same loopback rule).
  const [localElsewhere, setLocalElsewhere] = useState(false);
  // A provider turned off, a key removed or a key saved changes the lists in Rust (its rows leave, or
  // a row's status moves): whenever the own-keys card re-reads, so do the lists.
  const { reload: reloadRoutes } = routes;
  useEffect(() => {
    if (dp.state) void reloadRoutes();
  }, [dp.state, reloadRoutes]);
  useEffect(() => {
    if (!localOn) {
      setLocalModels({ chat: null, background: null });
      setLocalElsewhere(false);
      return;
    }
    let cancelled = false;
    getLocalLlmConfig()
      .then((c) => {
        if (!cancelled) {
          setLocalModels({
            chat: c.chat_model?.trim() || null,
            background: c.background_model?.trim() || null,
          });
          setLocalElsewhere(!!c.base_url?.trim() && !isLoopback(c.base_url));
        }
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [localOn]);
  // The two key fields' label/control wiring. Both labels sat above their Input with no htmlFor, so
  // the placeholder was the last-resort accessible name: the most consequential input in PM
  // announced as "sk-or-... , password", and lost even that the moment typing started.
  const keyField = useFieldA11y();
  const bgKeyField = useFieldA11y({ description: true });

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const [hasKey, hasBgKey, defs] = await Promise.all([
          hasOpenRouterKey(),
          hasOpenRouterBackgroundKey(),
          settingsDefaults(),
        ]);
        if (cancelled) return;
        setKeyAlreadySet(hasKey);
        setBgKeyAlreadySet(hasBgKey);
        setDefaults(defs);
      } catch (e) {
        if (!cancelled) setError(String(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Cost summary loads on its own (its first read may trigger a daily pricing fetch, which the backend
  // makes only with an OpenRouter key stored), so it never blocks the keys/models from showing.
  const loadCost = useCallback(() => {
    costSummary()
      .then(setCost)
      .catch(() => {});
  }, []);
  useEffect(loadCost, [loadCost]);

  // OpenRouter's limit state: read on opening and after every write to it. A read that fails leaves
  // the control out rather than showing an invented "off".
  const loadOrLimit = useCallback(async () => {
    try {
      setOrLimit(await openrouterLimitState());
    } catch {
      /* the control just isn't drawn */
    }
  }, []);
  useEffect(() => {
    void loadOrLimit();
  }, [loadOrLimit]);
  // A pause lifts at midnight on its own: an open page reads it again then (as the own-key cards do).
  useEffect(() => {
    const wait = msUntilResume([orLimit], Date.now());
    if (wait == null) return;
    const t = setTimeout(() => void loadOrLimit(), wait);
    return () => clearTimeout(t);
  }, [orLimit, loadOrLimit]);

  // The green "Saved" confirmations fade a couple of seconds after a key is stored.
  useEffect(() => {
    if (!keySaved) return;
    const t = setTimeout(() => setKeySaved(false), 2500);
    return () => clearTimeout(t);
  }, [keySaved]);
  useEffect(() => {
    if (!bgKeySaved) return;
    const t = setTimeout(() => setBgKeySaved(false), 2500);
    return () => clearTimeout(t);
  }, [bgKeySaved]);

  // Keys save the moment you click away — no Save button. A blank field is a no-op (so tabbing past an
  // already-saved key never wipes it); a real value is stored in the OS keychain and the field clears
  // to its masked "saved" placeholder with a green confirmation.
  async function saveKey() {
    const trimmed = key.trim();
    if (!trimmed) return;
    setError(null);
    try {
      await setOpenRouterKey(trimmed);
      // The first key saved here is what lets the price refresh run (`ensure_pricing_fresh`).
      loadCost();
      setKey("");
      setKeyAlreadySet(true);
      setKeySaved(true);
    } catch (e) {
      setError(String(e));
    }
  }
  async function saveBgKey() {
    const trimmed = bgKey.trim();
    if (!trimmed) return;
    setError(null);
    try {
      await setOpenRouterBackgroundKey(trimmed);
      // The first key saved here is what lets the price refresh run (`ensure_pricing_fresh`).
      loadCost();
      setBgKey("");
      setBgKeyAlreadySet(true);
      setBgKeySaved(true);
    } catch (e) {
      setError(String(e));
    }
  }

  function changeReviewAi(on: boolean) {
    setReviewAi(on);
    writeReviewAiEnabled(on); // shared with the Review tab's "Turn on AI" banner
  }

  function copyPrompt() {
    void navigator.clipboard?.writeText(IMPORT_PROMPT).then(() => {
      setPromptCopied(true);
      setTimeout(() => setPromptCopied(false), 1500);
    });
  }

  const memoryRef = useRef<HTMLTextAreaElement>(null);

  async function doImportMemory() {
    if (!memoryText.trim() || importing) return;
    setImporting(true);
    setImportResult(null);
    setImportError(null);
    try {
      const n = await importAiMemory(memoryText);
      setImportResult(
        n === 0
          ? "No new preferences found — anything importable was already saved."
          : `${n} preference${n === 1 ? "" : "s"} imported. Review and keep them in Teach.`,
      );
      setMemoryText("");
    } catch (e) {
      setImportError(String(e));
    } finally {
      setImporting(false);
      // The button that had focus is swapped out while the import runs, which would otherwise drop
      // focus to <body> and lose a keyboard user's place mid-operation.
      memoryRef.current?.focus();
    }
  }

  async function refreshPrices() {
    setRefreshingPrices(true);
    setError(null);
    try {
      setCost(await refreshPricing());
    } catch (e) {
      setError(String(e));
    } finally {
      setRefreshingPrices(false);
    }
  }

  // A role is at its default when its ordered list is the one a fresh install has. Until the
  // defaults load, nothing is offered a reset. Reset writes that list as the role's own, through the
  // same one list writer as every other edit: it narrows toward zero data retention (OpenRouter's
  // default row), so it can only ever lose a choice, never widen where data goes. It turns no
  // provider off and deletes no key.
  const roles: RoleName[] = ["chat", "background"];
  const chatRoleIsDefault = isDefaultRoute(routes.chat, defaults?.chat_route);
  const backgroundRoleIsDefault = isDefaultRoute(routes.background, defaults?.background_route);
  async function resetRoles() {
    for (const role of roles) {
      // A role already on the default list is left alone: nothing is stored for a role nobody edited.
      if (role === "chat" ? chatRoleIsDefault : backgroundRoleIsDefault) continue;
      const fresh = await routeReset(role);
      routes.set(role, fresh.route);
    }
  }

  // The cloud model each role will really use, for the Local AI prompt: the first cloud row of its
  // list.
  function cloudModelOf(role: RoleName): string | null {
    const route = role === "chat" ? routes.chat : routes.background;
    return route?.entries.find((e) => e.kind === "cloud")?.model ?? null;
  }
  const orKey = { chat: keyAlreadySet, background: keyAlreadySet || bgKeyAlreadySet };
  // An edit answered with the list as it now is. The own-keys card reads which provider each role is
  // on from the same list, so it is read again too.
  function onRoute(role: RoleName, route: RouteView) {
    routes.set(role, route);
    void dp.reload();
  }
  // "Refresh prices" asks OpenRouter for its list, which a person using only their own keys has
  // never opted into: hide it for them, and only them.
  const noOpenRouterAtAll =
    dp.state !== null &&
    !keyAlreadySet &&
    !bgKeyAlreadySet &&
    isDirect(dp.state.roles.chat.effective) &&
    isDirect(dp.state.roles.background.effective);
  const catalogueDate = dp.state?.providers[0]?.catalogue_checked_on ?? null;
  // One "Today: $1.20 of $5.00" line per provider whose limit is on, OpenRouter first.
  const todayLines = [
    orLimit ? todayLine("OpenRouter", orLimit) : null,
    ...(dp.state?.providers ?? []).map((p) =>
      todayLine(directInfo(p.provider)?.name ?? p.provider, p.limit),
    ),
  ].filter((l): l is string => l !== null);
  // Gemini's cost copy is only worth reading by someone who has turned it on or kept a key.
  const geminiInUse =
    dp.state?.providers.find(
      (p) => p.provider === "gemini" && (p.opted_in || p.key === "present"),
    ) ?? null;
  const anthropicInUse = dp.state?.providers.some(
    (p) => p.provider === "anthropic" && (p.opted_in || p.key === "present"),
  );

  return (
    <>
      {error && <Callout className="mt-4">{error}</Callout>}

      {/* This label doubles as the AI tab's scroll-spy anchor — `useScrollSpy` (SettingsView:151)
          and `scrollToSection` both query `#sec-ai-keys`. So `labelProps` is NOT spread here: its
          `id` would overwrite that anchor and silently break the sub-nav highlight. `htmlFor` does
          the association, and `aria-labelledby` is re-pointed at the anchor id — leaving the hook's
          own labelId in place would name the field after an element that is not in the DOM, which
          resolves to an EMPTY name rather than falling back. */}
      <label
        id="sec-ai-keys"
        data-settings-section
        htmlFor={keyField.labelProps.htmlFor}
        className="mt-5 block text-sm font-medium text-ink2"
      >
        OpenRouter API key
      </label>
      <Input
        {...keyField.controlProps}
        aria-labelledby="sec-ai-keys"
        type="password"
        autoComplete="off"
        data-help="settings-api-key"
        value={key}
        onChange={(e) => setKey(e.target.value)}
        onBlur={() => void saveKey()}
        placeholder={keyAlreadySet ? "•••••••• (saved — type to replace)" : "sk-or-..."}
        className="mt-1"
      />
      {keySaved ? (
        <p className="mt-1 text-xs font-medium text-st-track">✓ Saved to your keychain</p>
      ) : (
        <a
          href="https://openrouter.ai/keys"
          target="_blank"
          rel="noreferrer"
          className="mt-1 inline-block text-xs text-ink4 hover:text-ink2"
        >
          Get a key at openrouter.ai/keys →
        </a>
      )}

      <label {...bgKeyField.labelProps} className="mt-4 block text-sm font-medium text-ink2">
        Background API key
      </label>
      <Input
        {...bgKeyField.controlProps}
        type="password"
        autoComplete="off"
        data-help="settings-background-key"
        value={bgKey}
        onChange={(e) => setBgKey(e.target.value)}
        onBlur={() => void saveBgKey()}
        placeholder={bgKeyAlreadySet ? "•••••••• (saved — type to replace)" : "sk-or-..."}
        className="mt-1"
      />
      <p {...bgKeyField.descriptionProps} className="mt-1 text-xs text-ink4">
        Only used while Background is on OpenRouter.
      </p>
      {bgKeySaved && (
        <p className="mt-1 text-xs font-medium text-st-track">✓ Saved to your keychain</p>
      )}
      {/* Both key fields' explanation in one disclosure at the foot of the pair —
          including the keychain sentence that used to head every tab. */}
      <SectionInfo title="About your API keys">
        <p>
          Your OpenRouter key lives in your computer&apos;s keychain and saves as soon as you click
          away. You can swap the model any time.
        </p>
        <p>
          The background key is used for background work (sorting proposals, learning). Lets you
          track that spend separately. Falls back to your main key if blank.
        </p>
      </SectionInfo>

      {/* The limit lives beside the key it brakes. Drawn for a person who uses OpenRouter, or who
          already has a limit on it (a restore can bring one onto a machine with no key yet). */}
      {orLimit && (keyAlreadySet || bgKeyAlreadySet || orLimit.limit_usd != null) && (
        <DailyLimitControl
          provider="openrouter"
          name="OpenRouter"
          limit={orLimit}
          onChanged={loadOrLimit}
          onError={setError}
        />
      )}

      <DirectProviders dp={dp} orKey={orKey} onError={setError} />

      <div
        id="sec-ai-models"
        data-settings-section
        className="mt-5 space-y-5 border-t border-border pt-4"
      >
        {/* First in the section, above the cloud models it offers: the reader who just switched
            Local AI off lands here, and this is what they are being asked to do. */}
        {localAi && (
          <LocalAiPickPrompt
            sw={localAi}
            chatModel={routes.chat ? cloudModelOf("chat") : null}
            backgroundModel={routes.background ? cloudModelOf("background") : null}
            onError={setError}
          />
        )}
        <RoleRoute
          role="chat"
          route={routes.chat}
          dp={dp.state}
          openRouterKey={orKey.chat}
          local={{ on: localOn, model: localModels.chat, elsewhere: localElsewhere }}
          helpId="settings-chat-models"
          onRoute={onRoute}
          onChanged={() => void routes.reload()}
          onError={setError}
          defaultRoute={defaults?.chat_route}
          failed={routes.failed}
        />
        <RoleRoute
          role="background"
          route={routes.background}
          dp={dp.state}
          openRouterKey={orKey.background}
          local={{ on: localOn, model: localModels.background, elsewhere: localElsewhere }}
          helpId="settings-background-models"
          onRoute={onRoute}
          onChanged={() => void routes.reload()}
          onError={setError}
          defaultRoute={defaults?.background_route}
          failed={routes.failed}
        />
        {/* A pointer, deliberately not a mirror: this computer's model, server and battery rules keep
         *  one home, in the Local AI tab. Where this computer SITS in each list is set here, in the
         *  lists above, and the Local AI tab says so the other way round. While Local AI is off that
         *  tab is gone, and the pointer becomes the switch's one way back on (its home while on is
         *  the top of the tab, so it is never in two places at once). */}
        {localAi?.on && (
          <p className="text-xs text-ink4">
            To use a model on your own computer, add This computer to a list above. Which model it
            is, your server and the On battery rules are set separately.{" "}
            {onOpenLocalAi && (
              <button
                type="button"
                onClick={onOpenLocalAi}
                className="text-accent-text hover:brightness-110"
              >
                Open Local AI →
              </button>
            )}
          </p>
        )}
        {localAi && !localAi.on && (
          <div>
            <LocalAiOffRow sw={localAi} onTurnedOn={onOpenLocalAi} onError={setError} />
          </div>
        )}
      </div>

      <div id="sec-ai-review" data-settings-section className="mt-5 border-t border-border pt-4">
        <div className="flex items-center justify-between gap-3">
          <div className="min-w-0">
            <SectionLabel>Filing suggestions</SectionLabel>
            <p className="mt-1 text-xs text-ink4">
              Let PM propose a project, tags and importance for each new item in Review — a real
              help when you're importing a lot. Off by default; uses the background model above. You
              can also turn this on from the banner in Review.
            </p>
          </div>
          <Toggle
            checked={reviewAi}
            onChange={changeReviewAi}
            ariaLabel="AI filing suggestions in Review"
          />
        </div>
      </div>

      <div id="sec-ai-memory" data-settings-section className="mt-5 border-t border-border pt-4">
        <SectionLabel htmlFor="ai-memory-paste">Import AI memory</SectionLabel>
        <p className="mt-1 text-xs text-ink4">
          Bringing preferences over from ChatGPT, Gemini or Claude? Copy the prompt below, run it in
          that AI, then paste its reply here. PM turns it into preference records you review and
          keep in Teach — nothing takes effect until you keep it. Uses the background model above.
        </p>

        <div className="mt-3 rounded-[var(--radius-sm)] border border-border2 bg-surface p-2">
          <div className="flex items-start justify-between gap-2">
            <p className="min-w-0 flex-1 text-xs text-ink3">{IMPORT_PROMPT}</p>
            <Button variant="tertiary" size="sm" className="shrink-0" onClick={copyPrompt}>
              {promptCopied ? "Copied ✓" : "Copy"}
            </Button>
          </div>
        </div>

        <Textarea
          id="ai-memory-paste"
          ref={memoryRef}
          className="mt-2 h-48 w-full text-xs"
          placeholder="Paste the AI's reply here…"
          value={memoryText}
          onChange={(e) => setMemoryText(e.currentTarget.value)}
        />
        <div className="mt-2 flex flex-wrap items-center gap-2">
          {/* Swap the button out for the bar rather than dimming it in place — a 40%-opacity button
              whose label grows to "Importing…" reads as a second, ghosted control. `total={null}` is
              the honest tier: distilling is one non-streaming model call, so there is no count to
              report and a percentage would be invented. */}
          {importing ? (
            <div className="w-full max-w-xs">
              <IngestProgress processed={0} total={null} label="Importing AI memory" />
              <p className="mt-1 text-xs text-ink4">
                Reading your export with the background model — this usually takes a few seconds.
              </p>
            </div>
          ) : (
            <Button
              variant="secondary"
              onClick={() => void doImportMemory()}
              disabled={!memoryText.trim()}
            >
              Import
            </Button>
          )}
          {importError && <span className="text-xs text-st-due">{importError}</span>}
          {importResult && (
            <span className="flex items-center gap-1.5 text-xs text-ink3">
              {importResult}
              {onOpenTeach && (
                <button
                  type="button"
                  onClick={onOpenTeach}
                  className="text-accent-text hover:brightness-110"
                >
                  Open Teach →
                </button>
              )}
            </span>
          )}
          {/* The visible result/error lines are not announced on their own — this is what a screen
              reader hears when the import starts and finishes. */}
          <VisuallyHidden role="status" aria-live="polite">
            {importing ? "Importing AI memory" : (importError ?? importResult ?? "")}
          </VisuallyHidden>
        </div>
      </div>

      {cost && (
        <div
          id="sec-ai-usage"
          data-settings-section
          className="mt-5 border-t border-border pt-4"
          data-help="settings-usage-cost"
        >
          <SectionLabel
            action={
              noOpenRouterAtAll ? undefined : (
                <Button
                  variant="tertiary"
                  size="sm"
                  onClick={refreshPrices}
                  disabled={refreshingPrices}
                >
                  {refreshingPrices ? "Refreshing…" : "Refresh prices"}
                </Button>
              )
            }
          >
            Usage &amp; cost
          </SectionLabel>
          {/* The "Spend & breakdown" Collapsible is gone: it hid your own numbers
              behind a caret (and unfolded them only for Power), while its `meta` slot
              leaked the 30d total back out to prove the point. The totals and the
              per-model table are readouts — the thing you opened this section for — so
              they stay visible; only the pricing methodology folds away below. */}
          <div className="mt-3 flex gap-6 text-sm">
            <div>
              <div className="text-xs text-ink4">Last 30 days</div>
              <div
                className="font-mono text-ink2"
                title={partialTitle(cost.total_30d_usd, cost.total_30d_partial)}
              >
                {atLeast(cost.total_30d_usd, cost.total_30d_partial)}
              </div>
            </div>
            <div>
              <div className="text-xs text-ink4">All time</div>
              <div
                className="font-mono text-ink2"
                title={partialTitle(cost.total_all_time_usd, cost.total_all_time_partial)}
              >
                {atLeast(cost.total_all_time_usd, cost.total_all_time_partial)}
              </div>
            </div>
          </div>
          {/* A read-out of each limit that is on: today against it. The control lives with the key it
              brakes; this only reads. "At least" appears only when some of today's calls had no
              price (the same count the totals use). */}
          {todayLines.length > 0 && (
            <ul
              className="mt-2 space-y-0.5 text-xs text-ink3"
              aria-label="Today against your daily limits"
            >
              {todayLines.map((line) => (
                <li key={line}>{line}</li>
              ))}
            </ul>
          )}
          {/* Status, so it is never folded: how many calls the 30-day figure leaves out, and the
              named token types PM can't price. The "Why?" beside it is the explanation. */}
          {/* The count covers the last 30 days only; the "Why?" shows whenever either total says "at
              least", because an all-time floor can come from calls older than that. */}
          {cost.unpriced_30d > 0 && (
            <p className="mt-1 text-xs text-ink3">
              {cost.unpriced_30d === 1
                ? "1 call in the last 30 days couldn't be priced in full"
                : `${cost.unpriced_30d} calls in the last 30 days couldn't be priced in full`}
            </p>
          )}
          {/* Status, never folded: the record of replies a later row of the list answered. Background
              and Review have no strip, so this is where a bad first row would otherwise hide. */}
          {cost.list_walks && (
            <p className="mt-1 text-xs text-ink3" data-testid="list-walks">
              {listWalksLine(cost.list_walks)}
            </p>
          )}
          {(cost.unpriced_30d > 0 || cost.total_30d_partial || cost.total_all_time_partial) && (
            <SectionInfo title="Why?">
              <p>
                Some replies don&apos;t say what they cost: ones that came back with no token count,
                ones PM can only price in part, or ones that used a kind of token PM doesn&apos;t
                know how to price. Your provider&apos;s own usage page has the exact figure.
              </p>
            </SectionInfo>
          )}
          {cost.partial_reasons.map((r) => {
            const words = unpricedWarning(r.slug, r.replies);
            return words ? (
              <Callout key={r.slug} tone="warning" body="ink" live={false} className="mt-2">
                {words}
              </Callout>
            ) : null;
          })}
          {cost.all_time.length > 0 ? (
            <div className="mt-3">
              <p className="pb-1 font-mono text-[0.625rem] uppercase tracking-wide text-ink4">
                By model · most expensive first (all time)
              </p>
              <table className="w-full text-left text-xs" data-help="settings-usage-via">
                <thead className="font-mono uppercase tracking-wide text-ink4">
                  <tr className="border-b border-rule">
                    <th className="py-1 font-medium">Model</th>
                    <th className="py-1 font-medium">Via</th>
                    <th className="py-1 text-right font-medium">Reqs</th>
                    <th className="py-1 text-right font-medium">Tokens in/out</th>
                    <th className="py-1 text-right font-medium">Cost</th>
                  </tr>
                </thead>
                <tbody>
                  {cost.all_time.map((s) => {
                    const detail = detailLine(s);
                    return (
                      <tr key={`${s.model}|${s.served ?? ""}`} className="border-b border-rule">
                        <td className="py-1 pr-2 text-ink2">
                          {s.model}
                          {detail && <div className="text-ink4">{detail}</div>}
                        </td>
                        <td className="py-1 pr-2 text-ink3">{viaLabel(s.served)}</td>
                        <td className="py-1 text-right text-ink3">{s.request_count}</td>
                        <td className="py-1 text-right font-mono text-ink4">
                          {s.prompt_tokens.toLocaleString()} /{" "}
                          {s.completion_tokens.toLocaleString()}
                        </td>
                        <td
                          className="py-1 text-right font-mono text-ink3"
                          title={partialTitle(s.cost_usd, s.partial)}
                        >
                          {s.served === "local" ? "No cost" : atLeast(s.cost_usd, s.partial)}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          ) : (
            <p className="mt-2 text-xs text-ink4">No model calls logged yet.</p>
          )}
          {cost.by_mod.length > 0 && (
            <div className="mt-3" data-help="settings-usage-by-mod">
              <p className="pb-1 font-mono text-[0.625rem] uppercase tracking-wide text-ink4">
                By mod (all time)
              </p>
              <table className="w-full text-left text-xs">
                <thead className="font-mono uppercase tracking-wide text-ink4">
                  <tr className="border-b border-rule">
                    <th className="py-1 font-medium">Mod</th>
                    <th className="py-1 text-right font-medium">Reqs</th>
                    <th className="py-1 text-right font-medium">Tokens in/out</th>
                    <th className="py-1 text-right font-medium">Cost</th>
                  </tr>
                </thead>
                <tbody>
                  {cost.by_mod.map((m) => (
                    <tr key={m.mod_id} className="border-b border-rule">
                      {/* The name is the mod's own text, made one plain line; a mod that is gone
                          keeps its history under PM's words. */}
                      <td className="break-words py-1 pr-2 text-ink2">
                        {m.name == null ? "removed mod" : modText(m.name)}
                      </td>
                      <td className="py-1 text-right text-ink3">{m.request_count}</td>
                      <td className="py-1 text-right font-mono text-ink4">
                        {m.prompt_tokens.toLocaleString()} / {m.completion_tokens.toLocaleString()}
                      </td>
                      <td
                        className="py-1 text-right font-mono text-ink3"
                        title={partialTitle(m.cost_usd, m.partial)}
                      >
                        {/* A mod that only used models on this computer cost nothing (a dash would
                            read as "couldn't be priced"). */}
                        {m.cost_usd == null && !m.partial
                          ? "No cost"
                          : atLeast(m.cost_usd, m.partial)}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
              <p className="mt-1 text-xs text-ink4">
                Mods together:{" "}
                <span className="font-mono text-ink3">
                  {modTotal(cost.by_mod).partial ? "at least " : ""}
                  {fmtUsd(modTotal(cost.by_mod).usd)}
                </span>
                . Their calls are in the model table above too. Each mod&apos;s monthly limit, in
                the Mods tab, counts its own ledger, so it can differ from this log.
              </p>
            </div>
          )}
          {/* Names a tab that only exists while Local AI is on. The table above lists the models on
              this computer (the log keeps them), so this says what that listing means. */}
          {localAi?.on && (
            <p className="mt-2 text-xs text-ink4">
              Models on this computer are listed with their request count. They don&apos;t cost
              anything.
            </p>
          )}
          <SectionInfo title="How this is calculated">
            <p>
              Your real per-call cost as reported by OpenRouter where available, otherwise estimated
              from the tokens each call used × OpenRouter&apos;s per-token price
              {cost.pricing_updated_at
                ? ` (prices updated ${formatWhen(cost.pricing_updated_at)})`
                : ""}
              .
            </p>
            <p>
              Each model reply reports the tokens it used (your prompt + its reply). PM logs those
              per call. Once an OpenRouter key is saved, it fetches OpenRouter&apos;s public price
              list about once a day and caches it — no extra model call, and your API key is never
              used for it. Without a key it updates these prices only when you press Refresh prices
              (opening the model list to add a model still loads OpenRouter&apos;s public
              catalogue).
            </p>
            <p>
              Where OpenRouter reports a call&apos;s actual cost (reflecting any prompt-cache
              discount) PM shows that; for older calls without it, cost = prompt&nbsp;tokens ×
              prompt&nbsp;price + reply&nbsp;tokens × reply&nbsp;price. It&apos;s computed when you
              open this page, so a later price change re-prices your history. A model with no
              reported cost and not yet in the price cache shows{" "}
              <span className="font-mono text-ink4">—</span>, never an understated&nbsp;$0.
            </p>
            <p>
              Cached tokens are billed at a lower price and thinking tokens are billed as output; PM
              shows both so a total makes sense. A figure that says &ldquo;at least&rdquo; leaves
              out calls PM couldn&apos;t price. A reply that failed after the provider began billing
              it, and anything done with a key outside PM, isn&apos;t in this log at all.
            </p>
            {catalogueDate && (
              <p>
                Calls that go straight to your own key aren&apos;t priced by the provider. PM works
                the cost out from the tokens the provider reports times the prices built into this
                version of PM (checked on {dayMonthYear(catalogueDate)}). Cached input is priced at
                the cached rate. It leaves out tax and anything the provider adds. Check your
                provider&apos;s own dashboard for the bill.
              </p>
            )}
            {anthropicInUse && (
              <p>
                Writing to the cache costs a little more than plain input. On Claude Haiku, a
                request with more than 100,000 tokens of input costs five times as much per token.
                If Anthropic stops, cuts off or errors a reply partway, it still bills what it had
                written; PM can&apos;t see those tokens, so they aren&apos;t counted here.
              </p>
            )}
            {geminiInUse && (
              <p>
                Gemini&apos;s replies report thinking in a total, not in the output count, so PM
                works the output out as the total minus the prompt. A free Gemini key costs nothing,
                so PM shows $0.00 for free-tier models while its last check says the key is free. If
                you&apos;ve since upgraded the project, press Check again; until then, or until a
                paid-only model answers, the figure can be too low. Some prices change on a set date
                (checked {dayMonthYear(geminiInUse.catalogue_checked_on)}); PM&apos;s list knows the
                dates.
              </p>
            )}
          </SectionInfo>
        </div>
      )}

      <TabResetSection
        tabName="AI & Models"
        isDefault={chatRoleIsDefault && backgroundRoleIsDefault && !reviewAi}
        onReset={async () => {
          changeReviewAi(false);
          try {
            await resetRoles();
          } catch (e) {
            // Like the tab's other writes: say so instead of an unhandled rejection.
            setError(String(e));
          }
          await dp.reload();
          await routes.reload();
        }}
        confirmBody={
          <>
            Puts the chat and background lists back to PM&apos;s default model on OpenRouter, and
            turns Review filing suggestions back off. Your saved API keys, and which providers
            you&apos;ve turned on, aren&apos;t affected. Nor are your daily limits: they only ever
            hold spending back, so a reset leaves them as they are.
          </>
        }
      />
    </>
  );
}
