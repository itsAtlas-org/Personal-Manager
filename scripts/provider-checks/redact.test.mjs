// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { looksSecret, redact } from "./lib/redact.mjs";

// Built at run time so no secret-shaped literal is committed (gitleaks reads this file too).
const x = (n) => "a1B2c3D4e5".repeat(Math.ceil(n / 10)).slice(0, n);
const ant = "sk-ant-" + x(24);
const oai = "sk-" + "proj-" + x(30);
const goog = "AIza" + x(30);
const jwt = "eyJ" + x(20) + "." + x(20) + "." + x(20);

describe("redact", () => {
  it("removes each vendor's key shape, a JWT and a bearer value", () => {
    for (const secret of [ant, oai, goog, jwt]) {
      const out = redact(`before ${secret} after`);
      assert.ok(!out.includes(secret), secret.slice(0, 6));
      assert.match(out, /before \[redacted\] after/);
    }
    assert.equal(redact("authorization: Bearer abcdef0123456789"), "authorization: [redacted]");
  });

  it("removes the id shapes of a federation rule and of a request, and a UUID", () => {
    for (const s of [
      "fdrl_" + x(12),
      "svac_" + x(12),
      "wrkspc_" + x(12),
      "req_" + x(14),
      "msg_" + x(14),
      "0b9f2c1e-1234-4abc-8def-0123456789ab",
    ]) {
      assert.equal(redact(s), "[redacted]", s);
    }
  });

  it("removes the literal secrets it is given, even with no recognisable shape", () => {
    assert.equal(
      redact("the value is hunter2hunter2 ok", ["hunter2hunter2"]),
      "the value is [redacted] ok",
    );
    // Too short to be safe to replace: ordinary words must survive.
    assert.equal(redact("an ok line", ["ok"]), "an ok line");
  });

  it("leaves ordinary check output alone", () => {
    const line =
      "leg=openai step=models http=200 ok=true gpt-6-luna claude-haiku-5-5 gemini-3.5-flash-lite";
    assert.equal(redact(line), line);
    assert.equal(looksSecret(line), false);
    assert.equal(looksSecret(ant), true);
  });
});
