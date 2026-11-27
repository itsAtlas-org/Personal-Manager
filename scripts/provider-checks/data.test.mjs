// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  SYSTEM,
  USER,
  bodyFor,
  loadGolden,
  loadUsageKeys,
  numericLeaves,
  padding,
  parseSse,
  walkUsage,
} from "./lib/data.mjs";
import { fx } from "./testkit.mjs";

const golden = loadGolden();
const keys = loadUsageKeys();

describe("bodyFor", () => {
  it("puts the canary's prompt where the markers are, and nowhere else", () => {
    const b = bodyFor(golden, "openai", "plain", { system: "SYS TEXT", user: "USER TEXT" });
    assert.deepEqual(b.messages, [
      { role: "system", content: "SYS TEXT" },
      { role: "user", content: "USER TEXT" },
    ]);
    assert.equal(b.store, false);
    assert.ok(!JSON.stringify(b).includes("<<"));
  });

  it("fails loudly when a case is missing or has lost its markers", () => {
    assert.throws(() => bodyFor(golden, "openai", "nope"), /no openai\/nope case/);
    const bad = structuredClone(golden);
    bad.providers.openai.cases.plain.body.messages[1].content = "no marker here";
    assert.throws(() => bodyFor(bad, "openai", "plain"), /lost a prompt marker/);
    // the probes that carry no prompt marker are allowed to
    assert.ok(bodyFor(golden, "gemini", "tier_probe").contents);
  });

  it("does not alter the golden object it reads", () => {
    const before = JSON.stringify(golden);
    bodyFor(golden, "anthropic", "stream", { system: "x" });
    assert.equal(JSON.stringify(golden), before);
  });

  it("uses the same markers the Rust golden test writes", () => {
    assert.equal(golden.markers.system, SYSTEM);
    assert.equal(golden.markers.user, USER);
  });
});

describe("padding", () => {
  it("is long enough for a cache, starts with the run's nonce, and differs between runs", () => {
    const a = padding(80, "n1");
    assert.ok(a.length > 6000);
    assert.ok(a.startsWith("Reference sheet n1."));
    assert.notEqual(padding(80, "n2"), a);
  });
});

describe("parseSse", () => {
  it("reads Anthropic's recorded stream, padding spaces inside the data lines and all", () => {
    const events = parseSse(fx("anthropic-stream.txt"));
    assert.deepEqual(
      events.map((e) => e.event),
      [
        "message_start",
        "content_block_start",
        "ping",
        "content_block_delta",
        "content_block_delta",
        "content_block_stop",
        "message_delta",
        "message_stop",
      ],
    );
    assert.equal(events[6].data.usage.output_tokens, 11);
    assert.ok(events.every((e) => !e.data.__unparsed));
  });

  it("reads an OpenAI-style stream to [DONE] and flags a line that is not JSON", () => {
    const e = parseSse(fx("openai-stream.txt"));
    assert.equal(e.at(-1).data, "[DONE]");
    assert.equal(parseSse("data: {not json}\n\n")[0].data.__unparsed, true);
    assert.deepEqual(parseSse(""), []);
  });
});

describe("walkUsage on the vendors' real replies", () => {
  it("finds nothing new in the recorded OpenAI, Gemini and Anthropic usage", () => {
    const real = {
      openai: fx("openai-chat.json").usage,
      gemini: fx("gemini-chat.json").usage,
      anthropic: fx("anthropic-message.json").usage,
    };
    for (const [p, usage] of Object.entries(real)) {
      assert.deepEqual(walkUsage(keys, p, usage), { unknown: [], unpriced: [] }, p);
    }
    const cached = fx("gemini-cache-second.json").usage;
    assert.deepEqual(walkUsage(keys, "gemini", cached), { unknown: [], unpriced: [] });
  });

  it("flags a new numeric key, and a known-unpriced key above zero, but not at zero", () => {
    const usage = structuredClone(fx("openai-chat.json").usage);
    usage.prompt_tokens_details.video_tokens = 7;
    usage.completion_tokens_details.audio_tokens = 3;
    const r = walkUsage(keys, "openai", usage);
    assert.deepEqual(r.unknown, ["prompt_tokens_details.video_tokens"]);
    assert.deepEqual(r.unpriced, [
      { key: "completion_tokens_details.audio_tokens", slug: "audio_tokens", value: 3 },
    ]);
    assert.deepEqual(walkUsage(keys, "openai", fx("openai-chat.json").usage).unpriced, []);
  });

  it("reads numbers by dotted path and ignores strings, nulls and arrays", () => {
    assert.deepEqual(numericLeaves({ a: 1, b: { c: 2, d: "x", e: null, f: [3] }, g: "y" }), {
      a: 1,
      "b.c": 2,
    });
    assert.deepEqual(numericLeaves(null), {});
  });
});
