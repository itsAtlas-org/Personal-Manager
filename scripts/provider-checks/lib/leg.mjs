// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The frame every keyed leg runs in: what a call may reach, how an outcome becomes ONE explicit
// status, and what is written (one sanitised result, a summary, a red exit on `error`). A leg is a
// function of a context; the context carries the only way to send a request, and that way refuses any
// host that is not the provider's own (so a credential can only ever go to its vendor, like PM's
// own `ENDPOINT` consts).
//
// Pure Node built-ins, ESM, no dependencies (INVARIANTS.md I-18).

import { randomBytes } from "node:crypto";
import { pathToFileURL } from "node:url";
import { loadCatalogue, loadGolden, loadSources, loadUsageKeys } from "./data.mjs";
import { describe, makeResult, NOTICES, PROVIDER_NAMES } from "./findings.mjs";
import { Inconclusive, KeyRejected, NeedsCredit, RateLimited, send } from "./http.mjs";
import { log, registerSecret, setOutput, summary } from "./log.mjs";

/** Build a leg's context. `deps` swaps fetch and sleep, and the data files, in tests. */
export function makeContext(provider, env, deps = {}) {
  const sources = deps.sources ?? loadSources();
  const base = sources.api_hosts[provider];
  const origin = new URL(base).origin;
  const ctx = {
    provider,
    env,
    sources,
    golden: deps.golden ?? loadGolden(),
    catalogue: deps.catalogue ?? loadCatalogue(),
    usageKeys: deps.usageKeys ?? loadUsageKeys(),
    nonce: deps.nonce ?? randomBytes(6).toString("hex"),
    sleep: deps.sleep ?? ((ms) => new Promise((r) => setTimeout(r, ms))),
    /**
     * One request to the provider's own host. `path` begins with a slash; `step` names it in the log
     * (a whitelisted word), so the first run shows WHICH call a refused credential stopped at.
     */
    async call({ path, step, ...rest }) {
      const url = new URL(path, origin);
      if (url.origin !== origin) throw new Error("call: refused a host that is not the provider's");
      try {
        const res = await send(
          { url: url.href, ...rest },
          { fetch: deps.fetch, sleep: deps.sleep },
        );
        log({ leg: provider, step, http: res.status });
        return res;
      } catch (e) {
        if (e instanceof KeyRejected) log({ leg: provider, step, http: e.status, ok: false });
        throw e;
      }
    },
    /**
     * The one request that goes anywhere else: asking GitHub's own token service for the job's OIDC
     * token (the URL is the runner's `ACTIONS_ID_TOKEN_REQUEST_URL`). Refused unless it is https on a
     * githubusercontent.com host, so the bearer it carries can only reach GitHub.
     */
    async callGithub({ url, step, ...rest }) {
      const u = new URL(url);
      if (
        u.protocol !== "https:" ||
        !(u.hostname === "githubusercontent.com" || u.hostname.endsWith(".githubusercontent.com"))
      ) {
        throw new Error("callGithub: refused a host that is not GitHub's token service");
      }
      const res = await send({ url: u.href, ...rest }, { fetch: deps.fetch, sleep: deps.sleep });
      log({ leg: provider, step, http: res.status });
      return res;
    },
  };
  return ctx;
}

/**
 * Run a leg to one result. `configured(env)` says whether its secrets exist; without them the answer
 * is `skipped`. Whatever the leg throws becomes a status: busy -> inconclusive, a refused or
 * out-of-credit key or a crash -> error. Findings found before a throw are dropped: only a leg that
 * finished (`ok`) may speak about drift.
 */
export async function runLeg({ provider, configured, run, env = process.env, deps = {} }) {
  if (!configured(env)) return makeResult("skipped", { note: "not configured" });
  try {
    const ctx = makeContext(provider, env, deps);
    const out = await run(ctx);
    return makeResult("ok", { findings: out.findings ?? [], notices: out.notices ?? [] });
  } catch (e) {
    if (e instanceof Inconclusive) return makeResult("inconclusive", { note: e.message });
    if (e instanceof NeedsCredit) return makeResult("error", { note: "canary key needs credit" });
    if (e instanceof KeyRejected || e instanceof RateLimited)
      return makeResult("error", { note: e.message });
    if (e && e.name === "LegError") return makeResult("error", { note: e.message });
    return makeResult("error", {
      note: `the script crashed (${e && e.name ? e.name : "unknown"})`,
    });
  }
}

/** An error a leg raises with a FIXED sentence (never text from a reply). */
export class LegError extends Error {
  constructor(message) {
    super(message);
    this.name = "LegError";
  }
}

/** The summary lines for one result: public, so only template sentences. */
export function summaryLines(provider, result) {
  const name = PROVIDER_NAMES[provider];
  const lines = [
    `### ${name}`,
    `Status: ${result.status}${result.note ? ` (${result.note})` : ""}`,
  ];
  for (const f of result.findings) {
    const d = describe(f);
    lines.push(`- FINDING (${d.severity}) ${f.id}: ${d.detail}`);
  }
  for (const n of result.notices) lines.push(`- notice ${n.id}: ${NOTICES[n.id]}`);
  if (result.status === "ok" && result.findings.length === 0) lines.push("No findings.");
  return lines;
}

/** Entry point for a canary script: run, print, write the output, set the exit code. */
export async function main(provider, configured, run, env = process.env) {
  for (const k of [
    "OPENAI_CANARY_KEY",
    "GEMINI_CANARY_KEY",
    "ANTHROPIC_CANARY_KEY",
    "ANTHROPIC_ORG_ID",
    "ANTHROPIC_FEDERATION_RULE_ID",
    "ANTHROPIC_SERVICE_ACCOUNT_ID",
    "ANTHROPIC_WORKSPACE_ID",
    "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
  ]) {
    registerSecret(env[k]);
  }
  const result = await runLeg({ provider, configured, run, env });
  log({ leg: provider, status: result.status, count: result.findings.length });
  if (result.note) log({ leg: provider, msg: result.note });
  summary(summaryLines(provider, result), env);
  setOutput("result", JSON.stringify(result), env);
  if (result.status === "error") process.exitCode = 1;
  return result;
}

/** True when `metaUrl` is the script node was started with (the `main` guard). */
export function isEntry(metaUrl) {
  return process.argv[1] && pathToFileURL(process.argv[1]).href === metaUrl;
}
