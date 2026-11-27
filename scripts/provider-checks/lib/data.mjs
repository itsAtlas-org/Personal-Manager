// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The files the canary reads, all committed and all PM's own: the catalogue (direct_models.json),
// the golden request bodies (the ones PM's real builders make), the usage-key allow-list
// (provider_usage_keys.json) and sources.json (the vendors' API hosts). Plus three small pure helpers
// the legs share: the prompt padding for the cache pairs, an event-stream reader and a usage walker.
//
// Pure Node built-ins, ESM, no dependencies (INVARIANTS.md I-18).

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, "..", "..", "..");
const readJson = (rel) => JSON.parse(readFileSync(join(repoRoot, rel), "utf8"));

/** The markers in the golden bodies where the canary's own prompt goes. */
export const SYSTEM = "<<SYSTEM>>";
export const USER = "<<USER>>";

export function loadCatalogue(read = readJson) {
  return read("src-tauri/direct_models.json");
}
export function loadGolden(read = readJson) {
  return read("src-tauri/tests/golden/request_bodies.json");
}
export function loadUsageKeys(read = readJson) {
  return read("src-tauri/provider_usage_keys.json");
}
export function loadSources(read = readJson) {
  return read("scripts/provider-checks/sources.json");
}

/** One provider's section of the catalogue. */
export function catalogueFor(catalogue, provider) {
  const section = catalogue.providers.find((p) => p.provider === provider);
  if (!section) throw new Error(`the catalogue has no ${provider} section`);
  return section;
}

/**
 * A golden body with the canary's own prompt in place of the two markers. Throws if the case is
 * missing or a marker is not found, so a golden file that lost its markers is a loud failure and not
 * a canary that quietly sends something else. `mutate` edits the finished body (the probes that want a
 * different model or one extra field).
 */
export function bodyFor(golden, provider, name, { system, user, mutate } = {}) {
  const c = golden.providers?.[provider]?.cases?.[name];
  if (!c || typeof c.body !== "object") throw new Error(`golden: no ${provider}/${name} case`);
  const seen = { [SYSTEM]: 0, [USER]: 0 };
  const walk = (v) => {
    if (typeof v === "string") {
      if (v === SYSTEM || v === USER) {
        seen[v] += 1;
        return v === SYSTEM ? (system ?? "You are a terse test assistant.") : (user ?? "Say OK.");
      }
      return v;
    }
    if (Array.isArray(v)) return v.map(walk);
    if (v && typeof v === "object")
      return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, walk(x)]));
    return v;
  };
  const body = walk(c.body);
  if (!name.endsWith("probe") && name !== "key_check" && (seen[SYSTEM] === 0 || seen[USER] === 0)) {
    throw new Error(`golden: ${provider}/${name} lost a prompt marker`);
  }
  return mutate ? mutate(body) : body;
}

/**
 * Padding for a cache pair: numbered lines of plain text, led by a per-run nonce so the first call of
 * a pair is always a write and never a hit on an earlier run's prefix. About 14 tokens a line.
 */
export function padding(lines, nonce) {
  const out = [`Reference sheet ${nonce}. Read it, then answer the question after it in one word.`];
  for (let i = 1; i <= lines; i++) {
    out.push(
      `Entry ${i}: the garden gate stays closed on Tuesdays, and the blue ledger lists item ${i * 7} twice.`,
    );
  }
  return out.join("\n");
}

/**
 * Events of a text/event-stream body: `[{ event, data }]`, `data` parsed as JSON (a real parser, not
 * string matching: Anthropic pads its data lines with spaces inside the object) or the string
 * "[DONE]". A data line that does not parse is kept as `{ __unparsed: true }` so the caller can flag it.
 */
export function parseSse(text) {
  const events = [];
  for (const block of String(text).replace(/\r\n/g, "\n").split("\n\n")) {
    let event = null;
    const data = [];
    for (const line of block.split("\n")) {
      if (line.startsWith("event:")) event = line.slice(6).trim();
      else if (line.startsWith("data:")) data.push(line.slice(5).trim());
    }
    if (data.length === 0) continue;
    const raw = data.join("\n");
    if (raw === "[DONE]") {
      events.push({ event, data: "[DONE]" });
      continue;
    }
    try {
      events.push({ event, data: JSON.parse(raw) });
    } catch {
      events.push({ event, data: { __unparsed: true } });
    }
  }
  return events;
}

/**
 * Whether a stream carries an ERROR event: Anthropic's `event: error` (`overloaded_error` mid-stream),
 * or an `error` object in a chunk (OpenAI, Gemini; Gemini may wrap it in an array). A 200 stream that
 * ends in one is the vendor being busy, never drift, so the caller treats it as inconclusive.
 */
export function hasStreamError(events) {
  const isError = (d) =>
    d && typeof d === "object" && (d.type === "error" || (d.error && typeof d.error === "object"));
  return events.some(
    (e) =>
      e.event === "error" ||
      isError(e.data) ||
      (Array.isArray(e.data) && e.data.some((x) => isError(x))),
  );
}

/** Every numeric leaf of a usage object by dotted path. Arrays and non-numbers are not walked. */
export function numericLeaves(usage, prefix = "") {
  const out = {};
  if (!usage || typeof usage !== "object" || Array.isArray(usage)) return out;
  for (const [k, v] of Object.entries(usage)) {
    const path = prefix ? `${prefix}.${k}` : k;
    if (typeof v === "number") out[path] = v;
    else if (v && typeof v === "object" && !Array.isArray(v))
      Object.assign(out, numericLeaves(v, path));
  }
  return out;
}

/**
 * Walk a usage object against provider_usage_keys.json. `unknown`: numeric keys in no group (a new
 * token type). `unpriced`: known-unpriced keys with a value above 0, with their slug.
 */
export function walkUsage(keys, provider, usage) {
  const g = keys.providers[provider];
  const known = new Set([...g.known_priced, ...g.known_info, ...Object.keys(g.known_unpriced)]);
  const unknown = [];
  const unpriced = [];
  for (const [path, value] of Object.entries(numericLeaves(usage))) {
    if (!known.has(path)) unknown.push(path);
    else if (Object.hasOwn(g.known_unpriced, path) && value > 0)
      unpriced.push({ key: path, slug: g.known_unpriced[path], value });
  }
  return { unknown, unpriced };
}

/** The value at a dotted path, or undefined. */
export function at(obj, path) {
  return path.split(".").reduce((o, k) => (o && typeof o === "object" ? o[k] : undefined), obj);
}
