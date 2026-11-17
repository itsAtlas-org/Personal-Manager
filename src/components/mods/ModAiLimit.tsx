// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A mod's monthly AI limit on its row in the Mods tab (Mods spec §1.6, §1.13): the number the mod's
// paid requests are held under, what has counted against it this month, and a field to change it.
// PM-drawn like the rest of the row; the numbers are the backend's (`ModInfo.aiCapUsd` and
// `aiSpentUsd`, the limit's own ledger, which can differ from the Usage and cost table's log) and
// the field only sends a number to `mod_ai_cap_set`, which checks it again.
//
// Raising the limit is felt by the mod's next request; lowering it below what is used refuses its
// paid requests until the month rolls or it is raised; emptying it takes the limit away, so the next
// paid request asks for one. A model on the person's own computer never counts. The caller keys this
// by the stored limit, so the field starts again from the stored number after every save.

import { useState } from "react";
import type { ModInfo } from "../../lib/modsTypes";
import { Button, Field, Input } from "../ui";
import { formatUsd, readLimitField } from "./aiCap";

export function ModAiLimit({
  mod,
  disabled,
  onSave,
}: {
  mod: ModInfo;
  /** A command for this mod is running, or the mod can't be changed now. */
  disabled: boolean;
  /** Send the new limit (null takes it away). */
  onSave: (capUsd: number | null) => void;
}) {
  const stored = mod.aiCapUsd;
  const [text, setText] = useState(stored === null ? "" : formatUsd(stored));
  const typed = readLimitField(text);
  const target = typed === "none" ? null : typed;
  const changed = typed !== null && target !== stored;
  const reached = stored !== null && mod.aiSpentUsd >= stored;

  return (
    <div className="space-y-1" data-help="mods-ai-limit">
      <Field
        label="Monthly AI limit (US dollars)"
        description={
          stored === null
            ? "No limit is set. PM asks for one before this mod first uses an AI model that costs money."
            : "Paid AI requests from this mod stop at this amount each month. A model on your own computer is free and never counts."
        }
        error={
          typed === null ? "Type an amount in dollars, like 1.00, or leave this empty." : undefined
        }
      >
        {(controlProps) => (
          <div className="flex flex-wrap items-center gap-2">
            <Input
              {...controlProps}
              inputMode="decimal"
              autoComplete="off"
              className="max-w-[10rem]"
              value={text}
              disabled={disabled}
              onChange={(e) => setText(e.target.value)}
            />
            <Button
              variant="secondary"
              size="sm"
              disabled={disabled || !changed}
              aria-label={`Save the monthly AI limit for ${mod.name}`}
              onClick={() => {
                if (typed !== null) onSave(target);
              }}
            >
              {target === null ? "Remove limit" : "Save limit"}
            </Button>
          </div>
        )}
      </Field>
      {stored !== null && (
        <p className="text-xs text-ink3">
          Used in {mod.aiMonth}: ${formatUsd(mod.aiSpentUsd)} of ${formatUsd(stored)}.
          {reached
            ? " The limit is reached, so its paid AI requests are refused until the month rolls over or you raise it."
            : ""}
        </p>
      )}
    </div>
  );
}
