// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useId } from "react";

import {
  effortOptions,
  effortToWrite,
  modelWarning,
  roleDefaultEffort,
} from "../../lib/directProviders";
import {
  chipLabel,
  directTag,
  effortName,
  GROUP_LINE,
  localFailureLine,
  moveName,
  providerWarning,
  removeName,
  rowName,
  statusNeedsLook,
  statusText,
  tryAgainName,
} from "../../lib/roleRoute";
import type { DirectProviderView, RoleName, RouteRow as RowData } from "../../lib/types";
import { Button, Select, VisuallyHidden } from "../ui";

/** Why the only row has no working Remove. */
const KEEP_ONE = "A list keeps at least one model";

/** What the buttons of a row do. The list is the parent's: a row only says what was pressed. */
export interface RowActions {
  onMove: (dir: "up" | "down") => void;
  onRemove: () => void;
  onEffort: (effort: string | null) => void;
  onTryAgain: () => void;
}

/**
 * One row of a role's ordered list: its place, the model, who it goes to, whether it can answer
 * right now (in words, never colour alone), its effort, and the buttons that move or remove it.
 *
 * The buttons carry the model's name and its current place ("Move Claude Sonnet 5.5 up, now 2 of 3"),
 * so a screen reader hears which of six identical "Move up" buttons it is on. Moving is by button,
 * not by dragging: dragging is not keyboard-accessible by default and the settings doctrine prefers
 * plain controls. An end's Move button is `disabled` (not merely inert) because focus is moved off it
 * on purpose by the list. The `data-act` hooks are how the list puts focus back after a move.
 */
export function RouteRow({
  role,
  row,
  index,
  count,
  grouped,
  view,
  actions,
}: {
  role: RoleName;
  row: RowData;
  index: number;
  count: number;
  /** The row above is OpenRouter's too: the two go out as one request. */
  grouped: boolean;
  /** The provider's own view, for the effort levels and the terms its warning is worded from. */
  view?: DirectProviderView;
  actions: RowActions;
}) {
  const noteId = useId();
  const keepId = useId();
  const lastOne = count === 1;
  const name = rowName(row);
  const direct = row.kind === "cloud" && row.provider !== "openrouter";
  const model = view?.models.find((m) => m.id === row.model);
  // The model's own warning (Fable's retention) is a standing line, and is read when the control is
  // focused: the picker that showed it is closed by now.
  const note = direct ? modelWarning(row.provider, { note: row.warning ?? null }) : null;
  const efforts = direct ? effortOptions(role, model) : [];
  const effortValue = row.effort ?? (role === "chat" ? "" : (roleDefaultEffort(role, model) ?? ""));
  const warning = direct ? providerWarning(row.provider, view) : null;
  const look = statusNeedsLook(row.status.kind);
  const describedBy = note ? noteId : undefined;
  const effortShown = efforts.length > 0;
  // Background on a model whose thinking Low switches off: the model's name, or null.
  const thinkingOff = role === "background" && model?.thinking_off ? model.name : null;

  return (
    <li
      data-row-key={row.key}
      className="rounded-[var(--radius-sm)] border border-border bg-surface px-3 py-2"
    >
      <div className="flex flex-wrap items-start gap-x-3 gap-y-2">
        <span aria-hidden="true" className="w-4 shrink-0 pt-0.5 font-mono text-xs text-ink4">
          {index + 1}
        </span>
        <div className="min-w-0 flex-1 basis-48 space-y-1">
          <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
            <span className="break-words text-sm font-medium text-ink">{name}</span>
            <span
              data-help="settings-role-provider"
              className="shrink-0 rounded-[var(--radius-sm)] bg-accent-soft px-1.5 py-0.5 text-[0.625rem] font-medium text-accent-text"
            >
              {chipLabel(row)}
            </span>
          </div>
          <p className={`text-xs ${look ? "text-st-due" : "text-ink3"}`}>
            {statusText(row.status, row.provider)}
            {row.status.kind === "not_working" && row.kind === "cloud" && (
              <>
                {" "}
                <Button
                  type="button"
                  variant="tertiary"
                  size="xs"
                  data-act="again"
                  aria-label={tryAgainName(name)}
                  onClick={actions.onTryAgain}
                >
                  Try again now
                </Button>
              </>
            )}
          </p>
          {direct && (
            <p className="text-xs text-ink4">
              {directTag(row.provider)}
              {warning ? `. ${warning}` : ""}
            </p>
          )}
          {thinkingOff && (
            // Said beside the knob so it is not hidden ("Haiku" from "Claude Haiku 5.5").
            <p className="text-xs text-ink4">
              Low also switches {thinkingOff.replace(/^Claude /, "").split(" ")[0]}&apos;s thinking
              off.
            </p>
          )}
          {row.kind === "local" && (
            <p className="text-xs text-ink4">
              {localFailureLine(index === count - 1, row.local_elsewhere === true)}
            </p>
          )}
          {grouped && <p className="text-xs text-ink4">{GROUP_LINE}</p>}
          {note && (
            // text-ink4 is the measured 4.5:1 floor the neighbouring lines use.
            <p id={noteId} className="text-xs text-ink4">
              {note}
            </p>
          )}
        </div>
        <div className="flex shrink-0 flex-wrap items-center gap-1">
          {effortShown && (
            <Select
              compact
              data-help="settings-role-effort"
              aria-label={effortName(name)}
              aria-describedby={describedBy}
              value={effortValue}
              onChange={(e) => actions.onEffort(effortToWrite(role, model, e.target.value))}
            >
              {efforts.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </Select>
          )}
          <Button
            type="button"
            variant="tertiary"
            size="xs"
            data-act="up"
            disabled={index === 0}
            title="Move up"
            aria-label={moveName(name, "up", index, count)}
            onClick={() => actions.onMove("up")}
          >
            <span aria-hidden="true">↑</span>
          </Button>
          <Button
            type="button"
            variant="tertiary"
            size="xs"
            data-act="down"
            disabled={index === count - 1}
            title="Move down"
            aria-label={moveName(name, "down", index, count)}
            onClick={() => actions.onMove("down")}
          >
            <span aria-hidden="true">↓</span>
          </Button>
          <Button
            type="button"
            variant="tertiary"
            size="xs"
            data-act="remove"
            // The only row cannot go. `aria-disabled`, not `disabled`: a disabled button leaves the
            // tab order, so a keyboard or screen-reader user would never hear WHY it does nothing.
            aria-disabled={lastOne || undefined}
            title={lastOne ? KEEP_ONE : "Remove"}
            aria-label={removeName(name)}
            aria-describedby={
              [lastOne ? keepId : undefined, effortShown ? undefined : describedBy]
                .filter(Boolean)
                .join(" ") || undefined
            }
            className={lastOne ? "cursor-not-allowed opacity-60" : "hover:text-st-blocked"}
            onClick={lastOne ? undefined : actions.onRemove}
          >
            <span aria-hidden="true">×</span>
          </Button>
          {lastOne && <VisuallyHidden id={keepId}>{KEEP_ONE}.</VisuallyHidden>}
        </div>
      </div>
    </li>
  );
}
