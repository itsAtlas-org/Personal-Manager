// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// One HTTP helper for every leg: a timeout, three retries with backoff on what is plausibly
// transient (a network error, a 5xx, a 429 that is not about money), and a classification that keeps
// "the vendor was busy" apart from "the vendor changed". A leg that cannot get an answer after the
// retries is INCONCLUSIVE and never opens an issue; a key that is rejected or out of credit is an
// ERROR (a red job, so GitHub emails the actor) and is never reported as drift.
//
// Nothing here prints. Errors carry a FIXED sentence and a status code, never a body, a header or a
// URL, because the caller's message ends up in a public log.
//
// Pure Node built-ins, ESM, no dependencies (INVARIANTS.md I-18).

/** The key is rate limited or out of quota on every try: a setup problem (a red job), never drift. */
export class RateLimited extends Error {
  constructor() {
    super("canary key is rate limited or out of quota (HTTP 429 on every try)");
    this.name = "RateLimited";
  }
}

/** The vendor did not give an answer worth judging, after every retry. */
export class Inconclusive extends Error {
  constructor(message) {
    super(message);
    this.name = "Inconclusive";
  }
}

/** The canary key is out of credit or over its own spend limit: a setup problem, not drift. */
export class NeedsCredit extends Error {
  constructor() {
    super("canary key needs credit");
    this.name = "NeedsCredit";
  }
}

/** The vendor refused the canary's credentials (401, or 403 on a key). Not drift either. */
export class KeyRejected extends Error {
  constructor(status) {
    super(`canary credentials rejected (HTTP ${status})`);
    this.name = "KeyRejected";
    this.status = status;
  }
}

/** The text of a 429 or 4xx that says "money", in any of the three vendors' words. */
const BILLING_WORDS =
  /insufficient_quota|credit_balance|credit balance is too low|spend_limit|spend limit|spending cap|usage limits?|reached your (specified )?(api )?(usage )?limit|usage_limit_exceeded|billing_hard_limit|enforced_spend_limit_reached|organization_on_hold|prepayment/i;

/** Whether a reply is about the canary key's money. 402 always is; the others by their words. */
export function isBilling(status, text) {
  if (status === 402) return true;
  if (status !== 429 && status !== 400 && status !== 403) return false;
  return BILLING_WORDS.test(String(text).slice(0, 4000));
}

const MAX_TEXT = 4 * 1024 * 1024;
const DEFAULT_BACKOFF = [1000, 4000, 10000];

const real = {
  fetch: (...a) => globalThis.fetch(...a),
  sleep: (ms) => new Promise((r) => setTimeout(r, ms)),
};

/** Lower-cased header names to values. */
function headersOf(h) {
  const out = {};
  if (h && typeof h.forEach === "function") h.forEach((v, k) => (out[String(k).toLowerCase()] = v));
  return out;
}

/**
 * One request, retried on transient trouble.
 *
 * Resolves `{ status, headers, text, json }` for any reply that is a real answer (2xx, or a 4xx the
 * caller may want to judge: pass `expect: [404]` for a probe that wants one). Rejects with
 * `NeedsCredit` (billing words, or 402), `KeyRejected` (401/403, unless expected) or `Inconclusive`
 * (still failing after the retries). `deps` swaps fetch and sleep for the tests.
 */
export async function send(
  {
    url,
    method = "GET",
    headers = {},
    body,
    timeoutMs = 30000,
    retries = 3,
    backoff = DEFAULT_BACKOFF,
    expect = [],
  },
  deps = {},
) {
  const doFetch = deps.fetch ?? real.fetch;
  const sleep = deps.sleep ?? real.sleep;
  let last = "no answer";
  let every429 = true;
  for (let attempt = 0; attempt <= retries; attempt++) {
    if (attempt > 0) await sleep(backoff[Math.min(attempt - 1, backoff.length - 1)]);
    let res;
    let text;
    try {
      res = await doFetch(url, {
        method,
        headers,
        body:
          body === undefined ? undefined : typeof body === "string" ? body : JSON.stringify(body),
        redirect: "error",
        signal: AbortSignal.timeout(timeoutMs),
      });
      text = (await res.text()).slice(0, MAX_TEXT);
    } catch (e) {
      // A network error or a timeout. Say which kind, never the URL.
      last = e && e.name === "TimeoutError" ? "timed out" : "network error";
      every429 = false;
      continue;
    }
    const status = res.status;
    // Money first, even for a status the caller expects: a 400 or 429 that says the key is out of credit
    // is a setup problem, never an answer to judge.
    if (isBilling(status, text)) throw new NeedsCredit();
    if (expect.includes(status)) return finish(res, status, text);
    if (status === 401 || status === 403) throw new KeyRejected(status);
    if (status >= 500 || status === 429) {
      last = `HTTP ${status}`;
      if (status !== 429) every429 = false;
      continue;
    }
    return finish(res, status, text);
  }
  // A key used about ten times a week is never legitimately rate limited on every try: that is a
  // quota or cap the key has hit (an error to fix), not a busy vendor.
  if (every429) throw new RateLimited();
  throw new Inconclusive(`no usable answer after ${retries + 1} tries (${last})`);
}

function finish(res, status, text) {
  let json = null;
  try {
    json = JSON.parse(text);
  } catch {
    // Not JSON (an event stream, an HTML error page): the caller reads `text`.
  }
  return { status, headers: headersOf(res.headers), text, json };
}
