// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Ties the canary to PM: the golden request bodies (which the Rust test
// request_bodies_match_the_canary_golden pins to PM's real builders) and the usage-key allow-list must
// agree with the catalogue and with what each leg asserts, so the canary can never test a request or a
// field PM does not send or read.

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { catalogueFor, loadCatalogue, loadGolden, loadUsageKeys } from "./lib/data.mjs";
import { REQUIRED_USAGE as OPENAI_REQUIRED } from "./canary-openai.mjs";
import { REQUIRED_USAGE as GEMINI_REQUIRED } from "./canary-gemini.mjs";
import {
  DELTA_USAGE,
  REQUIRED_USAGE as ANTHROPIC_REQUIRED,
  START_USAGE,
} from "./canary-anthropic.mjs";

const golden = loadGolden();
const catalogue = loadCatalogue();
const keys = loadUsageKeys();

describe("golden request bodies", () => {
  it("name a model the catalogue lists, and use only a level that model offers", () => {
    for (const p of ["openai", "gemini", "anthropic"]) {
      const section = catalogueFor(catalogue, p);
      const wire = golden.providers[p].wire_model;
      const model = section.models.find((m) => m.wire === wire);
      assert.ok(model, `${p}: ${wire} is in direct_models.json`);
      for (const [name, c] of Object.entries(golden.providers[p].cases)) {
        if (name === "tier_probe") continue; // a native body for another model: the probe model, not the chat one
        assert.equal(c.body.model, wire, `${p}/${name}`);
        const effort = c.body.reasoning_effort ?? c.body.output_config?.effort;
        if (effort) assert.ok(model.effort_levels.includes(effort), `${p}/${name}: ${effort}`);
        const limit = c.body.max_completion_tokens ?? c.body.max_tokens;
        assert.ok(limit >= 1 && limit <= model.max_output, `${p}/${name} limit`);
      }
    }
  });

  it("use the cheapest catalogue model of OpenAI and Anthropic (the canary's cost claim)", () => {
    for (const p of ["openai", "anthropic"]) {
      const section = catalogueFor(catalogue, p);
      const cheapest = [...section.models].sort(
        (a, b) => a.price_in + a.price_out - (b.price_in + b.price_out),
      )[0];
      assert.equal(golden.providers[p].wire_model, cheapest.wire, p);
    }
  });

  it("carry the fields PM sends and none it never does", () => {
    for (const [name, c] of Object.entries(golden.providers.openai.cases)) {
      assert.equal(c.body.store, false, `openai/${name} sends store:false`);
      assert.ok(!("max_tokens" in c.body), "openai uses max_completion_tokens");
    }
    for (const c of Object.values(golden.providers.gemini.cases))
      assert.ok(!("store" in c.body), "gemini never sends store");
    for (const c of Object.values(golden.providers.anthropic.cases))
      assert.ok(!("temperature" in c.body));
    assert.equal(
      golden.providers.anthropic.cases.stream.body.system[0].cache_control.type,
      "ephemeral",
    );
    assert.equal(golden.providers.openai.cases.stream.body.stream_options.include_usage, true);
  });

  it("hold no credential of any shape", () => {
    const raw = JSON.stringify(golden);
    for (const re of [/sk-/i, /AIza/, /Bearer/i, /eyJ/, /api[_-]?key/i])
      assert.doesNotMatch(raw, re);
  });
});

describe("what the legs require of a usage report", () => {
  const known = (p) =>
    new Set([
      ...keys.providers[p].known_priced,
      ...keys.providers[p].known_info,
      ...Object.keys(keys.providers[p].known_unpriced),
    ]);

  it("is inside provider_usage_keys.json, so a required field is never reported as a new token type", () => {
    for (const [p, required] of [
      ["openai", OPENAI_REQUIRED],
      ["gemini", GEMINI_REQUIRED],
      ["anthropic", [...ANTHROPIC_REQUIRED, ...START_USAGE, ...DELTA_USAGE]],
    ]) {
      for (const field of required) assert.ok(known(p).has(field), `${p}: ${field}`);
    }
  });

  it("covers every key PM prices", () => {
    const required = {
      openai: OPENAI_REQUIRED,
      gemini: GEMINI_REQUIRED,
      anthropic: ANTHROPIC_REQUIRED,
    };
    for (const p of ["openai", "anthropic"]) {
      for (const field of keys.providers[p].known_priced)
        assert.ok(
          required[p].includes(field),
          `${p} prices ${field} but the canary does not require it`,
        );
    }
    for (const field of ["prompt_tokens", "completion_tokens"])
      assert.ok(GEMINI_REQUIRED.includes(field));
  });
});
