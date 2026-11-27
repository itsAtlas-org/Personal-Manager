// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Gemini leg of the weekly provider canary: PM's own bodies, sent to Gemini 3.5 Flash-Lite on
// Google's OpenAI-compatible endpoint (the path PM chats on), plus the native list and the one-token
// paid-tier probe PM's UK/EEA/Switzerland refusal rests on. It asserts the model list (ids, limits,
// generateContent), the usage fields and, above all, the identity "billable output = total - prompt"
// (on the compatible endpoint thinking is in total_tokens but NOT in completion_tokens, so a change
// there is exactly the drift that would make PM under-count), the array-wrapped error body, the
// final streamed usage, and a long prefix reaching the implicit cache (a notice, never a finding).
// The key travels in a header only, never in a URL. Costs about $0.003 a run.
//
// Pure Node built-ins, ESM, no dependencies (INVARIANTS.md I-18).

import { bodyFor, catalogueFor, hasStreamError, padding, parseSse } from "./lib/data.mjs";
import { cachePair, checkUsage, short } from "./lib/check.mjs";
import { finding, notice } from "./lib/findings.mjs";
import { isEntry, LegError, main } from "./lib/leg.mjs";
import { Inconclusive } from "./lib/http.mjs";

const PROVIDER = "gemini";
const CHAT = "/v1beta/openai/chat/completions";

/** What PM reads from a compatible reply's usage. */
export const REQUIRED_USAGE = ["prompt_tokens", "completion_tokens", "total_tokens"];

const FINISH = new Set(["stop", "length"]);

/** Gemini pages its list; the pages are merged before judging. */
export function modelRows(pages) {
  return pages.flatMap((p) => (Array.isArray(p?.models) ? p.models : []));
}

/** The native list: every catalogue wire id with the catalogue's limits and generateContent. */
export function assertModels(rows, section) {
  const out = [];
  for (const m of section.models) {
    const row = rows.find((r) => r && r.name === `models/${m.wire}`);
    if (!row) {
      out.push(finding("model-missing", PROVIDER, { model: m.wire }));
      continue;
    }
    if (row.inputTokenLimit !== m.max_input) {
      out.push(
        finding("model-limits-changed", PROVIDER, {
          model: m.wire,
          field: "inputTokenLimit",
          listed: short(row.inputTokenLimit),
          catalogue: short(m.max_input),
        }),
      );
    }
    if (row.outputTokenLimit !== m.max_output) {
      out.push(
        finding("model-limits-changed", PROVIDER, {
          model: m.wire,
          field: "outputTokenLimit",
          listed: short(row.outputTokenLimit),
          catalogue: short(m.max_output),
        }),
      );
    }
    const methods = Array.isArray(row.supportedGenerationMethods)
      ? row.supportedGenerationMethods
      : [];
    if (!methods.includes("generateContent")) {
      out.push(
        finding("model-capability-changed", PROVIDER, {
          model: m.wire,
          field: "generateContent",
          listed: "absent",
          catalogue: "present",
        }),
      );
    }
  }
  return out;
}

/**
 * The identity PM's `usage_from` relies on. thinking = total - prompt - completion must not be
 * negative (the total must cover prompt and completion, or billable output = total - prompt would
 * fall below what completion_tokens says); all three must be numbers.
 */
export function assertIdentity(usage) {
  const { prompt_tokens: p, completion_tokens: c, total_tokens: t } = usage ?? {};
  if (![p, c, t].every((n) => typeof n === "number")) {
    return [finding("gemini-usage-identity", PROVIDER, { what: "a count is missing" })];
  }
  if (t < p + c)
    return [
      finding("gemini-usage-identity", PROVIDER, { what: "total is below prompt plus completion" }),
    ];
  return [];
}

/**
 * Whether a usage report shows thinking: total_tokens beyond prompt + completion. On the compatible
 * endpoint completion_tokens leaves thinking out and the total includes it (PM reads output as
 * total - prompt); a total that stops including it would make PM under-count spend.
 */
export const thinkingInTotal = (usage) =>
  typeof usage?.total_tokens === "number" &&
  typeof usage?.prompt_tokens === "number" &&
  typeof usage?.completion_tokens === "number" &&
  usage.total_tokens > usage.prompt_tokens + usage.completion_tokens;

/** A plain compat chat: 200, a handled finish reason, the usage fields, the identity and the walk. */
export function assertChat(res, step, keys) {
  if (res.status !== 200) return [finding("request-refused", PROVIDER, { step, http: res.status })];
  const out = [];
  const reason = res.json?.choices?.[0]?.finish_reason;
  if (typeof reason !== "string" || !FINISH.has(reason)) {
    out.push(finding("stop-reason-unknown", PROVIDER, { step, value: short(reason) }));
  }
  out.push(
    ...checkUsage({
      provider: PROVIDER,
      step,
      usage: res.json?.usage,
      required: REQUIRED_USAGE,
      keys,
    }),
  );
  out.push(...assertIdentity(res.json?.usage));
  return out;
}

/** The stream: every chunk carries cumulative usage (build day 2026-11-26) and the last is followed by [DONE]. */
export function assertStream(res, step, keys) {
  if (res.status !== 200) return [finding("request-refused", PROVIDER, { step, http: res.status })];
  const events = parseSse(res.text);
  if (hasStreamError(events)) throw new Inconclusive("the stream carried an error event");
  if (events.length === 0 || events.some((e) => e.data && e.data.__unparsed)) {
    return [finding("stream-shape-changed", PROVIDER, { step, what: "a data line is not JSON" })];
  }
  const out = [];
  if (events[events.length - 1].data !== "[DONE]") {
    out.push(finding("stream-shape-changed", PROVIDER, { step, what: "no [DONE] at the end" }));
  }
  const chunks = events.filter((e) => e.data !== "[DONE]");
  const finalUsage = [...chunks]
    .reverse()
    .find((e) => e.data.usage && typeof e.data.usage === "object")?.data.usage;
  if (!finalUsage) {
    out.push(finding("stream-usage-missing", PROVIDER, { step }));
    return out;
  }
  out.push(
    ...checkUsage({ provider: PROVIDER, step, usage: finalUsage, required: REQUIRED_USAGE, keys }),
  );
  out.push(...assertIdentity(finalUsage));
  return out;
}

/** The compat endpoint wraps an error in a one-element ARRAY: `[{"error":{...}}]`. */
export function assertArrayError(res, step) {
  const e = Array.isArray(res.json) ? res.json[0]?.error : null;
  if (res.status !== 400 || !e || typeof e !== "object" || e.status !== "INVALID_ARGUMENT") {
    return [finding("error-envelope-changed", PROVIDER, { step, http: res.status })];
  }
  return [];
}

/** True when a 429 body is Google's free-tier quota failure (a quotaId ending -FreeTier). */
export function isFreeTier429(res) {
  const text = res.text ?? "";
  return res.status === 429 && /-FreeTier|free_tier/.test(text);
}

/**
 * The tier probe on a model with no free tier: a paid key answers 200 with a usage report. A free key
 * is a setup error (the canary key must be on a paid project), anything else is a change.
 */
export function assertTierProbe(res) {
  if (
    res.status === 200 &&
    res.json &&
    typeof res.json.usageMetadata === "object" &&
    Array.isArray(res.json.candidates)
  )
    return [];
  return [finding("tier-probe-changed", PROVIDER, { http: res.status })];
}

export async function run(ctx) {
  const key = ctx.env.GEMINI_CANARY_KEY;
  const native = { "x-goog-api-key": key, "content-type": "application/json" };
  const compat = { authorization: `Bearer ${key}`, "content-type": "application/json" };
  const section = catalogueFor(ctx.catalogue, PROVIDER);
  const findings = [];
  const notices = [];

  // 1. The native list, paged.
  const pages = [];
  let token = "";
  for (let i = 0; i < 5; i++) {
    const path = `/v1beta/models?pageSize=1000${token ? `&pageToken=${encodeURIComponent(token)}` : ""}`;
    const res = await ctx.call({ path, step: "models", headers: native });
    if (res.status !== 200 || !res.json) {
      findings.push(finding("request-refused", PROVIDER, { step: "models", http: res.status }));
      break;
    }
    pages.push(res.json);
    token = typeof res.json.nextPageToken === "string" ? res.json.nextPageToken : "";
    if (!token) break;
  }
  const rows = modelRows(pages);
  if (pages.length > 0 && rows.length === 0)
    findings.push(finding("list-shape-changed", PROVIDER, { step: "models" }));
  else if (rows.length > 0) findings.push(...assertModels(rows, section));

  // 2. PM's body, plain and streamed, on the compatible endpoint.
  const plain = await ctx.call({
    path: CHAT,
    step: "chat",
    method: "POST",
    headers: compat,
    body: bodyFor(ctx.golden, PROVIDER, "plain"),
    expect: [400],
  });
  findings.push(...assertChat(plain, "chat", ctx.usageKeys));
  const streamed = await ctx.call({
    path: CHAT,
    step: "stream",
    method: "POST",
    headers: compat,
    body: bodyFor(ctx.golden, PROVIDER, "stream"),
    expect: [400],
  });
  findings.push(...assertStream(streamed, "stream", ctx.usageKeys));

  // 2b. A request that makes the model think: thinking must be in total_tokens (the identity PM relies
  // on is otherwise vacuous at reasoning_effort "minimal", where the model thinks next to nothing).
  let think;
  for (let i = 0; i < 2; i++) {
    think = await ctx.call({
      path: CHAT,
      step: "think",
      method: "POST",
      headers: compat,
      body: bodyFor(ctx.golden, PROVIDER, "plain", {
        user: "What is 17 times 23, minus 41? Answer with the number only.",
        mutate: (b) => ({ ...b, max_tokens: 1024, reasoning_effort: "high" }),
      }),
      expect: [400],
    });
    // A second try only when a 200 showed no thinking (an easy prompt may be answered without any).
    if (think.status !== 200 || thinkingInTotal(think.json?.usage)) break;
  }
  if (think.status !== 200) {
    findings.push(finding("request-refused", PROVIDER, { step: "think", http: think.status }));
  } else {
    findings.push(
      ...checkUsage({
        provider: PROVIDER,
        step: "think",
        usage: think.json?.usage,
        required: REQUIRED_USAGE,
        keys: ctx.usageKeys,
      }),
      ...assertIdentity(think.json?.usage),
    );
    // total == prompt + completion on both tries is a model that simply did not think on an easy prompt,
    // not a broken identity: a HIGH finding would open an issue that closes the next week and flap. A
    // total BELOW the sum is the real break and assertIdentity above already raised it.
    if (!thinkingInTotal(think.json?.usage) && assertIdentity(think.json?.usage).length === 0)
      notices.push(notice("thinking-not-seen", PROVIDER));
  }

  // 3. The error body: PM never sends `store`, and the compatible endpoint answers it with an array.
  const refused = await ctx.call({
    path: CHAT,
    step: "error_shape",
    method: "POST",
    headers: compat,
    body: bodyFor(ctx.golden, PROVIDER, "plain", { mutate: (b) => ({ ...b, store: false }) }),
    expect: [400],
  });
  if (refused.status === 200) notices.push(notice("store-accepted", PROVIDER));
  else findings.push(...assertArrayError(refused, "error_shape"));

  // 4. The tier probe, on the first no-free-tier model the key lists (PM's own probe order).
  const probeIds = (section.probe_models ?? []).map((id) => id.replace(/^gemini\//, ""));
  const probeId = probeIds.find((id) => rows.some((r) => r.name === `models/${id}`)) ?? probeIds[0];
  if (!probeId) throw new LegError("the catalogue lists no probe model");
  const probe = await ctx.call({
    path: `/v1beta/models/${probeId}:generateContent`,
    step: "tier_probe",
    method: "POST",
    headers: native,
    body: bodyFor(ctx.golden, PROVIDER, "tier_probe"),
    expect: [429],
  });
  if (isFreeTier429(probe)) throw new LegError("the canary key is not on a paid Gemini project");
  // Any other 429 is a busy or per-minute limit, not a changed answer.
  if (probe.status === 429) throw new Inconclusive("the tier probe was rate limited");
  findings.push(...assertTierProbe(probe));

  // 5. A ~5k-token prefix twice, for the implicit cache: a notice, never a finding.
  const long = bodyFor(ctx.golden, PROVIDER, "plain", { system: padding(220, ctx.nonce) });
  const send = () =>
    ctx.call({
      path: CHAT,
      step: "cache",
      method: "POST",
      headers: compat,
      body: long,
      expect: [400],
    });
  const pair = await cachePair({
    first: send,
    second: send,
    hit: (r) => (r.json?.usage?.prompt_tokens_details?.cached_tokens ?? 0) > 0,
    sleep: ctx.sleep,
  });
  if (pair.firstReply.status === 200 && pair.firstReply.json?.usage)
    findings.push(
      ...checkUsage({
        provider: PROVIDER,
        step: "cache",
        usage: pair.firstReply.json.usage,
        required: [],
        keys: ctx.usageKeys,
      }),
    );
  if (!pair.hit) notices.push(notice("implicit-cache-not-seen", PROVIDER));

  return { findings, notices };
}

export const configured = (env) => Boolean(env.GEMINI_CANARY_KEY);

if (isEntry(import.meta.url)) await main(PROVIDER, configured, run);
