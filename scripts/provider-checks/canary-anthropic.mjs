// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Anthropic leg of the weekly provider canary: PM's own bodies, sent to Claude Haiku 5.5 on the
// Anthropic API ORGANISATION (Console, pay per token; a Claude.ai plan is a different account, so
// cancelling it changes nothing here).
//
// Credentials, in order: (1) Workload Identity Federation: GitHub proves who the job is with a
// short-lived OIDC token, Anthropic swaps it for an access token that lives ten minutes, and no
// Claude key is stored anywhere; (2) plan B, a stored ANTHROPIC_CANARY_KEY, used only if federation is
// not set up or its exchange is denied. With neither the leg says "not configured" and skips. The
// OIDC token and the access token are masked in the log the moment they exist (GitHub masks only
// registered secrets) and live in memory only: no file, no output, no print.
//
// It asserts the model list (ids, lifecycle, retirement, limits, effort levels, image input), the
// usage fields including thinking tokens and the one-hour cache write PM cannot price, a final usage
// taken from message_delta, a cache write then a cache read, and the error envelope. Fable is never
// probed. Costs about $0.001 a run.
//
// Pure Node built-ins, ESM, no dependencies (INVARIANTS.md I-18).

import { bodyFor, catalogueFor, hasStreamError, padding, parseSse } from "./lib/data.mjs";
import { cachePair, checkUsage, short } from "./lib/check.mjs";
import { finding, notice } from "./lib/findings.mjs";
import { isEntry, LegError, main } from "./lib/leg.mjs";
import { Inconclusive, KeyRejected } from "./lib/http.mjs";
import { mask } from "./lib/log.mjs";

const PROVIDER = "anthropic";
const MESSAGES = "/v1/messages";

/** What PM prices a reply by, in a whole (non-streamed) message. */
export const REQUIRED_USAGE = [
  "input_tokens",
  "cache_creation_input_tokens",
  "cache_read_input_tokens",
  "output_tokens",
  "output_tokens_details.thinking_tokens",
  "cache_creation.ephemeral_1h_input_tokens",
];
/** What a stream's `message_start` carries (its output count is a placeholder, so it is not required). */
export const START_USAGE = [
  "input_tokens",
  "cache_creation_input_tokens",
  "cache_read_input_tokens",
  "cache_creation.ephemeral_1h_input_tokens",
];
/** What a stream's final `message_delta` carries: cumulative, so PM takes it and never a sum. */
export const DELTA_USAGE = [
  "input_tokens",
  "cache_creation_input_tokens",
  "cache_read_input_tokens",
  "output_tokens",
  "output_tokens_details.thinking_tokens",
];

const STOP = new Set(["end_turn", "max_tokens"]);
const LIFECYCLES = new Set(["active", "deprecated", "retired"]);
const KNOWN_CAPS = new Set([
  "batch",
  "citations",
  "code_execution",
  "context_management",
  "effort",
  "image_input",
  "pdf_input",
  "server_tools",
  "structured_outputs",
  "thinking",
]);
const EFFORTS = ["low", "medium", "high", "xhigh", "max"];

const DENIED = "Anthropic WIF exchange denied (see Console > Workload identity > history)";

/** The four ids of a federation rule, and whether all are present. */
export const federationValues = (env) => ({
  federation_rule_id: env.ANTHROPIC_FEDERATION_RULE_ID,
  organization_id: env.ANTHROPIC_ORG_ID,
  service_account_id: env.ANTHROPIC_SERVICE_ACCOUNT_ID,
  workspace_id: env.ANTHROPIC_WORKSPACE_ID,
});
export const hasFederation = (env) => Object.values(federationValues(env)).every(Boolean);
/** One to three of the four ids: a setup that was started and not finished. */
export const partlyFederated = (env) =>
  !hasFederation(env) && Object.values(federationValues(env)).some(Boolean);

/**
 * Get the request headers. Federation first (see the header comment), else the stored key.
 * Returns `{ headers, notices }`.
 */
export async function authenticate(ctx) {
  const env = ctx.env;
  const version = ctx.sources.anthropic.version;
  const base = { "anthropic-version": version, "content-type": "application/json" };
  const canWif =
    hasFederation(env) && env.ACTIONS_ID_TOKEN_REQUEST_URL && env.ACTIONS_ID_TOKEN_REQUEST_TOKEN;
  if (canWif) {
    const token = await exchange(ctx);
    if (token)
      return {
        headers: { ...base, authorization: `Bearer ${token}` },
        notices: [notice("federation-used", PROVIDER)],
      };
  }
  if (env.ANTHROPIC_CANARY_KEY) {
    return {
      headers: { ...base, "x-api-key": env.ANTHROPIC_CANARY_KEY },
      notices: [notice("fallback-key-used", PROVIDER)],
    };
  }
  if (partlyFederated(env))
    throw new LegError(
      "Anthropic federation is partly configured (some of its four ids are missing) and no canary key is stored",
    );
  throw new LegError(
    canWif ? DENIED : "Anthropic federation is not usable here and no canary key is stored",
  );
}

/**
 * The exchange. A fresh OIDC token per attempt (GitHub's are single use, so a retry must never reuse
 * one), at most two attempts, each token masked the instant it exists. Returns the access token, or
 * null when Anthropic denied it (the caller falls back to a stored key or reports the denial).
 */
async function exchange(ctx) {
  const env = ctx.env;
  let last;
  for (let attempt = 0; attempt < 2; attempt++) {
    const audience = encodeURIComponent(ctx.sources.anthropic.oidc_audience);
    const jwtRes = await ctx.callGithub({
      url: `${env.ACTIONS_ID_TOKEN_REQUEST_URL}&audience=${audience}`,
      step: "oidc_token",
      headers: { authorization: `Bearer ${env.ACTIONS_ID_TOKEN_REQUEST_TOKEN}` },
    });
    const jwt = jwtRes.json?.value;
    if (typeof jwt !== "string" || jwt.length < 20) throw new LegError("GitHub gave no OIDC token");
    mask(jwt);
    try {
      const res = await ctx.call({
        path: ctx.sources.anthropic.token_path,
        step: "wif_exchange",
        method: "POST",
        headers: { "content-type": "application/json" },
        body: {
          grant_type: "urn:ietf:params:oauth:grant-type:jwt-bearer",
          assertion: jwt,
          ...federationValues(env),
        },
        retries: 0,
        expect: [400],
      });
      if (res.status === 200 && typeof res.json?.access_token === "string") {
        mask(res.json.access_token);
        return res.json.access_token;
      }
      return null; // a 400: the rule did not match
    } catch (e) {
      if (e instanceof KeyRejected) return null; // the opaque 401: the reason is only in the Console history
      if (e instanceof Inconclusive) {
        last = e;
        continue;
      }
      throw e;
    }
  }
  throw last;
}

/** `GET /v1/models?limit=1000`: every catalogue wire id, active, with the catalogue's limits and levels. */
export function assertModels(res, section) {
  if (res.status !== 200)
    return {
      findings: [finding("request-refused", PROVIDER, { step: "models", http: res.status })],
      notices: [],
    };
  const data = res.json?.data;
  if (!Array.isArray(data))
    return { findings: [finding("list-shape-changed", PROVIDER, { step: "models" })], notices: [] };
  const findings = [];
  const notices = [];
  for (const m of section.models) {
    const row = data.find((x) => x && x.id === m.wire);
    if (!row) {
      findings.push(finding("model-missing", PROVIDER, { model: m.wire }));
      continue;
    }
    if (!LIFECYCLES.has(row.lifecycle)) notices.push(notice("unknown-lifecycle", PROVIDER));
    else if (row.lifecycle !== "active") {
      findings.push(
        finding("model-capability-changed", PROVIDER, {
          model: m.wire,
          field: "lifecycle",
          listed: short(row.lifecycle),
          catalogue: "active",
        }),
      );
    }
    const listedRetire = row.retires_at ? String(row.retires_at).slice(0, 10) : null;
    const have = m.retires_on ?? null;
    if (listedRetire !== have) {
      findings.push(
        finding("model-retirement-changed", PROVIDER, {
          model: m.wire,
          listed: short(listedRetire),
          catalogue: short(have),
        }),
      );
    }
    const window = m.max_input + m.max_output;
    if (row.max_input_tokens !== window) {
      findings.push(
        finding("model-limits-changed", PROVIDER, {
          model: m.wire,
          field: "max_input_tokens",
          listed: short(row.max_input_tokens),
          catalogue: short(window),
        }),
      );
    }
    if (row.max_tokens !== m.max_output) {
      findings.push(
        finding("model-limits-changed", PROVIDER, {
          model: m.wire,
          field: "max_tokens",
          listed: short(row.max_tokens),
          catalogue: short(m.max_output),
        }),
      );
    }
    const caps = row.capabilities ?? {};
    const levels = EFFORTS.filter((l) => caps.effort?.[l]?.supported === true);
    const want = EFFORTS.filter((l) => m.effort_levels.includes(l));
    if (levels.join(",") !== want.join(",")) {
      findings.push(
        finding("model-capability-changed", PROVIDER, {
          model: m.wire,
          field: "effort",
          listed: short(levels.join(",")),
          catalogue: short(want.join(",")),
        }),
      );
    }
    const image = caps.image_input?.supported === true;
    if (image !== m.input_modalities.includes("image")) {
      findings.push(
        finding("model-capability-changed", PROVIDER, {
          model: m.wire,
          field: "image_input",
          listed: String(image),
          catalogue: String(!image),
        }),
      );
    }
    if (Object.keys(caps).some((k) => !KNOWN_CAPS.has(k)))
      notices.push(notice("new-capability-key", PROVIDER));
  }
  return { findings, notices: dedupe(notices) };
}

function dedupe(notices) {
  return notices.filter((n, i) => notices.findIndex((x) => x.id === n.id) === i);
}

/** A whole message: 200, a handled stop reason, the usage fields and the walk. */
export function assertMessage(res, step, keys) {
  if (res.status !== 200) return [finding("request-refused", PROVIDER, { step, http: res.status })];
  const out = [];
  const reason = res.json?.stop_reason;
  if (typeof reason !== "string" || !STOP.has(reason))
    out.push(finding("stop-reason-unknown", PROVIDER, { step, value: short(reason) }));
  out.push(
    ...checkUsage({
      provider: PROVIDER,
      step,
      usage: res.json?.usage,
      required: REQUIRED_USAGE,
      keys,
    }),
  );
  return out;
}

/** What a stream gave: the events by type, the start's usage and the final delta's usage and stop reason. */
export function readStream(text) {
  const events = parseSse(text);
  if (hasStreamError(events)) throw new Inconclusive("the stream carried an error event");
  const byType = (t) => events.filter((e) => e.data && e.data.type === t).map((e) => e.data);
  const start = byType("message_start")[0];
  const delta = byType("message_delta").at(-1);
  return {
    events,
    unparsed: events.some((e) => e.data && e.data.__unparsed),
    types: new Set(events.map((e) => e.data?.type).filter(Boolean)),
    startUsage: start?.message?.usage,
    deltaUsage: delta?.usage,
    stop: delta?.delta?.stop_reason,
  };
}

/** The stream: the events PM's reader needs, usage judged at the start and, from message_delta, at the end. */
export function assertStream(res, step, keys) {
  if (res.status !== 200) return [finding("request-refused", PROVIDER, { step, http: res.status })];
  const s = readStream(res.text);
  if (s.unparsed || s.events.length === 0)
    return [finding("stream-shape-changed", PROVIDER, { step, what: "a data line is not JSON" })];
  const out = [];
  for (const type of [
    "message_start",
    "content_block_start",
    "content_block_delta",
    "message_delta",
    "message_stop",
  ]) {
    if (!s.types.has(type))
      out.push(finding("stream-shape-changed", PROVIDER, { step, what: `no ${type} event` }));
  }
  if (!s.deltaUsage) {
    out.push(finding("stream-usage-missing", PROVIDER, { step }));
    return out;
  }
  if (typeof s.stop !== "string" || !STOP.has(s.stop))
    out.push(finding("stop-reason-unknown", PROVIDER, { step, value: short(s.stop) }));
  out.push(
    ...checkUsage({
      provider: PROVIDER,
      step: `${step}_start`,
      usage: s.startUsage,
      required: START_USAGE,
      keys,
    }),
  );
  out.push(
    ...checkUsage({
      provider: PROVIDER,
      step: `${step}_final`,
      usage: s.deltaUsage,
      required: DELTA_USAGE,
      keys,
    }),
  );
  return out;
}

/** The documented error envelope: `{type: "error", error: {type}, request_id}`. */
export function assertEnvelope(res, step, status, errorType) {
  const j = res.json;
  if (
    res.status !== status ||
    !j ||
    j.type !== "error" ||
    j.error?.type !== errorType ||
    typeof j.request_id !== "string"
  ) {
    return [finding("error-envelope-changed", PROVIDER, { step, http: res.status })];
  }
  return [];
}

export async function run(ctx) {
  const section = catalogueFor(ctx.catalogue, PROVIDER);
  const { headers, notices } = await authenticate(ctx);
  const findings = [];

  // 1. The list.
  const list = await ctx.call({ path: "/v1/models?limit=1000", step: "models", headers });
  const listed = assertModels(list, section);
  findings.push(...listed.findings);
  notices.push(...listed.notices);

  // 2. PM's body, whole and streamed.
  const plain = await ctx.call({
    path: MESSAGES,
    step: "message",
    method: "POST",
    headers,
    body: bodyFor(ctx.golden, PROVIDER, "plain"),
    expect: [400],
  });
  findings.push(...assertMessage(plain, "message", ctx.usageKeys));
  const streamed = await ctx.call({
    path: MESSAGES,
    step: "stream",
    method: "POST",
    headers,
    body: bodyFor(ctx.golden, PROVIDER, "stream"),
    expect: [400],
  });
  findings.push(...assertStream(streamed, "stream", ctx.usageKeys));

  // 3. A ~2k-token system block with a cache mark, twice within a minute: a write, then a read.
  const long = bodyFor(ctx.golden, PROVIDER, "stream", { system: padding(90, ctx.nonce) });
  const send = () =>
    ctx.call({ path: MESSAGES, step: "cache", method: "POST", headers, body: long, expect: [400] });
  const usageOf = (r) => readStream(r.text).deltaUsage ?? {};
  const pair = await cachePair({
    first: send,
    second: send,
    hit: (r) => (usageOf(r).cache_read_input_tokens ?? 0) > 0,
    wait: 2000,
    sleep: ctx.sleep,
  });
  const refused = [pair.firstReply, pair.secondReply].find((r) => r && r.status !== 200);
  if (refused)
    findings.push(finding("request-refused", PROVIDER, { step: "cache", http: refused.status }));
  else {
    // The write is where a changed cache price or lifetime would show (the 1-hour write PM cannot price).
    // On the real wire `cache_creation` rides in message_start's usage, so judge it AND the final delta.
    const wrote = readStream(pair.firstReply.text);
    const seen = new Set();
    for (const usage of [wrote.startUsage, wrote.deltaUsage]) {
      if (!usage) continue;
      for (const f of checkUsage({
        provider: PROVIDER,
        step: "cache",
        usage,
        required: [],
        keys: ctx.usageKeys,
      })) {
        const k = JSON.stringify([f.id, f.fields]);
        if (!seen.has(k)) {
          seen.add(k);
          findings.push(f);
        }
      }
    }
    if ((usageOf(pair.firstReply).cache_creation_input_tokens ?? 0) === 0)
      notices.push(notice("cache-first-write-not-seen", PROVIDER));
    if (!pair.hit) findings.push(finding("cache-never-hit", PROVIDER, { step: "cache" }));
  }

  // 4. Two deterministic errors, neither billed: an unknown model, and the `temperature` PM never sends.
  const unknown = await ctx.call({
    path: MESSAGES,
    step: "unknown_model",
    method: "POST",
    headers,
    body: bodyFor(ctx.golden, PROVIDER, "plain", {
      mutate: (b) => ({ ...b, model: "pm-canary-no-such-model" }),
    }),
    expect: [404],
  });
  findings.push(...assertEnvelope(unknown, "unknown_model", 404, "not_found_error"));
  const temp = await ctx.call({
    path: MESSAGES,
    step: "temperature",
    method: "POST",
    headers,
    body: bodyFor(ctx.golden, PROVIDER, "plain", { mutate: (b) => ({ ...b, temperature: 0.5 }) }),
    expect: [400],
  });
  if (temp.status === 200) notices.push(notice("temperature-accepted", PROVIDER));
  else findings.push(...assertEnvelope(temp, "temperature", 400, "invalid_request_error"));

  return { findings, notices: dedupe(notices) };
}

export const configured = (env) =>
  hasFederation(env) || partlyFederated(env) || Boolean(env.ANTHROPIC_CANARY_KEY);

if (isEntry(import.meta.url)) await main(PROVIDER, configured, run);
