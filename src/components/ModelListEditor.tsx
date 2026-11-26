// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useId } from "react";
import { ModelPicker, type ModelSource } from "./ModelPicker";
import { ResetLink } from "./settings/ResetControls";
import { Button, SectionInfo, Toggle } from "./ui";

interface Props {
  label: string;
  description?: string;
  /** Ordered preferred models (first = primary, the rest are fallbacks). */
  models: string[];
  onChange: (models: string[]) => void;
  autoSwitch: boolean;
  onAutoSwitchChange: (enabled: boolean) => void;
  helpId?: string;
  /** When set, a per-option "Reset" appears by the label (#445). The caller passes it only when this
   *  role differs from its default (list + auto-switch), and the handler restores both. */
  onReset?: () => void;
  /** Where the picker's models come from. OpenRouter when omitted: every existing caller. */
  source?: ModelSource;
  /** The most models the list holds. 1 makes picking REPLACE the model: no order to edit, no
   *  fallbacks, nothing to remove (a direct provider's role has exactly one model in this version,
   *  and the list is never empty). Unlimited when omitted. */
  maxModels?: number;
  /** false drops the auto-switch toggle AND the paragraph that explains it: auto-switch is an
   *  OpenRouter feature, and a direct list has nothing to fall through to. */
  showAutoSwitch?: boolean;
  /** Let the person type a model id that isn't in the catalogue (default). Off for a direct list. */
  allowCustom?: boolean;
  /** Which role this editor is for ("chat", "background"), so the two "Add a model…" triggers and the
   *  two auto-switch toggles can be told apart by name. */
  roleName?: string;
  /** A friendlier name for a model id, shown in the row with the id as its tooltip. */
  nameFor?: (id: string) => string | undefined;
}

/**
 * Edits one role's ordered list of preferred models plus its auto-switch toggle.
 * The first model is the primary; when auto-switch is on, the rest act as
 * fallbacks the backend hands to OpenRouter so it advances to the next model when
 * the active one hits its limit. Reuses `ModelPicker` as the "add a model" control.
 */
export function ModelListEditor({
  label,
  description,
  models,
  onChange,
  autoSwitch,
  onAutoSwitchChange,
  helpId,
  onReset,
  source,
  maxModels,
  showAutoSwitch = true,
  allowCustom,
  roleName,
  nameFor,
}: Props) {
  const headingId = useId();
  const single = maxModels === 1;
  function add(id: string) {
    const trimmed = id.trim();
    if (!trimmed || models.includes(trimmed)) return;
    onChange(single ? [trimmed] : [...models, trimmed]);
  }
  function remove(id: string) {
    onChange(models.filter((m) => m !== id));
  }
  function move(i: number, dir: -1 | 1) {
    const j = i + dir;
    if (j < 0 || j >= models.length) return;
    const next = [...models];
    [next[i], next[j]] = [next[j], next[i]];
    onChange(next);
  }

  // The trigger's visible words stay; the accessible name adds the role, so two triggers on one page
  // are two names and each still starts with what is drawn (WCAG 2.5.3).
  const triggerLabel = single ? "Choose a model…" : "Add a model…";
  const triggerName = `${triggerLabel.replace(/…$/, "")}${roleName ? ` for ${roleName}` : ""}`;

  return (
    <div data-help={helpId}>
      <div className="flex items-center justify-between gap-2">
        <h2 id={headingId} className="block text-sm font-medium text-ink2">
          {label}
        </h2>
        {onReset && <ResetLink onReset={onReset} />}
      </div>

      <div className="mt-2 space-y-1.5">
        {models.length === 0 && (
          <p className="rounded-[var(--radius-sm)] border border-dashed border-border px-3 py-2 text-xs text-ink4">
            No model chosen — the default will be used.
          </p>
        )}
        {models.map((id, i) => (
          <div
            key={id}
            className="flex items-center gap-2 rounded-[var(--radius-sm)] border border-border bg-surface px-2.5 py-1.5"
          >
            <span className="min-w-0 flex-1 truncate text-sm text-ink" title={id}>
              {nameFor?.(id) ?? id}
            </span>
            {i === 0 ? (
              <span className="shrink-0 rounded-[var(--radius-sm)] bg-accent-soft px-1.5 py-0.5 text-[0.625rem] font-medium text-accent-text">
                Primary
              </span>
            ) : (
              <span className="shrink-0 text-[0.625rem] text-ink4">fallback {i}</span>
            )}
            {!single && (
              <div className="flex shrink-0 items-center">
                <Button
                  variant="tertiary"
                  size="xs"
                  onClick={() => move(i, -1)}
                  disabled={i === 0}
                  title="Move up"
                  aria-label={`Move ${id} up`}
                >
                  ↑
                </Button>
                <Button
                  variant="tertiary"
                  size="xs"
                  onClick={() => move(i, 1)}
                  disabled={i === models.length - 1}
                  title="Move down"
                  aria-label={`Move ${id} down`}
                >
                  ↓
                </Button>
                <Button
                  variant="tertiary"
                  size="xs"
                  onClick={() => remove(id)}
                  title="Remove"
                  aria-label={`Remove ${id}`}
                  className="hover:text-st-blocked"
                >
                  ×
                </Button>
              </div>
            )}
          </div>
        ))}
      </div>

      {/* `value=""` keeps the picker as an "add" control — each pick appends (or, with one model
          allowed, replaces). */}
      <ModelPicker
        value=""
        triggerLabel={triggerLabel}
        ariaLabel={triggerName}
        onChange={add}
        source={source}
        allowCustom={allowCustom}
      />

      {showAutoSwitch && (
        <div className="mt-2 flex items-start justify-between gap-3">
          <span className="text-xs font-medium text-ink2">Auto-switch on limit</span>
          {/* The visible words stay the name; the heading says whose it is. */}
          <Toggle
            checked={autoSwitch}
            onChange={onAutoSwitchChange}
            ariaLabel="Auto-switch on limit"
            aria-describedby={headingId}
            className="mt-0.5"
          />
        </div>
      )}

      <SectionInfo title="What this model does">
        {description && <p>{description}</p>}
        {showAutoSwitch && (
          <p>
            When the active model hits its limit, auto-switch falls through to the next in the list.
            Add a second model to use it.
          </p>
        )}
      </SectionInfo>
    </div>
  );
}
