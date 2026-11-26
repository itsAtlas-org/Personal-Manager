// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What `direct_providers_state` answers, built once for the tests that need it: the card, the role
// blocks, the wizard and the chip all read the same shape, and four hand-written copies would drift.
// Four OpenAI rows with the levels and prices the bundled catalogue carries; the numbers only need to
// be plausible and stable here, since the sentences under test word what they are handed.

import type { DirectModelView, DirectProvidersState, DirectProviderView, Settings } from "./types";

export const SOL: DirectModelView = {
  id: "openai/gpt-6.2-sol",
  name: "GPT-6.2 Sol",
  note: null,
  context: 1_050_000,
  max_output: 128_000,
  price_in: 2,
  price_cached_in: 0.1,
  price_out: 10,
  effort_levels: ["low", "medium", "high", "xhigh", "max"],
  effort_default: null,
  input_modalities: ["text", "image"],
  checked_on: "2026-11-26",
  retires_on: null,
};

export const LUNA: DirectModelView = {
  ...SOL,
  id: "openai/gpt-6-luna",
  name: "GPT-6 Luna",
  price_in: 0.1,
  price_cached_in: 0.01,
  price_out: 0.5,
  effort_levels: ["none", "low", "medium", "high"],
  effort_default: "medium",
};

export const TERRA: DirectModelView = {
  ...SOL,
  id: "openai/gpt-6.1-terra",
  name: "GPT-6.1 Terra",
  price_in: 1,
  price_cached_in: 0.05,
  price_out: 5,
  // A model with no `low`, to prove a role default falls to the model's lowest level.
  effort_levels: ["medium", "high"],
  effort_default: "medium",
};

/** An OpenAI view: opted in with a key that checked out, Chat on Sol and Background on Luna, neither
 *  role using it yet. Override any part. */
export function openaiView(over: Partial<DirectProviderView> = {}): DirectProviderView {
  return {
    provider: "openai",
    name: "OpenAI",
    opted_in: true,
    key: "present",
    key_check: {
      at: "2026-11-26T10:00:00Z",
      outcome: "ok",
      listed: [SOL.id, LUNA.id],
      listed_total: 40,
    },
    shares_data: false,
    catalogue_checked_on: "2026-11-26",
    terms_checked_on: "2026-11-26",
    roles: {
      chat: { uses_this_provider: false, model: SOL.id, effort: null },
      background: { uses_this_provider: false, model: LUNA.id, effort: "low" },
    },
    models: [SOL, LUNA, TERRA],
    ...over,
  };
}

/** The whole state. `chat` and `background` are the roles' stored (and, by default, effective)
 *  provider. */
export function directState(
  over: {
    view?: DirectProviderView;
    chat?: "openrouter" | "openai";
    background?: "openrouter" | "openai";
    chatEffective?: "openrouter" | "openai";
    backgroundEffective?: "openrouter" | "openai";
  } = {},
): DirectProvidersState {
  const chat = over.chat ?? "openrouter";
  const background = over.background ?? "openrouter";
  const base = over.view ?? openaiView();
  const view: DirectProviderView = {
    ...base,
    roles: {
      chat: { ...base.roles.chat, uses_this_provider: (over.chatEffective ?? chat) === "openai" },
      background: {
        ...base.roles.background,
        uses_this_provider: (over.backgroundEffective ?? background) === "openai",
      },
    },
  };
  return {
    providers: [view],
    roles: {
      chat: { stored: chat, effective: over.chatEffective ?? chat },
      background: { stored: background, effective: over.backgroundEffective ?? background },
    },
  };
}

/** The part of `Settings` the footer and the chip read. */
export function settingsFor(over: Partial<Settings> = {}): Settings {
  return {
    chat_models: ["anthropic/claude-sonnet-4.6", "google/gemini-2.5-flash"],
    background_models: ["google/gemini-2.5-flash"],
    chat_auto_switch: true,
    background_auto_switch: false,
    chat_provider: "openrouter",
    background_provider: "openrouter",
    chat_cloud_model: "anthropic/claude-sonnet-4.6",
    background_cloud_model: "google/gemini-2.5-flash",
    ...over,
  } as Settings;
}
