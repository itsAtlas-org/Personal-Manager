// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A mod's monthly AI limit as the person types and reads it (Mods spec §1.6). Rust re-checks every
// number it is sent (`mods::ai::valid_cap`); this only keeps a typo from leaving the field, and
// says how a limit is written. Shared by the money prompt and the Installed row's limit field.

import { MAX_AI_CAP_USD } from "../../lib/modsTypes";

/** The number in a limit field, or null when it is not one PM will store: plain digits with an
 *  optional decimal part (no sign, exponent, thousands separator or currency mark), from 0 to the
 *  ceiling. 0 is a limit: the AI is allowed and cloud spending is not. Rounded to a whole cent, so
 *  what is stored is what a person reads back. */
export function parseCapUsd(text: string): number | null {
  const t = text.trim();
  if (!/^(\d+(\.\d*)?|\.\d+)$/.test(t)) return null;
  const n = Number(t);
  if (!Number.isFinite(n)) return null;
  const cents = Math.round(n * 100) / 100;
  return cents >= 0 && cents <= MAX_AI_CAP_USD ? cents : null;
}

/** What the Installed row's limit field holds: `"none"` for an empty field (take the limit away), a
 *  number, or null when what is typed is not a limit. */
export function readLimitField(text: string): number | "none" | null {
  if (text.trim() === "") return "none";
  return parseCapUsd(text);
}

/** A dollar amount for a field or a line: always two decimals ("1.00"), and a sum under a cent
 *  keeps four so a real but tiny spend is not shown as nothing. */
export function formatUsd(v: number): string {
  if (!Number.isFinite(v)) return "—";
  if (v > 0 && v < 0.01) return v.toFixed(4);
  return v.toFixed(2);
}
