// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The OpenAI leg against recorded replies (see testkit.mjs) through a fake fetch, and against
// deliberately drifted copies of them. Offline: no key, no network.

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { runLeg } from "./lib/leg.mjs";
import { configured, run } from "./canary-openai.mjs";
import { deps, fakeFetch, fx, quiet } from "./testkit.mjs";
import { assertPublic, issueBody } from "./report.mjs";

quiet();

const KEY = "test-key-not-real-0123456789";
const env = { OPENAI_CANARY_KEY: KEY };

/** A fake OpenAI. `edit` changes a fixture before it is served. */
function server({ edit = {}, cacheHits = true, secondRefused = false } = {}) {
  let cacheCalls = 0;
  return fakeFetch((url, init) => {
    const path = new URL(url).pathname;
    const body = init.body ? JSON.parse(init.body) : null;
    if (path === "/v1/models")
      return { body: (edit.models ?? ((x) => x))(fx("openai-models.json")) };
    if (body?.model === "pm-canary-no-such-model")
      return (edit.unknown ?? ((x) => x))({ status: 404, body: fx("openai-404.json") });
    if (body?.stream) {
      return { body: (edit.stream ?? ((x) => x))(fx("openai-stream.txt")) };
    }
    const chat = structuredClone(fx("openai-chat.json"));
    if (body.messages[0].content.startsWith("Reference sheet")) {
      cacheCalls += 1;
      if (cacheCalls > 1 && secondRefused)
        return { status: 400, body: { error: { message: "Unsupported parameter" } } };
      if (cacheCalls === 1) (edit.cacheFirst ?? ((x) => x))(chat);
      if (cacheCalls > 1 && cacheHits) chat.usage.prompt_tokens_details.cached_tokens = 1536;
      return { body: chat };
    }
    return (edit.chat ?? ((x) => x))({ status: 200, body: chat });
  });
}

const go = (f) => runLeg({ provider: "openai", configured, run, env, deps: deps(f) });
const ids = (r) => r.findings.map((f) => f.id);

describe("OpenAI leg", () => {
  it("is clean on the recorded replies, and sends PM's golden bodies to OpenAI's host with the key in a header only", async () => {
    const f = server();
    const r = await go(f);
    assert.equal(r.status, "ok");
    assert.deepEqual(r.findings, []);
    for (const c of f.calls) {
      assert.equal(new URL(c.url).origin, "https://api.openai.com");
      assert.ok(!c.url.includes(KEY), "the key is never in a URL");
      assert.equal(c.init.headers.authorization, `Bearer ${KEY}`);
    }
    const chats = f.calls.filter((c) => c.body?.messages);
    const plain = chats.find(
      (c) => !c.body.stream && c.body.messages[0].content === "You are a terse test assistant.",
    );
    assert.equal(plain.body.model, "gpt-6-luna");
    assert.equal(plain.body.store, false);
    assert.equal(plain.body.reasoning_effort, "none");
    assert.ok(plain.body.max_completion_tokens && !("max_tokens" in plain.body));
    const streamed = chats.find((c) => c.body.stream);
    assert.deepEqual(streamed.body.stream_options, { include_usage: true });
    assert.equal(
      f.calls.length,
      6,
      "list, chat, stream, two cache calls and the unknown model: nothing more is spent",
    );
  });

  it("flags a model that left the list, and a retirement date the catalogue does not have", async () => {
    const r1 = await go(
      server({
        edit: { models: (m) => ({ ...m, data: m.data.filter((x) => x.id !== "gpt-6-luna") }) },
      }),
    );
    assert.deepEqual(ids(r1), ["model-missing"]);
    const r2 = await go(
      server({
        edit: {
          models: (m) => ({
            ...m,
            data: m.data.map((x) =>
              x.id === "gpt-6.2-sol" ? { ...x, shutdown_date: "2027-03-01" } : x,
            ),
          }),
        },
      }),
    );
    assert.deepEqual(ids(r2), ["model-retirement-changed"]);
    assert.deepEqual(r2.findings[0].fields, {
      model: "gpt-6.2-sol",
      listed: "2027-03-01",
      catalogue: "none",
    });
  });

  it("flags a NEW token type, an unpriced one above zero, and a lost cache-write field", async () => {
    const withUsage = (fn) => ({
      chat: (r) => {
        fn(r.body.usage);
        return r;
      },
    });
    const r1 = await go(
      server({ edit: withUsage((u) => (u.prompt_tokens_details.video_tokens = 5)) }),
    );
    assert.deepEqual(
      r1.findings.map((f) => [f.id, f.fields.field]),
      [["new-token-type", "prompt_tokens_details.video_tokens"]],
    );
    const r2 = await go(
      server({
        edit: withUsage((u) => (u.completion_tokens_details.accepted_prediction_tokens = 9)),
      }),
    );
    assert.deepEqual(ids(r2), ["unpriced-token-seen"]);
    const r3 = await go(
      server({ edit: withUsage((u) => delete u.prompt_tokens_details.cache_write_tokens) }),
    );
    assert.deepEqual(
      r3.findings.map((f) => [f.id, f.fields.field]),
      [["usage-field-missing", "prompt_tokens_details.cache_write_tokens"]],
    );
  });

  it("flags a stream with no final usage chunk, and one that does not end in [DONE]", async () => {
    const noUsage = (t) =>
      t
        .split("\n\n")
        .filter((b) => !b.includes('"choices":[]'))
        .join("\n\n");
    assert.deepEqual(ids(await go(server({ edit: { stream: noUsage } }))), [
      "stream-usage-missing",
    ]);
    const noDone = (t) => t.replace("data: [DONE]\n\n", "");
    assert.deepEqual(ids(await go(server({ edit: { stream: noDone } }))), ["stream-shape-changed"]);
  });

  it("flags a cache that never hits, and an error envelope that changed", async () => {
    assert.deepEqual(ids(await go(server({ cacheHits: false }))), ["cache-never-hit"]);
    const r = await go(
      server({ edit: { unknown: () => ({ status: 404, body: { message: "nope" } }) } }),
    );
    assert.deepEqual(ids(r), ["error-envelope-changed"]);
  });

  it("walks the FIRST cache reply too: a cache-write detail shows only on a real write", async () => {
    const withDetail = (key, n) => (chat) => {
      chat.usage.prompt_tokens_details.cache_write_tokens_details = { [key]: n };
    };
    // the 30-minute write is known and priced inside cache_write_tokens: clean
    assert.deepEqual(
      (await go(server({ edit: { cacheFirst: withDetail("ttl_30m_tokens", 1536) } }))).findings,
      [],
    );
    // a 12-hour write costs more than PM prices: said, with the count
    const r = await go(server({ edit: { cacheFirst: withDetail("ttl_12h_tokens", 1536) } }));
    assert.deepEqual(
      r.findings.map((f) => [f.id, f.fields.step, f.fields.field, f.fields.value]),
      [
        [
          "unpriced-token-seen",
          "cache",
          "prompt_tokens_details.cache_write_tokens_details.ttl_12h_tokens",
          1536,
        ],
      ],
    );
    // a key nobody knew is a new token type (a long dotted name is fine)
    const n = await go(server({ edit: { cacheFirst: withDetail("ttl_99d_tokens", 5) } }));
    assert.deepEqual(
      n.findings.map((f) => [f.id, f.fields.field]),
      [["new-token-type", "prompt_tokens_details.cache_write_tokens_details.ttl_99d_tokens"]],
    );
  });

  it("a cache call refused on the SECOND try is a refusal, not 'never hit'; five tries are made before saying never", async () => {
    const r = await go(server({ secondRefused: true }));
    assert.deepEqual(
      r.findings.map((f) => [f.id, f.fields.step, f.fields.http]),
      [["request-refused", "cache", 400]],
    );
    const f = server({ cacheHits: false });
    await go(f);
    assert.equal(
      f.calls.filter((c) => c.body?.messages?.[0]?.content.startsWith("Reference sheet")).length,
      6,
      "one write and five reads",
    );
  });

  it("an error event inside a 200 stream is INCONCLUSIVE, never a changed stream", async () => {
    const cut = () =>
      'data: {"error":{"message":"The server had an error","type":"server_error"}}\n\n';
    const r = await go(server({ edit: { stream: cut } }));
    assert.deepEqual([r.status, r.findings], ["inconclusive", []]);
  });

  it("hostile vendor text (a mention, a link, an image, HTML) in a usage key or a stop reason never reaches an issue", async () => {
    const evil = {
      "@BBYU26 [click](http://evil.example)": 4,
      "</details>![x](http://e.x/a.png)": 2,
      "www.evil.example": 1,
    };
    const r = await go(
      server({
        edit: {
          chat: (x) => {
            x.body.choices[0].finish_reason = "</details>![x](http://e.x/a.png) @BBYU26";
            Object.assign(x.body.usage, evil);
            return x;
          },
        },
      }),
    );
    assert.equal(r.status, "ok");
    assert.deepEqual(ids(r).sort(), [
      "new-token-type",
      "new-token-type",
      "new-token-type",
      "stop-reason-unknown",
    ]);
    const body = issueBody({
      provider: "openai",
      kind: "canary",
      findings: r.findings,
      date: "27-11-2026",
    });
    assertPublic(body);
    for (const bad of ["@BBYU26", "</details>", "![", "](http", "[click]", "www."]) {
      assert.ok(!body.includes(bad), `the issue body carries ${bad}`);
    }
  });

  it("flags PM's own request being refused", async () => {
    const r = await go(
      server({
        edit: {
          chat: () => ({ status: 400, body: { error: { message: "Unsupported parameter" } } }),
        },
      }),
    );
    assert.deepEqual(
      r.findings.map((f) => [f.id, f.fields.http]),
      [["request-refused", 400]],
    );
  });

  it("is skipped with no key, inconclusive when busy, and an ERROR (not drift) on a refused or empty key", async () => {
    assert.equal(
      (await runLeg({ provider: "openai", configured, run, env: {}, deps: deps(server()) })).status,
      "skipped",
    );
    const busy = await go(fakeFetch(() => ({ status: 503, body: { error: "busy" } })));
    assert.equal(busy.status, "inconclusive");
    assert.deepEqual(busy.findings, []);
    const refused = await go(
      fakeFetch(() => ({ status: 401, body: { error: { code: "invalid_api_key" } } })),
    );
    assert.equal(refused.status, "error");
    assert.match(refused.note, /rejected \(HTTP 401\)/);
    const broke = await go(
      fakeFetch(() => ({
        status: 429,
        body: { error: { code: "insufficient_quota", type: "insufficient_quota" } },
      })),
    );
    assert.deepEqual(
      [broke.status, broke.note, broke.findings],
      ["error", "canary key needs credit", []],
    );
  });

  it("a crash in the script is an error with only the error's name, never its message", async () => {
    const r = await runLeg({
      provider: "openai",
      configured,
      run: async () => {
        throw new TypeError("secret-ish detail");
      },
      env,
      deps: deps(server()),
    });
    assert.deepEqual([r.status, r.note], ["error", "the script crashed (TypeError)"]);
  });
});
