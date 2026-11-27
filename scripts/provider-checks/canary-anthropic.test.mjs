// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Anthropic leg against recorded replies (see testkit.mjs) through a fake fetch, the Workload
// Identity Federation exchange against a fake GitHub token service and token endpoint, and the stored
// key plan B. Offline: no key, no network, no token that exists.

import { afterEach, describe, it } from "node:test";
import assert from "node:assert/strict";
import { runLeg } from "./lib/leg.mjs";
import { setSink } from "./lib/log.mjs";
import { configured, hasFederation, run } from "./canary-anthropic.mjs";
import { deps, fakeFetch, fx } from "./testkit.mjs";

const out = [];
setSink((l) => out.push(l));
afterEach(() => (out.length = 0));

const x = (n) => "a1B2c3D4e5".repeat(Math.ceil(n / 10)).slice(0, n);
const JWT = "eyJ" + x(30) + "." + x(30) + "." + x(30);
const ACCESS = "sk-ant-oat01-" + x(40);
const KEY = "sk-ant-test-" + x(30);
const wif = {
  ANTHROPIC_ORG_ID: "0b9f2c1e-1234-4abc-8def-0123456789ab",
  ANTHROPIC_FEDERATION_RULE_ID: "fdrl_" + x(12),
  ANTHROPIC_SERVICE_ACCOUNT_ID: "svac_" + x(12),
  ANTHROPIC_WORKSPACE_ID: "wrkspc_" + x(12),
  ACTIONS_ID_TOKEN_REQUEST_URL:
    "https://pipelines.actions.githubusercontent.com/abc/oidc?api-version=2.0",
  ACTIONS_ID_TOKEN_REQUEST_TOKEN: "runner-request-token-" + x(10),
};
const same = (v) => v;

/** The recorded stream with its message_delta event edited by `fn(deltaEvent)`. */
function editDelta(text, fn) {
  return text
    .split("\n")
    .map((line) => {
      if (!line.startsWith("data:") || !line.includes('"type":"message_delta"')) return line;
      const ev = JSON.parse(line.slice(5));
      fn(ev);
      return "data: " + JSON.stringify(ev);
    })
    .join("\n");
}

/** The usage a cache pair reports: a write on the first call, a read on the second. */
function cacheStream(kind) {
  if (kind === "error")
    return 'event: error\ndata: {"type":"error","error":{"type":"overloaded_error"}}\n\n';
  if (kind === "hour")
    // As on the real wire: `cache_creation` rides in message_start's usage, not in message_delta's.
    return cacheStream("write")
      .split("\n")
      .map((line) => {
        if (!line.startsWith("data:") || !line.includes('"type":"message_start"')) return line;
        const ev = JSON.parse(line.slice(5));
        ev.message.usage.cache_creation = {
          ephemeral_5m_input_tokens: 0,
          ephemeral_1h_input_tokens: 1383,
        };
        return "data: " + JSON.stringify(ev);
      })
      .join("\n");
  const [creation, read] = kind === "write" ? [1383, 0] : kind === "read" ? [0, 1383] : [0, 0];
  return editDelta(fx("anthropic-stream.txt"), (ev) => {
    ev.usage.cache_creation_input_tokens = creation;
    ev.usage.cache_read_input_tokens = read;
  });
}

/** A fake Anthropic (and, for federation, a fake GitHub token service and token endpoint). */
function server({
  edit = {},
  cache = ["write", "read"],
  exchange = { status: 200, body: { access_token: ACCESS, expires_in: 600, token_type: "Bearer" } },
} = {}) {
  let cacheCalls = 0;
  let jwtCalls = 0;
  const f = fakeFetch((url, init) => {
    const u = new URL(url);
    const body = init.body ? JSON.parse(init.body) : null;
    // The exact host the fake OIDC URL above names, never a suffix match (the same rule leg.mjs
    // applies to the real request).
    if (u.hostname === "pipelines.actions.githubusercontent.com") {
      jwtCalls += 1;
      return { body: { value: JWT + jwtCalls } };
    }
    if (u.pathname === "/v1/oauth/token")
      return typeof exchange === "function" ? exchange(body) : exchange;
    if (u.pathname === "/v1/models")
      return { body: (edit.models ?? same)(fx("anthropic-models.json")) };
    if (body.model === "pm-canary-no-such-model")
      return (edit.unknown ?? same)({ status: 404, body: fx("anthropic-404.json") });
    if (body.temperature !== undefined)
      return (edit.temp ?? same)({ status: 400, body: fx("anthropic-400-temperature.json") });
    if (body.stream) {
      if (body.system?.[0]?.text.startsWith("Reference sheet"))
        return { body: cacheStream(cache[Math.min(cacheCalls++, cache.length - 1)]) };
      return { body: (edit.stream ?? same)(fx("anthropic-stream.txt")) };
    }
    return (edit.message ?? same)({
      status: 200,
      body: structuredClone(fx("anthropic-message.json")),
    });
  });
  f.jwtCalls = () => jwtCalls;
  return f;
}
const go = (f, e) => runLeg({ provider: "anthropic", configured, run, env: e, deps: deps(f) });
const ids = (r) => r.findings.map((f) => f.id);

describe("Anthropic leg, with the stored key (plan B)", () => {
  it("is clean on the recorded replies, with x-api-key and the pinned version, and Fable is never probed", async () => {
    const f = server();
    const r = await go(f, { ANTHROPIC_CANARY_KEY: KEY });
    assert.equal(r.status, "ok");
    assert.deepEqual(r.findings, []);
    assert.deepEqual(r.notices, [{ id: "fallback-key-used", provider: "anthropic" }]);
    for (const c of f.calls) {
      assert.equal(new URL(c.url).origin, "https://api.anthropic.com");
      assert.equal(c.init.headers["x-api-key"], KEY);
      assert.equal(c.init.headers["anthropic-version"], "2026-10-21");
      assert.ok(!c.init.headers.authorization);
      assert.ok(!(c.body?.model ?? "").includes("fable"));
    }
    const sent = f.calls.find(
      (c) => c.body?.stream && !c.body.system[0].text.startsWith("Reference"),
    );
    assert.equal(sent.body.model, "claude-haiku-5-5");
    assert.deepEqual(sent.body.thinking, { type: "disabled" });
    assert.equal(sent.body.system[0].cache_control.type, "ephemeral");
    assert.ok(!("temperature" in f.calls.find((c) => c.url.endsWith("/v1/messages")).body));
  });

  it("flags a model gone, one not active, a changed retirement, window, output cap, effort level or image input", async () => {
    const edit = (fn) => ({ models: (m) => ({ ...m, data: fn(m.data) }) });
    const only = (r, id) => r.findings.filter((f) => f.id === id);
    assert.deepEqual(
      ids(
        await go(server({ edit: edit((d) => d.filter((m) => m.id !== "claude-opus-5-5")) }), {
          ANTHROPIC_CANARY_KEY: KEY,
        }),
      ),
      ["model-missing"],
    );
    const mod = (id, patch) =>
      edit((d) => d.map((m) => (m.id === id ? patch(structuredClone(m)) : m)));
    let r = await go(
      server({ edit: mod("claude-haiku-5-5", (m) => ({ ...m, lifecycle: "deprecated" })) }),
      { ANTHROPIC_CANARY_KEY: KEY },
    );
    assert.deepEqual(
      r.findings.map((f) => [f.id, f.fields.field]),
      [["model-capability-changed", "lifecycle"]],
    );
    r = await go(
      server({
        edit: mod("claude-haiku-5-5", (m) => ({ ...m, retires_at: "2027-06-01T00:00:00Z" })),
      }),
      { ANTHROPIC_CANARY_KEY: KEY },
    );
    assert.deepEqual(only(r, "model-retirement-changed")[0].fields, {
      model: "claude-haiku-5-5",
      listed: "2027-06-01",
      catalogue: "none",
    });
    r = await go(
      server({ edit: mod("claude-sonnet-5-5", (m) => ({ ...m, max_input_tokens: 200000 })) }),
      { ANTHROPIC_CANARY_KEY: KEY },
    );
    assert.deepEqual(
      r.findings.map((f) => [f.id, f.fields.field, f.fields.catalogue]),
      [["model-limits-changed", "max_input_tokens", "1000000"]],
    );
    r = await go(server({ edit: mod("claude-sonnet-5-5", (m) => ({ ...m, max_tokens: 64000 })) }), {
      ANTHROPIC_CANARY_KEY: KEY,
    });
    assert.deepEqual(
      r.findings.map((f) => f.fields.field),
      ["max_tokens"],
    );
    r = await go(
      server({
        edit: mod("claude-opus-5-5", (m) => {
          m.capabilities.effort.max.supported = false;
          return m;
        }),
      }),
      { ANTHROPIC_CANARY_KEY: KEY },
    );
    assert.deepEqual(
      r.findings.map((f) => [f.fields.field, f.fields.listed]),
      [["effort", "low,medium,high,xhigh"]],
    );
    r = await go(
      server({
        edit: mod("claude-opus-5-5", (m) => {
          m.capabilities.image_input.supported = false;
          return m;
        }),
      }),
      { ANTHROPIC_CANARY_KEY: KEY },
    );
    assert.deepEqual(
      r.findings.map((f) => f.fields.field),
      ["image_input"],
    );
  });

  it("flags a new token type, the thinking count going missing, and a changed stop reason", async () => {
    let r = await go(
      server({
        edit: {
          message: (m) => {
            m.body.usage.web_search_requests_total = 2;
            return m;
          },
        },
      }),
      { ANTHROPIC_CANARY_KEY: KEY },
    );
    assert.deepEqual(
      r.findings.map((f) => [f.id, f.fields.field]),
      [["new-token-type", "web_search_requests_total"]],
    );
    r = await go(
      server({
        edit: {
          message: (m) => {
            delete m.body.usage.output_tokens_details;
            return m;
          },
        },
      }),
      { ANTHROPIC_CANARY_KEY: KEY },
    );
    assert.deepEqual(
      r.findings.map((f) => [f.id, f.fields.field]),
      [["usage-field-missing", "output_tokens_details.thinking_tokens"]],
    );
    r = await go(
      server({
        edit: {
          message: (m) => {
            m.body.stop_reason = "pause_turn";
            return m;
          },
        },
      }),
      { ANTHROPIC_CANARY_KEY: KEY },
    );
    assert.deepEqual(ids(r), ["stop-reason-unknown"]);
  });

  it("flags the one-hour cache write PM cannot price, a server tool use, and a lost 1h field", async () => {
    let r = await go(
      server({
        edit: {
          message: (m) => {
            m.body.usage.cache_creation.ephemeral_1h_input_tokens = 500;
            return m;
          },
        },
      }),
      { ANTHROPIC_CANARY_KEY: KEY },
    );
    assert.deepEqual(
      r.findings.map((f) => [f.id, f.fields.field, f.fields.value]),
      [["unpriced-token-seen", "cache_creation.ephemeral_1h_input_tokens", 500]],
    );
    r = await go(
      server({
        edit: {
          message: (m) => {
            delete m.body.usage.cache_creation;
            return m;
          },
        },
      }),
      { ANTHROPIC_CANARY_KEY: KEY },
    );
    assert.deepEqual(
      r.findings.map((f) => f.fields.field),
      ["cache_creation.ephemeral_1h_input_tokens"],
    );
  });

  it("final usage comes from message_delta: a stream whose delta lost its usage is a finding", async () => {
    const noDelta = (t) => editDelta(t, (ev) => delete ev.usage);
    const r = await go(server({ edit: { stream: noDelta } }), { ANTHROPIC_CANARY_KEY: KEY });
    assert.ok(ids(r).includes("stream-usage-missing"), ids(r).join());
  });

  it("a cache that writes then reads is clean; one that never reads is a finding; no first write is a notice", async () => {
    let r = await go(server({ cache: ["write", "none", "none", "none"] }), {
      ANTHROPIC_CANARY_KEY: KEY,
    });
    assert.deepEqual(ids(r), ["cache-never-hit"]);
    r = await go(server({ cache: ["none", "read"] }), { ANTHROPIC_CANARY_KEY: KEY });
    assert.deepEqual(r.findings, []);
    assert.ok(r.notices.some((n) => n.id === "cache-first-write-not-seen"));
  });

  it("walks the first cache reply: a one-hour write there is the cost drift", async () => {
    const r = await go(server({ cache: ["hour", "read"] }), { ANTHROPIC_CANARY_KEY: KEY });
    assert.deepEqual(
      r.findings.map((f) => [f.id, f.fields.step, f.fields.field, f.fields.value]),
      [["unpriced-token-seen", "cache", "cache_creation.ephemeral_1h_input_tokens", 1383]],
    );
  });

  it("an error event inside a 200 stream (overloaded mid-stream) is INCONCLUSIVE, never a changed stream", async () => {
    const overloaded =
      'event: error\ndata: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}\n\n';
    for (const server_ of [
      server({ edit: { stream: () => overloaded } }),
      server({ cache: ["error"] }),
    ]) {
      const r = await go(server_, { ANTHROPIC_CANARY_KEY: KEY });
      assert.deepEqual([r.status, r.findings], ["inconclusive", []]);
    }
  });

  it("flags the error envelope changing, and notes (no finding) that temperature is now accepted", async () => {
    let r = await go(
      server({ edit: { unknown: () => ({ status: 404, body: { error: "gone" } }) } }),
      { ANTHROPIC_CANARY_KEY: KEY },
    );
    assert.deepEqual(ids(r), ["error-envelope-changed"]);
    r = await go(
      server({ edit: { temp: () => ({ status: 200, body: fx("anthropic-message.json") }) } }),
      { ANTHROPIC_CANARY_KEY: KEY },
    );
    assert.deepEqual(r.findings, []);
    assert.ok(r.notices.some((n) => n.id === "temperature-accepted"));
  });
});

describe("Anthropic leg, with Workload Identity Federation", () => {
  it("fetches a fresh OIDC token, masks it and the access token at once, and sends Bearer, not x-api-key", async () => {
    const f = server();
    const r = await go(f, wif);
    assert.equal(r.status, "ok");
    assert.deepEqual(r.findings, []);
    assert.deepEqual(r.notices, [{ id: "federation-used", provider: "anthropic" }]);
    const oidc = f.calls[0];
    assert.ok(oidc.url.startsWith("https://pipelines.actions.githubusercontent.com/"));
    assert.ok(oidc.url.endsWith("&audience=https%3A%2F%2Fapi.anthropic.com"));
    assert.equal(oidc.init.headers.authorization, `Bearer ${wif.ACTIONS_ID_TOKEN_REQUEST_TOKEN}`);
    const exch = f.calls[1];
    assert.equal(new URL(exch.url).pathname, "/v1/oauth/token");
    assert.deepEqual(Object.keys(exch.body).sort(), [
      "assertion",
      "federation_rule_id",
      "grant_type",
      "organization_id",
      "service_account_id",
      "workspace_id",
    ]);
    assert.equal(exch.body.grant_type, "urn:ietf:params:oauth:grant-type:jwt-bearer");
    assert.equal(exch.body.assertion, JWT + 1);
    for (const c of f.calls.slice(2)) {
      assert.equal(c.init.headers.authorization, `Bearer ${ACCESS}`);
      assert.ok(!c.init.headers["x-api-key"]);
    }
    // masks come BEFORE anything else could print them
    const masks = out.filter((l) => l.startsWith("::add-mask::"));
    assert.deepEqual(masks, ["::add-mask::" + JWT + 1, "::add-mask::" + ACCESS]);
    assert.ok(out.indexOf(masks[0]) < out.findIndex((l) => l.includes("wif_exchange")));
    // and nothing printed carries a token, an id or the org
    for (const line of out.filter((l) => !l.startsWith("::add-mask::"))) {
      for (const secret of [JWT, ACCESS, ...Object.values(wif)])
        assert.ok(!line.includes(secret), line);
    }
  });

  it("an exchange needs all four ids and the runner's token variables, else it is not attempted", async () => {
    assert.equal(hasFederation(wif), true);
    assert.equal(hasFederation({ ...wif, ANTHROPIC_WORKSPACE_ID: "" }), false);
    const f = server();
    const r = await go(f, {
      ANTHROPIC_CANARY_KEY: KEY,
      ...wif,
      ACTIONS_ID_TOKEN_REQUEST_URL: "",
      ACTIONS_ID_TOKEN_REQUEST_TOKEN: "",
    });
    assert.deepEqual(r.notices, [{ id: "fallback-key-used", provider: "anthropic" }]);
    assert.ok(f.calls.every((c) => !c.url.includes("githubusercontent")));
    assert.equal((await go(server(), {})).status, "skipped");
    // a federation set started and not finished, with no stored key, is an ERROR (not a silent skip)
    const partial = await go(server(), { ANTHROPIC_ORG_ID: wif.ANTHROPIC_ORG_ID });
    assert.deepEqual(
      [partial.status, partial.findings],
      ["error", []],
      "partly configured is a setup error",
    );
    assert.match(partial.note, /partly configured/);
    // ... but with a stored key the leg uses the key
    const withKey = await go(server(), {
      ANTHROPIC_ORG_ID: wif.ANTHROPIC_ORG_ID,
      ANTHROPIC_CANARY_KEY: KEY,
    });
    assert.equal(withKey.status, "ok");
  });

  it("a DENIED exchange (the opaque 401) falls back to the stored key, else is an ERROR that says where to look and nothing else", async () => {
    const denied = {
      status: 401,
      body: {
        type: "error",
        error: { type: "authentication_error", message: "Authentication failed" },
      },
    };
    const f = server({ exchange: denied });
    const r = await go(f, { ...wif, ANTHROPIC_CANARY_KEY: KEY });
    assert.equal(r.status, "ok");
    assert.deepEqual(r.notices, [{ id: "fallback-key-used", provider: "anthropic" }]);
    const none = await go(server({ exchange: denied }), wif);
    assert.deepEqual(
      [none.status, none.note, none.findings],
      ["error", "Anthropic WIF exchange denied (see Console > Workload identity > history)", []],
    );
    assert.ok(
      !out.some((l) => l.includes("Authentication failed")),
      "nothing from the denied response is printed",
    );
  });

  it("a 5xx on the exchange retries with a FRESH token (never the used one), then is inconclusive", async () => {
    let n = 0;
    const f = server({
      exchange: () =>
        ++n === 1 ? { status: 503, body: {} } : { status: 200, body: { access_token: ACCESS } },
    });
    const ok = await go(f, wif);
    assert.equal(ok.status, "ok");
    assert.equal(f.jwtCalls(), 2);
    const assertions = f.calls
      .filter((c) => c.url.endsWith("/oauth/token"))
      .map((c) => c.body.assertion);
    assert.equal(new Set(assertions).size, 2, "single-use tokens are never replayed");
    const down = server({ exchange: { status: 503, body: {} } });
    assert.equal((await go(down, wif)).status, "inconclusive");
    assert.equal(down.jwtCalls(), 2);
  });

  it("GitHub giving no OIDC token is an error (the job lacks id-token: write)", async () => {
    const f = fakeFetch(() => ({ status: 403, body: { message: "no" } }));
    const r = await go(f, wif);
    assert.equal(r.status, "error");
    assert.deepEqual(r.findings, []);
  });

  it("the OIDC request may only go to a githubusercontent.com host", async () => {
    const f = server();
    const r = await go(f, {
      ...wif,
      ACTIONS_ID_TOKEN_REQUEST_URL: "https://evil.example.test/oidc?x=1",
    });
    assert.equal(r.status, "error");
    assert.equal(f.calls.length, 0, "the bearer never left");
  });
});
