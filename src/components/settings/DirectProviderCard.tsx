// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useId, useRef, useState, type FormEvent } from "react";

import {
  dayMonthYear,
  flippedLine,
  keyVerdict,
  movedLine,
  notThereLine,
  offersRecheckFor,
  pausedText,
  recheckStores,
  recheckVerdict,
  refusalText,
  roleName,
  storedVerdict,
  termsFor,
  tierBadge,
  type DirectProviderInfo,
} from "../../lib/directProviders";
import {
  clearDirectKey,
  recheckDirectKey,
  saveDirectKey,
  setDirectDataSharing,
  setDirectOptIn,
} from "../../lib/ipc";
import type {
  DirectProvidersState,
  DirectProviderView,
  KeyCheckResult,
  RegionRefusal,
  RoleName,
} from "../../lib/types";
import {
  Button,
  Callout,
  ConfirmDialog,
  Field,
  Input,
  SectionInfo,
  SettingRow,
  Toggle,
  VisuallyHidden,
} from "../ui";
import { DailyLimitControl } from "./DailyLimitControl";

interface Props {
  info: DirectProviderInfo;
  view: DirectProviderView;
  /** Read the state again after a write. The card never works out a move or a verdict itself. */
  reload: () => Promise<DirectProvidersState | null>;
  /** Whether each role has an OpenRouter key to land on if it is moved back, for the sentence that
   *  says so. */
  orKey: { chat: boolean; background: boolean };
  onError?: (message: string | null) => void;
  /** Called once a key has been checked and kept, with the state read after it. The first-run
   *  wizard uses it to point the roles at the provider when there is no OpenRouter key. */
  onKeySaved?: (result: KeyCheckResult, state: DirectProvidersState | null) => void;
}

/**
 * One provider the person reaches with their own key (opt-in, off by default).
 *
 * Everything about the choice is said where it is made: the not-zero-data-retention warning is
 * unfolded the moment the toggle is on, a key is CHECKED before it is kept (so a mistyped one is
 * never stored), and what happened to a role that was on this provider stays on screen until the
 * next change. The long terms fold behind one disclosure at the foot. Nothing here ever holds a
 * stored key: the webview learns presence and a verdict, and a typed key goes once to Rust.
 *
 * The key is a SUBMIT, unlike the OpenRouter pair (which save on blur): it makes a network call and
 * has verdicts, and blur would fire a check on every tab-through and on a half-pasted key.
 */
export function DirectProviderCard({ info, view, reload, orKey, onError, onKeySaved }: Props) {
  const name = info.name;
  const [typed, setTyped] = useState("");
  const [checking, setChecking] = useState(false);
  // The last check made from this card. It stays until the next edit: a result that vanished on a
  // timer would not be announced reliably and could not be read again.
  const [result, setResult] = useState<KeyCheckResult | null>(null);
  // Whether that result came from Check again: the key was already saved, so a refusal is not
  // "Not saved", and it is told in the status line rather than as a field error. (An error the
  // command throws, such as "no key to check", still shows under the field.)
  const [again, setAgain] = useState(false);
  const [refusal, setRefusal] = useState<string | null>(null);
  // The region refusal on screen when a typed key is sent again. The refusal's own Check again is
  // inside its callout, so the callout must stay mounted while the check runs: a focused button
  // that unmounts drops focus to <body> (WebKitGTK), and its aria-disabled would never show.
  // Cleared by the next result, an edit, or anything that ends the typed key's turn.
  const [heldRefusal, setHeldRefusal] = useState<RegionRefusal | null>(null);
  const [moved, setMoved] = useState<string | null>(null);
  const [announce, setAnnounce] = useState("");
  const [confirming, setConfirming] = useState(false);
  const [focusTick, setFocusTick] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const cardRef = useRef<HTMLDivElement>(null);
  const sharesId = useId();

  // After a key is removed its button unmounts, which would drop focus to <body>. Put it on the key
  // field (where the next key is pasted), or on the switch when the card is off.
  useEffect(() => {
    if (focusTick === 0) return;
    const target =
      inputRef.current ?? cardRef.current?.querySelector<HTMLElement>('[role="switch"]');
    target?.focus();
  }, [focusTick]);

  const hasKey = view.key === "present";

  // A state that changes after a settings re-read (the card turns paused, or Google says the project
  // is free) is announced by the same hidden status line when it goes from empty to set, because a
  // failing chat reply would otherwise be the only signal (N4). Not on first render.
  const flippedAt = view.key_check?.flipped_free_at ?? null;
  const seen = useRef({ blocked: !!view.blocked, flipped: !!flippedAt });
  useEffect(() => {
    const was = seen.current;
    const paused = `${name} is now paused.`;
    const free = "Google says this key is on the free tier.";
    if (view.blocked && !was.blocked) setAnnounce(paused);
    else if (flippedAt && !was.flipped) setAnnounce(free);
    // Cleared when the state ends, so the same words set again later are a change and are read.
    else if (!view.blocked && !flippedAt && (was.blocked || was.flipped)) {
      setAnnounce((a) => (a === paused || a === free ? "" : a));
    }
    seen.current = { blocked: !!view.blocked, flipped: !!flippedAt };
  }, [view.blocked, flippedAt, name]);

  async function toggle(next: boolean) {
    onError?.(null);
    setResult(null);
    setRefusal(null);
    setHeldRefusal(null);
    // The form unmounts when the card goes off but this state doesn't: without this, turning it back
    // on would bring back a half-typed or pasted key as dots.
    if (!next) setTyped("");
    try {
      const moves = await setDirectOptIn(view.provider, next);
      setMoved(next ? null : movedLine(moves, orKey));
      await reload();
      setAnnounce(
        next
          ? hasKey
            ? `${name} is on. This isn't zero data retention.`
            : `${name} is on. Add your key. This isn't zero data retention.`
          : `${name} is off.`,
      );
    } catch (e) {
      onError?.(String(e));
    }
  }

  async function submit(e: FormEvent) {
    e.preventDefault();
    // A form still submits through an aria-disabled button and through Enter in the field, so the
    // guard is here and not on the button.
    if (checking) return;
    const key = typed.trim();
    if (!key) {
      setResult(null);
      setRefusal("Paste your key first.");
      return;
    }
    setChecking(true);
    setResult(null);
    setAgain(false);
    setRefusal(null);
    setAnnounce("Checking your key");
    onError?.(null);
    try {
      const r = await saveDirectKey(view.provider, key);
      setResult(r);
      setHeldRefusal(!r.saved ? (r.refusal ?? null) : null);
      if (r.saved) {
        setTyped("");
        setMoved(null);
        const next = await reload();
        onKeySaved?.(r, next);
      }
      setAnnounce("");
    } catch (err) {
      // A refusal before any check (not turned on, a keychain that can't be written): its own words.
      setRefusal(String(err));
      setHeldRefusal(null);
      setAnnounce("");
    } finally {
      setChecking(false);
    }
  }

  // Check again: the saved key goes to the provider once more (credit added, access given, a chat
  // that couldn't be tried). Same guard and the same announcement as a first check.
  async function checkAgain() {
    if (checking) return;
    setChecking(true);
    setResult(null);
    setHeldRefusal(null);
    setAgain(true);
    setRefusal(null);
    setAnnounce("Checking your key");
    onError?.(null);
    try {
      const r = await recheckDirectKey(view.provider);
      setResult(r);
      await reload();
      setAnnounce("");
      // The button goes away once there is nothing left to recheck, which would drop focus to
      // <body>: put it on the key field instead.
      // Only an answer that was stored can end the button's turn: for trouble reaching the provider
      // the stored verdict stands and the button stays.
      if (recheckStores(r.outcome) && !offersRecheckFor(info, r, r.tier, r.refusal)) {
        setFocusTick((t) => t + 1);
      }
    } catch (err) {
      setRefusal(String(err));
      setAnnounce("");
    } finally {
      setChecking(false);
    }
  }

  function askRemove() {
    if (view.roles.chat.uses_this_provider || view.roles.background.uses_this_provider) {
      setConfirming(true);
    } else {
      void remove();
    }
  }

  async function remove() {
    setConfirming(false);
    onError?.(null);
    try {
      const moves = await clearDirectKey(view.provider);
      setResult(null);
      setRefusal(null);
      setHeldRefusal(null);
      setTyped("");
      setMoved(movedLine(moves, orKey));
      await reload();
      setFocusTick((t) => t + 1);
    } catch (e) {
      onError?.(String(e));
    }
  }

  async function setShares(shares: boolean) {
    onError?.(null);
    try {
      await setDirectDataSharing(view.provider, shares);
      await reload();
    } catch (e) {
      onError?.(String(e));
    }
  }

  // What to say about the key. This session's check first; otherwise the stored one, which is shown
  // only beside a key that is really there.
  const session = result
    ? (again ? recheckVerdict : keyVerdict)(
        result.outcome,
        result.listed.length,
        name,
        result.note,
        {
          info,
          tier: result.tier,
          refusal: again ? result.refusal : null,
        },
      )
    : null;
  // A typed key PM didn't keep because it is free where PM only uses paid ones: the callout below
  // says it all, so the field carries no second, shorter error.
  // While a typed key is being sent again the refusal that is on screen stays (held), so the button
  // that was pressed is the same node, keeps focus, and shows aria-disabled.
  const typedRefusal =
    result && !result.saved
      ? (result.refusal ?? null)
      : checking && !again && !result
        ? heldRefusal
        : null;
  // While any check is out, the old verdict is not shown: an answer identical to it (the same
  // "Key works" over a saved key) would change nothing in the live region and never be announced.
  const verdict = checking
    ? null
    : session && result?.saved
      ? session
      : session
        ? null
        : storedVerdict(view, name);
  const fieldError = refusal ?? (session && !result?.saved && !typedRefusal ? session.text : null);
  // What the card shows now, from this session's check or the stored one.
  // A re-check that only failed to reach the provider stored nothing, so the stored verdict (and
  // its Check again button) still stand.
  const fresh = session && result?.saved && (!again || recheckStores(result.outcome));
  const current = fresh ? result : view.key_check;
  // The tier and the pause the card is showing: this session's re-check once it has been read back,
  // else the state's (which a 429 may have moved since).
  const tier = fresh && again && result ? (result.tier ?? view.tier) : view.tier;
  const canRecheck = hasKey && offersRecheckFor(info, current, tier, view.blocked);
  const terms = termsFor(info, view);
  const badge = tierBadge(info, view);
  const using = (["chat", "background"] as RoleName[]).filter(
    (r) => view.roles[r].uses_this_provider,
  );
  const usingSentence =
    using.length === 2
      ? "Chat and Background use it now. They'll go back to OpenRouter."
      : `${roleName(using[0] ?? "chat")} uses it now. It'll go back to OpenRouter.`;

  return (
    <div ref={cardRef} className="rounded-[var(--radius)] border border-border bg-surface p-3">
      <SettingRow
        label={info.toggleLabel}
        description={view.opted_in ? undefined : hasKey ? info.offWithKeyLine : info.offLine}
        spacing="none"
      >
        {(a11y) => <Toggle {...a11y} checked={view.opted_in} onChange={(on) => void toggle(on)} />}
      </SettingRow>

      {/* In the DOM from the first render and empty, so the announcement fires when it is filled. */}
      <VisuallyHidden role="status" aria-live="polite">
        {announce}
      </VisuallyHidden>

      {moved && (
        <Callout tone="info" live body="ink" className="mt-3">
          {moved}
        </Callout>
      )}

      {view.opted_in && (
        <>
          {badge && (
            // Real text with the glyph hidden from a screen reader: a bare span with an aria-label
            // isn't reliably announced, and the verdict is never colour alone.
            <p
              data-help={info.tiers ? "settings-gemini-tier" : undefined}
              className={`mt-3 flex items-center gap-1.5 text-xs font-medium ${
                badge.tone === "ok" ? "text-st-track" : "text-st-due"
              }`}
            >
              <span aria-hidden="true">{badge.glyph}</span>
              <span>{badge.text}</span>
            </p>
          )}
          <Callout tone={terms.tone} body="ink" live={false} className="mt-3">
            <p className="font-medium">{terms.headline}</p>
            <ul className="mt-1 list-disc space-y-1 pl-4">
              {terms.points.map((p) => (
                <li key={p}>{p}</li>
              ))}
            </ul>
          </Callout>

          {view.key === "unreadable" && (
            <Callout tone="warning" body="ink" live={false} className="mt-3">
              PM can't read your saved {name} key right now.
            </Callout>
          )}

          {view.blocked && (
            <Callout tone="warning" body="ink" live={false} className="mt-3">
              {pausedText(info, view.blocked, view.tier, flippedAt)}
            </Callout>
          )}

          {!view.blocked && flippedAt && view.tier === "free" && (
            <Callout tone="danger" body="ink" live={false} className="mt-3">
              {flippedLine(flippedAt)}
            </Callout>
          )}

          <form onSubmit={(e) => void submit(e)} className="mt-3" noValidate>
            <Field label={info.keyLabel} description={info.keyDescription} error={fieldError}>
              {(cp) => (
                <Input
                  {...cp}
                  ref={inputRef}
                  type="password"
                  autoComplete="off"
                  data-help={info.helpId}
                  value={typed}
                  onChange={(e) => {
                    setTyped(e.target.value);
                    // The next edit ends the last result's turn on screen.
                    setResult(null);
                    setRefusal(null);
                    setHeldRefusal(null);
                  }}
                  placeholder={hasKey ? "•••••••• (saved, type to replace)" : info.keyHint}
                />
              )}
            </Field>
            <a
              href={info.keyUrl}
              target="_blank"
              rel="noreferrer"
              className="mt-1 inline-block text-xs text-ink4 hover:text-ink2"
            >
              {info.keyLinkText}
            </a>
            <div className="mt-2 flex flex-wrap items-center gap-2">
              {/* The same element before, during and after a check: swapping a focused button drops
                  focus in WebKitGTK, and `disabled` would too. */}
              <Button
                type="submit"
                variant="secondary"
                aria-disabled={checking ? true : undefined}
                className={checking ? "cursor-wait" : undefined}
              >
                {checking ? "Checking..." : "Save and check"}
              </Button>
              {hasKey && (
                <Button
                  type="button"
                  variant="tertiary"
                  aria-label={`Remove key for ${name}`}
                  onClick={askRemove}
                >
                  Remove key
                </Button>
              )}
              {canRecheck && (
                // The same element for the whole check, like Save and check.
                <Button
                  type="button"
                  variant="secondary"
                  aria-label={`Check again for ${name}`}
                  aria-disabled={checking ? true : undefined}
                  className={checking ? "cursor-wait" : undefined}
                  onClick={() => void checkAgain()}
                >
                  {checking && again ? "Checking..." : "Check again"}
                </Button>
              )}
            </div>
            <div role="status" aria-live="polite" className="mt-2">
              {typedRefusal && (
                // Inside the one live region, so it is announced once and not by a second one.
                <Callout tone="warning" body="ink" live={false}>
                  <p className="font-medium">PM didn't save this key.</p>
                  <p className="mt-1">{refusalText(typedRefusal)}</p>
                  {notThereLine(typedRefusal) && (
                    <p className="mt-1">{notThereLine(typedRefusal)}</p>
                  )}
                  <p className="mt-1 flex flex-wrap gap-x-3">
                    <a
                      href="https://aistudio.google.com/projects"
                      target="_blank"
                      rel="noreferrer"
                      className="underline"
                    >
                      Link billing in AI Studio →
                    </a>
                    <a
                      href="https://aistudio.google.com/billing"
                      target="_blank"
                      rel="noreferrer"
                      className="underline"
                    >
                      Add prepay credit →
                    </a>
                  </p>
                  {/* A submit, so Enter and a click do the same thing: the typed key is still in the
                      field, and this is the same check as Save and check. */}
                  <Button
                    type="submit"
                    variant="secondary"
                    size="xs"
                    className={`mt-2${checking ? " cursor-wait" : ""}`}
                    aria-disabled={checking ? true : undefined}
                  >
                    Check again
                  </Button>
                </Callout>
              )}
              {verdict &&
                (verdict.tone !== "ok" ? (
                  <Callout tone="warning" body="ink" live={false}>
                    {verdict.text}
                  </Callout>
                ) : (
                  <p className="text-xs font-medium text-st-track">✓ {verdict.text}</p>
                ))}
            </div>
          </form>

          {/* The limit lives beside the provider it brakes. Its pause line stands inside it. */}
          <DailyLimitControl
            provider={view.provider}
            name={name}
            limit={view.limit}
            onChanged={reload}
            onError={onError}
          />

          {info.sharesDataOption && (
            <div className="mt-3 flex items-start gap-2 rounded-[var(--radius-sm)] py-1 hover:bg-surface">
              <input
                id={sharesId}
                type="checkbox"
                checked={view.shares_data}
                onChange={(e) => void setShares(e.target.checked)}
                aria-describedby={`${sharesId}-note`}
                className="mt-0.5 h-[max(1rem,var(--tap-min,24px))] w-[max(1rem,var(--tap-min,24px))] shrink-0"
              />
              <div className="text-sm leading-snug">
                <label htmlFor={sharesId} className="cursor-pointer text-ink2">
                  {info.sharesDataOption.label}
                </label>
                <span id={`${sharesId}-note`} className="block text-xs text-ink4">
                  {info.sharesDataOption.note}
                </span>
              </div>
            </div>
          )}

          <SectionInfo title={info.detailsTitle}>
            {info.details.map((d) => (
              <p key={d}>{d}</p>
            ))}
            {info.tiers && view.terms_checked_on && (
              <p>
                These are Google&apos;s terms when this version of PM was built (checked{" "}
                {dayMonthYear(view.terms_checked_on)}). They can change. Google&apos;s terms page is
                the one to trust.
              </p>
            )}
          </SectionInfo>
        </>
      )}

      {!view.opted_in && hasKey && (
        <div className="mt-2">
          <Button
            type="button"
            variant="tertiary"
            aria-label={`Remove key for ${name}`}
            onClick={askRemove}
          >
            Remove key
          </Button>
        </div>
      )}

      <ConfirmDialog
        open={confirming}
        title={`Remove your ${name} key?`}
        confirmLabel="Remove key"
        cancelLabel="Keep it"
        danger
        initialFocus="cancel"
        onConfirm={() => void remove()}
        onClose={() => setConfirming(false)}
      >
        {usingSentence} You can paste the key again any time.
      </ConfirmDialog>
    </div>
  );
}
