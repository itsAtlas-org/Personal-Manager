// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Redaction for everything the provider checks print or post. The logs, job summaries and issues of
// this repository are public and permanent, so every line is passed through here as well as being
// built from a whitelist (lib/log.mjs, lib/findings.mjs): belt and braces, mirroring PM's own
// `redact_detail`. It removes key shapes of the three vendors, JWTs, bearer tokens, the id shapes of
// Anthropic's federation objects and of a request, UUIDs (an organisation id is one), and the literal
// values of any secret the process holds.
//
// Pure Node built-ins, ESM, no dependencies (INVARIANTS.md I-18).

const SHAPES = [
  /sk-ant-[A-Za-z0-9_-]{8,}/g, // Anthropic keys and OAuth tokens
  /sk-[A-Za-z0-9_-]{16,}/g, // OpenAI keys (sk-proj-...), and anything shaped like one
  /AIza[0-9A-Za-z_-]{20,}/g, // Google API keys
  /eyJ[A-Za-z0-9_-]{10,}(?:\.[A-Za-z0-9_-]+){0,2}/g, // a JWT, whole or in part
  /\bBearer\s+[A-Za-z0-9._~+/=-]{8,}/gi, // an Authorization header's value
  /\b(?:fdrl|svac|wrkspc)_[A-Za-z0-9]{6,}/g, // federation rule, service account, workspace ids
  /\b(?:req|msg)_[A-Za-z0-9]{10,}/g, // a request or message id of an account
  /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/gi, // a UUID (an org id)
];

/** The shortest literal secret worth replacing: below this it would mangle ordinary words. */
const MIN_LITERAL = 6;

/** `text` with every secret-shaped run and every literal in `literals` replaced by [redacted]. */
export function redact(text, literals = []) {
  let out = String(text);
  for (const lit of literals) {
    if (typeof lit === "string" && lit.length >= MIN_LITERAL)
      out = out.split(lit).join("[redacted]");
  }
  for (const re of SHAPES) out = out.replace(re, "[redacted]");
  return out;
}

/** True when redacting would change `text`: the test a whitelist uses to refuse a value. */
export function looksSecret(text, literals = []) {
  return redact(text, literals) !== String(text);
}
