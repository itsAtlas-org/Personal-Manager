// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The OpenAI leg of the weekly provider canary. It sends PM's own request bodies (the golden file) to
// the cheapest catalogue model, GPT-6 Luna, and asserts what PM prices and parses by: the model list,
// the usage fields, the extra final usage chunk of a stream, a repeated prefix hitting the cache, and
// the shape of an error. Reply TEXT is never judged. A drift is a finding; a busy vendor is
// `inconclusive`; a refused or empty key is `error` (lib/leg.mjs). Costs about $0.0005 a run.
//
// Pure Node built-ins, ESM, no dependencies (INVARIANTS.md I-18).

import { bodyFor, catalogueFor, hasStreamError, padding, parseSse } from "./lib/data.mjs";
import { checkUsage, cachePair, short } from "./lib/check.mjs";
import { finding } from "./lib/findings.mjs";
import { Inconclusive } from "./lib/http.mjs";
import { isEntry, main } from "./lib/leg.mjs";

const PROVIDER = "openai";

/** The numbers PM prices a chat reply by (all inside provider_usage_keys.json). */
export const REQUIRED_USAGE = [
  "prompt_tokens",
  "completion_tokens",
  "total_tokens",
  "prompt_tokens_details.cached_tokens",
  "prompt_tokens_details.cache_write_tokens",
  "completion_tokens_details.reasoning_tokens",
];

const FINISH = new Set(["stop", "length"]);

/** `GET /v1/models`: every catalogue wire id is listed, and its shutdown date is the catalogue's retires_on. */
export function assertModels(res, section) {
  if (res.status !== 200)
    return [finding("request-refused", PROVIDER, { step: "models", http: res.status })];
  const data = res.json?.data;
  if (!Array.isArray(data)) return [finding("list-shape-changed", PROVIDER, { step: "models" })];
  const out = [];
  for (const m of section.models) {
    const row = data.find((x) => x && x.id === m.wire);
    if (!row) {
      out.push(finding("model-missing", PROVIDER, { model: m.wire }));
      continue;
    }
    const listed = row.shutdown_date ?? null;
    const have = m.retires_on ?? null;
    if (listed !== have) {
      out.push(
        finding("model-retirement-changed", PROVIDER, {
          model: m.wire,
          listed: short(listed),
          catalogue: short(have),
        }),
      );
    }
  }
  return out;
}

/** A plain chat completion: 200, a finish reason PM handles, and the usage judged by `checkUsage`. */
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
  return out;
}

/** A stream with `include_usage`: one extra chunk with `choices: []` and the usage, then [DONE]. */
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
  const last = events.filter(
    (e) => e.data !== "[DONE]" && e.data.usage && typeof e.data.usage === "object",
  );
  const finalChunk = last.find((e) => Array.isArray(e.data.choices) && e.data.choices.length === 0);
  if (!finalChunk) {
    out.push(finding("stream-usage-missing", PROVIDER, { step }));
    return out;
  }
  out.push(
    ...checkUsage({
      provider: PROVIDER,
      step,
      usage: finalChunk.data.usage,
      required: REQUIRED_USAGE,
      keys,
    }),
  );
  return out;
}

/** The documented 404 for an unknown model. */
export function assertUnknownModel(res, step) {
  const e = res.json?.error;
  if (
    res.status !== 404 ||
    !e ||
    typeof e !== "object" ||
    e.code !== "model_not_found" ||
    typeof e.type !== "string"
  ) {
    return [finding("error-envelope-changed", PROVIDER, { step, http: res.status })];
  }
  return [];
}

export async function run(ctx) {
  const key = ctx.env.OPENAI_CANARY_KEY;
  const headers = { authorization: `Bearer ${key}`, "content-type": "application/json" };
  const section = catalogueFor(ctx.catalogue, PROVIDER);
  const findings = [];

  // 1. The list.
  const list = await ctx.call({ path: "/v1/models", step: "models", headers });
  findings.push(...assertModels(list, section));

  // 2. PM's body, plain and streamed.
  const plain = await ctx.call({
    path: "/v1/chat/completions",
    step: "chat",
    method: "POST",
    headers,
    body: bodyFor(ctx.golden, PROVIDER, "plain"),
    expect: [400],
  });
  findings.push(...assertChat(plain, "chat", ctx.usageKeys));
  const streamed = await ctx.call({
    path: "/v1/chat/completions",
    step: "stream",
    method: "POST",
    headers,
    body: bodyFor(ctx.golden, PROVIDER, "stream"),
    expect: [400],
  });
  findings.push(...assertStream(streamed, "stream", ctx.usageKeys));

  // 3. A repeated prefix over OpenAI's documented 1,024-token minimum: the second answer reads the cache.
  const long = bodyFor(ctx.golden, PROVIDER, "plain", { system: padding(80, ctx.nonce) });
  const send = () =>
    ctx.call({
      path: "/v1/chat/completions",
      step: "cache",
      method: "POST",
      headers,
      body: long,
      expect: [400],
    });
  const pair = await cachePair({
    first: send,
    second: send,
    hit: (r) => (r.json?.usage?.prompt_tokens_details?.cached_tokens ?? 0) > 0,
    // Caching on a model this new was never seen to hit in a few seconds: give it five tries.
    tries: 5,
    wait: 5000,
    sleep: ctx.sleep,
  });
  const refused = [pair.firstReply, pair.secondReply].find((r) => r && r.status !== 200);
  if (refused)
    findings.push(finding("request-refused", PROVIDER, { step: "cache", http: refused.status }));
  else {
    // The write is where a cache-write detail (a longer lifetime, a new price) would show.
    if (pair.firstReply.json?.usage)
      findings.push(
        ...checkUsage({
          provider: PROVIDER,
          step: "cache",
          usage: pair.firstReply.json.usage,
          required: [],
          keys: ctx.usageKeys,
        }),
      );
    if (!pair.hit) findings.push(finding("cache-never-hit", PROVIDER, { step: "cache" }));
  }

  // 4. An unknown model gives the documented 404 envelope (nothing is billed).
  const unknown = await ctx.call({
    path: "/v1/chat/completions",
    step: "unknown_model",
    method: "POST",
    headers,
    body: bodyFor(ctx.golden, PROVIDER, "plain", {
      mutate: (b) => ({ ...b, model: "pm-canary-no-such-model" }),
    }),
    expect: [404],
  });
  findings.push(...assertUnknownModel(unknown, "unknown_model"));

  return { findings };
}

export const configured = (env) => Boolean(env.OPENAI_CANARY_KEY);

if (isEntry(import.meta.url)) await main(PROVIDER, configured, run);
