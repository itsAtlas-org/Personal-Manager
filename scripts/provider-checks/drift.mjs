// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The weekly keyless drift check (.github/workflows/provider-drift.yml): it reads the vendors' PUBLIC
// pages (Anthropic's, OpenAI's and Google's own pricing pages, OpenRouter's model list as a second
// opinion, and the terms and data-retention pages), compares them with src-tauri/direct_models.json and
// hands one sanitised result to report.mjs. No secret, no key, no LLM, no account: a cancelled Claude plan
// or a closed API organisation changes nothing here.
//
// What it says (lib/drift-rules.mjs has the authority rule in full):
//   - a vendor page that disagrees with the catalogue, a page that parses to nothing, a model the
//     catalogue lacks, a price column PM does not know and a terms sentence that has gone are FINDINGS
//     (one issue per provider, opened by report.mjs);
//   - OpenRouter disagreeing, a changed page address, a dated step due within 14 days, a changed terms
//     stamp and a changed cache storage price are NOTICES (the job summary only).
// It NEVER edits the catalogue: that file feeds the spend maths, so a person and a PR change it.
// Batch prices are not compared: PM never calls a batch API, so a batch change cannot raise PM's costs.
//
// A page that cannot be fetched (busy, or blocked after retries) leaves its provider UNCHECKED: the
// report touches that provider's issue not at all, because not knowing is not "clear".
//
// Pure Node built-ins, ESM, no dependencies (INVARIANTS.md I-18).

import { catalogueFor, loadCatalogue, loadSources } from "./lib/data.mjs";
import { comparePrices, crossCheck, newModels, rowKey, stepsDue } from "./lib/drift-rules.mjs";
import {
  describe,
  finding,
  makeResult,
  MAX_FINDINGS,
  notice,
  NOTICES,
  PROVIDER_NAMES,
  PROVIDERS,
} from "./lib/findings.mjs";
import { Inconclusive, send } from "./lib/http.mjs";
import { isEntry } from "./lib/leg.mjs";
import { log, setOutput, summary } from "./lib/log.mjs";
import {
  parseAnthropicPricing,
  parseGeminiPricing,
  parseOpenAIPricing,
  parseOpenRouter,
} from "./lib/parse-prices.mjs";
import { checkTerms } from "./lib/terms.mjs";

const PARSERS = {
  anthropic: parseAnthropicPricing,
  openai: parseOpenAIPricing,
  gemini: parseGeminiPricing,
};

/**
 * At most this many findings are kept per provider. A vendor-wide change (every price up by half, a unit
 * or currency change, a restructured page where every row differs) makes dozens of findings; a result with
 * more than MAX_FINDINGS is refused whole by parseResult and would open NO issue, which is exactly the
 * change that matters most. So the rest is folded into one finding that says to read the page.
 */
export const MAX_PER_PROVIDER = 10;

/** Vendor pages are 24 to 118 KB; far more than this is not the page this check was written for. */
export const MAX_PAGE_CHARS = 1_000_000;

const SEVERITY = { high: 0, medium: 1 };

/**
 * Keep the first MAX_PER_PROVIDER findings of each provider, plus one that says more were found. The most
 * severe go first (a page that cannot be read outranks an unknown column), then id, model and field, so
 * the same week always keeps the same ones.
 */
export function capFindings(findings) {
  const out = [];
  for (const provider of PROVIDERS) {
    const own = findings
      .filter((f) => f.provider === provider)
      .sort(
        (a, b) =>
          SEVERITY[describe(a).severity] - SEVERITY[describe(b).severity] ||
          `${a.id} ${a.fields.model ?? ""} ${a.fields.field ?? ""}`.localeCompare(
            `${b.id} ${b.fields.model ?? ""} ${b.fields.field ?? ""}`,
          ),
      );
    if (own.length <= MAX_PER_PROVIDER) {
      out.push(...own);
      continue;
    }
    const kept = own.slice(0, MAX_PER_PROVIDER);
    out.push(
      ...kept,
      finding("source-unparsable", provider, {
        source: own[MAX_PER_PROVIDER].fields.source ?? "none",
        what: "many differences, read the page",
      }),
    );
  }
  return out;
}

/** Fetch one public page: an exact allow-listed host, redirects followed, the final address recorded. */
async function fetchPage(key, page, drift, deps) {
  const hosts = new Set(drift.hosts);
  const url = new URL(page.url);
  if (url.protocol !== "https:" || !hosts.has(url.hostname))
    throw new Error("drift: refused a page whose host is not in sources.json");
  const res = await send(
    {
      url: url.href,
      headers: {
        "user-agent": drift.user_agent,
        accept: "text/markdown, text/html, application/json, */*",
      },
      redirect: "follow",
      keyless: true,
      timeoutMs: 60000,
    },
    { fetch: deps.fetch, sleep: deps.sleep },
  );
  const finalUrl = res.url ? new URL(res.url) : url;
  return {
    key,
    status: res.status,
    text: res.text,
    finalUrl,
    hostOk: hosts.has(finalUrl.hostname),
  };
}

/**
 * Run the check. `sources` and `catalogue` are the committed files; `today` is a UTC date; `deps` swaps
 * fetch and sleep in the tests. Resolves a result (never throws on a vendor's page).
 */
export async function runDrift({ sources, catalogue, today, deps = {} }) {
  const drift = sources.drift;
  const keys = Object.keys(drift.pages);
  // All pages at once: seven small GETs, and one slow page must not eat the job's time.
  const fetched = await Promise.all(
    keys.map(async (key) => {
      try {
        return { key, ...(await fetchPage(key, drift.pages[key], drift, deps)) };
      } catch (e) {
        if (e instanceof Inconclusive) return { key, unreachable: true };
        throw e;
      }
    }),
  );
  const findings = [];
  const notices = [];
  const unchecked = new Set();
  const agreedWith = new Set(); // wires the vendor's own page was compared for
  let note = "";
  let movedCrosscheck = "";

  for (const got of fetched) {
    const page = drift.pages[got.key];
    const provider = page.provider;
    const step = got.key;
    if (got.unreachable) {
      log({ provider: provider ?? "report", step, ok: false });
      if (provider) {
        unchecked.add(provider);
        notices.push(notice("source-unreachable", provider));
      } else note = "OpenRouter could not be read, so no cross-check was made";
      continue;
    }
    log({ provider: provider ?? "report", step, http: got.status, ok: got.status === 200 });
    if (provider) {
      const expected = new URL(page.final_url ?? page.url).href;
      if (got.finalUrl.href !== expected) notices.push(notice("source-url-changed", provider));
    }
    if (page.kind === "crosscheck") {
      // Judged after the vendor pages. It has no provider, so no notice can name it: a list that now sits
      // on another host or address is simply not read, and the note says so.
      if (!got.hostOk || got.finalUrl.href !== new URL(page.final_url ?? page.url).href)
        movedCrosscheck = got.key;
      continue;
    }
    const section = catalogueFor(catalogue, provider);
    const unusable = (what) =>
      findings.push(finding("source-unparsable", provider, { source: got.key, what }));
    if (got.status !== 200) {
      unusable(`the page answered HTTP ${got.status}`);
      continue;
    }
    if (!got.hostOk) {
      unusable("the page now ends on another host");
      continue;
    }
    // The readers work through the text with a few patterns; a page far beyond the real size is not
    // worth that work and is not the page this check knows.
    if (got.text.length > MAX_PAGE_CHARS) {
      unusable("the page is far larger than expected");
      continue;
    }
    if (page.kind === "prices") {
      const parsed = PARSERS[page.parser](got.text, page.known);
      const r = comparePrices({ provider, section, parsed, pageKey: got.key, today, page });
      findings.push(...r.findings);
      notices.push(...r.notices);
      if (parsed.rows.size > 0) {
        findings.push(...newModels({ provider, section, parsed, page, pageKey: got.key }));
        // Only a model the page HAS a row for was judged: a model with no row is an alert of its own, and
        // OpenRouter's notice must not say the vendor agreed about it.
        for (const m of section.models)
          if (parsed.rows.has(rowKey[provider](m))) agreedWith.add(`${provider}/${m.wire}`);
      }
    } else if (page.kind === "terms") {
      const r = checkTerms({ provider, text: got.text, pageKey: got.key, page, section });
      findings.push(...r.findings);
      notices.push(...r.notices);
    }
  }

  // The second opinion. It can only add notices.
  const or = fetched.find((g) => drift.pages[g.key].kind === "crosscheck");
  if (or && !or.unreachable && or.key === movedCrosscheck)
    note = "OpenRouter's list now answers from another address, so no cross-check was made";
  else if (or && !or.unreachable && or.status === 200) {
    const parsed = parseOpenRouter(or.text);
    if (parsed.rows.size > 0) {
      const driftedWires = new Set(
        findings
          .filter((f) => f.id === "price-drift")
          .map((f) => `${f.provider}/${f.fields.model}`),
      );
      notices.push(
        ...crossCheck({
          catalogue,
          rows: parsed.rows,
          today,
          vendors: drift.pages[or.key].vendor_prefix,
          // "The vendor agreed": its page was read for this model and no price finding was raised.
          agreed: (provider, wire) =>
            agreedWith.has(`${provider}/${wire}`) && !driftedWires.has(`${provider}/${wire}`),
        }),
      );
    } else note = "OpenRouter's list did not read as expected, so no cross-check was made";
  } else if (or && !or.unreachable)
    note = "OpenRouter did not answer 200, so no cross-check was made";

  notices.push(...stepsDue(catalogue, today));

  // Every provider unchecked: nothing was judged at all, so the run says "inconclusive", never "ok".
  const status = unchecked.size === PROVIDERS.length ? "inconclusive" : "ok";
  const result = makeResult(status, {
    findings: status === "ok" ? capFindings(findings) : [],
    // Notices are summary lines: past the cap (a broken OpenRouter list gives one per model) the rest is dropped.
    notices: notices.slice(0, MAX_FINDINGS),
    note,
    unchecked: [...unchecked],
  });
  return result;
}

/** The summary lines for a drift result: public, so only template sentences. */
export function driftSummaryLines(result, today) {
  const lines = [
    `## Provider drift check, ${today}`,
    `Status: ${result.status}${result.note ? ` (${result.note})` : ""}`,
  ];
  for (const provider of PROVIDERS) {
    lines.push("", `### ${PROVIDER_NAMES[provider]}`);
    if (result.unchecked?.includes(provider))
      lines.push("Not fully checked this week; its issue was left alone.");
    const own = result.findings.filter((f) => f.provider === provider);
    for (const f of own)
      lines.push(`- FINDING (${describe(f).severity}) ${f.id}: ${describe(f).detail}`);
    for (const n of result.notices.filter((x) => x.provider === provider)) {
      const who = [n.model, n.date].filter(Boolean).join(" ");
      lines.push(`- notice ${n.id}${who ? ` (${who})` : ""}: ${NOTICES[n.id]}`);
    }
    if (own.length === 0 && result.status === "ok" && !result.unchecked?.includes(provider))
      lines.push("No findings.");
  }
  return lines;
}

/** Entry point: run on the committed files, print, write the one output, exit non-zero on an error. */
export async function main(env = process.env, deps = {}) {
  const today = deps.today ?? new Date().toISOString().slice(0, 10);
  let result;
  try {
    result = await runDrift({
      sources: deps.sources ?? loadSources(),
      catalogue: deps.catalogue ?? loadCatalogue(),
      today,
      deps,
    });
  } catch (e) {
    // Only the error's name: a message could carry a page's text.
    result = makeResult("error", {
      note: `the script crashed (${e && e.name ? e.name : "unknown"})`,
    });
  }
  log({ provider: "report", status: result.status, count: result.findings.length });
  summary(driftSummaryLines(result, today), env);
  setOutput("result", JSON.stringify(result), env);
  if (result.status === "error") process.exitCode = 1;
  return result;
}

if (isEntry(import.meta.url)) await main();
