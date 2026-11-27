// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Gemini leg against Google's recorded replies (build day 2026-11-26) through a fake fetch, and
// against deliberately drifted copies. Offline: no key, no network.

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { runLeg } from "./lib/leg.mjs";
import {
  assertIdentity,
  assertTierProbe,
  configured,
  isFreeTier429,
  run,
} from "./canary-gemini.mjs";
import { deps, fakeFetch, fx, quiet } from "./testkit.mjs";

quiet();
const KEY = "test-key-not-real-0123456789";
const env = { GEMINI_CANARY_KEY: KEY };
const same = (x) => x;

function server({ edit = {}, cacheHits = true } = {}) {
  let cacheCalls = 0;
  return fakeFetch((url, init) => {
    const u = new URL(url);
    const body = init.body ? JSON.parse(init.body) : null;
    if (u.pathname === "/v1beta/models")
      return { body: (edit.models ?? same)(fx("gemini-models.json")) };
    if (u.pathname.endsWith(":generateContent"))
      return (edit.probe ?? same)({ status: 200, body: fx("gemini-tier-probe-200.json") });
    if (body?.store === false)
      return (edit.error ?? same)({ status: 400, body: fx("gemini-store-400.json") });
    if (body?.reasoning_effort === "high") {
      // A reply to the thinking request. Real numbers from build day: completion 11, prompt 32, total 74
      // (the 31 thinking tokens sit only in the total).
      const r = structuredClone(fx("gemini-chat.json"));
      r.usage = { completion_tokens: 11, prompt_tokens: 32, total_tokens: 74 };
      return (edit.think ?? same)({ status: 200, body: r });
    }
    if (body?.stream) return { body: (edit.stream ?? same)(fx("gemini-stream.txt")) };
    if (body.messages[0].content.startsWith("Reference sheet")) {
      cacheCalls += 1;
      const r = structuredClone(
        fx(cacheCalls > 1 && cacheHits ? "gemini-cache-second.json" : "gemini-chat.json"),
      );
      return { body: r };
    }
    const chat = structuredClone(fx("gemini-chat.json"));
    return (edit.chat ?? same)({ status: 200, body: chat });
  });
}
const go = (f, e = env) => runLeg({ provider: "gemini", configured, run, env: e, deps: deps(f) });
const ids = (r) => r.findings.map((f) => f.id);

describe("Gemini leg", () => {
  it("is clean on the recorded replies; the key rides in headers, never a URL; the probe is one token", async () => {
    const f = server();
    const r = await go(f);
    assert.equal(r.status, "ok");
    assert.deepEqual(r.findings, []);
    for (const c of f.calls) {
      assert.equal(new URL(c.url).origin, "https://generativelanguage.googleapis.com");
      assert.ok(!c.url.includes(KEY) && !/[?&]key=/.test(c.url), "no key in a URL");
    }
    const native = f.calls.find((c) => c.url.endsWith("/v1beta/models?pageSize=1000"));
    assert.equal(native.init.headers["x-goog-api-key"], KEY);
    const compat = f.calls.find((c) => c.body?.messages);
    assert.equal(compat.init.headers.authorization, `Bearer ${KEY}`);
    assert.equal(compat.body.model, "gemini-3.5-flash-lite");
    assert.ok(
      !("store" in compat.body),
      "the first chat is PM's own body, which never carries store",
    );
    const probe = f.calls.find((c) => c.url.includes(":generateContent"));
    assert.ok(probe.url.includes("/models/gemini-3.1-pro-preview:generateContent"));
    assert.deepEqual(probe.body, {
      contents: [{ parts: [{ text: "ok" }] }],
      generationConfig: { maxOutputTokens: 1 },
    });
  });

  it("a cache that never hits is a NOTICE, never a finding", async () => {
    const r = await go(server({ cacheHits: false }));
    assert.equal(r.status, "ok");
    assert.deepEqual(r.findings, []);
    assert.deepEqual(r.notices, [{ id: "implicit-cache-not-seen", provider: "gemini" }]);
  });

  it("flags a lost model, changed limits, and a model that can no longer generateContent", async () => {
    const edit = (fn) => ({ models: (m) => ({ models: fn(m.models) }) });
    assert.deepEqual(
      ids(
        await go(server({ edit: edit((l) => l.filter((x) => x.name !== "models/gemini-4-neon")) })),
      ),
      ["model-missing"],
    );
    const lim = await go(
      server({
        edit: edit((l) =>
          l.map((x) =>
            x.name === "models/gemini-4-carbon" ? { ...x, outputTokenLimit: 65536 } : x,
          ),
        ),
      }),
    );
    assert.deepEqual(
      lim.findings.map((f) => [f.id, f.fields.field, f.fields.listed, f.fields.catalogue]),
      [["model-limits-changed", "outputTokenLimit", "65536", "32768"]],
    );
    const gen = await go(
      server({
        edit: edit((l) =>
          l.map((x) =>
            x.name === "models/gemini-3.8-flash"
              ? { ...x, supportedGenerationMethods: ["countTokens"] }
              : x,
          ),
        ),
      }),
    );
    assert.deepEqual(ids(gen), ["model-capability-changed"]);
  });

  it("flags the usage identity breaking: total below prompt plus completion, or a count gone", async () => {
    const r = await go(
      server({
        edit: {
          chat: (x) => {
            x.body.usage.total_tokens = 3;
            return x;
          },
        },
      }),
    );
    assert.deepEqual(
      r.findings.map((f) => [f.id, f.fields.what]),
      [["gemini-usage-identity", "total is below prompt plus completion"]],
    );
    const gone = await go(
      server({
        edit: {
          chat: (x) => {
            delete x.body.usage.total_tokens;
            return x;
          },
        },
      }),
    );
    assert.deepEqual(ids(gone).sort(), ["gemini-usage-identity", "usage-field-missing"]);
    assert.deepEqual(
      assertIdentity({ prompt_tokens: 5, completion_tokens: 0, total_tokens: 34 }),
      [],
      "the spike's reply: thinking sits only in the total",
    );
  });

  it("asks once more, with thinking switched on, and notes a reply with no thinking and flags a total below the sum", async () => {
    const f = server();
    const clean = await go(f);
    assert.deepEqual(clean.findings, []);
    const think = f.calls.filter((c) => c.body?.reasoning_effort === "high");
    assert.equal(think.length, 1, "one try when thinking shows in the total");
    assert.equal(think[0].body.max_tokens, 1024);
    const flat = (x) => {
      x.body.usage.total_tokens = 43; // = prompt + completion: thinking no longer in the total
      return x;
    };
    const g = server({ edit: { think: flat } });
    const r = await go(g);
    assert.deepEqual(r.findings, [], "a model that did not think is not a broken identity");
    assert.deepEqual(
      r.notices.map((n) => n.id),
      ["thinking-not-seen"],
    );
    assert.equal(
      g.calls.filter((c) => c.body?.reasoning_effort === "high").length,
      2,
      "a second try before judging",
    );
    const below = (x) => {
      x.body.usage.total_tokens = 40; // below prompt + completion: the identity really broke
      return x;
    };
    const b = await go(server({ edit: { think: below } }));
    assert.deepEqual(
      b.findings.map((x) => [x.id, x.fields.what]),
      [["gemini-usage-identity", "total is below prompt plus completion"]],
    );
    assert.deepEqual(b.notices, []);
    const refused = await go(server({ edit: { think: () => ({ status: 400, body: {} }) } }));
    assert.deepEqual(
      refused.findings.map((x) => [x.id, x.fields.step]),
      [["request-refused", "think"]],
    );
  });

  it("a new usage key whose name is long, key-shaped or markup is a finding, never a crash", async () => {
    for (const key of [
      "prompt_tokens_details.cache_write_tokens_details.ttl_12h_tokens_and_more_and_more_x".repeat(
        2,
      ),
      "123e4567-e89b-12d3-a456-426614174000",
      "@BBYU26 [click](http://evil.example)",
      "sk-" + "a1B2c3D4e5".repeat(4),
    ]) {
      const r = await go(
        server({
          edit: {
            chat: (x) => {
              x.body.usage[key] = 4;
              return x;
            },
          },
        }),
      );
      assert.equal(r.status, "ok", key);
      assert.deepEqual(ids(r), ["new-token-type"], key);
      const field = r.findings[0].fields.field;
      assert.match(field, /^[A-Za-z0-9_.,:?-]{1,120}$/, "a plain word");
      assert.ok(!/123e4567|sk-a1/.test(field), "a key-shaped name is replaced, not echoed");
    }
  });

  it("flags a new token type in the usage", async () => {
    const r = await go(
      server({
        edit: {
          chat: (x) => {
            x.body.usage.tool_use_tokens = 4;
            return x;
          },
        },
      }),
    );
    assert.deepEqual(
      r.findings.map((f) => [f.id, f.fields.field]),
      [["new-token-type", "tool_use_tokens"]],
    );
  });

  it("flags a stream with no usage, and a compatible error that is no longer an array", async () => {
    const strip = (t) => t.replace(/,"usage":\{[^}]*\}/g, "");
    assert.deepEqual(ids(await go(server({ edit: { stream: strip } }))), ["stream-usage-missing"]);
    const obj = await go(
      server({
        edit: {
          error: () => ({
            status: 400,
            body: { error: { code: 400, status: "INVALID_ARGUMENT" } },
          }),
        },
      }),
    );
    assert.deepEqual(ids(obj), ["error-envelope-changed"]);
  });

  it("notes (no finding) that `store` is now accepted", async () => {
    const r = await go(
      server({ edit: { error: () => ({ status: 200, body: fx("gemini-chat.json") }) } }),
    );
    assert.deepEqual(r.findings, []);
    assert.deepEqual(
      r.notices.map((n) => n.id),
      ["store-accepted"],
    );
  });

  it("the paid-tier probe: 200 with usage is fine; a changed answer is a finding; a free-tier 429 is a setup ERROR", async () => {
    assert.deepEqual(assertTierProbe({ status: 200, json: fx("gemini-tier-probe-200.json") }), []);
    assert.equal(assertTierProbe({ status: 404, json: {} })[0].id, "tier-probe-changed");
    assert.equal(
      assertTierProbe({ status: 200, json: { candidates: [] } })[0].id,
      "tier-probe-changed",
    );
    const free = { status: 429, text: JSON.stringify(fx("gemini-free-429.json")) };
    assert.equal(isFreeTier429(free), true);
    assert.equal(
      isFreeTier429({ status: 429, text: '{"error":{"status":"RESOURCE_EXHAUSTED"}}' }),
      false,
    );
    const r = await go(
      server({ edit: { probe: () => ({ status: 429, body: fx("gemini-free-429.json") }) } }),
    );
    assert.deepEqual(
      [r.status, r.note, r.findings],
      ["error", "the canary key is not on a paid Gemini project", []],
    );
  });

  it("a plain 429 on the tier probe (busy, not the free-tier one) is INCONCLUSIVE, never a finding", async () => {
    const r = await go(
      server({
        edit: {
          probe: () => ({
            status: 429,
            body: { error: { code: 429, status: "RESOURCE_EXHAUSTED", message: "Try later." } },
          }),
        },
      }),
    );
    assert.deepEqual([r.status, r.findings], ["inconclusive", []]);
  });

  it("an error event inside a 200 stream is INCONCLUSIVE, not a changed stream", async () => {
    const cut = () => 'data: {"error":{"code":503,"status":"UNAVAILABLE"}}\n\n';
    const r = await go(server({ edit: { stream: cut } }));
    assert.deepEqual([r.status, r.findings], ["inconclusive", []]);
  });

  it("walks the first cache reply too: a new key that only a long prompt shows is found", async () => {
    const f = fakeFetch((url, init) => {
      const u = new URL(url);
      const body = init.body ? JSON.parse(init.body) : null;
      if (u.pathname === "/v1beta/models") return { body: fx("gemini-models.json") };
      if (u.pathname.endsWith(":generateContent"))
        return { body: fx("gemini-tier-probe-200.json") };
      if (body?.store === false) return { status: 400, body: fx("gemini-store-400.json") };
      if (body?.stream) return { body: fx("gemini-stream.txt") };
      const r = structuredClone(fx("gemini-chat.json"));
      if (body?.reasoning_effort === "high")
        r.usage = { completion_tokens: 11, prompt_tokens: 32, total_tokens: 74 };
      if (body.messages[0].content.startsWith("Reference sheet")) r.usage.cache_storage_tokens = 9;
      return { body: r };
    });
    const r = await go(f);
    assert.deepEqual(
      r.findings.map((x) => [x.id, x.fields.step, x.fields.field]),
      [["new-token-type", "cache", "cache_storage_tokens"]],
    );
  });

  it("is skipped with no key, inconclusive on a Google 503, an ERROR on a refused key", async () => {
    assert.equal((await go(server(), {})).status, "skipped");
    assert.equal(
      (
        await go(
          fakeFetch(() => ({
            status: 503,
            body: [{ error: { status: "UNAVAILABLE", message: "high demand" } }],
          })),
        )
      ).status,
      "inconclusive",
    );
    const bad = await go(
      fakeFetch(() => ({ status: 403, body: { error: { status: "PERMISSION_DENIED" } } })),
    );
    assert.deepEqual([bad.status, bad.findings], ["error", []]);
  });
});
