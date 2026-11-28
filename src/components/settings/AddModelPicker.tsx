// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useId, useState, type ReactNode } from "react";

import { directPickerRows, modelWarning, providerView, roleName } from "../../lib/directProviders";
import {
  directTag,
  LOCAL_ELSEWHERE_NAME,
  type LocalFacts,
  pickerSections,
  priceLine,
  providerWarning,
  type PickerSection,
} from "../../lib/roleRoute";
import type {
  CloudProviderId,
  DirectProvidersState,
  RoleName,
  RouteRow as RowData,
} from "../../lib/types";
import { ModelPicker } from "../ModelPicker";
import { Button, Dialog } from "../ui";

/**
 * "Add a model": a real dialog, with focus held inside it and handed back to the button that opened
 * it. One section per provider, each with its own line for why it can't be used right now; a model
 * already in the list is shown disabled rather than hidden, so what a person looks for is never
 * missing. OpenRouter's section is its live catalogue (with a custom id, as before); a provider of
 * the person's own is PM's bundled list of its models, with no custom id, because PM can only use a
 * model whose price and window it knows. Mythos is never offered (the catalogue rows are filtered
 * again here, and Rust refuses it in three layers).
 *
 * What the dialog shows is advice. Every pick is checked in Rust before it is stored.
 */
export function AddModelPicker({
  role,
  open,
  entries,
  dp,
  openRouterKey,
  local,
  onClose,
  onAdd,
}: {
  role: RoleName;
  open: boolean;
  /** The list as it is, to tell what is already in it. */
  entries: RowData[];
  dp: DirectProvidersState | null;
  openRouterKey: boolean;
  local: LocalFacts;
  onClose: () => void;
  /** Adds the row. A refusal throws, and its words are shown here. */
  onAdd: (provider: CloudProviderId | "local", model: string) => Promise<void>;
}) {
  const [error, setError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const errorId = useId();

  const listed = (provider: string, model: string) =>
    entries.some((e) => e.provider === provider && (provider === "local" || e.model === model));

  async function add(provider: CloudProviderId | "local", model: string) {
    if (adding) return;
    setError(null);
    setAdding(true);
    try {
      await onAdd(provider, model);
    } catch (e) {
      setError(String(e));
    } finally {
      setAdding(false);
    }
  }

  const sections = pickerSections(dp, openRouterKey, local);

  return (
    <Dialog
      open={open}
      onClose={onClose}
      chrome="bar"
      title={`Add a model to ${roleName(role)}`}
      subtitle="It goes at the end of the list. You can move it afterwards."
      closeLabel="Cancel"
      widthClassName="max-w-2xl"
    >
      <div className="space-y-5">
        {error && (
          <p id={errorId} role="alert" className="text-sm text-st-due">
            {error}
          </p>
        )}
        {sections.map((s) => (
          <Section
            key={s.provider}
            section={s}
            body={
              s.provider === "openrouter" ? (
                <ModelPicker
                  value=""
                  triggerLabel="Choose an OpenRouter model…"
                  ariaLabel="Choose an OpenRouter model"
                  disabledIds={entries
                    .filter((e) => e.provider === "openrouter")
                    .map((e) => e.model)}
                  onChange={(id) => void add("openrouter", id)}
                />
              ) : s.provider === "local" ? (
                s.blocked ? null : (
                  <LocalChoice
                    model={local.model}
                    already={listed("local", "")}
                    elsewhere={local.elsewhere === true}
                    onPick={() => void add("local", "")}
                  />
                )
              ) : s.blocked ? null : (
                <DirectChoices
                  provider={s.provider}
                  dp={dp}
                  listed={(id) => listed(s.provider, id)}
                  onPick={(id) => void add(s.provider, id)}
                />
              )
            }
          />
        ))}
      </div>
    </Dialog>
  );
}

function Section({ section, body }: { section: PickerSection; body: ReactNode }) {
  const headingId = useId();
  const direct = section.provider !== "openrouter" && section.provider !== "local";
  return (
    <section aria-labelledby={headingId}>
      <h2 id={headingId} className="text-sm font-medium text-ink2">
        {section.title}
      </h2>
      {direct && !section.blocked && (
        <p className="mt-0.5 text-xs text-ink4">{directTag(section.provider)}</p>
      )}
      {section.line && (
        <p className={`mt-0.5 text-xs ${section.blocked ? "text-st-due" : "text-ink3"}`}>
          {section.line}
        </p>
      )}
      {body && <div className="mt-2">{body}</div>}
    </section>
  );
}

function DirectChoices({
  provider,
  dp,
  listed,
  onPick,
}: {
  provider: CloudProviderId;
  dp: DirectProvidersState | null;
  listed: (id: string) => boolean;
  onPick: (id: string) => void;
}) {
  const view = dp ? providerView(dp, provider) : undefined;
  if (!view) return null;
  const rows = directPickerRows(view);
  const warning = providerWarning(provider, view);
  return (
    <>
      {warning && <p className="mb-1 text-xs text-ink4">{warning}</p>}
      <ul className="space-y-1">
        {rows.map((m) => {
          const taken = listed(m.id);
          const note = modelWarning(provider, { note: m.note });
          return (
            <li key={m.id}>
              <Button
                type="button"
                variant="secondary"
                size="md"
                // A model already in the list is `aria-disabled`, not `disabled`: it stays in the tab
                // order, so a keyboard or screen-reader user hears "Already in your list" (inside the
                // button's name) instead of the row vanishing from reach. The click does nothing.
                aria-disabled={taken || undefined}
                title={m.tip}
                onClick={taken ? undefined : () => onPick(m.id)}
                className={`w-full justify-start text-left ${taken ? "cursor-not-allowed opacity-60" : ""}`}
              >
                <span className="block min-w-0">
                  <span className="block text-sm text-ink">{m.name}</span>
                  <span className="block text-xs text-ink4">
                    {priceLine(
                      (m.prompt_price ?? 0) * 1_000_000,
                      (m.completion_price ?? 0) * 1_000_000,
                    )}
                  </span>
                  {(note ?? m.note) && (
                    <span className="block text-xs text-ink4">{note ?? m.note}</span>
                  )}
                  {taken && <span className="block text-xs text-ink3">Already in your list</span>}
                </span>
              </Button>
            </li>
          );
        })}
      </ul>
    </>
  );
}

function LocalChoice({
  model,
  already,
  elsewhere,
  onPick,
}: {
  model: string | null;
  already: boolean;
  /** Local AI points at another machine: say "Your Local AI server", never "This computer". */
  elsewhere: boolean;
  onPick: () => void;
}) {
  return (
    <Button
      type="button"
      variant="secondary"
      size="md"
      // `aria-disabled`, not `disabled`, for the same reason as the model rows above.
      aria-disabled={already || undefined}
      onClick={already ? undefined : onPick}
      className={`w-full justify-start text-left ${already ? "cursor-not-allowed opacity-60" : ""}`}
    >
      <span className="block min-w-0">
        <span className="block text-sm text-ink">
          {model ?? (elsewhere ? LOCAL_ELSEWHERE_NAME : "This computer")}
        </span>
        <span className="block text-xs text-ink4">Your own model, through Local AI.</span>
        {already && <span className="block text-xs text-ink3">Already in your list</span>}
      </span>
    </Button>
  );
}
