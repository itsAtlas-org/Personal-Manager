// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Assertions the three legs share: a usage object against the allow-list, and a cache pair. Each
// returns findings (never throws on a vendor's data) so a drifted reply is a finding, not a crash.
//
// Pure Node built-ins, ESM, no dependencies (INVARIANTS.md I-18).

import { at, walkUsage } from "./data.mjs";
import { finding } from "./findings.mjs";
import { looksSecret } from "./redact.mjs";

/**
 * The usage object judged three ways: every `required` path (the numbers PM prices a reply by) is a
 * number; no numeric key is outside provider_usage_keys.json (a NEW TOKEN TYPE, Bobby's "not obvious it's
 * broken but can rack up costs"); no known-unpriced key is above 0 on a plain text request.
 */
export function checkUsage({ provider, step, usage, required, keys }) {
  const findings = [];
  if (!usage || typeof usage !== "object" || Array.isArray(usage)) {
    return [finding("usage-field-missing", provider, { step, field: "usage" })];
  }
  for (const field of required) {
    if (typeof at(usage, field) !== "number")
      findings.push(finding("usage-field-missing", provider, { step, field }));
  }
  const { unknown, unpriced } = walkUsage(keys, provider, usage);
  // The key NAMES here come from the vendor's reply: `short` makes them plain words, so a new or odd
  // key is still reported (the drift Bobby asked about) and never crashes the leg or carries markup.
  for (const field of unknown)
    findings.push(finding("new-token-type", provider, { step, field: short(field) }));
  for (const u of unpriced)
    findings.push(
      finding("unpriced-token-seen", provider, { step, field: short(u.key), value: u.value }),
    );
  return findings;
}

/**
 * Two identical calls, the second expected to be served (in part) from the provider's cache.
 * `first()` and `second()` return a reply; `hit(reply)` says whether it shows a cache read. The second is
 * retried (the cache can take a moment to settle) up to `tries` times with `wait` ms between; "never hit" is
 * the caller's finding or notice. Returns `{ firstReply, secondReply, hit }`.
 */
export async function cachePair({ first, second, hit, tries = 3, wait = 3000, sleep }) {
  const firstReply = await first();
  let secondReply = null;
  for (let i = 0; i < tries; i++) {
    if (i > 0) await sleep(wait);
    secondReply = await second();
    if (hit(secondReply)) return { firstReply, secondReply, hit: true };
  }
  return { firstReply, secondReply, hit: false };
}

/**
 * A plain word for a finding field from vendor data: numbers and null become words, anything outside
 * letters, digits and `_ . , : -` becomes `?` (so does the dot of `www.`), long text is cut, and a secret-shaped run is `?`.
 */
export function short(v) {
  if (v === null || v === undefined) return "none";
  const s = String(v)
    .replace(/[^A-Za-z0-9_.,:-]/g, "?")
    .replace(/www\./gi, "www?") // GitHub autolinks a bare `www.` host
    .slice(0, 120);
  return looksSecret(s) ? "?" : s;
}
