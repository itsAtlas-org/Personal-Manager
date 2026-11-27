// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useRef, useState, type FormEvent } from "react";

import {
  FREE_KEY_LINE,
  LIMIT_FIELD_ERROR,
  LIMIT_FIELD_LABEL,
  OWN_LIMIT_WHERE,
  limitFieldText,
  limitHelp,
  limitUsd,
  parseLimit,
  pauseAnnouncement,
  pausedLine,
} from "../../lib/dailyLimit";
import { setDailyLimit } from "../../lib/ipc";
import type { CloudProviderId, LimitView } from "../../lib/types";
import { Callout, Field, Input, SectionInfo, SettingRow, Toggle, VisuallyHidden } from "../ui";

interface Props {
  provider: CloudProviderId;
  /** As the person names it: "Anthropic", "Google Gemini", "OpenRouter". */
  name: string;
  limit: LimitView;
  /** Read the state again after a write. The control never works out a day or a spend itself. */
  onChanged: () => Promise<unknown> | void;
  onError?: (message: string | null) => void;
}

/**
 * A provider's daily spend limit: off by default, and turning it on starts from the suggested $5 so
 * the brake is real from the first click. The amount saves when the field is left or Enter is
 * pressed. The pause itself is a standing line, never folded away; the help beside it says what the
 * limit counts and what it can't.
 *
 * Lives beside the thing it changes (a provider's card, the OpenRouter key block). Usage shows only
 * a read-out of today against it.
 */
export function DailyLimitControl({ provider, name, limit, onChanged, onError }: Props) {
  // A stored value PM can't read is still a limit that is ON (and pausing the provider): the switch
  // shows it so the person can turn it off, which deletes it.
  const on = limit.limit_usd != null || limit.unreadable;
  const [text, setText] = useState(limit.limit_usd != null ? limitFieldText(limit.limit_usd) : "");
  const [error, setError] = useState<string | null>(null);
  // Two lines, never the same words in both (a screen reader would say it twice): what a save did is
  // a visible muted line, and a pause that begins is announced from a hidden region because the
  // standing Callout below is already its visible form.
  const [saved, setSaved] = useState("");
  const [pauseSaid, setPauseSaid] = useState("");
  // The stored amount is the truth: a raise, a clear or a restore re-seeds the field from it.
  const seen = useRef(limit.limit_usd);
  useEffect(() => {
    if (seen.current !== limit.limit_usd) {
      setText(limit.limit_usd != null ? limitFieldText(limit.limit_usd) : "");
      seen.current = limit.limit_usd;
    }
  }, [limit.limit_usd]);

  // A pause that arrives after a re-read is announced when it begins, like the region pause is: a
  // failing reply would otherwise be the only signal. Not on first render. An unreadable limit gets
  // its own words: it was not "reached", PM could not tell.
  const wasReached = useRef(limit.reached);
  useEffect(() => {
    if (limit.reached && !wasReached.current)
      setPauseSaid(pauseAnnouncement(name, limit.unreadable));
    else if (!limit.reached && wasReached.current) setPauseSaid("");
    wasReached.current = limit.reached;
  }, [limit.reached, limit.unreadable, name]);

  async function write(usd: number | null, said: string) {
    onError?.(null);
    setError(null);
    setSaved("");
    try {
      await setDailyLimit(provider, usd);
      await onChanged();
      setSaved(said);
    } catch (e) {
      onError?.(String(e));
    }
  }

  async function toggle(next: boolean) {
    if (next) {
      await write(
        limit.suggested_usd,
        `Daily limit on, ${limitUsd(limit.suggested_usd)} a day for ${name}.`,
      );
    } else {
      await write(null, `Daily limit off for ${name}.`);
    }
  }

  async function save(e?: FormEvent) {
    e?.preventDefault();
    const usd = parseLimit(text);
    if (usd == null) {
      setError(LIMIT_FIELD_ERROR);
      return;
    }
    // Unchanged is no write. The field shows cents, so a stored $5.125 reads "5.13": leaving the
    // field untouched must not save that as a different limit.
    if (
      usd === limit.limit_usd ||
      (limit.limit_usd != null &&
        text.trim().replace(/^\$/, "") === limitFieldText(limit.limit_usd))
    ) {
      if (limit.limit_usd != null) setText(limitFieldText(limit.limit_usd));
      return;
    }
    await write(usd, `Daily limit ${limitUsd(usd)} a day for ${name}.`);
  }

  // A key proven free costs nothing: the switch can't be turned on. One already on can still be
  // turned off.
  const freeNoop = limit.free_key && !on;

  return (
    <div className="mt-3" data-testid={`daily-limit-${provider}`}>
      <SettingRow
        label={`Daily limit for ${name}`}
        helpId="settings-daily-limit"
        description={freeNoop ? FREE_KEY_LINE : undefined}
        spacing="none"
      >
        {(a11y) => (
          <Toggle
            {...a11y}
            checked={on}
            disabled={freeNoop}
            title={freeNoop ? FREE_KEY_LINE : undefined}
            onChange={(next) => void toggle(next)}
          />
        )}
      </SettingRow>

      {/* Both regions are in the DOM from the first render and empty, so a change is announced when
          it is filled. The saved line is seen and heard; it clears on the next edit. */}
      <p
        role="status"
        aria-live="polite"
        data-testid={`daily-limit-saved-${provider}`}
        className="mt-1 text-xs text-ink3"
      >
        {saved}
      </p>
      <VisuallyHidden
        role="status"
        aria-live="polite"
        data-testid={`daily-limit-pause-${provider}`}
      >
        {pauseSaid}
      </VisuallyHidden>

      {on && (
        <form onSubmit={(e) => void save(e)} className="mt-2" noValidate>
          <Field
            label={
              <>
                {LIMIT_FIELD_LABEL}
                <VisuallyHidden> for {name}</VisuallyHidden>
              </>
            }
            error={error}
            className="max-w-[12rem]"
          >
            {(cp) => (
              <Input
                {...cp}
                type="text"
                inputMode="decimal"
                autoComplete="off"
                value={text}
                onChange={(e) => {
                  setText(e.target.value);
                  setError(null);
                  setSaved("");
                }}
                onBlur={() => void save()}
              />
            )}
          </Field>
        </form>
      )}

      {limit.reached && (
        // Not folded and not a toast: the state a person must be able to find again.
        <Callout tone="warning" body="ink" live={false} className="mt-2">
          {pausedLine(limit, name)}
        </Callout>
      )}

      <SectionInfo title={`About the ${name} daily limit`}>
        {limitHelp(name, OWN_LIMIT_WHERE[provider] ?? "in your account").map((p) => (
          <p key={p}>{p}</p>
        ))}
      </SectionInfo>
    </div>
  );
}
