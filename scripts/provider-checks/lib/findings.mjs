// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What a leg may say, and the only shape in which it may say it. A keyed job hands the posting job
// one JSON string; the posting job accepts it ONLY if it validates here. A finding is a fixed id from
// the table below plus a few short fields; its sentence is filled from the id's own template, so no
// text a vendor sent can reach an issue or a log. Fields are numbers or short printable strings that
// carry no key shape (lib/redact.mjs), and each id lists the field names it may use.
//
// Statuses (every leg ends with exactly one):
//   ok            it ran; findings may be empty. The only status that can open, change or close an issue.
//   skipped       no secrets: "not configured". Touches no issue.
//   inconclusive  the vendor never gave a usable answer after the retries. Touches no issue.
//   error         the script crashed, a credential was refused, or the key needs credit. The job goes
//                 red; touches no issue.
//
// Pure Node built-ins, ESM, no dependencies (INVARIANTS.md I-18).

import { createHash } from "node:crypto";
import { looksSecret } from "./redact.mjs";

export const STATUSES = ["ok", "skipped", "inconclusive", "error"];
export const PROVIDERS = ["openai", "gemini", "anthropic"];
export const PROVIDER_NAMES = { openai: "OpenAI", gemini: "Gemini", anthropic: "Anthropic" };

/** The cap on a leg's serialised result (a job output is limited to 1 MB; this is far below). */
export const MAX_RESULT_BYTES = 64 * 1024;
export const MAX_FINDINGS = 50;

/** A finding: opens or updates an issue. `keys` are the fields its sentence may use. */
export const FINDINGS = {
  "model-missing": {
    severity: "high",
    keys: ["model"],
    text: (f) => `A model in direct_models.json is no longer listed by the provider: ${f.model}.`,
    action:
      "Check the provider's model page, then update or retire the row in src-tauri/direct_models.json.",
  },
  "model-retirement-changed": {
    severity: "high",
    keys: ["model", "listed", "catalogue"],
    text: (f) =>
      `${f.model} has a retirement date of ${f.listed}; the catalogue says ${f.catalogue}.`,
    action:
      "Set retires_on on the row in src-tauri/direct_models.json (or clear it) and bump checked_on.",
  },
  "model-limits-changed": {
    severity: "medium",
    keys: ["model", "field", "listed", "catalogue"],
    text: (f) =>
      `${f.model}: the provider lists ${f.field} as ${f.listed}; the catalogue says ${f.catalogue}.`,
    action:
      "Update the limit in src-tauri/direct_models.json; the context meter and the output cap read it.",
  },
  "model-capability-changed": {
    severity: "medium",
    keys: ["model", "field", "listed", "catalogue"],
    text: (f) =>
      `${f.model}: the provider lists ${f.field} as ${f.listed}; the catalogue says ${f.catalogue}.`,
    action: "Update effort_levels or input_modalities in src-tauri/direct_models.json.",
  },
  "list-shape-changed": {
    severity: "high",
    keys: ["step"],
    text: (f) => `The reply to the model list no longer has the shape PM reads (${f.step}).`,
    action:
      "Compare the vendor's model-list reference with PM's key check (listed_models in the provider's module).",
  },
  "request-refused": {
    severity: "high",
    keys: ["step", "http"],
    text: (f) => `A request built exactly like PM's was refused (${f.step}, HTTP ${f.http}).`,
    action:
      "PM's chat request may now fail for every person on this provider. Read the vendor's changelog first.",
  },
  "usage-field-missing": {
    severity: "high",
    keys: ["step", "field"],
    text: (f) => `The usage report no longer carries ${f.field} as a number (${f.step}).`,
    action:
      "PM prices a reply from this field. Update the usage reader and the prices, then provider_usage_keys.json.",
  },
  "new-token-type": {
    severity: "high",
    keys: ["step", "field"],
    text: (f) =>
      `The usage report has a new number PM does not know: ${f.field} (${f.step}). PM may not be pricing a new token type.`,
    action:
      "Find out what the new count costs. If it is billed, price it in PM; then add the key to src-tauri/provider_usage_keys.json.",
  },
  "unpriced-token-seen": {
    severity: "medium",
    keys: ["step", "field", "value"],
    text: (f) =>
      `A token type PM cannot price was reported on a plain text request: ${f.field} = ${f.value} (${f.step}).`,
    action:
      "Usage totals will say 'at least' for such replies. Decide whether PM should price this count.",
  },
  "stop-reason-unknown": {
    severity: "medium",
    keys: ["step", "value"],
    text: (f) => `The reply stopped for a reason PM does not handle: ${f.value} (${f.step}).`,
    action: "Add the reason to the provider's stop handling.",
  },
  "stream-shape-changed": {
    severity: "high",
    keys: ["step", "what"],
    text: (f) => `The streamed reply changed shape: ${f.what} (${f.step}).`,
    action:
      "PM's stream reader may drop or double-count text or tokens. Compare with the provider's stream parser.",
  },
  "stream-usage-missing": {
    severity: "high",
    keys: ["step"],
    text: (f) =>
      `The streamed reply no longer ends with the usage report PM logs spend from (${f.step}).`,
    action:
      "Spend would go unlogged. Check the stream options PM sends and the final-chunk reader.",
  },
  "cache-never-hit": {
    severity: "medium",
    keys: ["step"],
    text: (f) =>
      `A repeated long prefix was not served from the provider's cache, after retries (${f.step}).`,
    action:
      "A silent cost multiplier: PM prices cached input at a lower rate. Check the provider's caching terms and PM's cache marks.",
  },
  "gemini-usage-identity": {
    severity: "high",
    keys: ["what"],
    text: (f) =>
      `Gemini's usage identity (billable output = total - prompt) no longer holds: ${f.what}.`,
    action:
      "PM counts output as total_tokens minus prompt_tokens (usage_from in gemini_direct.rs); a change there would under-count spend.",
  },
  "error-envelope-changed": {
    severity: "medium",
    keys: ["step", "http"],
    text: (f) =>
      `An error reply no longer has the shape PM classifies by (${f.step}, HTTP ${f.http}).`,
    action:
      "Compare with the provider's classify() and error parsing; failures may read as the wrong kind.",
  },
  "tier-probe-changed": {
    severity: "high",
    keys: ["http"],
    text: (f) => `The one-token paid-tier probe no longer answers as PM expects (HTTP ${f.http}).`,
    action:
      "PM's free-key refusal in the UK, EEA and Switzerland rests on this probe. Re-check interpret_probe.",
  },
};

// The weekly keyless drift check's findings (provider-drift.yml). Prices are numbers per million
// tokens, public vendor data; `source` is a key of sources.json's `drift.pages` (the URL is added from
// that file when an issue is written, never taken from a reply); `phrase` is the id of a phrase PM's own
// copy depends on, not the vendor's text.
Object.assign(FINDINGS, {
  "price-drift": {
    severity: "high",
    keys: ["model", "field", "listed", "catalogue", "source"],
    text: (f) =>
      `${f.model}: the vendor page lists ${f.field} as ${f.listed}; the catalogue says ${f.catalogue} (USD per million tokens, or a multiplier for long_*).`,
    action:
      "Check the page, then update the row in src-tauri/direct_models.json (or add a dated price_changes step) and bump checked_on. The script never edits the catalogue: it feeds the spend maths.",
  },
  "price-step-differs": {
    severity: "high",
    keys: ["model", "field", "date", "listed", "catalogue", "source"],
    text: (f) =>
      `${f.model}: from ${f.date} the vendor page lists ${f.field} as ${f.listed}; the catalogue's dated step says ${f.catalogue}.`,
    action:
      "Fix or add the price_changes step in src-tauri/direct_models.json so the price changes on the right day.",
  },
  "new-price-dimension": {
    severity: "medium",
    keys: ["model", "header", "source"],
    text: (f) =>
      `${f.model} now has a price PM does not know on the vendor page: ${f.header}. PM may be under-pricing it.`,
    action:
      "Find out whether PM's requests can be billed under this price. If so, price it (and its usage field); then add the header to the page's known lists in scripts/provider-checks/sources.json.",
  },
  "free-tier-changed": {
    severity: "high",
    keys: ["model", "listed", "catalogue", "source"],
    text: (f) =>
      `${f.model}: the vendor page shows free_tier as ${f.listed}; the catalogue says ${f.catalogue}.`,
    action:
      "PM's free-key refusal and the picker read free_tier. Update src-tauri/direct_models.json.",
  },
  "new-model": {
    severity: "medium",
    keys: ["model", "in", "cached", "out", "source"],
    text: (f) =>
      `The vendor lists a model the catalogue does not have: ${f.model} (input ${f.in}, cached ${f.cached}, output ${f.out}, USD per million tokens).`,
    action:
      "Add it to src-tauri/direct_models.json, or add its name to known_other_models for that page in scripts/provider-checks/sources.json if PM should not offer it.",
  },
  "source-unparsable": {
    severity: "high",
    keys: ["source", "what"],
    text: (f) => `The page ${f.source} no longer reads the way this check expects: ${f.what}.`,
    action:
      "The page was restructured, moved or emptied, so this check cannot judge it. Read it, then fix the parser in scripts/provider-checks/lib/parse-prices.mjs or its URL in sources.json.",
  },
  "phrase-vanished": {
    severity: "high",
    keys: ["source", "phrase"],
    text: (f) =>
      `A sentence PM's wording depends on is no longer on the page ${f.source}: ${f.phrase}.`,
    action:
      "Read the page. If the vendor changed its terms, re-check the copy that rests on the sentence (the phrase text is in scripts/provider-checks/sources.json), then update the phrase.",
  },
  "covered-model-listed": {
    severity: "high",
    keys: ["model", "source"],
    text: (f) =>
      `Anthropic's Covered Models sentence now names ${f.model}, which PM does not treat as a 30-day-retention model.`,
    action:
      "PM's Fable-only retention note and its 400 handling assume only Fable needs retention. Read the page and update the copy and the catalogue notes.",
  },
});

/** A notice: the job summary only, never an issue. */
export const NOTICES = {
  "implicit-cache-not-seen":
    "Gemini's implicit cache did not report a hit on the repeated prefix (best effort, not a finding).",
  "unknown-lifecycle": "A catalogue model has a lifecycle value this check does not know.",
  "new-capability-key": "The model list carries a capability key this check does not know.",
  "store-accepted": "Gemini's compatible endpoint now accepts a `store` field it used to refuse.",
  "temperature-accepted": "Anthropic now accepts `temperature` on a model that used to refuse it.",
  "fallback-key-used": "The Claude leg used the stored canary key, not federation.",
  "federation-used": "The Claude leg used workload identity federation.",
  "thinking-not-seen":
    "Gemini's thinking request showed no thinking in total_tokens on either try, so that part of the usage identity was not exercised (the model may simply not have thought).",
  "cache-first-write-not-seen": "The first call of a cache pair did not report a write.",
  // The weekly drift check's notices. None of them opens an issue.
  "cross-check-only":
    "OpenRouter's price differs from the catalogue while the vendor's own page agrees with it (OpenRouter is a second opinion, never a reason to edit the catalogue).",
  "cross-check-no-row": "OpenRouter has no row for this catalogue model (it may simply lag).",
  "openrouter-expiry-differs":
    "OpenRouter lists an expiration date for this model that the catalogue's retires_on does not carry (cross-check only; read the vendor's deprecation page).",
  "source-url-changed":
    "A page this check reads ended at a different address than last time (the vendor may have moved its docs).",
  "source-unreachable":
    "A page could not be read this week (busy or blocked after retries), so its provider was not judged and its issue was left alone.",
  "price-step-due":
    "A dated price step in the catalogue falls within 14 days: confirm it on the vendor page.",
  "terms-stamp-changed":
    "A terms page's 'Effective' date changed: read what changed and re-check the copy that rests on it.",
  "cache-storage-changed":
    "The cache storage price per hour changed on a vendor page (PM does not price storage).",
};

// A field is a WORD: letters, digits and a few joiners, with `?` standing for anything else. Vendor
// text (a usage key name, a stop reason, a date) reaches these fields, and a public issue must never
// carry an @-mention, a link, an image or HTML out of a reply. `what` is the one field with
// spaces: it is a sentence this repo wrote (brackets only for "[DONE]").
const WORD = /^[A-Za-z0-9_.,:?-]{0,120}$/;
const WHAT = /^[A-Za-z0-9_.,:[\] -]{0,80}$/;
const SAFE_TEXT = /^[\x20-\x7e]{0,200}$/;

/** A finding for the leg's `provider`: `finding("model-missing", "openai", { model: "gpt-6-luna" })`. */
export function finding(id, provider, fields = {}) {
  const f = { id, provider, fields };
  const problem = checkFinding(f);
  if (problem) throw new Error(`finding: ${problem}`);
  return f;
}

/** The optional fields a notice may carry: which model, and which date. Plain words, like a finding's. */
const NOTICE_KEYS = ["model", "date"];

/** Why a notice is not acceptable, or null. */
function checkNotice(n) {
  if (!n || typeof n !== "object" || Array.isArray(n)) return "not an object";
  if (!Object.hasOwn(NOTICES, n.id) || !PROVIDERS.includes(n.provider))
    return "unknown id or provider";
  for (const key of Object.keys(n)) {
    if (key === "id" || key === "provider") continue;
    if (!NOTICE_KEYS.includes(key)) return `field not allowed on a notice: ${key}`;
    if (typeof n[key] !== "string" || !WORD.test(n[key]) || looksSecret(n[key]))
      return `notice field ${key} is not a plain word`;
  }
  return null;
}

/** A notice: `notice("fallback-key-used", "anthropic")`, or with a model or date: `notice("price-step-due", "gemini", { model, date })`. */
export function notice(id, provider, extra = {}) {
  const n = { id, provider, ...extra };
  const problem = checkNotice(n);
  if (problem) throw new Error(`notice: ${problem}`);
  return n;
}

/** Why a finding is not acceptable, or null. */
function checkFinding(f) {
  if (!f || typeof f !== "object" || Array.isArray(f)) return "not an object";
  if (!Object.hasOwn(FINDINGS, f.id)) return "unknown finding id";
  if (!PROVIDERS.includes(f.provider)) return "unknown provider";
  const spec = FINDINGS[f.id];
  const fields = f.fields ?? {};
  if (typeof fields !== "object" || Array.isArray(fields)) return "fields is not an object";
  for (const key of spec.keys) {
    if (!Object.hasOwn(fields, key)) return `missing field ${key}`;
  }
  for (const [key, value] of Object.entries(fields)) {
    if (!spec.keys.includes(key)) return `field not allowed here: ${key}`;
    if (typeof value === "number") {
      if (!Number.isFinite(value)) return `field ${key} is not finite`;
    } else if (typeof value === "string") {
      if (!(key === "what" ? WHAT : WORD).test(value)) return `field ${key} is not a plain word`;
      if (looksSecret(value)) return `field ${key} looks like a secret`;
    } else return `field ${key} is neither a number nor a string`;
  }
  return null;
}

/** The sentence, severity and advice of a finding, from its own template. */
export function describe(f) {
  const spec = FINDINGS[f.id];
  return { severity: spec.severity, detail: spec.text(f.fields ?? {}), action: spec.action };
}

/** `ok`, or a result with only the pieces that pass; used to build a leg's own result. */
export function makeResult(
  status,
  { findings = [], notices = [], note = "", unchecked = [] } = {},
) {
  if (!STATUSES.includes(status)) throw new Error("makeResult: unknown status");
  // `unchecked` (providers a drift run could not judge) is present only when there are some, so a
  // canary leg's result keeps its shape.
  return unchecked.length > 0
    ? { status, findings, notices, note, unchecked }
    : { status, findings, notices, note };
}

/**
 * Validate a result string from a keyed job. Never trusts it: an unparsable, oversized or
 * non-conforming string is `{ ok: false }` and the report treats the leg as errored (touching no issue).
 */
export function parseResult(text, provider) {
  // `provider` null: a result that speaks for every provider (the drift check's one job). Each
  // finding and notice still names a known provider; the report files them one provider at a time.
  const forProvider = (p) => (provider === null ? PROVIDERS.includes(p) : p === provider);
  if (typeof text !== "string" || text.length === 0) return { ok: false, reason: "no result" };
  if (Buffer.byteLength(text) > MAX_RESULT_BYTES) return { ok: false, reason: "result too large" };
  let v;
  try {
    v = JSON.parse(text);
  } catch {
    return { ok: false, reason: "result is not JSON" };
  }
  if (!v || typeof v !== "object" || Array.isArray(v))
    return { ok: false, reason: "result is not an object" };
  const allowed = new Set(["status", "findings", "notices", "note", "unchecked"]);
  for (const k of Object.keys(v))
    if (!allowed.has(k)) return { ok: false, reason: `unexpected key ${k}` };
  if (!STATUSES.includes(v.status)) return { ok: false, reason: "unknown status" };
  const findings = v.findings ?? [];
  const notices = v.notices ?? [];
  if (!Array.isArray(findings) || !Array.isArray(notices))
    return { ok: false, reason: "findings or notices not a list" };
  if (findings.length > MAX_FINDINGS || notices.length > MAX_FINDINGS)
    return { ok: false, reason: "too many entries" };
  if (v.status !== "ok" && findings.length > 0)
    return { ok: false, reason: "findings on a leg that did not finish ok" };
  for (const f of findings) {
    const problem = checkFinding(f);
    if (problem) return { ok: false, reason: problem };
    if (!forProvider(f.provider)) return { ok: false, reason: "finding for another provider" };
  }
  for (const n of notices) {
    if (checkNotice(n) || !forProvider(n.provider)) return { ok: false, reason: "bad notice" };
  }
  const unchecked = v.unchecked ?? [];
  if (
    !Array.isArray(unchecked) ||
    unchecked.length > PROVIDERS.length ||
    unchecked.some((p) => !PROVIDERS.includes(p))
  )
    return { ok: false, reason: "bad unchecked list" };
  const note = v.note ?? "";
  if (typeof note !== "string" || !SAFE_TEXT.test(note) || looksSecret(note)) {
    return { ok: false, reason: "note is not whitelisted text" };
  }
  return {
    ok: true,
    value: {
      status: v.status,
      findings,
      notices: notices.map((n) => ({ ...n })),
      note,
      unchecked: [...new Set(unchecked)],
    },
  };
}

/**
 * The fields that say WHICH problem a finding is, and WHAT the vendor says: `listed` (and a new model's
 * prices) are in, so a vendor that moves a price again re-edits the open issue, and re-opens one Bobby closed
 * as "not planned" for the old number (the "racks up costs" case: acknowledging 0.30 is not acknowledging
 * 0.60). Counts and codes (value, http) stay out, so a changing count never flaps an issue.
 */
const IDENTITY_KEYS = [
  "model",
  "field",
  "step",
  "what",
  "header",
  "phrase",
  "source",
  "date",
  "listed",
  "in",
  "cached",
  "out",
];

/**
 * Eight hex characters over the sorted findings, each as its id plus its identifying fields: the same
 * set of problems, the same issue body. A second model missing, or a second new usage key, changes it.
 */
export function fingerprint(findings) {
  const lines = findings
    .map((f) =>
      [
        f.id,
        ...IDENTITY_KEYS.map((k) =>
          f.fields && f.fields[k] !== undefined ? `${k}=${f.fields[k]}` : "",
        ),
      ].join("|"),
    )
    .sort();
  return createHash("sha256").update(lines.join("\n")).digest("hex").slice(0, 8);
}
