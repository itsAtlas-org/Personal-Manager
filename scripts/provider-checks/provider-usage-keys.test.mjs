// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Offline validation of src-tauri/provider_usage_keys.json: the one allow-list the Rust usage
// reader (PR 4a) and the weekly provider canary (PR 4e) both read. A typo here would make the
// canary cry "new token type" for a key PM already prices, or, worse, stay quiet about one it does
// not.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, it } from "node:test";
import assert from "node:assert/strict";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const raw = readFileSync(join(root, "src-tauri", "provider_usage_keys.json"), "utf8");
const file = JSON.parse(raw);

const PROVIDERS = ["openai", "gemini", "anthropic"];
const PATH = /^[A-Za-z][A-Za-z0-9_]*(\.[A-Za-z][A-Za-z0-9_]*)*$/;

describe("provider_usage_keys.json", () => {
  it("has the three providers and nothing else", () => {
    assert.equal(file.schema_version, 1);
    assert.deepEqual(Object.keys(file.providers).sort(), [...PROVIDERS].sort());
  });

  it("lists each provider's three groups in the right shape", () => {
    for (const p of PROVIDERS) {
      const g = file.providers[p];
      assert.deepEqual(Object.keys(g).sort(), ["known_info", "known_priced", "known_unpriced"], p);
      assert.ok(
        Array.isArray(g.known_priced) && g.known_priced.length > 0,
        `${p} prices something`,
      );
      assert.ok(Array.isArray(g.known_info), p);
      assert.ok(
        g.known_unpriced &&
          typeof g.known_unpriced === "object" &&
          !Array.isArray(g.known_unpriced),
        p,
      );
    }
  });

  it("uses well-formed dotted paths, each in exactly one group", () => {
    for (const p of PROVIDERS) {
      const g = file.providers[p];
      const all = [...g.known_priced, ...g.known_info, ...Object.keys(g.known_unpriced)];
      for (const k of all) assert.match(k, PATH, `${p}: ${k}`);
      assert.equal(new Set(all).size, all.length, `${p}: a key sits in two groups or twice`);
    }
  });

  it("maps every unpriced key to a slug from the closed list", () => {
    assert.ok(Array.isArray(file.slugs) && new Set(file.slugs).size === file.slugs.length);
    const used = new Set();
    for (const p of PROVIDERS) {
      for (const [k, slug] of Object.entries(file.providers[p].known_unpriced)) {
        assert.ok(file.slugs.includes(slug), `${p}: ${k} -> ${slug} is not a listed slug`);
        used.add(slug);
      }
    }
    for (const s of file.slugs)
      assert.ok(used.has(s), `slug ${s} is listed but nothing maps to it`);
  });

  it("holds the keys PM prices today (each provider's real replies)", () => {
    const priced = (p) => new Set(file.providers[p].known_priced);
    for (const k of ["prompt_tokens", "completion_tokens", "prompt_tokens_details.cached_tokens"]) {
      assert.ok(priced("openai").has(k), `openai ${k}`);
      assert.ok(priced("gemini").has(k), `gemini ${k}`);
    }
    assert.ok(priced("openai").has("prompt_tokens_details.cache_write_tokens"));
    for (const k of [
      "input_tokens",
      "cache_creation_input_tokens",
      "cache_read_input_tokens",
      "output_tokens",
    ]) {
      assert.ok(priced("anthropic").has(k), `anthropic ${k}`);
    }
    // Thinking is already inside output and is never priced twice.
    assert.ok(
      file.providers.anthropic.known_info.includes("output_tokens_details.thinking_tokens"),
    );
    assert.ok(
      file.providers.openai.known_info.includes("completion_tokens_details.reasoning_tokens"),
    );
    // One-hour cache writes cost 2x, not the 1.25x PM would charge.
    assert.equal(
      file.providers.anthropic.known_unpriced["cache_creation.ephemeral_1h_input_tokens"],
      "partial_usage",
    );
  });

  it("is plain data: ends in one newline, no tabs, no carriage returns", () => {
    assert.ok(raw.endsWith("}\n") && !raw.endsWith("\n\n"));
    assert.ok(!raw.includes("\t") && !raw.includes("\r"));
  });
});
