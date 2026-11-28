// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What `route_status` answers, built once for the tests that need it: the editor, the settings tab,
// the footer, the chip and the Local AI tab all read the same shape, and hand-written copies would
// drift. The names and ids only need to be plausible; the sentences under test word what they are
// handed.

import type { RouteRow, RouteView } from "./types";

const ROW_DEFAULTS: RouteRow = {
  key: "openrouter:anthropic/claude-sonnet-4.6",
  kind: "cloud",
  provider: "openrouter",
  model: "anthropic/claude-sonnet-4.6",
  name: "anthropic/claude-sonnet-4.6",
  effort: null,
  effort_options: [],
  status: { kind: "ready" },
};

/** An OpenRouter row (the default), or any other by `over`. The key follows the provider and model
 *  unless given. */
export function routeRow(over: Partial<RouteRow> = {}): RouteRow {
  const row = { ...ROW_DEFAULTS, ...over };
  return {
    ...row,
    key: over.key ?? (row.kind === "local" ? "local" : `${row.provider}:${row.model}`),
    name: over.name ?? row.model,
  };
}

export function orRow(model: string, over: Partial<RouteRow> = {}): RouteRow {
  return routeRow({ provider: "openrouter", model, ...over });
}

export function claudeRow(over: Partial<RouteRow> = {}): RouteRow {
  return routeRow({
    provider: "anthropic",
    model: "anthropic/claude-sonnet-5-5",
    name: "Claude Sonnet 5.5",
    effort_options: ["low", "medium", "high"],
    ...over,
  });
}

export function geminiRow(over: Partial<RouteRow> = {}): RouteRow {
  return routeRow({
    provider: "gemini",
    model: "gemini/gemini-3.8-flash",
    name: "Gemini 3.8 Flash",
    effort_options: ["low", "medium", "high"],
    ...over,
  });
}

export function localRow(over: Partial<RouteRow> = {}): RouteRow {
  return routeRow({
    kind: "local",
    provider: "local",
    model: "llama3.2",
    name: "llama3.2",
    key: "local",
    ...over,
  });
}

export function routeView(entries: RouteRow[], derived = false): RouteView {
  return { entries, derived };
}
