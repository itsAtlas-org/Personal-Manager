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
  // OpenAI has no free tier and no dated price change.
  free_tier: null,
  next_price: null,
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
    // OpenAI has no tiers: no verdict on terms and nothing blocked.
    tier: null,
    blocked: null,
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

// Gemini's rows, with the levels and prices the bundled catalogue carries: 3.8 Flash and 3.1
// Flash-Lite have a free allowance, 3.1 Pro only answers a paid key, and 3.8 Flash has a dated price
// step coming.
export const FLASH: DirectModelView = {
  id: "gemini/gemini-3.8-flash",
  name: "Gemini 3.8 Flash",
  note: null,
  context: 1_048_576,
  max_output: 65_536,
  price_in: 0.75,
  price_cached_in: 0.075,
  price_out: 3.75,
  effort_levels: ["low", "medium", "high"],
  effort_default: "medium",
  input_modalities: ["text", "image"],
  checked_on: "2026-11-26",
  retires_on: null,
  free_tier: true,
  next_price: { from: "2027-01-01", price_in: 1.5, price_out: 7.5 },
};

export const FLASH_LITE: DirectModelView = {
  ...FLASH,
  id: "gemini/gemini-3.1-flash-lite",
  name: "Gemini 3.1 Flash-Lite",
  price_in: 0.25,
  price_cached_in: 0.025,
  price_out: 1.5,
  effort_levels: ["minimal", "low", "medium", "high"],
  effort_default: "minimal",
  next_price: null,
};

export const PRO: DirectModelView = {
  ...FLASH,
  id: "gemini/gemini-3.1-pro-preview",
  name: "Gemini 3.1 Pro (preview)",
  price_in: 2,
  price_cached_in: 0.2,
  price_out: 12,
  effort_levels: ["low", "medium", "high"],
  effort_default: "high",
  free_tier: false,
  next_price: null,
};

/** A Google Gemini view: opted in with a PAID key that checked out, Chat on 3.8 Flash and Background
 *  on 3.1 Flash-Lite, neither role using it yet. Every field is set, so a test can't pass on a field
 *  the backend would have sent and a mock forgot. Override any part. */
export function geminiView(over: Partial<DirectProviderView> = {}): DirectProviderView {
  return {
    provider: "gemini",
    name: "Google Gemini",
    opted_in: true,
    key: "present",
    key_check: {
      at: "2026-11-26T10:00:00Z",
      outcome: "ok",
      listed: [FLASH.id, FLASH_LITE.id, PRO.id],
      listed_total: 49,
      tier: "paid",
      flipped_free_at: null,
    },
    shares_data: false,
    tier: "paid",
    blocked: null,
    catalogue_checked_on: "2026-11-26",
    terms_checked_on: "2026-11-26",
    roles: {
      chat: { uses_this_provider: false, model: FLASH.id, effort: null },
      background: { uses_this_provider: false, model: FLASH_LITE.id, effort: "low" },
    },
    models: [FLASH, FLASH_LITE, PRO],
    ...over,
  };
}

type Who = "openrouter" | "openai" | "gemini";

/** The whole state of every provider given. `chat` and `background` are the roles' stored (and, by
 *  default, effective) provider; each view's `uses_this_provider` follows the EFFECTIVE one. */
export function stateWith(
  views: DirectProviderView[],
  over: {
    chat?: Who;
    background?: Who;
    chatEffective?: Who;
    backgroundEffective?: Who;
  } = {},
): DirectProvidersState {
  const chat = over.chat ?? "openrouter";
  const background = over.background ?? "openrouter";
  const chatEff = over.chatEffective ?? chat;
  const backgroundEff = over.backgroundEffective ?? background;
  return {
    providers: views.map((base) => ({
      ...base,
      roles: {
        chat: { ...base.roles.chat, uses_this_provider: chatEff === base.provider },
        background: {
          ...base.roles.background,
          uses_this_provider: backgroundEff === base.provider,
        },
      },
    })),
    roles: {
      chat: { stored: chat, effective: chatEff },
      background: { stored: background, effective: backgroundEff },
    },
  };
}

/** The whole state with ONE provider (OpenAI unless `view` says otherwise). */
export function directState(
  over: {
    view?: DirectProviderView;
    chat?: Who;
    background?: Who;
    chatEffective?: Who;
    backgroundEffective?: Who;
  } = {},
): DirectProvidersState {
  const { view, ...roles } = over;
  return stateWith([view ?? openaiView()], roles);
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
    chat_route_terms: null,
    background_route_terms: null,
    ...over,
  } as Settings;
}
