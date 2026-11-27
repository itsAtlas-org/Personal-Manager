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
