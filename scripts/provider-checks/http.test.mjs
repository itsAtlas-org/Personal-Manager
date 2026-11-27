// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  Inconclusive,
  KeyRejected,
  NeedsCredit,
  RateLimited,
  isBilling,
  send,
} from "./lib/http.mjs";
import { fakeFetch } from "./testkit.mjs";

const noSleep = async () => {};
const req = {
  url: "https://api.example.test/v1/x",
  headers: { authorization: "Bearer not-a-real-key" },
};

describe("send", () => {
  it("returns a 200 with its parsed JSON", async () => {
    const f = fakeFetch(() => ({ body: { a: 1 } }));
    const r = await send(req, { fetch: f, sleep: noSleep });
    assert.equal(r.status, 200);
    assert.deepEqual(r.json, { a: 1 });
    assert.equal(f.calls.length, 1);
  });

  it("retries a 503 and a network error, then answers", async () => {
    const f = fakeFetch((u, i, n) => {
      if (n === 0) throw new TypeError("fetch failed");
      if (n === 1) return { status: 503, body: { error: "high demand" } };
      return { body: { ok: true } };
    });
    const r = await send(req, { fetch: f, sleep: noSleep });
    assert.equal(r.json.ok, true);
    assert.equal(f.calls.length, 3);
  });

  it("ends INCONCLUSIVE after three retries of 503 / 500 / a timeout, never as drift", async () => {
    for (const status of [503, 500]) {
      const f = fakeFetch(() => ({ status, body: { error: "busy" } }));
      await assert.rejects(send(req, { fetch: f, sleep: noSleep }), Inconclusive);
      assert.equal(f.calls.length, 4, `${status}: one try and three retries`);
    }
    const slow = fakeFetch(() => {
      const e = new Error("t");
      e.name = "TimeoutError";
      throw e;
    });
    await assert.rejects(send(req, { fetch: slow, sleep: noSleep }), /timed out/);
  });

  it("a 429 on every try is a quota problem (an error), a 429 among 5xx stays inconclusive", async () => {
    const f = fakeFetch(() => ({ status: 429, body: { error: { message: "Too many requests" } } }));
    await assert.rejects(send(req, { fetch: f, sleep: noSleep }), RateLimited);
    assert.equal(f.calls.length, 4);
    const mixed = fakeFetch((_u, _i, n) => ({ status: n === 0 ? 429 : 503 }));
    await assert.rejects(send(req, { fetch: mixed, sleep: noSleep }), Inconclusive);
  });

  it("backs off between tries", async () => {
    const waits = [];
    const f = fakeFetch(() => ({ status: 503 }));
    await assert.rejects(
      send({ ...req, backoff: [1, 2, 3] }, { fetch: f, sleep: async (ms) => waits.push(ms) }),
      Inconclusive,
    );
    assert.deepEqual(waits, [1, 2, 3]);
  });

  it("treats a money 4xx as the canary key needing credit, not drift, and never retries it", async () => {
    for (const [status, body] of [
      [402, {}],
      [429, { error: { code: "insufficient_quota", type: "insufficient_quota" } }],
      [429, { error: { code: "project_spend_limit_exceeded" } }],
      [
        400,
        {
          type: "error",
          error: {
            type: "invalid_request_error",
            details: { error_code: "enforced_spend_limit_reached" },
          },
        },
      ],
      [400, { error: { message: "Your credit balance is too low to access the API." } }],
      [
        400,
        {
          type: "error",
          error: {
            type: "invalid_request_error",
            message: "You have reached your specified workspace API usage limits.",
          },
        },
      ],
      [400, { error: { message: "You have reached your specified API usage limits" } }],
      [429, { error: { message: "Your project has exceeded its monthly spending cap." } }],
    ]) {
      const f = fakeFetch(() => ({ status, body }));
      await assert.rejects(send(req, { fetch: f, sleep: noSleep }), NeedsCredit, `${status}`);
      assert.equal(f.calls.length, 1);
    }
    // even when the caller expects that status
    const f = fakeFetch(() => ({
      status: 400,
      body: { error: { message: "credit balance is too low" } },
    }));
    await assert.rejects(
      send({ ...req, expect: [400] }, { fetch: f, sleep: noSleep }),
      NeedsCredit,
    );
  });

  it("a rate limit that is not about money is retried, a free-tier 429 can be expected", async () => {
    assert.equal(isBilling(429, '{"error":{"message":"Too many requests"}}'), false);
    const f = fakeFetch(() => ({
      status: 429,
      body: { error: { status: "RESOURCE_EXHAUSTED", message: "free_tier limit: 0" } },
    }));
    const r = await send({ ...req, expect: [429] }, { fetch: f, sleep: noSleep });
    assert.equal(r.status, 429);
    assert.equal(f.calls.length, 1);
  });

  it("a refused credential is KeyRejected unless the caller expected the status", async () => {
    const f = fakeFetch(() => ({ status: 401, body: { error: "no" } }));
    await assert.rejects(send(req, { fetch: f, sleep: noSleep }), KeyRejected);
    const g = fakeFetch(() => ({ status: 404, body: { error: "gone" } }));
    assert.equal((await send({ ...req, expect: [404] }, { fetch: g, sleep: noSleep })).status, 404);
    const h = fakeFetch(() => ({ status: 404, body: {} }));
    assert.equal(
      (await send(req, { fetch: h, sleep: noSleep })).status,
      404,
      "an unexpected 4xx is still returned for the caller to judge",
    );
  });

  it("never follows a redirect (a credential header must not leave the host)", async () => {
    const f = fakeFetch(() => ({ body: {} }));
    await send(req, { fetch: f, sleep: noSleep });
    assert.equal(f.calls[0].init.redirect, "error");
  });

  it("the error messages carry no URL, header or body", async () => {
    const f = fakeFetch(() => ({ status: 503, body: "secret-ish body text" }));
    try {
      await send(req, { fetch: f, sleep: noSleep });
      assert.fail("should reject");
    } catch (e) {
      assert.ok(
        !e.message.includes("example.test") &&
          !e.message.includes("secret-ish") &&
          !e.message.includes("not-a-real-key"),
      );
    }
  });
});
