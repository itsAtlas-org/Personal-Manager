// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Helpers for the offline tests: the recorded fixtures, a fake fetch that answers by route and
// records what was asked, and a fake `gh` that records its calls. Nothing here touches the network.
//
// The fixtures in ./fixtures are the vendors' real replies from the 2026-11-26 probes, trimmed, with
// every id of an account replaced by "fixture" and no key anywhere. Three are assembled by hand from the
// vendors' documented shapes because no real reply was recorded: openai-stream.txt (PM's own stream
// test has the same extra usage chunk), openai-404.json and anthropic-404.json; and the Anthropic
// model list is the one real Fable row recorded, with the other three catalogue rows copied from it.
//
// Pure Node built-ins, ESM, no dependencies (INVARIANTS.md I-18).

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { setSink } from "./lib/log.mjs";
import { loadCatalogue, loadGolden, loadUsageKeys, loadSources } from "./lib/data.mjs";

const here = dirname(fileURLToPath(import.meta.url));

/** A fixture: parsed if it ends .json, else the raw text. */
export function fx(name) {
  const text = readFileSync(join(here, "fixtures", name), "utf8");
  return name.endsWith(".json") ? JSON.parse(text) : text;
}

/** Dependencies for makeContext: the real data files, no sleeping, a fixed nonce. */
export function deps(fetch) {
  return {
    fetch,
    sleep: async () => {},
    nonce: "testnonce",
    sources: loadSources(),
    golden: loadGolden(),
    catalogue: loadCatalogue(),
    usageKeys: loadUsageKeys(),
  };
}

/**
 * A fake fetch. `answer(url, init, n)` returns `{ status, body, headers }` (body an object, JSON-encoded,
 * or a string sent as is) or throws to simulate a network error. Every request is pushed to `calls`.
 */
export function fakeFetch(answer) {
  const calls = [];
  const fetch = async (url, init = {}) => {
    const n = calls.length;
    calls.push({ url: String(url), init, body: init.body ? safeJson(init.body) : undefined });
    const r = await answer(String(url), init, n);
    const body = typeof r.body === "string" ? r.body : JSON.stringify(r.body ?? {});
    return new Response(body, { status: r.status ?? 200, headers: r.headers ?? {} });
  };
  fetch.calls = calls;
  return fetch;
}

function safeJson(s) {
  try {
    return JSON.parse(s);
  } catch {
    return s;
  }
}

/** A fake `gh`: `answers` is a function (args, opts) -> stdout string; every call is recorded. */
export function fakeGh(answers = () => "") {
  const calls = [];
  const gh = (args, opts = {}) => {
    calls.push({ args, input: opts.input });
    return answers(args, opts);
  };
  gh.calls = calls;
  return gh;
}

/** Silence the legs' log lines in a test file (they are public-log lines, not test output). */
export function quiet() {
  return setSink(() => {});
}

/** A fake gh with a tiny issue tracker. `open` lists issues already open: { provider, body }. */
export function tracker(open = [], closed = []) {
  const issues = open.map((o, i) => ({
    number: 100 + i,
    state: "open",
    labels: [
      "provider-check",
      `provider-check:${o.provider}`,
      `provider-check:${o.kind ?? "canary"}`,
    ],
    body: o.body,
  }));
  // Issues Bobby closed by hand: `closed` is "not_planned" or "completed".
  for (const c of closed)
    issues.push({
      number: 300 + issues.length,
      state: "closed",
      state_reason: c.reason,
      labels: [
        "provider-check",
        `provider-check:${c.provider}`,
        `provider-check:${c.kind ?? "canary"}`,
      ],
      body: c.body,
    });
  const gh = fakeGh((args, { input } = {}) => {
    if (args[0] === "api") {
      const q = new URL("https://x/" + args[1]).searchParams;
      const labels = q.get("labels").split(",");
      // Newest first, as GitHub's REST list is by default.
      return JSON.stringify(
        issues
          .filter((i) => i.state === q.get("state") && labels.every((l) => i.labels.includes(l)))
          .reverse(),
      );
    }
    if (args[0] === "issue" && args[1] === "create") {
      const labels = args.flatMap((a, i) => (args[i - 1] === "--label" ? [a] : []));
      issues.push({ number: 200 + issues.length, state: "open", labels, body: input });
      return "";
    }
    if (args[0] === "issue" && args[1] === "edit")
      issues.find((i) => i.number === Number(args[2])).body = input;
    if (args[0] === "issue" && args[1] === "close") {
      const issue = issues.find((i) => i.number === Number(args[2]));
      issue.state = "closed";
      issue.state_reason = args[args.indexOf("--reason") + 1].replace("not planned", "not_planned");
    }
    return "";
  });
  gh.issues = issues;
  return gh;
}

// ---- the keyless drift check's fixtures (fixtures/drift) ------------------------------------------
// Real, raw, trimmed excerpts of the vendors' PUBLIC pages as fetched on 2026-11-27 with a plain curl and
// the User-Agent `pm-provider-check`: whole lines cut out, nothing reworded. Each is a `.txt` so Prettier
// leaves its tables alone. gemini-terms.html.txt is the page's own `<noscript><pre>` text with `[...]`
// marking where whole paragraphs were left out (the markers are hand-made). openrouter-models.json keeps
// only the catalogue's rows plus three others, and only id, canonical_slug, name, created, pricing and
// expiration_date of each. The drifted and restructured copies the tests use are made in the tests from
// these (see `drifted`), never committed, and each edit must find its text or the test fails.

/** The page address -> fixture file the drift check reads (the same addresses as sources.json). */
export const DRIFT_PAGES = {
  "https://platform.claude.com/docs/en/about-claude/pricing.md": "drift/anthropic-pricing.md.txt",
  "https://developers.openai.com/api/docs/pricing.md": "drift/openai-pricing.md.txt",
  "https://aistudio.google.com/docs/pricing.md": "drift/gemini-pricing.md.txt",
  "https://openrouter.ai/api/v1/models": "drift/openrouter-models.json",
  "https://platform.claude.com/docs/en/manage-claude/api-and-data-retention.md":
    "drift/anthropic-retention.md.txt",
  "https://developers.openai.com/api/docs/guides/your-data.md": "drift/openai-data.md.txt",
  "https://aistudio.google.com/docs/terms": "drift/gemini-terms.html.txt",
};

/** `text` with `from` replaced by `to`; throws if `from` is not there, so a fixture edit can never silently miss. */
export function drifted(text, from, to) {
  if (!String(text).includes(from)) throw new Error(`drifted: the fixture has no ${from}`);
  // A function replacement, so a `$` in the new text (a price) is never read as a replacement pattern.
  return String(text).replace(from, () => to);
}

/**
 * A fake fetch for the drift check, by EXACT address. `pages` overrides a page's text (a string), its
 * status (`{ status }`), its final address (`{ url }`) or makes the fetch fail (`"network"`), keyed by the
 * address the check asks for. Every request goes to `calls`.
 */
export function driftFetch(pages = {}) {
  const calls = [];
  const fetch = async (url, init = {}) => {
    calls.push({ url: String(url), init });
    const o = pages[url];
    if (o === "network") throw new TypeError("fetch failed");
    const spec = typeof o === "string" ? { text: o } : (o ?? {});
    const file = DRIFT_PAGES[url];
    if (spec.text === undefined && !file)
      return { status: 404, url, headers: new Headers(), text: async () => "" };
    const text = spec.text ?? readFileSync(join(here, "fixtures", file), "utf8");
    return {
      status: spec.status ?? 200,
      url: spec.url ?? url,
      headers: new Headers(),
      text: async () => text,
    };
  };
  fetch.calls = calls;
  return fetch;
}
