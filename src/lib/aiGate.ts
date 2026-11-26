// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { AiProviderStatus } from "./types";

/**
 * Whether PM has an AI provider ready, so the first-run onboarding wizard can be dismissed (#295).
 * ANY one suffices: a cloud key, a configured local endpoint, or an explicit "set up AI later".
 * Pure. Existing keyed users satisfy `has_cloud_key`, so this is a strict SUPERSET of the old
 * key-only gate — no migration, no backfill, and a keyed user who later deletes the key falls back
 * to the wizard exactly as before.
 *
 * `has_cloud_key` is CHAT's key (its provider's), deliberately: onboarding asks "can PM answer a
 * chat?". Background work asks `background_has_cloud_key` instead, since a restore can leave Chat on
 * OpenRouter and Background on your own provider with no key on this device.
 */
export function aiReady(s: AiProviderStatus): boolean {
  return s.onboarding_done || s.has_cloud_key || s.local_configured;
}
