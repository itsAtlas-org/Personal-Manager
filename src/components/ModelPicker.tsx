// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useId, useMemo, useRef, useState } from "react";
import { listModels } from "../lib/ipc";
import type { DirectPickerRow } from "../lib/directProviders";
import { isKeyForDialogOver } from "../lib/useDialogLayer";
import type { ModelInfo } from "../lib/types";
import { Button, Input, Skeleton } from "./ui";

/** Where the picker's models come from. OpenRouter's own list is fetched when the menu first opens;
 *  a provider the person reaches with their own key is handed in (PM's bundled list of that
 *  provider's models), and the picker then never calls OpenRouter: a person who only uses their own
 *  key hasn't opted into OpenRouter. */
export type ModelSource =
  { kind: "openrouter" } | { kind: "direct"; models: DirectPickerRow[]; caption: string };

interface Props {
  /** The currently selected model id (e.g. "anthropic/claude-sonnet-4.6"). */
  value: string;
  onChange: (modelId: string) => void;
  /** Trigger label shown when nothing is selected (e.g. "Add a model…"). */
  triggerLabel?: string;
  /** Where the list comes from. OpenRouter when omitted, which is how every existing caller reads. */
  source?: ModelSource;
  /** Let the person type a model id that isn't in the list. Off for a direct provider: PM can only
   *  use a model whose price and window it knows. */
  allowCustom?: boolean;
  /** The trigger's accessible name. It must contain the visible `triggerLabel` words so speech input
   *  ("click Add a model") still lands, and say which control it is when there are two. */
  ariaLabel?: string;
  /** The id of a line that describes what is chosen, read after the name when the trigger is focused. */
  describedBy?: string;
  /** Models that can't be picked because they are already taken (a role's list can't hold one twice).
   *  They stay in the list, disabled and saying so, so nothing a person looks for goes missing. */
  disabledIds?: string[];
}

type Sort = "default" | "asc" | "desc";

/**
 * A searchable dropdown over OpenRouter's whole model catalogue. Shows each
 * model's input/output price and a couple of derived "best for" tags, plus a
 * search box (the list is long). Falls back to letting the user type a custom
 * model id, so PM is never locked to the catalogue (spec §6). The catalogue is
 * fetched lazily the first time the dropdown opens.
 */
export function ModelPicker({
  value,
  onChange,
  triggerLabel = "Choose a model…",
  source = { kind: "openrouter" },
  allowCustom = true,
  ariaLabel,
  describedBy,
  disabledIds,
}: Props) {
  const direct = source.kind === "direct";
  const [open, setOpen] = useState(false);
  const [fetched, setModels] = useState<ModelInfo[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<Sort>("default");
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelId = useId();
  // A direct list is already in hand; OpenRouter's arrives on first open.
  const models: ModelInfo[] | null = source.kind === "direct" ? source.models : fetched;
  const rowsById = useMemo(
    () => (source.kind === "direct" ? new Map(source.models.map((r) => [r.id, r])) : null),
    [source],
  );

  // Lazy-load the catalogue the first time the dropdown is opened. Never for a direct source.
  useEffect(() => {
    if (direct || !open || models || loading) return;
    setLoading(true);
    setError(null);
    listModels()
      .then(setModels)
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [direct, open, models, loading]);

  // Close on outside click or Escape.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      // Same rule as the Popover primitive: a modal dialog opened over this menu owns the key, and a
      // non-modal layer beside it (the calendar's quick create, left open behind Settings) only a key
      // pressed inside it; otherwise the menu closes and the key stops here, so the Settings dialog
      // around it stays open.
      if (isKeyForDialogOver(rootRef.current, e.target)) return;
      e.stopPropagation();
      setOpen(false);
      // The menu's search box unmounts with it, which would drop focus to <body>.
      triggerRef.current?.focus();
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const selected = useMemo(() => models?.find((m) => m.id === value) ?? null, [models, value]);

  const filtered = useMemo(() => {
    if (!models) return [];
    const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
    if (terms.length === 0) return models;
    return models.filter((m) => {
      const hay = `${m.id} ${m.name}`.toLowerCase();
      return terms.every((t) => hay.includes(t));
    });
  }, [models, query]);

  // Optional sort by input (prompt) price; unpriced models always sort last.
  const sorted = useMemo(() => {
    if (sort === "default") return filtered;
    const known = filtered.filter((m) => m.prompt_price != null);
    const unknown = filtered.filter((m) => m.prompt_price == null);
    known.sort((a, b) => (a.prompt_price! - b.prompt_price!) * (sort === "asc" ? 1 : -1));
    return [...known, ...unknown];
  }, [filtered, sort]);

  const MAX_SHOWN = 60;
  const shown = sorted.slice(0, MAX_SHOWN);

  // Let the user pick a model id that isn't in the catalogue (or hasn't loaded).
  const trimmed = query.trim();
  const customAvailable =
    allowCustom &&
    trimmed.length > 0 &&
    !filtered.some((m) => m.id === trimmed) &&
    !disabledIds?.includes(trimmed);

  function pick(modelId: string) {
    onChange(modelId);
    setOpen(false);
    setQuery("");
    triggerRef.current?.focus();
  }

  return (
    <div ref={rootRef} className="relative mt-1">
      <button
        ref={triggerRef}
        type="button"
        aria-label={ariaLabel}
        aria-describedby={describedBy}
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        onClick={() => setOpen((o) => !o)}
        className="flex w-full items-center justify-between gap-2 rounded-[var(--radius-sm)] border border-ink4 bg-surface px-3 py-2 text-left text-sm text-ink2 outline-none transition hover:border-ink2 focus:border-accent-text"
      >
        <span className="min-w-0 flex-1 truncate">
          {selected ? selected.name : value || triggerLabel}
          {selected && <span className="ml-2 font-mono text-xs text-ink4">{selected.id}</span>}
        </span>
        {/* Decoration: aria-expanded already says it, and the glyph would read into the name. */}
        <span aria-hidden="true" className="shrink-0 text-ink4">
          {open ? "▴" : "▾"}
        </span>
      </button>

      {open && (
        <div
          id={panelId}
          className="absolute z-20 mt-1 w-full overflow-hidden rounded-[var(--radius)] border border-border bg-panel shadow-2xl"
        >
          <div className="border-b border-rule p-2">
            <Input
              autoFocus
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search models…"
              className="py-1.5"
            />
          </div>

          {/* Count, a price-sort toggle, and the column hint for the prices. */}
          {models && !error && (
            <div className="flex items-center gap-2 px-3 py-1 font-mono text-[0.625rem] uppercase tracking-wide text-ink4">
              <span>
                {filtered.length} model{filtered.length === 1 ? "" : "s"}
              </span>
              <Button
                variant="tertiary"
                size="xs"
                onClick={() =>
                  setSort((s) => (s === "default" ? "asc" : s === "asc" ? "desc" : "default"))
                }
                title="Sort by input price"
                className="normal-case tracking-normal hover:bg-surface"
              >
                Sort: {sort === "asc" ? "price ↑" : sort === "desc" ? "price ↓" : "default"}
              </Button>
              <span className="ml-auto normal-case tracking-normal">
                price / 1M tokens · in → out
              </span>
            </div>
          )}
          {source.kind === "direct" && (
            <p className="px-3 pb-1 text-xs text-ink4">{source.caption}</p>
          )}

          <div className="max-h-72 overflow-y-auto">
            {loading && (
              <div className="flex flex-col gap-1 px-3 py-3">
                {Array.from({ length: 6 }).map((_, i) => (
                  <Skeleton key={i} className="h-9 w-full" />
                ))}
              </div>
            )}
            {error && (
              <div className="px-3 py-4 text-sm text-st-due">
                Couldn't load models ({error}). You can still type a model id below.
              </div>
            )}

            {!loading &&
              shown.map((m) => {
                const extra = rowsById?.get(m.id);
                const taken = disabledIds?.includes(m.id) ?? false;
                return (
                  <button
                    key={m.id}
                    type="button"
                    // `aria-disabled`, not `disabled`: a row already in the list stays in the tab
                    // order, so a keyboard or screen-reader person reaches it and hears "Already in
                    // your list" instead of the row being skipped. Inert on click.
                    aria-disabled={taken || undefined}
                    onClick={() => {
                      if (!taken) pick(m.id);
                    }}
                    title={extra?.tip}
                    className={`flex w-full items-start gap-3 px-3 py-2 text-left transition ${
                      taken ? "cursor-not-allowed" : "hover:bg-surface"
                    } ${m.id === value ? "bg-surface" : ""}`}
                  >
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-2">
                        <span className="truncate text-sm text-ink">{m.name}</span>
                        {m.id === value && (
                          <span className="shrink-0 text-xs text-st-quick">✓</span>
                        )}
                      </div>
                      <div className="truncate font-mono text-xs text-ink4">{m.id}</div>
                      {extra?.note && <div className="mt-0.5 text-xs text-ink4">{extra.note}</div>}
                      <div className="mt-1 flex flex-wrap gap-1">
                        {modelTags(m).map((t) => (
                          <span
                            key={t.label}
                            className={`rounded-[var(--radius-sm)] px-1.5 py-0.5 text-[0.625rem] font-medium ${t.cls}`}
                          >
                            {t.label}
                          </span>
                        ))}
                        {taken && (
                          <span className="rounded-[var(--radius-sm)] border border-ink4 px-1.5 py-0.5 text-[0.625rem] text-ink3">
                            Already in your list
                          </span>
                        )}
                        {extra?.unlisted && (
                          <span className="rounded-[var(--radius-sm)] border border-ink4 px-1.5 py-0.5 text-[0.625rem] text-ink3">
                            Not in your key's list
                          </span>
                        )}
                        {extra?.retires && (
                          <span className="rounded-[var(--radius-sm)] border border-ink4 px-1.5 py-0.5 text-[0.625rem] text-st-due">
                            Retires {extra.retires}
                          </span>
                        )}
                      </div>
                    </div>
                    <div className="shrink-0 whitespace-nowrap pt-0.5 text-right font-mono text-xs text-ink3">
                      {fmtPerM(m.prompt_price)}
                      <span className="text-ink4"> → </span>
                      {fmtPerM(m.completion_price)}
                    </div>
                  </button>
                );
              })}

            {!loading && !error && filtered.length > MAX_SHOWN && (
              <div className="px-3 py-2 text-center text-xs text-ink4">
                {filtered.length - MAX_SHOWN} more — keep typing to narrow it down.
              </div>
            )}

            {!loading && !error && filtered.length === 0 && !customAvailable && (
              <div className="px-3 py-6 text-center text-sm text-ink4">
                No models match “{query}”.
                {!allowCustom && (
                  <span className="mt-1 block text-xs">
                    PM can only use models it knows the price of.
                  </span>
                )}
              </div>
            )}

            {customAvailable && (
              <button
                type="button"
                onClick={() => pick(trimmed)}
                className="flex w-full items-center gap-2 border-t border-rule px-3 py-2 text-left text-sm text-ink3 transition hover:bg-surface"
              >
                Use custom model id:
                <span className="font-mono text-ink">{trimmed}</span>
              </button>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

interface Tag {
  label: string;
  cls: string;
}

/** Derive a couple of "best for" tags from a model's metadata. OpenRouter doesn't
 *  hand these out, so we infer them from id/name, pricing, context, and modality.
 *  Capped at three so rows stay readable. */
function modelTags(m: ModelInfo): Tag[] {
  const hay = `${m.id} ${m.name}`.toLowerCase();
  const promptPerM = (m.prompt_price ?? 0) * 1_000_000;
  const free = (m.prompt_price ?? 0) === 0 && (m.completion_price ?? 0) === 0;
  const tags: Tag[] = [];

  if (free)
    tags.push({
      label: "Free",
      cls: "bg-[color-mix(in_oklab,var(--st-quick)_15%,transparent)] text-[var(--st-quick)]",
    });
  if (/(?:^|[/-])(?:o[134]|r1|qwq)|reason|think/.test(hay))
    tags.push({
      label: "Reasoning",
      cls: "bg-[color-mix(in_oklab,var(--st-blocked)_15%,transparent)] text-[var(--st-blocked)]",
    });
  if (/cod(?:e|er|ing)/.test(hay))
    tags.push({
      label: "Coding",
      cls: "bg-[color-mix(in_oklab,var(--st-part)_15%,transparent)] text-[var(--st-part)]",
    });
  if (m.input_modalities?.includes("image"))
    tags.push({
      label: "Vision",
      cls: "bg-[color-mix(in_oklab,var(--st-look)_15%,transparent)] text-[var(--st-look)]",
    });
  if ((m.context_length ?? 0) >= 200_000)
    tags.push({
      label: "Long context",
      cls: "bg-[color-mix(in_oklab,var(--st-track)_15%,transparent)] text-[var(--st-track)]",
    });
  if (!free && promptPerM > 0 && promptPerM <= 1)
    tags.push({
      label: "Budget",
      cls: "bg-[color-mix(in_oklab,var(--st-quick)_15%,transparent)] text-[var(--st-quick)]",
    });
  else if (!free && promptPerM >= 10)
    tags.push({
      label: "Premium",
      cls: "bg-[color-mix(in_oklab,var(--st-due)_15%,transparent)] text-[var(--st-due)]",
    });

  return tags.slice(0, 3);
}

/** Format a per-token price as a friendly per-million-tokens dollar figure. */
function fmtPerM(perToken: number | null): string {
  if (perToken == null) return "—";
  if (perToken === 0) return "Free";
  const perM = perToken * 1_000_000;
  if (perM < 1) return `$${perM.toFixed(2)}`;
  if (perM < 10) return `$${perM.toFixed(2)}`;
  return `$${perM.toFixed(0)}`;
}
