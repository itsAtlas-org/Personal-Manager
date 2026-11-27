// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The keyless mode of the shared HTTP helper (the drift check reads public pages): redirects followed
// and the final address returned, and none of the key vocabulary applies. The keyed mode (the canary) is
// pinned by http.test.mjs and must not change.

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { Inconclusive, KeyRejected, NeedsCredit, RateLimited, send } from "./lib/http.mjs";

const NO_SLEEP = { sleep: async () => {} };
const reply = (status, text = "", url = "") => ({
  status,
  url,
  headers: new Headers(),
  text: async () => text,
});
const fetcher = (answers) => {
  const calls = [];
  const f = async (url, init) => {
    calls.push({ url, init });
    return answers[Math.min(calls.length - 1, answers.length - 1)];
  };
  f.calls = calls;
  return f;
};

describe("send({ keyless: true })", () => {
  it("passes the redirect mode through and returns the address the request ended on", async () => {
    const f = fetcher([reply(200, "ok", "https://b.example/final")]);
    const r = await send(
      { url: "https://a.example/start", redirect: "follow", keyless: true },
      { fetch: f, ...NO_SLEEP },
    );
    assert.equal(f.calls[0].init.redirect, "follow");
    assert.equal(r.url, "https://b.example/final");
    assert.equal(r.text, "ok");
  });

  it("a keyed request still refuses redirects, as before", async () => {
    const f = fetcher([reply(200)]);
    await send({ url: "https://a.example/x" }, { fetch: f, ...NO_SLEEP });
    assert.equal(f.calls[0].init.redirect, "error");
  });

  it("a page that talks about spend and usage limits is a page, not 'the key needs credit'", async () => {
    const prose =
      "Set a spend limit. If you reach your usage limit, billing_hard_limit applies. Insufficient_quota.";
    const r = await send(
      { url: "https://a.example/pricing", keyless: true },
      { fetch: fetcher([reply(200, prose)]), ...NO_SLEEP },
    );
    assert.equal(r.status, 200);
    // The keyed mode still reads the same words as money.
    await assert.rejects(
      send({ url: "https://a.example/x" }, { fetch: fetcher([reply(429, prose)]), ...NO_SLEEP }),
      NeedsCredit,
    );
  });

  it("a bot-wall or error page that SAYS 'usage limit' on a 403, 429 or 400 is never 'the key needs credit'", async () => {
    // The billing words only matter on the statuses isBilling reads (400, 403, 429), so 200 proves
    // nothing: a keyless fetch must read these as a page or as busy, and the keyed mode as money.
    const prose = "You have reached your usage limit. Spend limit applies. billing_hard_limit.";
    const blocked = await send(
      { url: "https://a.example/x", keyless: true },
      { fetch: fetcher([reply(400, prose)]), ...NO_SLEEP },
    );
    assert.equal(blocked.status, 400, "a 400 page is an answer the caller judges");
    for (const status of [403, 429]) {
      await assert.rejects(
        send(
          { url: "https://a.example/x", keyless: true },
          { fetch: fetcher([reply(status, prose)]), ...NO_SLEEP },
        ),
        Inconclusive,
        `HTTP ${status} with money words is busy, not a credit problem`,
      );
    }
    for (const status of [400, 403, 429])
      await assert.rejects(
        send(
          { url: "https://a.example/x" },
          { fetch: fetcher([reply(status, prose)]), ...NO_SLEEP },
        ),
        NeedsCredit,
        `the keyed mode still reads HTTP ${status} with these words as money`,
      );
  });

  it("a 403 or 401 is a busy or blocking page: retried, then inconclusive, never a rejected key", async () => {
    for (const status of [401, 403]) {
      const f = fetcher([reply(status)]);
      await assert.rejects(
        send({ url: "https://a.example/x", keyless: true }, { fetch: f, ...NO_SLEEP }),
        Inconclusive,
      );
      assert.equal(f.calls.length, 4);
      await assert.rejects(
        send({ url: "https://a.example/x" }, { fetch: fetcher([reply(status)]), ...NO_SLEEP }),
        KeyRejected,
      );
    }
  });

  it("a 429 on every try is inconclusive for a page, and RateLimited only for a key", async () => {
    await assert.rejects(
      send(
        { url: "https://a.example/x", keyless: true },
        { fetch: fetcher([reply(429)]), ...NO_SLEEP },
      ),
      Inconclusive,
    );
    await assert.rejects(
      send({ url: "https://a.example/x" }, { fetch: fetcher([reply(429)]), ...NO_SLEEP }),
      RateLimited,
    );
  });

  it("a 404 is an answer the caller judges, and a recovered 5xx is retried into an answer", async () => {
    const gone = await send(
      { url: "https://a.example/x", keyless: true },
      { fetch: fetcher([reply(404, "no")]), ...NO_SLEEP },
    );
    assert.equal(gone.status, 404);
    const f = fetcher([reply(503), reply(200, "back")]);
    const r = await send({ url: "https://a.example/x", keyless: true }, { fetch: f, ...NO_SLEEP });
    assert.equal(r.text, "back");
    assert.equal(f.calls.length, 2);
  });
});
